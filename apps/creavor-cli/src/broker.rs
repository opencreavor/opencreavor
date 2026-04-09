use crate::cli::BrokerSubcmd;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8765;

pub fn broker_addr() -> String {
    std::env::var("CREAVOR_BROKER_URL")
        .unwrap_or_else(|_| format!("http://{DEFAULT_HOST}:{DEFAULT_PORT}"))
}

fn parse_addr() -> std::net::SocketAddr {
    let url = broker_addr();
    let addr_str = url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');
    addr_str
        .parse()
        .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT)))
}

/// Raw TCP health check: send HTTP/1.0 GET /health, parse response.
/// Returns Ok(body) if 200, Err if unreachable.
pub fn health_check() -> anyhow::Result<String> {
    let addr = parse_addr();
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;

    let mut stream = stream;
    let request = format!(
        "GET /health HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes())?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;

    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.trim())
        .unwrap_or("");

    if !response.starts_with("HTTP/1.") || !response.contains("200") {
        anyhow::bail!("broker returned non-200 response");
    }

    Ok(body.to_string())
}

/// Print broker status to stdout.
pub fn status() -> anyhow::Result<()> {
    let url = broker_addr();
    match health_check() {
        Ok(body) => {
            println!("broker is healthy at {url}");
            println!("  response: {body}");
            Ok(())
        }
        Err(e) => {
            eprintln!("broker not reachable at {url}: {e}");
            std::process::exit(1);
        }
    }
}

/// Dispatch a broker subcommand.
pub fn handle_subcmd(subcmd: BrokerSubcmd) -> anyhow::Result<()> {
    match subcmd {
        BrokerSubcmd::Start { config } => cmd_start(config),
        BrokerSubcmd::Stop => cmd_stop(),
        BrokerSubcmd::Status => cmd_status(),
        BrokerSubcmd::Logs { last, blocked_only } => cmd_logs(last, blocked_only),
        BrokerSubcmd::RulesList => cmd_rules_list(),
    }
}

fn cmd_start(config: Option<String>) -> anyhow::Result<()> {
    let config_path = config.unwrap_or_else(|| {
        crate::settings::Settings::default_path().to_string_lossy().to_string()
    });

    println!("Starting creavor-broker with config: {config_path}");

    // Check if broker is already running
    if health_check().is_ok() {
        eprintln!("broker is already running at {}", broker_addr());
        std::process::exit(1);
    }

    // Spawn the broker binary
    let broker_bin = find_broker_binary()?;
    let mut cmd = std::process::Command::new(&broker_bin);
    cmd.env("CREAVOR_CONFIG", &config_path);

    let mut child = cmd.spawn().map_err(|e| {
        anyhow::anyhow!("failed to start broker binary '{}': {e}", broker_bin)
    })?;

    // Wait briefly and check health
    std::thread::sleep(Duration::from_millis(500));
    if health_check().is_ok() {
        println!("broker started successfully at {}", broker_addr());
    } else {
        // Check if process is still running
        match child.try_wait()? {
            Some(status) => {
                anyhow::bail!("broker exited prematurely with status: {status}");
            }
            None => {
                println!("broker process started (PID {:?}), waiting for health check...", child.id());
            }
        }
    }

    Ok(())
}

fn cmd_stop() -> anyhow::Result<()> {
    if health_check().is_err() {
        println!("broker is not running");
        return Ok(());
    }

    // Try to find and kill the broker process
    let output = std::process::Command::new("pkill")
        .arg("-f")
        .arg("creavor-broker")
        .output();

    match output {
        Ok(o) if o.status.success() => {
            println!("broker stopped");
            Ok(())
        }
        _ => {
            eprintln!("could not stop broker — try manually: pkill -f creavor-broker");
            std::process::exit(1);
        }
    }
}

fn cmd_status() -> anyhow::Result<()> {
    let url = broker_addr();
    match health_check() {
        Ok(body) => {
            println!("broker is running at {url}");
            println!("  response: {body}");
        }
        Err(_) => {
            println!("broker is NOT running at {url}");
        }
    }
    Ok(())
}

fn cmd_logs(_last: Option<String>, _blocked_only: bool) -> anyhow::Result<()> {
    let settings = crate::settings::Settings::load_or_default();
    let db_path = settings.audit.db_path.clone().unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{home}/.local/share/creavor/broker.db")
    });

    if !std::path::Path::new(&db_path).exists() {
        println!("No audit database found at {db_path}");
        return Ok(());
    }

    // Query SQLite for recent logs
    let output = std::process::Command::new("sqlite3")
        .arg(&db_path)
        .arg("SELECT id, method, path, status_code, upstream_id, created_at FROM requests ORDER BY created_at DESC LIMIT 20;")
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if stdout.trim().is_empty() {
                println!("No audit logs found.");
            } else {
                println!("Recent requests:\n{stdout}");
            }
        }
        Ok(o) => {
            eprintln!("Failed to query logs: {}", String::from_utf8_lossy(&o.stderr));
        }
        Err(_) => {
            eprintln!("sqlite3 not found — install it to query audit logs, or read the database directly at {db_path}");
        }
    }

    Ok(())
}

fn cmd_rules_list() -> anyhow::Result<()> {
    // List built-in rules
    println!("Built-in rules:");
    println!("  secrets       Detect API keys, tokens, and secret patterns");
    println!("  pii           Detect personally identifiable information");
    println!("  enterprise    Detect enterprise-sensitive patterns");
    println!();

    // Check for custom rules directory
    let settings = crate::settings::Settings::load_or_default();
    if let Some(ref rules_dir) = settings.rules.rules_dir {
        if std::path::Path::new(rules_dir).exists() {
            println!("Custom rules directory: {rules_dir}");
            let entries = std::fs::read_dir(rules_dir)?;
            let mut found = false;
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".yml") || name.ends_with(".yaml") {
                        println!("  - {name}");
                        found = true;
                    }
                }
            }
            if !found {
                println!("  (no custom rule files found)");
            }
        } else {
            println!("Custom rules directory not found: {rules_dir}");
        }
    } else {
        println!("No custom rules directory configured.");
    }

    Ok(())
}

fn find_broker_binary() -> anyhow::Result<String> {
    // Try PATH first
    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in path_var.split(':') {
        let candidate = std::path::Path::new(dir).join("creavor-broker");
        if candidate.is_file() {
            return Ok(candidate.to_string_lossy().to_string());
        }
    }

    // Try relative to current binary
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("creavor-broker");
            if candidate.is_file() {
                return Ok(candidate.to_string_lossy().to_string());
            }
        }
    }

    anyhow::bail!("could not find 'creavor-broker' binary. Is it installed?")
}
