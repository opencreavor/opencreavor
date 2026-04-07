# Creavor Broker: AI Tool Audit & Intercept System

**Date:** 2026-04-06
**Status:** Draft
**Scope:** Phase 1 — Claude Code, OpenCode, OpenClaw support

## Problem

Enterprise engineering teams use AI coding tools (Claude Code, OpenCode, OpenClaw, etc.) that send code and context to external LLM APIs. This creates data leakage risk — sensitive information like API keys, internal project details, and proprietary code may be exposed. Enterprises need a way to audit, control, and intercept these communications for compliance.

## Solution

**Creavor Broker** — a local proxy that sits between AI coding tools and LLM APIs. It intercepts all requests, runs them through a rule engine and optional LLM-based semantic analysis, and blocks any request containing sensitive information.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                    Engineer Machine                   │
│                                                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐          │
│  │Claude Code│  │ OpenCode │  │ OpenClaw │  ...     │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘          │
│       │              │              │                │
│       ▼              ▼              ▼                │
│  ┌─────────────────────────────────────────┐        │
│  │         Creavor Broker (Proxy)           │        │
│  │  ┌──────────┐ ┌──────────┐ ┌─────────┐ │        │
│  │  │ Rule     │ │ LLM      │ │ Audit   │ │        │
│  │  │ Engine   │ │ Analyzer │ │ Logger  │ │        │
│  │  └──────────┘ └──────────┘ └─────────┘ │        │
│  │           ↓                              │        │
│  │  ┌─────────────────────────────┐        │        │
│  │  │ Interceptor (block/allow)   │        │        │
│  │  └─────────────────────────────┘        │        │
│  └──────────────┬──────────────────────────┘        │
│                  │                                   │
│            ┌─────┴──────┐                           │
│            │  SQLite DB  │                           │
│            └─────┬──────┘                           │
│                  │ (optional sync)                   │
└──────────────────┼───────────────────────────────────┘
                   ▼
          ┌──────────────┐
          │  Centralized │
          │  Audit Server│
          └──────────────┘
```

### Data Flow

1. Engineer types in AI tool → tool sends request to `localhost:PORT` (Broker)
2. Broker receives request → Rule Engine scans request body
3. **Hit rule** → Block, return provider-compatible `4xx` error envelope (default `400`) with reason, do not forward to LLM
4. **Rule clear, LLM analyzer enabled (sync mode)** → Wait for LLM analysis, block if sensitive
5. **All clear** → Forward to real LLM API → Response returns to tool
6. Audit Logger records the complete chain

### Hybrid Architecture

Two complementary data streams:

1. **Hook event stream** — Runtime plugins capture user behavior (session lifecycle, prompt submissions, tool calls)
2. **API request stream** — Broker captures full LLM communication content

Both streams are correlated by session ID for complete audit trail.

### Correlation Key Strategy

API requests do not naturally carry session context. The following mechanisms bridge this gap:

**Primary: Header injection via runtime startup config (not hook-export)**

Each runtime generates a session-scoped ID at process startup (launcher/config stage) and injects it so the AI tool includes it in API requests:

```
X-Creavor-Session-Id: {runtime_type}:{local_session_id}:{timestamp_bucket}
```

Example: `claude-code:a3f2e1:20260406T1430`

Session ID is generated at runtime startup and injected through tool-supported startup configuration:
```bash
# runtime launcher (wrapper script) before starting Claude Code
export CREAVOR_SESSION_ID="claude-code:$(uuidgen | cut -d'-' -f1):$(date -u +%Y%m%dT%H%M)"
export ANTHROPIC_BASE_URL="http://localhost:8765/v1/anthropic"
export ANTHROPIC_CUSTOM_HEADERS="X-Creavor-Session-Id:${CREAVOR_SESSION_ID}"
claude
```

Broker strips `X-Creavor-Session-Id` from the request before forwarding to the real LLM API.

> P0 constraint: do **not** rely on hook scripts to `export` env vars into already-running parent processes. Hook processes are treated as isolated collectors. Header injection must come from launcher/config paths controlled at process startup.

**Fallback: Multi-key fuzzy matching**

When header injection is not possible (tool doesn't support custom headers, or request arrives without session ID), Broker correlates using:

| Key | Source | Matching |
|-----|--------|----------|
| `runtime_type` | Request path (`/v1/anthropic` → `claude-code`) | Exact |
| `timestamp_bucket` | Request timestamp vs. hook event timestamps | ±5 min window |
| `cwd` / `repo` | Hook `SessionStart` event payload vs. inferred context | Best effort |
| `process_tree` | PID lineage from request socket to known runtime process | OS-specific |

**Correlation flow:**

```
1. SessionStart hook fires → POST /api/v1/events to Broker
   → Broker creates session record: {session_id, runtime_type, cwd, pid, start_time}

