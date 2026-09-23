//! User configuration: `~/.config/carwash/config.toml`.

use anyhow::{Context, Result};
use carwash_core::clean::DeleteMode;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub scan: ScanConfig,
    pub clean: CleanConfig,
    pub updates: UpdatesConfig,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdatesConfig {
    /// Reuse registry lookups for this many hours.
    pub cache_hours: u64,
    /// Also look up known vulnerabilities (OSV).
    pub vulnerabilities: bool,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            cache_hours: 6,
            vulnerabilities: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScanConfig {
    /// Extra paths never scanned. `~` expands to the home directory.
    pub exclude: Vec<String>,
    /// Also skip well-known non-project locations under the home directory.
    pub default_excludes: bool,
    /// I/O threads; `0` picks a value from the CPU count.
    pub threads: usize,
    /// Enter hidden directories everywhere, not only inside projects and repositories.
    pub include_hidden: bool,
    pub same_filesystem: bool,
    pub max_depth: Option<usize>,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            exclude: Vec::new(),
            default_excludes: true,
            threads: 0,
            include_hidden: false,
            same_filesystem: true,
            max_depth: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CleanConfig {
    pub mode: DeleteMode,
    /// Artifacts modified within this many days are not selected by default.
    pub recent_days: u64,
}

impl Default for CleanConfig {
    fn default() -> Self {
        Self {
            mode: DeleteMode::Permanent,
            recent_days: 7,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    pub theme: String,
    /// `unicode`, `nerd` (Nerd Font glyphs) or `ascii`.
    pub icons: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "gestalt".into(),
            icons: "unicode".into(),
        }
    }
}

/// Home-relative locations that hold no projects but plenty of look-alikes.
const HOME_EXCLUDES: &[&str] = &[
    "Library",
    "Applications",
    "Pictures",
    "Music",
    "Movies",
    "go/pkg/mod",
    "snap",
];

impl Config {
    /// Loads `path`; a missing file yields defaults, a malformed one is an error.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("invalid configuration in {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    /// Absolute exclusion paths: configured ones plus, if enabled, the home defaults.
    pub fn excludes(&self) -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut out: Vec<PathBuf> = self
            .scan
            .exclude
            .iter()
            .map(|raw| expand_home(raw, home.as_deref()))
            .collect();
        if self.scan.default_excludes
            && let Some(home) = &home
        {
            out.extend(HOME_EXCLUDES.iter().map(|rel| home.join(rel)));
        }
        out
    }
}

pub fn expand_home(raw: &str, home: Option<&Path>) -> PathBuf {
    match (raw.strip_prefix("~/").or((raw == "~").then_some("")), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_partial_files() {
        let config: Config = toml::from_str("[clean]\nrecent_days = 3\n").unwrap();
        assert_eq!(config.clean.recent_days, 3);
        assert_eq!(config.clean.mode, DeleteMode::Permanent);
        assert!(config.scan.same_filesystem);
    }

    #[test]
    fn unknown_keys_are_errors() {
        assert!(toml::from_str::<Config>("[scan]\nexclud = []\n").is_err());
    }

    #[test]
    fn home_expansion() {
        let home = Path::new("/home/me");
        assert_eq!(expand_home("~/x", Some(home)), PathBuf::from("/home/me/x"));
        assert_eq!(expand_home("~", Some(home)), PathBuf::from("/home/me"));
        assert_eq!(expand_home("/abs", Some(home)), PathBuf::from("/abs"));
    }
}
