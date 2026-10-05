//! UPnP IGD port forwarding so peers behind home routers are reachable.

use std::net::{IpAddr, SocketAddr};
use std::sync::Weak;
use std::sync::atomic::Ordering;
use std::time::Duration;

use igd_next::aio::tokio::search_gateway;
use igd_next::{PortMappingProtocol, SearchOptions};
use tracing::{debug, info};

use crate::ctx::Ctx;

const LEASE: u32 = 3600;
const REFRESH: Duration = Duration::from_secs(30 * 60);
const RETRY: Duration = Duration::from_secs(5 * 60);

/// Map TCP + UDP for the listen port and keep the lease fresh while enabled.
pub(crate) fn spawn(ctx: Weak<Ctx>) {
    tokio::spawn(async move {
        let mut mapped: Option<u16> = None;
        loop {
            let Some(c) = ctx.upgrade() else { break };
            let enabled = c.settings.read().enable_upnp;
            let port = c.listen_port.load(Ordering::Relaxed);
            drop(c);
            let wait = if !enabled || port == 0 {
                Duration::from_secs(10)
            } else {
                match map(port).await {
                    Ok(ext) => {
                        if mapped != Some(port) {
                            info!("UPnP: forwarded port {port} (external IP {ext})");
                        }
                        mapped = Some(port);
                        set_status(&ctx, format!("forwarded · {ext}:{port}"));
                        REFRESH
                    }
                    Err(e) => {
                        debug!("UPnP: {e}");
                        mapped = None;
                        set_status(&ctx, e);
                        RETRY
                    }
                }
            };
            // Wake early if the port or toggle changes.
            let deadline = tokio::time::Instant::now() + wait;
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let Some(c) = ctx.upgrade() else { return };
                let now_port = c.listen_port.load(Ordering::Relaxed);
                let now_enabled = c.settings.read().enable_upnp;
                if now_enabled != enabled || (enabled && Some(now_port) != mapped) {
                    break;
                }
            }
        }
    });
}

fn set_status(ctx: &Weak<Ctx>, s: String) {
    if let Some(c) = ctx.upgrade() {
        *c.upnp_status.write() = Some(s);
    }
}

async fn map(port: u16) -> Result<IpAddr, String> {
    let opts = SearchOptions {
        timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    };
    let gw = search_gateway(opts)
        .await
        .map_err(|_| "no UPnP gateway found".to_string())?;
    // Learn which local interface routes to the gateway.
    let probe = tokio::net::UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    probe.connect(gw.addr).await.map_err(|e| e.to_string())?;
    let local_ip = probe.local_addr().map_err(|e| e.to_string())?.ip();
    let local = SocketAddr::new(local_ip, port);
    for proto in [PortMappingProtocol::TCP, PortMappingProtocol::UDP] {
        gw.add_port(proto, port, local, LEASE, "Trav")
            .await
            .map_err(|e| format!("gateway refused mapping: {e}"))?;
    }
    gw.get_external_ip().await.map_err(|e| e.to_string())
}
