//! User configuration stored as `config.toml`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Display name shown to the other peer.
    pub name: String,
    /// Permanent token obtained from an invitation code.
    pub token: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot access the config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("the config file is not valid TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("cannot serialize the config: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl Config {
    /// `~/.config/miroir/config.toml` on Linux, `%APPDATA%\Miroir\config.toml` on Windows.
    pub fn default_path() -> Option<PathBuf> {
        let dir = if cfg!(windows) { "Miroir" } else { "miroir" };
        Some(directories::BaseDirs::new()?.config_dir().join(dir).join("config.toml"))
    }

    /// A missing file is not an error: it yields the default config.
    pub fn load_from(path: &Path) -> Result<Config, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Creates the parent directories if needed.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(std::fs::write(path, toml::to_string_pretty(self)?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh, empty directory unique to one test.
    fn temp_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("miroir-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_missing_file_returns_default() {
        let path = temp_dir("missing").join("config.toml");
        assert_eq!(Config::load_from(&path).unwrap(), Config::default());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let path = temp_dir("roundtrip").join("not-created-yet").join("config.toml");
        let config = Config { name: "Alexis".into(), token: "abc123".into() };
        config.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap(), config);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let path = temp_dir("partial").join("config.toml");
        std::fs::write(&path, "name = \"Alexis\"\n").unwrap();
        let config = Config::load_from(&path).unwrap();
        assert_eq!(config.name, "Alexis");
        assert_eq!(config.token, "");
    }

    #[test]
    fn malformed_file_is_error_and_left_untouched() {
        let path = temp_dir("malformed").join("config.toml");
        std::fs::write(&path, "name = [").unwrap();
        assert!(matches!(Config::load_from(&path), Err(ConfigError::Parse(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "name = [");
    }

    #[test]
    fn default_path_matches_spec() {
        let path = Config::default_path().unwrap();
        let expected = if cfg!(windows) { Path::new("Miroir") } else { Path::new("miroir") };
        assert!(path.ends_with(expected.join("config.toml")), "{path:?}");
    }
}
