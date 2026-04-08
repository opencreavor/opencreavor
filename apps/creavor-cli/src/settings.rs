use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// CreavorSettings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct CreavorSettings {
    pub broker: BrokerConfig,
    pub upstream: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct BrokerConfig {
    pub port: u16,
    pub log_level: String,
}

impl Default for CreavorSettings {
    fn default() -> Self {
        Self {
            broker: BrokerConfig::default(),
            upstream: HashMap::new(),
        }
    }
}

impl Default for BrokerConfig {
    fn default() -> Self {
        Self {
            port: 8765,
            log_level: "info".to_string(),
        }
    }
}

impl CreavorSettings {
    /// Returns the path to `~/.opencreavor/settings.json`.
    pub fn path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".opencreavor/settings.json")
    }

    /// Load settings from disk. Returns defaults if the file is missing or
    /// cannot be parsed.
    pub fn load() -> Self {
        let path = Self::path();
        if !path.exists() {
            return Self::default();
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };
        serde_json::from_str(&content).unwrap_or_default()
    }

    /// Persist settings to disk, creating parent directories as needed.
    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Register (or overwrite) an upstream URL for a given runtime name.
    pub fn set_upstream(&mut self, runtime: &str, url: &str) {
        self.upstream.insert(runtime.to_string(), url.to_string());
    }

    /// Look up the upstream URL for a given runtime name.
    pub fn get_upstream(&self, runtime: &str) -> Option<&str> {
        self.upstream.get(runtime).map(|s| s.as_str())
    }

    /// Base URL of the local broker, e.g. `http://127.0.0.1:8765`.
    pub fn broker_base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.broker.port)
    }

    /// Full proxy URL for a given provider route, e.g.
    /// `http://127.0.0.1:8765/v1/anthropic`.
    pub fn broker_proxy_url(&self, route: &str) -> String {
        format!("{}/v1/{}", self.broker_base_url(), route)
    }
}

// ---------------------------------------------------------------------------
// RuntimeType
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeType {
    Claude,
    OpenCode,
    OpenClaw,
    Codex,
    Cline,
    Gemini,
}

impl RuntimeType {
    /// Canonical lowercase name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::OpenCode => "opencode",
            Self::OpenClaw => "openclaw",
            Self::Codex => "codex",
            Self::Cline => "cline",
            Self::Gemini => "gemini",
        }
    }

    /// Provider route: "anthropic" for Claude, "gemini" for Gemini, "openai"
    /// for the rest.
    pub fn provider_route(&self) -> &'static str {
        match self {
            Self::Claude => "anthropic",
            Self::Gemini => "gemini",
            _ => "openai",
        }
    }

    /// Environment variable used to override the base URL.
    pub fn base_url_env_var(&self) -> &'static str {
        match self {
            Self::Claude => "ANTHROPIC_BASE_URL",
            Self::Gemini => "GEMINI_API_BASE",
            _ => "OPENAI_BASE_URL",
        }
    }

    /// Binary name to launch.
    pub fn binary_name(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::OpenCode => "opencode",
            Self::OpenClaw => "openclaw",
            Self::Codex => "codex",
            Self::Cline => "cline",
            Self::Gemini => "gemini",
        }
    }

    /// Read the current API base URL from the runtime's own config.
    pub fn read_current_api_url(&self) -> Option<String> {
        match self {
            Self::Claude => {
                let home = std::env::var("HOME").ok()?;
                let settings_path = PathBuf::from(home).join(".claude/settings.json");
                if !settings_path.exists() {
                    return None;
                }
                let content = std::fs::read_to_string(&settings_path).ok()?;
                let json: serde_json::Value = serde_json::from_str(&content).ok()?;
                json.get("apiBaseUrl")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            }
            Self::OpenCode | Self::OpenClaw | Self::Codex | Self::Cline => {
                std::env::var("OPENAI_BASE_URL").ok()
            }
            Self::Gemini => {
                // Check env first, then config
                std::env::var("GEMINI_API_BASE").ok()
            }
        }
    }

    /// Permanently write a new API base URL to the runtime's config.
    pub fn write_api_url(&self, url: &str) -> Result<()> {
        match self {
            Self::Claude => {
                let home = std::env::var("HOME")?;
                let settings_path = PathBuf::from(home).join(".claude/settings.json");
                let mut settings: serde_json::Value = if settings_path.exists() {
                    let content = std::fs::read_to_string(&settings_path)?;
                    serde_json::from_str(&content)?
                } else {
                    serde_json::json!({})
                };
                settings["apiBaseUrl"] = serde_json::Value::String(url.to_string());
                if let Some(parent) = settings_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&settings_path, serde_json::to_string_pretty(&settings)?)?;
                Ok(())
            }
            _ => {
                // For other runtimes, print instructions (env-based config)
                println!(
                    "Set {}={url} to configure {}",
                    self.base_url_env_var(),
                    self.name()
                );
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Check if a URL points to the local broker.
pub fn is_broker_address(url: &str, broker_port: u16) -> bool {
    url.contains(&format!("127.0.0.1:{broker_port}"))
        || url.contains(&format!("localhost:{broker_port}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creavor_settings_defaults() {
        let settings = CreavorSettings::default();
        assert_eq!(settings.broker.port, 8765);
        assert_eq!(settings.broker.log_level, "info");
        assert!(settings.upstream.is_empty());
    }

    #[test]
    fn creavor_settings_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let _path = dir.path().join(".opencreavor/settings.json");

        // Temporarily override HOME so CreavorSettings::path() points at the
        // temp dir.
        std::env::set_var("HOME", dir.path());

        let mut settings = CreavorSettings::default();
        settings.set_upstream("claude", "https://api.example.com");
        settings.save().unwrap();

        let loaded = CreavorSettings::load();
        assert_eq!(loaded, settings);
        assert_eq!(loaded.get_upstream("claude"), Some("https://api.example.com"));
    }

    #[test]
    fn runtime_type_name_and_provider_route() {
        assert_eq!(RuntimeType::Claude.name(), "claude");
        assert_eq!(RuntimeType::Claude.provider_route(), "anthropic");
        assert_eq!(RuntimeType::OpenCode.provider_route(), "openai");
        assert_eq!(RuntimeType::Gemini.provider_route(), "gemini");
        assert_eq!(RuntimeType::Codex.provider_route(), "openai");
        assert_eq!(RuntimeType::Cline.provider_route(), "openai");
    }

    #[test]
    fn is_broker_address_detects_local() {
        assert!(is_broker_address("http://127.0.0.1:8765/v1/anthropic", 8765));
        assert!(is_broker_address("http://localhost:8765/v1/openai", 8765));
        assert!(!is_broker_address("https://api.anthropic.com", 8765));
        assert!(!is_broker_address("http://127.0.0.1:9999", 8765));
    }
}
