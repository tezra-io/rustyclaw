#!/usr/bin/env bash
#
# RustyClaw E2E Smoke Test Suite
# Builds release binary, boots gateway, runs phased feature verification.
# Exit 0 = all pass, non-zero = failure.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
SMOKE_DIR="$SCRIPT_DIR/smoke"

# ── Bootstrap shared helpers ────────────────────────────────────
source "$SMOKE_DIR/lib.sh"

SCRIPT_START=$(date +%s)
GATEWAY_PID=""
TMPDIR_PATH=""

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

fatal() { printf "${RED}FATAL: %s${RESET}\n" "$*" >&2; cleanup; exit 1; }

# ── 1. Build ──────────────────────────────────────────────────────
phase "Build"
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
if [[ -n "${OPENAI_API_KEY:-}" ]]; then
    PROVIDER_NAMES+=("openai")
    PROVIDER_KEYS+=("$OPENAI_API_KEY")
    PROVIDER_MODELS+=("gpt-4o-mini")
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
phase "Gateway Boot"
info "Starting gateway (random port, pairing disabled)..."

GATEWAY_LOG="$TMPDIR_PATH/gateway.log"
RUST_LOG=info RUSTYCLAW_CONFIG_DIR="$CONFIG_DIR" "$BINARY" gateway -p 0 > "$GATEWAY_LOG" 2>&1 &
GATEWAY_PID=$!

PORT=""
for _ in $(seq 1 30); do
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

GATEWAY_URL="http://127.0.0.1:${PORT}"
pass "Gateway listening on $GATEWAY_URL (pid $GATEWAY_PID)"

# ── 5. Export env for phase scripts ──────────────────────────────
export GATEWAY_URL GATEWAY_LOG GATEWAY_PID TMPDIR_PATH CONFIG_DIR BINARY
export PROVIDER_NAMES PROVIDER_KEYS PROVIDER_MODELS
export DEFAULT_PROVIDER DEFAULT_KEY
export PASS_COUNT FAIL_COUNT SKIP_COUNT

# ── 6. Run phase scripts sequentially ────────────────────────────
PHASE_RESULTS=()

run_phase() {
    local phase_script="$1"
    local phase_name
    phase_name=$(basename "$phase_script" .sh)

    local phase_pass_before=$PASS_COUNT
    local phase_fail_before=$FAIL_COUNT
    local phase_skip_before=$SKIP_COUNT

    # Source the phase script so counters are shared
    source "$phase_script" || true

    local p=$((PASS_COUNT - phase_pass_before))
    local f=$((FAIL_COUNT - phase_fail_before))
    local s=$((SKIP_COUNT - phase_skip_before))

    PHASE_RESULTS+=("$phase_name: ${p} passed, ${f} failed, ${s} skipped")
}

for phase_script in "$SMOKE_DIR"/[0-9]*.sh; do
    if [[ -f "$phase_script" ]]; then
        run_phase "$phase_script"
    fi
done

# ── 7. Summary ────────────────────────────────────────────────────
SCRIPT_END=$(date +%s)
ELAPSED=$((SCRIPT_END - SCRIPT_START))

echo ""
printf "${BOLD}━━━ Smoke Test Results ━━━${RESET}\n"
echo ""

# Per-phase breakdown
for result in "${PHASE_RESULTS[@]}"; do
    printf "  %s\n" "$result"
done
echo ""

# Totals
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
