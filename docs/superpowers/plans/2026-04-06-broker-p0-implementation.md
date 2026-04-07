# Creavor Broker P0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a production-usable P0 local broker for interception/audit with deterministic rules, provider-compatible blocking responses, authenticated local event ingestion, and robust streaming pass-through semantics.

**Architecture:** A Rust workspace with one `apps/broker` binary. `axum` handles HTTP routing, `hyper` handles upstream forwarding/streaming, `rusqlite` persists audit records, and a deterministic `rule_engine` decides allow/block before upstream calls. Runtime hooks post events to a local authenticated endpoint; request/event records are correlated by session id header or fallback keys.

**Tech Stack:** Rust, tokio, axum, hyper, serde, toml, regex, rusqlite, uuid, tracing, insta/snapshot (optional), cargo test.

---

### Task 1: Workspace And Binary Bootstrap

**Files:**
- Create: `Cargo.toml`
- Create: `apps/broker/Cargo.toml`
- Create: `apps/broker/src/main.rs`
- Create: `apps/broker/src/lib.rs`

- [ ] **Step 1: Create workspace manifests**
```toml
# Cargo.toml
[workspace]
members = ["apps/broker"]
resolver = "2"
```
```toml
# apps/broker/Cargo.toml
[package]
name = "creavor-broker"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "signal", "time"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
hyper = { version = "1", features = ["full"] }
hyper-util = { version = "0.1", features = ["client", "client-legacy", "http1", "http2", "tokio"] }
http-body-util = "0.1"
regex = "1"
rusqlite = { version = "0.32", features = ["bundled"] }
uuid = { version = "1", features = ["v4", "serde"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

- [ ] **Step 2: Add boot binary and library entry**
```rust
// apps/broker/src/main.rs
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    creavor_broker::run().await
}
```
```rust
// apps/broker/src/lib.rs
pub async fn run() -> anyhow::Result<()> {
    Ok(())
}
```

- [ ] **Step 3: Build to validate scaffold**
Run: `cargo build -p creavor-broker`  
Expected: build succeeds with no missing module errors.

- [ ] **Step 4: Commit**
Run: `git add Cargo.toml apps/broker/Cargo.toml apps/broker/src/main.rs apps/broker/src/lib.rs && git commit -m "chore: bootstrap broker workspace"`

### Task 2: Config Model And Strict P0 Defaults

**Files:**
- Create: `apps/broker/src/config.rs`
- Modify: `apps/broker/src/lib.rs`
- Test: `apps/broker/src/config.rs` (unit tests module)

- [ ] **Step 1: Write failing tests for defaults and env token resolution**
- [ ] **Step 2: Implement config structs with P0 fields**
Include fields from spec: `block_status_code`, `block_error_style`, `stream_passthrough`, `upstream_timeout`, `idle_stream_timeout`, `audit.event_auth_token`, `rules.*`, `llm_analyzer.enabled=false`.
- [ ] **Step 3: Implement `Config::load(path)` and `resolve_env_ref("env:FOO")`**
- [ ] **Step 4: Run tests**
Run: `cargo test -p creavor-broker config -- --nocapture`  
Expected: all config tests pass.
- [ ] **Step 5: Commit**
Run: `git add apps/broker/src/config.rs apps/broker/src/lib.rs && git commit -m "feat: add broker config with p0 defaults"`

### Task 3: Deterministic Rule Engine (Layer 1 Only)

**Files:**
- Create: `apps/broker/src/rule_engine.rs`
- Create: `apps/broker/rules/secrets.yaml`
- Create: `apps/broker/rules/pii.yaml`
- Create: `apps/broker/rules/enterprise.yaml`
- Test: `apps/broker/src/rule_engine.rs` (unit tests)

- [ ] **Step 1: Write failing tests for secret regex, pii, and no-false-positive baseline**
- [ ] **Step 2: Implement rule types**
Use `RuleMatch { rule_id, rule_name, severity, matched_content_sanitized }`.
- [ ] **Step 3: Implement engine scan API**
```rust
pub fn scan_request(body: &str, rules: &RuleSet) -> Option<RuleMatch>
```
First-match-wins for P0 predictability.
- [ ] **Step 4: Add sanitize helper**
Mask strategy: preserve small prefix/suffix, replace middle with `***`.
- [ ] **Step 5: Run tests**
Run: `cargo test -p creavor-broker rule_engine -- --nocapture`  
Expected: deterministic pass/fail behavior.
- [ ] **Step 6: Commit**
Run: `git add apps/broker/src/rule_engine.rs apps/broker/rules/*.yaml && git commit -m "feat: implement deterministic layer1 rule engine"`

### Task 4: Provider Routing And Blocking Error Envelopes

**Files:**
- Create: `apps/broker/src/router.rs`
- Create: `apps/broker/src/interceptor.rs`
- Modify: `apps/broker/src/lib.rs`
- Test: `apps/broker/src/router.rs` (unit tests)

- [ ] **Step 1: Write failing tests for path-to-provider mapping**
- [ ] **Step 2: Implement provider mapping**
`/v1/anthropic/* -> anthropic`, `/v1/openai/* -> openai`.
- [ ] **Step 3: Implement provider-compatible block response builders**
Anthropic envelope and OpenAI envelope with default status code `400`.
- [ ] **Step 4: Enforce header strip**
Remove `X-Creavor-Session-Id` before forwarding upstream.
- [ ] **Step 5: Run tests**
Run: `cargo test -p creavor-broker router interceptor -- --nocapture`  
Expected: response schema and status assertions pass.
- [ ] **Step 6: Commit**
Run: `git add apps/broker/src/router.rs apps/broker/src/interceptor.rs apps/broker/src/lib.rs && git commit -m "feat: add provider routing and block envelopes"`

### Task 5: Streaming Proxy Semantics And Timeouts

**Files:**
- Create: `apps/broker/src/proxy.rs`
- Modify: `apps/broker/src/lib.rs`
- Test: `apps/broker/src/proxy.rs` (integration-style unit tests with mock upstream)

- [ ] **Step 1: Write failing tests for SSE/chunked pass-through**
- [ ] **Step 2: Implement forwarder with zero-buffer streaming**
Forward request body and upstream chunks without full-body aggregation.
- [ ] **Step 3: Implement cancel and timeout handling**
Use `tokio::time::timeout` for connect/upstream/idle stream.
- [ ] **Step 4: Return deterministic terminal reasons**
`ok`, `client_cancelled`, `upstream_timeout`, `idle_timeout`, `network_error`.
- [ ] **Step 5: Run tests**
Run: `cargo test -p creavor-broker proxy -- --nocapture`  
Expected: stream behavior tests pass, including cancellation path.
- [ ] **Step 6: Commit**
Run: `git add apps/broker/src/proxy.rs apps/broker/src/lib.rs && git commit -m "feat: implement streaming passthrough and timeout controls"`

### Task 6: SQLite Audit Storage And Write Guarantees

**Files:**
- Create: `apps/broker/src/storage.rs`
- Create: `apps/broker/src/audit.rs`
- Modify: `apps/broker/src/lib.rs`
- Test: `apps/broker/src/storage.rs` and `apps/broker/src/audit.rs`

- [ ] **Step 1: Write failing schema/init tests**
- [ ] **Step 2: Implement schema creation**
Create `events`, `requests`, `request_payloads`, `response_payloads`, `violations`.
- [ ] **Step 3: Implement audit write APIs**
`insert_event`, `insert_request_start`, `finalize_request`, `insert_violation`.
- [ ] **Step 4: Enforce write-on-termination**
Finalize requests on success and all early termination reasons.
- [ ] **Step 5: Run tests**
Run: `cargo test -p creavor-broker storage audit -- --nocapture`  
Expected: all terminal states persist consistent rows.
- [ ] **Step 6: Commit**
Run: `git add apps/broker/src/storage.rs apps/broker/src/audit.rs apps/broker/src/lib.rs && git commit -m "feat: add sqlite audit schema and lifecycle writes"`

### Task 7: Authenticated Local Events API And Correlation

**Files:**
- Create: `apps/broker/src/events.rs`
- Modify: `apps/broker/src/router.rs`
- Modify: `apps/broker/src/audit.rs`
- Test: `apps/broker/src/events.rs`

- [ ] **Step 1: Write failing tests for event auth**
Cases: missing token -> 401, invalid token -> 401, valid token -> 202.
- [ ] **Step 2: Implement `POST /api/v1/events`**
Parse event payload, persist sanitized event JSON.
- [ ] **Step 3: Implement correlation strategy**
Prefer `X-Creavor-Session-Id`; fallback to runtime + timestamp bucket (+ optional cwd best effort).
- [ ] **Step 4: Add lightweight local rate limiting**
Per-process or per-IP token bucket for `/api/v1/events`.
- [ ] **Step 5: Run tests**
Run: `cargo test -p creavor-broker events -- --nocapture`  
Expected: auth + correlation tests pass.
- [ ] **Step 6: Commit**
Run: `git add apps/broker/src/events.rs apps/broker/src/router.rs apps/broker/src/audit.rs && git commit -m "feat: add authenticated local events ingestion and correlation"`

### Task 8: End-to-End P0 Acceptance Tests

**Files:**
- Create: `apps/broker/tests/p0_blocking.rs`
- Create: `apps/broker/tests/p0_streaming.rs`
- Create: `apps/broker/tests/p0_events_auth.rs`

- [ ] **Step 1: Add failing e2e test for blocked secret request**
- [ ] **Step 2: Add failing e2e test for allowed streaming passthrough**
- [ ] **Step 3: Add failing e2e test for event auth enforcement**
- [ ] **Step 4: Implement missing glue until all pass**
- [ ] **Step 5: Run full suite**
Run: `cargo test -p creavor-broker -- --nocapture`  
Expected: all broker tests pass.
- [ ] **Step 6: Commit**
Run: `git add apps/broker/tests/*.rs && git commit -m "test: add p0 end-to-end acceptance coverage"`

### Task 9: Runtime Setup Artifacts And Operator Docs

**Files:**
- Create: `runtimes/claude-code/README.md`
- Create: `runtimes/opencode/README.md`
- Create: `runtimes/openclaw/README.md`
- Create: `apps/broker/config/config.example.toml`
- Modify: `README.md`

- [ ] **Step 1: Document startup-based session id/header injection**
- [ ] **Step 2: Document `/api/v1/events` token provisioning**
Include generation example: `openssl rand -hex 32`.
- [ ] **Step 3: Document `block_status_code=400` default and provider envelope behavior**
- [ ] **Step 4: Document streaming timeout knobs and recommended defaults**
- [ ] **Step 5: Run smoke check commands**
Run: `cargo run -p creavor-broker -- --config apps/broker/config/config.example.toml`  
Expected: broker starts and prints listening address.
- [ ] **Step 6: Commit**
Run: `git add runtimes apps/broker/config/config.example.toml README.md && git commit -m "docs: add runtime setup and operator runbook for p0 broker"`

## Spec Coverage Check

- Session injection moved to startup launcher/config path: covered by Task 9 docs and Task 7 correlation.
- Blocking status code normalization to default 400: covered by Task 4.
- Local `/api/v1/events` authentication requirement: covered by Task 7 and Task 8.
- Streaming/cancellation/timeout semantics and terminal audit writes: covered by Task 5 and Task 6.

## Execution Notes

- Keep LLM analyzer disabled in P0 (`enabled = false`); do not route any payload to secondary LLM.
- Do not store raw request/response bodies unless explicit debug switch is enabled.
- Prefer frequent small commits exactly as listed above to simplify review and rollback.
