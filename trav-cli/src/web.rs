//! Headless web UI + JSON API (`trav --daemon`).
//!
//! * `POST /api/rpc {method, params}` — same surface the desktop app uses.
//! * `GET  /api/events` — server-sent engine events.
//! * everything else — the embedded Nova UI (static export of `trav-gui`).
//!
//! Without `--token` the server only answers requests whose `Host` is a
//! loopback name, which blocks DNS-rebinding attacks from web pages.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::Stream;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use trav_core::EngineHandle;

#[derive(rust_embed::RustEmbed)]
#[folder = "../trav-gui/out"]
#[allow_missing = true]
struct Ui;

#[derive(Clone)]
struct AppState {
    engine: EngineHandle,
    token: Option<Arc<str>>,
}

#[derive(Deserialize)]
struct RpcCall {
    method: String,
    #[serde(default)]
    params: Value,
}

pub async fn serve(
    engine: EngineHandle,
    bind: SocketAddr,
    token: Option<String>,
) -> anyhow::Result<()> {
    if token.is_none() && !bind.ip().is_loopback() {
        anyhow::bail!("refusing to expose the web UI on {bind} without --token");
    }
    let state = AppState {
        engine,
        token: token.map(Arc::from),
    };
    let app = Router::new()
        .route("/api/rpc", post(rpc))
        .route("/api/events", get(events))
        .route("/api/health", get(|| async { "ok" }))
        .fallback(get(static_file))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("web UI on http://{bind}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn loopback_host(host: &str) -> bool {
    let h = host.rsplit_once(':').map_or(host, |(h, p)| {
        if p.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            host
        }
    });
    matches!(h, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

fn presented_token(req: &Request) -> Option<String> {
    let h = req.headers();
    if let Some(v) = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok())
        && let Some(t) = v.strip_prefix("Bearer ")
    {
        return Some(t.to_string());
    }
    if let Some(v) = h.get("x-trav-token").and_then(|v| v.to_str().ok()) {
        return Some(v.to_string());
    }
    req.uri()
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")))
        .map(|t| t.to_string())
}

/// Auth + CORS for local dev servers (`next dev` on another port).
async fn guard(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let origin = req.headers().get(header::ORIGIN).cloned();
    let local_origin = origin
        .as_ref()
        .and_then(|o| o.to_str().ok())
        .is_some_and(|o| o.starts_with("http://localhost:") || o.starts_with("http://127.0.0.1:"));

    if req.method() == Method::OPTIONS && local_origin {
        return cors(StatusCode::NO_CONTENT.into_response(), origin);
    }

    let host_ok = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(loopback_host);
    let is_api = req.uri().path().starts_with("/api/");
    match &st.token {
        None if !host_ok => return (StatusCode::FORBIDDEN, "forbidden host").into_response(),
        Some(t)
            if is_api
                && req.uri().path() != "/api/health"
                && presented_token(&req).as_deref() != Some(&**t) =>
        {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "ok": false, "error": "unauthorized" })),
            )
                .into_response();
        }
        _ => {}
    }
    let res = next.run(req).await;
    if local_origin { cors(res, origin) } else { res }
}

fn cors(mut res: Response, origin: Option<HeaderValue>) -> Response {
    let h = res.headers_mut();
    if let Some(o) = origin {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o);
    }
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type, authorization, x-trav-token"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    res
}

async fn rpc(State(st): State<AppState>, Json(call): Json<RpcCall>) -> Response {
    match trav_core::rpc::dispatch(&st.engine, &call.method, call.params).await {
        Ok(result) => Json(json!({ "ok": true, "result": result })).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": error })),
        )
            .into_response(),
    }
}

async fn events(
    State(st): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let stream = BroadcastStream::new(st.engine.events())
        .filter_map(|e| e.ok())
        .map(|e| Ok(SseEvent::default().json_data(e).unwrap_or_default()));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let candidates = [
        if path.is_empty() {
            "index.html".to_string()
        } else {
            path.to_string()
        },
        format!("{path}.html"),
        format!("{}/index.html", path.trim_end_matches('/')),
    ];
    for c in candidates {
        if let Some(f) = Ui::get(&c) {
            let mime = mime_guess::from_path(&c).first_or_octet_stream();
            let cache = if c.starts_with("_next/static/") {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            return Response::builder()
                .header(header::CONTENT_TYPE, mime.as_ref())
                .header(header::CACHE_CONTROL, cache)
                .body(Body::from(f.data.into_owned()))
                .unwrap();
        }
    }
    match Ui::get("index.html") {
        Some(f) => Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(f.data.into_owned()))
            .unwrap(),
        None => (
            StatusCode::NOT_FOUND,
            "Web UI not built. Run `npm run build` in trav-gui, then rebuild trav (the API is live at /api/rpc).",
        )
            .into_response(),
    }
}
