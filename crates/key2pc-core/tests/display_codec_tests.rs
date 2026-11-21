use std::time::{Duration, Instant};

use key2pc_core::display::{
    composite_dxgi_pointer_over_bgra, compute_encode_dimensions, enumerate_monitors,
    inspect_virtual_driver_status, is_virtual_display_signature, select_best_display_mode_index,
    select_best_monitor, select_monitor_index, CaptureResult, DecodedFrame, DisplayViewer,
    DxgiScreenCapturer, MfH264Decoder, MfH264Encoder, MonitorInfo, PointerShapeDesc,
    ScreenCapturer, VideoDecoder, VideoEncoder, ViewerCommand, ViewerEvent, ViewerOptions,
};
use key2pc_core::input::DesktopBounds;
use key2pc_core::protocol::{
    chunk_video_frame, decode_video_packet, derive_auth_key, FrameAssembler,
};

#[test]
fn selects_secondary_monitor_automatically_when_available() {
    assert_eq!(select_monitor_index("auto", 0), 0);
    assert_eq!(select_monitor_index("auto", 1), 0);
    assert_eq!(select_monitor_index("auto", 2), 1);
    assert_eq!(select_monitor_index("0", 2), 0);
    assert_eq!(select_monitor_index("5", 2), 1);
}

#[test]
fn select_best_monitor_prefers_virtual_and_non_primary() {
    let monitors = vec![
        MonitorInfo {
            index: 0,
            name: "\\\\.\\DISPLAY1".to_string(),
            bounds: DesktopBounds::new(0, 0, 1920, 1080),
            is_primary: true,
            is_virtual: false,
        },
        MonitorInfo {
            index: 1,
            name: "\\\\.\\DISPLAY2".to_string(),
            bounds: DesktopBounds::new(1920, 0, 2560, 1440),
            is_primary: false,
            is_virtual: false,
        },
        MonitorInfo {
            index: 2,
            name: "\\\\.\\DISPLAY3".to_string(),
            bounds: DesktopBounds::new(4480, 0, 1920, 1080),
            is_primary: false,
            is_virtual: true,
        },
    ];
    assert_eq!(select_best_monitor("auto", &monitors), 2);
    assert_eq!(select_best_monitor("0", &monitors), 0);
    assert_eq!(select_best_monitor("1", &monitors), 1);
}

#[test]
fn detects_virtual_display_signatures_and_picks_best_mode() {
    assert!(is_virtual_display_signature(
        "\\\\.\\DISPLAY2",
        "Parsec Virtual Display Adapter",
        "MONITOR\\PSCCDD0"
    ));
    assert!(is_virtual_display_signature(
        "\\\\.\\DISPLAY3",
        "VDD by MTT",
        "ROOT\\MttVDD"
    ));
    assert!(!is_virtual_display_signature(
        "\\\\.\\DISPLAY1",
        "AMD Radeon 780M Graphics",
        "DISPLAY\\LEN1234"
    ));

    let modes = vec![
        (1280, 720, 60),
        (1920, 1080, 30),
        (1920, 1080, 60),
        (2560, 1440, 60),
    ];
    assert_eq!(
        select_best_display_mode_index(&modes, 1920, 1080, 60),
        Some(2)
    );
    assert_eq!(
        select_best_display_mode_index(&modes, 1920, 1080, 120),
        Some(2)
    );
    assert_eq!(select_best_display_mode_index(&[], 1920, 1080, 60), None);
    let _ = inspect_virtual_driver_status();
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
    let (viewer, rx) = DisplayViewer::start(ViewerOptions {
        initial_width: 1280,
        initial_height: 720,
        start_visible: false,
        start_fullscreen: false,
        fill_screen: false,
        capture_mouse: true,
    })
    .unwrap();

    viewer.push_frame(DecodedFrame {
        nv12_data: vec![64u8; 1280 * 720 * 3 / 2],
        width: 1280,
        height: 720,
    });
    viewer.push_frame(DecodedFrame {
        nv12_data: vec![96u8; 1920 * 1080 * 3 / 2],
        width: 1920,
        height: 1080,
    });
    viewer.send_command(ViewerCommand::SetFullscreen(true));
    let mut saw_fullscreen = false;
    for _ in 0..20 {
        if let Ok(ViewerEvent::FullscreenChanged(true)) = rx.recv_timeout(Duration::from_millis(50))
        {
            saw_fullscreen = true;
            break;
        }
    }
    assert!(saw_fullscreen);
    assert!(viewer.is_fullscreen());
    viewer.send_command(ViewerCommand::ToggleFillScreen);
    let mut saw_fill_screen = false;
    for _ in 0..20 {
        if let Ok(ViewerEvent::FillScreenChanged(true)) = rx.recv_timeout(Duration::from_millis(50))
        {
            saw_fill_screen = true;
            break;
        }
    }
    assert!(saw_fill_screen);
    assert!(viewer.is_fill_screen());
    viewer.send_command(ViewerCommand::SetFullscreen(false));
    std::thread::sleep(Duration::from_millis(50));
    viewer.shutdown();
}