2. API request arrives at Broker:
   a. Has X-Creavor-Session-Id header → direct match
   b. No header → match by runtime_type + timestamp_bucket + cwd → pick closest session

3. Each request and event row stores session_id for queries
```

**Per-runtime correlation specifics:**

| Runtime | Session ID Source | Header Injection Method |
|---------|-------------------|------------------------|
| Claude Code | Launcher generates UUID at process startup | `ANTHROPIC_CUSTOM_HEADERS` env var |
| OpenCode | Launcher/config generates UUID at process startup | Config file custom header field |
| OpenClaw | Launcher/config generates UUID at process startup | Env/config field (must be validated before GA) |

## Core Modules

### 1. API Router & Forwarding

Broker is a protocol-aware reverse proxy supporting multiple LLM providers:

| Provider | Base URL | Auth Method |
|----------|----------|-------------|
| Anthropic | `api.anthropic.com` | `x-api-key` header |
| OpenAI-compatible | `api.openai.com` | `Authorization: Bearer` |
| Custom | User-configured | Configurable |

Routing: Tools configure `http://localhost:{PORT}/v1/{provider}` as API endpoint. Broker identifies target provider from path prefix and forwards accordingly.

### 2. Rule Engine

Two-layer detection:

**Layer 1 — Deterministic rules (synchronous, low latency):**

- Regex matching: API keys, private keys, password patterns (`-----BEGIN PRIVATE KEY-----`, `sk-`, `ghp_`, etc.)
- File fingerprints: Known sensitive file paths (`/etc/shadow`, `.env`, `credentials.json`)
- Keyword dictionary: Configurable enterprise sensitive word list
- PII detection: Email, phone numbers, ID numbers

**Layer 2 — LLM semantic analysis (DISABLED in P0, config placeholder only):**

> **P0 硬约束：LLM Analyzer 默认禁用，不在第一版主路径中启用。**
>
> 原因：
> 1. 显著增加本地延迟，影响工程师体验
> 2. 合规悖论 — 为了判断是否泄露，又把数据发给另一个模型
> 3. 结果难以稳定复现，影响第一版的可信度
>
> P0 的成功标准是 **确定性规则优先**，做到可预测、可复现、可解释。
> LLM Analyzer 留作配置占位，待规则引擎稳定后再评估是否启用。

- For requests that pass Layer 1 but contain substantial content
- Calls a lightweight LLM to judge whether content contains sensitive business information
- Configurable as synchronous (block until analysis completes) or asynchronous (allow through, flag later)

### 3. Interception Strategy

```
Request → Rule Engine (Layer 1)
           ├─ Hit → Block (403 + reason)
           └─ Clear → Forward to LLM API

Note: Layer 2 (LLM Analyzer) disabled in P0. Interception is 100% rule-based.
```

Block response format — **provider-compatible error envelope**:

Broker returns errors in the format each tool/SDK expects, not a custom schema. HTTP status code is configurable.

**Anthropic-style (for Claude Code):**
```json
{
  "type": "error",
  "error": {
    "type": "invalid_request_error",
    "message": "Request blocked by security policy: detected suspected AWS Secret Key"
  }
}
```
HTTP status: configurable, default `400` (Anthropic SDK treats this as retryable=false, displays to user)

**OpenAI-style (for OpenCode / OpenClaw):**
```json
{
  "error": {
    "message": "Request blocked by security policy: detected suspected AWS Secret Key",
    "type": "invalid_request_error",
    "code": "content_policy_violation"
  }
}
```
HTTP status: configurable, default `400`

**Internal fields (logged to violations table, not returned to tool):**
- `rule_id`: `aws-secret-key-001`
- `severity`: `high`
- `matched_content`: `sk-***xyz` (sanitized)

## Tool Plugins (Runtimes)

### Claude Code

Hooks configured via `~/.claude/settings.json`, monitoring full lifecycle:

| Event | Purpose | Data Collected |
|-------|---------|----------------|
| `SessionStart` | Record session start | Session ID, time, user, working directory |
| `UserPromptSubmit` | Capture user input | Full prompt text |
| `PreToolUse` | Capture tool call intent | Tool name, parameters |
| `PostToolUse` | Record tool results | Tool output |
| `PostToolUseFailure` | Record tool failures | Error info |
| `Stop` | Session pause/end marker | Final state |
| `SessionEnd` | Record session end | Session summary, duration |
| `SubagentStart`* | Sub-agent start | Sub-agent task description |
| `SubagentStop`* | Sub-agent end | Sub-agent result |
| `ConfigChange`* | Config change audit | Change content |

