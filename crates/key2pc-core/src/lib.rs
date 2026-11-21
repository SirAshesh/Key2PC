pub mod config;
pub mod display;
pub mod error;
pub mod input;
pub mod network;
pub mod protocol;
pub mod runtime;
pub mod tray;

pub use config::{
    resolve_config_path, AppConfig, ConfigStore, DisplayConfig, HotkeyConfig, JsonFileConfigStore,
    NetworkConfig, ReceiverConfig, Role, SenderConfig, TrayConfig,
};
pub use display::{select_monitor_index, CaptureResult, DecodedFrame, EncodedFrame, MonitorInfo};
pub use error::{Key2PcError, Result};
pub use input::{
    format_virtual_key, parse_virtual_key, DesktopBounds, HookEvent, HookOptions, HookProcessor,
    HotkeyBinding, InputSink, ModifierState, PressedKey, PressedKeySet,
};
pub use network::{
    create_tuned_udp_socket, enumerate_broadcast_targets, resolve_target_host, PendingPacket,
    RetransmitQueue, UdpSocketConfig,
};
pub use protocol::{
    chunk_video_frame, decode_video_packet, derive_auth_key, encode_video_packet, AssembledFrame,
    FrameAssembler, KeyAction, KeyEvent, MouseAction, MouseEvent, Packet, PacketKind,
    SequenceWindow, VideoPacketHeader, VideoPacketKind, PACKET_SIZE, VIDEO_PACKET_HEADER_SIZE,
};
pub use runtime::{
    start_configured_service, write_error_log, ManagedService, ReceiverRuntime, SenderRuntime,
    ServiceStatus, StatusObserver,
};
