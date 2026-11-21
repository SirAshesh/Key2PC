use std::sync::Arc;

use crossbeam_channel::Receiver;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

use crate::config::Role;
use crate::error::{Key2PcError, Result};
use crate::runtime::ServiceStatus;
use crate::tray::icon::{build_status_icon, IconPalette};

const TRAY_GUID: u128 = 0x6B65_7932_7063_7472_6179_0000_0000_0001;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ToggleForwarding,
    ToggleDisplay,
    ToggleFullscreen,
    ToggleFillScreen,
    ToggleVirtualMonitor,
    InstallVirtualDriver,
    ReleaseAll,
    ReloadConfig,
    Exit,
}

pub struct TrayController {
    tray_icon: TrayIcon,
    status_item: MenuItem,
    toggle_item: CheckMenuItem,
    display_item: CheckMenuItem,
    fullscreen_item: CheckMenuItem,
    fill_screen_item: CheckMenuItem,
    release_item: MenuItem,
    virtual_monitor_item: CheckMenuItem,
    install_vdd_item: MenuItem,
    show_latency: bool,
    current_palette: IconPalette,
}

impl TrayController {
    pub fn new(
        initial_status: &ServiceStatus,
        show_latency: bool,
        wake_callback: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(Self, Receiver<TrayAction>)> {
        let menu = Menu::new();
        let status_item = MenuItem::new("Key2PC", false, None);
        let is_sender_ok =
            initial_status.role == Role::Sender && initial_status.error_message.is_none();
        let is_receiver_ok =
            initial_status.role == Role::Receiver && initial_status.error_message.is_none();
        let toggle_item = CheckMenuItem::new(
            format_toggle_label(initial_status),
            is_sender_ok,
            initial_status.active,
            None,
        );
        let display_item = CheckMenuItem::new(
            format_display_label(initial_status),
            is_sender_ok && initial_status.display_enabled,
            initial_status.display_visible,
            None,
        );
        let fullscreen_item = CheckMenuItem::new(
            format_fullscreen_label(initial_status),
            is_sender_ok && initial_status.display_enabled,
            initial_status.display_visible && initial_status.display_fullscreen,
            None,
        );
        let fill_screen_item = CheckMenuItem::new(
            "Fill Screen Without Bars (F10)",
            is_sender_ok && initial_status.display_enabled,
            is_sender_ok && initial_status.display_enabled && initial_status.display_fill_screen,
            None,
        );
        let release_item = MenuItem::new("Release All Remote Keys", is_sender_ok, None);
        let virtual_monitor_item = CheckMenuItem::new(
            format_virtual_monitor_label(initial_status),
            is_receiver_ok
                && initial_status.display_enabled
                && initial_status.virtual_driver_installed,
            is_receiver_ok && initial_status.virtual_monitor_active,
            None,
        );
        let install_vdd_item = MenuItem::new(
            format_install_vdd_label(initial_status),
            is_receiver_ok && initial_status.display_enabled,
            None,
        );
        let reload_item = MenuItem::new("Reload config.json", true, None);
        let exit_item = MenuItem::new("Exit", true, None);

        menu.append(&status_item)
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;
        menu.append(&toggle_item)
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;
        menu.append(&display_item)
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;
        menu.append(&fullscreen_item)
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;
        menu.append(&fill_screen_item)
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;
        menu.append(&release_item)
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;
        menu.append(&virtual_monitor_item)
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;
        menu.append(&install_vdd_item)
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;
        menu.append(&reload_item)
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;
        menu.append(&exit_item)
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;

        let palette = IconPalette::from_status(initial_status);
        let icon = build_status_icon(palette)?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(format_tooltip(initial_status, show_latency))
            .with_icon(icon)
            .with_guid(TRAY_GUID)
            .with_menu_on_left_click(true)
            .with_menu_on_right_click(true)
            .build()
            .map_err(|err| Key2PcError::Tray(err.to_string()))?;

        let toggle_id: MenuId = toggle_item.id().clone();
        let display_id: MenuId = display_item.id().clone();
        let fullscreen_id: MenuId = fullscreen_item.id().clone();
        let fill_screen_id: MenuId = fill_screen_item.id().clone();
        let release_id: MenuId = release_item.id().clone();
        let virtual_mon_id: MenuId = virtual_monitor_item.id().clone();
        let install_vdd_id: MenuId = install_vdd_item.id().clone();
        let reload_id: MenuId = reload_item.id().clone();
        let exit_id: MenuId = exit_item.id().clone();

        let (action_tx, action_rx) = crossbeam_channel::unbounded();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = if event.id == toggle_id {
                Some(TrayAction::ToggleForwarding)
            } else if event.id == display_id {
                Some(TrayAction::ToggleDisplay)
            } else if event.id == fullscreen_id {
                Some(TrayAction::ToggleFullscreen)
            } else if event.id == fill_screen_id {
                Some(TrayAction::ToggleFillScreen)
            } else if event.id == virtual_mon_id {
                Some(TrayAction::ToggleVirtualMonitor)
            } else if event.id == install_vdd_id {
                Some(TrayAction::InstallVirtualDriver)
            } else if event.id == release_id {
                Some(TrayAction::ReleaseAll)
            } else if event.id == reload_id {
                Some(TrayAction::ReloadConfig)
            } else if event.id == exit_id {
                Some(TrayAction::Exit)
            } else {
                None
            };
            if let Some(act) = action {
                let _ = action_tx.send(act);
                wake_callback();
            }
        }));

        let mut controller = Self {
            tray_icon,
            status_item,
            toggle_item,
            display_item,
            fullscreen_item,
            fill_screen_item,
            release_item,
            virtual_monitor_item,
            install_vdd_item,
            show_latency,
            current_palette: palette,
        };
        controller.apply_status(initial_status, show_latency);
        Ok((controller, action_rx))
    }

    pub fn apply_status(&mut self, status: &ServiceStatus, show_latency: bool) {
        self.show_latency = show_latency;
        let palette = IconPalette::from_status(status);
        if palette != self.current_palette {
            if let Ok(icon) = build_status_icon(palette) {
                let _ = self.tray_icon.set_icon(Some(icon));
                self.current_palette = palette;
            }
        }

        let tooltip = format_tooltip(status, self.show_latency);
        let _ = self.tray_icon.set_tooltip(Some(tooltip));
        self.status_item.set_text(format_status_line(status));

        let is_sender_ok = status.role == Role::Sender && status.error_message.is_none();
        let is_receiver_ok = status.role == Role::Receiver && status.error_message.is_none();
        self.toggle_item.set_enabled(is_sender_ok);
        self.release_item.set_enabled(is_sender_ok);
        self.toggle_item.set_checked(is_sender_ok && status.active);
        self.toggle_item.set_text(format_toggle_label(status));

        self.display_item
            .set_enabled(is_sender_ok && status.display_enabled);
        self.display_item
            .set_checked(is_sender_ok && status.display_visible);
        self.display_item.set_text(format_display_label(status));

        self.fullscreen_item
            .set_enabled(is_sender_ok && status.display_enabled);
        self.fullscreen_item
            .set_checked(is_sender_ok && status.display_visible && status.display_fullscreen);
        self.fullscreen_item
            .set_text(format_fullscreen_label(status));

        self.fill_screen_item
            .set_enabled(is_sender_ok && status.display_enabled);
        self.fill_screen_item
            .set_checked(is_sender_ok && status.display_enabled && status.display_fill_screen);

        self.virtual_monitor_item.set_enabled(
            is_receiver_ok && status.display_enabled && status.virtual_driver_installed,
        );
        self.virtual_monitor_item
            .set_checked(is_receiver_ok && status.virtual_monitor_active);
        self.virtual_monitor_item
            .set_text(format_virtual_monitor_label(status));

        self.install_vdd_item
            .set_enabled(is_receiver_ok && status.display_enabled);
        self.install_vdd_item
            .set_text(format_install_vdd_label(status));
    }
}

