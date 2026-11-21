use tray_icon::Icon;

use crate::config::Role;
use crate::error::{Key2PcError, Result};
use crate::runtime::ServiceStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconPalette {
    Active,
    Standby,
    Searching,
    Error,
}

impl IconPalette {
    pub fn from_status(status: &ServiceStatus) -> Self {
        if status.error_message.is_some() {
            return Self::Error;
        }
        match status.role {
            Role::Sender => {
                if !status.connected {
                    Self::Searching
                } else if status.active {
                    Self::Active
                } else {
                    Self::Standby
                }
            }
            Role::Receiver => {
                if status.connected {
                    Self::Active
                } else {
                    Self::Standby
                }
            }
        }
    }

    fn rgb(self) -> [u8; 3] {
        match self {
            Self::Active => [16, 185, 129],
            Self::Standby => [59, 130, 246],
            Self::Searching => [245, 158, 11],
            Self::Error => [239, 68, 68],
        }
    }
}

pub fn build_status_icon(palette: IconPalette) -> Result<Icon> {
    let size = 32u32;
    let [r, g, b] = palette.rgb();
    let mut rgba = vec![0u8; (size * size * 4) as usize];

    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            if !is_inside_rounded_rect(x, y, 2, 2, 29, 29, 6) {
                continue;
            }

            if is_keyboard_glyph_pixel(x, y) {
                rgba[idx] = 255;
                rgba[idx + 1] = 255;
                rgba[idx + 2] = 255;
                rgba[idx + 3] = 255;
            } else {
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = 255;
            }
        }
    }

    Icon::from_rgba(rgba, size, size)
        .map_err(|err| Key2PcError::Tray(format!("Icon build failed: {err}")))
}

fn is_inside_rounded_rect(
    x: u32,
    y: u32,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    radius: u32,
) -> bool {
    if x < left || x > right || y < top || y > bottom {
        return false;
    }
    let cx = if x < left + radius {
        left + radius
    } else if x > right - radius {
        right - radius
    } else {
        return true;
    };
    let cy = if y < top + radius {
        top + radius
    } else if y > bottom - radius {
        bottom - radius
    } else {
        return true;
    };
    let dx = x.abs_diff(cx);
    let dy = y.abs_diff(cy);
    dx * dx + dy * dy <= radius * radius
}

fn is_keyboard_glyph_pixel(x: u32, y: u32) -> bool {
    let on_top_or_bottom_border = (y == 9 || y == 22) && (6..=25).contains(&x);
    let on_side_border = (x == 6 || x == 25) && (9..=22).contains(&y);
    if on_top_or_bottom_border || on_side_border {
        return true;
    }

    let row1 = (y == 12 || y == 13)
        && matches!(x, 9..=10 | 12..=13 | 15..=16 | 18..=19 | 21..=22);
    let row2 = (y == 15 || y == 16)
        && matches!(x, 9..=10 | 12..=13 | 15..=16 | 18..=19 | 21..=22);
    let spacebar = (y == 18 || y == 19) && (11..=20).contains(&x);

    row1 || row2 || spacebar
}
