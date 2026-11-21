#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualDriverBackend {
    ParsecVdd,
    MttVdd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualDriverStatus {
    Ready(VirtualDriverBackend),
    RestartRequired,
    DriverError(u32),
    NotInstalled,
}

impl VirtualDriverStatus {
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready(_))
    }

    pub fn backend(self) -> Option<VirtualDriverBackend> {
        match self {
            Self::Ready(backend) => Some(backend),
            _ => None,
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Self::Ready(VirtualDriverBackend::ParsecVdd) => "Parsec VDD Ready",
            Self::Ready(VirtualDriverBackend::MttVdd) => "Virtual Display Driver Ready",
            Self::RestartRequired => "Restart Required",
            Self::DriverError(_) => "Driver Error",
            Self::NotInstalled => "Not Installed",
        }
    }
}

pub fn is_virtual_display_signature(
    device_name: &str,
    device_string: &str,
    device_id: &str,
) -> bool {
    let combined = format!(
        "{} {} {}",
        device_name.to_ascii_lowercase(),
        device_string.to_ascii_lowercase(),
        device_id.to_ascii_lowercase()
    );
    [
        "psccdd0",
        "parsec",
        "mttvdd",
        "vdd by mtt",
        "virtual display",
        "iddsample",
    ]
    .iter()
    .any(|needle| combined.contains(needle))
}

pub fn select_best_display_mode_index(
    modes: &[(u32, u32, u32)],
    target_w: u32,
    target_h: u32,
    target_fps: u32,
) -> Option<usize> {
    if modes.is_empty() {
        return None;
    }

    let mut best_idx = 0usize;
    let mut best_score = (u64::MAX, u64::MAX, u64::MAX);

    for (idx, &(w, h, hz)) in modes.iter().enumerate() {
        let exact_res_tier = if w == target_w && h == target_h {
            0u64
        } else if w == 1920 && h == 1080 {
            1u64
        } else if w >= target_w && h >= target_h {
            2u64
        } else {
            3u64
        };

        let area_target = u64::from(target_w) * u64::from(target_h);
        let area_mode = u64::from(w) * u64::from(h);
        let area_diff = area_target.abs_diff(area_mode);
        let fps_diff = u64::from(target_fps.abs_diff(hz));

        let score = (exact_res_tier, area_diff, fps_diff);
        if score < best_score {
            best_score = score;
            best_idx = idx;
        }
    }

    Some(best_idx)
}