fn format_toggle_label(status: &ServiceStatus) -> String {
    if status.hotkey_label.is_empty() {
        "Forward Keyboard to PC".to_string()
    } else {
        format!("Forward Keyboard to PC ({})", status.hotkey_label)
    }
}

fn format_display_label(status: &ServiceStatus) -> String {
    if status.display_hotkey_label.is_empty() {
        "Show Second Screen".to_string()
    } else {
        format!("Show Second Screen ({})", status.display_hotkey_label)
    }
}

fn format_fullscreen_label(status: &ServiceStatus) -> String {
    if status.display_fullscreen_hotkey_label.is_empty() {
        "Fullscreen Second Screen (F11 / Alt+Enter)".to_string()
    } else {
        format!(
            "Fullscreen Second Screen (F11 / {})",
            status.display_fullscreen_hotkey_label
        )
    }
}

fn format_virtual_monitor_label(status: &ServiceStatus) -> String {
    if status.role != Role::Receiver {
        "PC Virtual Extended Monitor (Receiver Only)".to_string()
    } else if !status.virtual_driver_installed {
        "PC Virtual Extended Monitor (Install Driver First)".to_string()
    } else {
        "PC Virtual Extended Monitor".to_string()
    }
}

fn format_install_vdd_label(status: &ServiceStatus) -> String {
    if status.role == Role::Receiver && status.virtual_driver_installed {
        "Virtual Monitor Driver Installed (Reinstall...)".to_string()
    } else {
        "Install Virtual Monitor Driver (UAC)...".to_string()
    }
}

