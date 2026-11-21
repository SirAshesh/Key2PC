use std::time::{Duration, Instant};

use key2pc_core::protocol::{
    chunk_video_frame, decode_video_packet, derive_auth_key, encode_video_packet, FrameAssembler,
    VideoPacketHeader, VideoPacketKind,
};

#[test]
fn video_packet_encodes_decodes_and_rejects_tampering() {
    let key = derive_auth_key("video-secret");
    let header = VideoPacketHeader {
        kind: VideoPacketKind::Chunk,
        frame_id: 7,
        chunk_idx: 0,
        chunk_count: 1,
        flags: 1,
        width: 1920,
        height: 1080,
        payload_len: 5,
        timestamp_us: 424242,
    };
    let mut buf = Vec::new();
    encode_video_packet(&header, b"hello", &key, &mut buf).unwrap();

    let (decoded_hdr, payload) = decode_video_packet(&buf, &key).unwrap();
    assert_eq!(decoded_hdr, header);
    assert_eq!(payload, b"hello");

    buf[45] ^= 0xFF;
    assert!(decode_video_packet(&buf, &key).is_err());
}

#[test]
fn chunks_and_reassembles_out_of_order_frames() {
    let key = derive_auth_key("chunk-secret");
    let raw_frame: Vec<u8> = (0..5000).map(|i| (i % 251) as u8).collect();
    let mut packets = Vec::new();
    chunk_video_frame(
        1,
        1920,
        1080,
        true,
        1000,
        &raw_frame,
        &key,
        |pkt| {
            packets.push(pkt.to_vec());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(packets.len(), 4);

    let mut assembler = FrameAssembler::new(Duration::from_millis(100));
    let now = Instant::now();
    for pkt in packets.iter().rev() {
        let (hdr, payload) = decode_video_packet(pkt, &key).unwrap();
        let (done, _) = assembler.push_chunk(hdr, payload, now);
        if let Some(frame) = done {
            assert_eq!(frame.frame_id, 1);
            assert!(frame.is_keyframe);
            assert_eq!(frame.data, raw_frame);
            return;
        }
    }
    panic!("Expected frame to assemble");
}

#[test]
fn detects_frame_gap_and_requests_keyframe() {
    let key = derive_auth_key("gap-secret");
    let mut assembler = FrameAssembler::new(Duration::from_millis(50));
    let start = Instant::now();

    let hdr1 = VideoPacketHeader {
        kind: VideoPacketKind::Chunk,
        frame_id: 1,
        chunk_idx: 0,
        chunk_count: 1,
        flags: 1,
        width: 1920,
        height: 1080,
        payload_len: 3,
        timestamp_us: 10,
    };
    let (f1, gap1) = assembler.push_chunk(hdr1, b"abc", start);
    assert!(f1.is_some());
    assert!(!gap1);

    let hdr3 = VideoPacketHeader {
        kind: VideoPacketKind::Chunk,
        frame_id: 3,
        chunk_idx: 0,
        chunk_count: 1,
        flags: 0,
        width: 1920,
        height: 1080,
        payload_len: 3,
        timestamp_us: 20,
    };
    let (f3, gap3) = assembler.push_chunk(hdr3, b"xyz", start + Duration::from_millis(16));
    assert!(f3.is_some());
    assert!(gap3);
    let _ = key;
}
