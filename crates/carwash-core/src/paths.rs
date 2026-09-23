//! Locations of carwash's own files.
//!
//! XDG layout on every Unix, including macOS (`~/.config/carwash`, `~/.cache/carwash`...),
//! which is what developers expect from CLI tools. `CARWASH_HOME` puts everything under one
//! directory, which tests and portable installs use.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Dirs {
    pub config: PathBuf,
    pub cache: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
}

impl Dirs {
    pub fn discover() -> Option<Self> {
        if let Some(home) = std::env::var_os("CARWASH_HOME").filter(|v| !v.is_empty()) {
            let home = PathBuf::from(home);
            return Some(Self {
                config: home.join("config"),
                cache: home.join("cache"),
                data: home.join("data"),
                state: home.join("state"),
            });
        }
        use etcetera::BaseStrategy;
        let base = etcetera::choose_base_strategy().ok()?;
        let app = "carwash";
        Some(Self {
            config: base.config_dir().join(app),
            cache: base.cache_dir().join(app),
            data: base.data_dir().join(app),
            state: base
                .state_dir()
                .unwrap_or_else(|| base.data_dir())
                .join(app),
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    pub fn rules_file(&self) -> PathBuf {
        self.config.join("ecosystems.toml")
    }

    pub fn size_cache_file(&self) -> PathBuf {
        self.cache.join("sizes.json")
    }

    pub fn history_file(&self) -> PathBuf {
        self.data.join("history.jsonl")
    }

    pub fn log_dir(&self) -> PathBuf {
        self.state.join("logs")
    }
}

/// The user's home directory.
pub fn home() -> Option<PathBuf> {
    etcetera::home_dir().ok()
}

/// Writes `contents` to `path` atomically (temporary file in the same directory, then rename).
pub fn write_atomic(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("path has no parent"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}
