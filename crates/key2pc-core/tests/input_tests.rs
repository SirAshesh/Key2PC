use std::cell::RefCell;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use key2pc_core::input::{
    DesktopBounds, HookEvent, HookOptions, HookProcessor, HotkeyBinding, PressedKey, PressedKeySet,
    VK_LCONTROL, VK_LMENU,
};
use key2pc_core::protocol::{KeyAction, KeyEvent};

#[test]
fn toggle_hotkey_releases_local_modifiers_and_suppresses_orphaned_key_ups() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let shared = Arc::new(AtomicBool::new(false));
    let mut processor = HookProcessor::new(
        HookOptions {
            start_active: false,
            ignore_injected_keys: true,
            toggle_hotkey: HotkeyBinding::new(true, true, false, false, b'K' as u16),
            release_all_hotkey: HotkeyBinding::new(true, true, false, false, 0x1B),
            toggle_display_hotkey: Some(HotkeyBinding::new(true, true, false, false, b'D' as u16)),
        },
        shared,
        tx,
    );

    let released_local = RefCell::new(Vec::new());
    let mut release_cb = |keys: &[PressedKey]| {
        released_local.borrow_mut().extend_from_slice(keys);
    };

    assert!(!processor.handle_key_raw(VK_LCONTROL, 0x1D, false, false, false, false, 0, &mut release_cb));
    assert!(!processor.handle_key_raw(VK_LMENU, 0x38, false, false, true, false, 0, &mut release_cb));

    assert!(processor.handle_key_raw(b'K' as u16, 0x25, false, false, true, false, 0, &mut release_cb));
    assert!(processor.is_active());
    assert_eq!(released_local.borrow().len(), 2);

    assert!(processor.handle_key_raw(b'K' as u16, 0x25, true, false, true, false, 0, &mut release_cb));
    assert!(processor.handle_key_raw(VK_LMENU, 0x38, true, false, false, false, 0, &mut release_cb));
    assert!(processor.handle_key_raw(VK_LCONTROL, 0x1D, true, false, false, false, 0, &mut release_cb));

    assert!(processor.handle_key_raw(b'W' as u16, 0x11, false, false, false, false, 0, &mut release_cb));

    let events: Vec<HookEvent> = rx.try_iter().collect();
    assert_eq!(
        events,
        vec![
            HookEvent::ActiveChanged(true),
            HookEvent::Key(KeyEvent {
                vk: b'W' as u16,
                scan_code: 0x11,
                action: KeyAction::Down,
                extended: false,
                alt_down: false,
            })
        ]
    );
}

#[test]
fn pressed_key_set_deduplicates_repeat_presses() {
    let mut set = PressedKeySet::default();
    let key = PressedKey {
        vk: b'A' as u16,
        scan_code: 0x1E,
        extended: false,
    };
    set.track(key, false);
    set.track(key, false);
    assert_eq!(set.len(), 1);
    set.track(key, true);
    assert!(set.is_empty());
}

#[test]
fn desktop_bounds_maps_extended_monitor_coordinates() {
    let virtual_desktop = DesktopBounds::new(-1920, 0, 3840, 1080);
    let left_monitor = DesktopBounds::new(-1920, 0, 1920, 1080);
    let right_monitor = DesktopBounds::new(0, 0, 1920, 1080);

    let (lx, ly) = left_monitor.map_normalized_to_virtual(0, 32767, virtual_desktop);
    assert_eq!(lx, 0);
    assert!((ly - 32768).abs() <= 64);

    let (rx, ry) = right_monitor.map_normalized_to_virtual(65535, 65535, virtual_desktop);
    assert_eq!(rx, 65535);
    assert_eq!(ry, 65535);
}
