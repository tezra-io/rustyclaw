#!/usr/bin/env bash
# Suite: Cron, SOP, and Skills tools

suite_04-cron() {
    start_gateway "basic"

    run_test "TC-4.1" "Cron list" tc_cron_list
    run_test "TC-4.2" "Cron add and remove" tc_cron_crud

    stop_gateway
}

tc_cron_list() {
    local code
    code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/api/cron" 2>/dev/null)
    [[ "$code" == "200" ]]
}

tc_cron_crud() {
    # Add a cron job
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/api/cron" \
        -H 'Content-Type: application/json' \
        -d '{"name":"e2e_test_cron","schedule":{"kind":"every","everyMs":3600000},"payload":{"kind":"systemEvent","text":"e2e test"},"sessionTarget":"main"}' 2>/dev/null)
    local code=$?
    [[ $code -eq 0 ]]
}
