use crate::error::{Key2PcError, Result};

const NAMED_KEYS: &[(&str, u16)] = &[
    ("BACKSPACE", 0x08),
    ("BACK", 0x08),
    ("TAB", 0x09),
    ("ENTER", 0x0D),
    ("RETURN", 0x0D),
    ("PAUSE", 0x13),
    ("CAPSLOCK", 0x14),
    ("CAPITAL", 0x14),
    ("ESCAPE", 0x1B),
    ("ESC", 0x1B),
    ("SPACE", 0x20),
    ("PAGEUP", 0x21),
    ("PGUP", 0x21),
    ("PRIOR", 0x21),
    ("PAGEDOWN", 0x22),
    ("PGDN", 0x22),
    ("NEXT", 0x22),
    ("END", 0x23),
    ("HOME", 0x24),
    ("LEFT", 0x25),
    ("UP", 0x26),
    ("RIGHT", 0x27),
    ("DOWN", 0x28),
    ("PRINTSCREEN", 0x2C),
    ("SNAPSHOT", 0x2C),
    ("INSERT", 0x2D),
    ("INS", 0x2D),
    ("DELETE", 0x2E),
    ("DEL", 0x2E),
    ("APPS", 0x5D),
    ("NUMLOCK", 0x90),
    ("SCROLLLOCK", 0x91),
    ("SCROLL", 0x91),
    ("SEMICOLON", 0xBA),
    ("EQUAL", 0xBB),
    ("COMMA", 0xBC),
    ("MINUS", 0xBD),
    ("PERIOD", 0xBE),
    ("SLASH", 0xBF),
    ("BACKTICK", 0xC0),
    ("GRAVE", 0xC0),
    ("TILDE", 0xC0),
    ("BRACKETLEFT", 0xDB),
    ("BACKSLASH", 0xDC),
    ("BRACKETRIGHT", 0xDD),
    ("QUOTE", 0xDE),
    ("NUMPAD0", 0x60),
    ("NUMPAD1", 0x61),
    ("NUMPAD2", 0x62),
    ("NUMPAD3", 0x63),
    ("NUMPAD4", 0x64),
    ("NUMPAD5", 0x65),
    ("NUMPAD6", 0x66),
    ("NUMPAD7", 0x67),
    ("NUMPAD8", 0x68),
    ("NUMPAD9", 0x69),
    ("MULTIPLY", 0x6A),
    ("ADD", 0x6B),
    ("SUBTRACT", 0x6D),
    ("DECIMAL", 0x6E),
    ("DIVIDE", 0x6F),
];

pub const VK_LSHIFT: u16 = 0xA0;
pub const VK_RSHIFT: u16 = 0xA1;
pub const VK_LCONTROL: u16 = 0xA2;
pub const VK_RCONTROL: u16 = 0xA3;
pub const VK_LMENU: u16 = 0xA4;
pub const VK_RMENU: u16 = 0xA5;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;
pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12;

pub fn parse_virtual_key(input: &str) -> Result<u16> {
    let normalized = input.trim().to_ascii_uppercase();
    if normalized.is_empty() {
        return Err(Key2PcError::InvalidConfig(
            "Empty hotkey key name".to_string(),
        ));
    }

    if normalized.len() == 1 {
        let ch = normalized.as_bytes()[0];
        if ch.is_ascii_uppercase() || ch.is_ascii_digit() {
            return Ok(u16::from(ch));
        }
    }

    if let Some(rest) = normalized.strip_prefix('F') {
        if let Ok(num) = rest.parse::<u16>() {
            if (1..=24).contains(&num) {
                return Ok(0x6F + num);
            }
        }
    }

    for &(name, vk) in NAMED_KEYS {
        if normalized == name {
            return Ok(vk);
        }
    }

    if let Some(hex) = normalized.strip_prefix("0X") {
        if let Ok(vk) = u16::from_str_radix(hex, 16) {
            if (1..=254).contains(&vk) {
                return Ok(vk);
            }
        }
    }

    if let Ok(vk) = normalized.parse::<u16>() {
        if (1..=254).contains(&vk) {
            return Ok(vk);
        }
    }

    Err(Key2PcError::InvalidConfig(format!(
        "Unknown hotkey key: {input}"
    )))
}

pub fn format_virtual_key(vk: u16) -> String {
    if (0x30..=0x39).contains(&vk) || (0x41..=0x5A).contains(&vk) {
        return ((vk as u8) as char).to_string();
    }
    if (0x70..=0x87).contains(&vk) {
        return format!("F{}", vk - 0x6F);
    }
    for &(name, code) in NAMED_KEYS {
        if code == vk {
            return name.to_string();
        }
    }
    format!("0x{vk:02X}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub vk: u16,
}

impl HotkeyBinding {
    pub fn new(ctrl: bool, alt: bool, shift: bool, win: bool, vk: u16) -> Self {
        Self {
            ctrl,
            alt,
            shift,
            win,
            vk,
        }
    }

    pub fn display_label(&self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.win {
            parts.push("Win".to_string());
        }
        parts.push(format_virtual_key(self.vk));
        parts.join("+")
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ModifierState {
    left_ctrl: bool,
    right_ctrl: bool,
    left_alt: bool,
    right_alt: bool,
    left_shift: bool,
    right_shift: bool,
    left_win: bool,
    right_win: bool,
    generic_ctrl: bool,
    generic_alt: bool,
    generic_shift: bool,
}

impl ModifierState {
    pub fn update(&mut self, vk: u16, is_up: bool) {
        let down = !is_up;
        match vk {
            VK_LCONTROL => self.left_ctrl = down,
            VK_RCONTROL => self.right_ctrl = down,
            VK_CONTROL => self.generic_ctrl = down,
            VK_LMENU => self.left_alt = down,
            VK_RMENU => self.right_alt = down,
            VK_MENU => self.generic_alt = down,
            VK_LSHIFT => self.left_shift = down,
            VK_RSHIFT => self.right_shift = down,
            VK_SHIFT => self.generic_shift = down,
            VK_LWIN => self.left_win = down,
            VK_RWIN => self.right_win = down,
            _ => {}
        }
    }

    pub fn ctrl(&self) -> bool {
        self.left_ctrl || self.right_ctrl || self.generic_ctrl
    }

    pub fn alt(&self) -> bool {
        self.left_alt || self.right_alt || self.generic_alt
    }

    pub fn shift(&self) -> bool {
        self.left_shift || self.right_shift || self.generic_shift
    }

    pub fn win(&self) -> bool {
        self.left_win || self.right_win
    }

    pub fn matches(&self, binding: &HotkeyBinding, vk: u16, is_up: bool) -> bool {
        !is_up
            && vk == binding.vk
            && self.ctrl() == binding.ctrl
            && self.alt() == binding.alt
            && self.shift() == binding.shift
            && self.win() == binding.win
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

pub fn is_modifier_vk(vk: u16) -> bool {
    matches!(
        vk,
        VK_LSHIFT
            | VK_RSHIFT
            | VK_SHIFT
            | VK_LCONTROL
            | VK_RCONTROL
            | VK_CONTROL
            | VK_LMENU
            | VK_RMENU
            | VK_MENU
            | VK_LWIN
            | VK_RWIN
    )
}
