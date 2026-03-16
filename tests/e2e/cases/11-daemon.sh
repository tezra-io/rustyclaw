#!/usr/bin/env bash
# Suite: Daemon Mode — startup, health, shutdown, concurrent load, config reload

suite_11-daemon() {
    # Export vars needed by test functions running in subshells
    export BINARY CONFIGS_DIR

    # ── Phase 1: Gateway running — daemon behavior tests ──────────
    start_gateway "basic"

    run_test "TC-11.1" "Daemon startup" tc_daemon_startup 30
    run_test "TC-11.2" "Daemon health over time" tc_daemon_health_time 30
    run_test "TC-11.4" "Daemon handles concurrent load" tc_daemon_concurrent 30
    run_test "TC-11.5" "Daemon survives config reload" tc_daemon_config_reload 30

    stop_gateway

    # ── Phase 2: Shutdown testing (manages own gateway) ───────────
    run_test "TC-11.3" "Daemon graceful shutdown" tc_daemon_shutdown 30
}

# ── TC-11.1: Daemon startup ──────────────────────────────────────────────────
# Verify /health responds and /api/status returns valid JSON.

tc_daemon_startup() {
    # Verify /health
    local health_code
    health_code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/health" 2>/dev/null || echo "000")

    if [[ "$health_code" != "200" ]]; then
        echo "FAIL: /health returned HTTP ${health_code}"
        return 1
    fi

    # Verify /api/status returns valid JSON
    local status_resp
    status_resp=$(curl -sf "${GATEWAY_URL}/api/status" 2>/dev/null)

    if [[ -z "$status_resp" ]]; then
        echo "FAIL: /api/status returned empty response"
        return 1
    fi

    if ! echo "$status_resp" | jq -e '.' >/dev/null 2>&1; then
        echo "FAIL: /api/status returned invalid JSON: ${status_resp:0:200}"
        return 1
    fi

    echo "PASS: Daemon started — /health=200, /api/status=valid JSON"
    return 0
}

# ── TC-11.2: Daemon health over time ─────────────────────────────────────────
# Wait 5 seconds, check health 3 times with 2s intervals. Verify consistent.

tc_daemon_health_time() {
    sleep 5

    local success=0
    for i in 1 2 3; do
        local code
        code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/health" 2>/dev/null || echo "000")
        if [[ "$code" == "200" ]]; then
            success=$((success + 1))
        else
            echo "FAIL: Health check $i returned HTTP ${code}"
        fi
        [[ $i -lt 3 ]] && sleep 2
    done

    if [[ $success -eq 3 ]]; then
        echo "PASS: All 3 health checks passed over 9s window"
        return 0
    fi

    echo "FAIL: Only ${success}/3 health checks passed"
    return 1
}

# ── TC-11.3: Daemon graceful shutdown ────────────────────────────────────────
# Start gateway, send SIGTERM, verify exit code and port release.

tc_daemon_shutdown() {
    # Create a self-contained config
    local config_file="$E2E_WORKSPACE/config/shutdown-test.toml"
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

    local log_file="$E2E_WORKSPACE/logs/gateway-shutdown.log"
    mkdir -p "$(dirname "$log_file")"

    RUST_LOG=debug "$BINARY" serve \
        --port 0 \
        --config "$config_file" \
        --workspace "$E2E_WORKSPACE" \
        --no-pairing \
        > "$log_file" 2>&1 &
    local gw_pid=$!
    sleep 2

    local port
    port=$(grep -oE 'listening on.*:([0-9]+)' "$log_file" | grep -oE '[0-9]+$' | head -1)

    if [[ -z "$port" ]]; then
        kill "$gw_pid" 2>/dev/null; wait "$gw_pid" 2>/dev/null || true
        echo "FAIL: Could not detect gateway port"
        return 1
    fi

    local gw_url="http://127.0.0.1:${port}"

    # Verify running
    local code
    code=$(curl -sf -o /dev/null -w '%{http_code}' "${gw_url}/health" 2>/dev/null || echo "000")

    if [[ "$code" != "200" ]]; then
        kill "$gw_pid" 2>/dev/null; wait "$gw_pid" 2>/dev/null || true
        echo "FAIL: Gateway not healthy before shutdown (HTTP ${code})"
        return 1
    fi

    # Send SIGTERM
    kill -TERM "$gw_pid" 2>/dev/null
    wait "$gw_pid" 2>/dev/null
    local exit_code=$?

    # Exit code 0 or 143 (128 + SIGTERM) is acceptable
    if [[ $exit_code -ne 0 && $exit_code -ne 143 ]]; then
        echo "FAIL: Gateway exited with unexpected code ${exit_code}"
        return 1
    fi

    # Verify port is released
    sleep 0.5
    local post_code
    post_code=$(curl -sf -o /dev/null -w '%{http_code}' "${gw_url}/health" --max-time 2 2>/dev/null || echo "000")

    if [[ "$post_code" == "000" || "$post_code" == "007" ]]; then
        echo "PASS: Graceful shutdown — exit ${exit_code}, port released"
        return 0
    fi

    echo "FAIL: Port still responding after shutdown (HTTP ${post_code})"
    return 1
}

# ── TC-11.4: Daemon handles concurrent load ──────────────────────────────────
# Send 10 concurrent /health requests. Verify all succeed.

tc_daemon_concurrent() {
    local tmpdir
    tmpdir=$(mktemp -d)

    for i in $(seq 1 10); do
        curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/health" \
            > "${tmpdir}/code_${i}.txt" 2>/dev/null &
    done

    wait

    local success=0
    for i in $(seq 1 10); do
        local code
        code=$(cat "${tmpdir}/code_${i}.txt" 2>/dev/null)
        if [[ "$code" == "200" ]]; then
            success=$((success + 1))
        fi
    done

    rm -rf "$tmpdir"

    if [[ $success -eq 10 ]]; then
        echo "PASS: All 10 concurrent health requests returned 200"
        return 0
    fi

    echo "FAIL: Only ${success}/10 concurrent requests succeeded"
    return 1
}

# ── TC-11.5: Daemon survives config reload ───────────────────────────────────
# Trigger PUT /api/config, verify /health still responds after reload.

tc_daemon_config_reload() {
    # Trigger config reload via PUT
    local put_code
    put_code=$(curl -sf -o /dev/null -w '%{http_code}' -X PUT "${GATEWAY_URL}/api/config" \
        -H 'Content-Type: application/toml' \
        -d '[general]
persona = "test-agent"

[providers.test]
kind = "anthropic"

[tools]
enabled = ["shell", "read", "write", "edit"]

[security]
approval_mode = "auto"' \
        2>/dev/null || echo "000")

    # Verify /health still responds
    local health_code
    health_code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/health" 2>/dev/null || echo "000")

    if [[ "$health_code" == "200" ]]; then
        echo "PASS: Gateway healthy after config reload (PUT returned ${put_code})"
        return 0
    fi

    echo "FAIL: Gateway unhealthy after config reload (health=${health_code}, PUT=${put_code})"
    return 1
}
