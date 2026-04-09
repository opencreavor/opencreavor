use crate::upstream::UpstreamRegistry;
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
    /// Legacy runtime→URL mapping (used as priority #4 fallback).
    pub upstream: HashMap<String, String>,
    /// New upstream registry (used as primary upstream resolution).
    pub upstream_registry: UpstreamRegistry,
    pub audit: AuditSettings,
    pub rules: RulesSettings,
    pub guard: GuardSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            broker: BrokerSettings::default(),
            upstream: HashMap::new(),
            upstream_registry: UpstreamRegistry::default(),
            audit: AuditSettings::default(),
            rules: RulesSettings::default(),
            guard: GuardSettings::default(),
        }
    }
}

impl Settings {
    /// New default path: `~/.config/creavor/config.toml`
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".config")
            .join("creavor")
            .join("config.toml")
    }

    /// Legacy path: `~/.opencreavor/settings.json`
    pub fn legacy_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".opencreavor")
            .join("settings.json")
    }

    /// Load settings from a specific file path, auto-detecting format by extension.
    pub fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path)?;
        let settings = if path.extension().map(|e| e == "toml").unwrap_or(false) {
            toml::from_str(&contents)?
        } else {
            serde_json::from_str(&contents)?
        };
        Ok(settings)
    }

    /// Load settings from the default path, with auto-migration from legacy JSON.
    ///
    /// Resolution order:
    /// 1. `~/.config/creavor/config.toml` (new format)
    /// 2. `~/.opencreavor/settings.json` (legacy, auto-migrated)
    /// 3. Built-in defaults
    pub fn load_or_default() -> Self {
        let new_path = Self::default_path();
        let legacy_path = Self::legacy_path();

        // Try new TOML path first
        if new_path.exists() {
            match Self::load(&new_path) {
                Ok(settings) => return settings,
                Err(e) => {
                    tracing::warn!("Failed to load settings from {}: {e}", new_path.display());
                }
            }
        }

        // Try legacy JSON path
        if legacy_path.exists() {
            match Self::load(&legacy_path) {
                Ok(settings) => {
                    tracing::info!("Loaded settings from legacy path {}, consider migrating to {}",
                        legacy_path.display(), new_path.display());
                    return settings;
                }
                Err(e) => {
                    tracing::warn!("Failed to load settings from {}: {e}", legacy_path.display());
                }
            }
        }

        tracing::info!("No settings file found, using defaults");
        Self::default()
    }

    /// Persist settings to the default path in TOML format.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::default_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Migrate settings from legacy JSON to new TOML path.
    /// Returns true if migration was performed.
    pub fn migrate_from_legacy() -> anyhow::Result<bool> {
        let legacy = Self::legacy_path();
        let new_path = Self::default_path();

        if !legacy.exists() || new_path.exists() {
            return Ok(false);
        }

        let settings = Self::load(&legacy)?;
        settings.save()?;
        tracing::info!("Migrated settings from {} to {}", legacy.display(), new_path.display());
        Ok(true)
    }

    /// Register (or overwrite) an upstream URL for a given runtime name.
    pub fn set_upstream(&mut self, runtime: &str, url: &str) {
        self.upstream.insert(runtime.to_string(), url.to_string());
    }

    /// Look up the upstream URL for a given runtime name.
    pub fn get_upstream(&self, runtime: &str) -> Option<&str> {
        self.upstream.get(runtime).map(|s| s.as_str())
    }

    /// Return the first configured upstream URL, useful as a default fallback.
    pub fn first_upstream(&self) -> Option<&str> {
        self.upstream.values().next().map(|s| s.as_str())
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
    pub db_path: Option<String>,
    pub retention_days: u32,
    pub store_request_payloads: bool,
    pub store_response_payloads: bool,
}

