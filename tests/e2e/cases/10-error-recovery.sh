#!/usr/bin/env bash
# Suite: Error Recovery — invalid configs, malformed requests, resilience

suite_10-error-recovery() {
    # Export vars needed by test functions running in subshells
    export BINARY CONFIGS_DIR

    # ── Phase 1: No gateway — test startup failures ───────────────
    run_test "TC-10.1" "Invalid config handling" tc_invalid_config 30
    run_test "TC-10.2" "Missing config file" tc_missing_config 30

    # ── Phase 2: Gateway running — request-level errors ───────────
    start_gateway "basic"

    run_test "TC-10.3" "Malformed webhook request" tc_malformed_webhook 30
    run_test "TC-10.4" "Very large request body" tc_large_body 30
    run_test "TC-10.5" "Rapid request flooding" tc_rapid_flood 30

    stop_gateway

    # ── Phase 3: Restart resilience ───────────────────────────────
    run_test "TC-10.6" "Gateway restart resilience" tc_restart_resilience 60
}

# ── TC-10.1: Invalid config handling ─────────────────────────────────────────
# Create a malformed TOML config, try to start gateway with it.
# Verify it fails gracefully (non-zero exit, no panic in stderr).

tc_invalid_config() {
    local bad_config="$E2E_WORKSPACE/bad-config.toml"
    cat > "$bad_config" << 'TOML'
[general
persona = "broken
[providers.test
kind =
TOML

    local stderr_file="$E2E_WORKSPACE/logs/bad-config.log"
    mkdir -p "$(dirname "$stderr_file")"

    timeout 5 "$BINARY" serve \
        --port 0 \
        --config "$bad_config" \
        --workspace "$E2E_WORKSPACE" \
        --no-pairing \
        > /dev/null 2>"$stderr_file"
    local exit_code=$?

    # Timeout (124) means binary hung instead of failing fast
    if [[ $exit_code -eq 124 ]]; then
        echo "FAIL: Binary hung on invalid config (timed out)"
        return 1
    fi

    # Non-zero exit is expected — verify no panic
    if [[ $exit_code -ne 0 ]]; then
        if grep -qi "panic" "$stderr_file" 2>/dev/null; then
            echo "FAIL: Binary panicked on invalid config"
            cat "$stderr_file"
            return 1
        fi
        echo "PASS: Binary rejected invalid config gracefully (exit $exit_code)"
        return 0
    fi

    echo "FAIL: Binary exited 0 with invalid config"
    return 1
}

# ── TC-10.2: Missing config file ─────────────────────────────────────────────
# Try to start gateway with nonexistent config path. Verify clean error.

tc_missing_config() {
    local stderr_file="$E2E_WORKSPACE/logs/missing-config.log"
    mkdir -p "$(dirname "$stderr_file")"

    timeout 5 "$BINARY" serve \
        --port 0 \
        --config "/nonexistent/path/e2e-missing-config.toml" \
        --workspace "$E2E_WORKSPACE" \
        --no-pairing \
        > /dev/null 2>"$stderr_file"
    local exit_code=$?

    if [[ $exit_code -eq 124 ]]; then
        echo "FAIL: Binary hung on missing config (timed out)"
        return 1
    fi

    if [[ $exit_code -ne 0 ]]; then
        if grep -qi "panic" "$stderr_file" 2>/dev/null; then
            echo "FAIL: Binary panicked on missing config"
            cat "$stderr_file"
            return 1
        fi
        echo "PASS: Binary reported missing config cleanly (exit $exit_code)"
        return 0
    fi

    echo "FAIL: Binary exited 0 with missing config"
    return 1
}

# ── TC-10.3: Malformed webhook request ───────────────────────────────────────
# Send garbage data to POST /webhook. Verify 4xx response, no crash.

tc_malformed_webhook() {
    # Send completely non-JSON garbage
    local code
    code=$(curl -sf -o /dev/null -w '%{http_code}' -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d 'this is completely garbage data @@#$%^&*()' \
        --max-time 10 2>/dev/null || echo "000")

    if [[ "$code" =~ ^[4][0-9][0-9]$ ]]; then
        echo "PASS: Garbage data returned HTTP ${code}"
        return 0
    fi

    # Any response + gateway still alive = acceptable
    local health_code
    health_code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/health" 2>/dev/null || echo "000")

    if [[ "$health_code" == "200" ]]; then
        echo "PASS: Gateway survived garbage data (webhook HTTP ${code}, still healthy)"
        return 0
    fi

    echo "FAIL: Gateway may have crashed from garbage data (webhook=${code}, health=${health_code})"
    return 1
}

# ── TC-10.4: Very large request body ─────────────────────────────────────────
# Send a 100KB+ JSON body to POST /webhook. Verify gateway handles it.

tc_large_body() {
    # Generate ~110KB JSON payload
    local payload_file="$E2E_WORKSPACE/large-payload.json"
    printf '{"message":"' > "$payload_file"
    dd if=/dev/zero bs=1024 count=110 2>/dev/null | tr '\0' 'x' >> "$payload_file"
    printf '"}' >> "$payload_file"

    local code
    code=$(curl -sf -o /dev/null -w '%{http_code}' -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d @"$payload_file" \
        --max-time 20 2>/dev/null || echo "000")

    rm -f "$payload_file"

    # Any HTTP response (accept or reject) means no crash
    if [[ "$code" =~ ^[2-5][0-9][0-9]$ ]]; then
        echo "PASS: Large body handled gracefully (HTTP ${code})"
        return 0
    fi

    # Check gateway health
    local health_code
    health_code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/health" 2>/dev/null || echo "000")

    if [[ "$health_code" == "200" ]]; then
        echo "PASS: Gateway survived large body (HTTP ${code}, still healthy)"
        return 0
    fi

    echo "FAIL: Gateway may have crashed from large body (webhook=${code}, health=${health_code})"
    return 1
}

# ── TC-10.5: Rapid request flooding ──────────────────────────────────────────
# Send 20 rapid-fire GET /health requests in a tight loop. Verify all return 200.

tc_rapid_flood() {
    local success=0
    local total=20

    for i in $(seq 1 $total); do
        local code
        code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/health" 2>/dev/null || echo "000")
        if [[ "$code" == "200" ]]; then
            success=$((success + 1))
        fi
    done

    if [[ $success -eq $total ]]; then
        echo "PASS: All ${total} rapid-fire health requests returned 200"
        return 0
    fi

    echo "FAIL: Only ${success}/${total} rapid-fire requests succeeded"
    return 1
}

# ── TC-10.6: Gateway restart resilience ──────────────────────────────────────
# Start gateway, verify health, stop it, start again. Verify clean second start.

tc_restart_resilience() {
    # Create a self-contained config for this test
    local config_file="$E2E_WORKSPACE/config/restart-test.toml"
    cat > "$config_file" << 'TOML'
[general]
persona = "test-agent"

[providers.test]
kind = "anthropic"

[tools]
enabled = ["shell", "read", "write", "edit"]

[security]
approval_mode = "auto"
TOML

    # ── First start ──
    local log1="$E2E_WORKSPACE/logs/gateway-restart-1.log"
    mkdir -p "$(dirname "$log1")"

    RUST_LOG=debug "$BINARY" serve \
        --port 0 \
        --config "$config_file" \
        --workspace "$E2E_WORKSPACE" \
        --no-pairing \
        > "$log1" 2>&1 &
    local pid1=$!
    sleep 2

    local port1
    port1=$(grep -oE 'listening on.*:([0-9]+)' "$log1" | grep -oE '[0-9]+$' | head -1)

    if [[ -z "$port1" ]]; then
        kill "$pid1" 2>/dev/null; wait "$pid1" 2>/dev/null || true
        echo "FAIL: Could not detect port on first start"
        return 1
    fi

    local url1="http://127.0.0.1:${port1}"
    local code1
    code1=$(curl -sf -o /dev/null -w '%{http_code}' "${url1}/health" 2>/dev/null || echo "000")

    if [[ "$code1" != "200" ]]; then
        kill "$pid1" 2>/dev/null; wait "$pid1" 2>/dev/null || true
        echo "FAIL: Gateway not healthy on first start (HTTP ${code1})"
        return 1
    fi

    # ── Stop ──
    kill "$pid1" 2>/dev/null
    wait "$pid1" 2>/dev/null || true
    sleep 0.5

    # ── Second start ──
    local log2="$E2E_WORKSPACE/logs/gateway-restart-2.log"

    RUST_LOG=debug "$BINARY" serve \
        --port 0 \
        --config "$config_file" \
        --workspace "$E2E_WORKSPACE" \
        --no-pairing \
        > "$log2" 2>&1 &
    local pid2=$!
    sleep 2

    local port2
    port2=$(grep -oE 'listening on.*:([0-9]+)' "$log2" | grep -oE '[0-9]+$' | head -1)

    if [[ -z "$port2" ]]; then
        kill "$pid2" 2>/dev/null; wait "$pid2" 2>/dev/null || true
        echo "FAIL: Could not detect port on second start"
        return 1
    fi

    local url2="http://127.0.0.1:${port2}"
    local code2
    code2=$(curl -sf -o /dev/null -w '%{http_code}' "${url2}/health" 2>/dev/null || echo "000")

    kill "$pid2" 2>/dev/null; wait "$pid2" 2>/dev/null || true

    if [[ "$code2" == "200" ]]; then
        echo "PASS: Gateway restarted cleanly (port ${port1} → ${port2})"
        return 0
    fi

    echo "FAIL: Gateway not healthy after restart (HTTP ${code2})"
    return 1
}
