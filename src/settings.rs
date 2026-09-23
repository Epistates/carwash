use serde::{Deserialize, Serialize};
use std::time::Duration;

const DEFAULT_CACHE_TTL_MINUTES: u64 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_minutes: u64,
    #[serde(default)]
    pub show_all_folders: bool,
}

fn default_cache_ttl() -> u64 {
    DEFAULT_CACHE_TTL_MINUTES
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            cache_ttl_minutes: DEFAULT_CACHE_TTL_MINUTES,
            show_all_folders: false,
        }
    }
}

impl AppSettings {
    pub fn cache_duration(&self) -> Duration {
        Duration::from_secs(self.cache_ttl_minutes.max(1) * 60)
    }

    pub fn normalize(mut self) -> Self {
        if self.cache_ttl_minutes == 0 {
            self.cache_ttl_minutes = DEFAULT_CACHE_TTL_MINUTES;
        }
        self
    }
}
