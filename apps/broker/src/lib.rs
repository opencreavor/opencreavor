pub mod audit;
pub mod config;
pub mod events;
pub mod interceptor;
pub mod proxy;
pub mod router;
pub mod rule_engine;
pub mod storage;

pub async fn run() -> anyhow::Result<()> {
    let config_path = parse_config_path(std::env::args().skip(1))?;
    let settings = match config_path {
        Some(path) => config::Settings::load(path)?,
        None => config::Settings::load_or_default(),
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&settings.broker.log_level)),
        )
        .init();

    let db_path = std::env::var("CREAVOR_BROKER_DB_PATH")
        .unwrap_or_else(|_| "/tmp/creavor-broker.sqlite".to_string());

    let port = settings.broker.port;

    tracing::info!(
        port = port,
        upstream_count = settings.upstream.len(),
        "starting broker-server"
    );

    let storage = storage::AuditStorage::open(db_path)?;
    let app = router::app(settings, storage);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("broker-server listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn parse_config_path<I>(mut args: I) -> anyhow::Result<Option<String>>
where
    I: Iterator<Item = String>,
{
    let mut config_path = None;
    while let Some(arg) = args.next() {
        if arg == "--config" {
            let value = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing value for --config"))?;
            config_path = Some(value);
            continue;
        }

        anyhow::bail!("unsupported argument: {arg}");
    }

    Ok(config_path)
}

#[cfg(test)]
mod tests {
    use super::parse_config_path;

    #[test]
    fn parse_config_path_reads_value() {
        let args = vec!["--config".to_string(), "a.toml".to_string()];
        let parsed = parse_config_path(args.into_iter()).unwrap();
        assert_eq!(parsed.as_deref(), Some("a.toml"));
    }

    #[test]
    fn parse_config_path_errors_on_missing_value() {
        let args = vec!["--config".to_string()];
        let err = parse_config_path(args.into_iter()).unwrap_err();
        assert!(err.to_string().contains("missing value"));
    }
}
