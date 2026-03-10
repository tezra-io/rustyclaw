#!/usr/bin/env bash
#
# Shared helpers for RustyClaw smoke test phases.
# Sourced by each phase script — not executed directly.
#

# ── Colours ─────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

# ── Counters (global, accumulated across phases) ────────────────
: "${PASS_COUNT:=0}"
: "${FAIL_COUNT:=0}"
: "${SKIP_COUNT:=0}"

# ── Output helpers ──────────────────────────────────────────────
info()  { printf "${BOLD}▸ %s${RESET}\n" "$*"; }
pass()  { printf "${GREEN}  ✓ %s${RESET}\n" "$*"; PASS_COUNT=$((PASS_COUNT + 1)); }
fail()  { printf "${RED}  ✗ %s${RESET}\n" "$*"; FAIL_COUNT=$((FAIL_COUNT + 1)); }
skip()  { printf "${YELLOW}  ⊘ %s${RESET}\n" "$*"; SKIP_COUNT=$((SKIP_COUNT + 1)); }
phase() { printf "\n${CYAN}${BOLD}━━━ Phase: %s ━━━${RESET}\n" "$*"; }

# ── HTTP helper ─────────────────────────────────────────────────
# Usage: http_request METHOD URL [DATA]
# Sets: HTTP_CODE, HTTP_BODY (in temp file at $HTTP_BODY_FILE)
HTTP_BODY_FILE=""

http_request() {
    local method="$1"
    local url="$2"
    local data="${3:-}"
    local timeout="${HTTP_TIMEOUT:-30}"

    HTTP_BODY_FILE=$(mktemp "${TMPDIR_PATH}/http_body.XXXXXX")

    local curl_args=(-s --max-time "$timeout" -o "$HTTP_BODY_FILE" -w '%{http_code}')

    case "$method" in
        GET)    curl_args+=(-X GET) ;;
        POST)   curl_args+=(-X POST -H "Content-Type: application/json") ;;
        PUT)    curl_args+=(-X PUT -H "Content-Type: application/json") ;;
        DELETE) curl_args+=(-X DELETE) ;;
    esac

    if [[ -n "$data" ]]; then
        curl_args+=(-d "$data")
    fi

    HTTP_CODE=$(curl "${curl_args[@]}" "$url" 2>/dev/null || echo "000")
}

http_body() {
    cat "$HTTP_BODY_FILE" 2>/dev/null || echo ""
}

http_json() {
    local field="$1"
    jq -r "$field" "$HTTP_BODY_FILE" 2>/dev/null || echo ""
}

# ── run_test ────────────────────────────────────────────────────
# Runs a test with inline gateway log analysis on failure.
# Usage: run_test "label" test_function
run_test() {
    local label="$1"
    local test_fn="$2"

    # Capture log position before test
    local log_lines_before=0
    if [[ -f "$GATEWAY_LOG" ]]; then
        log_lines_before=$(wc -l < "$GATEWAY_LOG" | tr -d ' ')
    fi

    # Run the test function (don't let set -e kill us)
    local result=0
    "$test_fn" || result=$?

    # On failure, analyze gateway logs for clues
    if [[ $result -ne 0 ]]; then
        analyze_logs "$label" "$log_lines_before"
    fi

    return 0
}

# ── Log analysis ────────────────────────────────────────────────
analyze_logs() {
    local label="$1"
    local since_line="$2"

    if [[ ! -f "$GATEWAY_LOG" ]]; then
        return
    fi

    local new_lines
    new_lines=$(tail -n +"$((since_line + 1))" "$GATEWAY_LOG" 2>/dev/null || true)

    if [[ -z "$new_lines" ]]; then
        return
    fi

    local category
    category=$(classify_failure "$new_lines")

    if [[ -n "$category" ]]; then
        printf "${RED}    ↳ Failure category: %s${RESET}\n" "$category"
    fi

    # Show relevant error lines (max 5)
    local error_lines
    error_lines=$(echo "$new_lines" | grep -iE 'error|panic|fatal|failed' | head -5)
    if [[ -n "$error_lines" ]]; then
        printf "${RED}    ↳ Log errors:${RESET}\n"
        echo "$error_lines" | while IFS= read -r line; do
            printf "${RED}      %s${RESET}\n" "${line:0:120}"
        done
    fi
}

# ── Failure classification ──────────────────────────────────────
classify_failure() {
    local log_text="$1"

    if echo "$log_text" | grep -qiE 'api.key|api_key|unauthorized|auth|token.*invalid'; then
        echo "PROVIDER"
    elif echo "$log_text" | grep -qiE 'config|toml|parse|deserialize|schema'; then
        echo "CONFIG"
    elif echo "$log_text" | grep -qiE 'connect|timeout|refused|dns|network|socket'; then
        echo "NETWORK"
    elif echo "$log_text" | grep -qiE 'panic|unwrap|index.*bounds|stack overflow'; then
        echo "CODE"
    else
        echo ""
    fi
}
