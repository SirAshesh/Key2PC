use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};

use crate::config::{AppConfig, Role};
use crate::error::{Key2PcError, Result};
use crate::input::{
    boost_current_thread_priority, HookEvent, HookOptions, PressedKey, PressedKeySet,
    WinKeyboardHook,
};
use crate::network::{
    create_tuned_udp_socket, enumerate_broadcast_targets, resolve_target_host, RetransmitQueue,
    UdpSocketConfig,
};
use crate::protocol::{derive_auth_key, KeyAction, Packet, PacketKind};
use crate::runtime::status::{ManagedService, ServiceStatus, StatusObserver};
use crate::runtime::video_host::VideoViewerHost;
use crate::runtime::{is_timeout_error, update_rtt_ewma};

struct SenderSharedState {
    peer_address: Option<SocketAddr>,
    connected: bool,
    last_rx_at: Option<Instant>,
    rtt_ms: Option<f32>,
    retransmit: RetransmitQueue,
}

pub struct SenderRuntime {
    running: Arc<AtomicBool>,
    hook: Option<Arc<WinKeyboardHook>>,
    video_host: Option<Arc<Mutex<VideoViewerHost>>>,
    shared: Arc<Mutex<SenderSharedState>>,
    status: Arc<Mutex<ServiceStatus>>,
    observer: StatusObserver,
    tx_handle: Option<JoinHandle<()>>,
    rx_handle: Option<JoinHandle<()>>,
    discovery_only: bool,
}

