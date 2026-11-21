mod model;
mod store;

pub use model::{
    AppConfig, HotkeyConfig, NetworkConfig, ReceiverConfig, Role, SenderConfig, TrayConfig,
};
pub use store::{resolve_config_path, ConfigStore, JsonFileConfigStore};
