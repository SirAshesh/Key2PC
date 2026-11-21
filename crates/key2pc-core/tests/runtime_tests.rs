use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use key2pc_core::config::{AppConfig, Role};
use key2pc_core::input::InputSink;
use key2pc_core::network::{create_tuned_udp_socket, UdpSocketConfig};
use key2pc_core::protocol::{
    derive_auth_key, KeyAction, KeyEvent, Packet, PacketKind, PACKET_SIZE,
};
use key2pc_core::runtime::{ManagedService, ReceiverRuntime, StatusObserver};
use key2pc_core::Result;

struct MockInputSink {
    events: Arc<Mutex<Vec<KeyEvent>>>,
    held: usize,
    release_calls: Arc<Mutex<usize>>,
}

impl InputSink for MockInputSink {
    fn handle_key(&mut self, event: KeyEvent) -> Result<()> {
        if event.action == KeyAction::Down {
            self.held += 1;
        } else {
            self.held = self.held.saturating_sub(1);
        }
        self.events.lock().unwrap().push(event);
        Ok(())
    }

    fn release_all(&mut self) -> Result<()> {
        self.held = 0;
        *self.release_calls.lock().unwrap() += 1;
        Ok(())
    }

    fn held_count(&self) -> usize {
        self.held
    }
}

#[test]
fn receiver_runtime_processes_key_events_deduplicates_and_heals() {
    let mut cfg = AppConfig::default();
    cfg.role = Role::Receiver;
    cfg.network.bind_ip = "127.0.0.1".to_string();
    cfg.network.port = 25891;
    cfg.network.discovery_port = 25892;
    cfg.network.shared_secret = "test-secret-xyz".to_string();

    let events = Arc::new(Mutex::new(Vec::new()));
    let release_calls = Arc::new(Mutex::new(0usize));
    let sink = Box::new(MockInputSink {
        events: Arc::clone(&events),
        held: 0,
        release_calls: Arc::clone(&release_calls),
    });
    let observer: StatusObserver = Arc::new(|_| {});
    let mut runtime = ReceiverRuntime::start_with_sink(&cfg, sink, observer).unwrap();

    let client = create_tuned_udp_socket(&UdpSocketConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        read_timeout: Some(Duration::from_millis(500)),
        ..Default::default()
    })
    .unwrap();
    let key = derive_auth_key(&cfg.network.shared_secret);

    let disc_req = Packet::discover_req(1, 100).encode(&key);
    client.send_to(&disc_req, "127.0.0.1:25892").unwrap();
    let mut buf = [0u8; PACKET_SIZE * 2];
    let (n, _) = client.recv_from(&mut buf).unwrap();
    let resp = Packet::decode(&buf[..n], &key).unwrap();
    assert_eq!(resp.kind, PacketKind::DiscoverResp);
    assert_eq!(resp.vk, 25891);

    let key_ev = KeyEvent {
        vk: 0x41,
        scan_code: 0x1E,
        action: KeyAction::Down,
        extended: false,
        alt_down: false,
    };
    let pkt = Packet::key_event(1, 200, key_ev).encode(&key);
    client.send_to(&pkt, "127.0.0.1:25891").unwrap();
    client.send_to(&pkt, "127.0.0.1:25891").unwrap();

    let (ack_n, _) = client.recv_from(&mut buf).unwrap();
    let ack = Packet::decode(&buf[..ack_n], &key).unwrap();
    assert_eq!(ack.kind, PacketKind::Ack);
    assert_eq!(ack.seq, 1);

    thread::sleep(Duration::from_millis(50));
    assert_eq!(events.lock().unwrap().len(), 1);

    let hb = Packet::heartbeat(2, 300, 0).encode(&key);
    client.send_to(&hb, "127.0.0.1:25891").unwrap();
    thread::sleep(Duration::from_millis(40));
    assert!(*release_calls.lock().unwrap() >= 1);

    runtime.shutdown();
}
