# OpenClaw Runtime Setup

## Quick Start

```bash
# Launch OpenClaw through the broker
creavor run openclaw
```

`creavor run openclaw` will:
1. Read the current `OPENAI_BASE_URL` env var
2. Save the original URL to `~/.opencreavor/settings.json` as upstream
3. Set `OPENAI_BASE_URL` to the broker proxy URL
4. Launch `openclaw` with the broker as proxy

## Permanent Configuration

```bash
# Print the OPENAI_BASE_URL export command
creavor config openclaw
```

## Manual Setup (Advanced)

```bash
export OPENAI_BASE_URL="http://127.0.0.1:8765/v1/openai"
export CREAVOR_SESSION_ID="openclaw:$(uuidgen | cut -d'-' -f1):$(date -u +%Y%m%dT%H%M)"
openclaw
```

## Notes

- If custom headers are unsupported, broker will fallback to fuzzy correlation.
- Broker block responses are OpenAI-compatible by default with HTTP `400`.
- Stream controls: `stream_passthrough`, `upstream_timeout_secs`, `idle_stream_timeout_secs`.
