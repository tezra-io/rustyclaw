#!/usr/bin/env bash
# Suite: Memory System — Store, Recall, Forget

suite_03-memory() {
    start_gateway "basic"

    run_test "TC-3.1" "Memory store" tc_memory_store
    run_test "TC-3.2" "Memory list" tc_memory_list
    run_test "TC-3.3" "Memory delete" tc_memory_delete

    stop_gateway
}

tc_memory_store() {
    local code
    code=$(curl -sf -o /dev/null -w '%{http_code}' -X POST "${GATEWAY_URL}/api/memory" \
        -H 'Content-Type: application/json' \
        -d '{"key":"e2e_test_key","value":"e2e_test_value"}' 2>/dev/null)
    [[ "$code" == "200" ]]
}

tc_memory_list() {
    local code
    code=$(curl -sf -o /dev/null -w '%{http_code}' "${GATEWAY_URL}/api/memory" 2>/dev/null)
    [[ "$code" == "200" ]]
}

tc_memory_delete() {
    local code
    code=$(curl -sf -o /dev/null -w '%{http_code}' -X DELETE "${GATEWAY_URL}/api/memory/e2e_test_key" 2>/dev/null)
    [[ "$code" == "200" ]]
}
