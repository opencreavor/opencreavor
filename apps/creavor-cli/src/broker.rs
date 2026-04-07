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
