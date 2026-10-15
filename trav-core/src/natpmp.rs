//! NAT-PMP (RFC 6886) port mapping: what Apple routers and many open-source
//! firmwares speak instead of UPnP. Tried when UPnP finds no gateway.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::UdpSocket;

const PORT: u16 = 5351;

/// The router to talk to: the IPv4 default gateway.
pub fn default_gateway() -> Option<Ipv4Addr> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/sbin/route")
            .args(["-n", "get", "default"])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|l| l.trim().strip_prefix("gateway:")?.trim().parse().ok())
    }
    #[cfg(target_os = "linux")]
    {
        // /proc/net/route: destination 00000000 is the default; little-endian hex.
        let table = std::fs::read_to_string("/proc/net/route").ok()?;
        table.lines().skip(1).find_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.get(1) == Some(&"00000000"))
                .then(|| u32::from_str_radix(f.get(2)?, 16).ok())
                .flatten()
                .map(|g| Ipv4Addr::from(g.swap_bytes()))
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Send `req` and wait for a reply to opcode `op`, retrying as RFC 6886 asks
/// (250 ms, doubling).
async fn ask(sock: &UdpSocket, req: &[u8], op: u8) -> Result<Vec<u8>, String> {
    let mut wait = Duration::from_millis(250);
    for _ in 0..4 {
        sock.send(req).await.map_err(|e| e.to_string())?;
        let mut buf = [0u8; 16];
        if let Ok(Ok(n)) = tokio::time::timeout(wait, sock.recv(&mut buf)).await {
            let reply = &buf[..n];
            if n >= 8 && reply[0] == 0 && reply[1] == 128 + op {
                let code = u16::from_be_bytes([reply[2], reply[3]]);
                if code != 0 {
                    return Err(format!("router refused (NAT-PMP error {code})"));
                }
                return Ok(reply.to_vec());
            }
        }
        wait *= 2;
    }
    Err("no NAT-PMP router answered".into())
}

/// Map TCP and UDP `port` for `lifetime` seconds. Returns the external IP and
/// the external port the router chose (usually `port`).
pub async fn map(port: u16, lifetime: u32) -> Result<(Ipv4Addr, u16), String> {
    let gw = default_gateway().ok_or("no default gateway")?;
    map_at(SocketAddr::from((gw, PORT)), port, lifetime).await
}

async fn map_at(router: SocketAddr, port: u16, lifetime: u32) -> Result<(Ipv4Addr, u16), String> {
    let sock = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    sock.connect(router).await.map_err(|e| e.to_string())?;
    let ip = ask(&sock, &[0, 0], 0).await?;
    if ip.len() < 12 {
        return Err("short NAT-PMP reply".into());
    }
    let external_ip = Ipv4Addr::new(ip[8], ip[9], ip[10], ip[11]);
    let mut external_port = port;
    // UDP (1) first: its port is the one µTP and the DHT are reachable on.
    for op in [1u8, 2] {
        let mut req = vec![0, op, 0, 0];
        req.extend_from_slice(&port.to_be_bytes());
        req.extend_from_slice(&external_port.to_be_bytes());
        req.extend_from_slice(&lifetime.to_be_bytes());
        let reply = ask(&sock, &req, op).await?;
        if reply.len() < 16 {
            return Err("short NAT-PMP reply".into());
        }
        external_port = u16::from_be_bytes([reply[10], reply[11]]);
    }
    Ok((external_ip, external_port))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A router that answers like RFC 6886 says, giving out `external`.
    async fn fake_router(external: u16, refuse_tcp: bool) -> SocketAddr {
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = sock.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 16];
            let mut first = true;
            while let Ok((n, from)) = sock.recv_from(&mut buf).await {
                // Lose the very first request, as a busy router might.
                if std::mem::take(&mut first) {
                    continue;
                }
                let op = buf[1];
                let mut reply = vec![0, 128 + op];
                let code: u16 = if op == 2 && refuse_tcp { 2 } else { 0 };
                reply.extend_from_slice(&code.to_be_bytes());
                reply.extend_from_slice(&1234u32.to_be_bytes());
                if op == 0 {
                    reply.extend_from_slice(&[203, 0, 113, 7]);
                } else {
                    reply.extend_from_slice(&buf[4..6]);
                    reply.extend_from_slice(&external.to_be_bytes());
                    reply.extend_from_slice(&buf[8..n.min(12)]);
                }
                let _ = sock.send_to(&reply, from).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn maps_both_protocols() {
        let r = fake_router(40000, false).await;
        assert_eq!(
            map_at(r, 51413, 3600).await,
            Ok((Ipv4Addr::new(203, 0, 113, 7), 40000))
        );
    }

    #[tokio::test]
    async fn reports_refusals() {
        let r = fake_router(51413, true).await;
        let e = map_at(r, 51413, 3600).await.unwrap_err();
        assert!(e.contains("error 2"), "{e}");
    }

    #[test]
    fn finds_a_gateway_or_none() {
        // Machines without a network have none; with one, it's a unicast address.
        if let Some(g) = super::default_gateway() {
            assert!(!g.is_unspecified() && !g.is_multicast(), "{g}");
        }
    }
}
