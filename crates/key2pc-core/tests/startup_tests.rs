use std::path::Path;

use key2pc_core::startup::{format_startup_command, is_run_on_startup_enabled};

#[test]
fn formats_startup_command_with_and_without_config_path() {
    let exe = Path::new(r"C:\Users\Test User\AppData\Local\Programs\Key2PC\key2pc.exe");
    assert_eq!(
        format_startup_command(exe, None),
        r#""C:\Users\Test User\AppData\Local\Programs\Key2PC\key2pc.exe""#
    );

    let cfg = Path::new(r"D:\My Configs\receiver.json");
    assert_eq!(
        format_startup_command(exe, Some(cfg)),
        r#""C:\Users\Test User\AppData\Local\Programs\Key2PC\key2pc.exe" --config "D:\My Configs\receiver.json""#
    );
}

#[test]
fn queries_run_on_startup_status_without_error() {
    let _ = is_run_on_startup_enabled();
}
