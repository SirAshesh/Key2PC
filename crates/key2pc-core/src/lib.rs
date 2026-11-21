pub mod config;
pub mod error;
pub mod input;
pub mod network;
pub mod protocol;
pub mod runtime;
pub mod tray;

pub use config::{
    resolve_config_path, AppConfig, ConfigStore, HotkeyConfig, JsonFileConfigStore, NetworkConfig,
    ReceiverConfig, Role, SenderConfig, TrayConfig,
};
pub use error::{Key2PcError, Result};
pub use input::{
    format_virtual_key, parse_virtual_key, HookEvent, HookOptions, HookProcessor, HotkeyBinding,
    InputSink, ModifierState, PressedKey, PressedKeySet,
};
pub use network::{
    create_tuned_udp_socket, enumerate_broadcast_targets, resolve_target_host, PendingPacket,
    RetransmitQueue, UdpSocketConfig,
};
pub use protocol::{
    derive_auth_key, KeyAction, KeyEvent, Packet, PacketKind, SequenceWindow, PACKET_SIZE,
};
pub use runtime::{
    start_configured_service, write_error_log, ManagedService, ReceiverRuntime, SenderRuntime,
    ServiceStatus, StatusObserver,
};
