#!/usr/bin/env bash
#
# RustyClaw E2E Test Runner
# Builds binary, sets up test env, runs test cases, generates report.
#
# Usage:
#   ./run_e2e.sh                 # Run all test suites
#   ./run_e2e.sh core            # Run specific suite(s)
#   ./run_e2e.sh --list          # List available suites
#   ./run_e2e.sh --clean         # Clean up only
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
CASES_DIR="$SCRIPT_DIR/cases"
CONFIGS_DIR="$SCRIPT_DIR/configs"
REPORTS_DIR="$SCRIPT_DIR/reports"
LOGS_DIR="$SCRIPT_DIR/logs"

# ── Colors ──────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

# ── State ───────────────────────────────────────────────────────
PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
ANOMALY_COUNT=0
TOTAL_COUNT=0
GATEWAY_PID=""
E2E_WORKSPACE=""
BINARY=""
SESSION_START=""
DATE_STAMP=""
REPORT_FILE=""
FAILED_TESTS=()
ANOMALY_TESTS=()

# ── Helpers ─────────────────────────────────────────────────────
info()    { printf "${BOLD}▸ %s${RESET}\n" "$*"; }
pass()    { printf "${GREEN}  ✓ %s${RESET}\n" "$*"; PASS_COUNT=$((PASS_COUNT + 1)); TOTAL_COUNT=$((TOTAL_COUNT + 1)); }
fail()    { printf "${RED}  ✗ %s${RESET}\n" "$*"; FAIL_COUNT=$((FAIL_COUNT + 1)); TOTAL_COUNT=$((TOTAL_COUNT + 1)); FAILED_TESTS+=("$*"); }
skip()    { printf "${YELLOW}  ⊘ %s${RESET}\n" "$*"; SKIP_COUNT=$((SKIP_COUNT + 1)); TOTAL_COUNT=$((TOTAL_COUNT + 1)); }
anomaly() { printf "${YELLOW}  ⚠ %s${RESET}\n" "$*"; ANOMALY_COUNT=$((ANOMALY_COUNT + 1)); TOTAL_COUNT=$((TOTAL_COUNT + 1)); ANOMALY_TESTS+=("$*"); }
phase()   { printf "\n${CYAN}${BOLD}━━━ Phase: %s ━━━${RESET}\n" "$*"; }
fatal()   { printf "${RED}FATAL: %s${RESET}\n" "$*" >&2; cleanup; exit 1; }
# Subshell-safe stubs (exported via declare -f into bash -c test subshells)
_sub_skip() { echo "SKIP: $*"; }
_sub_pass() { echo "PASS: $*"; }
_sub_fail() { echo "FAIL: $*"; }


# ── Cleanup ─────────────────────────────────────────────────────
cleanup() {
    if [[ -n "$GATEWAY_PID" ]] && kill -0 "$GATEWAY_PID" 2>/dev/null; then
        kill "$GATEWAY_PID" 2>/dev/null || true
        wait "$GATEWAY_PID" 2>/dev/null || true
    fi
    if [[ -n "$E2E_WORKSPACE" && -d "$E2E_WORKSPACE" ]]; then
        rm -rf "$E2E_WORKSPACE"
    fi
}
trap cleanup EXIT INT TERM HUP

# ── Environment ─────────────────────────────────────────────────
setup_env() {
    # Source secrets
    source ~/.secrets 2>/dev/null || true

    # Load .env.e2e if exists
    if [[ -f "$SCRIPT_DIR/.env.e2e.local" ]]; then
        set -a; source "$SCRIPT_DIR/.env.e2e.local"; set +a
    elif [[ -f "$SCRIPT_DIR/.env.e2e" ]]; then
        set -a; source "$SCRIPT_DIR/.env.e2e"; set +a
    fi

    # Load project .env
    if [[ -f "$PROJECT_DIR/.env" ]]; then
        set -a; source "$PROJECT_DIR/.env"; set +a
    fi

    export RUST_LOG="${RUST_LOG:-debug}"
    export E2E_DEFAULT_TIMEOUT="${E2E_DEFAULT_TIMEOUT:-60}"
    export E2E_LONG_TIMEOUT="${E2E_LONG_TIMEOUT:-300}"

    # Create workspace
    E2E_WORKSPACE=$(mktemp -d "${TMPDIR:-/tmp}/rustyclaw_e2e.XXXXXX")
    export E2E_WORKSPACE
    mkdir -p "$E2E_WORKSPACE"/{agents,memory,config,logs}

    # Setup dates
    DATE_STAMP=$(date '+%Y-%m-%d')
    SESSION_START=$(date +%s)
    REPORT_FILE="$REPORTS_DIR/${DATE_STAMP}.md"

    # Setup logging
    mkdir -p "$LOGS_DIR" "$REPORTS_DIR"

    # Verify at least one provider key
    if [[ -z "${ANTHROPIC_OAUTH_TOKEN:-}" && -z "${ANTHROPIC_API_KEY:-}" && -z "${OPENROUTER_API_KEY:-}" ]]; then
        fatal "No LLM provider credentials found. Set ANTHROPIC_OAUTH_TOKEN or OPENROUTER_API_KEY."
    fi

    info "Workspace: $E2E_WORKSPACE"
    info "Date: $DATE_STAMP"
}

