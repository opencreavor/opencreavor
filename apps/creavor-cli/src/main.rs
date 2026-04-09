mod broker;
mod cleanup;
mod cli;
mod doctor;
mod runtimes;
mod session;
mod settings;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = cli::parse(args)?;

    match command {
        cli::Command::Run { runtime } => runtimes::run(runtime),
        cli::Command::Config { runtime } => runtimes::config(runtime),
        cli::Command::Status => broker::status(),
        cli::Command::Doctor => doctor::run(),
        cli::Command::Cleanup { runtime } => cleanup::run(runtime),
        cli::Command::Broker { subcmd } => broker::handle_subcmd(subcmd),
    }
}
