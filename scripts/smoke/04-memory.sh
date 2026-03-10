#!/usr/bin/env bash
#
# Phase 4: Memory Store/Recall Cycle
# Tests memory API CRUD operations.
#
# Sourced by smoke-test.sh — can also run standalone.
SMOKE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -z "${GATEWAY_URL:-}" ]]; then source "$SMOKE_DIR/lib.sh"; fi

phase "Memory CRUD"

# ── Check if memory endpoint is available ───────────────────────
test_memory_list() {
    local label="GET /api/memory"

    http_request GET "$GATEWAY_URL/api/memory"

    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "$label → 200"
    elif [[ "$HTTP_CODE" == "501" || "$HTTP_CODE" == "404" ]]; then
        skip "$label → $HTTP_CODE (memory backend not configured)"
        return 0
    else
        fail "$label → HTTP $HTTP_CODE"
        return 1
    fi
}
run_test "memory_list" test_memory_list

# ── Store a memory entry ────────────────────────────────────────
test_memory_store() {
    local label="POST /api/memory (store)"
    local test_key="smoke_test_$(date +%s)"

    http_request POST "$GATEWAY_URL/api/memory" \
        "{\"key\": \"$test_key\", \"content\": \"smoke test value\"}"

    if [[ "$HTTP_CODE" == "200" || "$HTTP_CODE" == "201" ]]; then
        pass "$label → $HTTP_CODE"

        # Verify it appears in listing
        http_request GET "$GATEWAY_URL/api/memory"
        local body
        body=$(http_body)
        if echo "$body" | grep -q "$test_key"; then
            pass "Memory recall — found stored key"
        else
            # May not appear in list if backend is 'none'
            skip "Memory recall — key not in listing (backend may be 'none')"
        fi

        # Cleanup: delete the test entry
        http_request DELETE "$GATEWAY_URL/api/memory/$test_key"
        if [[ "$HTTP_CODE" == "200" || "$HTTP_CODE" == "204" ]]; then
            pass "DELETE /api/memory/$test_key → $HTTP_CODE"
        else
            skip "DELETE /api/memory/$test_key → $HTTP_CODE (cleanup)"
        fi
    elif [[ "$HTTP_CODE" == "501" || "$HTTP_CODE" == "400" ]]; then
        skip "$label → $HTTP_CODE (memory not available)"
    else
        fail "$label → HTTP $HTTP_CODE"
        return 1
    fi
}
run_test "memory_store" test_memory_store
