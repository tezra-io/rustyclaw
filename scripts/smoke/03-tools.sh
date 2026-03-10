#!/usr/bin/env bash
#
# Phase 3: Tool Execution via Webhook
# Sends messages that should trigger tool use, verifies output.
#
# Sourced by smoke-test.sh — can also run standalone.
SMOKE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -z "${GATEWAY_URL:-}" ]]; then source "$SMOKE_DIR/lib.sh"; fi

phase "Tool Execution"

# Tool tests need a working provider — check gateway is responding
test_gateway_alive() {
    http_request GET "$GATEWAY_URL/health"
    if [[ "$HTTP_CODE" != "200" ]]; then
        skip "Gateway not healthy — skipping tool tests"
        return 1
    fi
}

if ! test_gateway_alive; then
    return 0 2>/dev/null || true
fi

# ── Shell tool: compute 2+2 ─────────────────────────────────────
test_shell_tool() {
    local label="Shell tool (compute 2+2)"

    http_request POST "$GATEWAY_URL/webhook" \
        '{"message": "Use the shell tool to run: echo $((2+2)). Reply ONLY with the number result."}'

    if [[ "$HTTP_CODE" == "000" ]]; then
        fail "$label — request timed out"
        return 1
    fi

    local response
    response=$(http_json '.response // empty')

    if [[ -z "$response" ]]; then
        local err
        err=$(http_json '.error // empty')
        if [[ -n "$err" ]]; then
            fail "$label — error: ${err:0:120}"
        else
            fail "$label — empty response"
        fi
        return 1
    fi

    # Check if "4" appears in the response
    if echo "$response" | grep -q '4'; then
        pass "$label → contains '4'"
    else
        fail "$label → expected '4' in: ${response:0:120}"
        return 1
    fi
}
run_test "shell_tool" test_shell_tool

# ── Tools listing endpoint ──────────────────────────────────────
test_tools_available() {
    local label="Tools available via API"

    http_request GET "$GATEWAY_URL/api/tools"

    if [[ "$HTTP_CODE" != "200" ]]; then
        fail "$label → HTTP $HTTP_CODE"
        return 1
    fi

    local tool_count
    tool_count=$(http_json '.tools | length')

    if [[ "$tool_count" -gt 0 ]]; then
        pass "$label → $tool_count tools registered"
    else
        fail "$label → no tools found"
        return 1
    fi
}
run_test "tools_available" test_tools_available
