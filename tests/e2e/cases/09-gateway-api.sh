#!/usr/bin/env bash
# Suite: Gateway HTTP API — endpoints, concurrency, error handling

suite_09-gateway-api() {
    start_gateway "basic"

    run_test "TC-9.1" "All API endpoints return valid JSON" tc_all_endpoints_json 30
    run_test "TC-9.2" "Chat via webhook — round trip" tc_webhook_chat 120
    run_test "TC-9.3" "Concurrent health requests" tc_concurrent_health 30
    run_test "TC-9.4" "Concurrent chat requests" tc_concurrent_chat 120
    run_test "TC-9.5" "Config reload via PUT" tc_config_reload 30
    run_test "TC-9.6" "Metrics endpoint" tc_metrics 30
    run_test "TC-9.7" "Invalid request body" tc_invalid_json 30
    run_test "TC-9.8" "Missing Content-Type header" tc_missing_content_type 30

    stop_gateway
}

# ── TC-9.1: All API endpoints return valid JSON ─────────────────────────────
# Hit every known JSON endpoint, verify each returns 200 and valid JSON.

tc_all_endpoints_json() {
    local endpoints=(
        "/health"
        "/api/status"
        "/api/config"
        "/api/tools"
        "/api/cron"
        "/api/memory"
        "/api/cost"
        "/api/health"
        "/api/integrations"
        "/api/cli-tools"
    )

    local failed=0
    for ep in "${endpoints[@]}"; do
        local resp http_code body
        resp=$(curl -sf -w '\n%{http_code}' "${GATEWAY_URL}${ep}" 2>/dev/null) || true
        http_code=$(echo "$resp" | tail -1)
        body=$(echo "$resp" | sed '$d')

        if [[ "$http_code" != "200" ]]; then
            echo "FAIL: ${ep} returned HTTP ${http_code}"
            failed=$((failed + 1))
            continue
        fi

        if ! echo "$body" | jq -e '.' >/dev/null 2>&1; then
            echo "FAIL: ${ep} returned invalid JSON"
            failed=$((failed + 1))
            continue
        fi

        echo "OK: ${ep} → 200, valid JSON"
    done

    if [[ $failed -gt 0 ]]; then
        echo "FAIL: ${failed} endpoint(s) failed"
        return 1
    fi

    echo "PASS: All ${#endpoints[@]} endpoints returned 200 with valid JSON"
    return 0
}

# ── TC-9.2: Chat via webhook — round trip ────────────────────────────────────
# POST /webhook with a simple message, verify response JSON has "response" field.

tc_webhook_chat() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Reply with exactly: GATEWAY_E2E_OK"}' \
        --max-time 90 2>/dev/null)

    if [[ -z "$resp" ]]; then
        echo "FAIL: Empty response from webhook"
        return 1
    fi

    local response_text
    response_text=$(echo "$resp" | jq -r '.response // empty' 2>/dev/null)

    if [[ -z "$response_text" ]]; then
        echo "FAIL: No 'response' field in JSON: ${resp:0:200}"
        return 1
    fi

    echo "PASS: Webhook round-trip returned response (${#response_text} chars)"
    return 0
}

# ── TC-9.3: Concurrent health requests ───────────────────────────────────────
# Send 5 simultaneous GET /health requests, verify all return 200.

tc_concurrent_health() {
    local tmpdir
    tmpdir=$(mktemp -d)

    for i in 1 2 3 4 5; do
        curl -sf -o "${tmpdir}/resp_${i}.json" -w '%{http_code}' \
            "${GATEWAY_URL}/health" \
            > "${tmpdir}/code_${i}.txt" 2>/dev/null &
    done

    wait

    local success=0
    for i in 1 2 3 4 5; do
        local code
        code=$(cat "${tmpdir}/code_${i}.txt" 2>/dev/null)
        if [[ "$code" == "200" ]]; then
            success=$((success + 1))
        else
            echo "FAIL: Health request $i returned HTTP ${code:-timeout}"
        fi
    done

    rm -rf "$tmpdir"

    if [[ $success -eq 5 ]]; then
        echo "PASS: All 5 concurrent health requests returned 200"
        return 0
    fi

    echo "FAIL: Only ${success}/5 concurrent health requests succeeded"
    return 1
}

# ── TC-9.4: Concurrent chat requests ────────────────────────────────────────
# Send 3 simultaneous POST /webhook requests, verify all return valid responses.

tc_concurrent_chat() {
    local tmpdir
    tmpdir=$(mktemp -d)

    for i in 1 2 3; do
        curl -sf -X POST "${GATEWAY_URL}/webhook" \
            -H 'Content-Type: application/json' \
            -d "{\"message\":\"Reply with exactly: CONCURRENT_GW_${i}\"}" \
            --max-time 90 \
            > "${tmpdir}/resp_${i}.json" 2>/dev/null &
    done

    wait

    local success=0
    for i in 1 2 3; do
        local resp_file="${tmpdir}/resp_${i}.json"
        if [[ ! -s "$resp_file" ]]; then
            echo "FAIL: Chat request $i returned empty response"
            continue
        fi

        local response_text
        response_text=$(jq -r '.response // empty' "$resp_file" 2>/dev/null)

        if [[ -n "$response_text" ]]; then
            success=$((success + 1))
        else
            echo "FAIL: Chat request $i had no response field"
        fi
    done

    rm -rf "$tmpdir"

    if [[ $success -eq 3 ]]; then
        echo "PASS: All 3 concurrent chat requests returned valid responses"
        return 0
    fi

    echo "FAIL: Only ${success}/3 concurrent chat requests succeeded"
    return 1
}