impl Default for AuditSettings {
    fn default() -> Self {
        Self {
            event_auth_token: None,
            db_path: None,
            retention_days: 90,
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
    pub rules_dir: Option<String>,
    pub builtin_secrets: bool,
    pub builtin_pii: bool,
    pub builtin_key_patterns: bool,
}

impl Default for RulesSettings {
    fn default() -> Self {
        Self {
            llm_analyzer_enabled: false,
            rules_dir: None,
            builtin_secrets: true,
            builtin_pii: true,
            builtin_key_patterns: true,
        }
    }
}

// ---------------------------------------------------------------------------
// GuardSettings
// ---------------------------------------------------------------------------

/// Configuration for Creavor Guard (interactive approval component).
///
/// Defined in the design document:
/// ```toml
/// [guard]
/// approval_timeout_secs = 60
/// default_timeout_action = "block"
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GuardSettings {
    /// Timeout in seconds for pending approval requests.
    pub approval_timeout_secs: u64,
    /// Default action when an approval times out: "block" or "allow".
    pub default_timeout_action: String,
}

impl Default for GuardSettings {
    fn default() -> Self {
        Self {
            approval_timeout_secs: 60,
            default_timeout_action: "block".to_string(),
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
        assert!(settings.upstream_registry.is_empty());
        assert_eq!(settings.audit.event_auth_token, None);
        assert!(!settings.audit.store_request_payloads);
        assert!(!settings.audit.store_response_payloads);
        assert!(!settings.rules.llm_analyzer_enabled);
        assert!(settings.rules.builtin_secrets);
        assert!(settings.rules.builtin_pii);
        assert!(settings.rules.builtin_key_patterns);
        assert_eq!(settings.guard.approval_timeout_secs, 60);
        assert_eq!(settings.guard.default_timeout_action, "block");
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
        assert_eq!(settings.guard.approval_timeout_secs, 60);

        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn settings_load_with_upstream_registry() {
        let path = temp_path("-registry.json");
        fs::write(
            &path,
            r#"{
                "upstream_registry": {
                    "zhipu-anthropic": { "protocol": "anthropic", "upstream": "https://open.bigmodel.cn/api/anthropic" },
                    "openai-direct": { "protocol": "openai", "upstream": "https://api.openai.com/v1" }
                }
            }"#,
        )
        .unwrap();

        let settings = Settings::load(&path).unwrap();
        let entry = settings.upstream_registry.get("zhipu-anthropic").unwrap();
        assert_eq!(entry.protocol, "anthropic");
        assert_eq!(entry.upstream, "https://open.bigmodel.cn/api/anthropic");

        let entry2 = settings.upstream_registry.get("openai-direct").unwrap();
        assert_eq!(entry2.protocol, "openai");

        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn settings_load_with_guard_section() {
        let path = temp_path("-guard.json");
        fs::write(
            &path,
            r#"{
                "guard": { "approval_timeout_secs": 120, "default_timeout_action": "allow" }
            }"#,
        )
        .unwrap();

        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.guard.approval_timeout_secs, 120);
        assert_eq!(settings.guard.default_timeout_action, "allow");

        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn settings_load_with_audit_extensions() {
        let path = temp_path("-audit.json");
        fs::write(
            &path,
            r#"{
                "audit": {
                    "db_path": "~/.local/share/creavor/broker.db",
                    "retention_days": 30
                }
            }"#,
        )
        .unwrap();

        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.audit.db_path, Some("~/.local/share/creavor/broker.db".to_string()));
        assert_eq!(settings.audit.retention_days, 30);

        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn settings_load_from_toml() {
        let path = temp_path(".toml");
        fs::write(
            &path,
            r#"
[broker]
port = 9999

[audit]
retention_days = 30

[guard]
approval_timeout_secs = 120
"#,
        )
        .unwrap();

        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.broker.port, 9999);
        assert_eq!(settings.audit.retention_days, 30);
        assert_eq!(settings.guard.approval_timeout_secs, 120);

        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn settings_save_writes_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", dir.path());

        let mut settings = Settings::default();
        settings.set_upstream("claude", "https://api.anthropic.com");
        settings.save().unwrap();

        let path = Settings::default_path();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("port = 8765"), "TOML should contain broker config");
        assert!(content.contains("api.anthropic.com"), "TOML should contain upstream URL");
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
    fn settings_save_and_load_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", dir.path());

        let mut settings = Settings::default();
        settings.set_upstream("claude", "https://api.example.com");

        // Save to an explicit temp path to avoid HOME timing issues
        let save_path = dir.path().join("config.toml");
        let content = toml::to_string_pretty(&settings).unwrap();
        fs::write(&save_path, &content).unwrap();

        let loaded = Settings::load(&save_path).unwrap();
        assert_eq!(loaded.get_upstream("claude"), Some("https://api.example.com"));
        assert_eq!(loaded.broker.port, 8765);
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

    #[test]
    fn guard_settings_defaults() {
        let gs = GuardSettings::default();
        assert_eq!(gs.approval_timeout_secs, 60);
        assert_eq!(gs.default_timeout_action, "block");
    }

    #[test]
    fn audit_settings_defaults() {
        let audit = AuditSettings::default();
        assert!(audit.event_auth_token.is_none());
        assert!(audit.db_path.is_none());
        assert_eq!(audit.retention_days, 90);
        assert!(!audit.store_request_payloads);
        assert!(!audit.store_response_payloads);
    }
}
