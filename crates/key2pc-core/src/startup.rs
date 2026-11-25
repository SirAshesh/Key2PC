use std::path::Path;

pub fn format_startup_command(exe_path: &Path, explicit_config: Option<&Path>) -> String {
    let exe = exe_path.to_string_lossy();
    match explicit_config {
        Some(cfg) => format!("\"{}\" --config \"{}\"", exe, cfg.to_string_lossy()),
        None => format!("\"{}\"", exe),
    }
}

#[cfg(target_os = "windows")]
mod win {
    use std::path::Path;

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ,
    };

    use crate::error::{Key2PcError, Result};
    use crate::startup::format_startup_command;

    const RUN_SUBKEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    const RUN_VALUE_NAME: PCWSTR = w!("Key2PC");

    struct RegKeyGuard(HKEY);

    impl Drop for RegKeyGuard {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                unsafe {
                    let _ = RegCloseKey(self.0);
                }
            }
        }
    }

    pub fn is_run_on_startup_enabled() -> bool {
        let mut size_bytes = 0u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                RUN_SUBKEY,
                RUN_VALUE_NAME,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size_bytes),
            )
        };
        if status.is_err() || size_bytes <= 2 {
            return false;
        }

        let mut buf = vec![0u16; (size_bytes as usize).div_ceil(2)];
        let status2 = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                RUN_SUBKEY,
                RUN_VALUE_NAME,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size_bytes),
            )
        };
        if status2.is_err() {
            return false;
        }

        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let value = String::from_utf16_lossy(&buf[..len]);
        !value.trim().is_empty()
    }

    pub fn set_run_on_startup(enable: bool, explicit_config: Option<&Path>) -> Result<()> {
        let mut raw_key = HKEY::default();
        let open_err = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                RUN_SUBKEY,
                Some(0),
                KEY_SET_VALUE,
                &mut raw_key,
            )
        };
        if open_err.is_err() {
            return Err(Key2PcError::Tray(format!(
                "Failed to open Windows Run registry key (error {})",
                open_err.0
            )));
        }
        let key = RegKeyGuard(raw_key);

        if enable {
            let exe = std::env::current_exe()?;
            let cmd = format_startup_command(&exe, explicit_config);
            let utf16: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes: Vec<u8> = utf16.iter().flat_map(|u| u.to_le_bytes()).collect();
            let set_err = unsafe { RegSetValueExW(key.0, RUN_VALUE_NAME, Some(0), REG_SZ, Some(&bytes)) };
            if set_err.is_err() {
                return Err(Key2PcError::Tray(format!(
                    "Failed to write Windows startup entry (error {})",
                    set_err.0
                )));
            }
        } else {
            let del_err = unsafe { RegDeleteValueW(key.0, RUN_VALUE_NAME) };
            if del_err.is_err() && del_err != ERROR_FILE_NOT_FOUND {
                return Err(Key2PcError::Tray(format!(
                    "Failed to remove Windows startup entry (error {})",
                    del_err.0
                )));
            }
        }
        Ok(())
    }

    pub fn sync_startup_path_if_enabled(explicit_config: Option<&Path>) {
        if is_run_on_startup_enabled() {
            let _ = set_run_on_startup(true, explicit_config);
        }
    }
}

#[cfg(target_os = "windows")]
pub use win::{is_run_on_startup_enabled, set_run_on_startup, sync_startup_path_if_enabled};
