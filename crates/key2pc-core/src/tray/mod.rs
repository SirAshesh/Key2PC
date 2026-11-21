#[cfg(target_os = "windows")]
mod icon;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use icon::{build_status_icon, IconPalette};
#[cfg(target_os = "windows")]
pub use windows::{TrayAction, TrayController};
