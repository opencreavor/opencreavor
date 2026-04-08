# Gemini CLI Runtime Setup

## Quick Start

```bash
# Launch Gemini CLI through the broker
creavor run gemini
```

`creavor run gemini` will:
1. Read the current `GEMINI_API_BASE` env var
2. Save the original URL to `~/.opencreavor/settings.json` as upstream
3. Set `GEMINI_API_BASE` to the broker proxy URL
4. Launch `gemini` with the broker as proxy

## Permanent Configuration

```bash
# Print the GEMINI_API_BASE export command
creavor config gemini
```

## Manual Setup (Advanced)

```bash
export GEMINI_API_BASE="http://127.0.0.1:8765/v1/gemini"
export CREAVOR_SESSION_ID="gemini:$(uuidgen | cut -d'-' -f1):$(date -u +%Y%m%dT%H%M)"
gemini
```

## Notes

- Broker returns Gemini-compatible block response with `content_policy_violation` code.
- Stream controls: `stream_passthrough`, `upstream_timeout_secs`, `idle_stream_timeout_secs`.
