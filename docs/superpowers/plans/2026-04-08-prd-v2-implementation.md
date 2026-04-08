# OpenCreavor PRD v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor broker-server to read JSON config from `~/.opencreavor/settings.json` with per-runtime upstream routing, wire audit into the proxy flow, and update creavor-cli to support `run` (with upstream auto-registration) and `config` (permanent rewrite) commands for claude/opencode/codex/cline/gemini runtimes.

**Architecture:** Two binaries: `broker-server` (local proxy, reads `~/.opencreavor/settings.json` for config + upstream routing, uses `X-Creavor-Runtime` header to resolve per-runtime upstream URL) and `creavor` (CLI tool, `creavor run <runtime>` injects env vars + registers upstream in settings, `creavor config <runtime>` permanently rewrites runtime config). Broker provides three route prefixes: `/v1/anthropic/*` (Claude), `/v1/openai/*` (OpenCode/OpenClaw/Codex/Cline), `/v1/gemini/*` (Gemini CLI).

**Tech Stack:** Rust, tokio, axum, hyper, rusqlite, serde_json, tracing, cargo test.

---

## File Structure

### broker-server (`apps/broker`)

| File | Change | Responsibility |
|------|--------|---------------|
| `src/config.rs` | Rewrite | JSON config model, `~/.opencreavor/settings.json`, per-runtime upstream URLs |
| `src/storage.rs` | Modify | Schema: add runtime/session/rule fields, separate payload writes |
| `src/audit.rs` | Modify | Updated APIs for new schema fields |
| `src/interceptor.rs` | Modify | Strip `X-Creavor-Runtime` header, fix OpenAI `code` field, add Gemini block response |
| `src/events.rs` | Modify | Accept `Arc<Mutex<AuditStorage>>` for shared storage |
| `src/router.rs` | Rewrite | Single merged router, runtime-aware upstream, audit wired into proxy |
| `src/lib.rs` | Modify | Load JSON config, single app builder |
| `src/proxy.rs` | No change | Streaming proxy unchanged |
| `src/rule_engine.rs` | No change | Rule engine unchanged |
| `Cargo.toml` | Modify | Remove `toml` dep |

### creavor-cli (`apps/creavor-cli`)

| File | Change | Responsibility |
|------|--------|---------------|
| `src/cli.rs` | Modify | Add `config` command, add codex/cline/gemini runtimes |
| `src/settings.rs` | Rewrite | Read/write `~/.opencreavor/settings.json`, read runtime configs |
| `src/broker.rs` | No change | Health check unchanged |
| `src/session.rs` | No change | Session ID generation unchanged |
| `src/runtimes/mod.rs` | Modify | Add codex/cline/gemini dispatch |
| `src/runtimes/claude.rs` | Rewrite | Run with upstream registration + env vars, config command |
| `src/runtimes/opencode.rs` | Rewrite | Same pattern as claude |
| `src/runtimes/openclaw.rs` | Rewrite | Same pattern as claude |
| `src/runtimes/codex.rs` | Create | New runtime |
| `src/runtimes/cline.rs` | Create | New runtime |
| `src/runtimes/gemini.rs` | Create | New runtime (Gemini CLI, same category as Claude Code) |

---

### Task 1: broker-server — JSON Config with Per-Runtime Upstream

**Files:**
- Modify: `apps/broker/src/config.rs`
- Modify: `apps/broker/Cargo.toml` (remove `toml` dep)

- [ ] **Step 1: Replace `apps/broker/src/config.rs` entirely**

The new config uses JSON, supports per-runtime upstream URLs, and reads from `~/.opencreavor/settings.json` by default. Remove all old TOML-based Config structs. The new module defines `Settings`, `BrokerSettings`, `AuditSettings`, `RulesSettings` with serde defaults. Key method: `get_upstream(runtime) -> Option<&str>` to resolve per-runtime upstream.

New `config.rs` should define:
- `Settings` struct with `broker`, `upstream: HashMap<String, String>`, `audit`, `rules`
- `BrokerSettings` with `port`, `log_level`, `block_status_code`, `block_error_style`, `stream_passthrough`, `upstream_timeout_secs: u64`, `idle_stream_timeout_secs: u64`
- `AuditSettings` with `event_auth_token: Option<String>`, `store_request_payloads: bool` (default false), `store_response_payloads: bool` (default false)
- `RulesSettings` with `llm_analyzer_enabled: bool` (default false)
- `Settings::default_path()` → `$HOME/.opencreavor/settings.json`
- `Settings::load(path)` → parse JSON from file
- `Settings::load_or_default()` → try default path, fallback to default
- `Settings::resolve_env_ref(value)` → resolve `env:VAR_NAME` tokens
- `Settings::get_upstream(runtime)` → lookup in upstream map

