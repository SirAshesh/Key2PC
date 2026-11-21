use std::time::{Duration, Instant};

use key2pc_core::network::{
    create_tuned_udp_socket, resolve_target_host, RetransmitQueue, UdpSocketConfig,
};
use key2pc_core::protocol::{derive_auth_key, KeyAction, KeyEvent, Packet, PACKET_SIZE};

#[test]
fn retransmit_queue_tracks_acks_and_retries() {
    let mut queue = RetransmitQueue::default();
    let start = Instant::now();
    let raw = [7u8; PACKET_SIZE];
    queue.push(10, raw, start);
    assert!(!queue.is_empty());

    let due_early = queue.collect_due(start + Duration::from_millis(1), Duration::from_millis(3), 2);
    assert!(due_early.is_empty());

    let due_first = queue.collect_due(start + Duration::from_millis(3), Duration::from_millis(3), 2);
    assert_eq!(due_first.len(), 1);

    let rtt = queue.ack(10, start + Duration::from_millis(4));
    assert_eq!(rtt, Some(Duration::from_millis(4)));
    assert!(queue.is_empty());
}

#[test]
fn udp_sockets_exchange_packets_over_loopback() {
    let key = derive_auth_key("loopback-secret");
    let server = create_tuned_udp_socket(&UdpSocketConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        read_timeout: Some(Duration::from_millis(200)),
        ..Default::default()
    })
    .unwrap();
    let server_addr = server.local_addr().unwrap();

    let client = create_tuned_udp_socket(&UdpSocketConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        read_timeout: Some(Duration::from_millis(200)),
        ..Default::default()
    })
    .unwrap();

    let pkt = Packet::key_event(
        1,
        12345,
        KeyEvent {
            vk: 0x41,
            scan_code: 0x1E,
            action: KeyAction::Down,
            extended: false,
            alt_down: false,
        },
    );
    client.send_to(&pkt.encode(&key), server_addr).unwrap();

    let mut buf = [0u8; 128];
    let (n, peer) = server.recv_from(&mut buf).unwrap();
    let decoded = Packet::decode(&buf[..n], &key).unwrap();
    assert_eq!(decoded, pkt);

    server
        .send_to(&Packet::ack(decoded.seq, decoded.timestamp_us).encode(&key), peer)
        .unwrap();
    let (ack_n, _) = client.recv_from(&mut buf).unwrap();
    let ack = Packet::decode(&buf[..ack_n], &key).unwrap();
    assert_eq!(ack.seq, 1);
}

#[test]
fn resolves_auto_and_explicit_target_hosts() {
    assert_eq!(resolve_target_host("auto", 24800).unwrap(), None);
    let ip = resolve_target_host("192.168.1.50", 24800).unwrap().unwrap();
    assert_eq!(ip.to_string(), "192.168.1.50:24800");
}
