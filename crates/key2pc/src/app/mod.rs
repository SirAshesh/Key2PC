mod host;
mod timer;

use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use key2pc_core::tray::{TrayAction, TrayController};
use key2pc_core::updater::{spawn_staged_update_installer, AutoUpdater, StagedUpdate};
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

    let relaunch_args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let disable_auto_update = relaunch_args.iter().any(|arg| arg == "--no-auto-update");
    let explicit_config = parse_config_arg();
    key2pc_core::startup::sync_startup_path_if_enabled(explicit_config.as_deref());
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

    let pending_update: Arc<Mutex<Option<StagedUpdate>>> = Arc::new(Mutex::new(None));
    let mut updater = AutoUpdater::start(
        host.update_config(),
        !disable_auto_update,
        Arc::clone(&pending_update),
        Arc::clone(&wake_fn),
    );

    let startup_snapshot = latest_status.lock().unwrap().clone();
    let (mut tray, action_rx) =
        match TrayController::new(&startup_snapshot, host.show_latency(), wake_fn) {
            Ok(res) => res,
            Err(err) => {
                write_error_log(host.log_file(), &err.to_string());
                updater.shutdown();
                host.shutdown();
                return 1;
            }
        };

    let timer_id = unsafe { SetTimer(ptr::null_mut(), 0, 250, None) };
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    let mut update_to_apply: Option<StagedUpdate> = None;

    loop {
        while let Ok(action) = action_rx.try_recv() {
            match action {
                TrayAction::ToggleForwarding => host.toggle_active(),
                TrayAction::ToggleDisplay => host.toggle_display(),
                TrayAction::ToggleFullscreen => host.toggle_fullscreen(),
                TrayAction::ToggleVirtualMonitor => host.toggle_virtual_monitor(),
                TrayAction::InstallVirtualDriver => host.install_virtual_driver(),
                TrayAction::ReleaseAll => host.release_all(),
                TrayAction::ToggleRunOnStartup => {
                    let next = !key2pc_core::startup::is_run_on_startup_enabled();
                    match key2pc_core::startup::set_run_on_startup(next, explicit_config.as_deref())
                    {
                        Ok(()) => tray.set_run_on_startup_checked(next),
                        Err(err) => {
                            write_error_log(host.log_file(), &err.to_string());
                            tray.set_run_on_startup_checked(
                                key2pc_core::startup::is_run_on_startup_enabled(),
                            );
                        }
                    }
                }
                TrayAction::CheckForUpdates => updater.trigger_manual_check(),
                TrayAction::ReloadConfig => {
                    host.start_or_reload();
                    updater.update_config(host.update_config());
                }
                TrayAction::Exit => unsafe {
                    PostQuitMessage(0);
                },
            }
        }

        if let Ok(mut guard) = pending_update.lock() {
            if let Some(staged) = guard.take() {
                update_to_apply = Some(staged);
                break;
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
    updater.shutdown();
    host.shutdown();
    if let Some(staged) = update_to_apply {
        if let Err(err) = spawn_staged_update_installer(&staged, &relaunch_args) {
            write_error_log(host.log_file(), &format!("Auto-update launch failed: {err}"));
        }
    }
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
