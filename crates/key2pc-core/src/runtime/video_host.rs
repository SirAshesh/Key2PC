use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use crate::config::AppConfig;
use crate::display::{
    CaptureResult, DisplayViewer, DxgiScreenCapturer, MfH264Decoder, MfH264Encoder,
    ScreenCapturer, VideoDecoder, VideoEncoder, ViewerCommand, ViewerEvent, ViewerOptions,
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
            buffer_size: 524_288,
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
                                VideoPacketKind::StreamStop => {
                                    if guard.viewer_addr == Some(src) {
                                        guard.viewer_addr = None;
                                        guard.last_hello_at = None;
                                    }
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

fn run_video_streamer_loop(
    running: Arc<AtomicBool>,
    socket: Arc<UdpSocket>,
    peer_state: Arc<Mutex<StreamPeerState>>,
    auth_key: [u8; 32],
    cfg: crate::config::DisplayConfig,
    bounds_callback: Arc<dyn Fn(DesktopBounds) + Send + Sync>,
) {
    let mut capturer: Option<DxgiScreenCapturer> = None;
    let mut encoder: Option<MfH264Encoder> = None;
    let mut nv12_buf = Vec::with_capacity((cfg.width * cfg.height * 3 / 2) as usize);
    let mut frame_id = 1u32;
    let epoch = Instant::now();
    let frame_interval = Duration::from_micros((1_000_000 / u64::from(cfg.fps)).max(4000));
    let mut last_sent_at = Instant::now() - frame_interval;

    while running.load(Ordering::Acquire) {
        let (viewer_opt, force_idr) = {
            let mut guard = peer_state.lock().unwrap();
            if let Some(last_hello) = guard.last_hello_at {
                if last_hello.elapsed() >= Duration::from_secs(3) {
                    guard.viewer_addr = None;
                    guard.last_hello_at = None;
                }
            }
            let force = guard.force_keyframe;
            guard.force_keyframe = false;
            (guard.viewer_addr, force)
        };

        let Some(viewer_addr) = viewer_opt else {
            capturer = None;
            encoder = None;
            thread::sleep(Duration::from_millis(40));
            continue;
        };

        if capturer.is_none() {
            match DxgiScreenCapturer::new(&cfg.monitor, cfg.width, cfg.height) {
                Ok(cap) => {
                    bounds_callback(cap.bounds());
                    capturer = Some(cap);
                }
                Err(_) => {
                    thread::sleep(Duration::from_millis(250));
                    continue;
                }
            }
        }
        if encoder.is_none() {
            let (tw, th) = capturer.as_ref().unwrap().target_size();
            match MfH264Encoder::new(tw, th, cfg.fps, cfg.bitrate_kbps) {
                Ok(enc) => encoder = Some(enc),
                Err(_) => {
                    thread::sleep(Duration::from_millis(250));
                    continue;
                }
            }
        }

        let cap = capturer.as_mut().unwrap();
        let enc = encoder.as_mut().unwrap();
        if force_idr {
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
            Ok(CaptureResult::Captured) => {}
            Ok(CaptureResult::Timeout) => {
                if !force_idr || nv12_buf.is_empty() {
                    continue;
                }
            }
            Ok(CaptureResult::Reinitialized) => {
                bounds_callback(cap.bounds());
                enc.force_keyframe();
                continue;
            }
            Err(_) => {
                capturer = None;
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        }

        let pts_100ns = (epoch.elapsed().as_nanos() / 100) as i64;
        let encoded_frames = match enc.encode_nv12(&nv12_buf, pts_100ns) {
            Ok(frames) => frames,
            Err(_) => {
                encoder = None;
                continue;
            }
        };

        last_sent_at = Instant::now();
        let timestamp_us = epoch.elapsed().as_micros() as u64;
        for frame in encoded_frames {
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
    pub fn start(
        config: &AppConfig,
        initial_peer_video: Option<SocketAddr>,
        hook_tx: Sender<HookEvent>,
        on_focus_active: Arc<dyn Fn(bool) + Send + Sync>,
        on_visibility_changed: Arc<dyn Fn(bool) + Send + Sync>,
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
            read_timeout: Some(Duration::from_millis(50)),
            buffer_size: 2_097_152,
        })?);

        let (viewer, viewer_rx) = DisplayViewer::start(ViewerOptions {
            initial_width: config.display.width,
            initial_height: config.display.height,
            start_visible: config.display.open_viewer_on_start,
            start_fullscreen: config.display.fullscreen,
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
                    rx_running,
                    rx_socket,
                    rx_peer,
                    rx_viewer,
                    auth_key,
                    cfg_w,
                    cfg_h,
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
    let mut assembler = FrameAssembler::new(Duration::from_millis(40));
    let mut buf = vec![0u8; 2048];
    let mut control_buf = Vec::with_capacity(64);
    let mut last_hello = Instant::now() - Duration::from_secs(2);
    let mut last_keyframe_req = Instant::now() - Duration::from_secs(2);
    let epoch = Instant::now();
    let mut was_visible = false;

    while running.load(Ordering::Acquire) {
        let is_visible = viewer.is_visible();
        let peer_opt = *peer_addr_slot.lock().unwrap();

        if !is_visible {
            if was_visible {
                if let Some(peer) = peer_opt {
                    let hdr = VideoPacketHeader::control(VideoPacketKind::StreamStop, 0, 0, 0);
                    if encode_video_packet(&hdr, &[], &auth_key, &mut control_buf).is_ok() {
                        let _ = socket.send_to(&control_buf, peer);
                    }
                }
                assembler.reset();
                was_visible = false;
            }
            thread::sleep(Duration::from_millis(40));
            continue;
        }
        was_visible = true;

        if let Some(peer) = peer_opt {
            if last_hello.elapsed() >= Duration::from_millis(450) {
                last_hello = Instant::now();
                let hdr = VideoPacketHeader::control(
                    VideoPacketKind::StreamHello,
                    default_w as u16,
                    default_h as u16,
                    epoch.elapsed().as_micros() as u64,
                );
                if encode_video_packet(&hdr, &[], &auth_key, &mut control_buf).is_ok() {
                    let _ = socket.send_to(&control_buf, peer);
                }
            }
        }

        match socket.recv_from(&mut buf) {
            Ok((len, src)) => {
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
                if needs_keyframe && last_keyframe_req.elapsed() >= Duration::from_millis(250) {
                    if let Some(peer) = peer_opt {
                        last_keyframe_req = Instant::now();
                        let req = VideoPacketHeader::control(VideoPacketKind::KeyframeReq, 0, 0, 0);
                        if encode_video_packet(&req, &[], &auth_key, &mut control_buf).is_ok() {
                            let _ = socket.send_to(&control_buf, peer);
                        }
                    }
                }

                if let Some(assembled) = frame_opt {
                    let fw = u32::from(assembled.width).max(default_w);
                    let fh = u32::from(assembled.height).max(default_h);
                    if decoder.is_none() || decoded_size != (fw, fh) {
                        decoder = MfH264Decoder::new(fw, fh).ok();
                        decoded_size = (fw, fh);
                    }
                    if let Some(dec) = decoder.as_mut() {
                        let pts = (assembled.timestamp_us as i64).saturating_mul(10);
                        if let Ok(decoded_frames) = dec.decode_annex_b(&assembled.data, pts) {
                            for df in decoded_frames {
                                viewer.push_frame(df);
                            }
                        }
                    }
                }
            }
            Err(err) if is_timeout_error(&err) => {}
            Err(_) => thread::sleep(Duration::from_millis(5)),
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
            Ok(ViewerEvent::VisibilityChanged(visible)) => {
                on_visibility_changed(visible);
                if visible {
                    if let Some(peer) = *peer_addr_slot.lock().unwrap() {
                        let hdr = VideoPacketHeader::control(VideoPacketKind::StreamHello, 0, 0, 0);
                        if encode_video_packet(&hdr, &[], &auth_key, &mut control_buf).is_ok() {
                            let _ = socket.send_to(&control_buf, peer);
                        }
                    }
                } else {
                    if auto_focus {
                        on_focus_active(false);
                    }
                    if let Some(peer) = *peer_addr_slot.lock().unwrap() {
                        let hdr = VideoPacketHeader::control(VideoPacketKind::StreamStop, 0, 0, 0);
                        if encode_video_packet(&hdr, &[], &auth_key, &mut control_buf).is_ok() {
                            let _ = socket.send_to(&control_buf, peer);
                        }
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
}
