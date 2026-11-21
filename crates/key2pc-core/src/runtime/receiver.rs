use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::{AppConfig, DisplayConfig, Role};
use crate::display::{
    enumerate_monitors, inspect_virtual_driver_status, request_elevated_vdd_install,
    select_best_monitor, VirtualMonitorGuard,
};
use crate::error::{Key2PcError, Result};
use crate::input::{boost_current_thread_priority, DesktopBounds, InputSink, WinInputInjector};
use crate::network::{create_tuned_udp_socket, UdpSocketConfig};
use crate::protocol::{derive_auth_key, Packet, PacketKind, SequenceWindow};
use crate::runtime::is_timeout_error;
use crate::runtime::status::{ManagedService, ServiceStatus, StatusObserver};
use crate::runtime::video_host::VideoStreamerHost;

pub struct ReceiverRuntime {
    running: Arc<AtomicBool>,
    display_cfg: DisplayConfig,
    capture_epoch: Arc<AtomicU64>,
    shared_bounds: Arc<Mutex<Option<DesktopBounds>>>,
    virtual_guard: Arc<Mutex<Option<VirtualMonitorGuard>>>,
    status: Arc<Mutex<ServiceStatus>>,
    observer: StatusObserver,
    rx_handle: Option<JoinHandle<()>>,
    discovery_handle: Option<JoinHandle<()>>,
    video_host: Option<VideoStreamerHost>,
}

impl ReceiverRuntime {
    pub fn start(config: &AppConfig, observer: StatusObserver) -> Result<Self> {
        let sink = Box::new(WinInputInjector::new(config.receiver.prefer_scancode));
        Self::start_with_sink(config, sink, observer)
    }

