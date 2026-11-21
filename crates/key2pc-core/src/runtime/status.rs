use std::net::SocketAddr;
use std::sync::Arc;

use crate::config::Role;
use crate::error::Result;

pub type StatusObserver = Arc<dyn Fn(&ServiceStatus) + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub struct ServiceStatus {
    pub role: Role,
    pub device_name: String,
    pub connected: bool,
    pub active: bool,
    pub peer_address: Option<SocketAddr>,
    pub rtt_ms: Option<f32>,
    pub hotkey_label: String,
    pub display_enabled: bool,
    pub display_visible: bool,
    pub display_fullscreen: bool,
    pub display_fill_screen: bool,
    pub display_hotkey_label: String,
    pub display_fullscreen_hotkey_label: String,
    pub virtual_driver_installed: bool,
    pub virtual_monitor_active: bool,
    pub capture_monitor_label: String,
    pub error_message: Option<String>,
}

impl ServiceStatus {
    pub fn error(role: Role, device_name: String, message: String) -> Self {
        Self {
            role,
            device_name,
            connected: false,
            active: false,
            peer_address: None,
            rtt_ms: None,
            hotkey_label: String::new(),
            display_enabled: false,
            display_visible: false,
            display_fullscreen: false,
            display_fill_screen: false,
            display_hotkey_label: String::new(),
            display_fullscreen_hotkey_label: String::new(),
            virtual_driver_installed: false,
            virtual_monitor_active: false,
            capture_monitor_label: String::new(),
            error_message: Some(message),
        }
    }
}

pub trait ManagedService: Send {
    fn toggle_active(&self);
    fn set_active(&self, active: bool);
    fn toggle_display(&self);
    fn toggle_fullscreen(&self);
    fn toggle_fill_screen(&self);
    fn toggle_virtual_monitor(&self);
    fn install_virtual_driver(&self) -> Result<()>;
    fn release_all(&self);
    fn status(&self) -> ServiceStatus;
    fn shutdown(&mut self);
}