#[cfg(target_os = "windows")]
mod win {
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use windows::core::{w, GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_Status, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo,
        SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
        SetupDiGetDeviceRegistryPropertyW, CM_DEVNODE_STATUS_FLAGS, CM_PROB, CM_PROB_NEED_RESTART,
        CR_SUCCESS, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, DN_DRIVER_LOADED, DN_HAS_PROBLEM,
        DN_STARTED, SPDRP_HARDWAREID, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
        SP_DEVINFO_DATA,
    };
    use windows::Win32::Devices::Display::{
        SetDisplayConfig, SDC_ALLOW_CHANGES, SDC_APPLY, SDC_TOPOLOGY_EXTEND,
    };
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE, POINTL,
    };
    use windows::Win32::Graphics::Gdi::{
        ChangeDisplaySettingsExW, EnumDisplayDevicesW, EnumDisplaySettingsW, CDS_NORESET,
        CDS_UPDATEREGISTRY, DEVMODEW, DISPLAY_DEVICEW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP,
        DISPLAY_DEVICE_PRIMARY_DEVICE, DISP_CHANGE_SUCCESSFUL, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT,
        DM_PELSWIDTH, DM_POSITION, ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_MODE,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED,
        FILE_FLAG_WRITE_THROUGH, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Threading::CreateEventW;
    use windows::Win32::System::IO::{DeviceIoControl, GetOverlappedResultEx, OVERLAPPED};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, SW_SHOWNORMAL,
    };

    use crate::display::virtual_display::{
        is_virtual_display_signature, select_best_display_mode_index, VirtualDriverBackend,
        VirtualDriverStatus,
    };
    use crate::display::{utf16_z_to_string, EDD_GET_DEVICE_INTERFACE_NAME};
    use crate::error::{Key2PcError, Result};

    const VDD_ADAPTER_GUID: GUID = GUID::from_values(
        0x00b4_1627,
        0x04c4,
        0x429e,
        [0xa2, 0x6e, 0x02, 0x65, 0xcf, 0x50, 0xc8, 0xfa],
    );
    const VDD_CLASS_GUID: GUID = GUID::from_values(
        0x4d36_e968,
        0xe325,
        0x11ce,
        [0xbf, 0xc1, 0x08, 0x00, 0x2b, 0xe1, 0x03, 0x18],
    );
    const PARSEC_HW_ID: &str = "root\\parsec\\vda";
    const MTT_HW_ID: &str = "root\\mttvdd";
    const VDD_IOCTL_ADD: u32 = 0x0022_e004;
    const VDD_IOCTL_REMOVE: u32 = 0x0022_a008;
    const VDD_IOCTL_UPDATE: u32 = 0x0022_a00c;
    const PARSEC_DRIVER_URL: &str =
        "https://raw.githubusercontent.com/nomi-san/parsec-vdd/distro/drivers/parsec-vdd-0.45.0.0.exe";

    struct RawHandle(HANDLE);

    unsafe impl Send for RawHandle {}
    unsafe impl Sync for RawHandle {}

    impl Drop for RawHandle {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    #[derive(Debug, Clone)]
    struct EnumeratedGdiDisplay {
        device_name: String,
        is_primary: bool,
        is_attached: bool,
        is_virtual: bool,
    }

    pub struct VirtualMonitorGuard {
        backend: VirtualDriverBackend,
        handle: Option<Arc<RawHandle>>,
        driver_index: Option<i32>,
        keepalive_stop: Arc<(Mutex<bool>, Condvar)>,
        keepalive_running: Arc<AtomicBool>,
        keepalive_handle: Option<JoinHandle<()>>,
        pub device_name: Option<String>,
    }

    impl VirtualMonitorGuard {
        pub fn acquire(width: u32, height: u32, fps: u32) -> Result<Self> {
            let status = inspect_virtual_driver_status();
            let backend = match status {
                VirtualDriverStatus::Ready(b) => b,
                VirtualDriverStatus::RestartRequired => {
                    return Err(Key2PcError::Display(
                        "Virtual display driver requires a Windows restart before use".to_string(),
                    ));
                }
                VirtualDriverStatus::DriverError(code) => {
                    return Err(Key2PcError::Display(format!(
                        "Virtual display driver reported error code {code}"
                    )));
                }
                VirtualDriverStatus::NotInstalled => {
                    return Err(Key2PcError::Display(
                        "No virtual display driver installed".to_string(),
                    ));
                }
            };

            match backend {
                VirtualDriverBackend::ParsecVdd => Self::acquire_parsec(width, height, fps),
                VirtualDriverBackend::MttVdd => Self::acquire_mtt(width, height, fps),
            }
        }

        fn acquire_parsec(width: u32, height: u32, fps: u32) -> Result<Self> {
            let existing_virtual = find_virtual_gdi_displays();
            let raw_handle = Arc::new(RawHandle(open_parsec_adapter_handle()?));
            let idx = parsec_vdd_ioctl(raw_handle.0, VDD_IOCTL_ADD, &[])? as i32;
            if idx < 0 {
                return Err(Key2PcError::Display(format!(
                    "Parsec VDD_IOCTL_ADD failed with code {idx}"
                )));
            }
            let _ = parsec_vdd_ioctl(raw_handle.0, VDD_IOCTL_UPDATE, &[]);

            let keepalive_running = Arc::new(AtomicBool::new(true));
            let keepalive_stop = Arc::new((Mutex::new(false), Condvar::new()));
            let thread_handle_ref = Arc::clone(&raw_handle);
            let thread_running = Arc::clone(&keepalive_running);
            let thread_stop = Arc::clone(&keepalive_stop);

            let keepalive_handle = thread::Builder::new()
                .name("key2pc-vdd-ping".to_string())
                .spawn(move || {
                    let (lock, cvar) = &*thread_stop;
                    while thread_running.load(Ordering::Acquire) {
                        let _ = parsec_vdd_ioctl(thread_handle_ref.0, VDD_IOCTL_UPDATE, &[]);
                        let guard = lock.lock().unwrap();
                        if *guard {
                            break;
                        }
                        let _ = cvar.wait_timeout(guard, Duration::from_millis(100));
                    }
                })
                .map_err(Key2PcError::Io)?;

            let mut guard = Self {
                backend: VirtualDriverBackend::ParsecVdd,
                handle: Some(raw_handle),
                driver_index: Some(idx),
                keepalive_stop,
                keepalive_running,
                keepalive_handle: Some(keepalive_handle),
                device_name: None,
            };

            guard.device_name = wait_and_extend_virtual_monitor(
                &existing_virtual,
                width,
                height,
                fps,
                Duration::from_millis(2500),
            );
            Ok(guard)
        }

        fn acquire_mtt(width: u32, height: u32, fps: u32) -> Result<Self> {
            let mut guard = Self {
                backend: VirtualDriverBackend::MttVdd,
                handle: None,
                driver_index: None,
                keepalive_stop: Arc::new((Mutex::new(false), Condvar::new())),
                keepalive_running: Arc::new(AtomicBool::new(false)),
                keepalive_handle: None,
                device_name: None,
            };
            guard.device_name = wait_and_extend_virtual_monitor(
                &[],
                width,
                height,
                fps,
                Duration::from_millis(1000),
            );
            if guard.device_name.is_none() {
                return Err(Key2PcError::Display(
                    "Virtual Display Driver detected but no active virtual monitor appeared"
                        .to_string(),
                ));
            }
            Ok(guard)
        }

        pub fn backend(&self) -> VirtualDriverBackend {
            self.backend
        }

        pub fn shutdown(&mut self) {
            self.keepalive_running.store(false, Ordering::Release);
            {
                let (lock, cvar) = &*self.keepalive_stop;
                if let Ok(mut stop) = lock.lock() {
                    *stop = true;
                }
                cvar.notify_all();
            }
            if let Some(h) = self.keepalive_handle.take() {
                let _ = h.join();
            }
            if self.backend == VirtualDriverBackend::ParsecVdd {
                if let (Some(handle), Some(idx)) = (self.handle.take(), self.driver_index.take()) {
                    if idx >= 0 {
                        let bytes = (idx as u16).to_be_bytes();
                        let _ = parsec_vdd_ioctl(handle.0, VDD_IOCTL_REMOVE, &bytes);
                        let _ = parsec_vdd_ioctl(handle.0, VDD_IOCTL_UPDATE, &[]);
                    }
                }
            }
        }
    }

    impl Drop for VirtualMonitorGuard {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    pub fn inspect_virtual_driver_status() -> VirtualDriverStatus {
        let Ok(dev_info) = (unsafe {
            SetupDiGetClassDevsW(Some(&VDD_CLASS_GUID), PCWSTR::null(), None, DIGCF_PRESENT)
        }) else {
            return VirtualDriverStatus::NotInstalled;
        };

        let mut parsec_status: Option<VirtualDriverStatus> = None;
        let mut mtt_status: Option<VirtualDriverStatus> = None;
        let mut idx = 0u32;

        loop {
            let mut dev_data = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            if unsafe { SetupDiEnumDeviceInfo(dev_info, idx, &mut dev_data) }.is_err() {
                break;
            }
            idx += 1;

            let hw_ids = read_hardware_ids(dev_info, &dev_data);
            if hw_ids.iter().any(|id| id == PARSEC_HW_ID) {
                parsec_status = Some(read_devnode_status(
                    dev_data.DevInst,
                    VirtualDriverBackend::ParsecVdd,
                ));
            } else if hw_ids.iter().any(|id| id == MTT_HW_ID) {
                mtt_status = Some(read_devnode_status(
                    dev_data.DevInst,
                    VirtualDriverBackend::MttVdd,
                ));
            }
        }

        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(dev_info);
        }

        parsec_status
            .or(mtt_status)
            .unwrap_or(VirtualDriverStatus::NotInstalled)
    }

    pub fn request_elevated_vdd_install() -> Result<()> {
        let exe = std::env::current_exe().map_err(Key2PcError::Io)?;
        let file_w: Vec<u16> = exe
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let params_w = w!("--install-vdd");
        let verb_w = w!("runas");
        let res = unsafe {
            ShellExecuteW(
                None,
                verb_w,
                PCWSTR(file_w.as_ptr()),
                params_w,
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if (res.0 as isize) <= 32 {
            return Err(Key2PcError::Display(format!(
                "UAC driver setup cancelled or failed (ShellExecute code {})",
                res.0 as isize
            )));
        }
        Ok(())
    }

    pub fn run_vdd_installer_cli() -> i32 {
        match install_parsec_vdd_silent() {
            Ok(msg) => {
                show_message_box("Key2PC Virtual Display Setup", &msg, false);
                0
            }
            Err(err) => {
                show_message_box("Key2PC Virtual Display Setup", &err.to_string(), true);
                1
            }
        }
    }

    fn install_parsec_vdd_silent() -> Result<String> {
        if let VirtualDriverStatus::Ready(backend) = inspect_virtual_driver_status() {
            return Ok(format!(
                "{} is already installed and ready.",
                VirtualDriverStatus::Ready(backend).summary()
            ));
        }

        let local_driver = std::env::current_exe().ok().and_then(|p| {
            p.parent()
                .map(|dir| dir.join("drivers").join("parsec-vdd-0.45.0.0.exe"))
        });

        let (installer_path, cleanup_dir): (PathBuf, Option<PathBuf>) =
            if let Some(local) = local_driver.filter(|p| p.is_file()) {
                (local, None)
            } else {
                let dir = std::env::temp_dir().join(format!("key2pc-vdd-{}", std::process::id()));
                std::fs::create_dir_all(&dir)?;
                let target = dir.join("parsec-vdd-0.45.0.0.exe");
                download_parsec_driver_installer(&target)?;
                (target, Some(dir))
            };

        verify_authenticode_signature(&installer_path)?;

        let status = Command::new(&installer_path).arg("/S").status();
        if let Some(dir) = cleanup_dir {
            let _ = std::fs::remove_dir_all(dir);
        }

        let exit = status?;
        if !exit.success() {
            return Err(Key2PcError::Display(format!(
                "Driver installer exited with status {}",
                exit
            )));
        }

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let drv = inspect_virtual_driver_status();
            match drv {
                VirtualDriverStatus::Ready(_) => {
                    return Ok(
                        "Parsec Virtual Display Driver installed and ready. Enable 'Virtual Extended Monitor' in Key2PC tray on your PC."
                            .to_string(),
                    );
                }
                VirtualDriverStatus::RestartRequired => {
                    return Ok(
                        "Parsec Virtual Display Driver installed. Windows requires a PC restart before the virtual monitor can activate."
                            .to_string(),
                    );
                }
                VirtualDriverStatus::DriverError(code) => {
                    return Err(Key2PcError::Display(format!(
                        "Driver installed but Windows reported device error {code}"
                    )));
                }
                VirtualDriverStatus::NotInstalled => {
                    if Instant::now() >= deadline {
                        return Err(Key2PcError::Display(
                            "Driver installer finished, but Root\\Parsec\\VDA device did not appear"
                                .to_string(),
                        ));
                    }
                    thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }

    fn download_parsec_driver_installer(target: &std::path::Path) -> Result<()> {
        let target_str = target.to_string_lossy();
        let curl_ok = Command::new("curl.exe")
            .args([
                "-fsSL",
                "--retry",
                "2",
                "-o",
                &target_str,
                PARSEC_DRIVER_URL,
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !curl_ok || !target.is_file() {
            let ps_script = format!(
                "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri '{}' -OutFile '{}' -UseBasicParsing",
                PARSEC_DRIVER_URL,
                target_str.replace('\'', "''")
            );
            let ps_status = Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    &ps_script,
                ])
                .status()?;
            if !ps_status.success() {
                return Err(Key2PcError::Display(
                    "Failed to download signed Parsec Virtual Display Driver from GitHub"
                        .to_string(),
                ));
            }
        }

        let meta = std::fs::metadata(target)?;
        if meta.len() < 100_000 {
            return Err(Key2PcError::Display(
                "Downloaded driver installer is incomplete or corrupted".to_string(),
            ));
        }
        Ok(())
    }

    fn verify_authenticode_signature(path: &std::path::Path) -> Result<()> {
        let ps_cmd = format!(
            "$sig = Get-AuthenticodeSignature -LiteralPath '{}'; if ($sig.Status -ne 'Valid') {{ exit 2 }}",
            path.to_string_lossy().replace('\'', "''")
        );
        let status = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &ps_cmd,
            ])
            .status()?;
        if !status.success() {
            return Err(Key2PcError::Display(
                "Driver installer failed Authenticode signature check".to_string(),
            ));
        }
        Ok(())
    }

    fn show_message_box(title: &str, text: &str, is_error: bool) {
        let title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let text_w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let flags = if is_error {
            MB_OK | MB_ICONERROR
        } else {
            MB_OK | MB_ICONINFORMATION
        };
        unsafe {
            let _ = MessageBoxW(
                None,
                PCWSTR(text_w.as_ptr()),
                PCWSTR(title_w.as_ptr()),
                flags,
            );
        }
    }

    fn read_hardware_ids(
        dev_info: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
        dev_data: &SP_DEVINFO_DATA,
    ) -> Vec<String> {
        let mut required = 0u32;
        let _ = unsafe {
            SetupDiGetDeviceRegistryPropertyW(
                dev_info,
                dev_data,
                SPDRP_HARDWAREID,
                None,
                None,
                Some(&mut required),
            )
        };
        if required == 0 {
            return Vec::new();
        }

        let mut buf = vec![0u8; required as usize];
        let mut reg_type = 0u32;
        if unsafe {
            SetupDiGetDeviceRegistryPropertyW(
                dev_info,
                dev_data,
                SPDRP_HARDWAREID,
                Some(&mut reg_type),
                Some(&mut buf),
                Some(&mut required),
            )
        }
        .is_err()
        {
            return Vec::new();
        }

        let wide: Vec<u16> = buf
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        wide.split(|&ch| ch == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf16_lossy(part).to_ascii_lowercase())
            .collect()
    }

    fn read_devnode_status(dev_inst: u32, backend: VirtualDriverBackend) -> VirtualDriverStatus {
        let mut dev_status = CM_DEVNODE_STATUS_FLAGS(0);
        let mut problem = CM_PROB(0);
        let cr = unsafe { CM_Get_DevNode_Status(&mut dev_status, &mut problem, dev_inst, 0) };
        if cr != CR_SUCCESS {
            return VirtualDriverStatus::NotInstalled;
        }
        if (dev_status & (DN_DRIVER_LOADED | DN_STARTED)) != CM_DEVNODE_STATUS_FLAGS(0) {
            return VirtualDriverStatus::Ready(backend);
        }
        if (dev_status & DN_HAS_PROBLEM) != CM_DEVNODE_STATUS_FLAGS(0) {
            if problem == CM_PROB_NEED_RESTART {
                return VirtualDriverStatus::RestartRequired;
            }
            return VirtualDriverStatus::DriverError(problem.0);
        }
        VirtualDriverStatus::DriverError(0)
    }

    fn open_parsec_adapter_handle() -> Result<HANDLE> {
        let dev_info = unsafe {
            SetupDiGetClassDevsW(
                Some(&VDD_ADAPTER_GUID),
                PCWSTR::null(),
                None,
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )
        }
        .map_err(|err| Key2PcError::Display(format!("SetupDiGetClassDevsW failed: {err}")))?;

        let mut opened: Option<HANDLE> = None;
        let mut idx = 0u32;

        loop {
            let mut iface_data = SP_DEVICE_INTERFACE_DATA {
                cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if unsafe {
                SetupDiEnumDeviceInterfaces(dev_info, None, &VDD_ADAPTER_GUID, idx, &mut iface_data)
            }
            .is_err()
            {
                break;
            }
            idx += 1;

            let mut detail_size = 0u32;
            let _ = unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    dev_info,
                    &iface_data,
                    None,
                    0,
                    Some(&mut detail_size),
                    None,
                )
            };
            if detail_size == 0 {
                continue;
            }

            let mut detail_buf = vec![
                0u8;
                (detail_size as usize).max(std::mem::size_of::<
                    SP_DEVICE_INTERFACE_DETAIL_DATA_W,
                >())
            ];
            let detail_ptr = detail_buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            unsafe {
                (*detail_ptr).cbSize =
                    std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            }

            if unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    dev_info,
                    &iface_data,
                    Some(detail_ptr),
                    detail_size,
                    Some(&mut detail_size),
                    None,
                )
            }
            .is_err()
            {
                continue;
            }

            let path_ptr = unsafe { std::ptr::addr_of!((*detail_ptr).DevicePath) as *const u16 };
            let flags = FILE_ATTRIBUTE_NORMAL
                | FILE_FLAG_NO_BUFFERING
                | FILE_FLAG_OVERLAPPED
                | FILE_FLAG_WRITE_THROUGH;
            if let Ok(handle) = unsafe {
                CreateFileW(
                    PCWSTR(path_ptr),
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    flags,
                    None,
                )
            } {
                opened = Some(handle);
                break;
            }
        }

        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(dev_info);
        }

        opened.ok_or_else(|| {
            Key2PcError::Display("Failed to open Parsec Virtual Display Adapter device".to_string())
        })
    }

    fn parsec_vdd_ioctl(handle: HANDLE, code: u32, payload: &[u8]) -> Result<u32> {
        if handle.is_invalid() {
            return Err(Key2PcError::Display("Invalid VDD handle".to_string()));
        }
        let mut in_buf = [0u8; 32];
        let copy_len = payload.len().min(in_buf.len());
        in_buf[..copy_len].copy_from_slice(&payload[..copy_len]);

        let event = unsafe { CreateEventW(None, true, false, None) }
            .map_err(|err| Key2PcError::Display(format!("CreateEventW failed: {err}")))?;
        let mut overlapped = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        let mut out_val = 0u32;

        let ioctl_res = unsafe {
            DeviceIoControl(
                handle,
                code,
                Some(in_buf.as_ptr().cast()),
                in_buf.len() as u32,
                Some((&raw mut out_val).cast()),
                std::mem::size_of::<u32>() as u32,
                None,
                Some(&raw mut overlapped),
            )
        };

        if let Err(err) = ioctl_res {
            if err.code() != ERROR_IO_PENDING.into() {
                unsafe {
                    let _ = CloseHandle(event);
                }
                return Err(Key2PcError::Display(format!(
                    "DeviceIoControl failed: {err}"
                )));
            }
        }

        let mut transferred = 0u32;
        let wait_res =
            unsafe { GetOverlappedResultEx(handle, &overlapped, &mut transferred, 5000, false) };
        unsafe {
            let _ = CloseHandle(event);
        }
        wait_res.map_err(|err| Key2PcError::Display(format!("VDD IOCTL wait failed: {err}")))?;
        Ok(out_val)
    }

    fn wait_and_extend_virtual_monitor(
        existing: &[String],
        width: u32,
        height: u32,
        fps: u32,
        timeout: Duration,
    ) -> Option<String> {
        let deadline = Instant::now() + timeout;
        let mut target_dev: Option<String> = None;

        while Instant::now() < deadline {
            let displays = enumerate_gdi_displays();
            if let Some(found) = displays
                .iter()
                .find(|d| d.is_virtual && !existing.contains(&d.device_name))
                .or_else(|| displays.iter().find(|d| d.is_virtual))
            {
                target_dev = Some(found.device_name.clone());
                if found.is_attached {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(120));
        }

        let dev_name = target_dev?;
        let _ = apply_extend_topology_and_mode(&dev_name, width, height, fps);
        Some(dev_name)
    }

    fn find_virtual_gdi_displays() -> Vec<String> {
        enumerate_gdi_displays()
            .into_iter()
            .filter(|d| d.is_virtual)
            .map(|d| d.device_name)
            .collect()
    }

    fn enumerate_gdi_displays() -> Vec<EnumeratedGdiDisplay> {
        let mut out = Vec::new();
        let mut idx = 0u32;

        loop {
            let mut adapter = DISPLAY_DEVICEW {
                cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), idx, &mut adapter, 0) }.as_bool() {
                break;
            }
            idx += 1;

            let adapter_name = utf16_z_to_string(&adapter.DeviceName);
            let adapter_desc = utf16_z_to_string(&adapter.DeviceString);
            let is_primary = (adapter.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE).0 != 0;
            let is_attached = (adapter.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP).0 != 0;

            let mut monitor_id = String::new();
            let mut monitor_desc = String::new();
            let mut mon = DISPLAY_DEVICEW {
                cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            if unsafe {
                EnumDisplayDevicesW(
                    PCWSTR(adapter.DeviceName.as_ptr()),
                    0,
                    &mut mon,
                    EDD_GET_DEVICE_INTERFACE_NAME,
                )
            }
            .as_bool()
            {
                monitor_id = utf16_z_to_string(&mon.DeviceID);
                monitor_desc = utf16_z_to_string(&mon.DeviceString);
            }

            let is_virtual = is_virtual_display_signature(
                &adapter_name,
                &format!("{adapter_desc} {monitor_desc}"),
                &monitor_id,
            );

            out.push(EnumeratedGdiDisplay {
                device_name: adapter_name,
                is_primary,
                is_attached,
                is_virtual,
            });
        }
        out
    }

    fn apply_extend_topology_and_mode(
        virtual_dev_name: &str,
        target_w: u32,
        target_h: u32,
        target_fps: u32,
    ) -> Result<()> {
        let displays = enumerate_gdi_displays();
        let attached_count = displays.iter().filter(|d| d.is_attached).count();
        let target_attached = displays
            .iter()
            .any(|d| d.device_name == virtual_dev_name && d.is_attached);

        if attached_count < 2 || !target_attached {
            let rc = unsafe {
                SetDisplayConfig(
                    None,
                    None,
                    SDC_APPLY | SDC_TOPOLOGY_EXTEND | SDC_ALLOW_CHANGES,
                )
            };
            if rc != 0 {
                return Err(Key2PcError::Display(format!(
                    "SetDisplayConfig extend topology failed with code {rc}"
                )));
            }
            thread::sleep(Duration::from_millis(150));
        }

        let primary_right_edge = displays
            .iter()
            .find(|d| d.is_primary)
            .and_then(|d| read_current_devmode(&d.device_name))
            .map(|dm| unsafe { dm.Anonymous1.Anonymous2.dmPosition.x + dm.dmPelsWidth as i32 })
            .unwrap_or(1920);

        let dev_w: Vec<u16> = virtual_dev_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let all_modes = enumerate_display_modes(&dev_w);
        if all_modes.is_empty() {
            return Ok(());
        }
        let tuples: Vec<(u32, u32, u32)> = all_modes
            .iter()
            .map(|m| (m.dmPelsWidth, m.dmPelsHeight, m.dmDisplayFrequency))
            .collect();
        let Some(mode_idx) =
            select_best_display_mode_index(&tuples, target_w, target_h, target_fps)
        else {
            return Ok(());
        };
        let mut chosen = all_modes[mode_idx];

        let current = read_current_devmode(virtual_dev_name);
        let already_matches = current.is_some_and(|cur| unsafe {
            cur.dmPelsWidth == chosen.dmPelsWidth
                && cur.dmPelsHeight == chosen.dmPelsHeight
                && cur.dmDisplayFrequency == chosen.dmDisplayFrequency
                && cur.Anonymous1.Anonymous2.dmPosition.x != 0
        });
        if already_matches {
            return Ok(());
        }

        chosen.dmFields |= DM_POSITION | DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
        chosen.Anonymous1.Anonymous2.dmPosition = POINTL {
            x: primary_right_edge,
            y: 0,
        };

        let rc = unsafe {
            ChangeDisplaySettingsExW(
                PCWSTR(dev_w.as_ptr()),
                Some(&chosen),
                None,
                CDS_UPDATEREGISTRY | CDS_NORESET,
                None,
            )
        };
        if rc != DISP_CHANGE_SUCCESSFUL {
            return Err(Key2PcError::Display(format!(
                "ChangeDisplaySettingsExW failed with code {}",
                rc.0
            )));
        }
        let commit_rc = unsafe {
            ChangeDisplaySettingsExW(PCWSTR::null(), None, None, CDS_UPDATEREGISTRY, None)
        };
        if commit_rc != DISP_CHANGE_SUCCESSFUL {
            let _ = unsafe {
                ChangeDisplaySettingsExW(PCWSTR::null(), None, None, Default::default(), None)
            };
        }
        Ok(())
    }

    fn read_current_devmode(device_name: &str) -> Option<DEVMODEW> {
        let dev_w: Vec<u16> = device_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut dm = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        if unsafe { EnumDisplaySettingsW(PCWSTR(dev_w.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm) }
            .as_bool()
        {
            Some(dm)
        } else {
            None
        }
    }

    fn enumerate_display_modes(dev_w: &[u16]) -> Vec<DEVMODEW> {
        let mut modes = Vec::new();
        let mut idx = 0u32;
        loop {
            let mut dm = DEVMODEW {
                dmSize: std::mem::size_of::<DEVMODEW>() as u16,
                ..Default::default()
            };
            if !unsafe {
                EnumDisplaySettingsW(
                    PCWSTR(dev_w.as_ptr()),
                    ENUM_DISPLAY_SETTINGS_MODE(idx),
                    &mut dm,
                )
            }
            .as_bool()
            {
                break;
            }
            modes.push(dm);
            idx += 1;
        }
        modes
    }
}

#[cfg(target_os = "windows")]
pub use win::{
    inspect_virtual_driver_status, request_elevated_vdd_install, run_vdd_installer_cli,
    VirtualMonitorGuard,
};