**P0: Hooks only collect, never block.** The hook stage captures user input, but this is not the same as what actually gets sent to the LLM — Claude Code appends tool results, system prompts, and context files. Blocking at hook level would be inconsistent with the final API body. Only the Broker has the full picture and is the sole interception point.

Hook scripts send collected data to Broker's local endpoint for audit and session correlation:

```
POST http://localhost:8765/api/v1/events
```

P0 local auth requirement for `/api/v1/events`:
- Require `Authorization: Bearer ${CREAVOR_BROKER_EVENT_TOKEN}` (token read from local file/env, never hard-coded)
- Reject missing/invalid token with `401`
- Optional hardened mode: Unix domain socket transport + file permission isolation
- Rate-limit endpoint to reduce local spam/flood risk

API endpoint redirect via environment variable:

```bash
export ANTHROPIC_BASE_URL=http://localhost:8765/v1/anthropic
```

### OpenCode

Custom provider configuration via config file:

```toml
[provider]
base_url = "http://localhost:8765/v1/openai"
```

### OpenClaw

Similar to OpenCode, supports environment variable or config file for LLM endpoint.

### Plugin Responsibility Summary

| Responsibility | Owner |
|---------------|-------|
| API request interception/audit | Broker |
| Sensitive info detection | Broker |
| Block notification display | Plugin (tool-native) |
| API endpoint configuration | Plugin (env vars / config files) |
| Audit log queries | Broker CLI |

## Audit Storage

### Storage Principles

Broker 本身是"防泄露"工具，不能自己成为泄露源。P0 存储策略：

- **默认只存脱敏数据** — 规则引擎匹配到的敏感字段用 `***` 替换后存入
- **Response body 默认不存全文** — 只存 status code + token count
- **Headers 只存白名单字段** — `content-type`, `x-request-id`, `x-cends-correlation-id` 等
- **原始全文仅在 debug/高权限模式下启用** — 通过配置开关控制

### Data Model

**events table — Hook event stream:**

```sql
CREATE TABLE events (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL,
    event_type  TEXT NOT NULL,
    tool_name   TEXT,
    payload     TEXT NOT NULL,   -- 脱敏后的 JSON
    timestamp   TEXT NOT NULL,
    source      TEXT NOT NULL
);
```

**requests table — 元数据 + 摘要 + 风险结果（不存大字段）:**

```sql
CREATE TABLE requests (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL,
    method          TEXT NOT NULL,
    path            TEXT NOT NULL,
    provider        TEXT NOT NULL,
    -- Headers: 只存白名单字段，JSON
    header_whitelist TEXT,        -- {"content-type":"...", "x-request-id":"..."}
    -- Request summary（不存全文）
    request_summary TEXT NOT NULL, -- 脱敏后的摘要，如 "messages[3 turns, ~2k tokens]"
    request_tokens  INTEGER,      -- 估算 token 数
    -- Response summary
    response_status INTEGER,
    response_summary TEXT,        -- 如 "completion: 847 tokens"
    response_tokens  INTEGER,
    -- Risk assessment
    blocked         BOOLEAN DEFAULT FALSE,
    block_reason    TEXT,
    rule_id         TEXT,
    severity        TEXT,         -- low / medium / high / critical
    -- Meta
    latency_ms      INTEGER,
    timestamp       TEXT NOT NULL
);
```

**request_payloads table — 可选，脱敏后的请求全文（按需开启）:**

```sql
CREATE TABLE request_payloads (
    id          TEXT PRIMARY KEY REFERENCES requests(id),
    body        TEXT NOT NULL,    -- 脱敏后的完整请求体
    stored_at   TEXT NOT NULL
);
```

**response_payloads table — 默认关闭，仅在 debug 模式下写入:**

```sql
CREATE TABLE response_payloads (
    id          TEXT PRIMARY KEY REFERENCES requests(id),
    body        TEXT NOT NULL,
    stored_at   TEXT NOT NULL
);
```

**violations table — Violation records:**

```sql
CREATE TABLE violations (
    id              TEXT PRIMARY KEY,
    request_id      TEXT REFERENCES requests(id),
    event_id        TEXT REFERENCES events(id),
    rule_id         TEXT NOT NULL,
    rule_name       TEXT NOT NULL,
    severity        TEXT NOT NULL,
    matched_content TEXT,         -- 脱敏后的匹配片段，如 "sk-***...***xyz"
    action          TEXT NOT NULL, -- blocked / flagged
    timestamp       TEXT NOT NULL
);
```

### Storage Configuration

