# Bridge Architecture Review — 2026-03-23

## Reviewer: Claude Code (second opinion)
## Requested by: Sujeeth

## Core Principle
Elixir and Rust should work like a single component even though they are modular.
Zero config for the user. Auto-discovery, auto-auth, auto-recovery.

## Recommended Architecture

### Startup Sequence
1. Daemon generates ephemeral credentials: `bridge_token` + `bridge_secret` (random 256-bit each)
2. Start gateway on port P
3. Spawn Elixir with env vars: RUSTYCLAW_BRIDGE_PORT, RUSTYCLAW_BRIDGE_TOKEN, RUSTYCLAW_BRIDGE_SECRET
4. Both health-check each other at 15s intervals

### Auth Model
- Rust→Elixir: X-Bridge-Secret header (existing)
- Elixir→Rust: X-Bridge-Token header (new, checked in require_auth before pairing check)
- Both tokens generated at daemon startup, ephemeral (die with the process)

### Key: Startup token > Loopback bypass
Loopback bypass is insecure — any local process can call bridge endpoints. Startup token is:
- Generated fresh each boot
- Passed via env vars (same UID only)
- Constant-time comparison
- Auto-revoked on restart

## New Issues Found (beyond TEZ-233-238)

### A. Bridge secret never auto-generated
elixir.rs passes RUSTYCLAW_BRIDGE_PORT but NOT RUSTYCLAW_BRIDGE_SECRET. User must set it manually.
Fix: Generate in daemon startup, pass to both sides.

### B. Plugin API binds to 0.0.0.0 (SECURITY)
application.ex PluginRouter has no `ip: {127, 0, 0, 1}`. Exposed to network.
Fix: Add loopback bind. One line.

### C. RustBridge sends no auth headers to gateway
build_req only sets content-type. Needs Authorization header with bridge_token.

### D. 30s gateway timeout kills bridge calls
REQUEST_TIMEOUT_SECS = 30 applies globally. /api/agent/run needs 300s for LLM+tools.
Fix: Per-route timeout layer in axum (already partially done for bridge_router).

### E. 64KB body limit on bridge endpoints
MAX_BODY_SIZE = 65536 applies globally. Long LLM responses from bridge will truncate.
Fix: Higher limit on bridge router.

### F. Port discovery is one-directional
Rust tells Elixir the gateway port, but Elixir ports (4001, 4002) are hardcoded in Rust.
Fix: Daemon assigns all ports, passes bidirectionally.

## Transport Recommendation
- v1 (now): Keep HTTP, fix auth/config issues
- v2 (later): Unix Domain Socket — removes TCP overhead, auth via filesystem permissions
- NOT Erlang Port — wrong for concurrent bidirectional calls

## Timeout Tiers
- External /api/*: 30s
- /api/agent/run: 300s (already has bridge_router with 5min timeout)
- /api/channel/send: 15s
- /health: 5s
