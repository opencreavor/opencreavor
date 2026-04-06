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
            stream_passthrough: false,
            upstream_timeout: Duration::from_secs(30),
            idle_stream_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditConfig {
    pub event_auth_token: Option<String>,
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
        assert!(!config.broker.stream_passthrough);
        assert_eq!(config.broker.upstream_timeout, Duration::from_secs(30));
        assert_eq!(config.broker.idle_stream_timeout, Duration::from_secs(30));
        assert_eq!(config.audit.event_auth_token, None);
        assert!(!config.rules.llm.analyzer.enabled);
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
}
