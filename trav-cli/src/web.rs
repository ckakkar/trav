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
    let app = router(engine, token);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("web UI on http://{bind}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Largest RPC body accepted: base64 of a very large `.torrent` (multi-GB, many files).
const MAX_BODY: usize = 64 * 1024 * 1024;

/// The full HTTP surface, separated from socket binding so it can be tested in-process.
pub fn router(engine: EngineHandle, token: Option<String>) -> Router {
    let state = AppState {
        engine,
        token: token.map(Arc::from),
    };
    Router::new()
        .route("/api/rpc", post(rpc))
        .route("/api/events", get(events))
        .route("/api/health", get(|| async { "ok" }))
        .fallback(get(static_file))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// Compare secrets without leaking the matching prefix length through timing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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

    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    let host_ok = loopback_host(&host);
    // CSRF: a browser request from another site carries a foreign Origin. Only
    // same-origin and local dev servers may drive the API.
    let origin_ok = match origin.as_ref().and_then(|o| o.to_str().ok()) {
        None => true,
        Some(o) => local_origin || o == format!("http://{host}") || o == format!("https://{host}"),
    };
    let is_api = req.uri().path().starts_with("/api/");
    if is_api && !origin_ok {
        return (StatusCode::FORBIDDEN, "cross-origin request refused").into_response();
    }
    match &st.token {
        None if !host_ok => return (StatusCode::FORBIDDEN, "forbidden host").into_response(),
        Some(t)
            if is_api
                && req.uri().path() != "/api/health"
                && !presented_token(&req).is_some_and(|p| ct_eq(p.as_bytes(), t.as_bytes())) =>
        {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "ok": false, "error": "unauthorized" })),
            )
                .into_response();
        }
        _ => {}
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;
    use trav_core::{Engine, Settings};

    async fn engine(dir: &std::path::Path) -> EngineHandle {
        let s = Settings {
            download_dir: dir.join("dl"),
            listen_port: 0,
            enable_dht: false,
            enable_upnp: false,
            ..Settings::default()
        };
        Engine::start_with(dir.join("state"), Some(s))
            .await
            .unwrap()
    }

    fn rpc_req(host: &str, body: &str) -> HttpRequest<Body> {
        HttpRequest::post("/api/rpc")
            .header(header::HOST, host)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn json(res: Response) -> Value {
        serde_json::from_slice(&to_bytes(res.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn rpc_roundtrip_on_loopback() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(engine(tmp.path()).await, None);
        let res = app
            .clone()
            .oneshot(rpc_req("127.0.0.1:9696", r#"{"method":"snapshot"}"#))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::X_FRAME_OPTIONS], "DENY");
        let v = json(res).await;
        assert_eq!(v["ok"], true);
        assert!(v["result"]["torrents"].as_array().unwrap().is_empty());

        let magnet = r#"{"method":"add","params":{"magnet":"magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=x","paused":true}}"#;
        let v = json(
            app.clone()
                .oneshot(rpc_req("localhost", magnet))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            v["result"]["infoHash"],
            "c12fe1c06bba254a9dc9f519b335aa7c1367a88a"
        );

        let res = app
            .oneshot(rpc_req("localhost", r#"{"method":"nope"}"#))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert!(
            json(res).await["error"]
                .as_str()
                .unwrap()
                .contains("unknown method")
        );
    }

    #[tokio::test]
    async fn blocks_dns_rebinding_and_cross_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(engine(tmp.path()).await, None);
        let res = app
            .clone()
            .oneshot(rpc_req("evil.example:9696", r#"{"method":"snapshot"}"#))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let mut req = rpc_req("127.0.0.1:9696", r#"{"method":"pauseAll"}"#);
        req.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );

        let mut req = rpc_req("127.0.0.1:9696", r#"{"method":"snapshot"}"#);
        req.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:9696"),
        );
        assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn token_is_enforced() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(engine(tmp.path()).await, Some("s3cret".into()));
        let body = r#"{"method":"snapshot"}"#;
        let res = app
            .clone()
            .oneshot(rpc_req("10.0.0.5:9696", body))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        let mut req = rpc_req("10.0.0.5:9696", body);
        req.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer wrong!"),
        );
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        let mut req = rpc_req("10.0.0.5:9696", body);
        req.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer s3cret"),
        );
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::OK
        );

        let health = HttpRequest::get("/api/health")
            .header(header::HOST, "10.0.0.5")
            .body(Body::empty())
            .unwrap();
        assert_eq!(app.oneshot(health).await.unwrap().status(), StatusCode::OK);
    }

    #[test]
    fn helpers() {
        assert!(loopback_host("localhost:9696"));
        assert!(loopback_host("[::1]:80"));
        assert!(!loopback_host("localhost.evil.com"));
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