#[test]
fn captures_live_dxgi_desktop_when_session_attached() {
    let mut capturer = match DxgiScreenCapturer::new("auto", 1920, 1080) {
        Ok(cap) => cap,
        Err(err) => {
            println!("DXGI init skipped/err: {err}");
            return;
        }
    };
    let mut nv12 = Vec::new();
    match capturer.capture_nv12(200, &mut nv12) {
        Ok(CaptureResult::Captured) => {
            assert_eq!(nv12.len(), 1920 * 1080 * 3 / 2);
            let y_plane = &nv12[..1920 * 1080];
            let max_y = y_plane.iter().copied().max().unwrap_or(0);
            assert!(max_y > 0);

            let mut encoder = MfH264Encoder::new(1920, 1080, 60, 16000).unwrap();
            let mut decoder = MfH264Decoder::new(1920, 1080).unwrap();
            let frames = encoder.encode_nv12(&nv12, 0).unwrap();
            assert!(!frames.is_empty());
            for ef in frames {
                let decoded = decoder.decode_annex_b(&ef.data, 0).unwrap();
                for df in decoded {
                    assert_eq!(df.width, 1920);
                    assert_eq!(df.height, 1080);
                }
            }
        }
        Ok(CaptureResult::Timeout | CaptureResult::Reinitialized) | Err(_) => {}
    }
}

#[test]
fn handles_dynamic_resolution_switch_without_blocking() {
    for (w, h) in [(1920u32, 1080u32), (1280u32, 720u32)] {
        let mut encoder = MfH264Encoder::new(w, h, 60, 12000).unwrap();
        let mut decoder = MfH264Decoder::new(w, h).unwrap();
        let nv12 = vec![120u8; (w * h * 3 / 2) as usize];
        let mut got_decoded = false;
        for idx in 0..4 {
            let encoded = encoder
                .encode_nv12(&nv12, i64::from(idx) * 166_666)
                .unwrap();
            for frame in encoded {
                let decoded = decoder
                    .decode_annex_b(&frame.data, i64::from(idx) * 166_666)
                    .unwrap();
                for df in decoded {
                    assert_eq!(df.width, w);
                    assert_eq!(df.height, h);
                    got_decoded = true;
                }
            }
        }
        assert!(got_decoded);
    }
}

#[test]
fn computes_native_source_aspect_encode_dimensions() {
    assert_eq!(
        compute_encode_dimensions(1920, 1200, 1920, 1080, true),
        (1920, 1200)
    );
    assert_eq!(
        compute_encode_dimensions(1920, 1200, 1920, 1080, false),
        (1920, 1080)
    );
    assert_eq!(
        compute_encode_dimensions(2560, 1600, 1920, 1080, true),
        (2560, 1600)
    );
    assert_eq!(
        compute_encode_dimensions(3840, 2400, 1920, 1080, true),
        (2560, 1600)
    );
}

#[test]
fn composites_dxgi_monochrome_and_color_pointers_onto_bgra() {
    let mut frame = vec![40u8; 4 * 4 * 4];
    let mono_shape = PointerShapeDesc {
        pointer_type: 1,
        width: 4,
        height: 2,
        pitch: 1,
    };
    let mono_data = [0b0110_0000u8, 0b0011_0000u8];
    composite_dxgi_pointer_over_bgra(&mut frame, 4, 4, 16, 0, 0, 0, 0, &mono_shape, &mono_data);
    assert_eq!(&frame[0..4], &[0, 0, 0, 255]);
    assert_eq!(&frame[4..8], &[40, 40, 40, 40]);
    assert_eq!(&frame[8..12], &[40 ^ 0xFF, 40 ^ 0xFF, 40 ^ 0xFF, 40]);
    assert_eq!(&frame[12..16], &[255, 255, 255, 255]);

    let color_shape = PointerShapeDesc {
        pointer_type: 2,
        width: 1,
        height: 1,
        pitch: 4,
    };
    let color_data = [10u8, 200u8, 30u8, 255u8];
    composite_dxgi_pointer_over_bgra(&mut frame, 4, 4, 16, 0, 0, 1, 1, &color_shape, &color_data);
    let idx = (4 + 1) * 4;
    assert_eq!(&frame[idx..idx + 4], &[10, 200, 30, 255]);

    let masked_shape = PointerShapeDesc {
        pointer_type: 4,
        width: 2,
        height: 1,
        pitch: 8,
    };
    let masked_data = [20u8, 30u8, 40u8, 0u8, 0xFF, 0x00, 0xFF, 1u8];
    composite_dxgi_pointer_over_bgra(
        &mut frame,
        4,
        4,
        16,
        0,
        0,
        2,
        2,
        &masked_shape,
        &masked_data,
    );
    let m0 = (2 * 4 + 2) * 4;
    let m1 = (2 * 4 + 3) * 4;
    assert_eq!(&frame[m0..m0 + 4], &[20, 30, 40, 255]);
    assert_eq!(&frame[m1..m1 + 4], &[40 ^ 0xFF, 40, 40 ^ 0xFF, 40]);
}

#[test]
fn encodes_and_decodes_1920x1200_h264_frames() {
    let width = 1920u32;
    let height = 1200u32;
    let mut encoder = MfH264Encoder::new(width, height, 60, 16000).unwrap();
    let mut decoder = MfH264Decoder::new(width, height).unwrap();
    let nv12 = vec![128u8; (width * height * 3 / 2) as usize];
    let mut decoded_count = 0usize;
    for idx in 0..5 {
        let encoded = encoder
            .encode_nv12(&nv12, i64::from(idx) * 166_666)
            .unwrap();
        for ef in encoded {
            let decoded = decoder
                .decode_annex_b(&ef.data, i64::from(idx) * 166_666)
                .unwrap();
            for df in decoded {
                assert_eq!(df.width, width);
                assert_eq!(df.height, height);
                decoded_count += 1;
            }
        }
    }
    assert!(decoded_count >= 1);
}
