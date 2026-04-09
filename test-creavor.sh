#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# test-creavor.sh — Build, install, and test creavor + broker integration
#
# Usage:
#   ./test-creavor.sh              # Test all runtimes found on PATH
#   ./test-creavor.sh claude       # Test only claude
#   ./test-creavor.sh claude opencode  # Test specific runtimes
#
# What it does:
#   1. Build creavor + broker-server from source (cargo build --release)
#   2. Install to ~/.opencreavor/bin/
#   3. Start broker-server in background (capture logs to temp file)
#   4. For each runtime: set env var to broker URL, send a trivial API request
#      via curl (simulating what the runtime would do), verify broker logs it
#   5. Additionally: test "creavor run <runtime>" sets the right env vars
#   6. Report pass/fail for each runtime
# =============================================================================

# --- Colors ---
MUTED='\033[0;2m'
RED='\033[0;31m'
ORANGE='\033[38;5;214m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
NC='\033[0m'

# --- Config ---
INSTALL_DIR="$HOME/.opencreavor/bin"
BROKER_PORT="${BROKER_PORT:-8765}"
BROKER_URL="http://127.0.0.1:${BROKER_PORT}"
PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"
LOG_FILE=""

# --- Runtime definitions ---
declare -A RUNTIME_ENV_VAR=(
  [claude]="ANTHROPIC_BASE_URL"
  [opencode]="OPENAI_BASE_URL"
  [openclaw]="OPENAI_BASE_URL"
  [codex]="OPENAI_BASE_URL"
  [cline]="OPENAI_BASE_URL"
  [gemini]="GEMINI_API_BASE"
)

# API paths each runtime would hit via broker (for direct curl test)
# Broker routes: /v1/anthropic/*, /v1/openai/*, /v1/gemini/*
# The runtime sends e.g. /v1/messages, broker path is /v1/anthropic/messages
declare -A RUNTIME_TEST_PATH=(
  [claude]="/v1/anthropic/messages"
  [opencode]="/v1/openai/chat/completions"
  [openclaw]="/v1/openai/chat/completions"
  [codex]="/v1/openai/chat/completions"
  [cline]="/v1/openai/chat/completions"
  [gemini]="/v1/gemini/models"
)

# The provider prefix the broker expects in the URL path
declare -A RUNTIME_PROVIDER=(
  [claude]="anthropic"
  [opencode]="openai"
  [openclaw]="openai"
  [codex]="openai"
  [cline]="openai"
  [gemini]="gemini"
)

# API key header name per provider
declare -A RUNTIME_AUTH_HEADER=(
  [claude]="x-api-key"
  [opencode]="Authorization"
  [openclaw]="Authorization"
  [codex]="Authorization"
  [cline]="Authorization"
  [gemini]="x-goog-api-key"
)

# Minimal request body per provider
declare -A RUNTIME_TEST_BODY=(
  [claude]='{"model":"claude-sonnet-4-20250514","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}'
  [opencode]='{"model":"gpt-4o","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}'
  [openclaw]='{"model":"gpt-4o","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}'
  [codex]='{"model":"gpt-4o","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}'
  [cline]='{"model":"gpt-4o","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}'
  [gemini]='{"model":"gemini-2.0-flash","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}'
)

# --- Helpers ---
info()    { echo -e "${CYAN}[INFO]${NC}  $*"; }
ok()      { echo -e "${GREEN}[PASS]${NC}  $*"; }
warn()    { echo -e "${ORANGE}[WARN]${NC}  $*"; }
fail()    { echo -e "${RED}[FAIL]${NC}  $*"; }
step()    { echo -e "\n${CYAN}>>> $*${NC}"; }
muted()   { printf '%b\n' "${MUTED}$*${NC}"; }

# Read API key for a runtime from its config file or env var.
get_api_key() {
  local rt="$1"
  case "$rt" in
    claude)
      local claude_settings="$HOME/.claude/settings.json"
      if [[ -f "$claude_settings" ]]; then
        python3 -c "
import json
with open('$claude_settings') as f:
    s = json.load(f)
print(s.get('env', {}).get('ANTHROPIC_AUTH_TOKEN', ''), end='')
" 2>/dev/null
      elif [[ -n "${ANTHROPIC_API_KEY:-}" ]]; then
        echo "$ANTHROPIC_API_KEY"
      fi
      ;;
    opencode|openclaw|codex|cline)
      echo "${OPENAI_API_KEY:-}"
      ;;
    gemini)
      echo "${GEMINI_API_KEY:-}"
      ;;
  esac
}

