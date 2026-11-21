use key2pc_core::protocol::{derive_auth_key, KeyAction, KeyEvent, Packet, PacketKind, SequenceWindow};

#[test]
fn encodes_decodes_and_rejects_tampered_packets() {
    let key = derive_auth_key("secret-123");
    let wrong_key = derive_auth_key("wrong-key");
    let event = KeyEvent {
        vk: 0x41,
        scan_code: 0x1E,
        action: KeyAction::Down,
        extended: false,
        alt_down: false,
    };
    let packet = Packet::key_event(42, 999_999, event);
    let mut raw = packet.encode(&key);

    let decoded = Packet::decode(&raw, &key).unwrap();
    assert_eq!(decoded, packet);
    assert_eq!(decoded.as_key_event(), Some(event));
    assert!(Packet::decode(&raw, &wrong_key).is_err());

    raw[20] ^= 0x01;
    assert!(Packet::decode(&raw, &key).is_err());
}

#[test]
fn encodes_control_packets_with_session_flags() {
    let key = derive_auth_key("control-secret");
    let ping = Packet::ping(7, 500, true);
    let decoded_ping = Packet::decode(&ping.encode(&key), &key).unwrap();
    assert_eq!(decoded_ping.kind, PacketKind::Ping);
    assert!(decoded_ping.is_session_start());

    let release = Packet::release_all(8, 600, false);
    let decoded_release = Packet::decode(&release.encode(&key), &key).unwrap();
    assert_eq!(decoded_release.kind, PacketKind::ReleaseAll);
    assert!(!decoded_release.is_session_start());
}

#[test]
fn deduplicates_retransmits_and_allows_reordered_packets() {
    let mut window = SequenceWindow::default();
    assert!(window.accept(1));
    assert!(!window.accept(1));
    assert!(window.accept(3));
    assert!(window.accept(2));
    assert!(!window.accept(2));
    assert!(!window.accept(3));

    window.reset();
    assert!(window.accept(1));
}
