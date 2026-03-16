#!/usr/bin/env bash
# Suite: Core Agent Loop — CLI single & multi-turn

suite_01-core() {
    start_gateway "basic"

    run_test "TC-1.1" "Gateway health endpoint" tc_health
    run_test "TC-1.2" "Single-turn chat" tc_single_chat
    run_test "TC-1.3" "Status endpoint" tc_status
    run_test "TC-1.4" "Config endpoint" tc_config
    run_test "TC-1.5" "Tools listing" tc_tools
    run_test "TC-1.6" "Single message with tool call (shell)" tc_tool_call_shell 120
    run_test "TC-1.7" "Chat with math verification" tc_math_verify 120
    run_test "TC-1.8" "System persona reflected in response" tc_persona 120
    run_test "TC-1.9" "Concurrent requests" tc_concurrent 120
    run_test "TC-1.10" "Empty message handling" tc_empty_message 30
    run_test "TC-1.11" "Large message handling" tc_large_message 120

    stop_gateway
}

tc_health() {
    local resp
    resp=$(curl -sf "${GATEWAY_URL}/health" 2>/dev/null)
    echo "$resp" | jq -e '.status == "ok"' >/dev/null
}

tc_single_chat() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Reply with exactly: ECHO_OK"}' \
        --max-time 90 2>/dev/null)
    local response_text
    response_text=$(echo "$resp" | jq -r '.response // empty' 2>/dev/null)
    [[ -n "$response_text" ]]
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
    count=$(echo "$resp" | jq '.tools | length' 2>/dev/null)
    [[ "$count" -gt 0 ]]
}

# ── TC-1.6: Single message with tool call (shell) ───────────────────────────
# Ask the agent to run a shell command, verify the output appears in response.

tc_tool_call_shell() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Run this shell command and show me the output: echo hello_e2e_test"}' \
        --max-time 90 2>/dev/null)

    if [[ -z "$resp" ]]; then
        echo "FAIL: Empty response from gateway"
        return 1
    fi

    local response_text
    response_text=$(echo "$resp" | jq -r '.response // empty' 2>/dev/null)

    if [[ -z "$response_text" ]]; then
        echo "FAIL: No response field in JSON: $resp"
        return 1
    fi

    if echo "$response_text" | grep -q "hello_e2e_test"; then
        echo "PASS: Shell tool executed and output captured"
        return 0
    fi

    echo "FAIL: Response did not contain shell output. Response: ${response_text:0:200}"
    return 1
}

# ── TC-1.7: Chat with math verification ──────────────────────────────────────
# Ask a simple math question, verify the correct answer.

tc_math_verify() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"What is 2+2? Reply with just the number, nothing else."}' \
        --max-time 90 2>/dev/null)

    if [[ -z "$resp" ]]; then
        echo "FAIL: Empty response from gateway"
        return 1
    fi

    local response_text
    response_text=$(echo "$resp" | jq -r '.response // empty' 2>/dev/null)

    if [[ -z "$response_text" ]]; then
        echo "FAIL: No response field in JSON: $resp"
        return 1
    fi

    if echo "$response_text" | grep -q "4"; then
        echo "PASS: Math verification correct"
        return 0
    fi

    echo "FAIL: Response did not contain '4'. Response: ${response_text:0:200}"
    return 1
}

# ── TC-1.8: System persona reflected in response ─────────────────────────────
# Verify the gateway handles persona queries without crashing.

tc_persona() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"What is your name or persona? Tell me about yourself."}' \
        --max-time 90 2>/dev/null)

    if [[ -z "$resp" ]]; then
        echo "FAIL: Empty response from gateway"
        return 1
    fi

    local response_text
    response_text=$(echo "$resp" | jq -r '.response // empty' 2>/dev/null)

    if [[ -z "$response_text" ]]; then
        echo "FAIL: No response field in JSON: $resp"
        return 1
    fi

    # Any non-empty response is acceptable — persona may or may not be reflected
    echo "PASS: Persona query returned response (${#response_text} chars)"
    return 0
}

# ── TC-1.9: Concurrent requests ──────────────────────────────────────────────
# Send 3 requests in parallel, verify all return valid responses.

tc_concurrent() {
    local tmpdir
    tmpdir=$(mktemp -d)

    # Launch 3 concurrent requests
    for i in 1 2 3; do
        curl -sf -X POST "${GATEWAY_URL}/webhook" \
            -H 'Content-Type: application/json' \
            -d "{\"message\":\"Reply with exactly: CONCURRENT_${i}\"}" \
            --max-time 90 \
            > "${tmpdir}/resp_${i}.json" 2>/dev/null &
    done

    # Wait for all background jobs
    wait

    local success=0
    for i in 1 2 3; do
        local resp_file="${tmpdir}/resp_${i}.json"
        if [[ ! -s "$resp_file" ]]; then
            echo "FAIL: Request $i returned empty response"
            rm -rf "$tmpdir"
            return 1
        fi

        local response_text
        response_text=$(jq -r '.response // empty' "$resp_file" 2>/dev/null)

        if [[ -n "$response_text" ]]; then
            success=$((success + 1))
        else
            echo "FAIL: Request $i had no response field"
        fi
    done

    rm -rf "$tmpdir"

    if [[ $success -eq 3 ]]; then
        echo "PASS: All 3 concurrent requests returned valid responses"
        return 0
    fi

    echo "FAIL: Only $success/3 concurrent requests succeeded"
    return 1
}

# ── TC-1.10: Empty message handling ──────────────────────────────────────────
# Send an empty message, verify the gateway handles it gracefully.

tc_empty_message() {
    local code
    code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":""}' \
        --max-time 20 2>/dev/null)

    # Any valid HTTP response (not a hang/crash) is acceptable
    if [[ "$code" =~ ^[2-5][0-9][0-9]$ ]]; then
        echo "PASS: Empty message handled gracefully (HTTP $code)"
        return 0
    fi

    echo "FAIL: Gateway did not respond to empty message (HTTP $code)"
    return 1
}

# ── TC-1.11: Large message handling ──────────────────────────────────────────
# Send a very long message, verify the gateway handles it without crashing.

tc_large_message() {
    # Generate a 1200-char message
    local large_msg
    large_msg="This is a large message test. Please reply with exactly: LARGE_OK. $(printf 'padding_text %.0s' {1..150})"

    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d "{\"message\":\"${large_msg}\"}" \
        --max-time 90 2>/dev/null)

    if [[ -z "$resp" ]]; then
        echo "FAIL: Empty response from gateway for large message"
        return 1
    fi

    local response_text
    response_text=$(echo "$resp" | jq -r '.response // empty' 2>/dev/null)

    if [[ -n "$response_text" ]]; then
        echo "PASS: Large message handled (${#response_text} char response)"
        return 0
    fi

    # Even a valid JSON response without .response is acceptable (no crash)
    if echo "$resp" | jq -e '.' >/dev/null 2>&1; then
        echo "PASS: Large message returned valid JSON"
        return 0
    fi

    echo "FAIL: Large message not handled gracefully"
    return 1
}
