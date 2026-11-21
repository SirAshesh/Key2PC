mod host;
mod timer;

use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use key2pc_core::tray::{TrayAction, TrayController};
use key2pc_core::{resolve_config_path, write_error_log, JsonFileConfigStore, Role, ServiceStatus};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, KillTimer, PostQuitMessage, PostThreadMessageW, SetTimer,
    TranslateMessage, MSG, WM_APP, WM_TIMER,
};

use host::AppHost;
use timer::TimerResolutionGuard;

const WM_APP_WAKE: u32 = WM_APP + 1;

pub fn run() -> i32 {
    let _timer_guard = TimerResolutionGuard::new(1);

    let explicit_config = parse_config_arg();
    let config_path = resolve_config_path(explicit_config.as_deref());
    let store = JsonFileConfigStore::new(config_path);

    let main_thread_id = unsafe { GetCurrentThreadId() };
    let wake_fn: Arc<dyn Fn() + Send + Sync> = Arc::new(move || unsafe {
        PostThreadMessageW(main_thread_id, WM_APP_WAKE, 0, 0);
    });

    let initial_status = ServiceStatus::error(
        Role::Sender,
        "Key2PC".to_string(),
        "Starting...".to_string(),
    );
    let latest_status = Arc::new(Mutex::new(initial_status));
    let status_dirty = Arc::new(AtomicBool::new(true));

    let mut host = AppHost::new(
        store,
        Arc::clone(&latest_status),
        Arc::clone(&status_dirty),
        Arc::clone(&wake_fn),
    );
    host.start_or_reload();

    let startup_snapshot = latest_status.lock().unwrap().clone();
    let (mut tray, action_rx) =
        match TrayController::new(&startup_snapshot, host.show_latency(), wake_fn) {
            Ok(res) => res,
            Err(err) => {
                write_error_log(host.log_file(), &err.to_string());
                host.shutdown();
                return 1;
            }
        };

    let timer_id = unsafe { SetTimer(ptr::null_mut(), 0, 250, None) };
    let mut msg: MSG = unsafe { std::mem::zeroed() };

    loop {
        while let Ok(action) = action_rx.try_recv() {
            match action {
                TrayAction::ToggleForwarding => host.toggle_active(),
                TrayAction::ToggleDisplay => host.toggle_display(),
                TrayAction::ToggleFullscreen => host.toggle_fullscreen(),
                TrayAction::ToggleFillScreen => host.toggle_fill_screen(),
                TrayAction::ToggleVirtualMonitor => host.toggle_virtual_monitor(),
                TrayAction::InstallVirtualDriver => host.install_virtual_driver(),
                TrayAction::ReleaseAll => host.release_all(),
                TrayAction::ReloadConfig => host.start_or_reload(),
                TrayAction::Exit => unsafe {
                    PostQuitMessage(0);
                },
            }
        }

        if status_dirty.swap(false, Ordering::AcqRel) {
            let snap = latest_status.lock().unwrap().clone();
            tray.apply_status(&snap, host.show_latency());
        }

        let status = unsafe { GetMessageW(&raw mut msg, ptr::null_mut(), 0, 0) };
        if status <= 0 {
            break;
        }
        if msg.message == WM_APP_WAKE || msg.message == WM_TIMER {
            continue;
        }
        unsafe {
            TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }

    if timer_id != 0 {
        unsafe {
            KillTimer(ptr::null_mut(), timer_id);
        }
    }
    host.shutdown();
    0
}

fn parse_config_arg() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--config" || arg == "-c" {
            if let Some(path) = args.next() {
                return Some(PathBuf::from(path));
            }
        }
    }
    None
}
