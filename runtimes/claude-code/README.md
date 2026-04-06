# Claude Code Runtime Setup

Use a startup wrapper so session metadata is injected at process start.
Do not rely on hook scripts to mutate parent shell environment.

## 1) Set broker endpoint and session header

```bash
export CREAVOR_SESSION_ID="claude-code:$(uuidgen | cut -d'-' -f1):$(date -u +%Y%m%dT%H%M)"
export ANTHROPIC_BASE_URL="http://127.0.0.1:8765/v1/anthropic"
export ANTHROPIC_CUSTOM_HEADERS="X-Creavor-Session-Id:${CREAVOR_SESSION_ID}"
```

## 2) Start Claude Code from the same shell

```bash
claude
```

## 3) Configure local event auth token for hooks

```bash
export CREAVOR_BROKER_EVENT_TOKEN="$(openssl rand -hex 32)"
```

When posting to `POST /api/v1/events`, include:

```http
Authorization: Bearer <CREAVOR_BROKER_EVENT_TOKEN>
```

## Notes

- Broker strips `X-Creavor-Session-Id` before forwarding upstream.
- Block responses are provider-compatible and default to HTTP `400`.
- Stream controls are configured by broker settings: `stream_passthrough`, `upstream_timeout`, `idle_stream_timeout`.
