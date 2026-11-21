//! User configuration stored as `config.toml`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Display name shown to the other peer.
    pub name: String,
    /// Permanent token obtained from an invitation code.
    pub token: String,
}

/// Manual impl so the token never ends up in a log file.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("name", &self.name)
            .field("token", &"<redacted>")
            .finish()
    }
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
        let text = toml::to_string_pretty(self)?;

        // The file holds the token: owner-only on Unix. On Windows, %APPDATA% is already
        // restricted to the user by its ACL.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(path)?;
        // `mode` only applies to a file created by this call: tighten an existing one too.
        #[cfg(unix)]
        file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        Ok(std::io::Write::write_all(&mut file, text.as_bytes())?)
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

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn new_file_is_readable_by_owner_only() {
        let path = temp_dir("private-new").join("config.toml");
        Config::default().save_to(&path).unwrap();
        assert_eq!(mode(&path), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn existing_world_readable_file_is_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_dir("private-existing").join("config.toml");
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        Config::default().save_to(&path).unwrap();
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn debug_output_hides_token() {
        let config = Config { name: "Alexis".into(), token: "abc123".into() };
        let debug = format!("{config:?}");
        assert!(debug.contains("Alexis") && !debug.contains("abc123"), "{debug}");
    }

    #[test]
    fn default_path_matches_spec() {
        let path = Config::default_path().unwrap();
        let expected = if cfg!(windows) { Path::new("Miroir") } else { Path::new("miroir") };
        assert!(path.ends_with(expected.join("config.toml")), "{path:?}");
    }
}