# ── Build ───────────────────────────────────────────────────────
build_binary() {
    phase "Build"
    info "Building release binary..."
    local build_start=$(date +%s)

    if ! cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml" 2>&1; then
        fatal "cargo build --release failed"
    fi

    local build_end=$(date +%s)
    BINARY="$PROJECT_DIR/target/release/rustyclaw"

    if [[ ! -x "$BINARY" ]]; then
        fatal "Binary not found at $BINARY"
    fi

    pass "Release build succeeded ($((build_end - build_start))s)"
}

# ── Gateway management ──────────────────────────────────────────
start_gateway() {
    local config="${1:-basic}"
    local config_file="$CONFIGS_DIR/${config}.toml"
    local port="${2:-0}"  # 0 = random
    local log_file="$E2E_WORKSPACE/logs/gateway-${config}.log"

    # Stop existing gateway if running
    stop_gateway

    if [[ ! -f "$config_file" ]]; then
        fail "Config not found: $config_file"
        return 1
    fi

    # Prepare config directory (binary expects --config-dir with config.toml inside)
    local config_dir="$E2E_WORKSPACE/config-${config}"
    mkdir -p "$config_dir"
    cp "$config_file" "$config_dir/config.toml"

    # Start gateway with test config
    RUST_LOG=debug "$BINARY" gateway \
        --config-dir "$config_dir" \
        -p "$port" \
        > "$log_file" 2>&1 &
    GATEWAY_PID=$!

    # Wait for gateway to be ready (extract port from log output)
    local attempts=0
    local max_attempts=40
    while [[ $attempts -lt $max_attempts ]]; do
        sleep 0.3
        attempts=$((attempts + 1))
        if [[ -f "$log_file" ]]; then
            local detected_port
            detected_port=$(grep -oE 'listening on.*:([0-9]+)' "$log_file" | grep -oE '[0-9]+$' | head -1)
            if [[ -n "$detected_port" ]]; then
                GATEWAY_PORT="$detected_port"
                export GATEWAY_URL="http://127.0.0.1:${GATEWAY_PORT}"
                if curl -sf "${GATEWAY_URL}/health" >/dev/null 2>&1; then
                    pass "Gateway listening on ${GATEWAY_URL} (pid $GATEWAY_PID)"
                    return 0
                fi
            fi
        fi
        # Check if process died
        if ! kill -0 "$GATEWAY_PID" 2>/dev/null; then
            fail "Gateway process died during startup"
            if [[ -f "$log_file" ]]; then
                tail -5 "$log_file" >&2
            fi
            return 1
        fi
    done

    fail "Gateway failed to start within $((max_attempts * 3 / 10))s"
    return 1
}

stop_gateway() {
    if [[ -n "$GATEWAY_PID" ]] && kill -0 "$GATEWAY_PID" 2>/dev/null; then
        kill "$GATEWAY_PID" 2>/dev/null || true
        wait "$GATEWAY_PID" 2>/dev/null || true
        GATEWAY_PID=""
    fi
}

# ── Portable timeout ───────────────────────────────────────────
# macOS lacks coreutils timeout; use perl fallback
_timeout() {
    local secs="$1"; shift
    if command -v gtimeout >/dev/null 2>&1; then
        gtimeout "$secs" "$@"
    elif command -v timeout >/dev/null 2>&1; then
        timeout "$secs" "$@"
    else
        # Perl-based timeout fallback for macOS
        perl -e '
            alarm shift @ARGV;
            $SIG{ALRM} = sub { kill 9, $pid; exit 124 };
            $pid = fork // die;
            if ($pid == 0) { exec @ARGV; die "exec: $!" }
            waitpid($pid, 0);
            exit($? >> 8);
        ' "$secs" "$@"
    fi
}

