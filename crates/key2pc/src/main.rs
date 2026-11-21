#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
mod app;

#[cfg(target_os = "windows")]
fn main() {
    std::process::exit(app::run());
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("Key2PC headless tray host currently targets Windows.");
    std::process::exit(1);
}
