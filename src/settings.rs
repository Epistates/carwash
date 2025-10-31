use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_CACHE_TTL_MINUTES: u64 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub background_updates_enabled: bool,
    pub cache_ttl_minutes: u64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            background_updates_enabled: false,
            cache_ttl_minutes: DEFAULT_CACHE_TTL_MINUTES,
        }
    }
}

impl AppSettings {
    pub fn load() -> Self {
        let path = settings_path();
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Ok(settings) = serde_json::from_str::<AppSettings>(&contents) {
                return settings.normalize();
            }
        }
        AppSettings::default()
    }

    pub fn save(&self) -> Result<()> {
        let normalized = self.clone().normalize();
        if let Some(parent) = settings_path().parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create settings directory {}", parent.display())
            })?;
        }
        let json =
            serde_json::to_string_pretty(&normalized).context("Failed to serialize settings")?;
        fs::write(settings_path(), json)
            .with_context(|| "Failed to write settings file".to_string())?;
        Ok(())
    }

    pub fn cache_duration(&self) -> Duration {
        Duration::from_secs(self.cache_ttl_minutes.max(1) * 60)
    }

    fn normalize(mut self) -> Self {
        if self.cache_ttl_minutes == 0 {
            self.cache_ttl_minutes = DEFAULT_CACHE_TTL_MINUTES;
        }
        self
    }
}

fn settings_path() -> PathBuf {
    if let Some(dirs) = ProjectDirs::from("com", "epistates", "carwash") {
        dirs.config_dir().join("settings.json")
    } else {
        PathBuf::from("./carwash-settings.json")
    }
}