fn format_status_line(status: &ServiceStatus) -> String {
    if let Some(err) = &status.error_message {
        return format!("Error: {err}");
    }
    match status.role {
        Role::Sender => {
            let mode = if status.active {
                "Active -> PC"
            } else {
                "Local Laptop"
            };
            match (status.connected, status.peer_address, status.rtt_ms) {
                (true, Some(addr), Some(rtt)) => format!("{mode} | {addr} ({rtt:.1} ms)"),
                (true, Some(addr), None) => format!("{mode} | {addr}"),
                (false, Some(addr), _) => format!("{mode} | Waiting {addr}"),
                _ => format!("{mode} | Searching PC..."),
            }
        }
        Role::Receiver => {
            let mon = if status.capture_monitor_label.is_empty() {
                "Listening".to_string()
            } else {
                status.capture_monitor_label.clone()
            };
            match (status.connected, status.peer_address) {
                (true, Some(addr)) => format!("Receiver | {} | {mon}", addr.ip()),
                _ => format!("Receiver | {mon}"),
            }
        }
    }
}

fn format_tooltip(status: &ServiceStatus, show_latency: bool) -> String {
    if let Some(err) = &status.error_message {
        return format!("Key2PC - Error: {err}");
    }
    match status.role {
        Role::Sender => {
            let mode = if status.active { "Active" } else { "Local" };
            let peer = status
                .peer_address
                .map(|a| a.to_string())
                .unwrap_or_else(|| "Searching LAN".to_string());
            if show_latency {
                if let Some(rtt) = status.rtt_ms {
                    return format!("Key2PC Sender ({mode}) -> {peer} [{rtt:.1} ms]");
                }
            }
            let state = if status.connected {
                "Online"
            } else {
                "Offline"
            };
            format!("Key2PC Sender ({mode}) -> {peer} [{state}]")
        }
        Role::Receiver => {
            let mon = if status.capture_monitor_label.is_empty() {
                "Screen"
            } else {
                &status.capture_monitor_label
            };
            match status.peer_address {
                Some(addr) if status.connected => {
                    format!("Key2PC Receiver | {} | {mon}", addr.ip())
                }
                _ => format!("Key2PC Receiver | {mon} | Waiting for Laptop"),
            }
        }
    }
}
