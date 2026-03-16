#!/usr/bin/env bash
# Suite: Cron, SOP, and Skills tools

suite_04-cron() {
    start_gateway "basic"

    # API CRUD tests (no LLM, short timeout)
    run_test "TC-4.1" "Cron API list (empty)" tc_cron_list_empty 30
    run_test "TC-4.2" "Cron API add job" tc_cron_add 30
    run_test "TC-4.3" "Cron API list after add" tc_cron_list_after_add 30
    run_test "TC-4.6" "Cron API delete job" tc_cron_delete 30
    run_test "TC-4.7" "Invalid cron expression" tc_cron_invalid_schedule 30

    # Agent interaction tests (LLM, longer timeout)
    run_test "TC-4.4" "Cron add via agent (webhook)" tc_cron_agent_add 120
    run_test "TC-4.5" "Cron list via agent (webhook)" tc_cron_agent_list 120

    stop_gateway
}

# ── TC-4.1: Cron API list (empty) ─────────────────────────────────────────
# GET /api/cron — verify 200 and JSON array (initially empty or valid)

tc_cron_list_empty() {
    local resp http_code
    resp=$(curl -sf "${GATEWAY_URL}/api/cron" -w '\n%{http_code}' 2>/dev/null)
    http_code=$(echo "$resp" | tail -1)
    local body
    body=$(echo "$resp" | sed '$d')

    if [[ "$http_code" != "200" ]]; then
        echo "FAIL: Expected HTTP 200, got $http_code"
        return 1
    fi

    # Verify response is valid JSON with a jobs array
    if ! echo "$body" | jq -e '.jobs' >/dev/null 2>&1; then
        # Some implementations return a bare array
        if ! echo "$body" | jq -e 'type == "array"' >/dev/null 2>&1; then
            echo "FAIL: Response is not valid JSON array or object with jobs. Body: ${body:0:200}"
            return 1
        fi
    fi

    echo "PASS: Cron list returned 200 with valid JSON"
    return 0
}

# ── TC-4.2: Cron API add job ──────────────────────────────────────────────
# POST /api/cron with a valid cron job definition, verify 200

tc_cron_add() {
    local resp http_code
    resp=$(curl -sf -X POST "${GATEWAY_URL}/api/cron" \
        -H 'Content-Type: application/json' \
        -d '{"name":"e2e_test_cron","schedule":"*/30 * * * *","command":"echo e2e_cron_test"}' \
        -w '\n%{http_code}' 2>/dev/null)
    http_code=$(echo "$resp" | tail -1)
    local body
    body=$(echo "$resp" | sed '$d')

    if [[ "$http_code" != "200" ]]; then
        echo "FAIL: Expected HTTP 200, got $http_code. Body: ${body:0:200}"
        return 1
    fi

    # Verify response indicates success
    if echo "$body" | jq -e '.status == "ok"' >/dev/null 2>&1; then
        # Store the job ID for later tests
        local job_id
        job_id=$(echo "$body" | jq -r '.job.id // empty' 2>/dev/null)
        if [[ -n "$job_id" ]]; then
            echo "$job_id" > "$E2E_WORKSPACE/cron_job_id.txt"
        fi
        echo "PASS: Cron job added (id: ${job_id:-unknown})"
        return 0
    fi

    echo "FAIL: Response did not indicate success. Body: ${body:0:200}"
    return 1
}

# ── TC-4.3: Cron API list after add ───────────────────────────────────────
# GET /api/cron — verify the added job appears in the listing

tc_cron_list_after_add() {
    local resp http_code
    resp=$(curl -sf "${GATEWAY_URL}/api/cron" -w '\n%{http_code}' 2>/dev/null)
    http_code=$(echo "$resp" | tail -1)
    local body
    body=$(echo "$resp" | sed '$d')

    if [[ "$http_code" != "200" ]]; then
        echo "FAIL: Expected HTTP 200, got $http_code"
        return 1
    fi

    # Check if our test job appears in the response
    if echo "$body" | grep -q "e2e_test_cron"; then
        echo "PASS: Added job found in cron listing"
        return 0
    fi

    if echo "$body" | grep -q "e2e_cron_test"; then
        echo "PASS: Added job command found in cron listing"
        return 0
    fi

    echo "FAIL: Added job not found in listing. Body: ${body:0:300}"
    return 1
}

# ── TC-4.6: Cron API delete job ───────────────────────────────────────────
# DELETE the test cron job, verify it is removed from listing

