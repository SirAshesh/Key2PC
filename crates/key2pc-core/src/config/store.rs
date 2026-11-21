use std::fs;
use std::path::{Path, PathBuf};

use crate::config::model::AppConfig;
use crate::error::Result;

pub trait ConfigStore {
    fn load(&self) -> Result<AppConfig>;
    fn save(&self, config: &AppConfig) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct JsonFileConfigStore {
    path: PathBuf,
}

impl JsonFileConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_or_create_default(&self) -> Result<AppConfig> {
        if !self.path.exists() {
            let defaults = AppConfig::default();
            self.save(&defaults)?;
            return Ok(defaults);
        }
        self.load()
    }
}

impl ConfigStore for JsonFileConfigStore {
    fn load(&self) -> Result<AppConfig> {
        let raw = fs::read_to_string(&self.path)?;
        let config: AppConfig = serde_json::from_str(&raw)?;
        config.validate()?;
        Ok(config)
    }

    fn save(&self, config: &AppConfig) -> Result<()> {
        config.validate()?;
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let json = serde_json::to_string_pretty(config)?;
        fs::write(&self.path, format!("{json}\n"))?;
        Ok(())
    }
}

pub fn resolve_config_path(explicit_path: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit_path {
        return path.to_path_buf();
    }

    let cwd_config = PathBuf::from("config.json");
    if cwd_config.exists() {
        return cwd_config;
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let exe_config = exe_dir.join("config.json");
            if exe_config.exists() {
                return exe_config;
            }
            if !Path::new("Cargo.toml").exists() {
                return exe_config;
            }
        }
    }

    cwd_config
}
