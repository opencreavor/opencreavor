use std::process::Command;

use crate::{broker, session, settings::{RuntimeType, Settings, is_broker_address}};

pub fn run() -> anyhow::Result<()> {
    let mut settings = Settings::load_or_default();
    let original_url = RuntimeType::Gemini.read_current_api_url();

    if let Some(ref url) = original_url {
        if !is_broker_address(url, settings.broker.port) {
            settings.set_upstream("gemini", url);
            settings.save()?;
        }
    }

    let session_id = session::generate_session_id("gemini");
    let broker_healthy = broker::health_check().is_ok();

    let binary = find_binary("gemini")?;

    if !broker_healthy {
        tracing::warn!("broker not detected — launching gemini without proxy");
        let status = Command::new(&binary).status()?;
        std::process::exit(status.code().unwrap_or(1));
    }

    let proxy_url = settings.broker_proxy_url("gemini");

    tracing::info!(proxy_url = %proxy_url, session_id = %session_id, "launching gemini with proxy");

    let status = Command::new(&binary)
        .env("GEMINI_API_BASE", &proxy_url)
        .env("CREAVOR_SESSION_ID", &session_id)
        .status()?;
    std::process::exit(status.code().unwrap_or(1))
}

pub fn config() -> anyhow::Result<()> {
    let mut settings = Settings::load_or_default();
    let proxy_url = settings.broker_proxy_url("gemini");
    let original_url = RuntimeType::Gemini.read_current_api_url();

    if let Some(ref url) = original_url {
        if is_broker_address(url, settings.broker.port) {
            println!("gemini is already configured to use broker");
            return Ok(());
        }
        settings.set_upstream("gemini", url);
        settings.save()?;
    }

    RuntimeType::Gemini.write_api_url(&proxy_url)?;
    println!("gemini configured to use broker at {}", proxy_url);
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
