fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("creavor-guard starting");

    // P0.5: Guard runs as a local service for interactive approval.
    // Full MCP server integration will be added in P0.5-2.
    // For now, start the basic approval state machine and local HTTP endpoint.

    creavor_guard::run()?;

    Ok(())
}
