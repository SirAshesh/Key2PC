use crate::error::Result;
use crate::input::key_map::HotkeyBinding;
use crate::protocol::{KeyEvent, MouseEvent};

pub const KEY2PC_EXTRA_INFO: usize = 0x4B32_5043;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopBounds {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl DesktopBounds {
    pub fn new(left: i32, top: i32, width: i32, height: i32) -> Self {
        Self {
            left,
            top,
            width: width.max(1),
            height: height.max(1),
        }
    }

    pub fn map_normalized_to_virtual(
        self,
        norm_x: u16,
        norm_y: u16,
        virtual_desktop: DesktopBounds,
    ) -> (i32, i32) {
        let mon_w = i64::from((self.width - 1).max(1));
        let mon_h = i64::from((self.height - 1).max(1));
        let px_x = i64::from(self.left) + (i64::from(norm_x) * mon_w + 32767) / 65535;
        let px_y = i64::from(self.top) + (i64::from(norm_y) * mon_h + 32767) / 65535;

        let virt_w = i64::from((virtual_desktop.width - 1).max(1));
        let virt_h = i64::from((virtual_desktop.height - 1).max(1));
        let vx = ((px_x - i64::from(virtual_desktop.left)) * 65535 + virt_w / 2) / virt_w;
        let vy = ((px_y - i64::from(virtual_desktop.top)) * 65535 + virt_h / 2) / virt_h;

        (vx.clamp(0, 65535) as i32, vy.clamp(0, 65535) as i32)
    }
}

pub trait InputSink: Send {
    fn handle_key(&mut self, event: KeyEvent) -> Result<()>;
    fn handle_mouse(&mut self, _event: MouseEvent) -> Result<()> {
        Ok(())
    }
    fn set_monitor_bounds(&mut self, _bounds: DesktopBounds) {}
    fn release_all(&mut self) -> Result<()>;
    fn held_count(&self) -> usize;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PressedKey {
    pub vk: u16,
    pub scan_code: u16,
    pub extended: bool,
}

#[derive(Debug, Default, Clone)]
pub struct PressedKeySet {
    keys: Vec<PressedKey>,
}

impl PressedKeySet {
    pub fn track(&mut self, key: PressedKey, is_up: bool) {
        if is_up {
            self.keys
                .retain(|existing| !(existing.vk == key.vk && existing.extended == key.extended));
        } else if !self
            .keys
            .iter()
            .any(|existing| existing.vk == key.vk && existing.extended == key.extended)
        {
            self.keys.push(key);
        }
    }

    pub fn take_all(&mut self) -> Vec<PressedKey> {
        std::mem::take(&mut self.keys)
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn clear(&mut self) {
        self.keys.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    ActiveChanged(bool),
    ReleaseAllRequested,
    ToggleDisplayRequested,
}

#[derive(Debug, Clone)]
pub struct HookOptions {
    pub start_active: bool,
    pub ignore_injected_keys: bool,
    pub toggle_hotkey: HotkeyBinding,
    pub release_all_hotkey: HotkeyBinding,
    pub toggle_display_hotkey: Option<HotkeyBinding>,
}

#[derive(Debug, Clone, Copy)]
pub enum HookControl {
    SetActive(bool),
    ToggleActive,
    ReleaseAllRemote,
    Shutdown,
}
