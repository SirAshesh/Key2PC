use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossbeam_channel::Sender;

use crate::input::key_map::{HotkeyBinding, ModifierState};
use crate::input::types::{HookEvent, HookOptions, PressedKey, PressedKeySet, KEY2PC_EXTRA_INFO};
use crate::protocol::{KeyAction, KeyEvent};

pub struct HookProcessor {
    active: bool,
    shared_active: Arc<AtomicBool>,
    ignore_injected_keys: bool,
    toggle_hotkey: HotkeyBinding,
    release_all_hotkey: HotkeyBinding,
    toggle_display_hotkey: Option<HotkeyBinding>,
    toggle_fullscreen_hotkey: Option<HotkeyBinding>,
    modifiers: ModifierState,
    local_held: PressedKeySet,
    remote_held: PressedKeySet,
    suppressed_until_up: [bool; 256],
    event_tx: Sender<HookEvent>,
}

impl HookProcessor {
    pub fn new(
        options: HookOptions,
        shared_active: Arc<AtomicBool>,
        event_tx: Sender<HookEvent>,
    ) -> Self {
        shared_active.store(options.start_active, Ordering::Release);
        Self {
            active: options.start_active,
            shared_active,
            ignore_injected_keys: options.ignore_injected_keys,
            toggle_hotkey: options.toggle_hotkey,
            release_all_hotkey: options.release_all_hotkey,
            toggle_display_hotkey: options.toggle_display_hotkey,
            toggle_fullscreen_hotkey: options.toggle_fullscreen_hotkey,
            modifiers: ModifierState::default(),
            local_held: PressedKeySet::default(),
            remote_held: PressedKeySet::default(),
            suppressed_until_up: [false; 256],
            event_tx,
        }
    }

    pub fn handle_key_raw(
        &mut self,
        vk: u16,
        scan_code: u16,
        is_up: bool,
        extended: bool,
        alt_down: bool,
        injected: bool,
        extra_info: usize,
        local_release_fn: &mut dyn FnMut(&[PressedKey]),
    ) -> bool {
        if extra_info == KEY2PC_EXTRA_INFO {
            return false;
        }
        if self.ignore_injected_keys && injected {
            return false;
        }

        self.modifiers.update(vk, is_up);

        if self.modifiers.matches(&self.toggle_hotkey, vk, is_up) {
            self.mark_suppressed_until_up(vk);
            let next = !self.active;
            self.transition_active(next, local_release_fn);
            return true;
        }

        if self.modifiers.matches(&self.release_all_hotkey, vk, is_up) {
            self.mark_suppressed_until_up(vk);
            self.request_release_all();
            return true;
        }

        if let Some(display_hotkey) = &self.toggle_display_hotkey {
            if self.modifiers.matches(display_hotkey, vk, is_up) {
                self.mark_suppressed_until_up(vk);
                let _ = self.event_tx.try_send(HookEvent::ToggleDisplayRequested);
                return true;
            }
        }

        if let Some(fullscreen_hotkey) = &self.toggle_fullscreen_hotkey {
            if self.modifiers.matches(fullscreen_hotkey, vk, is_up) {
                self.mark_suppressed_until_up(vk);
                let _ = self.event_tx.try_send(HookEvent::ToggleFullscreenRequested);
                return true;
            }
        }

        if self.consume_if_suppressed(vk, is_up) {
            let key = PressedKey {
                vk,
                scan_code,
                extended,
            };
            self.local_held.track(key, true);
            self.remote_held.track(key, true);
            return true;
        }

        let key = PressedKey {
            vk,
            scan_code,
            extended,
        };

        if !self.active {
            self.local_held.track(key, is_up);
            return false;
        }

        self.remote_held.track(key, is_up);
        let event = KeyEvent {
            vk,
            scan_code,
            action: if is_up { KeyAction::Up } else { KeyAction::Down },
            extended,
            alt_down,
        };
        let _ = self.event_tx.try_send(HookEvent::Key(event));
        true
    }

    pub fn transition_active(
        &mut self,
        next: bool,
        local_release_fn: &mut dyn FnMut(&[PressedKey]),
    ) {
        if self.active == next {
            return;
        }
        self.active = next;
        self.shared_active.store(next, Ordering::Release);

        if next {
            let local_keys = self.local_held.take_all();
            for key in &local_keys {
                self.mark_suppressed_until_up(key.vk);
            }
            if !local_keys.is_empty() {
                local_release_fn(&local_keys);
            }
        } else {
            self.mark_held_remote_suppressed();
            let _ = self.event_tx.try_send(HookEvent::ReleaseAllRequested);
        }

        let _ = self.event_tx.try_send(HookEvent::ActiveChanged(next));
    }

    pub fn request_release_all(&mut self) {
        self.mark_held_remote_suppressed();
        let _ = self.event_tx.try_send(HookEvent::ReleaseAllRequested);
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    fn mark_held_remote_suppressed(&mut self) {
        let remote_keys = self.remote_held.take_all();
        for key in remote_keys {
            self.mark_suppressed_until_up(key.vk);
        }
    }

    fn mark_suppressed_until_up(&mut self, vk: u16) {
        if let Some(slot) = self.suppressed_until_up.get_mut(vk as usize) {
            *slot = true;
        }
    }

    fn consume_if_suppressed(&mut self, vk: u16, is_up: bool) -> bool {
        let Some(slot) = self.suppressed_until_up.get_mut(vk as usize) else {
            return false;
        };
        if !*slot {
            return false;
        }
        if is_up {
            *slot = false;
        }
        true
    }
}
