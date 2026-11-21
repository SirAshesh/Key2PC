#[cfg(target_os = "windows")]
mod capture;
#[cfg(target_os = "windows")]
mod codec;
#[cfg(target_os = "windows")]
mod viewer;

use crate::error::Result;
use crate::input::DesktopBounds;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    pub index: usize,
    pub name: String,
    pub bounds: DesktopBounds,
    pub is_primary: bool,
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

#[cfg(target_os = "windows")]
pub use capture::{enumerate_monitors, DxgiScreenCapturer};
#[cfg(target_os = "windows")]
pub use codec::{MfH264Decoder, MfH264Encoder};
#[cfg(target_os = "windows")]
pub use viewer::{DisplayViewer, ViewerCommand, ViewerEvent, ViewerOptions};
