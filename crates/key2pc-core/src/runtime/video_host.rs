use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use crate::config::AppConfig;
use crate::display::{
    CaptureResult, DisplayViewer, DxgiScreenCapturer, MfH264Decoder, MfH264Encoder, ScreenCapturer,
    VideoDecoder, VideoEncoder, ViewerCommand, ViewerEvent, ViewerOptions,
};
use crate::error::Result;
use crate::input::{boost_current_thread_priority, DesktopBounds, HookEvent};
use crate::network::{create_tuned_udp_socket, UdpSocketConfig};
use crate::protocol::{
    chunk_video_frame, decode_video_packet, derive_auth_key, encode_video_packet, FrameAssembler,
    VideoPacketHeader, VideoPacketKind,
};
use crate::runtime::is_timeout_error;

struct StreamPeerState {
    viewer_addr: Option<SocketAddr>,
    last_hello_at: Option<Instant>,
    force_keyframe: bool,
}

pub struct VideoStreamerHost {
    running: Arc<AtomicBool>,
    rx_handle: Option<JoinHandle<()>>,
    tx_handle: Option<JoinHandle<()>>,
}

impl VideoStreamerHost {
    pub fn start(
        config: &AppConfig,
        capture_epoch: Arc<AtomicU64>,
        bounds_callback: Arc<dyn Fn(DesktopBounds) + Send + Sync>,
    ) -> Result<Self> {
        let bind_ip: IpAddr = config
            .network
            .bind_ip
            .parse()
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        let allowed_ip: Option<IpAddr> = config
            .receiver
            .allow_only_ip
            .as_deref()
            .and_then(|s| s.parse().ok());

        let socket = Arc::new(create_tuned_udp_socket(&UdpSocketConfig {
            bind_addr: SocketAddr::new(bind_ip, config.display.video_port),
            low_delay_tos: config.network.low_delay_tos,
            broadcast: false,
            read_timeout: Some(Duration::from_millis(120)),
            buffer_size: 1_048_576,
        })?);

        let running = Arc::new(AtomicBool::new(true));
        let peer_state = Arc::new(Mutex::new(StreamPeerState {
            viewer_addr: None,
            last_hello_at: None,
            force_keyframe: true,
        }));
        let auth_key = derive_auth_key(&config.network.shared_secret);

        let rx_running = Arc::clone(&running);
        let rx_socket = Arc::clone(&socket);
        let rx_state = Arc::clone(&peer_state);
        let rx_handle = thread::Builder::new()
            .name("key2pc-video-srv-rx".to_string())
            .spawn(move || {
                let mut buf = [0u8; 256];
                while rx_running.load(Ordering::Acquire) {
                    match rx_socket.recv_from(&mut buf) {
                        Ok((len, src)) => {
                            if let Some(allowed) = allowed_ip {
                                if src.ip() != allowed {
                                    continue;
                                }
                            }
                            let Ok((hdr, _)) = decode_video_packet(&buf[..len], &auth_key) else {
                                continue;
                            };
                            let mut guard = rx_state.lock().unwrap();
                            match hdr.kind {
                                VideoPacketKind::StreamHello => {
                                    if guard.viewer_addr != Some(src) {
                                        guard.force_keyframe = true;
                                    }
                                    guard.viewer_addr = Some(src);
                                    guard.last_hello_at = Some(Instant::now());
                                }
                                VideoPacketKind::KeyframeReq => {
                                    guard.force_keyframe = true;
                                }
                                VideoPacketKind::StreamStop if guard.viewer_addr == Some(src) => {
                                    guard.viewer_addr = None;
                                    guard.last_hello_at = None;
                                }
                                _ => {}
                            }
                        }
                        Err(err) if is_timeout_error(&err) => {}
                        Err(_) => thread::sleep(Duration::from_millis(10)),
                    }
                }
            })?;

        let tx_running = Arc::clone(&running);
        let tx_cfg = config.display.clone();
        let tx_epoch = Arc::clone(&capture_epoch);
        let tx_handle = thread::Builder::new()
            .name("key2pc-video-srv-tx".to_string())
            .spawn(move || {
                boost_current_thread_priority(false);
                run_video_streamer_loop(
                    tx_running,
                    socket,
                    peer_state,
                    auth_key,
                    tx_cfg,
                    tx_epoch,
                    bounds_callback,
                );
            })?;

        Ok(Self {
            running,
            rx_handle: Some(rx_handle),
            tx_handle: Some(tx_handle),
        })
    }

