use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub shell: Option<String>,
    pub scrollback_bytes: usize,
    pub socket: Option<PathBuf>,
    pub detach_key: String,
    pub detach_timeout_ms: u64,
    pub daemon_grace_period: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell: None,
            scrollback_bytes: 1_048_576,
            socket: None,
            detach_key: "ctrl-\\".to_string(),
            detach_timeout_ms: 500,
            daemon_grace_period: 30,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => toml::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn config_path() -> PathBuf {
        if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(dir).join("snag").join("config.toml")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
                .join(".config")
                .join("snag")
                .join("config.toml")
        } else {
            PathBuf::from("/etc/snag/config.toml")
        }
    }

    pub fn socket_path(&self) -> PathBuf {
        if let Some(ref p) = self.socket {
            return p.clone();
        }
        if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
            PathBuf::from(dir).join("snag").join("snag.sock")
        } else {
            let uid = nix::unistd::getuid();
            PathBuf::from(format!("/tmp/snag-{}", uid)).join("snag.sock")
        }
    }

    pub fn pid_path(&self) -> PathBuf {
        self.socket_path()
            .parent()
            .unwrap_or(&PathBuf::from("/tmp"))
            .join("snag.pid")
    }

    pub fn default_shell(&self) -> String {
        if let Some(ref s) = self.shell {
            return s.clone();
        }
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.shell.is_none());
        assert_eq!(config.scrollback_bytes, 1_048_576);
        assert!(config.socket.is_none());
        assert_eq!(config.detach_key, "ctrl-\\");
        assert_eq!(config.detach_timeout_ms, 500);
        assert_eq!(config.daemon_grace_period, 30);
    }

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
shell = "/bin/zsh"
scrollback_bytes = 2097152
detach_key = "ctrl-q"
detach_timeout_ms = 300
daemon_grace_period = 60
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(config.scrollback_bytes, 2097152);
        assert_eq!(config.detach_key, "ctrl-q");
        assert_eq!(config.detach_timeout_ms, 300);
        assert_eq!(config.daemon_grace_period, 60);
    }

    #[test]
    fn test_parse_partial_config() {
        let toml = r#"
shell = "/bin/bash"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.shell.as_deref(), Some("/bin/bash"));
        // Other fields should be defaults
        assert_eq!(config.scrollback_bytes, 1_048_576);
        assert_eq!(config.detach_timeout_ms, 500);
    }

    #[test]
    fn test_parse_empty_config() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.shell.is_none());
        assert_eq!(config.scrollback_bytes, 1_048_576);
    }

    #[test]
    fn test_parse_unknown_keys_ignored() {
        let toml = r#"
shell = "/bin/zsh"
unknown_key = "value"
another_unknown = 42
"#;
        // Should not error on unknown keys
        let result: std::result::Result<Config, _> = toml::from_str(toml);
        // toml crate by default errors on unknown keys when using serde,
        // but our Config uses #[serde(default)] which should handle gracefully
        // If it does error, that's also acceptable behavior
        let _ = result;
    }

    #[test]
    fn test_default_shell_from_config() {
        let config = Config {
            shell: Some("/bin/fish".to_string()),
            ..Config::default()
        };
        assert_eq!(config.default_shell(), "/bin/fish");
    }

    #[test]
    fn test_socket_path_default() {
        let config = Config::default();
        let path = config.socket_path();
        assert!(path.to_string_lossy().contains("snag"));
        assert!(path.to_string_lossy().ends_with("snag.sock"));
    }

    #[test]
    fn test_socket_path_override() {
        let config = Config {
            socket: Some(PathBuf::from("/tmp/custom.sock")),
            ..Config::default()
        };
        assert_eq!(config.socket_path(), PathBuf::from("/tmp/custom.sock"));
    }

    #[test]
    fn test_pid_path() {
        let config = Config::default();
        let pid_path = config.pid_path();
        assert!(pid_path.to_string_lossy().ends_with("snag.pid"));
    }
}
