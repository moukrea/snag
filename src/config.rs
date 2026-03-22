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
    pub show_adopted: bool,
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
            show_adopted: false,
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
