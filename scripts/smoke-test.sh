#!/usr/bin/env bash
#
# RustyClaw E2E Smoke Test
# Builds release binary, boots gateway, validates health + chat round-trip.
# Exit 0 = all pass, non-zero = failure.
#
set -euo pipefail

# ── Colours & helpers ─────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
RESET='\033[0m'

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
SCRIPT_START=$(date +%s)
GATEWAY_PID=""
TMPDIR_PATH=""

info()  { printf "${BOLD}▸ %s${RESET}\n" "$*"; }
pass()  { printf "${GREEN}  ✓ %s${RESET}\n" "$*"; PASS_COUNT=$((PASS_COUNT + 1)); }
fail()  { printf "${RED}  ✗ %s${RESET}\n" "$*"; FAIL_COUNT=$((FAIL_COUNT + 1)); }
skip()  { printf "${YELLOW}  ⊘ %s${RESET}\n" "$*"; SKIP_COUNT=$((SKIP_COUNT + 1)); }
fatal() { printf "${RED}FATAL: %s${RESET}\n" "$*" >&2; cleanup; exit 1; }

cleanup() {
    if [[ -n "$GATEWAY_PID" ]] && kill -0 "$GATEWAY_PID" 2>/dev/null; then
        kill "$GATEWAY_PID" 2>/dev/null || true
        wait "$GATEWAY_PID" 2>/dev/null || true
    fi
    if [[ -n "$TMPDIR_PATH" && -d "$TMPDIR_PATH" ]]; then
        rm -rf "$TMPDIR_PATH"
    fi
}
trap cleanup EXIT INT TERM HUP

# ── 1. Build ──────────────────────────────────────────────────────
info "Building release binary..."
BUILD_START=$(date +%s)
if ! cargo build --release 2>&1; then
    fatal "cargo build --release failed"
fi
BUILD_END=$(date +%s)
pass "Release build succeeded ($((BUILD_END - BUILD_START))s)"

BINARY="./target/release/rustyclaw"
if [[ ! -x "$BINARY" ]]; then
    fatal "Binary not found at $BINARY"
fi

# ── 2. Detect provider keys ──────────────────────────────────────
# Source .env if present (project-local keys)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
if [[ -f "$PROJECT_DIR/.env" ]]; then
    set -a
    source "$PROJECT_DIR/.env"
    set +a
    info "Loaded .env from $PROJECT_DIR"
fi

PROVIDER_NAMES=()
PROVIDER_KEYS=()
PROVIDER_MODELS=()

if [[ -n "${OPENROUTER_API_KEY:-}" ]]; then
    PROVIDER_NAMES+=("openrouter")
    PROVIDER_KEYS+=("$OPENROUTER_API_KEY")
    PROVIDER_MODELS+=("anthropic/claude-sonnet-4")
fi
if [[ -n "${ANTHROPIC_API_KEY:-}" ]]; then
    PROVIDER_NAMES+=("anthropic")
    PROVIDER_KEYS+=("$ANTHROPIC_API_KEY")
    PROVIDER_MODELS+=("claude-sonnet-4-20250514")
elif [[ -n "${ANTHROPIC_OAUTH_TOKEN:-}" ]]; then
    PROVIDER_NAMES+=("anthropic")
    PROVIDER_KEYS+=("$ANTHROPIC_OAUTH_TOKEN")
    PROVIDER_MODELS+=("claude-sonnet-4-20250514")
fi

