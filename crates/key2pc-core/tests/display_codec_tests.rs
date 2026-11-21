use std::time::{Duration, Instant};

use key2pc_core::display::{
    enumerate_monitors, select_monitor_index, CaptureResult, DecodedFrame, DisplayViewer,
    DxgiScreenCapturer, MfH264Decoder, MfH264Encoder, ScreenCapturer, VideoDecoder, VideoEncoder,
    ViewerOptions,
};
use key2pc_core::protocol::{chunk_video_frame, decode_video_packet, derive_auth_key, FrameAssembler};

#[test]
fn selects_secondary_monitor_automatically_when_available() {
    assert_eq!(select_monitor_index("auto", 0), 0);
    assert_eq!(select_monitor_index("auto", 1), 0);
    assert_eq!(select_monitor_index("auto", 2), 1);
    assert_eq!(select_monitor_index("0", 2), 0);
    assert_eq!(select_monitor_index("5", 2), 1);
}

#[test]
fn enumerates_desktop_monitors_on_windows() {
    let monitors = enumerate_monitors().unwrap();
    for mon in monitors {
        assert!(mon.bounds.width > 0);
        assert!(mon.bounds.height > 0);
    }
}

#[test]
fn encodes_chunks_reassembles_and_decodes_1080p_h264_frames() {
    let width = 1920u32;
    let height = 1080u32;
    let mut encoder = MfH264Encoder::new(width, height, 60, 16000).unwrap();
    let mut decoder = MfH264Decoder::new(width, height).unwrap();
    let key = derive_auth_key("codec-e2e-secret");
    let mut assembler = FrameAssembler::new(Duration::from_millis(100));

    let mut nv12 = vec![128u8; (width * height * 3 / 2) as usize];
    let mut total_decoded = 0usize;

    for frame_idx in 0u32..10 {
        let y_fill = ((frame_idx * 25) % 220) as u8;
        nv12[..(width * height) as usize].fill(y_fill);

        let pts = i64::from(frame_idx) * 166_666;
        let encoded = encoder.encode_nv12(&nv12, pts).unwrap();
        for enc_frame in encoded {
            let mut chunks = Vec::new();
            chunk_video_frame(
                frame_idx + 1,
                enc_frame.width as u16,
                enc_frame.height as u16,
                enc_frame.is_keyframe,
                u64::from(frame_idx) * 16_666,
                &enc_frame.data,
                &key,
                |pkt| {
                    chunks.push(pkt.to_vec());
                    Ok(())
                },
            )
            .unwrap();

            for pkt in chunks {
                let (hdr, payload) = decode_video_packet(&pkt, &key).unwrap();
                let (assembled_opt, _) = assembler.push_chunk(hdr, payload, Instant::now());
                if let Some(assembled) = assembled_opt {
                    let decoded = decoder.decode_annex_b(&assembled.data, pts).unwrap();
                    for df in decoded {
                        assert_eq!(df.width, width);
                        assert_eq!(df.height, height);
                        assert_eq!(df.nv12_data.len(), (width * height * 3 / 2) as usize);
                        total_decoded += 1;
                    }
                }
            }
        }
    }

    assert!(total_decoded >= 1);
}

#[test]
fn viewer_initializes_renders_and_shuts_down_cleanly() {
    let (viewer, _rx) = DisplayViewer::start(ViewerOptions {
        initial_width: 1280,
        initial_height: 720,
        start_visible: false,
        start_fullscreen: false,
        capture_mouse: true,
    })
    .unwrap();

    viewer.push_frame(DecodedFrame {
        nv12_data: vec![64u8; 1280 * 720 * 3 / 2],
        width: 1280,
        height: 720,
    });
    viewer.shutdown();
}

#[test]
fn captures_live_dxgi_desktop_when_session_attached() {
    let Ok(mut capturer) = DxgiScreenCapturer::new("auto", 1920, 1080) else {
        return;
    };
    let mut nv12 = Vec::new();
    match capturer.capture_nv12(100, &mut nv12).unwrap() {
        CaptureResult::Captured => {
            assert_eq!(nv12.len(), 1920 * 1080 * 3 / 2);
            let mut encoder = MfH264Encoder::new(1920, 1080, 60, 16000).unwrap();
            let frames = encoder.encode_nv12(&nv12, 0).unwrap();
            assert!(!frames.is_empty());
        }
        CaptureResult::Timeout | CaptureResult::Reinitialized => {}
    }
}