```toml
[audit]
# ...
store_request_payloads = false   # P0: 关闭。开启后写入 request_payloads 表
store_response_payloads = false  # P0: 关闭。debug 模式下可临时开启
sanitize_mode = "mask"           # mask (***替换) | hash (保留哈希) | drop (直接删除)
header_whitelist = ["content-type", "x-request-id", "x-ratelimit-remaining"]
```

### Centralized Reporting

- Broker periodically syncs local logs to centralized audit server (configurable interval)
- Queue and retry on sync failure
- Centralized server provides unified audit reports and alerts

## Management Interface

### CLI

```bash
# Start Broker
creavor-broker start --port 8765 --config /etc/creavor/config.toml

# Status
creavor-broker status

# Audit log queries
creavor-broker logs --source claude-code --last 24h
creavor-broker logs --blocked-only

# Rule management
creavor-broker rules list
creavor-broker rules add --from-file sensitive-patterns.yaml
creavor-broker rules test --input "sk-ant-xxxxx"

# Statistics
creavor-broker stats --summary
```

### Configuration (`config.toml`)

```toml
[broker]
port = 8765
log_level = "info"
block_status_code = 400          # HTTP status for blocked requests (configurable)
block_error_style = "auto"       # auto | anthropic | openai — auto detects from request path
stream_passthrough = true        # P0: SSE/chunked zero-buffer pass-through
upstream_timeout = "300s"        # Long-running generation timeout
idle_stream_timeout = "120s"     # No-chunk timeout for stream connections

[audit]
retention_days = 90
central_url = "https://audit.enterprise.internal/api/v1/report"
central_token = "env:CENTRAL_AUDIT_TOKEN"
sync_interval = "5m"
event_auth_token = "env:CREAVOR_BROKER_EVENT_TOKEN"

[rules]
rules_dir = "/etc/creavor/rules"
builtin_secrets = true
builtin_pii = true
builtin_key_patterns = true

[llm_analyzer]
# P0: DISABLED. Config placeholder only. Do not enable in production.
# Will be evaluated for Phase 1.1 after rule engine stabilizes.
enabled = false
# model = "claude-haiku-4-5-20251001"
# sync_mode = true
# max_tokens = 1024
```

## Deployment

- **macOS**: Homebrew tap, launchd daemon
- **Linux**: .deb / .rpm packages, systemd service
- Enterprise IT can distribute configuration via MDM / config management tools

## Streaming & Connection Semantics (P0 hard requirements)

- Broker must transparently pass through SSE/chunked responses without body rebuffering.
- Client cancellation must propagate upstream immediately; request row should still be finalized with `response_status=499` (or local canceled marker) and measured `latency_ms`.
- Upstream timeout, idle stream timeout, and connect timeout are configurable and logged on failure.
- For blocked requests, Broker returns provider-compatible JSON error envelope and never opens upstream stream.
- Audit writes must happen on both success and early termination (timeout/cancel/network reset), with explicit terminal reason.

## Project Structure

```
opencreavor/
├── Cargo.toml                  # workspace
├── apps/
│   └── broker/
│       ├── Cargo.toml
│       ├── rules/
│       │   ├── secrets.yaml    # Key/credential patterns
│       │   ├── pii.yaml        # PII patterns
│       │   └── enterprise.yaml # Enterprise custom template
│       └── src/
│           ├── main.rs         # CLI entry
│           ├── proxy.rs        # HTTP reverse proxy
│           ├── router.rs       # API route dispatch
│           ├── rule_engine.rs  # Rule engine
│           ├── analyzer.rs     # LLM semantic analysis
│           ├── interceptor.rs  # Interceptor
│           ├── audit.rs        # Audit logging
│           ├── storage.rs      # SQLite storage
│           ├── sync.rs         # Centralized reporting
│           └── config.rs       # Configuration management
├── runtimes/
│   ├── claude-code/            # Claude Code hooks
│   ├── opencode/               # OpenCode plugin
│   └── openclaw/               # OpenClaw plugin
└── docs/
```

## Tech Stack

- **Language**: Rust
- **Async runtime**: tokio
- **HTTP proxy**: hyper / axum
- **Storage**: SQLite via rusqlite
- **Rule engine**: Custom (regex + YAML config)
- **LLM analysis**: Calls external LLM API (configurable)

## Phase 1 Scope

- [x] Broker proxy core (route, forward, intercept)
- [x] Rule engine with built-in patterns (P0 success criteria: deterministic, reproducible, explainable)
- [x] Claude Code runtime (hooks: collect-only + endpoint redirect)
- [x] OpenCode runtime
- [x] OpenClaw runtime
- [x] Audit logging (SQLite)
- [ ] LLM analyzer (disabled in P0, config placeholder only)
- [ ] Centralized reporting (optional, Phase 1.1)
- [ ] CLI management interface