# ── Test execution ──────────────────────────────────────────────
# Run a single test case with timeout and logging
# Usage: run_test "TC-1.1" "description" test_function [timeout_seconds]
run_test() {
    local tc_id="$1"
    local desc="$2"
    local test_fn="$3"
    local test_timeout="${4:-$E2E_DEFAULT_TIMEOUT}"
    local log_file="$E2E_WORKSPACE/logs/${tc_id}.log"

    # Run test function in a subshell with timeout
    local result=0
    local start_time=$(date +%s)

    # Export all variables the test functions need
    export BINARY CONFIGS_DIR PROJECT_DIR E2E_WORKSPACE GATEWAY_URL GATEWAY_PORT GATEWAY_PID

    if _timeout "$test_timeout" bash -c "$(declare -f _sub_skip _sub_pass _sub_fail "$test_fn" start_gateway_raw start_gateway stop_gateway _mem_store _mem_has_key _mem_delete 2>/dev/null)"'
skip() { _sub_skip "$@"; }
pass() { _sub_pass "$@"; }
fail() { _sub_fail "$@"; }
'"$test_fn" > "$log_file" 2>&1; then
        result=0
    else
        result=$?
    fi

    local end_time=$(date +%s)
    local duration=$((end_time - start_time))

    if [[ $result -eq 0 ]]; then
        pass "${tc_id}: ${desc} (${duration}s)"
    elif [[ $result -eq 124 ]]; then
        fail "${tc_id}: ${desc} — TIMEOUT after ${test_timeout}s"
    else
        fail "${tc_id}: ${desc} — exit code ${result}"
        # Show last few lines of log
        if [[ -f "$log_file" ]]; then
            printf "${RED}    ↳ Log tail:${RESET}\n"
            tail -3 "$log_file" | while IFS= read -r line; do
                printf "${RED}      %s${RESET}\n" "${line:0:120}"
            done
        fi
    fi
}

# ── Helper: start a raw gateway (for tests that manage their own) ──
# Usage: start_gateway_raw <config_toml_path> <log_file>
# Sets: _GW_PID, _GW_PORT, _GW_URL
start_gateway_raw() {
    local config_file="$1"
    local log_file="$2"
    local config_dir
    config_dir=$(mktemp -d "${E2E_WORKSPACE}/config-raw.XXXXXX")
    cp "$config_file" "$config_dir/config.toml"
    mkdir -p "$(dirname "$log_file")"

    RUST_LOG=debug "$BINARY" gateway \
        --config-dir "$config_dir" \
        -p 0 \
        > "$log_file" 2>&1 &
    _GW_PID=$!

    local attempts=0
    while [[ $attempts -lt 40 ]]; do
        sleep 0.3
        attempts=$((attempts + 1))
        if [[ -f "$log_file" ]]; then
            _GW_PORT=$(grep -oE 'listening on.*:([0-9]+)' "$log_file" | grep -oE '[0-9]+$' | head -1)
            if [[ -n "$_GW_PORT" ]]; then
                _GW_URL="http://127.0.0.1:${_GW_PORT}"
                return 0
            fi
        fi
        if ! kill -0 "$_GW_PID" 2>/dev/null; then
            return 1
        fi
    done
    return 1
}

