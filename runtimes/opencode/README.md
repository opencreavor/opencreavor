# OpenCode Runtime Setup

## 1) Configure provider endpoint

```toml
[provider]
base_url = "http://127.0.0.1:8765/v1/openai"
custom_headers = { "X-Creavor-Session-Id" = "opencode:${SESSION_ID}" }
```

Generate session id at process startup (wrapper or launcher script):

```bash
export SESSION_ID="$(uuidgen | cut -d'-' -f1):$(date -u +%Y%m%dT%H%M)"
```

## 2) Local events auth token

```bash
export CREAVOR_BROKER_EVENT_TOKEN="$(openssl rand -hex 32)"
```

Send hook events with:

```http
Authorization: Bearer <CREAVOR_BROKER_EVENT_TOKEN>
```

## Notes

- Broker returns OpenAI-compatible block envelope by default with HTTP `400`.
- Keep `X-Creavor-Session-Id` set for high-confidence correlation.
- Tune stream behavior in broker config: `stream_passthrough`, `upstream_timeout`, `idle_stream_timeout`.
