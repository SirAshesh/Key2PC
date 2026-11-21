use std::net::SocketAddr;
use std::sync::Arc;

use crate::config::Role;

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
            error_message: Some(message),
        }
    }
}

pub trait ManagedService: Send {
    fn toggle_active(&self);
    fn set_active(&self, active: bool);
    fn release_all(&self);
    fn status(&self) -> ServiceStatus;
    fn shutdown(&mut self);
}
