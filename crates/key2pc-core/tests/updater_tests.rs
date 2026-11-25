use std::path::Path;

use key2pc_core::updater::{
    build_update_powershell_script, parse_latest_release_update, validate_downloaded_payload,
    SemanticVersion, UpdateInstallMethod,
};

#[test]
fn parses_and_compares_semantic_versions() {
    assert_eq!(
        SemanticVersion::parse("v0.4.0"),
        Some(SemanticVersion {
            major: 0,
            minor: 4,
            patch: 0
        })
    );
    assert_eq!(
        SemanticVersion::parse("V1.2.3"),
        Some(SemanticVersion {
            major: 1,
            minor: 2,
            patch: 3
        })
    );
    assert!(SemanticVersion::parse("v0.4.1").unwrap() > SemanticVersion::parse("0.4.0").unwrap());
    assert!(SemanticVersion::parse("v0.3.9").unwrap() < SemanticVersion::parse("0.4.0").unwrap());
    assert_eq!(SemanticVersion::parse("v0.4"), None);
    assert_eq!(SemanticVersion::parse("v0.4.0-rc1"), None);
}

#[test]
fn parses_github_release_assets_for_msi_and_portable() {
    let release_json = r#"{
        "tag_name": "v0.5.0",
        "draft": false,
        "prerelease": false,
        "assets": [
            {
                "name": "key2pc.exe",
                "browser_download_url": "https://github.com/SirAshesh/Key2PC/releases/download/v0.5.0/key2pc.exe",
                "size": 2500000
            },
            {
                "name": "key2pc-windows-x86_64.msi",
                "browser_download_url": "https://github.com/SirAshesh/Key2PC/releases/download/v0.5.0/key2pc-windows-x86_64.msi",
                "size": 3100000
            }
        ]
    }"#;

    let msi_choice = parse_latest_release_update(release_json, "0.4.0", true)
        .unwrap()
        .unwrap();
    assert_eq!(msi_choice.method, UpdateInstallMethod::Msi);
    assert_eq!(msi_choice.asset_name, "key2pc-windows-x86_64.msi");
    assert_eq!(msi_choice.expected_size, 3_100_000);

    let exe_choice = parse_latest_release_update(release_json, "0.4.0", false)
        .unwrap()
        .unwrap();
    assert_eq!(exe_choice.method, UpdateInstallMethod::PortableExe);
    assert_eq!(exe_choice.asset_name, "key2pc.exe");

    assert!(parse_latest_release_update(release_json, "0.5.0", true)
        .unwrap()
        .is_none());
    assert!(parse_latest_release_update(release_json, "0.6.0", false)
        .unwrap()
        .is_none());

    let draft_json = release_json.replace(r#""draft": false"#, r#""draft": true"#);
    assert!(parse_latest_release_update(&draft_json, "0.4.0", true)
        .unwrap()
        .is_none());

    let prerelease_json = release_json.replace(r#""prerelease": false"#, r#""prerelease": true"#);
    assert!(
        parse_latest_release_update(&prerelease_json, "0.4.0", true)
            .unwrap()
            .is_none()
    );

    let untrusted_json = release_json.replace("https://github.com/", "https://evil.example.com/");
    assert!(parse_latest_release_update(&untrusted_json, "0.4.0", true).is_err());
}

#[test]
fn validates_downloaded_payload_magic_and_size() {
    assert!(
        validate_downloaded_payload(b"MZ\x90\x00", 200_000, 200_000, UpdateInstallMethod::PortableExe)
            .is_ok()
    );
    assert!(
        validate_downloaded_payload(b"NOPE", 200_000, 200_000, UpdateInstallMethod::PortableExe)
            .is_err()
    );
    assert!(
        validate_downloaded_payload(b"MZ\x90\x00", 50_000, 50_000, UpdateInstallMethod::PortableExe)
            .is_err()
    );
    assert!(
        validate_downloaded_payload(b"MZ\x90\x00", 200_000, 210_000, UpdateInstallMethod::PortableExe)
            .is_err()
    );

    let ole_magic = [0xD0u8, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    assert!(
        validate_downloaded_payload(&ole_magic, 300_000, 300_000, UpdateInstallMethod::Msi).is_ok()
    );
    assert!(
        validate_downloaded_payload(b"MZ\x90\x00", 300_000, 300_000, UpdateInstallMethod::Msi)
            .is_err()
    );
}

#[test]
fn generates_safe_powershell_update_scripts_for_msi_and_portable() {
    let msi_script = build_update_powershell_script(
        4242,
        Path::new(r"C:\Users\O'Connor\AppData\Local\Programs\Key2PC\key2pc.exe"),
        Path::new(r"C:\Temp\key2pc-windows-x86_64.msi"),
        UpdateInstallMethod::Msi,
        &["--config".to_string(), r"C:\My Config\c.json".to_string()],
        Path::new(r"C:\Temp\work"),
    );
    assert!(msi_script.contains("O''Connor"));
    assert!(msi_script.contains("msiexec.exe"));
    assert!(msi_script.contains("/qn"));
    assert!(msi_script.contains("HKCU:\\Software\\Key2PC"));
    assert!(msi_script.contains("'--config', 'C:\\My Config\\c.json'"));

    let exe_script = build_update_powershell_script(
        5150,
        Path::new(r"D:\Tools\Key2PC\key2pc.exe"),
        Path::new(r"C:\Temp\key2pc.exe"),
        UpdateInstallMethod::PortableExe,
        &[],
        Path::new(r"C:\Temp\work"),
    );
    assert!(exe_script.contains("$backupExe = \"$currentExe.bak\""));
    assert!(exe_script.contains("Copy-Item -LiteralPath $downloaded -Destination $currentExe"));
    assert!(!exe_script.contains("msiexec.exe"));
}
