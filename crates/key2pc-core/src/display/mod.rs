#[cfg(target_os = "windows")]
mod capture;
#[cfg(target_os = "windows")]
mod codec;
#[cfg(target_os = "windows")]
mod viewer;
mod virtual_display;

use crate::error::Result;
use crate::input::DesktopBounds;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    pub index: usize,
    pub name: String,
    pub bounds: DesktopBounds,
    pub is_primary: bool,
    pub is_virtual: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureResult {
    Captured,
    Timeout,
    Reinitialized,
}

#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub is_keyframe: bool,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct DecodedFrame {
    pub nv12_data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub trait ScreenCapturer: Send {
    fn bounds(&self) -> DesktopBounds;
    fn target_size(&self) -> (u32, u32);
    fn capture_nv12(&mut self, timeout_ms: u32, out_nv12: &mut Vec<u8>) -> Result<CaptureResult>;
}

pub trait VideoEncoder: Send {
    fn encode_nv12(&mut self, nv12_data: &[u8], pts_100ns: i64) -> Result<Vec<EncodedFrame>>;
    fn force_keyframe(&mut self);
}

pub trait VideoDecoder: Send {
    fn decode_annex_b(&mut self, data: &[u8], pts_100ns: i64) -> Result<Vec<DecodedFrame>>;
}

pub fn select_monitor_index(setting: &str, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    let trimmed = setting.trim();
    if trimmed.eq_ignore_ascii_case("auto") {
        if count >= 2 {
            1
        } else {
            0
        }
    } else if let Ok(idx) = trimmed.parse::<usize>() {
        idx.min(count - 1)
    } else {
        0
    }
}

pub fn select_best_monitor(setting: &str, monitors: &[MonitorInfo]) -> usize {
    if monitors.is_empty() {
        return 0;
    }
    let trimmed = setting.trim();
    if trimmed.eq_ignore_ascii_case("auto") {
        if let Some(idx) = monitors.iter().position(|m| !m.is_primary && m.is_virtual) {
            return idx;
        }
        if let Some(idx) = monitors.iter().position(|m| m.is_virtual) {
            return idx;
        }
        if let Some(idx) = monitors.iter().position(|m| !m.is_primary) {
            return idx;
        }
        0
    } else if let Ok(idx) = trimmed.parse::<usize>() {
        idx.min(monitors.len() - 1)
    } else {
        0
    }
}

#[cfg(target_os = "windows")]
pub(crate) const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 0x1;

#[cfg(target_os = "windows")]
pub(crate) fn utf16_z_to_string(slice: &[u16]) -> String {
    let len = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
    String::from_utf16_lossy(&slice[..len])
}

pub use virtual_display::{
    is_virtual_display_signature, select_best_display_mode_index, VirtualDriverBackend,
    VirtualDriverStatus,
};
#[cfg(target_os = "windows")]
pub use virtual_display::{
    inspect_virtual_driver_status, request_elevated_vdd_install, run_vdd_installer_cli,
    VirtualMonitorGuard,
};
#[cfg(target_os = "windows")]
pub use capture::{enumerate_monitors, DxgiScreenCapturer};
#[cfg(target_os = "windows")]
pub use codec::{MfH264Decoder, MfH264Encoder};
#[cfg(target_os = "windows")]
pub use viewer::{DisplayViewer, ViewerCommand, ViewerEvent, ViewerOptions};
