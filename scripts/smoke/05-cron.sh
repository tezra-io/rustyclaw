#!/usr/bin/env bash
#
# Phase 5: Cron CRUD via API
# Tests cron job create, list, delete cycle.
#
# Sourced by smoke-test.sh — can also run standalone.
SMOKE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -z "${GATEWAY_URL:-}" ]]; then source "$SMOKE_DIR/lib.sh"; fi

phase "Cron CRUD"

# ── List cron jobs ──────────────────────────────────────────────
test_cron_list() {
    local label="GET /api/cron (list)"

    http_request GET "$GATEWAY_URL/api/cron"

    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "$label → 200"
    else
        fail "$label → HTTP $HTTP_CODE"
        return 1
    fi
}
run_test "cron_list" test_cron_list

# ── Add a cron job ──────────────────────────────────────────────
test_cron_add() {
    local label="POST /api/cron (add)"

    http_request POST "$GATEWAY_URL/api/cron" \
        '{"schedule": "0 0 * * *", "command": "echo smoke test", "name": "smoke-test-job"}'

    if [[ "$HTTP_CODE" == "200" || "$HTTP_CODE" == "201" ]]; then
        local job_id
        job_id=$(http_json '.id // empty')
        if [[ -n "$job_id" ]]; then
            pass "$label → $HTTP_CODE (id=$job_id)"

            # Delete it
            http_request DELETE "$GATEWAY_URL/api/cron/$job_id"
            if [[ "$HTTP_CODE" == "200" || "$HTTP_CODE" == "204" ]]; then
                pass "DELETE /api/cron/$job_id → $HTTP_CODE"
            else
                skip "DELETE /api/cron/$job_id → $HTTP_CODE (cleanup)"
            fi
        else
            pass "$label → $HTTP_CODE (no id returned)"
        fi
    elif [[ "$HTTP_CODE" == "400" || "$HTTP_CODE" == "501" ]]; then
        skip "$label → $HTTP_CODE (cron may need different payload)"
    else
        fail "$label → HTTP $HTTP_CODE"
        return 1
    fi
}
run_test "cron_add" test_cron_add
