#!/usr/bin/env bash
# Suite: Security & Approval — policy, scrubbing, path traversal

suite_05-security() {
    start_gateway "basic"

    run_test "TC-5.1" "API rejects unauthenticated with auth gateway" tc_auth_reject
    run_test "TC-5.2" "Health endpoint always accessible" tc_health_always

    stop_gateway
}

tc_auth_reject() {
    # Start a separate gateway with pairing enabled
    stop_gateway

    local log_file="$E2E_WORKSPACE/logs/gateway-auth.log"
    local port=0

    RUST_LOG=debug "$BINARY" serve \
        --port 0 \
        --config "$CONFIGS_DIR/security.toml" \
        --workspace "$E2E_WORKSPACE" \
        --pairing \
        > "$log_file" 2>&1 &
    local auth_pid=$!
    sleep 2

    # Extract port
    local auth_port
    auth_port=$(grep -oE 'listening on.*:([0-9]+)' "$log_file" | grep -oE '[0-9]+$' | head -1)

    if [[ -n "$auth_port" ]]; then
        local code
        code=$(curl -sf -o /dev/null -w '%{http_code}' "http://127.0.0.1:${auth_port}/api/status" 2>/dev/null || echo "000")
        kill "$auth_pid" 2>/dev/null; wait "$auth_pid" 2>/dev/null || true
        [[ "$code" == "401" ]]
    else
        kill "$auth_pid" 2>/dev/null; wait "$auth_pid" 2>/dev/null || true
        return 1
    fi
}

tc_health_always() {
    local code
    code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL:-http://127.0.0.1:0}/health" 2>/dev/null || echo "000")
    [[ "$code" == "200" ]]
}
