#!/usr/bin/env bash
# Suite: Tool Execution — File I/O, Shell, Git

suite_02-tools() {
    start_gateway "basic"

    run_test "TC-2.1" "Shell tool — compute" tc_shell_compute
    run_test "TC-2.2" "File write and read" tc_file_write_read

    stop_gateway
}

tc_shell_compute() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/api/chat" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Use the shell tool to compute echo $((6 * 7)) and tell me the result","model":"claude-sonnet-4-20250514"}' 2>/dev/null)
    echo "$resp" | grep -q "42"
}

tc_file_write_read() {
    local test_file="${E2E_WORKSPACE}/test_file.txt"
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/api/chat" \
        -H 'Content-Type: application/json' \
        -d "{\"message\":\"Write the text 'E2E_TEST_CONTENT' to ${test_file} using the write tool, then read it back and confirm\",\"model\":\"claude-sonnet-4-20250514\"}" 2>/dev/null)
    [[ -f "$test_file" ]] && grep -q "E2E_TEST_CONTENT" "$test_file"
}
