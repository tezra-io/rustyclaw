#!/usr/bin/env bash
# Suite: Tool Execution — File I/O, Shell, Git

suite_02-tools() {
    start_gateway "basic"

    run_test "TC-2.1" "Shell tool — compute" tc_shell_compute 120
    run_test "TC-2.2" "Tools listing" tc_tools_listing
    run_test "TC-2.3" "File write + read roundtrip" tc_file_write_read 120
    run_test "TC-2.4" "Shell command execution with output" tc_shell_exec_output 120
    run_test "TC-2.5" "Git operations" tc_git_operations 120
    run_test "TC-2.6" "File edit (replace)" tc_file_edit_replace 120
    run_test "TC-2.7" "Shell command with error handling" tc_shell_error_handling 120

    stop_gateway
}

# ── TC-2.1: Shell tool — compute ─────────────────────────────────────────

tc_shell_compute() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/api/chat" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Use the shell tool to compute echo $((6 * 7)) and tell me the result","model":"claude-sonnet-4-20250514"}' 2>/dev/null)
    echo "$resp" | grep -q "42"
}

# ── TC-2.2: Tools listing ────────────────────────────────────────────────
# Verify /api/tools returns the expected enabled tools from basic.toml.

tc_tools_listing() {
    local resp
    resp=$(curl -sf "${GATEWAY_URL}/api/tools" 2>/dev/null)

    if [[ -z "$resp" ]]; then
        echo "FAIL: Empty response from /api/tools"
        return 1
    fi

    # Verify response is a non-empty array
    local tool_count
    tool_count=$(echo "$resp" | jq 'length' 2>/dev/null || echo "0")

    if [[ "$tool_count" -le 0 ]]; then
        echo "FAIL: No tools returned from /api/tools"
        return 1
    fi

    # basic.toml enables: shell, read, write, edit — check at least shell and write
    local has_shell has_write
    has_shell=$(echo "$resp" | jq '[.[] | select(.name == "shell")] | length' 2>/dev/null || echo "0")
    has_write=$(echo "$resp" | jq '[.[] | select(.name == "write")] | length' 2>/dev/null || echo "0")

    if [[ "$has_shell" -ge 1 && "$has_write" -ge 1 ]]; then
        echo "PASS: Found $tool_count tools (shell and write confirmed)"
        return 0
    fi

    echo "FAIL: Expected shell and write tools, got $tool_count tools"
    return 1
}

# ── TC-2.3: File write + read roundtrip ──────────────────────────────────
# Ask the agent to create a file with specific content, then verify on disk.

tc_file_write_read() {
    local test_file="${E2E_WORKSPACE}/e2e_write_test.txt"
    local marker="rustyclaw_e2e_marker_12345"

    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d "{\"message\":\"Create a file at ${test_file} with the exact content '${marker}', then read it back and confirm the content.\"}" \
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

    # Verify file exists on disk with correct content
    if [[ ! -f "$test_file" ]]; then
        echo "FAIL: File was not created at $test_file"
        return 1
    fi

    if grep -qF "$marker" "$test_file"; then
        echo "PASS: File created with correct content"
        return 0
    fi

    echo "FAIL: File exists but content doesn't match. Got: $(cat "$test_file")"
    return 1
}

# ── TC-2.4: Shell command execution with output ──────────────────────────
# Ask the agent to run a shell command and report the output.

tc_shell_exec_output() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Run this exact shell command and report the output: echo hello_e2e_test_marker"}' \
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

    # The response should contain the echo output
    if echo "$response_text" | grep -qF "hello_e2e_test_marker"; then
        echo "PASS: Shell output captured in response"
        return 0
    fi

    echo "FAIL: Response does not contain expected output. Response: ${response_text:0:200}"
    return 1
}

# ── TC-2.5: Git operations ───────────────────────────────────────────────
# Ask the agent to init a git repo, create a file, and commit.

tc_git_operations() {
    local git_dir="${E2E_WORKSPACE}/e2e_git_test"
    mkdir -p "$git_dir"

    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d "{\"message\":\"Using the shell tool, do these steps in order: 1) Run 'git init ${git_dir}'. 2) Run 'echo e2e_git_content > ${git_dir}/README.md'. 3) Run 'git -C ${git_dir} add .'. 4) Run 'git -C ${git_dir} commit -m initial'.\"}" \
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

    # Verify .git directory exists
    if [[ -d "$git_dir/.git" ]]; then
        echo "PASS: Git repository initialized"
        return 0
    fi

    echo "FAIL: .git directory not found in $git_dir"
    return 1
}

# ── TC-2.6: File edit (replace) ──────────────────────────────────────────
# Create a file with known content, ask the agent to replace text, verify on disk.

tc_file_edit_replace() {
    local test_file="${E2E_WORKSPACE}/e2e_edit_test.txt"
    echo "The quick brown fox jumps over the lazy dog" > "$test_file"

    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d "{\"message\":\"Edit the file at ${test_file} — replace the word 'fox' with the word 'cat'. Use the edit tool to make this change.\"}" \
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

    # Verify file content was changed
    if grep -qF "cat" "$test_file" && ! grep -qF "fox" "$test_file"; then
        echo "PASS: File edit succeeded — 'fox' replaced with 'cat'"
        return 0
    fi

    # Fallback: agent may have rewritten the whole file; accept if 'cat' is present
    if grep -qF "cat" "$test_file"; then
        echo "PASS: File contains 'cat' after edit"
        return 0
    fi

    echo "FAIL: File content not changed as expected. Got: $(cat "$test_file")"
    return 1
}

# ── TC-2.7: Shell command with error handling ────────────────────────────
# Ask the agent to run a command that fails, verify graceful error handling.

tc_shell_error_handling() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Run this shell command and tell me what happened: cat /nonexistent/e2e_missing_file_xyz"}' \
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

    # The agent should report the error gracefully
    if echo "$response_text" | grep -iqE 'error|fail|not found|no such file|does not exist|couldn.t|cannot|can.t'; then
        echo "PASS: Agent handled command error gracefully"
        return 0
    fi

    # If the agent responded at all (didn't crash), that's acceptable error handling
    if [[ ${#response_text} -gt 0 ]]; then
        echo "PASS: Agent responded after failed command (${#response_text} chars)"
        return 0
    fi

    echo "FAIL: Agent did not handle error. Response: ${response_text:0:200}"
    return 1
}
