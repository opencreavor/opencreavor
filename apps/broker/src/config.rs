use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fs, path::Path, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub broker: BrokerConfig,
    pub audit: AuditConfig,
    pub rules: RulesConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            broker: BrokerConfig::default(),
            audit: AuditConfig::default(),
            rules: RulesConfig::default(),
        }
    }
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let contents = fs::read_to_string(path)?;
        let mut config: Self = toml::from_str(&contents)?;
        if let Some(token) = config.audit.event_auth_token.clone() {
            config.audit.event_auth_token = Some(resolve_env_ref(&token)?);
        }
        Ok(config)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BrokerConfig {
    pub block_status_code: u16,
    pub block_error_style: String,
    pub stream_passthrough: bool,
    #[serde(with = "duration_secs")]
    pub upstream_timeout: Duration,
    #[serde(with = "duration_secs")]
    pub idle_stream_timeout: Duration,
}

impl Default for BrokerConfig {
    fn default() -> Self {
        Self {
            block_status_code: 400,
            block_error_style: "auto".to_string(),
            stream_passthrough: true,
            upstream_timeout: Duration::from_secs(300),
            idle_stream_timeout: Duration::from_secs(120),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditConfig {
    #[serde(serialize_with = "redact_event_auth_token")]
    pub event_auth_token: Option<String>,
}

impl std::fmt::Debug for AuditConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("AuditConfig");
        match self.event_auth_token {
            Some(_) => {
                debug.field("event_auth_token", &Some("<redacted>"));
            }
            None => {
                debug.field("event_auth_token", &Option::<&str>::None);
            }
        }
        debug.finish()
    }
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            event_auth_token: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RulesConfig {
    pub llm: RulesLlmConfig,
}

impl Default for RulesConfig {
    fn default() -> Self {
        Self {
            llm: RulesLlmConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RulesLlmConfig {
    pub analyzer: RulesLlmAnalyzerConfig,
}

impl Default for RulesLlmConfig {
    fn default() -> Self {
        Self {
            analyzer: RulesLlmAnalyzerConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RulesLlmAnalyzerConfig {
    pub enabled: bool,
}

impl Default for RulesLlmAnalyzerConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}

fn resolve_env_ref(value: &str) -> anyhow::Result<String> {
    if let Some(name) = value.strip_prefix("env:") {
        if name.is_empty() {
            anyhow::bail!("invalid env reference: missing variable name");
        }

        return std::env::var(name)
            .map_err(|_| anyhow::anyhow!("missing environment variable: {name}"));
    }

    Ok(value.to_owned())
}

fn redact_event_auth_token<S>(value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(_) => serializer.serialize_some("<redacted>"),
        None => serializer.serialize_none(),
    }
}

mod duration_secs {
    use super::*;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Duration::from_secs(u64::deserialize(deserializer)?))
    }
}

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

    #[test]
    fn config_defaults_match_p0_spec() {
        let config = Config::default();

        assert_eq!(config.broker.block_status_code, 400);
        assert_eq!(config.broker.block_error_style, "auto");
        assert!(config.broker.stream_passthrough);
        assert_eq!(config.broker.upstream_timeout, Duration::from_secs(300));
        assert_eq!(config.broker.idle_stream_timeout, Duration::from_secs(120));
        assert_eq!(config.audit.event_auth_token, None);
        assert!(!config.rules.llm.analyzer.enabled);
    }

    #[test]
    fn config_debug_redacts_event_auth_token() {
        let config = Config {
            audit: AuditConfig {
                event_auth_token: Some("super-secret".to_string()),
            },
            ..Config::default()
        };

        let debug = format!("{:?}", config);

        assert!(!debug.contains("super-secret"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn config_serialize_redacts_event_auth_token() {
        let config = Config {
            audit: AuditConfig {
                event_auth_token: Some("super-secret".to_string()),
            },
            ..Config::default()
        };

        let serialized = serde_json::to_string(&config).unwrap();

        assert!(!serialized.contains("super-secret"));
        assert!(serialized.contains("<redacted>"));
    }

    #[test]
    fn resolve_env_ref_reads_environment_variable() {
        let _guard = poison_resilient_lock();
        env::set_var("CREAVOR_BROKER_TEST_TOKEN", "secret-token");

        let resolved = resolve_env_ref("env:CREAVOR_BROKER_TEST_TOKEN").unwrap();

        assert_eq!(resolved, "secret-token");
        env::remove_var("CREAVOR_BROKER_TEST_TOKEN");
    }

    #[test]
    fn config_load_resolves_event_auth_token_env_ref() {
        let _guard = poison_resilient_lock();
        env::set_var("CREAVOR_BROKER_AUDIT_TOKEN", "resolved-from-env");

        let mut path: PathBuf = env::temp_dir();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("creavor-broker-config-{unique}.toml"));
        fs::write(
            &path,
            r#"
[audit]
event_auth_token = "env:CREAVOR_BROKER_AUDIT_TOKEN"
"#,
        )
        .unwrap();

        let config = Config::load(&path).unwrap();

        assert_eq!(
            config.audit.event_auth_token.as_deref(),
            Some("resolved-from-env")
        );
        env::remove_var("CREAVOR_BROKER_AUDIT_TOKEN");
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn config_load_inherits_defaults_for_partial_toml() {
        let mut path: PathBuf = env::temp_dir();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("creavor-broker-config-partial-{unique}.toml"));
        fs::write(&path, "[broker]\nblock_status_code = 403\n").unwrap();

        let config = Config::load(&path).unwrap();

        assert_eq!(config.broker.block_status_code, 403);
        assert_eq!(config.broker.block_error_style, "auto");
        assert!(config.broker.stream_passthrough);
        assert_eq!(config.broker.upstream_timeout, Duration::from_secs(300));
        assert_eq!(config.broker.idle_stream_timeout, Duration::from_secs(120));
        assert_eq!(config.audit.event_auth_token, None);
        assert!(!config.rules.llm.analyzer.enabled);

        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn config_load_rejects_unknown_fields() {
        let mut path: PathBuf = env::temp_dir();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("creavor-broker-config-unknown-{unique}.toml"));
        fs::write(&path, "[broker]\nunknown_field = true\n").unwrap();

        let err = Config::load(&path).unwrap_err().to_string();

        assert!(err.contains("unknown_field"));

        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn config_load_rejects_malformed_env_ref() {
        let mut path: PathBuf = env::temp_dir();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("creavor-broker-config-bad-env-{unique}.toml"));
        fs::write(&path, "[audit]\nevent_auth_token = \"env:\"\n").unwrap();

        let err = Config::load(&path).unwrap_err().to_string();

        assert!(err.contains("missing variable name"));

        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn config_load_rejects_missing_env_ref() {
        let _guard = poison_resilient_lock();
        let mut path: PathBuf = env::temp_dir();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("creavor-broker-config-missing-env-{unique}.toml"));
        fs::write(
            &path,
            "[audit]\nevent_auth_token = \"env:CREAVOR_BROKER_MISSING_TOKEN\"\n",
        )
        .unwrap();

        let err = Config::load(&path).unwrap_err().to_string();

        assert!(err.contains("missing environment variable"));
        assert!(err.contains("CREAVOR_BROKER_MISSING_TOKEN"));

        fs::remove_file(&path).unwrap();
    }
}