    pub fn start_with_sink(
        config: &AppConfig,
        mut sink: Box<dyn InputSink>,
        observer: StatusObserver,
    ) -> Result<Self> {
        config.validate()?;
        let bind_ip: IpAddr = config.network.bind_ip.parse().map_err(|_| {
            Key2PcError::InvalidConfig(format!("Invalid bind_ip: {}", config.network.bind_ip))
        })?;
        let allowed_ip: Option<IpAddr> = config
            .receiver
            .allow_only_ip
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(|_| Key2PcError::InvalidConfig("Invalid allow_only_ip".to_string()))?;

        let drv_status = inspect_virtual_driver_status();
        let mut initial_vdd_guard = None;
        if config.display.enabled && config.display.virtual_monitor && drv_status.is_ready() {
            if let Ok(guard) = VirtualMonitorGuard::acquire(
                config.display.width,
                config.display.height,
                config.display.fps,
            ) {
                initial_vdd_guard = Some(guard);
            }
        }

        let (initial_bounds, monitor_label, has_virtual_active) =
            resolve_capture_monitor_info(&config.display, initial_vdd_guard.is_some());
        if let Some(bounds) = initial_bounds {
            sink.set_monitor_bounds(bounds);
        }

        let main_socket = create_tuned_udp_socket(&UdpSocketConfig {
            bind_addr: SocketAddr::new(bind_ip, config.network.port),
            low_delay_tos: config.network.low_delay_tos,
            broadcast: true,
            read_timeout: Some(Duration::from_millis(80)),
            buffer_size: 65536,
        })?;

        let discovery_socket = if config.network.discovery_port != config.network.port {
            Some(create_tuned_udp_socket(&UdpSocketConfig {
                bind_addr: SocketAddr::new(bind_ip, config.network.discovery_port),
                low_delay_tos: config.network.low_delay_tos,
                broadcast: true,
                read_timeout: Some(Duration::from_millis(150)),
                buffer_size: 8192,
            })?)
        } else {
            None
        };

        let initial_status = ServiceStatus {
            role: Role::Receiver,
            device_name: config.device_name.clone(),
            connected: false,
            active: true,
            peer_address: None,
            rtt_ms: None,
            hotkey_label: String::new(),
            display_enabled: config.display.enabled,
            display_visible: false,
            display_fullscreen: false,
            display_hotkey_label: String::new(),
            display_fullscreen_hotkey_label: String::new(),
            virtual_driver_installed: drv_status.is_ready(),
            virtual_monitor_active: has_virtual_active,
            capture_monitor_label: monitor_label,
            error_message: None,
        };
        observer(&initial_status);

        let running = Arc::new(AtomicBool::new(true));
        let status = Arc::new(Mutex::new(initial_status));
        let auth_key = derive_auth_key(&config.network.shared_secret);
        let listen_port = main_socket.local_addr()?.port();
        let video_port = config.display.video_port;
        let disconnect_release = Duration::from_millis(config.network.disconnect_release_ms);
        let peer_idle_drop = Duration::from_millis(
            config
                .network
                .disconnect_release_ms
                .saturating_mul(10)
                .max(15_000),
        );

        let capture_epoch = Arc::new(AtomicU64::new(0));
        let shared_bounds = Arc::new(Mutex::new(initial_bounds));
        let virtual_guard = Arc::new(Mutex::new(initial_vdd_guard));
        let video_host = if config.display.enabled {
            let bounds_slot = Arc::clone(&shared_bounds);
            let bounds_cb: Arc<dyn Fn(DesktopBounds) + Send + Sync> = Arc::new(move |bounds| {
                if let Ok(mut guard) = bounds_slot.lock() {
                    *guard = Some(bounds);
                }
            });
            Some(VideoStreamerHost::start(
                config,
                Arc::clone(&capture_epoch),
                bounds_cb,
            )?)
        } else {
            None
        };

        let discovery_handle = discovery_socket.and_then(|disc_sock| {
            let disc_running = Arc::clone(&running);
            thread::Builder::new()
                .name("key2pc-receiver-disc".to_string())
                .spawn(move || {
                    let mut buf = [0u8; 128];
                    while disc_running.load(Ordering::Acquire) {
                        match disc_sock.recv_from(&mut buf) {
                            Ok((len, src)) => {
                                if let Some(allowed) = allowed_ip {
                                    if src.ip() != allowed {
                                        continue;
                                    }
                                }
                                let Ok(pkt) = Packet::decode(&buf[..len], &auth_key) else {
                                    continue;
                                };
                                if pkt.kind == PacketKind::DiscoverReq {
                                    let resp = Packet::discover_resp(
                                        pkt.seq,
                                        pkt.timestamp_us,
                                        listen_port,
                                        video_port,
                                    )
                                    .encode(&auth_key);
                                    let _ = disc_sock.send_to(&resp, src);
                                }
                            }
                            Err(err) if is_timeout_error(&err) => {}
                            Err(_) => thread::sleep(Duration::from_millis(10)),
                        }
                    }
                })
                .ok()
        });

        let rx_running = Arc::clone(&running);
        let rx_status = Arc::clone(&status);
        let rx_observer = Arc::clone(&observer);
        let rx_bounds = Arc::clone(&shared_bounds);
        let rx_handle = thread::Builder::new()
            .name("key2pc-receiver-rx".to_string())
            .spawn(move || {
                boost_current_thread_priority(true);
                let mut sink = sink;
                let mut buf = [0u8; 128];
                let mut seq_window = SequenceWindow::default();
                let mut active_peer: Option<SocketAddr> = None;
                let mut last_peer_at: Option<Instant> = None;
                let mut last_key_event_at: Option<Instant> = None;

                while rx_running.load(Ordering::Acquire) {
                    if let Some(new_bounds) = rx_bounds.lock().ok().and_then(|mut g| g.take()) {
                        sink.set_monitor_bounds(new_bounds);
                        let snap = {
                            let mut st = rx_status.lock().unwrap();
                            let prefix = if st.virtual_monitor_active {
                                "Extended Virtual"
                            } else {
                                "Display"
                            };
                            st.capture_monitor_label =
                                format!("{prefix} ({}x{})", new_bounds.width, new_bounds.height);
                            st.clone()
                        };
                        rx_observer(&snap);
                    }

                    match main_socket.recv_from(&mut buf) {
                        Ok((len, src)) => {
                            if let Some(allowed) = allowed_ip {
                                if src.ip() != allowed {
                                    continue;
                                }
                            }
                            let Ok(pkt) = Packet::decode(&buf[..len], &auth_key) else {
                                continue;
                            };

                            let now = Instant::now();
                            last_peer_at = Some(now);
                            if active_peer != Some(src) {
                                active_peer = Some(src);
                                let snap = {
                                    let mut st = rx_status.lock().unwrap();
                                    st.connected = true;
                                    st.peer_address = Some(src);
                                    st.clone()
                                };
                                rx_observer(&snap);
                            }

                            match pkt.kind {
                                PacketKind::KeyEvent => {
                                    let ack =
                                        Packet::ack(pkt.seq, pkt.timestamp_us).encode(&auth_key);
                                    let _ = main_socket.send_to(&ack, src);
                                    if pkt.is_session_start() {
                                        seq_window.reset();
                                    }
                                    if seq_window.accept(pkt.seq) {
                                        if let Some(ev) = pkt.as_key_event() {
                                            let _ = sink.handle_key(ev);
                                            last_key_event_at = Some(now);
                                        }
                                    }
                                }
                                PacketKind::MouseEvent => {
                                    if let Ok(Some(mouse)) = pkt.as_mouse_event() {
                                        if mouse.action.requires_reliable_ack() {
                                            let ack = Packet::ack(pkt.seq, pkt.timestamp_us)
                                                .encode(&auth_key);
                                            let _ = main_socket.send_to(&ack, src);
                                            if seq_window.accept(pkt.seq) {
                                                let _ = sink.handle_mouse(mouse);
                                            }
                                        } else {
                                            let _ = sink.handle_mouse(mouse);
                                        }
                                    }
                                }
                                PacketKind::ReleaseAll => {
                                    let ack =
                                        Packet::ack(pkt.seq, pkt.timestamp_us).encode(&auth_key);
                                    let _ = main_socket.send_to(&ack, src);
                                    let _ = sink.release_all();
                                    seq_window.reset();
                                }
                                PacketKind::Heartbeat => {
                                    if pkt.vk == 0 && sink.held_count() > 0 {
                                        let quiet = last_key_event_at
                                            .map(|t| {
                                                now.saturating_duration_since(t)
                                                    >= Duration::from_millis(40)
                                            })
                                            .unwrap_or(true);
                                        if quiet {
                                            let _ = sink.release_all();
                                        }
                                    }
                                }
                                PacketKind::Ping => {
                                    if pkt.is_session_start() {
                                        seq_window.reset();
                                        let _ = sink.release_all();
                                    }
                                    let pong =
                                        Packet::pong(pkt.seq, pkt.timestamp_us).encode(&auth_key);
                                    let _ = main_socket.send_to(&pong, src);
                                }
                                PacketKind::DiscoverReq => {
                                    let resp = Packet::discover_resp(
                                        pkt.seq,
                                        pkt.timestamp_us,
                                        listen_port,
                                        video_port,
                                    )
                                    .encode(&auth_key);
                                    let _ = main_socket.send_to(&resp, src);
                                }
                                _ => {}
                            }
                        }
                        Err(err) if is_timeout_error(&err) => {
                            let now = Instant::now();
                            if sink.held_count() > 0 {
                                if let Some(last) = last_peer_at {
                                    if now.saturating_duration_since(last) >= disconnect_release {
                                        let _ = sink.release_all();
                                    }
                                }
                            }
                            if active_peer.is_some() {
                                if let Some(last) = last_peer_at {
                                    if now.saturating_duration_since(last) >= peer_idle_drop {
                                        active_peer = None;
                                        seq_window.reset();
                                        let snap = {
                                            let mut st = rx_status.lock().unwrap();
                                            st.connected = false;
                                            st.peer_address = None;
                                            st.clone()
                                        };
                                        rx_observer(&snap);
                                    }
                                }
                            }
                        }
                        Err(_) => thread::sleep(Duration::from_millis(10)),
                    }
                }

                let _ = sink.release_all();
            })
            .map_err(Key2PcError::Io)?;

        Ok(Self {
            running,
            display_cfg: config.display.clone(),
            capture_epoch,
            shared_bounds,
            virtual_guard,
            status,
            observer,
            rx_handle: Some(rx_handle),
            discovery_handle,
            video_host,
        })
    }
}

