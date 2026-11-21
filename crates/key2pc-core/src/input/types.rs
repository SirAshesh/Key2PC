use crate::error::Result;
use crate::input::key_map::HotkeyBinding;
use crate::protocol::KeyEvent;

pub const KEY2PC_EXTRA_INFO: usize = 0x4B32_5043;

pub trait InputSink: Send {
    fn handle_key(&mut self, event: KeyEvent) -> Result<()>;
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
    ActiveChanged(bool),
    ReleaseAllRequested,
}

#[derive(Debug, Clone)]
pub struct HookOptions {
    pub start_active: bool,
    pub ignore_injected_keys: bool,
    pub toggle_hotkey: HotkeyBinding,
    pub release_all_hotkey: HotkeyBinding,
}

#[derive(Debug, Clone, Copy)]
pub enum HookControl {
    SetActive(bool),
    ToggleActive,
    ReleaseAllRemote,
    Shutdown,
}
