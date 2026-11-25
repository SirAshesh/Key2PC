use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::error::{Key2PcError, Result};
use crate::input::{parse_virtual_key, HotkeyBinding};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[serde(alias = "client", alias = "laptop")]
    Sender,
    #[serde(alias = "server", alias = "pc")]
    Receiver,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct NetworkConfig {
    pub bind_ip: String,
    pub port: u16,
    pub target_host: String,
    pub discovery_port: u16,
    pub shared_secret: String,
    pub retransmit_ms: u64,
    pub max_retransmits: u8,
    pub heartbeat_ms: u64,
    pub disconnect_release_ms: u64,
    pub redundant_sends: u8,
    pub low_delay_tos: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            bind_ip: "0.0.0.0".to_string(),
            port: 24800,
            target_host: "auto".to_string(),
            discovery_port: 24801,
            shared_secret: "key2pc-change-this-secret".to_string(),
            retransmit_ms: 3,
            max_retransmits: 6,
            heartbeat_ms: 100,
            disconnect_release_ms: 1500,
            redundant_sends: 1,
            low_delay_tos: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HotkeyConfig {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
            key: "K".to_string(),
        }
    }
}

impl HotkeyConfig {
    pub fn to_binding(&self) -> Result<HotkeyBinding> {
        let vk = parse_virtual_key(&self.key)?;
        Ok(HotkeyBinding::new(
            self.ctrl, self.alt, self.shift, self.win, vk,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SenderConfig {
    pub start_active: bool,
    pub ignore_injected_keys: bool,
    pub toggle_hotkey: HotkeyConfig,
    pub release_all_hotkey: HotkeyConfig,
    pub toggle_display_hotkey: HotkeyConfig,
    pub toggle_fullscreen_hotkey: HotkeyConfig,
}

impl Default for SenderConfig {
    fn default() -> Self {
        Self {
            start_active: false,
            ignore_injected_keys: true,
            toggle_hotkey: HotkeyConfig {
                ctrl: true,
                alt: true,
                shift: false,
                win: false,
                key: "K".to_string(),
            },
            release_all_hotkey: HotkeyConfig {
                ctrl: true,
                alt: true,
                shift: false,
                win: false,
                key: "Escape".to_string(),
            },
            toggle_display_hotkey: HotkeyConfig {
                ctrl: true,
                alt: true,
                shift: false,
                win: false,
                key: "D".to_string(),
            },
            toggle_fullscreen_hotkey: HotkeyConfig {
                ctrl: true,
                alt: true,
                shift: false,
                win: false,
                key: "F".to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ReceiverConfig {
    pub allow_only_ip: Option<String>,
    pub prefer_scancode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DisplayConfig {
    pub enabled: bool,
    pub virtual_monitor: bool,
    pub match_source_resolution: bool,
    pub video_port: u16,
    pub monitor: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub fullscreen: bool,
    pub open_viewer_on_start: bool,
    pub capture_mouse: bool,
    pub auto_forward_on_viewer_focus: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            virtual_monitor: true,
            match_source_resolution: true,
            video_port: 24802,
            monitor: "auto".to_string(),
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: 16000,
            fullscreen: false,
            open_viewer_on_start: true,
            capture_mouse: true,
            auto_forward_on_viewer_focus: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TrayConfig {
    pub show_tooltip_latency: bool,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            show_tooltip_latency: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct UpdateConfig {
    pub enabled: bool,
    pub repo: String,
    pub check_interval_minutes: u64,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            repo: "SirAshesh/Key2PC".to_string(),
            check_interval_minutes: 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AppConfig {
    pub role: Role,
    pub device_name: String,
    pub log_file: Option<String>,
    pub network: NetworkConfig,
    pub sender: SenderConfig,
    pub receiver: ReceiverConfig,
    pub display: DisplayConfig,
    pub tray: TrayConfig,
    pub update: UpdateConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            role: Role::Sender,
            device_name: default_device_name(),
            log_file: None,
            network: NetworkConfig::default(),
            sender: SenderConfig::default(),
            receiver: ReceiverConfig::default(),
            display: DisplayConfig::default(),
            tray: TrayConfig::default(),
            update: UpdateConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn validate(&self) -> Result<()> {
        if self.network.port == 0 {
            return Err(Key2PcError::InvalidConfig(
                "network.port must be between 1 and 65535".to_string(),
            ));
        }
        if self.network.discovery_port == 0 {
            return Err(Key2PcError::InvalidConfig(
                "network.discovery_port must be between 1 and 65535".to_string(),
            ));
        }
        if self.display.video_port == 0 {
            return Err(Key2PcError::InvalidConfig(
                "display.video_port must be between 1 and 65535".to_string(),
            ));
        }
        if self.network.shared_secret.trim().is_empty() {
            return Err(Key2PcError::InvalidConfig(
                "network.shared_secret must not be empty".to_string(),
            ));
        }
        if self.network.retransmit_ms == 0 {
            return Err(Key2PcError::InvalidConfig(
                "network.retransmit_ms must be >= 1".to_string(),
            ));
        }
        if self.network.heartbeat_ms < 10 {
            return Err(Key2PcError::InvalidConfig(
                "network.heartbeat_ms must be >= 10".to_string(),
            ));
        }
        if self.network.disconnect_release_ms < 100 {
            return Err(Key2PcError::InvalidConfig(
                "network.disconnect_release_ms must be >= 100".to_string(),
            ));
        }
        if self.network.redundant_sends == 0 || self.network.redundant_sends > 4 {
            return Err(Key2PcError::InvalidConfig(
                "network.redundant_sends must be between 1 and 4".to_string(),
            ));
        }
        if self.display.width < 320
            || self.display.height < 240
            || !self.display.width.is_multiple_of(2)
            || !self.display.height.is_multiple_of(2)
        {
            return Err(Key2PcError::InvalidConfig(
                "display.width and display.height must be even integers >= 320x240".to_string(),
            ));
        }
        if self.display.fps < 15 || self.display.fps > 240 {
            return Err(Key2PcError::InvalidConfig(
                "display.fps must be between 15 and 240".to_string(),
            ));
        }
        if self.display.bitrate_kbps < 500 {
            return Err(Key2PcError::InvalidConfig(
                "display.bitrate_kbps must be >= 500".to_string(),
            ));
        }
        let monitor_trimmed = self.display.monitor.trim();
        if !monitor_trimmed.eq_ignore_ascii_case("auto")
            && monitor_trimmed.parse::<usize>().is_err()
        {
            return Err(Key2PcError::InvalidConfig(format!(
                "display.monitor must be 'auto' or an index, got '{}'",
                self.display.monitor
            )));
        }
        self.network.bind_ip.parse::<IpAddr>().map_err(|_| {
            Key2PcError::InvalidConfig(format!("Invalid network.bind_ip: {}", self.network.bind_ip))
        })?;
        if let Some(ip) = &self.receiver.allow_only_ip {
            ip.parse::<IpAddr>().map_err(|_| {
                Key2PcError::InvalidConfig(format!("Invalid receiver.allow_only_ip: {ip}"))
            })?;
        }
        self.sender.toggle_hotkey.to_binding()?;
        self.sender.release_all_hotkey.to_binding()?;
        self.sender.toggle_display_hotkey.to_binding()?;
        self.sender.toggle_fullscreen_hotkey.to_binding()?;
        if self.update.enabled {
            if !is_valid_github_repo(&self.update.repo) {
                return Err(Key2PcError::InvalidConfig(format!(
                    "update.repo must be 'owner/repo', got '{}'",
                    self.update.repo
                )));
            }
            if self.update.check_interval_minutes < 5 {
                return Err(Key2PcError::InvalidConfig(
                    "update.check_interval_minutes must be >= 5".to_string(),
                ));
            }
        }
        Ok(())
    }
}

fn is_valid_github_repo(repo: &str) -> bool {
    let parts: Vec<&str> = repo.trim().split('/').collect();
    if parts.len() != 2 {
        return false;
    }
    parts.iter().all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    })
}

fn default_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Key2PC-Device".to_string())
}
