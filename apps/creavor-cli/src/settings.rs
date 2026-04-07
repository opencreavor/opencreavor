/// Runtime type for settings resolution.
pub enum Runtime {
    Claude,
    OpenCode,
    OpenClaw,
}

/// Read Claude Code's settings to discover the user's configured apiBaseUrl.
/// Returns None if the file doesn't exist or has no apiBaseUrl.
fn read_claude_api_base_url() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let settings_path = std::path::Path::new(&home).join(".claude/settings.json");

    if !settings_path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&settings_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    json.get("apiBaseUrl")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Resolve the original upstream base URL for a given runtime.
///
/// Resolution order:
///   1. Runtime-specific settings file (e.g. ~/.claude/settings.json)
///   2. Runtime-specific env var
///   3. Hardcoded default
pub fn resolve_original_base_url(runtime: Runtime) -> String {
    match runtime {
        Runtime::Claude => {
            read_claude_api_base_url()
                .or_else(|| std::env::var("ANTHROPIC_BASE_URL").ok())
                .unwrap_or_else(|| "https://api.anthropic.com".to_string())
        }
        Runtime::OpenCode | Runtime::OpenClaw => {
            std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string())
        }
    }
}
