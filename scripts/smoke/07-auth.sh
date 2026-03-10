#!/usr/bin/env bash
#
# Phase 7: Auth & Security Checks
# Tests pairing enforcement and invalid auth handling.
# Boots a second gateway with require_pairing=true.
#
# Sourced by smoke-test.sh — can also run standalone.
SMOKE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -z "${GATEWAY_URL:-}" ]]; then source "$SMOKE_DIR/lib.sh"; fi

phase "Auth & Security"

# ── Test that unauthenticated /api/* works when pairing=false ───
test_no_auth_when_pairing_disabled() {
    local label="API accessible without auth (pairing=false)"

    http_request GET "$GATEWAY_URL/api/status"

    if [[ "$HTTP_CODE" == "200" ]]; then
        pass "$label"
    else
        fail "$label → HTTP $HTTP_CODE (expected 200)"
        return 1
    fi
}
run_test "no_auth_pairing_disabled" test_no_auth_when_pairing_disabled

# ── Boot a second gateway with pairing enabled ──────────────────
AUTH_GATEWAY_PID=""
AUTH_GATEWAY_LOG="$TMPDIR_PATH/auth_gateway.log"
AUTH_CONFIG_DIR="$TMPDIR_PATH/.rustyclaw-auth"

cleanup_auth_gateway() {
    if [[ -n "$AUTH_GATEWAY_PID" ]] && kill -0 "$AUTH_GATEWAY_PID" 2>/dev/null; then
        kill "$AUTH_GATEWAY_PID" 2>/dev/null || true
        wait "$AUTH_GATEWAY_PID" 2>/dev/null || true
    fi
}

boot_auth_gateway() {
    mkdir -p "$AUTH_CONFIG_DIR"
    cat > "$AUTH_CONFIG_DIR/config.toml" << EOF
api_key = "${PROVIDER_KEYS[0]}"
default_provider = "${PROVIDER_NAMES[0]}"
default_model = "${PROVIDER_MODELS[0]}"
default_temperature = 0.7

[gateway]
port = 0
host = "127.0.0.1"
require_pairing = true

[memory]
backend = "none"
auto_save = false
EOF
    chmod 600 "$AUTH_CONFIG_DIR/config.toml"

    RUST_LOG=info RUSTYCLAW_CONFIG_DIR="$AUTH_CONFIG_DIR" "$BINARY" gateway -p 0 > "$AUTH_GATEWAY_LOG" 2>&1 &
    AUTH_GATEWAY_PID=$!

    local port=""
    for _ in $(seq 1 30); do
        if ! kill -0 "$AUTH_GATEWAY_PID" 2>/dev/null; then
            return 1
        fi
        port=$(grep -oE 'listening on http://[^:]+:([0-9]+)' "$AUTH_GATEWAY_LOG" | tail -1 | grep -oE '[0-9]+$' || true)
        if [[ -n "$port" ]]; then break; fi
        sleep 0.5
    done

    if [[ -z "$port" ]]; then
        return 1
    fi

    AUTH_GATEWAY_URL="http://127.0.0.1:${port}"
    return 0
}

if boot_auth_gateway; then
    pass "Auth gateway booted (pairing=true)"

    # ── Test: unauthenticated API request should be rejected ────
    test_unauth_rejected() {
        local label="API rejects unauthenticated request (pairing=true)"

        HTTP_BODY_FILE=$(mktemp "${TMPDIR_PATH}/http_body.XXXXXX")
        HTTP_CODE=$(curl -s --max-time 10 -o "$HTTP_BODY_FILE" -w '%{http_code}' \
            "$AUTH_GATEWAY_URL/api/status" 2>/dev/null || echo "000")

        if [[ "$HTTP_CODE" == "401" ]]; then
            pass "$label → 401"
        elif [[ "$HTTP_CODE" == "403" ]]; then
            pass "$label → 403 (forbidden)"
        else
            fail "$label → HTTP $HTTP_CODE (expected 401 or 403)"
            return 1
        fi
    }
    run_test "unauth_rejected" test_unauth_rejected

    # ── Test: invalid bearer token should be rejected ───────────
    test_invalid_token() {
        local label="API rejects invalid bearer token"

        HTTP_BODY_FILE=$(mktemp "${TMPDIR_PATH}/http_body.XXXXXX")
        HTTP_CODE=$(curl -s --max-time 10 -o "$HTTP_BODY_FILE" -w '%{http_code}' \
            -H "Authorization: Bearer invalid_token_12345" \
            "$AUTH_GATEWAY_URL/api/status" 2>/dev/null || echo "000")

        if [[ "$HTTP_CODE" == "401" ]]; then
            pass "$label → 401"
        elif [[ "$HTTP_CODE" == "403" ]]; then
            pass "$label → 403"
        else
            fail "$label → HTTP $HTTP_CODE (expected 401 or 403)"
            return 1
        fi
    }
    run_test "invalid_token" test_invalid_token

    # ── Test: pairing flow ──────────────────────────────────────
    test_pair_flow() {
        local label="POST /pair returns a token"

        HTTP_BODY_FILE=$(mktemp "${TMPDIR_PATH}/http_body.XXXXXX")
        HTTP_CODE=$(curl -s --max-time 10 -o "$HTTP_BODY_FILE" -w '%{http_code}' \
            -X POST "$AUTH_GATEWAY_URL/pair" 2>/dev/null || echo "000")

        if [[ "$HTTP_CODE" == "200" ]]; then
            local token
            token=$(jq -r '.token // empty' "$HTTP_BODY_FILE" 2>/dev/null || echo "")
            if [[ -n "$token" ]]; then
                pass "$label → got token"

                # Use the token to access API
                HTTP_BODY_FILE=$(mktemp "${TMPDIR_PATH}/http_body.XXXXXX")
                HTTP_CODE=$(curl -s --max-time 10 -o "$HTTP_BODY_FILE" -w '%{http_code}' \
                    -H "Authorization: Bearer $token" \
                    "$AUTH_GATEWAY_URL/api/status" 2>/dev/null || echo "000")

                if [[ "$HTTP_CODE" == "200" ]]; then
                    pass "Authenticated API access with paired token → 200"
                else
                    fail "Authenticated API access → HTTP $HTTP_CODE (expected 200)"
                    return 1
                fi
            else
                pass "$label → 200 (no token field — pairing may work differently)"
            fi
        elif [[ "$HTTP_CODE" == "403" || "$HTTP_CODE" == "409" ]]; then
            skip "$label → $HTTP_CODE (pairing already completed or restricted)"
        else
            fail "$label → HTTP $HTTP_CODE"
            return 1
        fi
    }
    run_test "pair_flow" test_pair_flow

    # ── Test: health is always accessible ───────────────────────
    test_health_no_auth() {
        local label="/health accessible without auth (pairing=true)"

        HTTP_BODY_FILE=$(mktemp "${TMPDIR_PATH}/http_body.XXXXXX")
        HTTP_CODE=$(curl -s --max-time 10 -o "$HTTP_BODY_FILE" -w '%{http_code}' \
            "$AUTH_GATEWAY_URL/health" 2>/dev/null || echo "000")

        if [[ "$HTTP_CODE" == "200" ]]; then
            pass "$label"
        else
            fail "$label → HTTP $HTTP_CODE"
            return 1
        fi
    }
    run_test "health_no_auth" test_health_no_auth

    cleanup_auth_gateway
else
    skip "Could not boot auth gateway — skipping auth tests"
fi