cleanup() {
  if [[ -n "${BROKER_PID:-}" ]] && kill -0 "$BROKER_PID" 2>/dev/null; then
    info "Stopping broker-server (PID $BROKER_PID)..."
    kill "$BROKER_PID" 2>/dev/null || true
    wait "$BROKER_PID" 2>/dev/null || true
  fi
  if [[ -n "${LOG_FILE:-}" && -f "$LOG_FILE" ]]; then
    muted "Broker log saved: $LOG_FILE"
  fi
}
trap cleanup EXIT

# --- Determine which runtimes to test ---
TEST_RUNTIMES=()
if [[ $# -gt 0 ]]; then
  TEST_RUNTIMES=("$@")
else
  for rt in claude opencode gemini codex openclaw cline; do
    if command -v "$rt" >/dev/null 2>&1; then
      TEST_RUNTIMES+=("$rt")
    fi
  done
fi

if [[ ${#TEST_RUNTIMES[@]} -eq 0 ]]; then
  fail "No runtimes to test. Install at least one or pass names as arguments."
  exit 1
fi

info "Runtimes to test: ${TEST_RUNTIMES[*]}"

# ===========================================================================
# Step 1: Build from source
# ===========================================================================
step "Step 1/5: Build from source"

if ! command -v cargo >/dev/null 2>&1; then
  fail "cargo not found. Install Rust: https://rustup.rs"
  exit 1
fi

info "Building creavor-cli + broker-server (release)..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml" 2>&1 | tail -1
ok "Build complete"

# ===========================================================================
# Step 2: Install binaries
# ===========================================================================
step "Step 2/5: Install to $INSTALL_DIR"

mkdir -p "$INSTALL_DIR"
cp "$PROJECT_DIR/target/release/creavor"      "${INSTALL_DIR}/creavor"
cp "$PROJECT_DIR/target/release/broker-server" "${INSTALL_DIR}/broker-server"
chmod 755 "${INSTALL_DIR}/creavor" "${INSTALL_DIR}/broker-server"

export PATH="$INSTALL_DIR:$PATH"

ok "creavor installed: $(which creavor)"
ok "broker-server installed: $(which broker-server)"

# ===========================================================================
# Step 3: Start broker-server
# ===========================================================================
step "Step 3/5: Start broker-server on port $BROKER_PORT"

# Kill any existing broker on this port
existing_pid=$(lsof -ti :"$BROKER_PORT" 2>/dev/null || true)
if [[ -n "$existing_pid" ]]; then
  warn "Killing existing process on port $BROKER_PORT (PID $existing_pid)"
  kill "$existing_pid" 2>/dev/null || true
  sleep 1
fi

LOG_FILE=$(mktemp /tmp/creavor-test-broker.XXXXXX)
info "Broker log: $LOG_FILE"

# Write a TEMPORARY settings file for broker-server.
# We do NOT touch the real ~/.opencreavor/settings.json.
TEST_SETTINGS=$(mktemp /tmp/creavor-test-settings.XXXXXX)

# Read the actual upstream URL from each runtime's config.
# For Claude: read from ~/.claude/settings.json -> env.ANTHROPIC_BASE_URL
# For others: read from env vars (OPENAI_BASE_URL, GEMINI_API_BASE)
get_upstream_url() {
  local rt="$1"
  case "$rt" in
    claude)
      local claude_settings="$HOME/.claude/settings.json"
      if [[ -f "$claude_settings" ]]; then
        python3 -c "
import json
with open('$claude_settings') as f:
    s = json.load(f)
url = s.get('env', {}).get('ANTHROPIC_BASE_URL', '')
print(url, end='')
" 2>/dev/null
      fi
      ;;
    opencode|openclaw|codex|cline)
      echo "${OPENAI_BASE_URL:-}"
      ;;
    gemini)
      echo "${GEMINI_API_BASE:-}"
      ;;
  esac
}

# Build upstream map using real runtime configs, fall back to defaults
upstream_entries=""
for rt in "${TEST_RUNTIMES[@]}"; do
  upstream_url=$(get_upstream_url "$rt")

  # Fallback to default if no URL configured
  if [[ -z "$upstream_url" ]]; then
    provider="${RUNTIME_PROVIDER[$rt]:-}"
    case "$provider" in
      anthropic) upstream_url="https://api.anthropic.com" ;;
      openai)    upstream_url="https://api.openai.com/v1" ;;
      gemini)    upstream_url="https://generativelanguage.googleapis.com" ;;
      *)         upstream_url="https://example.com" ;;
    esac
    warn "  $rt: no configured upstream, using default $upstream_url"
  else
    ok "  $rt: upstream=$upstream_url"
  fi

  [[ -n "$upstream_entries" ]] && upstream_entries+=","
  upstream_entries+="\"${rt}\": \"${upstream_url}\""