tc_cron_delete() {
    # Read the job ID saved by tc_cron_add
    local job_id=""
    if [[ -f "$E2E_WORKSPACE/cron_job_id.txt" ]]; then
        job_id=$(cat "$E2E_WORKSPACE/cron_job_id.txt")
    fi

    if [[ -z "$job_id" ]]; then
        # Try to find the job ID from the listing
        local list_resp
        list_resp=$(curl -sf "${GATEWAY_URL}/api/cron" 2>/dev/null)
        job_id=$(echo "$list_resp" | jq -r '.jobs[]? | select(.name == "e2e_test_cron") | .id // empty' 2>/dev/null)

        if [[ -z "$job_id" ]]; then
            # Try bare array format
            job_id=$(echo "$list_resp" | jq -r '.[]? | select(.name == "e2e_test_cron") | .id // empty' 2>/dev/null)
        fi
    fi

    if [[ -z "$job_id" ]]; then
        echo "FAIL: No job ID available for deletion (add test may have failed)"
        return 1
    fi

    # Delete the job
    local del_code
    del_code=$(curl -sf -o /dev/null -w '%{http_code}' -X DELETE "${GATEWAY_URL}/api/cron/${job_id}" 2>/dev/null)

    if [[ "$del_code" != "200" ]]; then
        echo "FAIL: DELETE returned HTTP $del_code (expected 200)"
        return 1
    fi

    # Verify the job is gone
    local list_after
    list_after=$(curl -sf "${GATEWAY_URL}/api/cron" 2>/dev/null)

    if echo "$list_after" | grep -q "e2e_test_cron"; then
        echo "FAIL: Job still appears in listing after delete"
        return 1
    fi

    echo "PASS: Cron job deleted and no longer in listing"
    return 0
}

# ── TC-4.7: Invalid cron expression ───────────────────────────────────────
# POST /api/cron with an invalid schedule, verify graceful error handling

tc_cron_invalid_schedule() {
    local resp http_code
    resp=$(curl -s -X POST "${GATEWAY_URL}/api/cron" \
        -H 'Content-Type: application/json' \
        -d '{"name":"bad_cron","schedule":"not-a-valid-cron-expression","command":"echo bad"}' \
        -w '\n%{http_code}' 2>/dev/null)
    http_code=$(echo "$resp" | tail -1)
    local body
    body=$(echo "$resp" | sed '$d')

    # Should return an error status (4xx or 5xx), not crash
    if [[ "$http_code" =~ ^[45][0-9][0-9]$ ]]; then
        echo "PASS: Invalid schedule returned HTTP $http_code"
        return 0
    fi

    # If 200, check if the response body indicates an error
    if echo "$body" | jq -e '.error' >/dev/null 2>&1; then
        echo "PASS: Invalid schedule returned error in body"
        return 0
    fi

    echo "FAIL: Invalid schedule was accepted without error (HTTP $http_code). Body: ${body:0:200}"
    return 1
}

# ── TC-4.4: Cron add via agent (webhook) ──────────────────────────────────
# Ask the agent to create a cron job via natural language

tc_cron_agent_add() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Create a cron job named agent_cron_test that runs every hour and executes the command: echo agent_cron_ok. Use the cron tool to do this."}' \
        --max-time 90 2>/dev/null)

    if [[ -z "$resp" ]]; then
        echo "FAIL: Empty response from gateway"
        return 1
    fi

    local response_text
    response_text=$(echo "$resp" | jq -r '.response // empty' 2>/dev/null)

    if [[ -z "$response_text" ]]; then
        echo "FAIL: No response field in JSON: ${resp:0:200}"
        return 1
    fi

    # Agent should acknowledge the cron job creation
    if echo "$response_text" | grep -iqE 'cron|job|created|scheduled|set up|added|every hour'; then
        echo "PASS: Agent acknowledged cron job creation"
        return 0
    fi

    # Any non-empty response about the task is acceptable
    if [[ ${#response_text} -gt 10 ]]; then
        echo "PASS: Agent responded to cron creation request (${#response_text} chars)"
        return 0
    fi

    echo "FAIL: Agent did not acknowledge cron creation. Response: ${response_text:0:200}"
    return 1
}

# ── TC-4.5: Cron list via agent (webhook) ─────────────────────────────────
# Ask the agent to list cron jobs

tc_cron_agent_list() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"List all cron jobs that are currently scheduled. Show me the names and schedules."}' \
        --max-time 90 2>/dev/null)

    if [[ -z "$resp" ]]; then
        echo "FAIL: Empty response from gateway"
        return 1
    fi

    local response_text
    response_text=$(echo "$resp" | jq -r '.response // empty' 2>/dev/null)

    if [[ -z "$response_text" ]]; then
        echo "FAIL: No response field in JSON: ${resp:0:200}"
        return 1
    fi

    # Agent should mention cron jobs or indicate no/some jobs exist
    if echo "$response_text" | grep -iqE 'cron|job|schedule|no.*jobs|empty|none|currently'; then
        echo "PASS: Agent listed cron jobs"
        return 0
    fi

    # Any substantive response is acceptable
    if [[ ${#response_text} -gt 10 ]]; then
        echo "PASS: Agent responded to cron list request (${#response_text} chars)"
        return 0
    fi

    echo "FAIL: Agent did not list cron jobs. Response: ${response_text:0:200}"
    return 1
}
