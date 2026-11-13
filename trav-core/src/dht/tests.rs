use super::*;

#[test]
fn buckets_and_closest() {
    let own = [0u8; 20];
    let mut t = Table::new(own, Family::V4);
    for i in 1..=50u8 {
        let mut id = [0u8; 20];
        id[19] = i;
        t.insert(id, v4([10, 0, 0, i], 6881));
    }
    let mut target = [0u8; 20];
    target[19] = 3;
    let c = t.closest(&target, 3);
    assert_eq!(c[0].id[19], 3);
    assert!(t.len() <= 50);
    assert_eq!(bucket_index(&own, &[0x80; 20]), Some(0));
}

#[test]
fn random_ids_land_in_the_requested_bucket() {
    for _ in 0..20 {
        let own: NodeId = rand::random();
        for i in 0..160 {
            let id = random_id_in_bucket(&own, i);
            assert_eq!(bucket_index(&own, &id), Some(i), "bucket {i}");
        }
    }
}

#[test]
fn one_node_per_public_ip() {
    let own = [0u8; 20];
    let mut t = Table::new(own, Family::V4);
    let node = |b: u8| {
        let mut id = [0u8; 20];
        id[0] = b;
        id
    };
    t.insert(node(0x80), v4([203, 0, 113, 7], 6881));
    t.insert(node(0x40), v4([203, 0, 113, 7], 6882));
    assert_eq!(t.len(), 1, "a second id from the same public IP is ignored");
    t.insert(node(0x80), v4([203, 0, 113, 7], 6883));
    assert_eq!(
        t.closest(&own, 8)[0].addr.port(),
        6883,
        "a known id may move port"
    );
    t.insert(node(0x20), v4([192, 168, 1, 2], 1));
    t.insert(node(0x10), v4([192, 168, 1, 2], 2));
    assert_eq!(t.len(), 3, "private addresses may hold several nodes");
}

#[test]
fn lookup_takes_one_candidate_per_public_ip() {
    let target = [0u8; 20];
    let id = |b: u8| {
        let mut id = [0u8; 20];
        id[19] = b;
        id
    };
    let mut c = Candidates::default();
    // A cluster of fake ids on one host, each claiming to be closer.
    for b in (1..=9).rev() {
        c.add(&target, id(b), v4([198, 51, 100, 9], 6881 + b as u16));
    }
    c.add(&target, id(20), v4([203, 0, 113, 1], 6881));
    c.add(&target, id(20), v4([203, 0, 113, 2], 6881));
    let ips: Vec<_> = c.by_distance.values().map(|x| x.1.ip()).collect();
    assert_eq!(
        ips,
        [
            Ipv4Addr::new(198, 51, 100, 9),
            Ipv4Addr::new(203, 0, 113, 1)
        ]
        .map(IpAddr::V4)
    );
    // Loopback / LAN nodes are exempt.
    c.add(&target, id(30), v4([127, 0, 0, 1], 1));
    c.add(&target, id(31), v4([127, 0, 0, 1], 2));
    assert_eq!(c.by_distance.len(), 4);
}

#[test]
fn refresh_targets_underfilled_buckets_once() {
    let own = [0u8; 20];
    let mut t = Table::new(own, Family::V4);
    let mut id = [0u8; 20];
    id[0] = 0x08; // bucket 4
    t.insert(id, v4([10, 0, 0, 1], 1));
    let now = Instant::now();
    let targets = t.refresh_targets(now);
    let buckets: Vec<_> = targets.iter().map(|x| bucket_index(&own, x)).collect();
    assert_eq!(buckets, (0..=4).map(Some).collect::<Vec<_>>());
    assert!(
        t.refresh_targets(now).is_empty(),
        "not again until REFRESH_EVERY"
    );
    assert_eq!(t.refresh_targets(now + REFRESH_EVERY).len(), 5);
}

#[tokio::test]
async fn two_nodes_find_each_other_and_share_peers() {
    let a = start_test().await;
    let b = start_test().await;
    let b_addr: SocketAddr = format!("127.0.0.1:{}", b.port()).parse().unwrap();
    a.ping(b_addr).await;
    assert_eq!(a.node_count(), 1);
    let ih = [7u8; 20];
    // a announces into b, then b's store answers a lookup from a fresh node c.
    let _ = a.lookup(ih, true, LOOKUP_BUDGET).await;
    let peers = a.get_peers(ih, Some(4242)).await;
    assert!(peers.is_empty());
    let c = start_test().await;
    c.ping(b_addr).await;
    let found = c.get_peers(ih, None).await;
    assert_eq!(found, vec!["127.0.0.1:4242".parse::<SocketAddr>().unwrap()]);
}