impl SenderRuntime {
    pub fn start(config: &AppConfig, observer: StatusObserver) -> Result<Self> {
        config.validate()?;
        let toggle_hotkey = config.sender.toggle_hotkey.to_binding()?;
        let release_hotkey = config.sender.release_all_hotkey.to_binding()?;
        let display_hotkey = config.sender.toggle_display_hotkey.to_binding()?;
        let fullscreen_hotkey = config.sender.toggle_fullscreen_hotkey.to_binding()?;
        let hotkey_label = toggle_hotkey.display_label();
        let display_hotkey_label = display_hotkey.display_label();
        let display_fullscreen_hotkey_label = fullscreen_hotkey.display_label();

        let bind_ip: IpAddr = config.network.bind_ip.parse().map_err(|_| {
            Key2PcError::InvalidConfig(format!("Invalid bind_ip: {}", config.network.bind_ip))
        })?;
        let resolved_target =
            resolve_target_host(&config.network.target_host, config.network.port)?;
        let discovery_only = resolved_target.is_none();

        let socket = Arc::new(create_tuned_udp_socket(&UdpSocketConfig {
            bind_addr: SocketAddr::new(bind_ip, 0),
            low_delay_tos: config.network.low_delay_tos,
            broadcast: true,
            read_timeout: Some(Duration::from_millis(120)),
            buffer_size: 65536,
        })?);

        let hook_options = HookOptions {
            start_active: config.sender.start_active,
            ignore_injected_keys: config.sender.ignore_injected_keys,
            toggle_hotkey,
            release_all_hotkey: release_hotkey,
            toggle_display_hotkey: if config.display.enabled {
                Some(display_hotkey)
            } else {
                None
            },
            toggle_fullscreen_hotkey: if config.display.enabled {
                Some(fullscreen_hotkey)
            } else {
                None
            },
        };
        let (hook, hook_rx) = WinKeyboardHook::start(hook_options)?;
        let hook = Arc::new(hook);

        let initial_video_peer =
            resolved_target.map(|addr| SocketAddr::new(addr.ip(), config.display.video_port));

        let initial_status = ServiceStatus {
            role: Role::Sender,
            device_name: config.device_name.clone(),
            connected: false,
            active: config.sender.start_active,
            peer_address: resolved_target,
            rtt_ms: None,
            hotkey_label,
            display_enabled: config.display.enabled,
            display_visible: config.display.enabled && config.display.open_viewer_on_start,
            display_fullscreen: config.display.enabled
                && config.display.open_viewer_on_start
                && config.display.fullscreen,
            display_fill_screen: config.display.enabled && config.display.fill_screen,
            display_hotkey_label,
            display_fullscreen_hotkey_label,
            virtual_driver_installed: false,
            virtual_monitor_active: false,
            capture_monitor_label: String::new(),
            error_message: None,
        };
        observer(&initial_status);

        let status = Arc::new(Mutex::new(initial_status));

        let video_host = if config.display.enabled {
            let focus_hook = Arc::clone(&hook);
            let on_focus: Arc<dyn Fn(bool) + Send + Sync> = Arc::new(move |active| {
                focus_hook.set_active(active);
            });
            let vis_status = Arc::clone(&status);
            let vis_observer = Arc::clone(&observer);
            let on_vis: Arc<dyn Fn(bool) + Send + Sync> = Arc::new(move |visible| {
                let snap = {
                    let mut st = vis_status.lock().unwrap();
                    st.display_visible = visible;
                    st.clone()
                };
                vis_observer(&snap);
            });
            let fs_status = Arc::clone(&status);
            let fs_observer = Arc::clone(&observer);
            let on_fs: Arc<dyn Fn(bool) + Send + Sync> = Arc::new(move |fullscreen| {
                let snap = {
                    let mut st = fs_status.lock().unwrap();
                    st.display_fullscreen = fullscreen;
                    st.clone()
                };
                fs_observer(&snap);
            });
            let fill_status = Arc::clone(&status);
            let fill_observer = Arc::clone(&observer);
            let on_fill: Arc<dyn Fn(bool) + Send + Sync> = Arc::new(move |fill_screen| {
                let snap = {
                    let mut st = fill_status.lock().unwrap();
                    st.display_fill_screen = fill_screen;
                    st.clone()
                };
                fill_observer(&snap);
            });

            let host = VideoViewerHost::start(
                config,
                initial_video_peer,
                hook.event_sender(),
                on_focus,
                on_vis,
                on_fs,
                on_fill,
            )?;
            Some(Arc::new(Mutex::new(host)))
        } else {
            None
        };

        let shared = Arc::new(Mutex::new(SenderSharedState {
            peer_address: resolved_target,
            connected: false,
            last_rx_at: None,
            rtt_ms: None,
            retransmit: RetransmitQueue::default(),
        }));
        let running = Arc::new(AtomicBool::new(true));
        let auth_key = derive_auth_key(&config.network.shared_secret);
        let clock_epoch = Instant::now();
        let next_seq = Arc::new(AtomicU64::new(1));

        let rx_handle = spawn_sender_rx_thread(
            Arc::clone(&running),
            Arc::clone(&socket),
            Arc::clone(&shared),
            Arc::clone(&status),
            Arc::clone(&observer),
            video_host.clone(),
            Arc::clone(&next_seq),
            auth_key,
            clock_epoch,
            config.network.port,
            config.display.video_port,
        )?;

        let tx_handle = spawn_sender_tx_thread(
            Arc::clone(&running),
            socket,
            hook_rx,
            Arc::clone(&shared),
            Arc::clone(&status),
            Arc::clone(&observer),
            video_host.clone(),
            next_seq,
            auth_key,
            clock_epoch,
            config.clone(),
            discovery_only,
        )?;

        Ok(Self {
            running,
            hook: Some(hook),
            video_host,
            shared,
            status,
            observer,
            tx_handle: Some(tx_handle),
            rx_handle: Some(rx_handle),
            discovery_only,
        })
    }

    pub fn discovery_mode(&self) -> bool {
        self.discovery_only
    }
}

impl ManagedService for SenderRuntime {
    fn toggle_active(&self) {
        if let Some(hook) = &self.hook {
            hook.toggle_active();
        }
    }

    fn set_active(&self, active: bool) {
        if let Some(hook) = &self.hook {
            hook.set_active(active);
        }
    }

    fn toggle_display(&self) {
        if let Some(host) = &self.video_host {
            if let Ok(guard) = host.lock() {
                guard.toggle_visible();
            }
        }
    }

