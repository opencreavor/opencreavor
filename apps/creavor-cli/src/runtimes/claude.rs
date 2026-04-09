use std::process::Command;

use crate::{broker, session, settings::{RuntimeType, Settings, is_broker_address}};
use creavor_core::UpstreamEntry;

pub fn run() -> anyhow::Result<()> {
    let mut settings = Settings::load_or_default();
    let original_url = RuntimeType::Claude.read_current_api_url();

    // Save the original upstream URL into settings and resolve an upstream_id.
    let upstream_id = if let Some(ref url) = original_url {
        if !is_broker_address(url, settings.broker.port) {
            settings.set_upstream("claude-code", url);
            settings.save()?;
        }
        // Try to find an existing registry entry matching the URL.
        settings
            .upstream_registry
            .find_by_url(url)
            .map(|(id, _entry)| id.to_string())
            // If not in registry, register a new entry under a stable id.
            .unwrap_or_else(|| {
                let id = derive_upstream_id(url);
                if settings.upstream_registry.get(&id).is_none() {
                    settings.upstream_registry.insert(
                        id.clone(),
                        UpstreamEntry {
                            protocol: "anthropic".to_string(),
                            upstream: url.clone(),
                        },
                    );
                    let _ = settings.save();
                }
                id
            })
    } else {
        // No original URL configured — fall back to first upstream in the map.
        settings
            .upstream
            .iter()
            .next()
            .map(|(k, _)| k.clone())
            .unwrap_or_default()
    };

    let session_id = session::generate_session_id("claude");
    let broker_healthy = broker::health_check().is_ok();

    let binary = find_binary("claude")?;

    if !broker_healthy {
        tracing::warn!("broker not detected — launching claude without proxy");
        let status = Command::new(&binary).status()?;
        std::process::exit(status.code().unwrap_or(1));
    }

    let proxy_url = settings.broker_proxy_url("anthropic");

    // Build custom headers per the unified header protocol (v1.3.2 §Unified Header):
    //   X-Creavor-Runtime:    identifies the runtime (required)
    //   X-Creavor-Upstream:   identifies the broker upstream-id (strongly recommended)
    //   X-Creavor-Session-Id: session correlation (recommended)
    let mut header_parts = vec![
        "X-Creavor-Runtime:claude-code".to_string(),
        format!("X-Creavor-Session-Id:{session_id}"),
    ];
    if !upstream_id.is_empty() {
        header_parts.push(format!("X-Creavor-Upstream:{upstream_id}"));
    }
    let custom_header = header_parts.join(",");

    // Use --settings to inject a temporary JSON override for ANTHROPIC_BASE_URL.
    // Claude Code's --settings flag has higher priority than settings.json,
    // so this avoids modifying the user's settings file entirely.
    let settings_json = format!(
        r#"{{"env":{{"ANTHROPIC_BASE_URL":"{}"}}}}"#,
        proxy_url
    );

    tracing::info!(
        proxy_url = %proxy_url,
        session_id = %session_id,
        upstream_id = %upstream_id,
        "launching claude with proxy"
    );

    let status = Command::new(&binary)
        .arg("--settings")
        .arg(&settings_json)
        .env("ANTHROPIC_BASE_URL", &proxy_url)
        .env("ANTHROPIC_CUSTOM_HEADERS", &custom_header)
        .env("CREAVOR_SESSION_ID", &session_id)
        .status()?;

    std::process::exit(status.code().unwrap_or(1))
}

pub fn config() -> anyhow::Result<()> {
    let mut settings = Settings::load_or_default();
    let proxy_url = settings.broker_proxy_url("anthropic");
    let original_url = RuntimeType::Claude.read_current_api_url();

    if let Some(ref url) = original_url {
        if is_broker_address(url, settings.broker.port) {
            println!("claude is already configured to use broker");
            return Ok(());
        }
        settings.set_upstream("claude-code", url);
        settings.save()?;
    }

    // Print the --settings override for the user to use directly.
    let settings_json = format!(
        r#"{{"env":{{"ANTHROPIC_BASE_URL":"{}"}}}}"#,
        proxy_url
    );
    println!("Run claude with broker:");
    println!("  claude --settings '{}'", settings_json);
    println!();
    println!("Or add to ~/.claude/settings.json env section:");
    println!("  \"ANTHROPIC_BASE_URL\": \"{}\"", proxy_url);
    Ok(())
}

/// Derive a stable upstream-id from a URL.
/// e.g. "https://open.bigmodel.cn/api/anthropic" → "zhipu-anthropic"
///      "https://api.anthropic.com"               → "anthropic-direct"
fn derive_upstream_id(url: &str) -> String {
    let normalized = url
        .trim()
 .trim_end_matches('/')
        .trim_start_matches("https://")
        .trim_start_matches("http://");

    // Recognise known hosts for friendly names.
    if normalized.starts_with("open.bigmodel.cn") {
        return "zhipu-anthropic".to_string();
    }
    if normalized.starts_with("api.anthropic.com") {
        return "anthropic-direct".to_string();
    }

    // Generic: use the hostname, replacing dots and special chars with dashes.
    let host = normalized.split('/').next().unwrap_or(normalized);
    let id = host
        .replace('.', "-")
        .replace('_', "-")
        .to_lowercase();
    format!("{id}-anthropic")
}

fn find_binary(name: &str) -> anyhow::Result<String> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in path_var.split(':') {
        let candidate = std::path::Path::new(dir).join(name);
        if candidate.is_file() {
            return Ok(candidate.to_string_lossy().to_string());
        }
    }
    anyhow::bail!("could not find '{name}' on PATH. Is {name} installed?")
}