if [[ ${#PROVIDER_NAMES[@]} -eq 0 ]]; then
    fatal "No provider API keys found. Set OPENROUTER_API_KEY, ANTHROPIC_API_KEY, or ANTHROPIC_OAUTH_TOKEN."
fi

info "Detected ${#PROVIDER_NAMES[@]} provider(s): ${PROVIDER_NAMES[*]}"

# ── 3. Generate temp config ──────────────────────────────────────
TMPDIR_PATH=$(mktemp -d)
chmod 700 "$TMPDIR_PATH"
CONFIG_DIR="$TMPDIR_PATH/.rustyclaw"
mkdir -p "$CONFIG_DIR"

DEFAULT_PROVIDER="${PROVIDER_NAMES[0]}"
DEFAULT_KEY="${PROVIDER_KEYS[0]}"

cat > "$CONFIG_DIR/config.toml" << EOF
api_key = "$DEFAULT_KEY"
default_provider = "$DEFAULT_PROVIDER"
default_model = "${PROVIDER_MODELS[0]}"
default_temperature = 0.7

[gateway]
port = 0
host = "127.0.0.1"
require_pairing = false

[memory]
backend = "none"
auto_save = false
EOF
chmod 600 "$CONFIG_DIR/config.toml"

# ── 4. Boot gateway ──────────────────────────────────────────────
info "Starting gateway (random port, pairing disabled)..."

GATEWAY_LOG="$TMPDIR_PATH/gateway.log"
RUST_LOG=info RUSTYCLAW_CONFIG_DIR="$CONFIG_DIR" "$BINARY" gateway -p 0 > "$GATEWAY_LOG" 2>&1 &
GATEWAY_PID=$!

# Wait for gateway to print its listening address
PORT=""
for i in $(seq 1 30); do
    if ! kill -0 "$GATEWAY_PID" 2>/dev/null; then
        echo "--- Gateway log ---"
        cat "$GATEWAY_LOG"
        echo "---"
        fatal "Gateway process exited prematurely"
    fi
    PORT=$(grep -oE 'listening on http://[^:]+:([0-9]+)' "$GATEWAY_LOG" | tail -1 | grep -oE '[0-9]+$' || true)
    if [[ -n "$PORT" ]]; then
        break
    fi
    sleep 0.5
done

if [[ -z "$PORT" ]]; then
    echo "--- Gateway log ---"
    cat "$GATEWAY_LOG"
    echo "---"
    fatal "Gateway did not report a listening port within 15s"
fi

BASE_URL="http://127.0.0.1:${PORT}"
pass "Gateway listening on $BASE_URL (pid $GATEWAY_PID)"

# ── 5. Health check ──────────────────────────────────────────────
info "Health check: GET /health"

HTTP_CODE="000"
for attempt in $(seq 1 5); do
    HTTP_CODE=$(curl -s -o "$TMPDIR_PATH/health.json" -w '%{http_code}' "$BASE_URL/health" 2>/dev/null || echo "000")
    if [[ "$HTTP_CODE" == "200" ]]; then
        break
    fi
    sleep 1
done

if [[ "$HTTP_CODE" == "200" ]]; then
    STATUS=$(jq -r '.status' "$TMPDIR_PATH/health.json" 2>/dev/null || echo "")
    if [[ "$STATUS" == "ok" ]]; then
        pass "GET /health → 200, status=ok"
    else
        fail "GET /health → 200 but status='$STATUS' (expected 'ok')"
    fi
else
    fail "GET /health → HTTP $HTTP_CODE (expected 200)"
fi

# ── 6. Chat round-trip (default provider) ────────────────────────
run_chat_test() {
    local provider_name="$1"
    local provider_key="$2"
    local model="$3"
    local label="Chat round-trip ($provider_name / $model)"

    info "$label"

    # Reconfigure if not the default provider
    if [[ "$provider_name" != "$DEFAULT_PROVIDER" ]]; then
        # Update config for different provider
        cat > "$CONFIG_DIR/config.toml" << INNEREOF
api_key = "$provider_key"
default_provider = "$provider_name"
default_model = "$model"
default_temperature = 0.7

[gateway]
port = 0
host = "127.0.0.1"
require_pairing = false

[memory]
backend = "none"
auto_save = false
INNEREOF
        chmod 600 "$CONFIG_DIR/config.toml"

        # Restart gateway with new config (random port to avoid bind race)
        kill "$GATEWAY_PID" 2>/dev/null || true
        wait "$GATEWAY_PID" 2>/dev/null || true

        # Truncate log to avoid reading stale port
        > "$GATEWAY_LOG"

        RUST_LOG=info RUSTYCLAW_CONFIG_DIR="$CONFIG_DIR" "$BINARY" gateway -p 0 > "$GATEWAY_LOG" 2>&1 &
        GATEWAY_PID=$!

        # Re-capture port
        PORT=""
        for i in $(seq 1 30); do
            if ! kill -0 "$GATEWAY_PID" 2>/dev/null; then
                fail "$label — gateway failed to restart"
                return
            fi
            PORT=$(grep -oE 'listening on http://[^:]+:([0-9]+)' "$GATEWAY_LOG" | tail -1 | grep -oE '[0-9]+$' || true)
            if [[ -n "$PORT" ]]; then
                break
            fi
            sleep 0.5
        done
        if [[ -z "$PORT" ]]; then
            fail "$label — gateway did not report port after restart"
            return
        fi
        BASE_URL="http://127.0.0.1:${PORT}"
    fi

    local CHAT_RESPONSE
    CHAT_RESPONSE=$(curl -s --max-time 30 -X POST "$BASE_URL/webhook" \
        -H "Content-Type: application/json" \
        -d '{"message": "Say hello in exactly 3 words"}' 2>/dev/null || echo "")

    if [[ -z "$CHAT_RESPONSE" ]]; then
        fail "$label — empty or timed-out response"
        return
    fi

    local RESPONSE_TEXT
    RESPONSE_TEXT=$(echo "$CHAT_RESPONSE" | jq -r '.response // empty' 2>/dev/null || echo "")

    if [[ -z "$RESPONSE_TEXT" ]]; then
        local ERROR_TEXT
        ERROR_TEXT=$(echo "$CHAT_RESPONSE" | jq -r '.error // empty' 2>/dev/null || echo "")
        if [[ -n "$ERROR_TEXT" ]]; then
            fail "$label — error: $ERROR_TEXT"
        else
            fail "$label — no 'response' field in: ${CHAT_RESPONSE:0:200}"
        fi
        return
    fi

    pass "$label → \"${RESPONSE_TEXT:0:80}\""
}

# Test default provider
run_chat_test "$DEFAULT_PROVIDER" "$DEFAULT_KEY" "${PROVIDER_MODELS[0]}"

# Test additional providers (if any)
if [[ ${#PROVIDER_NAMES[@]} -gt 1 ]]; then
    for i in $(seq 1 $((${#PROVIDER_NAMES[@]} - 1))); do
        run_chat_test "${PROVIDER_NAMES[$i]}" "${PROVIDER_KEYS[$i]}" "${PROVIDER_MODELS[$i]}"
    done
fi

# ── 7. Summary ────────────────────────────────────────────────────
SCRIPT_END=$(date +%s)
ELAPSED=$((SCRIPT_END - SCRIPT_START))

echo ""
printf "${BOLD}━━━ Smoke Test Results ━━━${RESET}\n"
printf "${GREEN}  Passed:  %d${RESET}\n" "$PASS_COUNT"
if [[ $FAIL_COUNT -gt 0 ]]; then
    printf "${RED}  Failed:  %d${RESET}\n" "$FAIL_COUNT"
fi
if [[ $SKIP_COUNT -gt 0 ]]; then
    printf "${YELLOW}  Skipped: %d${RESET}\n" "$SKIP_COUNT"
fi
printf "  Time:    %ds\n" "$ELAPSED"
echo ""

if [[ $FAIL_COUNT -gt 0 ]]; then
    printf "${RED}${BOLD}SMOKE TEST FAILED${RESET}\n"
    exit 1
else
    printf "${GREEN}${BOLD}SMOKE TEST PASSED${RESET}\n"
    exit 0
fi