    pub fn shutdown(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(h) = self.tx_handle.take() {
            let _ = h.join();
        }
        if let Some(h) = self.rx_handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for VideoStreamerHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(15);

#[allow(clippy::too_many_arguments)]
fn send_video_control(
    socket: &UdpSocket,
    peer: SocketAddr,
    kind: VideoPacketKind,
    width: u16,
    height: u16,
    timestamp_us: u64,
    auth_key: &[u8; 32],
    control_buf: &mut Vec<u8>,
) {
    let hdr = VideoPacketHeader::control(kind, width, height, timestamp_us);
    if encode_video_packet(&hdr, &[], auth_key, control_buf).is_ok() {
        let _ = socket.send_to(control_buf, peer);
    }
}

fn run_video_streamer_loop(
    running: Arc<AtomicBool>,
    socket: Arc<UdpSocket>,
    peer_state: Arc<Mutex<StreamPeerState>>,
    auth_key: [u8; 32],
    cfg: crate::config::DisplayConfig,
    capture_epoch: Arc<AtomicU64>,
    bounds_callback: Arc<dyn Fn(DesktopBounds) + Send + Sync>,
) {
    let mut capturer: Option<DxgiScreenCapturer> = None;
    let mut encoder: Option<MfH264Encoder> = None;
    let mut encoded_size = (0u32, 0u32);
    let mut observed_epoch = capture_epoch.load(Ordering::Acquire);
    let mut nv12_buf = Vec::with_capacity((cfg.width * cfg.height * 3 / 2) as usize);
    let mut frame_id = 1u32;
    let epoch = Instant::now();
    let frame_interval = Duration::from_micros((1_000_000 / u64::from(cfg.fps)).max(4000));
    let mut last_sent_at = Instant::now() - frame_interval;
    let mut idle_capture_started_at = Instant::now();
    let mut pending_keyframe = true;
    let mut logged_error = false;

    while running.load(Ordering::Acquire) {
        let current_epoch = capture_epoch.load(Ordering::Acquire);
        if current_epoch != observed_epoch {
            observed_epoch = current_epoch;
            capturer = None;
            encoder = None;
            nv12_buf.clear();
            pending_keyframe = true;
            idle_capture_started_at = Instant::now();
            if let Ok(mut guard) = peer_state.lock() {
                guard.force_keyframe = true;
            }
        }

        let (viewer_opt, force_idr) = {
            let mut guard = peer_state.lock().unwrap();
            if let Some(last_hello) = guard.last_hello_at {
                if last_hello.elapsed() >= STREAM_IDLE_TIMEOUT {
                    guard.viewer_addr = None;
                    guard.last_hello_at = None;
                }
            }
            let force = guard.force_keyframe;
            guard.force_keyframe = false;
            (guard.viewer_addr, force)
        };
        if force_idr {
            pending_keyframe = true;
        }

        let Some(viewer_addr) = viewer_opt else {
            capturer = None;
            encoder = None;
            encoded_size = (0, 0);
            pending_keyframe = true;
            thread::sleep(Duration::from_millis(40));
            continue;
        };

        if capturer.is_none() {
            match DxgiScreenCapturer::new_with_mode(
                &cfg.monitor,
                cfg.width,
                cfg.height,
                cfg.match_source_resolution,
            ) {
                Ok(cap) => {
                    bounds_callback(cap.bounds());
                    if cap.target_size() != encoded_size {
                        encoder = None;
                    }
                    capturer = Some(cap);
                    pending_keyframe = true;
                    idle_capture_started_at = Instant::now();
                    logged_error = false;
                }
                Err(err) => {
                    if !logged_error {
                        crate::runtime::write_error_log(None, &err.to_string());
                        logged_error = true;
                    }
                    thread::sleep(Duration::from_millis(150));
                    continue;
                }
            }
        }
        if encoder.is_none() {
            let (tw, th) = capturer.as_ref().unwrap().target_size();
            match MfH264Encoder::new(tw, th, cfg.fps, cfg.bitrate_kbps) {
                Ok(enc) => {
                    encoder = Some(enc);
                    encoded_size = (tw, th);
                    pending_keyframe = true;
                }
                Err(err) => {
                    if !logged_error {
                        crate::runtime::write_error_log(None, &err.to_string());
                        logged_error = true;
                    }
                    thread::sleep(Duration::from_millis(150));
                    continue;
                }
            }
        }

        let cap = capturer.as_mut().unwrap();
        let enc = encoder.as_mut().unwrap();
        if pending_keyframe {
            enc.force_keyframe();
        }

        let elapsed = last_sent_at.elapsed();
        if elapsed < frame_interval {
            let rem = frame_interval - elapsed;
            if rem > Duration::from_millis(2) {
                thread::sleep(rem - Duration::from_millis(1));
            }
        }

        let timeout_ms = if nv12_buf.is_empty() {
            100
        } else {
            (frame_interval.as_millis() as u32).clamp(4, 33)
        };
        match cap.capture_nv12(timeout_ms, &mut nv12_buf) {
            Ok(CaptureResult::Captured) => {
                idle_capture_started_at = Instant::now();
            }
            Ok(CaptureResult::Timeout) => {
                if nv12_buf.is_empty()
                    && idle_capture_started_at.elapsed() >= Duration::from_millis(250)
                {
                    let _ = cap.capture_gdi_snapshot(&mut nv12_buf);
                    idle_capture_started_at = Instant::now();
                }
                let need_keepalive_refresh =
                    !nv12_buf.is_empty() && last_sent_at.elapsed() >= Duration::from_secs(2);
                if (!pending_keyframe && !need_keepalive_refresh) || nv12_buf.is_empty() {
                    continue;
                }
            }
            Ok(CaptureResult::Reinitialized) => {
                bounds_callback(cap.bounds());
                if cap.target_size() != encoded_size {
                    encoder = None;
                } else {
                    enc.force_keyframe();
                }
                pending_keyframe = true;
                nv12_buf.clear();
                idle_capture_started_at = Instant::now();
                continue;
            }
            Err(_) => {
                capturer = None;
                pending_keyframe = true;
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        }

        let pts_100ns = (epoch.elapsed().as_nanos() / 100) as i64;
        let encoded_frames = match enc.encode_nv12(&nv12_buf, pts_100ns) {
            Ok(frames) => frames,
            Err(_) => {
                encoder = None;
                pending_keyframe = true;
                continue;
            }
        };

        last_sent_at = Instant::now();
        let timestamp_us = epoch.elapsed().as_micros() as u64;
        for frame in encoded_frames {
            if frame.is_keyframe {
                pending_keyframe = false;
            }
            let mut pkt_counter = 0u32;
            let _ = chunk_video_frame(
                frame_id,
                frame.width as u16,
                frame.height as u16,
                frame.is_keyframe,
                timestamp_us,
                &frame.data,
                &auth_key,
                |pkt_bytes| {
                    let _ = socket.send_to(pkt_bytes, viewer_addr);
                    pkt_counter += 1;
                    if pkt_counter.is_multiple_of(16) {
                        thread::sleep(Duration::from_micros(120));
                    }
                    Ok(())
                },
            );
            frame_id = frame_id.wrapping_add(1);
        }
    }
}

pub struct VideoViewerHost {
    running: Arc<AtomicBool>,
    viewer: Arc<DisplayViewer>,
    peer_video_addr: Arc<Mutex<Option<SocketAddr>>>,
    rx_handle: Option<JoinHandle<()>>,
    bridge_handle: Option<JoinHandle<()>>,
}

impl VideoViewerHost {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        config: &AppConfig,
        initial_peer_video: Option<SocketAddr>,
        hook_tx: Sender<HookEvent>,
        on_focus_active: Arc<dyn Fn(bool) + Send + Sync>,
        on_visibility_changed: Arc<dyn Fn(bool) + Send + Sync>,
        on_fullscreen_changed: Arc<dyn Fn(bool) + Send + Sync>,
        on_fill_screen_changed: Arc<dyn Fn(bool) + Send + Sync>,
    ) -> Result<Self> {
        let bind_ip: IpAddr = config
            .network
            .bind_ip
            .parse()
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));

        let socket = Arc::new(create_tuned_udp_socket(&UdpSocketConfig {
            bind_addr: SocketAddr::new(bind_ip, 0),
            low_delay_tos: config.network.low_delay_tos,
            broadcast: false,
            read_timeout: Some(Duration::from_millis(25)),
            buffer_size: 4_194_304,
        })?);

        let (viewer, viewer_rx) = DisplayViewer::start(ViewerOptions {
            initial_width: config.display.width,
            initial_height: config.display.height,
            start_visible: config.display.open_viewer_on_start,
            start_fullscreen: config.display.fullscreen,
            fill_screen: config.display.fill_screen,
            capture_mouse: config.display.capture_mouse,
        })?;
        let viewer = Arc::new(viewer);

        let running = Arc::new(AtomicBool::new(true));
        let peer_video_addr = Arc::new(Mutex::new(initial_peer_video));
        let auth_key = derive_auth_key(&config.network.shared_secret);

        let rx_running = Arc::clone(&running);
        let rx_socket = Arc::clone(&socket);
        let rx_peer = Arc::clone(&peer_video_addr);
        let rx_viewer = Arc::clone(&viewer);
        let cfg_w = config.display.width;
        let cfg_h = config.display.height;

        let rx_handle = thread::Builder::new()
            .name("key2pc-video-cli-rx".to_string())
            .spawn(move || {
                boost_current_thread_priority(true);
                run_video_viewer_rx_loop(
                    rx_running, rx_socket, rx_peer, rx_viewer, auth_key, cfg_w, cfg_h,
                );
            })?;

        let bridge_running = Arc::clone(&running);
        let bridge_socket = Arc::clone(&socket);
        let bridge_peer = Arc::clone(&peer_video_addr);
        let auto_focus = config.display.auto_forward_on_viewer_focus;
        let bridge_handle = thread::Builder::new()
            .name("key2pc-video-cli-ev".to_string())
            .spawn(move || {
                run_viewer_event_bridge(
                    bridge_running,
                    viewer_rx,
                    hook_tx,
                    bridge_socket,
                    bridge_peer,
                    auth_key,
                    auto_focus,
                    on_focus_active,
                    on_visibility_changed,
                    on_fullscreen_changed,
                    on_fill_screen_changed,
                );
            })?;

        Ok(Self {
            running,
            viewer,
            peer_video_addr,
            rx_handle: Some(rx_handle),
            bridge_handle: Some(bridge_handle),
        })
    }

