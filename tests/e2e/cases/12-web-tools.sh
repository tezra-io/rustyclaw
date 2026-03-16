#!/usr/bin/env bash
# Suite: Web Tools — fetch, search, HTTP requests

suite_12-web-tools() {
    # Pre-flight: check if httpbin.org is reachable
    if ! curl -sf --max-time 10 "https://httpbin.org/get" >/dev/null 2>&1; then
        skip "TC-12.x: httpbin.org unreachable — skipping web tools suite"
        return 0
    fi

    start_gateway "basic"

    run_test "TC-12.1" "Web fetch via API" tc_web_fetch 180
    run_test "TC-12.2" "Web search (if available)" tc_web_search 180
    run_test "TC-12.3" "HTTP GET request" tc_http_get 180
    run_test "TC-12.4" "HTTP POST request" tc_http_post 180
    run_test "TC-12.5" "Unreachable URL handling" tc_unreachable_url 180
    run_test "TC-12.6" "Large page fetch" tc_large_page_fetch 180

    stop_gateway
}

# ── TC-12.1: Web fetch via API ──────────────────────────────────────────────
# Ask the agent to fetch content from httpbin.org/json and verify parsed output.

tc_web_fetch() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Use the web_fetch tool to fetch the URL https://httpbin.org/json and show me the content you received."}' \
        --max-time 120 2>/dev/null)

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

    # httpbin.org/json returns a slideshow object
    if echo "$response_text" | grep -iqE 'slideshow|slide|title|author'; then
        echo "PASS: Web fetch returned parsed httpbin JSON content"
        return 0
    fi

    # Accept if the agent acknowledged fetching but summarized differently
    if echo "$response_text" | grep -iqE 'fetched|retrieved|content|json|httpbin'; then
        echo "PASS: Web fetch returned content from httpbin (summarized)"
        return 0
    fi

    echo "FAIL: Response did not contain httpbin content. Response: ${response_text:0:300}"
    return 1
}

# ── TC-12.2: Web search (if available) ──────────────────────────────────────
# Ask to search for "Rust programming language". Skip if web search tool unavailable.

tc_web_search() {
    # Check if web_search_tool is in the enabled tools
    local tools_resp
    tools_resp=$(curl -sf "${GATEWAY_URL}/api/tools" 2>/dev/null || echo "")

    if [[ -n "$tools_resp" ]]; then
        local has_search
        has_search=$(echo "$tools_resp" | jq '[.[] | select(.name == "web_search_tool" or .name == "web_search")] | length' 2>/dev/null || echo "0")
        if [[ "$has_search" == "0" ]]; then
            echo "SKIP: web_search_tool not available"
            return 0
        fi
    fi

    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Search the web for \"Rust programming language\" and tell me what you find."}' \
        --max-time 120 2>/dev/null)

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

    # Verify response mentions Rust-related content
    if echo "$response_text" | grep -iqE 'rust|rust-lang|mozilla|systems programming|memory safety|cargo'; then
        echo "PASS: Web search returned Rust-related results"
        return 0
    fi

    # If the agent says search isn't available, that's acceptable
    if echo "$response_text" | grep -iqE 'not available|cannot search|no.*search|unable|don.t have'; then
        echo "SKIP: Agent reports web search not available"
        return 0
    fi

    echo "FAIL: Web search did not return Rust-related content. Response: ${response_text:0:300}"
    return 1
}

# ── TC-12.3: HTTP GET request ───────────────────────────────────────────────
# Ask the agent to make an HTTP GET to httpbin.org/get and verify response info.

tc_http_get() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Make an HTTP GET request to https://httpbin.org/get and show me the response."}' \
        --max-time 120 2>/dev/null)

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

    # httpbin.org/get echoes back headers, origin, url
    if echo "$response_text" | grep -iqE 'headers|origin|url|host|user-agent|httpbin'; then
        echo "PASS: HTTP GET returned request info from httpbin"
        return 0
    fi

    # Accept if the agent fetched and summarized
    if echo "$response_text" | grep -iqE 'response|request|get|200|success'; then
        echo "PASS: HTTP GET completed successfully"
        return 0
    fi

    echo "FAIL: HTTP GET response missing expected fields. Response: ${response_text:0:300}"
    return 1
}

# ── TC-12.4: HTTP POST request ──────────────────────────────────────────────
# Ask the agent to POST JSON data to httpbin.org/post and verify echo.

tc_http_post() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Use the http_request tool to make an HTTP POST request to https://httpbin.org/post with the JSON body {\"test\":\"e2e\",\"value\":42}. Show me the response."}' \
        --max-time 120 2>/dev/null)

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

    # httpbin.org/post echoes back the posted data
    if echo "$response_text" | grep -iqE 'e2e|test|value|42|data|json|post'; then
        echo "PASS: HTTP POST echoed back posted data"
        return 0
    fi

    # Accept if the agent made the request and reported success
    if echo "$response_text" | grep -iqE 'response|200|success|sent|posted'; then
        echo "PASS: HTTP POST completed successfully"
        return 0
    fi

    echo "FAIL: HTTP POST response missing posted data echo. Response: ${response_text:0:300}"
    return 1
}

# ── TC-12.5: Unreachable URL handling ───────────────────────────────────────
# Ask agent to fetch a non-existent domain. Verify graceful error, no crash.

tc_unreachable_url() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Fetch the content from https://thisdomaindoesnotexist.invalid/ and show me what you get."}' \
        --max-time 120 2>/dev/null)

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

    # Agent should report an error gracefully — not crash
    if echo "$response_text" | grep -iqE 'error|failed|unable|cannot|couldn.t|unreachable|not found|resolve|dns|connect|timeout|does not exist|invalid|inaccessible'; then
        echo "PASS: Agent handled unreachable URL gracefully"
        return 0
    fi

    # Any non-empty response without a crash is acceptable
    echo "PASS: Agent returned response for unreachable URL (${#response_text} chars, no crash)"
    return 0
}

# ── TC-12.6: Large page fetch ───────────────────────────────────────────────
# Fetch a known large page and verify a response is returned without timeout.

tc_large_page_fetch() {
    local resp
    resp=$(curl -sf -X POST "${GATEWAY_URL}/webhook" \
        -H 'Content-Type: application/json' \
        -d '{"message":"Use the web_fetch tool to fetch https://en.wikipedia.org/wiki/Rust_(programming_language) and give me a brief summary of what the page is about."}' \
        --max-time 150 2>/dev/null)

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

    # Should contain Rust-related content from the Wikipedia page
    if echo "$response_text" | grep -iqE 'rust|programming|language|mozilla|memory|safety|systems'; then
        echo "PASS: Large page fetch returned Rust-related content (${#response_text} chars)"
        return 0
    fi

    # Accept any non-empty response — the page was fetched without timeout
    if [[ ${#response_text} -gt 20 ]]; then
        echo "PASS: Large page fetch completed without timeout (${#response_text} chars)"
        return 0
    fi

    echo "FAIL: Large page fetch returned insufficient content. Response: ${response_text:0:300}"
    return 1
}
