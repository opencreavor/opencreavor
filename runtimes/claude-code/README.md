# Claude Code Runtime Setup

## Quick Start

```bash
# Launch Claude Code through the broker
creavor run claude
```

`creavor run claude` will:
1. Read Claude's current `apiBaseUrl` from `~/.claude/settings.json`
2. Save the original URL to `~/.opencreavor/settings.json` as upstream
3. Set `ANTHROPIC_BASE_URL` and `ANTHROPIC_CUSTOM_HEADERS` env vars
4. Launch `claude` with the broker as proxy

## Permanent Configuration

```bash
# Write broker URL into Claude's settings permanently
creavor config claude
```

After this, Claude will use the broker even when launched directly (without `creavor run`).

## Manual Setup (Advanced)

If you prefer to configure manually:

```bash
export ANTHROPIC_BASE_URL="http://127.0.0.1:8765/v1/anthropic"
export ANTHROPIC_CUSTOM_HEADERS="X-Creavor-Session-Id:claude:$(uuidgen | cut -d'-' -f1):$(date -u +%Y%m%dT%H%M)"
claude
```

## Notes

- Broker strips `X-Creavor-Session-Id` before forwarding upstream.
- Block responses use Anthropic-compatible envelope with HTTP `400`.
- Stream controls: `stream_passthrough`, `upstream_timeout_secs`, `idle_stream_timeout_secs` in `~/.opencreavor/settings.json`.