impl ManagedService for ReceiverRuntime {
    fn toggle_active(&self) {}
    fn set_active(&self, _active: bool) {}
    fn toggle_display(&self) {}
    fn toggle_fullscreen(&self) {}

    fn toggle_virtual_monitor(&self) {
        if !self.display_cfg.enabled {
            return;
        }
        let guard_slot = Arc::clone(&self.virtual_guard);
        let bounds_slot = Arc::clone(&self.shared_bounds);
        let capture_epoch = Arc::clone(&self.capture_epoch);
        let status = Arc::clone(&self.status);
        let observer = Arc::clone(&self.observer);
        let cfg = self.display_cfg.clone();

        let _ = thread::Builder::new()
            .name("key2pc-vdd-toggle".to_string())
            .spawn(move || {
                toggle_virtual_monitor_sync(
                    &cfg,
                    &guard_slot,
                    &bounds_slot,
                    &capture_epoch,
                    &status,
                    &observer,
                );
            });
    }

    fn install_virtual_driver(&self) -> Result<()> {
        request_elevated_vdd_install()?;
        let running = Arc::clone(&self.running);
        let guard_slot = Arc::clone(&self.virtual_guard);
        let bounds_slot = Arc::clone(&self.shared_bounds);
        let capture_epoch = Arc::clone(&self.capture_epoch);
        let status = Arc::clone(&self.status);
        let observer = Arc::clone(&self.observer);
        let cfg = self.display_cfg.clone();

        let _ = thread::Builder::new()
            .name("key2pc-vdd-watch".to_string())
            .spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(60);
                while running.load(Ordering::Acquire) && Instant::now() < deadline {
                    if inspect_virtual_driver_status().is_ready() {
                        if let Ok(mut st) = status.lock() {
                            st.virtual_driver_installed = true;
                        }
                        if cfg.enabled && cfg.virtual_monitor {
                            let is_active = guard_slot
                                .lock()
                                .map(|g| g.is_some())
                                .unwrap_or(false);
                            if !is_active {
                                toggle_virtual_monitor_sync(
                                    &cfg,
                                    &guard_slot,
                                    &bounds_slot,
                                    &capture_epoch,
                                    &status,
                                    &observer,
                                );
                                return;
                            }
                        }
                        let snap = status.lock().unwrap().clone();
                        observer(&snap);
                        return;
                    }
                    thread::sleep(Duration::from_millis(500));
                }
            });
        Ok(())
    }

    fn release_all(&self) {}

    fn status(&self) -> ServiceStatus {
        self.status.lock().unwrap().clone()
    }

    fn shutdown(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(mut vhost) = self.video_host.take() {
            vhost.shutdown();
        }
        if let Ok(mut guard) = self.virtual_guard.lock() {
            if let Some(mut vdd) = guard.take() {
                vdd.shutdown();
            }
        }
        if let Some(handle) = self.rx_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.discovery_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ReceiverRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn resolve_capture_monitor_info(
    cfg: &DisplayConfig,
    virtual_guard_active: bool,
) -> (Option<DesktopBounds>, String, bool) {
    let Ok(monitors) = enumerate_monitors() else {
        return (None, String::new(), virtual_guard_active);
    };
    if monitors.is_empty() {
        return (None, String::new(), virtual_guard_active);
    }
    let idx = select_best_monitor(&cfg.monitor, &monitors);
    let chosen = &monitors[idx];
    let virtual_active = virtual_guard_active || chosen.is_virtual;
    let label = if chosen.is_virtual {
        format!(
            "Extended Virtual ({}x{})",
            chosen.bounds.width, chosen.bounds.height
        )
    } else if !chosen.is_primary {
        format!(
            "Extended #{} ({}x{})",
            chosen.index + 1,
            chosen.bounds.width,
            chosen.bounds.height
        )
    } else {
        format!("Mirror ({}x{})", chosen.bounds.width, chosen.bounds.height)
    };
    (Some(chosen.bounds), label, virtual_active)
}

fn toggle_virtual_monitor_sync(
    cfg: &DisplayConfig,
    guard_slot: &Arc<Mutex<Option<VirtualMonitorGuard>>>,
    bounds_slot: &Arc<Mutex<Option<DesktopBounds>>>,
    capture_epoch: &Arc<AtomicU64>,
    status: &Arc<Mutex<ServiceStatus>>,
    observer: &StatusObserver,
) {
    let mut active_now = {
        let mut guard = guard_slot.lock().unwrap();
        if let Some(mut vdd) = guard.take() {
            vdd.shutdown();
            false
        } else {
            true
        }
    };

    if active_now {
        match VirtualMonitorGuard::acquire(cfg.width, cfg.height, cfg.fps) {
            Ok(vdd) => {
                *guard_slot.lock().unwrap() = Some(vdd);
            }
            Err(err) => {
                active_now = false;
                if let Ok(mut st) = status.lock() {
                    st.error_message = Some(err.to_string());
                }
            }
        }
    }

    let drv_ready = inspect_virtual_driver_status().is_ready();
    let (new_bounds, monitor_label, has_virtual) = resolve_capture_monitor_info(cfg, active_now);
    if let Some(bounds) = new_bounds {
        if let Ok(mut b) = bounds_slot.lock() {
            *b = Some(bounds);
        }
    }
    capture_epoch.fetch_add(1, Ordering::Release);

    let snap = {
        let mut st = status.lock().unwrap();
        st.virtual_driver_installed = drv_ready;
        st.virtual_monitor_active = has_virtual;
        st.capture_monitor_label = monitor_label;
        if active_now {
            st.error_message = None;
        }
        st.clone()
    };
    observer(&snap);
}
