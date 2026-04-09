use crate::settings::RuntimeType;
use std::path::PathBuf;

/// Clean up creavor changes for all or a specific runtime.
///
/// Restores runtime config files and removes .creavor-changes.* backup files.
pub fn run(runtime: Option<RuntimeType>) -> anyhow::Result<()> {
    match runtime {
        Some(rt) => cleanup_runtime(rt),
        None => cleanup_all(),
    }
}

fn cleanup_all() -> anyhow::Result<()> {
    println!("Cleaning up all runtime configs...\n");

    let runtimes = [
        RuntimeType::Claude,
        RuntimeType::OpenCode,
        RuntimeType::OpenClaw,
        RuntimeType::Codex,
        RuntimeType::Cline,
        RuntimeType::Gemini,
        RuntimeType::Qwen,
    ];

    let mut cleaned = 0;
    for rt in &runtimes {
        if cleanup_runtime_files(*rt)? {
            cleaned += 1;
        }
    }

    if cleaned == 0 {
        println!("No residual creavor changes found.");
    } else {
        println!("\nCleaned up {} runtime(s).", cleaned);
    }
    Ok(())
}

fn cleanup_runtime(runtime: RuntimeType) -> anyhow::Result<()> {
    println!("Cleaning up {}...", runtime.name());
    if cleanup_runtime_files(runtime)? {
        println!("  Restored config and removed backup files.");
    } else {
        println!("  No residual creavor changes found.");
    }
    Ok(())
}

/// Try to clean up .creavor-changes.* files for a runtime.
/// Returns true if any files were found and cleaned.
fn cleanup_runtime_files(runtime: RuntimeType) -> anyhow::Result<bool> {
    let config_dir = runtime_config_dir(runtime);
    let Some(dir) = config_dir else {
        return Ok(false);
    };

    if !dir.exists() {
        return Ok(false);
    }

    let changes_files = find_creavor_changes_files(&dir);
    if changes_files.is_empty() {
        return Ok(false);
    }

    for file in &changes_files {
        println!("  Removing {}", file.display());
        std::fs::remove_file(file)?;
    }

    Ok(true)
}

/// Get the config directory for a runtime.
fn runtime_config_dir(runtime: RuntimeType) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    match runtime {
        RuntimeType::Claude => Some(PathBuf::from(home).join(".claude")),
        RuntimeType::OpenCode => Some(PathBuf::from(home).join(".config").join("opencode")),
        RuntimeType::Codex => Some(PathBuf::from(home).join(".codex")),
        RuntimeType::Qwen => Some(PathBuf::from(home).join(".qwen")),
        _ => None,
    }
}

/// Find all .creavor-changes.* files in a directory.
fn find_creavor_changes_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .contains(".creavor-changes.")
        })
        .map(|e| e.path())
        .collect()
}
