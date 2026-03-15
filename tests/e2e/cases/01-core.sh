#!/usr/bin/env bash
# Suite: Core Agent Loop — CLI single & multi-turn

suite_01-core() {
    start_gateway "basic"

    run_test "TC-1.1" "Gateway health endpoint" tc_health
    run_test "TC-1.2" "Single-turn chat" tc_single_chat
    run_test "TC-1.3" "Status endpoint" tc_status
    run_test "TC-1.4" "Config endpoint" tc_config
    run_test "TC-1.5" "Tools listing" tc_tools

    stop_gateway
}

tc_health() {
    local resp
    resp=$(curl -sf "${GATEWAY_URL}/health" 2>/dev/null)
    echo "$resp" | jq -e '.status == "ok"' >/dev/null
}

tc_single_chat() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/api/chat" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Reply with exactly: ECHO_OK","model":"claude-sonnet-4-20250514"}' 2>/dev/null)
    echo "$resp" | grep -q "ECHO_OK"
}

tc_status() {
    local code
    code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/api/status" 2>/dev/null)
    [[ "$code" == "200" ]]
}

tc_config() {
    local code
    code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/api/config" 2>/dev/null)
    [[ "$code" == "200" ]]
}

tc_tools() {
    local resp
    resp=$(curl -sf "${GATEWAY_URL}/api/tools" 2>/dev/null)
    local count
    count=$(echo "$resp" | jq '. | length' 2>/dev/null)
    [[ "$count" -gt 0 ]]
}
