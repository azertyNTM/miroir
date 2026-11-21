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
    /// Linux: lets the desktop reuse the last shared screen or window without asking again.
    pub capture_restore_token: String,
}

/// Manual impl so the token never ends up in a log file.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("name", &self.name)
            .field("token", &"<redacted>")
            .field("capture_restore_token", &self.capture_restore_token)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot access the config file: {0}")]
    Io(#[from] std::io::Error),
    /// Line number and message only: the `toml` error itself quotes the file in its Display
    /// and Debug output, which would put the token in the logs.
    #[error("the config file is not valid TOML: {0}")]
    Parse(String),
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
            Ok(text) => toml::from_str(&text).map_err(|e| {
                let before = e.span().map_or(0, |span| span.start.min(text.len()));
                let line = text.as_bytes()[..before].iter().filter(|b| **b == b'\n').count() + 1;
                ConfigError::Parse(format!("line {line}: {}", e.message()))
            }),
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

        // Written aside then renamed over the target: a crash in the middle of a save must not
        // leave a truncated file, which would load as an empty config and lose the token.
        let tmp = path.with_extension("toml.tmp");
        let _ = std::fs::remove_file(&tmp); // left over from an interrupted save
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        // The file holds the token: owner-only on Unix. On Windows, %APPDATA% is already
        // restricted to the user by its ACL.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp)?;
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        Ok(std::fs::rename(&tmp, path)?)
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
        let config = Config {
            name: "Alexis".into(),
            token: "abc123".into(),
            capture_restore_token: "restore-me".into(),
        };
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
    fn unreadable_file_is_an_error() {
        // A directory where the file should be: reading fails with something else than NotFound.
        let path = temp_dir("unreadable");
        assert!(matches!(Config::load_from(&path), Err(ConfigError::Io(_))));
    }

    #[test]
    fn parse_error_does_not_leak_the_file_content() {
        let path = temp_dir("leak").join("config.toml");
        std::fs::write(&path, "name = \"Alexis\"\ntoken = \"SECRET-abc123\n").unwrap();
        let error = Config::load_from(&path).unwrap_err();
        let printed = format!("{error} {error:?}");
        assert!(!printed.contains("SECRET"), "{printed}");
        assert!(printed.contains("line 2"), "{printed}");
    }

    /// A reader that opened the old file keeps seeing the old content: the new one is written
    /// aside and renamed over it, so a crash in the middle of a save cannot leave an empty config.
    #[cfg(unix)]
    #[test]
    fn save_never_truncates_the_existing_file() {
        use std::io::Read;
        let path = temp_dir("atomic").join("config.toml");
        let old = Config { name: "Alexis".into(), token: "abc123".into(), ..Default::default() };
        old.save_to(&path).unwrap();
        let mut reader = std::fs::File::open(&path).unwrap();

        Config::default().save_to(&path).unwrap();

        let mut text = String::new();
        reader.read_to_string(&mut text).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), old);
        assert_eq!(Config::load_from(&path).unwrap(), Config::default());
        let leftovers = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "temporary file left behind");
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
        let config = Config { name: "Alexis".into(), token: "abc123".into(), ..Default::default() };
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
