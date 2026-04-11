#!/usr/bin/env bash
#
# Local agent orchestration test harness.
# Tests: spawn -> list -> message -> kill -> auth rejection
#
# Prerequisites:
#   - Elixir orchestrator running on SYNTH_PORT (default 4001)
#   - RUSTYCLAW_BRIDGE_SECRET set
#
# Usage:
#   RUSTYCLAW_BRIDGE_SECRET=mysecret ./scripts/test-agents.sh
#
set -euo pipefail

SYNTH_PORT="${SYNTH_PORT:-4001}"
BASE="http://127.0.0.1:${SYNTH_PORT}"
SECRET="${RUSTYCLAW_BRIDGE_SECRET:-}"
PASS=0
FAIL=0

# ── Require secret ──────────────────────────────────────────────
if [ -z "$SECRET" ]; then
    echo "ERROR: Set RUSTYCLAW_BRIDGE_SECRET before running"
    exit 1
fi

auth_header="x-bridge-secret: ${SECRET}"

# ── Helpers ─────────────────────────────────────────────────────
check() {
    local name="$1" expected="$2" actual="$3"
    if echo "$actual" | grep -q "$expected"; then
        echo "  PASS: $name"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: $name (expected '$expected')"
        echo "    got: $actual"
        FAIL=$((FAIL + 1))
    fi
}

echo "=== RustyClaw Agent Test Harness ==="
echo "  Target: $BASE"
echo ""

# ── 1. Health check ─────────────────────────────────────────────
echo "1. Health check"
resp=$(curl -sf "${BASE}/health" 2>&1 || echo "CURL_FAILED")
check "GET /health returns ok" "ok" "$resp"

# ── 2. Spawn researcher ────────────────────────────────────────
echo "2. Spawn researcher"
resp=$(curl -sf -X POST "${BASE}/api/agents/spawn" \
    -H "Content-Type: application/json" \
    -H "$auth_header" \
    -d '{"name":"researcher","capabilities":["research"]}' 2>&1 || echo "CURL_FAILED")
check "POST /api/agents/spawn (researcher)" '"ok":true' "$resp"

# ── 3. Spawn coder ─────────────────────────────────────────────
echo "3. Spawn coder"
resp=$(curl -sf -X POST "${BASE}/api/agents/spawn" \
    -H "Content-Type: application/json" \
    -H "$auth_header" \
    -d '{"name":"coder","capabilities":["coding"]}' 2>&1 || echo "CURL_FAILED")
check "POST /api/agents/spawn (coder)" '"ok":true' "$resp"

# ── 4. List agents ─────────────────────────────────────────────
echo "4. List agents"
resp=$(curl -sf "${BASE}/api/agents" \
    -H "$auth_header" 2>&1 || echo "CURL_FAILED")
check "GET /api/agents count=2" '"count":2' "$resp"

# ── 5. Filter by capability ────────────────────────────────────
echo "5. Filter by capability"
resp=$(curl -sf "${BASE}/api/agents?capability=research" \
    -H "$auth_header" 2>&1 || echo "CURL_FAILED")
check "GET /api/agents?capability=research count=1" '"count":1' "$resp"

# ── 6. Async message ───────────────────────────────────────────
echo "6. Async message"
resp=$(curl -sf -X POST "${BASE}/api/agents/message" \
    -H "Content-Type: application/json" \
    -H "$auth_header" \
    -d '{"target":"researcher","message":"ping","mode":"async"}' 2>&1 || echo "CURL_FAILED")
check "POST /api/agents/message (async ping)" '"ok":true' "$resp"

# ── 7. Kill researcher ─────────────────────────────────────────
echo "7. Kill researcher"
resp=$(curl -sf -X DELETE "${BASE}/api/agents/researcher" \
    -H "$auth_header" 2>&1 || echo "CURL_FAILED")
check "DELETE /api/agents/researcher" '"ok":true' "$resp"

# ── 8. Verify kill ──────────────────────────────────────────────
echo "8. Verify kill"
resp=$(curl -sf "${BASE}/api/agents" \
    -H "$auth_header" 2>&1 || echo "CURL_FAILED")
check "GET /api/agents count=1 after kill" '"count":1' "$resp"

# ── 9. Kill remaining (cleanup, no assertion) ──────────────────
echo "9. Cleanup: kill coder"
curl -sf -X DELETE "${BASE}/api/agents/coder" \
    -H "$auth_header" > /dev/null 2>&1 || true
echo "  (cleanup, no assertion)"

# ── 10. Auth rejection ─────────────────────────────────────────
echo "10. Auth rejection (no secret)"
http_code=$(curl -s -o /dev/null -w "%{http_code}" -X POST "${BASE}/api/agents/spawn" \
    -H "Content-Type: application/json" \
    -d '{"name":"unauthorized","capabilities":["nope"]}' 2>&1)
if [ "$http_code" = "401" ] || [ "$http_code" = "403" ]; then
    echo "  PASS: Unauthenticated request rejected (HTTP $http_code)"
    PASS=$((PASS + 1))
else
    echo "  FAIL: Expected HTTP 401 or 403, got $http_code"
    FAIL=$((FAIL + 1))
fi

# ── Results ─────────────────────────────────────────────────────
echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -gt 0 ] && exit 1
exit 0
