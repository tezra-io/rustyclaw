#!/usr/bin/env bash
#
# Phase 2: Multi-Provider Chat Round-Trips
# Detects available providers from env, runs chat for each.
# Skips missing providers — never fails for absent keys.
#
# Sourced by smoke-test.sh — can also run standalone.
SMOKE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -z "${GATEWAY_URL:-}" ]]; then source "$SMOKE_DIR/lib.sh"; fi

phase "Multi-Provider Chat"

# ── Provider detection ──────────────────────────────────────────
PROV_NAMES=()
PROV_KEYS=()
PROV_MODELS=()

if [[ -n "${OPENROUTER_API_KEY:-}" ]]; then
    PROV_NAMES+=("openrouter")
    PROV_KEYS+=("$OPENROUTER_API_KEY")
    PROV_MODELS+=("anthropic/claude-sonnet-4")
fi
if [[ -n "${ANTHROPIC_API_KEY:-}" ]]; then
    PROV_NAMES+=("anthropic")
    PROV_KEYS+=("$ANTHROPIC_API_KEY")
    PROV_MODELS+=("claude-sonnet-4-20250514")
elif [[ -n "${ANTHROPIC_OAUTH_TOKEN:-}" ]]; then
    PROV_NAMES+=("anthropic")
    PROV_KEYS+=("$ANTHROPIC_OAUTH_TOKEN")
    PROV_MODELS+=("claude-sonnet-4-20250514")
fi
if [[ -n "${OPENAI_API_KEY:-}" ]]; then
    PROV_NAMES+=("openai")
    PROV_KEYS+=("$OPENAI_API_KEY")
    PROV_MODELS+=("gpt-4o-mini")
fi

if [[ ${#PROV_NAMES[@]} -eq 0 ]]; then
    skip "No provider keys found — skipping chat tests"
    return 0 2>/dev/null || true
fi

info "Detected ${#PROV_NAMES[@]} provider(s): ${PROV_NAMES[*]}"

# ── Chat round-trip per provider ────────────────────────────────
run_chat_test() {
    local provider_name="$1"
    local provider_key="$2"
    local model="$3"
    local label="Chat ($provider_name / $model)"

    # Reconfigure gateway for this provider
    cat > "$CONFIG_DIR/config.toml" << EOF
api_key = "$provider_key"
default_provider = "$provider_name"
default_model = "$model"
default_temperature = 0.7

[gateway]
port = 0
host = "127.0.0.1"
require_pairing = false

[memory]
backend = "none"
auto_save = false
EOF
    chmod 600 "$CONFIG_DIR/config.toml"

    # Restart gateway with new config
    kill "$GATEWAY_PID" 2>/dev/null || true
    wait "$GATEWAY_PID" 2>/dev/null || true
    > "$GATEWAY_LOG"

    RUST_LOG=info RUSTYCLAW_CONFIG_DIR="$CONFIG_DIR" "$BINARY" gateway -p 0 > "$GATEWAY_LOG" 2>&1 &
    GATEWAY_PID=$!
    export GATEWAY_PID

    # Wait for port
    local port=""
    for _ in $(seq 1 30); do
        if ! kill -0 "$GATEWAY_PID" 2>/dev/null; then
            fail "$label — gateway failed to restart"
            return 1
        fi
        port=$(grep -oE 'listening on http://[^:]+:([0-9]+)' "$GATEWAY_LOG" | tail -1 | grep -oE '[0-9]+$' || true)
        if [[ -n "$port" ]]; then break; fi
        sleep 0.5
    done
    if [[ -z "$port" ]]; then
        fail "$label — gateway did not report port"
        return 1
    fi
    GATEWAY_URL="http://127.0.0.1:${port}"
    export GATEWAY_URL

    # Send chat request
    http_request POST "$GATEWAY_URL/webhook" '{"message": "Say hello in exactly 3 words"}'

    if [[ "$HTTP_CODE" == "000" ]]; then
        fail "$label — request timed out"
        return 1
    fi

    local response_text
    response_text=$(http_json '.response // empty')

    if [[ -n "$response_text" ]]; then
        pass "$label → \"${response_text:0:80}\""
    else
        local error_text
        error_text=$(http_json '.error // empty')
        if [[ -n "$error_text" ]]; then
            fail "$label — error: ${error_text:0:120}"
        else
            fail "$label — no 'response' field in body"
        fi
        return 1
    fi
}

for i in "${!PROV_NAMES[@]}"; do
    run_chat_test "${PROV_NAMES[$i]}" "${PROV_KEYS[$i]}" "${PROV_MODELS[$i]}" || true
done
