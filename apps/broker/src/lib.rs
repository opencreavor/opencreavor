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
    let config = match config_path {
        Some(path) => config::Config::load(path)?,
        None => config::Config::default(),
    };

    let db_path = std::env::var("CREAVOR_BROKER_DB_PATH")
        .unwrap_or_else(|_| "/tmp/creavor-broker.sqlite".to_string());
    let upstream_base_url = std::env::var("CREAVOR_UPSTREAM_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

    let storage = storage::AuditStorage::open(db_path)?;
    let events_app = router::app(config.clone(), storage);
    let proxy_app = router::proxy_app(config.clone(), upstream_base_url);
    let app = events_app.merge(proxy_app);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], config.broker.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("creavor-broker listening on http://{addr}");
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
