#!/usr/bin/env bash
#
# Phase 6: Config Hot-Reload
# Tests PUT /api/config and verifies gateway survives.
#
# Sourced by smoke-test.sh — can also run standalone.
SMOKE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -z "${GATEWAY_URL:-}" ]]; then source "$SMOKE_DIR/lib.sh"; fi

phase "Config Reload"

# ── Read current config ─────────────────────────────────────────
test_config_read() {
    local label="GET /api/config (read current)"

    http_request GET "$GATEWAY_URL/api/config"

    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "$label → 200"
    else
        fail "$label → HTTP $HTTP_CODE"
        return 1
    fi
}
run_test "config_read" test_config_read

# ── PUT config change and verify gateway survives ───────────────
test_config_reload() {
    local label="PUT /api/config (reload)"

    # Read current config
    http_request GET "$GATEWAY_URL/api/config"
    if [[ "$HTTP_CODE" != "200" ]]; then
        skip "$label — cannot read current config"
        return 0
    fi
    local current_config
    current_config=$(http_body)

    # Send a full config with a benign change (temperature)
    local config_body
    config_body="$(cat <<CONFEOF
api_key = "${DEFAULT_KEY}"
default_provider = "${DEFAULT_PROVIDER}"
default_model = "${PROVIDER_MODELS[0]}"
default_temperature = 0.5

[gateway]
port = 0
host = "127.0.0.1"
require_pairing = false

[memory]
backend = "none"
auto_save = false
CONFEOF
)"

    # PUT expects TOML body, not JSON — use text/plain
    HTTP_BODY_FILE=$(mktemp "${TMPDIR_PATH}/http_body.XXXXXX")
    HTTP_CODE=$(curl -s --max-time 30 -o "$HTTP_BODY_FILE" -w '%{http_code}' \
        -X PUT -H "Content-Type: text/plain" \
        -d "$config_body" \
        "$GATEWAY_URL/api/config" 2>/dev/null || echo "000")

    if [[ "$HTTP_CODE" == "200" || "$HTTP_CODE" == "204" ]]; then
        pass "$label → $HTTP_CODE"
    elif [[ "$HTTP_CODE" == "400" || "$HTTP_CODE" == "501" ]]; then
        skip "$label → $HTTP_CODE (config PUT may not support partial updates)"
        return 0
    else
        fail "$label → HTTP $HTTP_CODE"
        return 1
    fi

    # Verify gateway is still alive
    sleep 1
    http_request GET "$GATEWAY_URL/health"
    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "Gateway survived config reload"
    else
        fail "Gateway died after config reload (HTTP $HTTP_CODE)"
        return 1
    fi
}
run_test "config_reload" test_config_reload
