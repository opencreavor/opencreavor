use std::process::Command;

use crate::{broker, session, settings};

const BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";
const CUSTOM_HEADERS_ENV: &str = "ANTHROPIC_CUSTOM_HEADERS";
const BINARY_NAME: &str = "claude";
const RUNTIME_NAME: &str = "claude";

pub fn run() -> anyhow::Result<()> {
    let broker_healthy = broker::health_check().is_ok();

    if !broker_healthy {
        tracing::warn!("broker not detected — launching {BINARY_NAME} without proxy");
        return launch_direct();
    }

    launch_with_proxy()
}

fn launch_direct() -> anyhow::Result<()> {
    let binary = find_binary(BINARY_NAME)?;
    tracing::info!("launching {BINARY_NAME} (direct, no proxy)");
    let status = Command::new(&binary)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to launch {BINARY_NAME}: {e}"))?;
    std::process::exit(status.code().unwrap_or(1));
}

fn launch_with_proxy() -> anyhow::Result<()> {
    let binary = find_binary(BINARY_NAME)?;
    let runtime = settings::RuntimeType::Claude;
    let creavor_settings = settings::CreavorSettings::load();

    let original_url = runtime
        .read_current_api_url()
        .unwrap_or_else(|| "https://api.anthropic.com".to_string());
    let broker_proxy_url = creavor_settings.broker_proxy_url(runtime.provider_route());
    let session_id = session::generate_session_id(RUNTIME_NAME);

    tracing::info!(
        original_url = %original_url,
        proxy_url = %broker_proxy_url,
        session_id = %session_id,
        "launching {BINARY_NAME} with creavor proxy"
    );

    let custom_header = format!("X-Creavor-Session-Id:{session_id}");

    let status = Command::new(&binary)
        .env(BASE_URL_ENV, &broker_proxy_url)
        .env(CUSTOM_HEADERS_ENV, &custom_header)
        .env("CREAVOR_SESSION_ID", &session_id)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to launch {BINARY_NAME}: {e}"))?;

    std::process::exit(status.code().unwrap_or(1));
}

pub fn config() -> anyhow::Result<()> {
    let runtime = settings::RuntimeType::Claude;
    let creavor_settings = settings::CreavorSettings::load();
    let broker_proxy_url = creavor_settings.broker_proxy_url(runtime.provider_route());
    runtime.write_api_url(&broker_proxy_url)?;
    tracing::info!("configured {BINARY_NAME} to use creavor broker at {broker_proxy_url}");
    Ok(())
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
