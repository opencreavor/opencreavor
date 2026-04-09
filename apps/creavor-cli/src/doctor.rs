use crate::settings::Settings;
use std::net::TcpStream;

/// Run diagnostic checks on the creavor environment.
pub fn run() -> anyhow::Result<()> {
    let mut passed = 0;
    let mut failed = 0;
    let mut warnings = 0;

    println!("creavor doctor\n");

    // 1. Configuration file check
    let settings = Settings::load_or_default();
    let new_path = Settings::default_path();
    let legacy_path = Settings::legacy_path();

    if new_path.exists() {
        println!("[PASS] Config found at {}", new_path.display());
        passed += 1;
    } else if legacy_path.exists() {
        println!("[WARN] Using legacy config at {} (consider migrating to {})",
            legacy_path.display(), new_path.display());
        warnings += 1;
    } else {
        println!("[INFO] No config file found, using defaults");
        passed += 1;
    }

    // 2. Configuration validity
    let port = settings.broker.port;
    if port > 0 && (port as u32) <= 65535 {
        println!("[PASS] Broker port: {}", port);
        passed += 1;
    } else {
        println!("[FAIL] Invalid broker port: {}", port);
        failed += 1;
    }

    // 3. Port availability
    if check_port_available(settings.broker.port) {
        println!("[PASS] Port {} is available", settings.broker.port);
        passed += 1;
    } else {
        // Port might be in use by broker itself, which is fine
        if check_broker_health(settings.broker.port) {
            println!("[PASS] Port {} in use by creavor-broker", settings.broker.port);
            passed += 1;
        } else {
            println!("[WARN] Port {} in use by another process", settings.broker.port);
            warnings += 1;
        }
    }

    // 4. Upstream connectivity
    if settings.upstream.is_empty() && settings.upstream_registry.is_empty() {
        println!("[WARN] No upstreams configured");
        warnings += 1;
    } else {
        for (name, url) in settings.upstream.iter() {
            if check_upstream_connectivity(url) {
                println!("[PASS] Upstream '{}' reachable", name);
                passed += 1;
            } else {
                println!("[FAIL] Upstream '{}' unreachable at {}", name, url);
                failed += 1;
            }
        }
        for (id, entry) in settings.upstream_registry.iter() {
            if check_upstream_connectivity(&entry.upstream) {
                println!("[PASS] Registry upstream '{}' ({}) reachable", id, entry.protocol);
                passed += 1;
            } else {
                println!("[FAIL] Registry upstream '{}' unreachable at {}", id, entry.upstream);
                failed += 1;
            }
        }
    }

    // 5. Database path check
    let db_path = settings.audit.db_path.clone().unwrap_or_else(|| {
        dirs_data_path("broker.db")
    });
    if let Some(parent) = std::path::Path::new(&db_path).parent() {
        if parent.exists() {
            println!("[PASS] Database directory exists: {}", parent.display());
            passed += 1;
        } else {
            println!("[FAIL] Database directory missing: {}", parent.display());
            failed += 1;
        }
    }

    // 6. Rules directory
    if let Some(ref rules_dir) = settings.rules.rules_dir {
        if std::path::Path::new(rules_dir).exists() {
            println!("[PASS] Rules directory exists: {}", rules_dir);
            passed += 1;
        } else {
            println!("[WARN] Rules directory not found: {}", rules_dir);
            warnings += 1;
        }
    }

    // Summary
    println!("\n--- Summary ---");
    println!("Passed: {}, Failed: {}, Warnings: {}", passed, failed, warnings);

    if failed > 0 {
        println!("\nSome checks failed. Please fix the issues above.");
        std::process::exit(1);
    } else {
        println!("\nAll checks passed.");
        Ok(())
    }
}

fn check_port_available(port: u16) -> bool {
    TcpStream::connect(format!("127.0.0.1:{port}")).is_err()
}

fn check_broker_health(port: u16) -> bool {
    use std::io::{Read, Write};
    if let Ok(mut stream) = TcpStream::connect(format!("127.0.0.1:{port}")) {
        let request = format!(
            "GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        );
        if stream.write_all(request.as_bytes()).is_ok() {
            let mut response = Vec::new();
            if stream.read_to_end(&mut response).is_ok() {
                let response_str = String::from_utf8_lossy(&response);
                return response_str.contains("creavor-broker");
            }
        }
    }
    false
}

fn check_upstream_connectivity(url: &str) -> bool {
    // Parse host from URL and try a TCP connection
    let host_port = extract_host_port(url);
    if let Some((host, port)) = host_port {
        TcpStream::connect(format!("{host}:{port}")).is_ok()
    } else {
        false
    }
}

fn extract_host_port(url: &str) -> Option<(String, u16)> {
    let url = url.strip_prefix("https://").unwrap_or(url);
    let url = url.strip_prefix("http://").unwrap_or(url);
    let host_port = url.split('/').next()?;
    if let Some((host, port_str)) = host_port.rsplit_once(':') {
        let port = port_str.parse::<u16>().ok()?;
        Some((host.to_string(), port))
    } else if url.starts_with("https://") {
        Some((host_port.to_string(), 443))
    } else {
        Some((host_port.to_string(), 80))
    }
}

fn dirs_data_path(filename: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    format!("{home}/.local/share/creavor/{filename}")
}
