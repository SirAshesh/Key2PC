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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerShapeDesc {
    pub pointer_type: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
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

pub fn compute_encode_dimensions(
    src_w: u32,
    src_h: u32,
    cfg_w: u32,
    cfg_h: u32,
    match_source: bool,
) -> (u32, u32) {
    if !match_source || src_w < 320 || src_h < 240 {
        return ((cfg_w.max(320)) & !1, (cfg_h.max(240)) & !1);
    }
    let max_w = cfg_w.max(2560);
    let max_h = cfg_h.max(1600);
    if src_w <= max_w && src_h <= max_h {
        return ((src_w.max(320)) & !1, (src_h.max(240)) & !1);
    }
    let sx = f64::from(max_w) / f64::from(src_w);
    let sy = f64::from(max_h) / f64::from(src_h);
    let scale = sx.min(sy);
    let w = ((f64::from(src_w) * scale).floor() as u32).max(320) & !1;
    let h = ((f64::from(src_h) * scale).floor() as u32).max(240) & !1;
    (w, h)
}

#[cfg(target_os = "windows")]
pub fn enable_dpi_awareness() {
    let _ = unsafe {
        windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
    };
}

#[cfg(target_os = "windows")]
pub(crate) const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 0x1;

#[cfg(target_os = "windows")]
pub(crate) fn utf16_z_to_string(slice: &[u16]) -> String {
    let len = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
    String::from_utf16_lossy(&slice[..len])
}

#[cfg(target_os = "windows")]
pub use capture::{composite_dxgi_pointer_over_bgra, enumerate_monitors, DxgiScreenCapturer};
#[cfg(target_os = "windows")]
pub use codec::{MfH264Decoder, MfH264Encoder};
#[cfg(target_os = "windows")]
pub use viewer::{DisplayViewer, ViewerCommand, ViewerEvent, ViewerOptions};
#[cfg(target_os = "windows")]
pub use virtual_display::{
    inspect_virtual_driver_status, request_elevated_vdd_install, run_vdd_installer_cli,
    save_current_virtual_display_layout, VirtualMonitorGuard,
};
pub use virtual_display::{
    compute_restored_monitor_placements, is_virtual_display_signature,
    select_best_display_mode_index, LiveMonitorRect, PlannedMonitorChange, SavedDisplayLayout,
    SavedMonitorEntry, VirtualDriverBackend, VirtualDriverStatus,
};
