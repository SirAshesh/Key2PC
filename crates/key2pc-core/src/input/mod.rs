mod key_map;
mod processor;
mod types;
#[cfg(target_os = "windows")]
mod windows;

pub use key_map::{
    format_virtual_key, is_modifier_vk, parse_virtual_key, HotkeyBinding, ModifierState,
    VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU,
    VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
pub use processor::HookProcessor;
pub use types::{HookControl, HookEvent, HookOptions, InputSink, PressedKey, PressedKeySet, KEY2PC_EXTRA_INFO};

#[cfg(target_os = "windows")]
pub use windows::{boost_current_thread_priority, WinKeyboardHook, WinKeyboardInjector};

#[cfg(not(target_os = "windows"))]
pub fn boost_current_thread_priority(_time_critical: bool) {}