    fn toggle_fullscreen(&self) {
        if let Some(host) = &self.video_host {
            if let Ok(guard) = host.lock() {
                guard.toggle_fullscreen();
            }
        }
    }

    fn toggle_fill_screen(&self) {
        if let Some(host) = &self.video_host {
            if let Ok(guard) = host.lock() {
                guard.toggle_fill_screen();
            }
        }
    }

    fn toggle_virtual_monitor(&self) {}

    fn install_virtual_driver(&self) -> Result<()> {
        Ok(())
    }

    fn release_all(&self) {
        if let Some(hook) = &self.hook {
            hook.release_all_remote();
        }
    }

    fn status(&self) -> ServiceStatus {
        self.status.lock().unwrap().clone()
    }

    fn shutdown(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(host) = self.video_host.take() {
            if let Ok(mut guard) = host.lock() {
                guard.shutdown();
            }
        }
        if let Some(hook) = &self.hook {
            hook.release_all_remote();
        }
        if let Some(hook) = self.hook.take() {
            if let Ok(mut owned) = Arc::try_unwrap(hook) {
                owned.shutdown();
            }
        }
        if let Some(handle) = self.tx_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.rx_handle.take() {
            let _ = handle.join();
        }
        if let Ok(mut guard) = self.shared.lock() {
            guard.retransmit.clear();
        }
        let mut snap = self.status.lock().unwrap().clone();
        snap.active = false;
        snap.connected = false;
        snap.display_visible = false;
        (self.observer)(&snap);
    }
}