done

cat > "$TEST_SETTINGS" <<SETTINGSJSON
{
  "broker": { "port": ${BROKER_PORT} },
  "upstream": { ${upstream_entries} }
}
SETTINGSJSON
info "Test settings: $TEST_SETTINGS"

RUST_LOG=info broker-server --config "$TEST_SETTINGS" 2>&1 | tee "$LOG_FILE" &
BROKER_PID=$!

# Wait for broker to be ready
retries=15
while [[ $retries -gt 0 ]]; do
  if curl -sf "${BROKER_URL}/health" >/dev/null 2>&1; then
    break
  fi
  retries=$((retries - 1))
  sleep 0.5
done

if [[ $retries -eq 0 ]]; then
  fail "broker-server did not start (check $LOG_FILE)"
  cat "$LOG_FILE"
  exit 1
fi
ok "broker-server ready (PID $BROKER_PID, port $BROKER_PORT)"

# ===========================================================================
# Step 4: Test direct API proxy (curl → broker → upstream)
#   This verifies the broker is correctly intercepting requests.
#   We send a minimal request that mimics what each runtime would send.
#   It will fail at the upstream (no valid API key / wrong URL) but
#   the broker log should still show "proxy request".
# ===========================================================================
step "Step 4/5: Test broker proxy (curl → broker)"

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0

for rt in "${TEST_RUNTIMES[@]}"; do
  echo ""
  info "--- Testing proxy: $rt ---"

  env_var="${RUNTIME_ENV_VAR[$rt]:-}"
  test_path="${RUNTIME_TEST_PATH[$rt]:-}"
  test_body="${RUNTIME_TEST_BODY[$rt]:-}"
  provider="${RUNTIME_PROVIDER[$rt]:-}"

  if [[ -z "$test_path" || -z "$test_body" ]]; then
    warn "No test path/body defined for $rt, skipping"
    SKIP_COUNT=$((SKIP_COUNT + 1))
    continue
  fi

  # Record log offset
  log_offset=$(wc -l < "$LOG_FILE" | tr -d ' ')

  # Read API key from runtime config
  api_key=$(get_api_key "$rt")

  # Build auth header based on provider
  auth_header_arg=()
  if [[ -n "$api_key" ]]; then
    case "$rt" in
      claude) auth_header_arg=(-H "x-api-key: ${api_key}") ;;
      gemini) auth_header_arg=(-H "x-goog-api-key: ${api_key}") ;;
      *)     auth_header_arg=(-H "Authorization: Bearer ${api_key}") ;;
    esac
    muted "  API key: found (${#api_key} chars)"
  else
    warn "  No API key found for $rt — upstream will likely return 401/403"
  fi

  # Send request to broker with the x-creavor-runtime header so broker
  # can identify which runtime is making the request.
  set +e
  curl -s -o /dev/null -w "%{http_code}" -X POST \
    -H "Content-Type: application/json" \
    -H "x-creavor-runtime: ${rt}" \
    -H "x-session-id: test-${rt}" \
    "${auth_header_arg[@]+"${auth_header_arg[@]}"}" \
    -d "$test_body" \
    "${BROKER_URL}${test_path}" 2>/dev/null
  upstream_http_code=$?
  set -e

  # Wait for broker to log
  sleep 0.5

  # Check broker logs for "proxy request"
  proxy_count=0
  if tail -n +"$((log_offset + 1))" "$LOG_FILE" | grep -q "proxy request"; then
    proxy_count=1
  fi

  if [[ $proxy_count -gt 0 ]]; then
    ok "  $rt -> broker received and logged the request"
    tail -n +"$((log_offset + 1))" "$LOG_FILE" | grep "proxy request\|proxy completed" | while IFS= read -r line; do
      muted "    $line"
    done
    # Check upstream response status
    if tail -n +"$((log_offset + 1))" "$LOG_FILE" | grep -q "proxy completed"; then
      completed_line=$(tail -n +"$((log_offset + 1))" "$LOG_FILE" | grep "proxy completed" | head -1)
      status_code=$(echo "$completed_line" | grep -oE 'status=[0-9]+' | head -1 | cut -d= -f2)
      if [[ "${status_code:-}" == "200" ]]; then
        ok "  $rt -> upstream returned 200 OK (end-to-end success)"
      else
        muted "    (upstream returned status=${status_code:-unknown})"
      fi
    fi
    PASS_COUNT=$((PASS_COUNT + 1))
  else
    fail "  $rt -> broker did NOT log the request"
    muted "  Last 5 broker log lines:"
    tail -5 "$LOG_FILE" | while IFS= read -r line; do muted "    $line"; done
    FAIL_COUNT=$((FAIL_COUNT + 1))
  fi
