# Cline Runtime Setup

## Quick Start

```bash
# Launch Cline through the broker
creavor run cline
```

`creavor run cline` will:
1. Read the current `OPENAI_BASE_URL` env var
2. Save the original URL to `~/.opencreavor/settings.json` as upstream
3. Set `OPENAI_BASE_URL` to the broker proxy URL
4. Launch `cline` with the broker as proxy

## Permanent Configuration

```bash
# Print the OPENAI_BASE_URL export command
creavor config cline
```

## Manual Setup (Advanced)

```bash
export OPENAI_BASE_URL="http://127.0.0.1:8765/v1/openai"
export CREAVOR_SESSION_ID="cline:$(uuidgen | cut -d'-' -f1):$(date -u +%Y%m%dT%H%M)"
cline
```

## Notes

- Broker returns OpenAI-compatible block envelope by default with HTTP `400`.
- Stream controls: `stream_passthrough`, `upstream_timeout_secs`, `idle_stream_timeout_secs`.