impl Drop for SenderRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_sender_rx_thread(
    running: Arc<AtomicBool>,
    socket: Arc<UdpSocket>,
    shared: Arc<Mutex<SenderSharedState>>,
    status: Arc<Mutex<ServiceStatus>>,
    observer: StatusObserver,
    video_host: Option<Arc<Mutex<VideoViewerHost>>>,
    next_seq: Arc<AtomicU64>,
    auth_key: [u8; 32],
    clock_epoch: Instant,
    default_port: u16,
    default_video_port: u16,
) -> Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("key2pc-sender-rx".to_string())
        .spawn(move || {
            boost_current_thread_priority(true);
            let mut buf = [0u8; 128];
            let mut last_rtt_notify = Instant::now() - Duration::from_secs(2);
            while running.load(Ordering::Acquire) {
                match socket.recv_from(&mut buf) {
                    Ok((len, src)) => {
                        let Ok(pkt) = Packet::decode(&buf[..len], &auth_key) else {
                            continue;
                        };
                        let now = Instant::now();
                        let now_us = clock_epoch.elapsed().as_micros() as u64;
                        let mut state_changed = false;
                        let mut rtt_updated = false;
                        let mut discovered_video_peer: Option<SocketAddr> = None;

                        let snapshot_opt = {
                            let mut guard = shared.lock().unwrap();
                            match pkt.kind {
                                PacketKind::Ack => {
                                    guard.last_rx_at = Some(now);
                                    if !guard.connected {
                                        guard.connected = true;
                                        state_changed = true;
                                    }
                                    if let Some(rtt) = guard.retransmit.ack(pkt.seq, now) {
                                        let sample_ms = rtt.as_secs_f32() * 1000.0;
                                        update_rtt_ewma(&mut guard.rtt_ms, sample_ms);
                                        rtt_updated = true;
                                    }
                                }
                                PacketKind::Pong => {
                                    guard.last_rx_at = Some(now);
                                    if !guard.connected {
                                        guard.connected = true;
                                        state_changed = true;
                                    }
                                    if pkt.timestamp_us <= now_us {
                                        let sample_ms = (now_us - pkt.timestamp_us) as f32 / 1000.0;
                                        if sample_ms < 5000.0 {
                                            update_rtt_ewma(&mut guard.rtt_ms, sample_ms);
                                            rtt_updated = true;
                                        }
                                    }
                                }
                                PacketKind::DiscoverResp => {
                                    let target_port =
                                        if pkt.vk != 0 { pkt.vk } else { default_port };
                                    let video_port = if pkt.scan_code != 0 {
                                        pkt.scan_code
                                    } else {
                                        default_video_port
                                    };
                                    let peer = SocketAddr::new(src.ip(), target_port);
                                    discovered_video_peer =
                                        Some(SocketAddr::new(src.ip(), video_port));
                                    if guard.peer_address != Some(peer) || !guard.connected {
                                        guard.peer_address = Some(peer);
                                        guard.last_rx_at = Some(now);
                                        guard.connected = true;
                                        state_changed = true;
                                        let seq = next_seq.fetch_add(1, Ordering::Relaxed);
                                        let ping =
                                            Packet::ping(seq, now_us, true).encode(&auth_key);
                                        let _ = socket.send_to(&ping, peer);
                                    }
                                }
                                _ => {}
                            }

                            let should_notify = state_changed
                                || (rtt_updated
                                    && now.saturating_duration_since(last_rtt_notify)
                                        >= Duration::from_millis(500));
                            if should_notify {
                                last_rtt_notify = now;
                                Some((guard.connected, guard.peer_address, guard.rtt_ms))
                            } else {
                                None
                            }
                        };

                        if let Some(vpeer) = discovered_video_peer {
                            if let Some(host) = &video_host {
                                if let Ok(guard) = host.lock() {
                                    guard.set_peer_video_addr(Some(vpeer));
                                }
                            }
                        }

                        if let Some((connected, peer_address, rtt_ms)) = snapshot_opt {
                            let snap = {
                                let mut st = status.lock().unwrap();
                                st.connected = connected;
                                st.peer_address = peer_address;
                                st.rtt_ms = rtt_ms;
                                st.clone()
                            };
                            observer(&snap);
                        }
                    }
                    Err(err) if is_timeout_error(&err) => {}
                    Err(_) => {
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        })
        .map_err(Key2PcError::Io)
}

#[allow(clippy::too_many_arguments)]
fn spawn_sender_tx_thread(
    running: Arc<AtomicBool>,
    socket: Arc<UdpSocket>,
    hook_rx: Receiver<HookEvent>,
    shared: Arc<Mutex<SenderSharedState>>,
    status: Arc<Mutex<ServiceStatus>>,
    observer: StatusObserver,
    video_host: Option<Arc<Mutex<VideoViewerHost>>>,
    next_seq: Arc<AtomicU64>,
    auth_key: [u8; 32],
    clock_epoch: Instant,
    config: AppConfig,
    discovery_only: bool,
) -> Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("key2pc-sender-tx".to_string())
        .spawn(move || {
            boost_current_thread_priority(false);
            let retransmit_interval = Duration::from_millis(config.network.retransmit_ms);
            let heartbeat_interval = Duration::from_millis(config.network.heartbeat_ms);
            let disconnect_timeout = Duration::from_millis(
                config
                    .network
                    .disconnect_release_ms
                    .saturating_mul(10)
                    .max(15_000),
            );
            let redundant_sends = config.network.redundant_sends.max(1);

            let mut held_keys = PressedKeySet::default();
            let mut last_discover_at = Instant::now() - Duration::from_secs(5);
            let mut last_heartbeat_at = Instant::now();
            let mut last_ping_at = Instant::now() - Duration::from_secs(5);
            let mut session_started = false;
            let mut is_active = config.sender.start_active;
            let mut last_known_peer: Option<SocketAddr> = None;

            while running.load(Ordering::Acquire) {
                let wait_timeout = {
                    let guard = shared.lock().unwrap();
                    guard
                        .retransmit
                        .next_wake_delay(Instant::now(), retransmit_interval)
                        .unwrap_or(heartbeat_interval)
                        .min(heartbeat_interval)
                };

                match hook_rx.recv_timeout(wait_timeout) {
                    Ok(first_event) => {
                        process_hook_event(
                            first_event,
                            &socket,
                            &shared,
                            &status,
                            &observer,
                            &video_host,
                            &next_seq,
                            &auth_key,
                            clock_epoch,
                            &mut held_keys,
                            &mut is_active,
                            redundant_sends,
                        );
                        while let Ok(extra) = hook_rx.try_recv() {
                            process_hook_event(
                                extra,
                                &socket,
                                &shared,
                                &status,
                                &observer,
                                &video_host,
                                &next_seq,
                                &auth_key,
                                clock_epoch,
                                &mut held_keys,
                                &mut is_active,
                                redundant_sends,
                            );
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }

                let now = Instant::now();
                let now_us = clock_epoch.elapsed().as_micros() as u64;

                let (peer_opt, due_packets, dropped_connection) = {
                    let mut guard = shared.lock().unwrap();
                    let due = guard.retransmit.collect_due(
                        now,
                        retransmit_interval,
                        config.network.max_retransmits,
                    );

                    let mut dropped = false;
                    if guard.connected {
                        if let Some(last_rx) = guard.last_rx_at {
                            if now.saturating_duration_since(last_rx) >= disconnect_timeout {
                                guard.connected = false;
                                guard.rtt_ms = None;
                                guard.retransmit.clear();
                                if discovery_only {
                                    guard.peer_address = None;
                                }
                                dropped = true;
                            }
                        }
                    }
                    (guard.peer_address, due, dropped)
                };

                if dropped_connection {
                    session_started = false;
                    if discovery_only {
                        if let Some(host) = &video_host {
                            if let Ok(guard) = host.lock() {
                                guard.set_peer_video_addr(None);
                            }
                        }
                    }
                    let snap = {
                        let mut st = status.lock().unwrap();
                        st.connected = false;
                        st.rtt_ms = None;
                        if discovery_only {
                            st.peer_address = None;
                        }
                        st.clone()
                    };
                    observer(&snap);
                }

                if let Some(peer) = peer_opt {
                    last_known_peer = Some(peer);
                    for raw in due_packets {
                        let _ = socket.send_to(&raw, peer);
                    }

                    if !session_started
                        && now.saturating_duration_since(last_ping_at) >= Duration::from_millis(250)
                    {
                        last_ping_at = now;
                        let seq = next_seq.fetch_add(1, Ordering::Relaxed);
                        let pkt = Packet::ping(seq, now_us, true).encode(&auth_key);
                        let _ = socket.send_to(&pkt, peer);
                        session_started = true;
                    } else if now.saturating_duration_since(last_ping_at)
                        >= Duration::from_millis(250)
                    {
                        last_ping_at = now;
                        let seq = next_seq.fetch_add(1, Ordering::Relaxed);
                        let pkt = Packet::ping(seq, now_us, false).encode(&auth_key);
                        let _ = socket.send_to(&pkt, peer);
                    }

                    if (is_active || !held_keys.is_empty())
                        && now.saturating_duration_since(last_heartbeat_at) >= heartbeat_interval
                    {
                        last_heartbeat_at = now;
                        let seq = next_seq.fetch_add(1, Ordering::Relaxed);
                        let count = held_keys.len().min(u16::MAX as usize) as u16;
                        let pkt = Packet::heartbeat(seq, now_us, count).encode(&auth_key);
                        let _ = socket.send_to(&pkt, peer);
                    }
                } else if now.saturating_duration_since(last_discover_at)
                    >= Duration::from_millis(450)
                {
                    last_discover_at = now;
                    let seq = next_seq.fetch_add(1, Ordering::Relaxed);
                    let req = Packet::discover_req(seq, now_us).encode(&auth_key);
                    if let Some(last_peer) = last_known_peer {
                        let _ = socket.send_to(
                            &req,
                            SocketAddr::new(last_peer.ip(), config.network.discovery_port),
                        );
                        let _ = socket
                            .send_to(&req, SocketAddr::new(last_peer.ip(), config.network.port));
                    }
                    for bcast in enumerate_broadcast_targets(config.network.discovery_port) {
                        let _ = socket.send_to(&req, bcast);
                    }
                    if config.network.discovery_port != config.network.port {
                        for bcast in enumerate_broadcast_targets(config.network.port) {
                            let _ = socket.send_to(&req, bcast);
                        }
                    }
                }
            }

            let shutdown_peer = shared.lock().unwrap().peer_address;
            if let Some(peer) = shutdown_peer {
                let seq = next_seq.fetch_add(1, Ordering::Relaxed);
                let now_us = clock_epoch.elapsed().as_micros() as u64;
                let release_pkt = Packet::release_all(seq, now_us, false).encode(&auth_key);
                let _ = socket.send_to(&release_pkt, peer);
            }
        })
        .map_err(Key2PcError::Io)
}

#[allow(clippy::too_many_arguments)]
fn process_hook_event(
    event: HookEvent,
    socket: &UdpSocket,
    shared: &Arc<Mutex<SenderSharedState>>,
    status: &Arc<Mutex<ServiceStatus>>,
    observer: &StatusObserver,
    video_host: &Option<Arc<Mutex<VideoViewerHost>>>,
    next_seq: &Arc<AtomicU64>,
    auth_key: &[u8; 32],
    clock_epoch: Instant,
    held_keys: &mut PressedKeySet,
    is_active: &mut bool,
    redundant_sends: u8,
) {
    let now = Instant::now();
    let now_us = clock_epoch.elapsed().as_micros() as u64;

    match event {
        HookEvent::Key(key_ev) => {
            held_keys.track(
                PressedKey {
                    vk: key_ev.vk,
                    scan_code: key_ev.scan_code,
                    extended: key_ev.extended,
                },
                key_ev.action == KeyAction::Up,
            );

            let peer = { shared.lock().unwrap().peer_address };
            let Some(peer_addr) = peer else {
                return;
            };
            let seq = next_seq.fetch_add(1, Ordering::Relaxed);
            let raw = Packet::key_event(seq, now_us, key_ev).encode(auth_key);
            for _ in 0..redundant_sends {
                let _ = socket.send_to(&raw, peer_addr);
            }
            let mut guard = shared.lock().unwrap();
            guard.retransmit.push(seq, raw, now);
        }
        HookEvent::Mouse(mouse_ev) => {
            let peer = { shared.lock().unwrap().peer_address };
            let Some(peer_addr) = peer else {
                return;
            };
            let seq = next_seq.fetch_add(1, Ordering::Relaxed);
            let raw = Packet::mouse_event(seq, now_us, mouse_ev).encode(auth_key);
            let _ = socket.send_to(&raw, peer_addr);
            if mouse_ev.action.requires_reliable_ack() {
                let mut guard = shared.lock().unwrap();
                guard.retransmit.push(seq, raw, now);
            }
        }
        HookEvent::ReleaseAllRequested => {
            held_keys.clear();
            let peer = {
                let mut guard = shared.lock().unwrap();
                guard.retransmit.clear();
                guard.peer_address
            };
            if let Some(peer_addr) = peer {
                let seq = next_seq.fetch_add(1, Ordering::Relaxed);
                let raw = Packet::release_all(seq, now_us, false).encode(auth_key);
                let _ = socket.send_to(&raw, peer_addr);
                let _ = socket.send_to(&raw, peer_addr);
            }
        }
        HookEvent::ActiveChanged(active) => {
            *is_active = active;
            let snap = {
                let mut st = status.lock().unwrap();
                st.active = active;
                st.clone()
            };
            observer(&snap);
        }
        HookEvent::ToggleDisplayRequested => {
            if let Some(host) = video_host {
                if let Ok(guard) = host.lock() {
                    guard.toggle_visible();
                }
            }
        }
        HookEvent::ToggleFullscreenRequested => {
            if let Some(host) = video_host {
                if let Ok(guard) = host.lock() {
                    guard.toggle_fullscreen();
                }
            }
        }
    }
}