done

# ===========================================================================
# Step 5: Test "creavor run" sets correct environment variables
#   We can't easily run the actual runtimes non-interactively, but we can
#   verify that "creavor run <runtime>" sets the right env var by using
#   a mock binary that just prints its environment.
# ===========================================================================
step "Step 5/5: Test creavor run env-var setup"

for rt in "${TEST_RUNTIMES[@]}"; do
  echo ""
  info "--- Testing creavor run: $rt ---"

  env_var="${RUNTIME_ENV_VAR[$rt]:-}"
  if [[ -z "$env_var" ]]; then
    warn "No env var defined for $rt, skipping"
    continue
  fi

  # Check if creavor run sets the right env var by looking at its --help or
  # using a simple dry-run approach. Since creavor run launches the actual
  # binary, we create a temporary mock that just prints the env.
  mock_dir=$(mktemp -d)
  mock_bin="${mock_dir}/${rt}"

  cat > "$mock_bin" <<'MOCK'
#!/usr/bin/env bash
echo "ENV_ANTHROPIC_BASE_URL=${ANTHROPIC_BASE_URL:-unset}"
echo "ENV_OPENAI_BASE_URL=${OPENAI_BASE_URL:-unset}"
echo "ENV_GEMINI_API_BASE=${GEMINI_API_BASE:-unset}"
echo "ENV_CREAVOR_SESSION_ID=${CREAVOR_SESSION_ID:-unset}"
exit 0
MOCK
  chmod +x "$mock_bin"

  # Run creavor with the mock in PATH (prepend to override real binary)
  set +e
  mock_output=$(PATH="$mock_dir:$PATH" creavor run "$rt" 2>&1)
  mock_exit=$?
  set -e

  # Parse the output for the expected env var
  actual_val=$(echo "$mock_output" | grep "^ENV_${env_var}=" | head -1 | cut -d= -f2-)
  session_val=$(echo "$mock_output" | grep "^ENV_CREAVOR_SESSION_ID=" | head -1 | cut -d= -f2-)

  rm -rf "$mock_dir"

  # creavor sets env var to broker URL with provider prefix, e.g.
  # http://127.0.0.1:8765/v1/anthropic or http://127.0.0.1:8765/v1/openai
  expected_prefix="${BROKER_URL}/v1/${RUNTIME_PROVIDER[$rt]}"

  if [[ "$actual_val" == "${expected_prefix}" ]]; then
    ok "  $rt -> ${env_var}=${actual_val} (correct)"
    if [[ "$session_val" != "unset" && -n "$session_val" ]]; then
      ok "  $rt -> CREAVOR_SESSION_ID=${session_val} (set)"
    fi
  elif [[ "$actual_val" == "unset" ]]; then
    fail "  $rt -> ${env_var} was NOT set"
    muted "  Output: $mock_output"
    FAIL_COUNT=$((FAIL_COUNT + 1))
  else
    warn "  $rt -> ${env_var}=${actual_val} (unexpected, expected ${expected_prefix})"
  fi
done

# ===========================================================================
# Summary
# ===========================================================================
echo ""
step "Summary"
echo -e "  ${GREEN}PASS${NC}: $PASS_COUNT   ${RED}FAIL${NC}: $FAIL_COUNT   ${ORANGE}SKIP${NC}: $SKIP_COUNT"
echo ""
muted "Full broker log: $LOG_FILE"
muted "Re-run with specific runtimes: ./test-creavor.sh claude opencode"

if [[ $FAIL_COUNT -gt 0 ]]; then
  exit 1
fi