Tests:
- `settings_defaults_match_p0_spec` — all defaults correct
- `settings_load_from_json_with_upstream` — parse upstream map
- `settings_load_partial_json_inherits_defaults` — missing fields get defaults
- `settings_rejects_unknown_fields` — deny_unknown_fields
- `resolve_env_ref_reads_environment_variable`
- `resolve_env_ref_rejects_empty_variable_name`

- [ ] **Step 2: Remove `toml` from `apps/broker/Cargo.toml`**

Remove the line `toml = "0.8"`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p creavor-broker config`
Expected: all new config tests pass. Other tests may fail due to old `Config` references — that's expected, fixed in later tasks.

- [ ] **Step 4: Commit**

Run: `git add apps/broker/src/config.rs apps/broker/Cargo.toml && git commit -m "refactor: migrate broker config from TOML to JSON with per-runtime upstream"`

---

### Task 2: broker-server — Update Storage Schema

**Files:**
- Modify: `apps/broker/src/storage.rs`
- Modify: `apps/broker/src/audit.rs`

- [ ] **Step 1: Update `storage.rs` schema and methods**

Schema changes:
- `events`: add `session_id TEXT`, `tool_name TEXT`, `source TEXT` columns
- `requests`: add `session_id TEXT`, `runtime TEXT NOT NULL`, `blocked BOOLEAN DEFAULT FALSE`, `block_reason TEXT`, `rule_id TEXT`, `severity TEXT`, `latency_ms INTEGER`; remove old `provider` → rename is fine, keep `provider` too
- `violations`: add `rule_id TEXT NOT NULL`, `severity TEXT NOT NULL`, `matched_content TEXT`; remove old `detail` column

Update `AuditStorage` methods:
- `insert_event(session_id, event_type, tool_name, payload, source)` — new signature
- `insert_request_start(request_id, session_id, runtime, provider, method, path, blocked, block_reason, rule_id, severity)` — new signature, no longer writes payloads
- `insert_request_payload(request_id, body)` — new separate method
- `finalize_request(request_id, terminal_reason, response_status, latency_ms)` — add `latency_ms`
- `insert_response_payload(request_id, body)` — new separate method
- `insert_violation(request_id, rule_id, rule_name, severity, matched_content, action)` — new signature

Tests: rewrite all tests in `storage.rs` to match new schema.

- [ ] **Step 2: Update `audit.rs`**

Update `insert_event` call signature. Keep all correlation/sanitize functions unchanged. Update tests to match new signatures.

- [ ] **Step 3: Run tests**

Run: `cargo test -p creavor-broker -- storage::tests audit::tests`
Expected: all storage and audit tests pass.

- [ ] **Step 4: Commit**

Run: `git add apps/broker/src/storage.rs apps/broker/src/audit.rs && git commit -m "refactor: update storage schema with runtime/session/rule fields"`

---

### Task 3: broker-server — Update Interceptor

**Files:**
- Modify: `apps/broker/src/interceptor.rs`

- [ ] **Step 1: Update interceptor.rs**

Changes:
1. Add `RUNTIME_HEADER` constant for `x-creavor-runtime`
2. Add `strip_runtime_header(headers)` function
3. Add `strip_creavor_headers(headers)` that strips both session and runtime headers
4. Fix `openai_block_response_with_status`: change `"code": null` → `"code": "content_policy_violation"`
5. Keep `anthropic_block_response*` functions unchanged
6. Add `gemini_block_response(message)` / `gemini_block_response_with_status(status, message)` — same envelope style as OpenAI since Gemini CLI supports OpenAI-compatible error format

Tests to update:
- Update `openai_block_response_uses_provider_envelope` to assert `code == "content_policy_violation"`
- Add `strip_creavor_headers_removes_both_session_and_runtime` test

- [ ] **Step 2: Run tests**

Run: `cargo test -p creavor-broker interceptor`
Expected: all interceptor tests pass.

- [ ] **Step 3: Commit**

Run: `git add apps/broker/src/interceptor.rs && git commit -m "feat: strip X-Creavor-Runtime header, fix OpenAI block code field"`

---

### Task 4: broker-server — Update Events to Share Storage

**Files:**
- Modify: `apps/broker/src/events.rs`

- [ ] **Step 1: Update EventsState to accept `Arc<Mutex<AuditStorage>>`**

Change `EventsState::new` signature from `new(expected_token, storage: AuditStorage)` to `new(expected_token, storage: Arc<Mutex<AuditStorage>>)`.

Remove the internal `Arc<Mutex<>>` wrapping since storage already arrives wrapped.

The `persist_event` method now just locks and calls storage directly without double-wrapping.

Update internal tests to construct `EventsState` with `Arc::new(Mutex::new(AuditStorage::open_in_memory().unwrap()))`.

- [ ] **Step 2: Run tests**

Run: `cargo test -p creavor-broker events`
Expected: all events tests pass.

- [ ] **Step 3: Commit**

Run: `git add apps/broker/src/events.rs && git commit -m "refactor: EventsState accepts shared Arc<Mutex<AuditStorage>>"`

---

### Task 5: broker-server — Runtime-Aware Router with Audit Wiring

**Files:**
- Modify: `apps/broker/src/router.rs`
- Modify: `apps/broker/src/lib.rs`

This is the core task. The router now:
1. Reads `X-Creavor-Runtime` header to identify the source runtime
2. Resolves upstream URL from `settings.upstream[runtime]`
3. Falls back to `settings.upstream["default"]` or the first matching entry
4. Writes audit records for every request (blocked and forwarded)
5. Uses shared `Arc<Mutex<AuditStorage>>` between events and proxy
6. Supports three providers: Anthropic (`/v1/anthropic/*`), OpenAI (`/v1/openai/*`), Gemini (`/v1/gemini/*`)

- [ ] **Step 1: Rewrite `router.rs`**

Key changes:
- Single `AppState` struct with `settings: Settings`, `rules: RuleSet`, `storage: Arc<Mutex<AuditStorage>>`, `client`
- Single `app(settings, storage)` function that:
  - Creates shared storage: `Arc::new(Mutex::new(storage))`
  - Creates `EventsState` for `/api/v1/events` route
  - Creates `AppState` for proxy routes
  - Merges both routers
- Routes: `/v1/openai`, `/v1/openai/{*path}`, `/v1/anthropic`, `/v1/anthropic/{*path}`, `/v1/gemini`, `/v1/gemini/{*path}`
- `Provider` enum: `Anthropic`, `OpenAI`, `Gemini`
- `provider_for_path`: maps `/v1/gemini/*` → `Provider::Gemini`
- `proxy_request` handler:
  1. Read `X-Creavor-Runtime` header → runtime name (e.g. "claude")
  2. Read `X-Creavor-Session-Id` header → session ID
  3. Resolve upstream from `state.settings.get_upstream(runtime)` — if None, return 502
  4. Strip both Creavor headers before forwarding
  5. Scan request body with rule engine
  6. If blocked:
     - Write audit: `insert_request_start(blocked=true)`, `insert_violation(...)`, `finalize_request(...)`
     - Return provider-compatible block response
  7. If passed:
     - Write audit: `insert_request_start(blocked=false)`
     - If `store_request_payloads`: `insert_request_payload(...)`
     - Forward to upstream with streaming
     - On completion: `finalize_request(...)` with latency_ms
     - If `store_response_payloads`: `insert_response_payload(...)`
- `upstream_uri(base_url, provider, uri)` — unchanged logic but now base_url comes from per-runtime lookup

Keep all existing unit tests for `provider_for_path`, `upstream_uri`, health, events auth, correlation, rate limiting. Update test helper to use new `Settings` and `Arc<Mutex<AuditStorage>>`.

- [ ] **Step 2: Update `lib.rs` entry point**

Replace the old `run()` function to use `Settings` instead of `Config`:

```rust
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

    tracing::info!(
        port = settings.broker.port,
        upstream_count = settings.upstream.len(),
        "starting broker-server"
    );

    let storage = storage::AuditStorage::open(db_path)?;
    let app = router::app(settings, storage);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], settings.broker.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("broker-server listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p creavor-broker`
Expected: all broker tests pass.

- [ ] **Step 4: Commit**

Run: `git add apps/broker/src/router.rs apps/broker/src/lib.rs && git commit -m "feat: runtime-aware routing with per-runtime upstream and audit wiring"`

---

### Task 6: broker-server — Update Integration Tests

**Files:**
- Modify: `apps/broker/tests/https_proxy.rs`
- Modify: `apps/broker/tests/blocking.rs`
- Modify: `apps/broker/tests/streaming.rs`
- Modify: `apps/broker/tests/events_auth.rs`

- [ ] **Step 1: Update all integration tests to use new `Settings` type**

Replace any `Config` references with `Settings`. Update test helper functions that construct config objects.

For `https_proxy.rs`:
- Update the test server setup to use `Settings`
- Ensure `X-Creavor-Runtime` header is sent in proxy tests
- Verify `X-Creavor-Session-Id` is still stripped (existing test)
- Add a test verifying `X-Creavor-Runtime` is stripped before forwarding
- Add a test verifying upstream is resolved from settings per-runtime

For `blocking.rs`:
- Update to use `Settings`
- Verify block responses have `"code": "content_policy_violation"` for OpenAI

For `streaming.rs`:
- Update to use `Settings`

For `events_auth.rs`:
- Update to use `Settings`

- [ ] **Step 2: Run all broker tests**

Run: `cargo test -p creavor-broker`
Expected: all tests pass.

- [ ] **Step 3: Commit**

Run: `git add apps/broker/tests/ && git commit -m "test: update integration tests for new Settings config"`

---

### Task 7: creavor-cli — Shared Settings Module

**Files:**
- Rewrite: `apps/creavor-cli/src/settings.rs`

- [ ] **Step 1: Rewrite settings.rs**

The new `settings.rs` manages `~/.opencreavor/settings.json` — both reading broker config and writing upstream URLs when a runtime is registered.

Define:

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// The shared settings file at ~/.opencreavor/settings.json
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct CreavorSettings {
    pub broker: BrokerConfig,
    pub upstream: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct BrokerConfig {
    pub port: u16,
    pub log_level: String,
}

impl Default for CreavorSettings {
    fn default() -> Self {
        Self {
            broker: BrokerConfig::default(),
            upstream: HashMap::new(),
        }
    }
}

impl Default for BrokerConfig {
    fn default() -> Self {
        Self {
            port: 8765,
            log_level: "info".to_string(),
        }
    }
}
```

Methods:
- `CreavorSettings::path()` → `$HOME/.opencreavor/settings.json`
- `CreavorSettings::load()` → read from path, or default
- `CreavorSettings::save(&self)` → write to path (create dirs if needed)
- `CreavorSettings::set_upstream(runtime, url)` → insert into upstream map
- `CreavorSettings::get_upstream(runtime)` → lookup
- `CreavorSettings::broker_base_url()` → `http://127.0.0.1:{port}`

Also define runtime-specific config reading:

```rust
pub enum RuntimeType {
    Claude,
    OpenCode,
    OpenClaw,
    Codex,
    Cline,
    Gemini,
}

impl RuntimeType {
    pub fn name(&self) -> &'static str { ... }
    pub fn provider(&self) -> &'static str { ... }  // "anthropic" or "openai"
    pub fn broker_route(&self) -> String { ... }    // "/v1/anthropic" or "/v1/openai"
    pub fn base_url_env_var(&self) -> &'static str { ... }
    pub fn binary_name(&self) -> &'static str { ... }
    pub fn read_current_api_url(&self) -> Option<String> { ... }
    pub fn write_api_url(&self, url: &str) -> anyhow::Result<()> { ... }
}
```

For `read_current_api_url`:
- Claude: read `~/.claude/settings.json` → `apiBaseUrl` field
- OpenCode: check env `OPENAI_BASE_URL`, then config file
- OpenClaw: check env `OPENAI_BASE_URL`, then config file
- Codex: check env `OPENAI_BASE_URL`, then config file
- Cline: read VS Code settings or env `OPENAI_BASE_URL`
- Gemini: read `~/.gemini/settings.json` or env `GEMINI_API_BASE`, falls under Google Gemini protocol

For `write_api_url` (used by `creavor config`):
- Claude: write `apiBaseUrl` to `~/.claude/settings.json`
- OpenCode: write to its config file
- OpenClaw/Codex/Cline: write to respective config or env
- Gemini: write to `~/.gemini/settings.json` or relevant Gemini CLI config

- [ ] **Step 2: Run tests**

Run: `cargo test -p creavor-cli settings`
Expected: settings tests pass.

- [ ] **Step 3: Commit**

Run: `git add apps/creavor-cli/src/settings.rs && git commit -m "feat: creavor-cli settings module for ~/.opencreavor/settings.json"`

---

### Task 8: creavor-cli — Update CLI Parser

**Files:**
- Modify: `apps/creavor-cli/src/cli.rs`

- [ ] **Step 1: Update cli.rs**

Add `Config` command variant and new runtimes:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Claude,
    OpenCode,
    OpenClaw,
    Codex,
    Cline,
    Gemini,
}

#[derive(Debug)]
pub enum Command {
    Run { runtime: Runtime },
    Config { runtime: Runtime },
    Status,
}
```

Update USAGE text to include:
- `creavor config <runtime>` — Permanently configure runtime to use broker
- Add `codex`, `cline`, `gemini` to runtime list

Update `parse()` to handle `config` subcommand and new runtime names.

- [ ] **Step 2: Run tests**

Run: `cargo test -p creavor-cli cli`
Expected: parser tests pass.

- [ ] **Step 3: Commit**

Run: `git add apps/creavor-cli/src/cli.rs && git commit -m "feat: add config command and codex/cline/gemini runtimes to CLI parser"`

---

### Task 9: creavor-cli — Rewrite Runtime Launchers

**Files:**
- Rewrite: `apps/creavor-cli/src/runtimes/claude.rs`
- Rewrite: `apps/creavor-cli/src/runtimes/opencode.rs`
- Rewrite: `apps/creavor-cli/src/runtimes/openclaw.rs`
- Modify: `apps/creavor-cli/src/runtimes/mod.rs`

Each runtime launcher implements two functions: `run()` and `config()`.

- [ ] **Step 1: Rewrite `claude.rs`**

`run()` flow:
1. Read current API URL from `~/.claude/settings.json`
2. Check if URL is already broker address (contains `127.0.0.1:8765`)
   - If yes: skip upstream registration
   - If no: write original URL to `~/.opencreavor/settings.json` upstream.claude, save settings
3. Generate session ID
4. Set env vars:
   - `ANTHROPIC_BASE_URL=http://127.0.0.1:8765/v1/anthropic`
   - `ANTHROPIC_CUSTOM_HEADERS=X-Creavor-Session-Id:{session_id},X-Creavor-Runtime:claude`
   - `CREAVOR_SESSION_ID={session_id}`
5. Launch `claude` binary

`config()` flow:
1. Read current API URL from `~/.claude/settings.json`
2. If already broker address: print "already configured" and exit
3. Write original URL to `~/.opencreavor/settings.json` upstream.claude
4. Write `apiBaseUrl: http://127.0.0.1:8765/v1/anthropic` to `~/.claude/settings.json`
5. Print success message

Helper function `is_broker_address(url: &str) -> bool` checks if URL contains `127.0.0.1:8765` or `localhost:8765`.

- [ ] **Step 2: Rewrite `opencode.rs`**

Same pattern as claude but:
- Reads OpenCode config for API URL
- Uses `OPENAI_BASE_URL` env var
- Route: `/v1/openai`
- Runtime header: `X-Creavor-Runtime:opencode`

- [ ] **Step 3: Rewrite `openclaw.rs`**

Same pattern as opencode.

- [ ] **Step 4: Update `mod.rs` dispatch**

```rust
pub fn run(runtime: Runtime) -> anyhow::Result<()> {
    match runtime {
        Runtime::Claude => claude::run(),
        Runtime::OpenCode => opencode::run(),
        Runtime::OpenClaw => openclaw::run(),
        Runtime::Codex => codex::run(),
        Runtime::Cline => cline::run(),
        Runtime::Gemini => gemini::run(),
    }
}

pub fn config(runtime: Runtime) -> anyhow::Result<()> {
    match runtime {
        Runtime::Claude => claude::config(),
        Runtime::OpenCode => opencode::config(),
        Runtime::OpenClaw => openclaw::config(),
        Runtime::Codex => codex::config(),
        Runtime::Cline => cline::config(),
        Runtime::Gemini => gemini::config(),
    }
}
```

- [ ] **Step 5: Update `main.rs` to dispatch config command**

```rust
match command {
    cli::Command::Run { runtime } => runtimes::run(runtime),
    cli::Command::Config { runtime } => runtimes::config(runtime),
    cli::Command::Status => broker::status(),
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p creavor-cli`
Expected: all CLI tests pass.

- [ ] **Step 7: Commit**

Run: `git add apps/creavor-cli/src/ && git commit -m "feat: rewrite runtime launchers with upstream auto-registration"`

---

### Task 10: creavor-cli — Add Codex, Cline, and Gemini Runtimes

**Files:**
- Create: `apps/creavor-cli/src/runtimes/codex.rs`
- Create: `apps/creavor-cli/src/runtimes/cline.rs`
- Create: `apps/creavor-cli/src/runtimes/gemini.rs`

- [ ] **Step 1: Create `codex.rs`**

Codex uses OpenAI-compatible API:
- Binary: `codex`
- Route: `/v1/openai`
- Env var: `OPENAI_BASE_URL`
- Session header: injected via env or config depending on what codex supports

`run()` and `config()` follow the same pattern as opencode.

- [ ] **Step 2: Create `cline.rs`**

Cline uses OpenAI-compatible API:
- Binary: `cline` (or launched via VS Code extension — check PATH)
- Route: `/v1/openai`
- Env var: `OPENAI_BASE_URL`
- Config may need VS Code settings.json modification for permanent config

`run()` and `config()` follow same pattern, with `config()` writing to VS Code settings if applicable.

- [ ] **Step 3: Create `gemini.rs`**

Gemini CLI is the same category of tool as Claude Code — a standalone CLI coding agent:
- Binary: `gemini`
- Route: `/v1/openai` (Gemini CLI supports OpenAI-compatible endpoints via config/env)
- Env var: `GEMINI_API_BASE` or `OPENAI_BASE_URL` (depends on gemini-cli config)
- Session header: `X-Creavor-Session-Id` and `X-Creavor-Runtime:gemini` injected via env or config
- Config file: `~/.gemini/settings.json` (or equivalent gemini-cli config location)

`run()` flow (same pattern):
1. Read current API URL from gemini config
2. If not broker address: write original to `~/.opencreavor/settings.json` upstream.gemini
3. Generate session ID
4. Set env vars for base URL and custom headers
5. Launch `gemini` binary

`config()` flow: permanently modify gemini-cli config to point to broker.

- [ ] **Step 4: Run tests**

Run: `cargo test -p creavor-cli`
Expected: all tests pass.

- [ ] **Step 5: Commit**

Run: `git add apps/creavor-cli/src/runtimes/codex.rs apps/creavor-cli/src/runtimes/cline.rs apps/creavor-cli/src/runtimes/gemini.rs && git commit -m "feat: add codex, cline, and gemini runtime support"`

---

### Task 11: End-to-End Verification

**Files:**
- No new files — verification only

- [ ] **Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 2: Manual smoke test — broker-server**

```bash
# Create settings file
mkdir -p ~/.opencreavor
cat > ~/.opencreavor/settings.json << 'EOF'
{
  "broker": { "port": 8765 },
  "upstream": {
    "claude": "https://api.anthropic.com",
    "opencode": "https://api.openai.com/v1"
  }
}
EOF

# Start broker
cargo run -p creavor-broker
# Expected: "broker-server listening on http://127.0.0.1:8765"

# In another terminal:
curl http://127.0.0.1:8765/health
# Expected: {"status":"ok","service":"creavor-broker"}
```

- [ ] **Step 3: Manual smoke test — creavor-cli**

```bash
# Check status
cargo run -p creavor-cli -- status
# Expected: "broker is healthy at http://127.0.0.1:8765"

# Check settings.json was created
cat ~/.opencreavor/settings.json
```

- [ ] **Step 4: Commit if any fixes needed**

Run: `git add -A && git commit -m "fix: e2e verification fixes"`

---

## Spec Coverage Check

| Requirement | Task |
|-------------|------|
| JSON config from `~/.opencreavor/settings.json` | Task 1 |
| Per-runtime upstream URLs | Task 1, Task 5 |
| `X-Creavor-Runtime` header for runtime identification | Task 3, Task 5 |
| `X-Creavor-Session-Id` for session correlation | Task 3, Task 5 (existing) |
| Strip both Creavor headers before forwarding | Task 3 |
| Audit wired into proxy flow (blocked + forwarded) | Task 2, Task 5 |
| Payload storage configurable (default off) | Task 2 |
| Storage schema with runtime/session/rule/latency | Task 2 |
| OpenAI block `code: "content_policy_violation"` | Task 3 |
| Shared storage between events and proxy | Task 4, Task 5 |
| `creavor run <runtime>` with upstream registration | Task 9 |
| `creavor config <runtime>` permanent rewrite | Task 9 |
| `creavor status` health check | Existing (unchanged) |
| Codex runtime support | Task 10 |
| Cline runtime support | Task 10 |
| Gemini CLI runtime support | Task 10 |
| Integration tests updated | Task 6 |