    pub fn set_peer_video_addr(&self, addr: Option<SocketAddr>) {
        if let Ok(mut guard) = self.peer_video_addr.lock() {
            *guard = addr;
        }
    }

    pub fn toggle_visible(&self) {
        self.viewer.send_command(ViewerCommand::ToggleVisible);
    }

    pub fn toggle_fullscreen(&self) {
        self.viewer.send_command(ViewerCommand::ToggleFullscreen);
    }

    pub fn toggle_fill_screen(&self) {
        self.viewer.send_command(ViewerCommand::ToggleFillScreen);
    }

    pub fn shutdown(&mut self) {
        self.running.store(false, Ordering::Release);
        self.viewer.shutdown();
        if let Some(h) = self.rx_handle.take() {
            let _ = h.join();
        }
        if let Some(h) = self.bridge_handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for VideoViewerHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_video_viewer_rx_loop(
    running: Arc<AtomicBool>,
    socket: Arc<UdpSocket>,
    peer_addr_slot: Arc<Mutex<Option<SocketAddr>>>,
    viewer: Arc<DisplayViewer>,
    auth_key: [u8; 32],
    default_w: u32,
    default_h: u32,
) {
    let mut decoder: Option<MfH264Decoder> = None;
    let mut decoded_size = (0u32, 0u32);
    let mut assembler = FrameAssembler::new(Duration::from_millis(250));
    let mut buf = vec![0u8; 2048];
    let mut control_buf = Vec::with_capacity(64);
    let mut last_hello = Instant::now() - Duration::from_secs(2);
    let mut last_keyframe_req = Instant::now() - Duration::from_secs(2);
    let mut last_frame_rx = Instant::now();
    let epoch = Instant::now();
    let mut was_visible = false;
    let mut has_keyframe = false;

    while running.load(Ordering::Acquire) {
        let is_visible = viewer.is_visible();
        let peer_opt = *peer_addr_slot.lock().unwrap();

        if !is_visible {
            if was_visible {
                if let Some(peer) = peer_opt {
                    send_video_control(
                        &socket,
                        peer,
                        VideoPacketKind::StreamStop,
                        0,
                        0,
                        0,
                        &auth_key,
                        &mut control_buf,
                    );
                }
                assembler.reset();
                has_keyframe = false;
                was_visible = false;
            }
            thread::sleep(Duration::from_millis(40));
            continue;
        }
        was_visible = true;

        if let Some(peer) = peer_opt {
            if !has_keyframe {
                viewer.set_status_text(format!(
                    "Connected to {}\nWaiting for {}x{} stream on UDP {}...",
                    peer.ip(),
                    default_w,
                    default_h,
                    peer.port()
                ));
            }
            if last_hello.elapsed() >= Duration::from_millis(350) {
                last_hello = Instant::now();
                send_video_control(
                    &socket,
                    peer,
                    VideoPacketKind::StreamHello,
                    default_w as u16,
                    default_h as u16,
                    epoch.elapsed().as_micros() as u64,
                    &auth_key,
                    &mut control_buf,
                );
            }
            if has_keyframe
                && last_frame_rx.elapsed() >= Duration::from_secs(3)
                && last_keyframe_req.elapsed() >= Duration::from_millis(850)
            {
                last_keyframe_req = Instant::now();
                send_video_control(
                    &socket,
                    peer,
                    VideoPacketKind::KeyframeReq,
                    0,
                    0,
                    0,
                    &auth_key,
                    &mut control_buf,
                );
            }
        } else {
            viewer.set_status_text(
                "Searching for PC on local network...\nEnsure Key2PC is running on PC with role: receiver",
            );
            has_keyframe = false;
        }

        let mut packets_handled = 0u32;
        while packets_handled < 32 {
            match socket.recv_from(&mut buf) {
                Ok((len, src)) => {
                    packets_handled += 1;
                    let Ok((hdr, payload)) = decode_video_packet(&buf[..len], &auth_key) else {
                        continue;
                    };
                    if let Some(expected_peer) = peer_opt {
                        if src.ip() != expected_peer.ip() {
                            continue;
                        }
                    }
                    let (frame_opt, needs_keyframe) =
                        assembler.push_chunk(hdr, payload, Instant::now());
                    let req_cooldown = if has_keyframe {
                        Duration::from_millis(850)
                    } else {
                        Duration::from_millis(350)
                    };
                    if needs_keyframe && last_keyframe_req.elapsed() >= req_cooldown {
                        if let Some(peer) = peer_opt {
                            last_keyframe_req = Instant::now();
                            send_video_control(
                                &socket,
                                peer,
                                VideoPacketKind::KeyframeReq,
                                0,
                                0,
                                0,
                                &auth_key,
                                &mut control_buf,
                            );
                        }
                    }

                    if let Some(assembled) = frame_opt {
                        if assembled.is_keyframe {
                            has_keyframe = true;
                        } else if !has_keyframe {
                            if last_keyframe_req.elapsed() >= Duration::from_millis(350) {
                                if let Some(peer) = peer_opt {
                                    last_keyframe_req = Instant::now();
                                    send_video_control(
                                        &socket,
                                        peer,
                                        VideoPacketKind::KeyframeReq,
                                        0,
                                        0,
                                        0,
                                        &auth_key,
                                        &mut control_buf,
                                    );
                                }
                            }
                            continue;
                        }

                        last_frame_rx = Instant::now();
                        let fw = if assembled.width >= 2 {
                            u32::from(assembled.width)
                        } else {
                            default_w
                        };
                        let fh = if assembled.height >= 2 {
                            u32::from(assembled.height)
                        } else {
                            default_h
                        };
                        if decoder.is_none() || decoded_size != (fw, fh) {
                            decoder = MfH264Decoder::new(fw, fh).ok();
                            decoded_size = (fw, fh);
                        }
                        if let Some(dec) = decoder.as_mut() {
                            let pts = (assembled.timestamp_us as i64).saturating_mul(10);
                            match dec.decode_annex_b(&assembled.data, pts) {
                                Ok(decoded_frames) => {
                                    for df in decoded_frames {
                                        viewer.push_frame(df);
                                    }
                                }
                                Err(_) => {
                                    decoder = None;
                                    has_keyframe = false;
                                    if let Some(peer) = peer_opt {
                                        last_keyframe_req = Instant::now();
                                        send_video_control(
                                            &socket,
                                            peer,
                                            VideoPacketKind::KeyframeReq,
                                            0,
                                            0,
                                            0,
                                            &auth_key,
                                            &mut control_buf,
                                        );
                                    }
                                }
                            }
                        }
                        break;
                    }
                }
                Err(err) if is_timeout_error(&err) => break,
                Err(_) => {
                    thread::sleep(Duration::from_millis(5));
                    break;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_viewer_event_bridge(
    running: Arc<AtomicBool>,
    viewer_rx: Receiver<ViewerEvent>,
    hook_tx: Sender<HookEvent>,
    socket: Arc<UdpSocket>,
    peer_addr_slot: Arc<Mutex<Option<SocketAddr>>>,
    auth_key: [u8; 32],
    auto_focus: bool,
    on_focus_active: Arc<dyn Fn(bool) + Send + Sync>,
    on_visibility_changed: Arc<dyn Fn(bool) + Send + Sync>,
    on_fullscreen_changed: Arc<dyn Fn(bool) + Send + Sync>,
    on_fill_screen_changed: Arc<dyn Fn(bool) + Send + Sync>,
) {
    let mut control_buf = Vec::with_capacity(64);
    while running.load(Ordering::Acquire) {
        match viewer_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ViewerEvent::Mouse(mouse)) => {
                let _ = hook_tx.try_send(HookEvent::Mouse(mouse));
            }
            Ok(ViewerEvent::FocusChanged(focused)) => {
                if auto_focus {
                    on_focus_active(focused);
                }
            }
            Ok(ViewerEvent::FullscreenChanged(fullscreen)) => {
                on_fullscreen_changed(fullscreen);
            }
            Ok(ViewerEvent::FillScreenChanged(fill_screen)) => {
                on_fill_screen_changed(fill_screen);
            }
            Ok(ViewerEvent::VisibilityChanged(visible)) => {
                on_visibility_changed(visible);
                if visible {
                    if let Some(peer) = *peer_addr_slot.lock().unwrap() {
                        send_video_control(
                            &socket,
                            peer,
                            VideoPacketKind::StreamHello,
                            0,
                            0,
                            0,
                            &auth_key,
                            &mut control_buf,
                        );
                    }
                } else {
                    if auto_focus {
                        on_focus_active(false);
                    }
                    if let Some(peer) = *peer_addr_slot.lock().unwrap() {
                        send_video_control(
                            &socket,
                            peer,
                            VideoPacketKind::StreamStop,
                            0,
                            0,
                            0,
                            &auth_key,
                            &mut control_buf,
                        );
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
}
