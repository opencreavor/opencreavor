use std::process::Command;

use crate::broker;

const BASE_URL_ENV: &str = "OPENAI_BASE_URL";
const BROKER_PROXY_URL: &str = "http://127.0.0.1:8765/v1/openai";
const BINARY_NAME: &str = "opencode";

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
    tracing::info!(
        proxy_url = %BROKER_PROXY_URL,
        "launching {BINARY_NAME} with creavor proxy"
    );

    let status = Command::new(&binary)
        .env(BASE_URL_ENV, BROKER_PROXY_URL)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to launch {BINARY_NAME}: {e}"))?;

    std::process::exit(status.code().unwrap_or(1));
}

pub fn config() -> anyhow::Result<()> {
    let runtime = crate::settings::RuntimeType::OpenCode;
    let creavor_settings = crate::settings::CreavorSettings::load();
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