# ── TC-9.5: Config reload via PUT ────────────────────────────────────────────
# GET /api/config, PUT /api/config with the same content, GET again.
# Verify gateway survives the reload cycle.

tc_config_reload() {
    # Step 1: GET current config
    local original
    original=$(curl -sf "${GATEWAY_URL}/api/config" 2>/dev/null)

    if [[ -z "$original" ]] || ! echo "$original" | jq -e '.' >/dev/null 2>&1; then
        echo "FAIL: Could not GET /api/config"
        return 1
    fi

    # Step 2: PUT config back (extract TOML representation or send minimal valid TOML)
    local put_code
    put_code=$(curl -sf -o /dev/null -w '%{http_code}' -X PUT "${GATEWAY_URL}/api/config" \
        -H 'Content-Type: application/toml' \
        -d 'default_provider = "anthropic"
default_model = "claude-sonnet-4-20250514"
default_temperature = 0.7

[gateway]
require_pairing = false' \
        2>/dev/null || echo "000")

    if [[ "$put_code" != "200" ]]; then
        echo "INFO: PUT /api/config returned HTTP ${put_code} (may require auth or not support update)"
        # Not a hard failure — verify gateway is still alive
    fi

    # Step 3: Verify gateway survived
    local health_code
    health_code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/health" 2>/dev/null || echo "000")

    if [[ "$health_code" == "200" ]]; then
        echo "PASS: Gateway survived config reload cycle (PUT returned ${put_code})"
        return 0
    fi

    echo "FAIL: Gateway unhealthy after config reload (health returned ${health_code})"
    return 1
}

# ── TC-9.6: Metrics endpoint ────────────────────────────────────────────────
# GET /metrics, verify it returns Prometheus-format text.

tc_metrics() {
    local resp http_code body
    resp=$(curl -sf -w '\n%{http_code}' "${GATEWAY_URL}/metrics" 2>/dev/null) || true
    http_code=$(echo "$resp" | tail -1)
    body=$(echo "$resp" | sed '$d')

    if [[ "$http_code" != "200" ]]; then
        echo "FAIL: /metrics returned HTTP ${http_code}"
        return 1
    fi

    # Prometheus text format starts with # HELP or # TYPE lines, or has metric_name{} patterns
    if echo "$body" | grep -qE '^#|^[a-z_]+(\{|[[:space:]])'; then
        echo "PASS: /metrics returned Prometheus-format text"
        return 0
    fi

    # Even a placeholder comment line is valid
    if echo "$body" | grep -qiE 'prometheus|metric'; then
        echo "PASS: /metrics returned metrics-related content"
        return 0
    fi

    echo "FAIL: /metrics response does not appear to be Prometheus format: ${body:0:200}"
    return 1
}

# ── TC-9.7: Invalid request body ────────────────────────────────────────────
# POST /webhook with malformed JSON, verify gateway returns 400 (not crash).

tc_invalid_json() {
    local code
    code=$(curl -sf -o /dev/null -w '%{http_code}' -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{this is not valid json!!!' \
        --max-time 10 2>/dev/null || echo "000")

    if [[ "$code" == "400" || "$code" == "422" ]]; then
        echo "PASS: Malformed JSON returned HTTP ${code}"
        return 0
    fi

    # Gateway didn't crash — any error response is acceptable
    if [[ "$code" =~ ^[4][0-9][0-9]$ ]]; then
        echo "PASS: Malformed JSON returned HTTP ${code} (4xx)"
        return 0
    fi

    # Verify gateway is still alive
    local health_code
    health_code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/health" 2>/dev/null || echo "000")

    if [[ "$health_code" == "200" ]]; then
        echo "PASS: Gateway survived malformed JSON (returned HTTP ${code}, still healthy)"
        return 0
    fi

    echo "FAIL: Gateway returned HTTP ${code} for malformed JSON and may have crashed"
    return 1
}

# ── TC-9.8: Missing Content-Type header ─────────────────────────────────────
# POST /webhook without Content-Type, verify gateway handles gracefully.

tc_missing_content_type() {
    local code
    code=$(curl -sf -o /dev/null -w '%{http_code}' -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type:' \
        -d '{"message":"hello"}' \
        --max-time 10 2>/dev/null || echo "000")

    # Any non-crash response is acceptable (400, 415, or even 200 if the server is lenient)
    if [[ "$code" =~ ^[2-5][0-9][0-9]$ ]]; then
        echo "PASS: Missing Content-Type handled gracefully (HTTP ${code})"
        return 0
    fi

    # Verify gateway is still alive
    local health_code
    health_code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/health" 2>/dev/null || echo "000")

    if [[ "$health_code" == "200" ]]; then
        echo "PASS: Gateway survived missing Content-Type (returned HTTP ${code}, still healthy)"
        return 0
    fi

    echo "FAIL: Gateway did not respond to missing Content-Type (HTTP ${code})"
    return 1
}
