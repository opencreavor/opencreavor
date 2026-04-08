# <svg viewBox="0 0 100 100" xmlns="http://www.w3.org/2000/svg" >
<defs>
  <linearGradient id="logoGradient" x1="0%" y1="0%" x2="100%" y2="100%">
    <stop offset="0%" stop-color="#3b82f6" />
    <stop offset="100%" stop-color="#06b6d4" />
  </linearGradient>
</defs>
<!-- Outer Hexagon -->
<path d="M50 8 L86 29 L86 71 L50 92 L14 71 L14 29 Z" stroke="url(#logoGradient)" stroke-width="12" stroke-linecap="round" stroke-linejoin="round" fill="none" />
<!-- Connection Lines -->
<path d="M50 20 L50 36" stroke="url(#logoGradient)" stroke-width="10" stroke-linecap="round" />
<path d="M26 62 L40 54" stroke="url(#logoGradient)" stroke-width="10" stroke-linecap="round" />
<path d="M74 62 L60 54" stroke="url(#logoGradient)" stroke-width="10" stroke-linecap="round" />
<!-- Core Circle -->
<circle cx="50" cy="50" r="14" fill="#3b82f6" />
</svg>

# opencreavor

### Your AI-native R&D organization in a box.
**Spec-driven. Multi-agent. Privacy-first.**
From idea to deployable product — with full control.

---

## What is opencreavor?

opencreavor is an open-source, privacy-first AI R&D system designed for **technical founders** and **enterprise engineering leaders**.

It transforms creative ideas into production-ready systems through **Spec-Driven Development (SDD)** and **multi-agent orchestration**, while preserving full control over knowledge, compliance, and intellectual property.

Unlike traditional AI coding assistants, opencreavor models a **structured engineering organization**, where specifications, policies, knowledge, and agents work together as a cohesive system.

---

## Why opencreavor?

Modern AI tools generate code. They do not manage engineering systems. opencreavor provides:

- **Structured specifications as the single source of truth**
- **Multi-agent role orchestration**: Product, Architect, Backend, Frontend, QA, DevOps
- **Version-aware technical knowledge alignment**
- **Private, self-hosted knowledge system**
- **Policy- and compliance-aware generation**
- **Plugin-driven extensibility**: agents, connectors, models, deployment targets

It bridges the gap between large language models and **real-world software delivery**.

---

## Built for Control, Speed, and Scale

opencreavor is designed for teams that require:

- Local-first or fully self-hosted deployment
- Protection of confidential knowledge and known internal risks
- Enforcement of compliance policies (e.g., GDPR, internal IT rules)
- Long-term system evolution rather than one-off generation
- Auditability and traceable decision-making

All specifications, architectural decisions, compliance rules, and internal knowledge remain **fully under your control**.

---

## Out-of-the-Box and Extensible

opencreavor works immediately after installation:

- Predefined multi-agent workflow
- Built-in spec templates
- Integrated private knowledge layer
- Default project scaffolding

At the same time, everything is pluggable:

- Agents
- Model providers
- Knowledge ingestion connectors
- Policy modules
- Deployment targets

It is **ready to run**, and built to **evolve with your organization**.

---

## Project Structure

```
opencreavor/
├── libs/creavor-core/     # Shared types: Settings, RuntimeType, utilities
├── apps/broker/           # broker-server: local interception proxy
└── apps/creavor-cli/      # creavor: CLI tool to launch runtimes through broker
```

All three crates share configuration via `~/.opencreavor/settings.json`.

---

## Quick Start

### 1. Start the broker

```bash
# Option A: with example config
cargo run -p creavor-broker -- --config apps/broker/config/settings.example.json

# Option B: using defaults (reads ~/.opencreavor/settings.json)
cargo run -p creavor-broker
```

### 2. Launch a runtime through the broker

```bash
# Build and install the CLI
cargo install --path apps/creavor-cli

# Run a runtime (auto-registers upstream, launches with proxy)
creavor run claude
creavor run gemini
creavor run codex
```

### 3. (Optional) Permanently configure a runtime

```bash
creavor config claude    # Writes proxy URL into Claude's settings
creavor config gemini    # Prints GEMINI_API_BASE export instruction
```

### 4. (Optional) Check broker status

```bash
creavor status
```

---

## Supported Runtimes

| Runtime | Provider Route | Config Method |
|---------|---------------|---------------|
| Claude Code | `/v1/anthropic` | `~/.claude/settings.json` (`apiBaseUrl`) |
| Gemini CLI | `/v1/gemini` | `GEMINI_API_BASE` env var |
| OpenCode | `/v1/openai` | `OPENAI_BASE_URL` env var |
| OpenClaw | `/v1/openai` | `OPENAI_BASE_URL` env var |
| Codex | `/v1/openai` | `OPENAI_BASE_URL` env var |
| Cline | `/v1/openai` | `OPENAI_BASE_URL` env var |

---

## Configuration

All config lives in `~/.opencreavor/settings.json`:

```json
{
  "broker": {
    "port": 8765,
    "log_level": "info",
    "block_status_code": 400,
    "block_error_style": "auto",
    "stream_passthrough": true,
    "upstream_timeout_secs": 300,
    "idle_stream_timeout_secs": 120
  },
  "upstream": {
    "claude": "https://api.anthropic.com"
  },
  "audit": {
    "event_auth_token": "env:CREAVOR_BROKER_EVENT_TOKEN",
    "store_request_payloads": false,
    "store_response_payloads": false
  },
  "rules": {
    "llm_analyzer_enabled": false
  }
}
```

See `apps/broker/config/settings.example.json` for a full example.

---

## Runtime Setup Docs

- `runtimes/claude-code/README.md`
- `runtimes/opencode/README.md`
- `runtimes/openclaw/README.md`
