use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::clock::Clock;

#[derive(Clone)]
pub struct Paths {
    pub client_secret: PathBuf,
    pub token_cache: PathBuf,
    pub database: PathBuf,
    pub settings: PathBuf,
}

pub fn resolve() -> Result<Paths> {
    let config_dir = dirs::config_dir()
        .context("could not determine a config directory")?
        .join("lazycal");
    let data_dir = dirs::data_dir()
        .context("could not determine a data directory")?
        .join("lazycal");
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;

    Ok(Paths {
        client_secret: config_dir.join("client_secret.json"),
        token_cache: data_dir.join("tokencache.json"),
        database: data_dir.join("cache.db"),
        settings: config_dir.join("settings.json"),
    })
}

/// Preferences that outlive the cache.
///
/// Kept beside the credentials rather than in `cache.db`, which the README
/// says is safe to delete: losing this file would silently drop the user back
/// to read-only without explaining why.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Whether the user has asked for write access. Opt-in: the default
    /// install requests a read-only scope and cannot change a calendar.
    pub write: bool,
    /// Whether times read as `14:30` or `2:30pm`.
    pub clock: Clock,
}

impl Settings {
    /// Reads the settings, falling back to the defaults.
    ///
    /// A missing file is the normal first-run case. An unreadable or corrupt
    /// one is reported and then ignored, because refusing to start over a
    /// preferences file would be worse than starting read-only.
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                tracing::warn!("ignoring unreadable {}: {err}", path.display());
                return Self::default();
            }
        };
        match serde_json::from_str(&text) {
            Ok(settings) => settings,
            Err(err) => {
                tracing::warn!("ignoring unparseable {}: {err}", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).context("encoding settings")?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
    }

    pub fn access(&self) -> crate::google::Access {
        if self.write {
            crate::google::Access::ReadWrite
        } else {
            crate::google::Access::ReadOnly
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_settings_file_is_read_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = Settings::load(&dir.path().join("settings.json"));
        assert!(!settings.write, "write is opt-in");
        assert_eq!(settings.clock, Clock::H24, "the display does not change");
        assert_eq!(settings.access(), crate::google::Access::ReadOnly);
    }

    #[test]
    fn one_preference_can_be_changed_without_naming_the_other() {
        // Every field is `#[serde(default)]`, so a file written by a version
        // that had never heard of the clock still reads back.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"write": true}"#).expect("write");
        let loaded = Settings::load(&path);
        assert!(loaded.write);
        assert_eq!(loaded.clock, Clock::H24);

        std::fs::write(&path, r#"{"clock": "h12"}"#).expect("write");
        let loaded = Settings::load(&path);
        assert!(!loaded.write);
        assert_eq!(loaded.clock, Clock::H12);
    }

    #[test]
    fn settings_survive_a_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A directory that doesn't exist yet, as on a first run.
        let path = dir.path().join("nested").join("settings.json");
        Settings {
            write: true,
            clock: Clock::H12,
        }
        .save(&path)
        .expect("save");

        let loaded = Settings::load(&path);
        assert!(loaded.write);
        assert_eq!(loaded.clock, Clock::H12);
        assert_eq!(loaded.access(), crate::google::Access::ReadWrite);
    }

    #[test]
    fn a_corrupt_settings_file_falls_back_to_read_only() {
        // Starting read-only is recoverable; refusing to start is not.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ not json").expect("write");
        assert!(!Settings::load(&path).write);
    }

    #[test]
    fn an_unknown_key_does_not_discard_the_rest() {
        // Forward compatibility: a newer version's key must not reset write.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"write": true, "from_the_future": 1}"#).expect("write");
        assert!(Settings::load(&path).write);
    }
}
