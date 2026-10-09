//! User settings and XDG locations.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const APP_DIR: &str = "pangram-desktop";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Save scans to the local history database.
    pub save_history: bool,
    /// Last chosen model selector.
    pub model: Option<String>,
    /// Keep the API key in the Secret Service between sessions.
    pub remember_key: bool,
    /// Price of one Pangram credit, for cost estimates.
    pub usd_per_credit: f64,
    /// Show a system tray icon; closing or minimizing the window hides it there.
    pub minimize_to_tray: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            save_history: true,
            model: None,
            remember_key: true,
            usd_per_credit: crate::cost::DEFAULT_USD_PER_CREDIT,
            minimize_to_tray: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

fn xdg_dir(var: &str, fallback: &[&str]) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            fallback.iter().fold(home, |p, s| p.join(s))
        })
}

impl Paths {
    pub fn from_env() -> Self {
        Self {
            config_dir: xdg_dir("XDG_CONFIG_HOME", &[".config"]).join(APP_DIR),
            data_dir: xdg_dir("XDG_DATA_HOME", &[".local", "share"]).join(APP_DIR),
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    pub fn history_db(&self) -> PathBuf {
        self.data_dir.join("history.sqlite3")
    }
}

impl Settings {
    /// Missing or unreadable settings fall back to defaults.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Atomically replaces the settings file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg/settings.json");
        assert_eq!(Settings::load(&path), Settings::default());
        let s = Settings {
            save_history: false,
            model: Some("pangram-4".into()),
            remember_key: false,
            usd_per_credit: 0.04,
            minimize_to_tray: true,
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
        std::fs::write(&path, r#"{"model":"x","unknown":1}"#).unwrap();
        let partial = Settings::load(&path);
        assert_eq!(partial.model.as_deref(), Some("x"));
        assert!(partial.save_history);
        assert!(!partial.minimize_to_tray);
    }
}
