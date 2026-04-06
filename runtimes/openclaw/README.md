# OpenClaw Runtime Setup

## 1) Configure OpenAI-compatible endpoint

Use env or config to point OpenClaw to broker:

```bash
export OPENAI_BASE_URL="http://127.0.0.1:8765/v1/openai"
```

Inject session id at startup (wrapper script):

```bash
export CREAVOR_SESSION_ID="openclaw:$(uuidgen | cut -d'-' -f1):$(date -u +%Y%m%dT%H%M)"
export OPENAI_CUSTOM_HEADERS="X-Creavor-Session-Id:${CREAVOR_SESSION_ID}"
```

## 2) Local events auth token

```bash
export CREAVOR_BROKER_EVENT_TOKEN="$(openssl rand -hex 32)"
```

Hook events must include:

```http
Authorization: Bearer <CREAVOR_BROKER_EVENT_TOKEN>
```

## Notes

- If custom headers are unsupported, broker will fallback to fuzzy correlation.
- Broker block responses are OpenAI-compatible by default with HTTP `400`.
- Stream behavior is controlled by broker config (`stream_passthrough`, `upstream_timeout`, `idle_stream_timeout`).
