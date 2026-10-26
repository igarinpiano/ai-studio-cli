//! Where files live and the persisted config file.
//!
//! The data directory is the OS config dir plus `ai-studio-cli`:
//!   Windows: %APPDATA%\ai-studio-cli
//!   macOS:   ~/Library/Application Support/ai-studio-cli
//!   Linux:   $XDG_CONFIG_HOME/ai-studio-cli (default ~/.config/ai-studio-cli)
//! `AISTUDIO_CLI_HOME` overrides it.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::settings::RunSettings;

const APP_DIR: &str = "ai-studio-cli";
pub const API_KEY_ENV_VARS: [&str; 2] = ["GEMINI_API_KEY", "GOOGLE_API_KEY"];

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Default run settings applied to every new chat.
    pub settings: RunSettings,
    /// "markdown" or "raw".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub render: Option<String>,
}

pub fn app_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("AISTUDIO_CLI_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR)
}

pub fn config_path() -> PathBuf {
    app_home().join("config.json")
}

pub fn chats_dir() -> PathBuf {
    app_home().join("chats")
}

pub fn images_dir() -> PathBuf {
    app_home().join("images")
}

pub fn input_history_path() -> PathBuf {
    app_home().join("input_history.txt")
}

impl Config {
    pub fn load() -> Config {
        let path = config_path();
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("warning: ignoring unreadable {}: {e}", path.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<PathBuf> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        {
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            // The file can hold the API key, so keep it private to the user.
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            f.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
        }
        fs::rename(&tmp, &path)?;
        Ok(path)
    }

    /// Returns (key, where it came from). Environment variables win over the config file.
    pub fn resolve_api_key(&self) -> Option<(String, String)> {
        for name in API_KEY_ENV_VARS {
            if let Ok(v) = std::env::var(name) {
                let v = v.trim().to_string();
                if !v.is_empty() {
                    return Some((v, format!("${name}")));
                }
            }
        }
        self.api_key
            .as_ref()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .map(|k| (k, config_path().display().to_string()))
    }
}

pub fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len());
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}
