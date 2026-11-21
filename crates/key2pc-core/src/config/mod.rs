mod model;
mod store;

pub use model::{
    AppConfig, DisplayConfig, HotkeyConfig, NetworkConfig, ReceiverConfig, Role, SenderConfig,
    TrayConfig,
};
pub use store::{resolve_config_path, ConfigStore, JsonFileConfigStore};
