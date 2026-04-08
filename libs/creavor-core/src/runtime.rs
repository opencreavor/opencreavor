use std::path::PathBuf;

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
                std::env::var("GEMINI_API_BASE").ok()
            }
        }
    }

    /// Permanently write a new API base URL to the runtime's config.
    pub fn write_api_url(&self, url: &str) -> anyhow::Result<()> {
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
    fn runtime_type_name_and_provider_route() {
        assert_eq!(RuntimeType::Claude.name(), "claude");
        assert_eq!(RuntimeType::Claude.provider_route(), "anthropic");
        assert_eq!(RuntimeType::OpenCode.provider_route(), "openai");
        assert_eq!(RuntimeType::Gemini.provider_route(), "gemini");
        assert_eq!(RuntimeType::Codex.provider_route(), "openai");
        assert_eq!(RuntimeType::Cline.provider_route(), "openai");
    }

    #[test]
    fn runtime_type_binary_name() {
        assert_eq!(RuntimeType::Claude.binary_name(), "claude");
        assert_eq!(RuntimeType::Gemini.binary_name(), "gemini");
        assert_eq!(RuntimeType::Codex.binary_name(), "codex");
    }

    #[test]
    fn runtime_type_base_url_env_var() {
        assert_eq!(RuntimeType::Claude.base_url_env_var(), "ANTHROPIC_BASE_URL");
        assert_eq!(RuntimeType::Gemini.base_url_env_var(), "GEMINI_API_BASE");
        assert_eq!(RuntimeType::Codex.base_url_env_var(), "OPENAI_BASE_URL");
    }

    #[test]
    fn is_broker_address_detects_local() {
        assert!(is_broker_address("http://127.0.0.1:8765/v1/anthropic", 8765));
        assert!(is_broker_address("http://localhost:8765/v1/openai", 8765));
        assert!(!is_broker_address("https://api.anthropic.com", 8765));
        assert!(!is_broker_address("http://127.0.0.1:9999", 8765));
    }
}
