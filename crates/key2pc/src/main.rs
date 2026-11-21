#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
mod app;

#[cfg(target_os = "windows")]
fn main() {
    if std::env::args_os().any(|arg| arg == "--install-vdd") {
        std::process::exit(key2pc_core::display::run_vdd_installer_cli());
    }
    std::process::exit(app::run());
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("Key2PC headless tray host currently targets Windows.");
    std::process::exit(1);
}
