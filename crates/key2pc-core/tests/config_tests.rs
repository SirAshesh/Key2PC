use key2pc_core::config::{AppConfig, ConfigStore, JsonFileConfigStore, Role};

#[test]
fn parses_role_aliases_and_validates_defaults() {
    let json = r#"{"role":"pc"}"#;
    let cfg: AppConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.role, Role::Receiver);
    assert!(cfg.validate().is_ok());

    let json_laptop = r#"{"role":"laptop"}"#;
    let cfg_laptop: AppConfig = serde_json::from_str(json_laptop).unwrap();
    assert_eq!(cfg_laptop.role, Role::Sender);
}

#[test]
fn rejects_invalid_port_and_empty_secret() {
    let mut cfg = AppConfig::default();
    cfg.network.port = 0;
    assert!(cfg.validate().is_err());

    let mut cfg2 = AppConfig::default();
    cfg2.network.shared_secret = "   ".to_string();
    assert!(cfg2.validate().is_err());

    let mut cfg3 = AppConfig::default();
    cfg3.network.redundant_sends = 0;
    assert!(cfg3.validate().is_err());
}

#[test]
fn json_file_store_creates_saves_and_reloads_config() {
    let temp_dir = std::env::temp_dir().join(format!("key2pc_cfg_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);
    let path = temp_dir.join("config.json");

    let store = JsonFileConfigStore::new(&path);
    let created = store.load_or_create_default().unwrap();
    assert_eq!(created, AppConfig::default());

    let mut updated = created;
    updated.role = Role::Receiver;
    updated.network.port = 24900;
    store.save(&updated).unwrap();

    let reloaded = store.load().unwrap();
    assert_eq!(reloaded.role, Role::Receiver);
    assert_eq!(reloaded.network.port, 24900);

    let _ = std::fs::remove_dir_all(&temp_dir);
}
