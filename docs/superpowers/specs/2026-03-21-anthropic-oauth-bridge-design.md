# Anthropic OAuth Credential Bridge

**Date:** 2026-03-21
**Status:** Approved
**Author:** Claude + Sujeeth

## Problem

RustyClaw cannot use Anthropic OAuth tokens (`sk-ant-oat01-*`) directly because Anthropic restricts these to first-party apps (Claude Code, Claude.ai). The raw token passed via `ANTHROPIC_OAUTH_TOKEN` env var fails with 400/401 errors. Users on Claude Max subscriptions want to route RustyClaw through their subscription instead of paying per-token API pricing.

## Solution

A credential bridge that reads Claude Code's OAuth credentials, handles token refresh with write-back, and provides fresh access tokens to `AnthropicProvider`. This makes Claude Code's subscription the primary auth path for Anthropic.

## Reference Implementation

Based on [tezra-io/usg](https://github.com/tezra-io/usg.git) (`internal/claude/auth.go`), which implements the same pattern in Go.

## Design

### Credential Reading

New module: `src/auth/claude_code_oauth.rs`

**Public API:**

```rust
/// Returns a valid Anthropic OAuth access token by reading Claude Code's
/// credentials and refreshing if expired. Returns None if Claude Code
/// credentials are not available.
pub async fn get_valid_access_token() -> Option<String>

/// Force-refresh the access token regardless of expiry.
/// Called on 401 responses to handle server-side token revocation.
pub async fn force_refresh() -> Option<String>
```

The module creates its own internal `reqwest::Client` (with 15s timeout) for refresh calls.

**Credential sources (tried in order):**

1. **macOS Keychain** (`#[cfg(target_os = "macos")]`)
   - Service: `"Claude Code-credentials"`
   - Account: `"Claude Code"`
   - Command: `security find-generic-password -s "Claude Code-credentials" -w`
   - Falls back to treating raw keychain value as bare token if JSON parsing fails
2. **Credentials file**
   - Path: `~/.claude/.credentials.json`
   - Logs a warning if file permissions are broader than `0600`

**JSON format (both field name variants):**

```json
{
  "claude_ai_oauth": {
    "accessToken": "sk-ant-oat01-...",
    "refreshToken": "sk-ant-ort01-...",
    "expiresAt": 1711234567890,
    "scopes": ["usage:read"],
    "rateLimitTier": "pro"
  },
  "claudeAiOauth": null
}
```

- `claude_ai_oauth` (snake_case): typically in credentials file
- `claudeAiOauth` (camelCase): typically in keychain
- Both checked; first non-null entry wins

**Internal structs (not public):**

```rust
#[derive(Deserialize, Serialize)]
struct CredentialsFile {
    #[serde(default)]
    claude_ai_oauth: Option<OAuthEntry>,
    #[serde(default, rename = "claudeAiOauth")]
    claude_ai_oauth_camel: Option<OAuthEntry>,
}

#[derive(Deserialize, Serialize, Clone)]
struct OAuthEntry {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken", default)]
    refresh_token: Option<String>,
    #[serde(rename = "expiresAt", default)]
    expires_at: Option<f64>,  // milliseconds since epoch
    #[serde(default)]
    scopes: Option<Vec<String>>,
    #[serde(rename = "rateLimitTier", default)]
    rate_limit_tier: Option<String>,
}
```

### Token Refresh

**When to refresh (lazy check):**
- `expiresAt` is present AND current time >= `expiresAt - 5_000ms` (5s skew — smaller than the 90s used for RustyClaw's own tokens, to reduce race conditions with Claude Code sharing the same credential store)
- If `expiresAt` is missing or zero, assume token is valid

**Force refresh:**
- Called when the Anthropic API returns 401 (token revoked server-side before expiry)
- Ignores `expiresAt`, refreshes unconditionally
- Single attempt per 401; does not retry on subsequent 401s to avoid loops

**Refresh endpoint:**
- URL: `https://console.anthropic.com/v1/oauth/token`
- Method: `POST`
- Content-Type: `application/json`
- Timeout: 15 seconds
- Body: `{"grant_type": "refresh_token", "refresh_token": "<token>"}`

**Response:**

```json
{
  "access_token": "sk-ant-new-...",
  "refresh_token": "optional-rotated-token",
  "expires_in": 3600
}
```

**After successful refresh:**
- Update `accessToken` with new value
- Update `refreshToken` only if response includes one (token rotation)
- Compute `expiresAt` = `now_ms + expires_in * 1000`
- Write back to the same source (keychain or file)
- File writes use mode `0600`
- Keychain writes use `security add-generic-password -U -s "Claude Code-credentials" -a "Claude Code" -w <json>`

**Concurrency control:**
- A module-level `tokio::sync::Mutex` guards the refresh path, preventing duplicate concurrent refreshes (matches the `refresh_lock_for_profile()` pattern used by OpenAI/Gemini OAuth in `auth/mod.rs`)
- Both `get_valid_access_token()` and `force_refresh()` acquire the lock before checking expiry or refreshing

**Failure handling:**
- Log warning via `tracing::warn!`
- Return existing `accessToken` (graceful degradation)
- Single attempt, no retry loop (provider-level retries handle transient failures)

### Write-Back

Refreshed credentials are written back to the **same source** they were read from:

- **Keychain source:** `security add-generic-password -U -s "Claude Code-credentials" -a "Claude Code" -w <json>` (account name must match Claude Code's entry)
- **File source:** Write JSON with `0600` permissions to `~/.claude/.credentials.json`

This keeps Claude Code and RustyClaw in sync. Known race condition: if both refresh simultaneously, one may invalidate the other's refresh token. This is an accepted trade-off (same as usg).

### Provider Integration

**Credential resolution order in `AnthropicProvider::resolve_credential()`:**

1. **Direct credential** — from `ANTHROPIC_API_KEY` env var or config (explicit config always wins)
2. **Auth profile store** — `AuthService::get_provider_bearer_token("anthropic", ...)` (existing)
3. **Claude Code OAuth bridge** — `claude_code_oauth::get_valid_access_token()` (new, auto-discovery fallback)

When no explicit credential is configured (the common case for Max subscribers), the bridge activates automatically. Explicit `ANTHROPIC_API_KEY` takes priority if set.

**401 retry with force-refresh:**
When the Anthropic API returns 401 and the credential came from the OAuth bridge, the provider calls `claude_code_oauth::force_refresh()` and retries once. This handles server-side token revocation before `expiresAt`.

**Cleanup:**
- Remove `ANTHROPIC_OAUTH_TOKEN` from `resolve_provider_credential()` env var candidates (dead code — Anthropic blocks raw OAuth tokens from third-party apps)
- Update error messages in `anthropic.rs` to reference `ANTHROPIC_API_KEY` and Claude Code OAuth instead of `ANTHROPIC_OAUTH_TOKEN`

**Auth header selection** uses `detect_auth_kind()` from `anthropic_token.rs` (updated to handle `sk-ant-oat01-*` prefix in addition to existing JWT-shape and `sk-ant-api` detection):
- OAuth tokens (`sk-ant-oat01-*` or JWT-shaped): `Authorization: Bearer <token>` + `anthropic-beta: oauth-2025-04-20`
- API keys (`sk-ant-api*`): `x-api-key: <token>`

### Refresh Timing

Refresh is **lazy/on-demand** — triggered by each LLM API call via `resolve_credential()`. No startup check, no background daemon, no schedule. First request after token expiry incurs ~1-2s extra latency for the refresh round-trip.

## Files Changed

| File | Change | LOC |
|------|--------|-----|
| `src/auth/claude_code_oauth.rs` | **New** — credential reading, refresh, write-back, force-refresh, concurrency guard | ~250-300 |
| `src/auth/mod.rs` | Add `pub mod claude_code_oauth;` | ~1 |
| `src/auth/anthropic_token.rs` | Add `sk-ant-oat01-` prefix detection to `detect_auth_kind()` | ~3 |
| `src/providers/anthropic.rs` | Update `resolve_credential()`, use `detect_auth_kind()` in `apply_auth()`, add 401 force-refresh retry, update error messages | ~30 |
| `src/providers/mod.rs` | Remove `ANTHROPIC_OAUTH_TOKEN` from env var candidates | ~2 |

**Total:** ~300 LOC new, ~35 LOC modified. No new dependencies.

**Out of scope:** `src/onboard/wizard.rs` references to `ANTHROPIC_OAUTH_TOKEN` — the onboard wizard is a separate flow and will be updated in a follow-up.

## Testing

| Test | Type | Description |
|------|------|-------------|
| JSON parsing (snake_case) | Unit | Parse `claude_ai_oauth` field variant |
| JSON parsing (camelCase) | Unit | Parse `claudeAiOauth` field variant |
| JSON parsing (both present) | Unit | First non-null entry wins |
| Expiry detection | Unit | Expired, not expired, missing `expiresAt`, within 5s skew |
| Refresh response parsing | Unit | Valid response, missing optional fields |
| Refresh write-back | Unit | Verify updated JSON structure after refresh |
| Credential resolution order | Unit | Explicit key → auth profile → OAuth bridge fallback |
| Graceful degradation | Unit | Refresh failure returns existing token |
| Force refresh | Unit | Ignores expiresAt, refreshes unconditionally |
| File permission warning | Unit | Warns on overly permissive credential file |

Keychain tests are gated behind `#[cfg(target_os = "macos")]`. File-based tests use `tempfile` for isolation.

## Security Considerations

- Credentials file permissions enforced at `0600` on write-back
- Warning logged if `~/.claude/.credentials.json` has permissions broader than `0600`
- Refresh tokens are never logged (even at debug level)
- Keychain access may trigger macOS permission prompt on first use
- Keychain write-back uses `-a "Claude Code"` account name matching Claude Code's own entry
- No new network endpoints beyond `console.anthropic.com`