# ── Test case discovery ─────────────────────────────────────────
list_suites() {
    info "Available test suites:"
    for suite_file in "$CASES_DIR"/*.sh; do
        if [[ -f "$suite_file" ]]; then
            local name
            name=$(basename "$suite_file" .sh)
            local desc
            desc=$(head -5 "$suite_file" | grep -oP '(?<=# Suite: ).*' || echo "$name")
            printf "  %-25s %s\n" "$name" "$desc"
        fi
    done
}

run_suite() {
    local suite_name="$1"
    local suite_file="$CASES_DIR/${suite_name}.sh"

    if [[ ! -f "$suite_file" ]]; then
        fail "Suite not found: $suite_name"
        return 1
    fi

    phase "Suite: $suite_name"

    # Source the suite (it defines test functions) and run its main
    source "$suite_file"

    if declare -f "suite_${suite_name}" >/dev/null 2>&1; then
        "suite_${suite_name}"
    else
        fail "Suite $suite_name does not define suite_${suite_name}()"
    fi
}

# ── Report generation ───────────────────────────────────────────
generate_report() {
    local session_end=$(date +%s)
    local duration=$((session_end - SESSION_START))
    local duration_fmt
    duration_fmt=$(printf '%dm%ds' $((duration / 60)) $((duration % 60)))

    cat > "$REPORT_FILE" << EOF
# E2E Test Report — ${DATE_STAMP}

## Summary
- **Total:** ${TOTAL_COUNT} test cases
- **Passed:** ${PASS_COUNT}
- **Failed:** ${FAIL_COUNT}
- **Skipped:** ${SKIP_COUNT}
- **Anomalies:** ${ANOMALY_COUNT}
- **Duration:** ${duration_fmt}
- **Binary:** $(git -C "$PROJECT_DIR" rev-parse --short HEAD 2>/dev/null || echo "unknown")
- **Branch:** $(git -C "$PROJECT_DIR" branch --show-current 2>/dev/null || echo "unknown")
EOF

    if [[ ${#FAILED_TESTS[@]} -gt 0 ]]; then
        echo "" >> "$REPORT_FILE"
        echo "## Failed Tests" >> "$REPORT_FILE"
        for test in "${FAILED_TESTS[@]}"; do
            echo "- ❌ ${test}" >> "$REPORT_FILE"
        done
    fi

    if [[ ${#ANOMALY_TESTS[@]} -gt 0 ]]; then
        echo "" >> "$REPORT_FILE"
        echo "## Anomalies" >> "$REPORT_FILE"
        for test in "${ANOMALY_TESTS[@]}"; do
            echo "- ⚠️ ${test}" >> "$REPORT_FILE"
        done
    fi

    echo "" >> "$REPORT_FILE"
    echo "## Environment" >> "$REPORT_FILE"
    echo "- **Workspace:** \`${E2E_WORKSPACE}\`" >> "$REPORT_FILE"
    echo "- **OS:** $(uname -s) $(uname -m)" >> "$REPORT_FILE"
    echo "- **Rust:** $(rustc --version 2>/dev/null || echo 'unknown')" >> "$REPORT_FILE"
    echo "- **Elixir:** $(elixir --version 2>/dev/null | head -1 || echo 'unknown')" >> "$REPORT_FILE"

    info "Report written to $REPORT_FILE"
}

# ── Summary ─────────────────────────────────────────────────────
print_summary() {
    local session_end=$(date +%s)
    local duration=$((session_end - SESSION_START))

    printf "\n${BOLD}━━━ E2E Test Results ━━━${RESET}\n\n"
    printf "${GREEN}  Passed:    %d${RESET}\n" "$PASS_COUNT"
    [[ $FAIL_COUNT -gt 0 ]]    && printf "${RED}  Failed:    %d${RESET}\n" "$FAIL_COUNT"
    [[ $SKIP_COUNT -gt 0 ]]    && printf "${YELLOW}  Skipped:   %d${RESET}\n" "$SKIP_COUNT"
    [[ $ANOMALY_COUNT -gt 0 ]] && printf "${YELLOW}  Anomalies: %d${RESET}\n" "$ANOMALY_COUNT"
    printf "  Time:      %ds\n\n" "$duration"

    if [[ $FAIL_COUNT -gt 0 ]]; then
        printf "${RED}${BOLD}E2E TESTS FAILED${RESET}\n"
        return 1
    else
        printf "${GREEN}${BOLD}E2E TESTS PASSED${RESET}\n"
        return 0
    fi
}

# ── Main ────────────────────────────────────────────────────────
main() {
    # Handle special flags
    case "${1:-}" in
        --list)  list_suites; exit 0 ;;
        --clean) cleanup; echo "Cleaned up."; exit 0 ;;
        --help)
            echo "Usage: $0 [suite1 suite2 ...] [--list] [--clean]"
            exit 0
            ;;
    esac

    setup_env
    build_binary

    # Determine which suites to run
    local suites=()
    if [[ $# -gt 0 ]]; then
        suites=("$@")
    else
        # Run all suites in order
        for suite_file in "$CASES_DIR"/*.sh; do
            if [[ -f "$suite_file" ]]; then
                suites+=("$(basename "$suite_file" .sh)")
            fi
        done
    fi

    if [[ ${#suites[@]} -eq 0 ]]; then
        info "No test suites found in $CASES_DIR/"
        info "Create test suites as .sh files in the cases/ directory."
        exit 0
    fi

    # Run suites
    for suite in "${suites[@]}"; do
        run_suite "$suite"
    done

    # Report and summary
    generate_report
    print_summary
}

main "$@"
