use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub broker: BrokerSettings,
    pub upstream: HashMap<String, String>,
    pub audit: AuditSettings,
    pub rules: RulesSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            broker: BrokerSettings::default(),
            upstream: HashMap::new(),
            audit: AuditSettings::default(),
            rules: RulesSettings::default(),
        }
    }
}

impl Settings {
    /// Returns the path to `~/.opencreavor/settings.json`.
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".opencreavor")
            .join("settings.json")
    }

    /// Load settings from a specific path.
    pub fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let settings: Self = serde_json::from_str(&contents)?;
        Ok(settings)
    }

    /// Load settings from the default path, falling back to defaults on error.
    pub fn load_or_default() -> Self {
        let path = Self::default_path();
        match Self::load(&path) {
            Ok(settings) => settings,
            Err(e) => {
                tracing::warn!(
                    "Failed to load settings from {}: {e}; using defaults",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Persist settings to the default path, creating parent directories as needed.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::default_path();
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

    /// Resolve `env:VAR_NAME` references to their values.
    pub fn resolve_env_ref(value: &str) -> anyhow::Result<String> {
        if let Some(name) = value.strip_prefix("env:") {
            if name.is_empty() {
                anyhow::bail!("invalid env reference: missing variable name");
            }
            return std::env::var(name)
                .map_err(|_| anyhow::anyhow!("missing environment variable: {name}"));
        }
        Ok(value.to_owned())
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
// BrokerSettings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BrokerSettings {
    pub port: u16,
    pub log_level: String,
    pub block_status_code: u16,
    pub block_error_style: String,
    pub stream_passthrough: bool,
    pub upstream_timeout_secs: u64,
    pub idle_stream_timeout_secs: u64,
}

impl Default for BrokerSettings {
    fn default() -> Self {
        Self {
            port: 8765,
            log_level: "info".to_string(),
            block_status_code: 400,
            block_error_style: "auto".to_string(),
            stream_passthrough: true,
            upstream_timeout_secs: 300,
            idle_stream_timeout_secs: 120,
        }
    }
}

// ---------------------------------------------------------------------------
// AuditSettings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditSettings {
    pub event_auth_token: Option<String>,
    pub store_request_payloads: bool,
    pub store_response_payloads: bool,
}

impl Default for AuditSettings {
    fn default() -> Self {
        Self {
            event_auth_token: None,
            store_request_payloads: false,
            store_response_payloads: false,
        }
    }
}

// ---------------------------------------------------------------------------
// RulesSettings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RulesSettings {
    pub llm_analyzer_enabled: bool,
}

impl Default for RulesSettings {
    fn default() -> Self {
        Self {
            llm_analyzer_enabled: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env, fs,
        path::PathBuf,
        sync::{Mutex, OnceLock},
        time::{SystemTime, UNIX_EPOCH},
    };

    fn env_guard() -> &'static Mutex<()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD.get_or_init(|| Mutex::new(()))
    }

    fn poison_resilient_lock() -> std::sync::MutexGuard<'static, ()> {
        match env_guard().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn temp_path(suffix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut path: PathBuf = env::temp_dir();
        path.push(format!("creavor-core-settings-{unique}{suffix}"));
        path
    }

    #[test]
    fn settings_defaults_match_spec() {
        let settings = Settings::default();
        assert_eq!(settings.broker.port, 8765);
        assert_eq!(settings.broker.log_level, "info");
        assert_eq!(settings.broker.block_status_code, 400);
        assert_eq!(settings.broker.block_error_style, "auto");
        assert!(settings.broker.stream_passthrough);
        assert_eq!(settings.broker.upstream_timeout_secs, 300);
        assert_eq!(settings.broker.idle_stream_timeout_secs, 120);
        assert!(settings.upstream.is_empty());
        assert_eq!(settings.audit.event_auth_token, None);
        assert!(!settings.audit.store_request_payloads);
        assert!(!settings.audit.store_response_payloads);
        assert!(!settings.rules.llm_analyzer_enabled);
    }

    #[test]
    fn settings_load_from_json_with_upstream() {
        let path = temp_path(".json");
        fs::write(
            &path,
            r#"{
                "broker": { "port": 9999 },
                "upstream": {
                    "claude": "https://api.anthropic.com",
                    "cursor": "https://api.cursor.com"
                }
            }"#,
        )
        .unwrap();

        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.broker.port, 9999);
        assert_eq!(settings.get_upstream("claude"), Some("https://api.anthropic.com"));
        assert_eq!(settings.get_upstream("cursor"), Some("https://api.cursor.com"));
        assert_eq!(settings.get_upstream("copilot"), None);

        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn settings_load_partial_json_inherits_defaults() {
        let path = temp_path("-partial.json");
        fs::write(&path, r#"{ "broker": { "port": 4321 } }"#).unwrap();

        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.broker.port, 4321);
        assert_eq!(settings.broker.log_level, "info");
        assert!(settings.broker.stream_passthrough);
        assert!(settings.upstream.is_empty());

        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn settings_rejects_unknown_fields() {
        let path = temp_path("-unknown.json");
        fs::write(&path, r#"{ "broker": { "unknown_field": true } }"#).unwrap();

        let err = Settings::load(&path).unwrap_err().to_string();
        assert!(err.contains("unknown field"), "error was: {err}");

        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn settings_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", dir.path());

        let mut settings = Settings::default();
        settings.set_upstream("claude", "https://api.example.com");
        settings.save().unwrap();

        let loaded = Settings::load_or_default();
        assert_eq!(loaded, settings);
        assert_eq!(loaded.get_upstream("claude"), Some("https://api.example.com"));
    }

    #[test]
    fn settings_broker_urls() {
        let settings = Settings::default();
        assert_eq!(settings.broker_base_url(), "http://127.0.0.1:8765");
        assert_eq!(settings.broker_proxy_url("anthropic"), "http://127.0.0.1:8765/v1/anthropic");
        assert_eq!(settings.broker_proxy_url("openai"), "http://127.0.0.1:8765/v1/openai");
        assert_eq!(settings.broker_proxy_url("gemini"), "http://127.0.0.1:8765/v1/gemini");
    }

    #[test]
    fn resolve_env_ref_reads_environment_variable() {
        let _guard = poison_resilient_lock();
        env::set_var("CREAVOR_CORE_TEST_TOKEN", "secret-token");

        let resolved = Settings::resolve_env_ref("env:CREAVOR_CORE_TEST_TOKEN").unwrap();
        assert_eq!(resolved, "secret-token");

        env::remove_var("CREAVOR_CORE_TEST_TOKEN");
    }

    #[test]
    fn resolve_env_ref_rejects_empty_variable_name() {
        let err = Settings::resolve_env_ref("env:").unwrap_err().to_string();
        assert!(err.contains("missing variable name"), "error was: {err}");
    }

    #[test]
    fn resolve_env_ref_rejects_missing_env_var() {
        let err = Settings::resolve_env_ref("env:NONEXISTENT").unwrap_err().to_string();
        assert!(err.contains("missing environment variable"), "error was: {err}");
    }
}