#[tokio::test]
async fn replies_only_count_from_the_node_asked() {
    let a = start_test().await;
    let asked: SocketAddr = "127.0.0.1:6881".parse().unwrap();
    let spoofer: SocketAddr = "127.0.0.1:6999".parse().unwrap();
    let (tx, mut rx) = oneshot::channel();
    a.inner.pending.lock().insert(42, (asked, Some(tx)));
    let reply = DictBuilder::new()
        .value("r", DictBuilder::new().bytes("id", vec![1u8; 20]).build())
        .bytes("t", 42u16.to_be_bytes().to_vec())
        .bytes("y", "r")
        .build()
        .encode();
    a.on_packet(&reply, spoofer).await;
    assert!(
        rx.try_recv().is_err(),
        "a reply from elsewhere must be ignored"
    );
    a.on_packet(&reply, asked).await;
    assert!(rx.try_recv().unwrap().is_ok());
}

#[tokio::test]
async fn stopping_frees_the_port_and_fails_queries() {
    let a = start_test().await;
    let port = a.port();
    // A query to a silent address is in flight when we stop.
    let silent: SocketAddr = "127.0.0.1:9".parse().unwrap();
    let pending = {
        let a = a.clone();
        tokio::spawn(async move {
            let t = Instant::now();
            let args = DictBuilder::new().bytes("id", vec![0u8; 20]).build();
            let r = a.query(silent, "ping", args).await;
            (r.is_err(), t.elapsed())
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    a.stop();
    let (failed, took) = pending.await.unwrap();
    assert!(failed && took < QUERY_TIMEOUT);
    drop(a);
    let b = Dht::start_with_routers(port, vec![], &[]).await.unwrap();
    assert_eq!(b.port(), port);
}

fn v4(ip: [u8; 4], port: u16) -> SocketAddr {
    SocketAddr::new(Ipv4Addr::from(ip).into(), port)
}

fn init_logs() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .with_ansi(false)
        .try_init();
}

async fn start_test() -> Dht {
    init_logs();
    Dht::start_with_routers(0, vec![], &[]).await.unwrap()
}

async fn ipv6_available() -> bool {
    match UdpSocket::bind((Ipv6Addr::LOCALHOST, 0)).await {
        Ok(_) => true,
        Err(error) => {
            eprintln!("SKIP IPv6 loopback test: ::1 unavailable: {error}");
            false
        }
    }
}

fn query_message(method: &str, args: DictBuilder) -> Value {
    DictBuilder::new()
        .bytes("q", method)
        .value("a", args.bytes("id", vec![0x55; 20]).build())
        .bytes("t", vec![1, 2])
        .bytes("y", "q")
        .build()
}

#[test]
fn compact_v6_nodes_and_peers_roundtrip_and_truncation() {
    let addr: SocketAddr = "[2001:db8:1::1234]:6881".parse().unwrap();
    let node = Node {
        id: [0x11; 20],
        addr,
        last_seen: Instant::now(),
        fails: 0,
    };
    let encoded = encode_nodes(&[node]);
    assert_eq!(encoded.len(), 38);
    assert_eq!(
        decode_nodes(&encoded, Family::V6).collect::<Vec<_>>(),
        vec![([0x11; 20], addr)]
    );
    for length in 0..38 {
        assert_eq!(decode_nodes(&encoded[..length], Family::V6).count(), 0);
    }
    let mut with_tail = encoded.clone();
    with_tail.extend_from_slice(&encoded[..37]);
    assert_eq!(decode_nodes(&with_tail, Family::V6).count(), 1);
    let mut peer = Vec::new();
    crate::tracker::encode_compact(&addr, &mut peer);
    assert_eq!(peer.len(), 18);
    assert_eq!(decode_peer(&peer), Some(addr));
    for length in 0..18 {
        // A six-byte string is a valid IPv4-shaped entry, not truncated v6.
        if length != 6 {
            assert_eq!(decode_peer(&peer[..length]), None);
        }
    }
    peer.push(0);
    assert_eq!(decode_peer(&peer), None);
    let mut zero_port = encoded;
    zero_port[36..].fill(0);
    assert_eq!(decode_nodes(&zero_port, Family::V6).count(), 0);
    assert_eq!(decode_peer(&zero_port[20..]), None);
}

#[test]
fn tables_keep_families_separate_and_bounded() {
    let mut v4_table = Table::new([0; 20], Family::V4);
    let mut v6_table = Table::new([0; 20], Family::V6);
    let a4: SocketAddr = "127.0.0.1:1234".parse().unwrap();
    let a6: SocketAddr = "[::1]:1234".parse().unwrap();
    for table in [&mut v4_table, &mut v6_table] {
        table.insert([0x80; 20], a4);
        table.insert([0x80; 20], a6);
    }
    assert_eq!(v4_table.all_addrs(), vec![a4]);
    assert_eq!(v6_table.all_addrs(), vec![a6]);
    for family in [Family::V4, Family::V6] {
        let mut table = Table::new([0; 20], family);
        for bucket in 0..160 {
            for port in 1..=32 {
                table.insert(
                    random_id_in_bucket(&[0; 20], bucket),
                    SocketAddr::new(
                        if family == Family::V4 {
                            Ipv4Addr::LOCALHOST.into()
                        } else {
                            Ipv6Addr::LOCALHOST.into()
                        },
                        port,
                    ),
                );
            }
        }
        assert!(table.len() <= 160 * K);
        assert!(table.len() > 1200);
        assert!(table.buckets.iter().all(|bucket| bucket.len() <= K));
    }
}

#[test]
fn ipv6_public_prefix_limits_tables_and_candidates() {
    let target = [0; 20];
    let mut table = Table::new(target, Family::V6);
    let mut candidates = Candidates::default();
    for index in 1..=64u8 {
        let mut id = [0; 20];
        id[0] = index;
        let addr: SocketAddr = format!("[2001:db8:1::{index}]:6881").parse().unwrap();
        table.insert(id, addr);
        candidates.add(&target, id, addr);
    }
    assert_eq!(table.len(), 1);
    assert_eq!(candidates.by_distance.len(), 1);
    let mut id = [0; 20];
    id[0] = 0x80;
    let other: SocketAddr = "[2001:db8:2::1]:6881".parse().unwrap();
    table.insert(id, other);
    candidates.add(&target, id, other);
    assert_eq!(table.len(), 2);
    assert_eq!(candidates.by_distance.len(), 2);
}

#[test]
fn lookup_candidates_are_bounded() {
    let mut candidates = Candidates::default();
    for value in (1..=MAX_CANDIDATES * 2).rev() {
        let mut id = [0; 20];
        id[12..].copy_from_slice(&(value as u64).to_be_bytes());
        candidates.add(&[0; 20], id, v4([127, 0, 0, 1], value as u16));
    }
    assert_eq!(candidates.by_distance.len(), MAX_CANDIDATES);
    assert_eq!(
        candidates.by_distance.first_key_value().unwrap().1.1.port(),
        1
    );
}

#[tokio::test]
async fn want_selects_response_tables_and_values_follow_transport() {
    let node = start_test().await;
    let a4: SocketAddr = "127.0.0.1:6881".parse().unwrap();
    let a6: SocketAddr = "[::1]:6881".parse().unwrap();
    let ih = [7; 20];
    node.inner
        .store
        .lock()
        .insert(ih, vec![(a4, Instant::now()), (a6, Instant::now())]);
    for method in ["find_node", "get_peers"] {
        for from in [a4, a6] {
            for flags in [
                None,
                Some(vec!["n4"]),
                Some(vec!["n6"]),
                Some(vec!["n4", "n6"]),
                Some(vec!["future"]),
            ] {
                let mut args = DictBuilder::new()
                    .bytes("target", ih.to_vec())
                    .bytes("info_hash", ih.to_vec());
                if let Some(flags) = &flags {
                    args = args.value(
                        "want",
                        Value::List(
                            flags
                                .iter()
                                .map(|flag| Value::Bytes(flag.as_bytes().to_vec()))
                                .collect(),
                        ),
                    );
                }
                let reply = node.answer(&query_message(method, args), from).unwrap();
                let expected4 = flags
                    .as_ref()
                    .map_or(from.is_ipv4(), |flags| flags.contains(&"n4"));
                let expected6 = flags
                    .as_ref()
                    .map_or(from.is_ipv6(), |flags| flags.contains(&"n6"));
                assert_eq!(reply.get("nodes").is_some(), expected4);
                assert_eq!(reply.get("nodes6").is_some(), expected6);
                if method == "get_peers" {
                    let values = reply.get("values").unwrap().as_list().unwrap();
                    assert_eq!(values.len(), 1);
                    assert_eq!(decode_peer(values[0].as_bytes().unwrap()), Some(from));
                }
            }
        }
    }
    node.stop();
}

#[tokio::test]
async fn ipv6_loopback_announces_and_finds_peers() {
    if !ipv6_available().await {
        return;
    }
    let a = start_test().await;
    let b = start_test().await;
    let c = start_test().await;
    let b_addr: SocketAddr = format!("[::1]:{}", b.port()).parse().unwrap();
    a.ping(b_addr).await;
    assert_eq!(a.inner.tables[1].lock().len(), 1);
    assert_eq!(a.inner.tables[0].lock().len(), 0);
    let ih = [0x66; 20];
    assert!(a.get_peers(ih, Some(4242)).await.is_empty());
    c.ping(b_addr).await;
    let found = c.get_peers(ih, None).await;
    assert_eq!(found, vec!["[::1]:4242".parse::<SocketAddr>().unwrap()]);
    // The opposite direction also receives 18-byte values over IPv6.
    c.get_peers(ih, Some(4343)).await;
    assert!(
        a.get_peers(ih, None)
            .await
            .contains(&"[::1]:4343".parse().unwrap())
    );
    for node in [&a, &b, &c] {
        node.stop();
    }
}

#[tokio::test]
async fn mixed_loopback_merges_and_announces_both_families() {
    if !ipv6_available().await {
        return;
    }
    let a = start_test().await;
    let b = start_test().await;
    let c = start_test().await;
    for host in ["127.0.0.1", "[::1]"] {
        a.ping(format!("{host}:{}", b.port()).parse().unwrap())
            .await;
        c.ping(format!("{host}:{}", b.port()).parse().unwrap())
            .await;
    }
    // Maintenance may already have learned c through b as well.
    assert!(a.node_count() >= 2);
    assert!(a.known_nodes().iter().any(SocketAddr::is_ipv4));
    assert!(a.known_nodes().iter().any(SocketAddr::is_ipv6));
    let ih = [0x67; 20];
    a.get_peers(ih, Some(4444)).await;
    let found = c.get_peers(ih, None).await;
    assert_eq!(
        found.len(),
        2,
        "duplicates from multiple responders must be removed: {found:?}"
    );
    assert!(found.contains(&"127.0.0.1:4444".parse().unwrap()));
    assert!(found.contains(&"[::1]:4444".parse().unwrap()));
    for node in [&a, &b, &c] {
        node.stop();
    }
}

#[tokio::test]
async fn ipv6_replies_only_count_from_the_node_asked() {
    let a = start_test().await;
    let asked: SocketAddr = "[::1]:6881".parse().unwrap();
    let reply = DictBuilder::new()
        .bytes("t", 42u16.to_be_bytes().to_vec())
        .bytes("y", "r")
        .value("r", DictBuilder::new().bytes("id", vec![1; 20]).build())
        .build()
        .encode();
    let (tx, mut rx) = oneshot::channel();
    a.inner.pending.lock().insert(42, (asked, Some(tx)));
    for spoofer in ["[::1]:6882", "[::2]:6881", "127.0.0.1:6881"] {
        a.on_packet(&reply, spoofer.parse().unwrap()).await;
        assert!(rx.try_recv().is_err());
    }
    a.on_packet(&reply, asked).await;
    assert!(rx.try_recv().unwrap().is_ok());
    a.stop();
}

#[tokio::test]
async fn ipv6_send_failure_backs_off_without_delaying_ipv4() {
    init_logs();
    let socket = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap());
    let (tx, rx) = mpsc::channel(32);
    let a = Dht::start_on_with_routers(UdpSender::new(socket.clone()), rx, vec![], &[]);
    let receive = tokio::spawn(async move {
        let mut bytes = [0; 2048];
        while let Ok((length, from)) = socket.recv_from(&mut bytes).await {
            if tx
                .send((Bytes::copy_from_slice(&bytes[..length]), from))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    let b = start_test().await;
    let b4: SocketAddr = format!("127.0.0.1:{}", b.port()).parse().unwrap();
    a.ping(b4).await;
    let ih = [0x68; 20];
    b.inner.store.lock().insert(
        ih,
        vec![("127.0.0.1:4545".parse().unwrap(), Instant::now())],
    );
    let b6: SocketAddr = format!("[::1]:{}", b.port()).parse().unwrap();
    a.inner.tables[1].lock().insert(b.inner.id, b6);
    let started = Instant::now();
    let peers = tokio::time::timeout(Duration::from_secs(1), a.get_peers(ih, None))
        .await
        .unwrap();
    assert_eq!(peers, vec!["127.0.0.1:4545".parse::<SocketAddr>().unwrap()]);
    assert!(!a.ipv6_usable());
    let retry = *a.inner.ipv6_retry.lock();
    for _ in 0..20 {
        a.ping(b6).await;
    }
    assert_eq!(
        *a.inner.ipv6_retry.lock(),
        retry,
        "failed probes do not extend cooldown or log repeatedly"
    );
    assert_eq!(a.get_peers(ih, None).await, peers);
    eprintln!(
        "IPv4 lookup with IPv6 send failure completed in {:?}",
        started.elapsed()
    );
    *a.inner.ipv6_retry.lock() = Some(Instant::now() - Duration::from_secs(1));
    assert!(a.ipv6_usable(), "IPv6 retries resume after backoff");
    a.stop();
    b.stop();
    receive.abort();
    let _ = receive.await;
}

#[tokio::test]
async fn malformed_packets_and_compact_entries_never_panic() {
    let dht = start_test().await;
    for length in 0..256usize {
        let bytes: Vec<u8> = (0..length).map(|_| rand::random()).collect();
        for family in [Family::V4, Family::V6] {
            let _ = decode_nodes(&bytes, family).collect::<Vec<_>>();
        }
        let _ = decode_peer(&bytes);
        for from in ["127.0.0.1:6881", "[::1]:6881"] {
            dht.on_packet(&bytes, from.parse().unwrap()).await;
            // Exercise valid bencode with malformed field types and lengths.
            for method in ["get_peers", "find_node", "announce_peer"] {
                let query = query_message(
                    method,
                    DictBuilder::new()
                        .bytes("id", bytes.clone())
                        .bytes("info_hash", bytes.clone())
                        .bytes("target", bytes.clone())
                        .bytes("token", bytes.clone())
                        .value(
                            "want",
                            Value::List(vec![Value::Int(-1), Value::Bytes(bytes.clone())]),
                        ),
                );
                let _ = dht.answer(&query, from.parse().unwrap());
            }
        }
    }
    dht.stop();
}

#[tokio::test]
async fn persisted_dual_family_nodes_bootstrap_both_tables() {
    if !ipv6_available().await {
        return;
    }
    let b = start_test().await;
    let seeds: Vec<SocketAddr> = ["127.0.0.1", "[::1]"]
        .iter()
        .map(|host| format!("{host}:{}", b.port()).parse().unwrap())
        .collect();
    let saved = serde_json::to_string(&seeds).unwrap();
    let loaded: Vec<SocketAddr> = serde_json::from_str(&saved).unwrap();
    let a = Dht::start_with_routers(0, loaded, &[]).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while a.node_count() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let known = a.known_nodes();
    assert_eq!(known.len(), 2);
    assert!(seeds.iter().all(|seed| known.contains(seed)));
    a.stop();
    b.stop();
}

#[tokio::test]
async fn shared_utp_socket_carries_ipv6_dht_queries_and_values() {
    if !ipv6_available().await {
        return;
    }
    init_logs();
    let socket = crate::utp::UtpSocket::bind(0).await.unwrap();
    let (tx, rx) = mpsc::channel(32);
    socket.set_other(Some(tx));
    let a = Dht::start_on_with_routers(socket.sender(), rx, vec![], &[]);
    let b = start_test().await;
    a.ping(format!("[::1]:{}", b.port()).parse().unwrap()).await;
    let ih = [0x69; 20];
    a.get_peers(ih, Some(4646)).await;
    assert!(
        b.inner.store.lock()[&ih]
            .iter()
            .any(|(peer, _)| *peer == "[::1]:4646".parse::<SocketAddr>().unwrap())
    );
    assert_eq!(
        a.get_peers(ih, None).await,
        vec!["[::1]:4646".parse::<SocketAddr>().unwrap()]
    );
    // b asks back over the shared socket, then receives 18-byte values from it.
    assert_eq!(b.inner.tables[1].lock().len(), 1);
    b.get_peers(ih, Some(4747)).await;
    assert_eq!(
        b.get_peers(ih, None).await,
        vec!["[::1]:4747".parse::<SocketAddr>().unwrap()]
    );
    a.stop();
    b.stop();
    socket.close();
}

#[tokio::test]
async fn lookup_parses_hybrid_values_and_rejects_truncated_entries() {
    let a = start_test().await;
    let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = server.local_addr().unwrap();
    a.inner.tables[0].lock().insert([0x77; 20], addr);
    let peer4: SocketAddr = "192.0.2.1:4777".parse().unwrap();
    let peer6: SocketAddr = "[2001:db8:3::1]:4888".parse().unwrap();
    let mut compact4 = Vec::new();
    let mut compact6 = Vec::new();
    crate::tracker::encode_compact(&peer4, &mut compact4);
    crate::tracker::encode_compact(&peer6, &mut compact6);
    let fixture = tokio::spawn(async move {
        let mut bytes = [0; 2048];
        let (length, from) = server.recv_from(&mut bytes).await.unwrap();
        let query = bencode::decode(&bytes[..length]).unwrap();
        let args = query.get("a").unwrap();
        assert_eq!(
            wants(args, from),
            (true, true),
            "our query explicitly requests both families"
        );
        let reply = DictBuilder::new()
            .bytes("y", "r")
            .bytes("t", query.get("t").unwrap().as_bytes().unwrap().to_vec())
            .value(
                "r",
                DictBuilder::new()
                    .bytes("id", vec![0x77; 20])
                    .bytes("nodes6", vec![0; 37])
                    .value(
                        "values",
                        Value::List(vec![
                            Value::Bytes(compact4.clone()),
                            Value::Bytes(compact6.clone()),
                            Value::Bytes(compact4),
                            Value::Bytes(compact6),
                            Value::Bytes(vec![1; 17]),
                            Value::Bytes(vec![1; 19]),
                            Value::Int(123),
                        ]),
                    )
                    .build(),
            )
            .build()
            .encode();
        server.send_to(&reply, from).await.unwrap();
    });
    let peers = a.get_peers([0x70; 20], None).await;
    assert_eq!(peers.len(), 2);
    assert!(peers.contains(&peer4) && peers.contains(&peer6));
    fixture.await.unwrap();
    a.stop();
}

#[tokio::test]
async fn v4_node_refers_lookup_into_ipv6_table() {
    if !ipv6_available().await {
        return;
    }
    let a = start_test().await;
    let b = start_test().await;
    let c = start_test().await;
    let c6: SocketAddr = format!("[::1]:{}", c.port()).parse().unwrap();
    b.ping(c6).await;
    a.ping(format!("127.0.0.1:{}", b.port()).parse().unwrap())
        .await;
    assert_eq!(a.inner.tables[1].lock().len(), 0);
    let ih = [0x71; 20];
    c.inner
        .store
        .lock()
        .insert(ih, vec![("[::1]:4999".parse().unwrap(), Instant::now())]);
    let peers = a.get_peers(ih, None).await;
    assert_eq!(peers, vec!["[::1]:4999".parse::<SocketAddr>().unwrap()]);
    assert!(
        a.known_nodes().contains(&c6),
        "a referred v6 node enters the table only after answering"
    );
    for node in [&a, &b, &c] {
        node.stop();
    }
}

#[test]
fn ipv6_maintenance_tracks_stale_nodes_and_refreshes_buckets() {
    let own = [0; 20];
    let mut table = Table::new(own, Family::V6);
    let mut id = [0; 20];
    id[0] = 0x08;
    let addr: SocketAddr = "[::1]:6881".parse().unwrap();
    table.insert(id, addr);
    let now = Instant::now();
    table.buckets[4][0].last_seen = now - STALE - Duration::from_secs(1);
    assert_eq!(table.questionable(16), vec![addr]);
    assert_eq!(table.refresh_targets(now).len(), 5);
    assert!(table.refresh_targets(now).is_empty());
    assert_eq!(table.refresh_targets(now + REFRESH_EVERY).len(), 5);
    for _ in 0..4 {
        table.failed(&id);
    }
    assert_eq!(table.len(), 0);
}
