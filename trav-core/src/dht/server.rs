use tokio::net::UdpSocket;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use bytes::Bytes;
use tracing::{info, debug, error};

use super::krpc::{KrpcMessage, QueryArgs, ResponseArgs};
use super::routing::RoutingTable;
use crate::error::Result;

pub struct DhtServer {
    socket: Arc<UdpSocket>,
    routing_table: Arc<Mutex<RoutingTable>>,
    pub port: u16,
}

impl DhtServer {
    pub async fn start(port: u16, node_id: [u8; 20]) -> Result<Self> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", port)).await
            .map_err(|e| crate::error::BitTorrentError::Engine(e.to_string()))?;
        let socket = Arc::new(socket);
        let routing_table = Arc::new(Mutex::new(RoutingTable::new(node_id)));

        // Spawn listener
        let listen_socket = socket.clone();
        let listen_table = routing_table.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                if let Ok((len, addr)) = listen_socket.recv_from(&mut buf).await {
                    let data = &buf[..len];
                    if let Ok(msg) = serde_bencode::from_bytes::<KrpcMessage>(data) {
                        Self::handle_message(msg, addr, &listen_table, &listen_socket).await;
                    }
                }
            }
        });

        Ok(Self { socket, routing_table, port })
    }

    async fn handle_message(
        msg: KrpcMessage, 
        addr: SocketAddr, 
        table: &Arc<Mutex<RoutingTable>>,
        socket: &Arc<UdpSocket>
    ) {
        let mut t = table.lock().await;

        if msg.y == "q" {
            if let Some(args) = msg.a {
                if args.id.len() == 20 {
                    let mut node_id = [0u8; 20];
                    node_id.copy_from_slice(&args.id);
                    t.add_node(node_id, addr);
                }

                if let Some(method) = msg.q.as_deref() {
                    match method {
                        "ping" => {
                            let response = KrpcMessage {
                                t: msg.t.clone(),
                                y: "r".to_string(),
                                q: None,
                                a: None,
                                r: Some(ResponseArgs {
                                    id: t.our_id.to_vec(),
                                    nodes: None,
                                    values: None,
                                    token: None,
                                }),
                                e: None,
                            };
                            if let Ok(encoded) = serde_bencode::to_bytes(&response) {
                                let _ = socket.send_to(&encoded, addr).await;
                            }
                        }
                        // Other queries like find_node, get_peers handle similarly by generating ResponseArgs
                        _ => {
                            debug!("Unhandled DHT method {}", method);
                        }
                    }
                }
            }
        }
    }
}
