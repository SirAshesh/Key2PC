use key2pc_core::input::{
    format_virtual_key, parse_virtual_key, HotkeyBinding, ModifierState, VK_LCONTROL, VK_LMENU,
    VK_LSHIFT,
};

#[test]
fn parses_letters_digits_function_and_named_keys() {
    assert_eq!(parse_virtual_key("k").unwrap(), b'K' as u16);
    assert_eq!(parse_virtual_key("5").unwrap(), b'5' as u16);
    assert_eq!(parse_virtual_key("f12").unwrap(), 0x7B);
    assert_eq!(parse_virtual_key("ScrollLock").unwrap(), 0x91);
    assert_eq!(parse_virtual_key("0x1B").unwrap(), 0x1B);
    assert!(parse_virtual_key("NotARealKey").is_err());
    assert_eq!(format_virtual_key(0x7B), "F12");
}

#[test]
fn matches_exact_modifier_combinations_and_formats_label() {
    let binding = HotkeyBinding::new(true, true, false, false, b'K' as u16);
    assert_eq!(binding.display_label(), "Ctrl+Alt+K");

    let mut mods = ModifierState::default();
    mods.update(VK_LCONTROL, false);
    mods.update(VK_LMENU, false);
    assert!(mods.matches(&binding, b'K' as u16, false));
    assert!(!mods.matches(&binding, b'K' as u16, true));

    mods.update(VK_LSHIFT, false);
    assert!(!mods.matches(&binding, b'K' as u16, false));

    mods.clear();
    assert!(!mods.ctrl());
    assert!(!mods.alt());
    assert!(!mods.shift());
}
