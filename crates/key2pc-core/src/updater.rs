use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Key2PcError, Result};

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemanticVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SemanticVersion {
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        let without_v = trimmed
            .strip_prefix('v')
            .or_else(|| trimmed.strip_prefix('V'))
            .unwrap_or(trimmed);
        let mut parts = without_v.split('.');
        let major = parts.next()?.parse::<u32>().ok()?;
        let minor = parts.next()?.parse::<u32>().ok()?;
        let patch = parts.next()?.parse::<u32>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for SemanticVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateInstallMethod {
    Msi,
    PortableExe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAssetChoice {
    pub version: SemanticVersion,
    pub tag_name: String,
    pub method: UpdateInstallMethod,
    pub asset_name: String,
    pub download_url: String,
    pub expected_size: u64,
}

#[derive(Debug, Clone)]
pub struct StagedUpdate {
    pub choice: ReleaseAssetChoice,
    pub downloaded_path: PathBuf,
    pub work_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseJson {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAssetJson>,
}

#[derive(Debug, Deserialize)]
struct GithubAssetJson {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

fn is_trusted_github_download_url(url: &str) -> bool {
    url.starts_with("https://github.com/")
        || url.starts_with("https://objects.githubusercontent.com/")
        || url.starts_with("https://release-assets.githubusercontent.com/")
}

pub fn parse_latest_release_update(
    json: &str,
    current_version: &str,
    prefer_msi: bool,
) -> Result<Option<ReleaseAssetChoice>> {
    let release: GithubReleaseJson = serde_json::from_str(json)?;
    if release.draft || release.prerelease {
        return Ok(None);
    }

    let current = SemanticVersion::parse(current_version).ok_or_else(|| {
        Key2PcError::InvalidConfig(format!("Invalid current version: {current_version}"))
    })?;
    let Some(latest) = SemanticVersion::parse(&release.tag_name) else {
        return Ok(None);
    };
    if latest <= current {
        return Ok(None);
    }

    let msi_asset = release
        .assets
        .iter()
        .find(|a| a.name.to_ascii_lowercase().ends_with(".msi"));
    let exe_asset = release
        .assets
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case("key2pc.exe"));

    let (method, asset) = if prefer_msi {
        if let Some(msi) = msi_asset {
            (UpdateInstallMethod::Msi, msi)
        } else if let Some(exe) = exe_asset {
            (UpdateInstallMethod::PortableExe, exe)
        } else {
            return Ok(None);
        }
    } else if let Some(exe) = exe_asset {
        (UpdateInstallMethod::PortableExe, exe)
    } else {
        return Ok(None);
    };

    if !is_trusted_github_download_url(&asset.browser_download_url) {
        return Err(Key2PcError::Protocol(format!(
            "Rejected untrusted release asset URL: {}",
            asset.browser_download_url
        )));
    }

    Ok(Some(ReleaseAssetChoice {
        version: latest,
        tag_name: release.tag_name,
        method,
        asset_name: asset.name.clone(),
        download_url: asset.browser_download_url.clone(),
        expected_size: asset.size,
    }))
}

const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

pub fn validate_downloaded_payload(
    prefix: &[u8],
    actual_size: u64,
    expected_size: u64,
    method: UpdateInstallMethod,
) -> Result<()> {
    if actual_size < 100_000 {
        return Err(Key2PcError::Protocol(format!(
            "Downloaded update payload is too small ({actual_size} bytes)"
        )));
    }
    if expected_size > 0 && actual_size != expected_size {
        return Err(Key2PcError::Protocol(format!(
            "Downloaded update size mismatch: expected {expected_size} bytes, got {actual_size}"
        )));
    }
    match method {
        UpdateInstallMethod::PortableExe => {
            if prefix.len() < 2 || &prefix[..2] != b"MZ" {
                return Err(Key2PcError::Protocol(
                    "Downloaded executable failed PE (MZ) header check".to_string(),
                ));
            }
        }
        UpdateInstallMethod::Msi => {
            if prefix.len() < OLE_MAGIC.len() || prefix[..OLE_MAGIC.len()] != OLE_MAGIC {
                return Err(Key2PcError::Protocol(
                    "Downloaded installer failed MSI (OLE) header check".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn ps_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub fn build_update_powershell_script(
    pid: u32,
    current_exe: &Path,
    downloaded_file: &Path,
    method: UpdateInstallMethod,
    relaunch_args: &[String],
    work_dir: &Path,
) -> String {
    let exe_lit = ps_single_quote(&current_exe.to_string_lossy());
    let dl_lit = ps_single_quote(&downloaded_file.to_string_lossy());
    let work_lit = ps_single_quote(&work_dir.to_string_lossy());

    let args_array = if relaunch_args.is_empty() {
        "@()".to_string()
    } else {
        let items: Vec<String> = relaunch_args.iter().map(|a| ps_single_quote(a)).collect();
        format!("@({})", items.join(", "))
    };

    let launch_block = r#"if ($relaunchArgs.Count -gt 0) {
    Start-Process -FilePath $launchExe -ArgumentList $relaunchArgs -WorkingDirectory $launchDir | Out-Null
} else {
    Start-Process -FilePath $launchExe -WorkingDirectory $launchDir | Out-Null
}"#;

    let apply_block = match method {
        UpdateInstallMethod::Msi => format!(
            r#"$msiArgs = @('/i', $downloaded, '/qn', '/norestart')
$proc = Start-Process -FilePath 'msiexec.exe' -ArgumentList $msiArgs -Wait -PassThru -WindowStyle Hidden
if ($proc.ExitCode -ne 0 -and $proc.ExitCode -ne 3010) {{
    exit $proc.ExitCode
}}
$launchExe = $currentExe
$reg = Get-ItemProperty -Path 'HKCU:\Software\Key2PC' -ErrorAction SilentlyContinue
if ($null -ne $reg -and -not [string]::IsNullOrWhiteSpace($reg.InstallDir)) {{
    $candidate = Join-Path $reg.InstallDir 'key2pc.exe'
    if (Test-Path -LiteralPath $candidate) {{
        $launchExe = $candidate
    }}
}}
$launchDir = Split-Path -Parent $launchExe
{launch_block}"#
        ),
        UpdateInstallMethod::PortableExe => format!(
            r#"$backupExe = "$currentExe.bak"
$replaced = $false
for ($i = 0; $i -lt 25; $i++) {{
    try {{
        if (Test-Path -LiteralPath $backupExe) {{
            Remove-Item -LiteralPath $backupExe -Force -ErrorAction SilentlyContinue
        }}
        if (Test-Path -LiteralPath $currentExe) {{
            Move-Item -LiteralPath $currentExe -Destination $backupExe -Force
        }}
        Copy-Item -LiteralPath $downloaded -Destination $currentExe -Force
        $replaced = $true
        break
    }} catch {{
        if (-not (Test-Path -LiteralPath $currentExe) -and (Test-Path -LiteralPath $backupExe)) {{
            Move-Item -LiteralPath $backupExe -Destination $currentExe -Force -ErrorAction SilentlyContinue
        }}
        Start-Sleep -Milliseconds 300
    }}
}}
if (-not $replaced) {{
    exit 1
}}
$launchExe = $currentExe
$launchDir = Split-Path -Parent $launchExe
{launch_block}
Remove-Item -LiteralPath $backupExe -Force -ErrorAction SilentlyContinue"#
        ),
    };

    format!(
        r#"$ErrorActionPreference = 'Stop'
$targetPid = {pid}
$currentExe = {exe_lit}
$downloaded = {dl_lit}
$workDir = {work_lit}
$relaunchArgs = {args_array}

$deadline = (Get-Date).AddSeconds(20)
while ((Get-Date) -lt $deadline) {{
    $p = Get-Process -Id $targetPid -ErrorAction SilentlyContinue
    if ($null -eq $p) {{ break }}
    Start-Sleep -Milliseconds 200
}}
Start-Sleep -Milliseconds 500

{apply_block}

Remove-Item -LiteralPath $workDir -Recurse -Force -ErrorAction SilentlyContinue
"#
    )
}

#[cfg(target_os = "windows")]
mod win {
    use std::fs::{self, File};
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::Command;
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use windows::core::{w, PCWSTR};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK,
    };

    use crate::config::UpdateConfig;
    use crate::error::{Key2PcError, Result};
    use crate::updater::{
        build_update_powershell_script, parse_latest_release_update, validate_downloaded_payload,
        StagedUpdate, APP_VERSION,
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    enum UpdateSignal {
        CheckManual,
        UpdateConfig(UpdateConfig),
        Shutdown,
    }

    pub struct AutoUpdater {
        signal_tx: Sender<UpdateSignal>,
        handle: Option<JoinHandle<()>>,
    }

    impl AutoUpdater {
        pub fn start(
            initial_config: UpdateConfig,
            auto_enabled: bool,
            pending_update: Arc<Mutex<Option<StagedUpdate>>>,
            wake_fn: Arc<dyn Fn() + Send + Sync>,
        ) -> Self {
            let (signal_tx, signal_rx) = mpsc::channel();
            let handle = thread::Builder::new()
                .name("key2pc-updater".to_string())
                .spawn(move || {
                    run_updater_loop(
                        initial_config,
                        auto_enabled,
                        signal_rx,
                        pending_update,
                        wake_fn,
                    );
                })
                .ok();

            Self { signal_tx, handle }
        }

        pub fn trigger_manual_check(&self) {
            let _ = self.signal_tx.send(UpdateSignal::CheckManual);
        }

        pub fn update_config(&self, config: UpdateConfig) {
            let _ = self.signal_tx.send(UpdateSignal::UpdateConfig(config));
        }

        pub fn shutdown(&mut self) {
            let _ = self.signal_tx.send(UpdateSignal::Shutdown);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    impl Drop for AutoUpdater {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    fn run_updater_loop(
        mut config: UpdateConfig,
        auto_enabled: bool,
        signal_rx: mpsc::Receiver<UpdateSignal>,
        pending_update: Arc<Mutex<Option<StagedUpdate>>>,
        wake_fn: Arc<dyn Fn() + Send + Sync>,
    ) {
        let mut next_timeout = if auto_enabled && config.enabled {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(3600)
        };

        loop {
            match signal_rx.recv_timeout(next_timeout) {
                Ok(UpdateSignal::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(UpdateSignal::UpdateConfig(new_cfg)) => {
                    config = new_cfg;
                    next_timeout = if auto_enabled && config.enabled {
                        Duration::from_secs(config.check_interval_minutes.max(5) * 60)
                    } else {
                        Duration::from_secs(3600)
                    };
                }
                Ok(UpdateSignal::CheckManual) => {
                    match check_and_stage_update(&config, APP_VERSION) {
                        Ok(Some(staged)) => {
                            if let Ok(mut guard) = pending_update.lock() {
                                *guard = Some(staged);
                            }
                            wake_fn();
                            break;
                        }
                        Ok(None) => {
                            show_update_message_box(
                                "Key2PC Updates",
                                &format!("Key2PC v{APP_VERSION} is already the latest version."),
                                false,
                            );
                        }
                        Err(err) => {
                            show_update_message_box(
                                "Key2PC Updates",
                                &format!("Failed to check for updates:\n{err}"),
                                true,
                            );
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if auto_enabled && config.enabled {
                        if let Ok(Some(staged)) = check_and_stage_update(&config, APP_VERSION) {
                            if let Ok(mut guard) = pending_update.lock() {
                                *guard = Some(staged);
                            }
                            wake_fn();
                            break;
                        }
                        next_timeout =
                            Duration::from_secs(config.check_interval_minutes.max(5) * 60);
                    }
                }
            }
        }
    }

    pub fn detect_msi_installation(current_exe: &Path) -> bool {
        let Some(method) = read_hkcu_string(w!("Software\\Key2PC"), w!("InstallMethod")) else {
            return false;
        };
        if !method.eq_ignore_ascii_case("msi") {
            return false;
        }
        let Some(install_dir) = read_hkcu_string(w!("Software\\Key2PC"), w!("InstallDir")) else {
            return false;
        };
        let norm_dir = normalize_dir_for_compare(Path::new(install_dir.trim()));
        let Some(exe_parent) = current_exe.parent() else {
            return false;
        };
        normalize_dir_for_compare(exe_parent) == norm_dir
    }

    fn normalize_dir_for_compare(path: &Path) -> String {
        path.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_ascii_lowercase()
    }

    fn read_hkcu_string(subkey: PCWSTR, value_name: PCWSTR) -> Option<String> {
        let mut size_bytes = 0u32;
        let err = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey,
                value_name,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size_bytes),
            )
        };
        if err.is_err() || size_bytes < 2 {
            return None;
        }
        let mut buf = vec![0u16; (size_bytes as usize).div_ceil(2)];
        let err2 = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey,
                value_name,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size_bytes),
            )
        };
        if err2.is_err() {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    pub fn check_and_stage_update(
        config: &UpdateConfig,
        current_version: &str,
    ) -> Result<Option<StagedUpdate>> {
        let repo = config.repo.trim();
        let api_url = format!("https://api.github.com/repos/{repo}/releases/latest");
        let json = fetch_latest_release_json(&api_url)?;

        let current_exe = std::env::current_exe()?;
        let prefer_msi = detect_msi_installation(&current_exe);
        let Some(choice) = parse_latest_release_update(&json, current_version, prefer_msi)? else {
            return Ok(None);
        };

        let work_dir = std::env::temp_dir().join(format!(
            "key2pc-update-{}-{}",
            choice.version,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&work_dir);
        fs::create_dir_all(&work_dir)?;

        let safe_name = Path::new(&choice.asset_name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("key2pc-update.bin");
        let target_path = work_dir.join(safe_name);

        if let Err(err) = download_release_asset(&choice.download_url, &target_path) {
            let _ = fs::remove_dir_all(&work_dir);
            return Err(err);
        }

        let meta = match fs::metadata(&target_path) {
            Ok(m) => m,
            Err(err) => {
                let _ = fs::remove_dir_all(&work_dir);
                return Err(err.into());
            }
        };
        let mut prefix = [0u8; 16];
        let read_res = File::open(&target_path).and_then(|mut f| f.read(&mut prefix));
        let n = match read_res {
            Ok(n) => n,
            Err(err) => {
                let _ = fs::remove_dir_all(&work_dir);
                return Err(err.into());
            }
        };
        if let Err(err) = validate_downloaded_payload(
            &prefix[..n],
            meta.len(),
            choice.expected_size,
            choice.method,
        ) {
            let _ = fs::remove_dir_all(&work_dir);
            return Err(err);
        }

        Ok(Some(StagedUpdate {
            choice,
            downloaded_path: target_path,
            work_dir,
        }))
    }

    pub fn spawn_staged_update_installer(
        staged: &StagedUpdate,
        relaunch_args: &[String],
    ) -> Result<()> {
        let current_exe = std::env::current_exe()?;
        let script = build_update_powershell_script(
            std::process::id(),
            &current_exe,
            &staged.downloaded_path,
            staged.choice.method,
            relaunch_args,
            &staged.work_dir,
        );
        let script_path = staged.work_dir.join("apply-update.ps1");
        fs::write(&script_path, script)?;

        Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script_path)
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .spawn()?;

        Ok(())
    }

    fn fetch_latest_release_json(url: &str) -> Result<String> {
        if let Ok(output) = Command::new("curl.exe")
            .args([
                "-fsSL",
                "--connect-timeout",
                "10",
                "--max-time",
                "20",
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "User-Agent: Key2PC-Updater",
                url,
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if output.status.success() {
                return String::from_utf8(output.stdout).map_err(|err| {
                    Key2PcError::Protocol(format!("Invalid UTF-8 in release response: {err}"))
                });
            }
        }

        let ps_cmd = format!(
            "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; \
            (Invoke-WebRequest -Uri '{}' -Headers @{{'Accept'='application/vnd.github+json'; 'User-Agent'='Key2PC-Updater'}} -TimeoutSec 20 -UseBasicParsing).Content",
            url.replace('\'', "''")
        );
        let ps_out = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &ps_cmd,
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;
        if !ps_out.status.success() {
            return Err(Key2PcError::Protocol(
                "GitHub release check request failed".to_string(),
            ));
        }
        String::from_utf8(ps_out.stdout)
            .map_err(|err| Key2PcError::Protocol(format!("Invalid UTF-8 from PowerShell: {err}")))
    }

    fn download_release_asset(url: &str, target: &Path) -> Result<()> {
        let target_str = target.to_string_lossy();
        let curl_ok = Command::new("curl.exe")
            .args([
                "-fsSL",
                "--retry",
                "2",
                "--connect-timeout",
                "10",
                "--max-time",
                "180",
                "-H",
                "User-Agent: Key2PC-Updater",
                "-o",
                &target_str,
                url,
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !curl_ok || !target.is_file() {
            let ps_cmd = format!(
                "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; \
                Invoke-WebRequest -Uri '{}' -OutFile '{}' -Headers @{{'User-Agent'='Key2PC-Updater'}} -TimeoutSec 180 -UseBasicParsing",
                url.replace('\'', "''"),
                target_str.replace('\'', "''")
            );
            let ps_status = Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    &ps_cmd,
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .status()?;
            if !ps_status.success() {
                return Err(Key2PcError::Protocol(
                    "Failed to download Key2PC update asset from GitHub".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn show_update_message_box(title: &str, text: &str, is_error: bool) {
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

    pub fn format_staged_update_summary(staged: &StagedUpdate) -> String {
        format!(
            "Updating to v{} via {}",
            staged.choice.version, staged.choice.asset_name
        )
    }
}

#[cfg(target_os = "windows")]
pub use win::{
    check_and_stage_update, detect_msi_installation, format_staged_update_summary,
    spawn_staged_update_installer, AutoUpdater,
};
