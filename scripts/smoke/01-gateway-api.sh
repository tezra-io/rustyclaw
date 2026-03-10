#!/usr/bin/env bash
#
# Phase 1: Gateway API Endpoints
# Tests all API endpoints return valid responses.
#
# Sourced by smoke-test.sh — can also run standalone.
SMOKE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -z "${GATEWAY_URL:-}" ]]; then source "$SMOKE_DIR/lib.sh"; fi

phase "Gateway API Endpoints"

# ── Health ──────────────────────────────────────────────────────
test_health() {
    http_request GET "$GATEWAY_URL/health"
    if [[ "$HTTP_CODE" != "200" ]]; then
        fail "GET /health → HTTP $HTTP_CODE (expected 200)"
        return 1
    fi
    local status
    status=$(http_json '.status')
    if [[ "$status" == "ok" ]]; then
        pass "GET /health → 200, status=ok"
    else
        fail "GET /health → 200 but status='$status' (expected 'ok')"
        return 1
    fi
}
run_test "health" test_health

# ── Metrics ─────────────────────────────────────────────────────
test_metrics() {
    http_request GET "$GATEWAY_URL/metrics"
    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "GET /metrics → 200"
    else
        fail "GET /metrics → HTTP $HTTP_CODE (expected 200)"
        return 1
    fi
}
run_test "metrics" test_metrics

# ── API Status ──────────────────────────────────────────────────
test_api_status() {
    http_request GET "$GATEWAY_URL/api/status"
    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "GET /api/status → 200"
    else
        fail "GET /api/status → HTTP $HTTP_CODE (expected 200)"
        return 1
    fi
}
run_test "api_status" test_api_status

# ── API Config ──────────────────────────────────────────────────
test_api_config() {
    http_request GET "$GATEWAY_URL/api/config"
    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "GET /api/config → 200"
    else
        fail "GET /api/config → HTTP $HTTP_CODE (expected 200)"
        return 1
    fi
}
run_test "api_config" test_api_config

# ── API Tools ───────────────────────────────────────────────────
test_api_tools() {
    http_request GET "$GATEWAY_URL/api/tools"
    if [[ "$HTTP_CODE" == "200" ]]; then
        local tool_count
        tool_count=$(http_json '.tools | length')
        pass "GET /api/tools → 200 ($tool_count tools)"
    else
        fail "GET /api/tools → HTTP $HTTP_CODE (expected 200)"
        return 1
    fi
}
run_test "api_tools" test_api_tools

# ── API Cron ────────────────────────────────────────────────────
test_api_cron() {
    http_request GET "$GATEWAY_URL/api/cron"
    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "GET /api/cron → 200"
    else
        fail "GET /api/cron → HTTP $HTTP_CODE (expected 200)"
        return 1
    fi
}
run_test "api_cron" test_api_cron

# ── API Memory ──────────────────────────────────────────────────
test_api_memory() {
    http_request GET "$GATEWAY_URL/api/memory"
    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "GET /api/memory → 200"
    else
        fail "GET /api/memory → HTTP $HTTP_CODE (expected 200)"
        return 1
    fi
}
run_test "api_memory" test_api_memory

# ── API Cost ────────────────────────────────────────────────────
test_api_cost() {
    http_request GET "$GATEWAY_URL/api/cost"
    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "GET /api/cost → 200"
    else
        fail "GET /api/cost → HTTP $HTTP_CODE (expected 200)"
        return 1
    fi
}
run_test "api_cost" test_api_cost

# ── API CLI Tools ───────────────────────────────────────────────
test_api_cli_tools() {
    http_request GET "$GATEWAY_URL/api/cli-tools"
    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "GET /api/cli-tools → 200"
    else
        fail "GET /api/cli-tools → HTTP $HTTP_CODE (expected 200)"
        return 1
    fi
}
run_test "api_cli_tools" test_api_cli_tools

# ── API Doctor ──────────────────────────────────────────────────
test_api_doctor() {
    http_request GET "$GATEWAY_URL/api/doctor"
    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "GET /api/doctor → 200"
    else
        fail "GET /api/doctor → HTTP $HTTP_CODE (expected 200)"
        return 1
    fi
}
run_test "api_doctor" test_api_doctor

# ── API Integrations ────────────────────────────────────────────
test_api_integrations() {
    http_request GET "$GATEWAY_URL/api/integrations"
    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "GET /api/integrations → 200"
    else
        fail "GET /api/integrations → HTTP $HTTP_CODE (expected 200)"
        return 1
    fi
}
run_test "api_integrations" test_api_integrations

# ── API Health (dashboard) ──────────────────────────────────────
test_api_health() {
    http_request GET "$GATEWAY_URL/api/health"
    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "GET /api/health → 200"
    else
        fail "GET /api/health → HTTP $HTTP_CODE (expected 200)"
        return 1
    fi
}
run_test "api_health" test_api_health
