# Anthropic OAuth Credential Bridge — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Read Claude Code's OAuth credentials, handle token refresh with write-back, and provide fresh access tokens to RustyClaw's Anthropic provider — enabling Claude Max subscription usage without API keys.

**Architecture:** New `src/auth/claude_code_oauth.rs` module reads credentials from macOS Keychain or `~/.claude/.credentials.json`, refreshes expired tokens via Anthropic's OAuth endpoint, writes back to the source. `AnthropicProvider::resolve_credential()` tries explicit API key first, then auth profiles, then the OAuth bridge as auto-discovery fallback.

**Tech Stack:** Rust, reqwest (existing), serde (existing), tokio (existing). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-03-21-anthropic-oauth-bridge-design.md`

---

## File Structure

| File | Responsibility |
|------|---------------|
| `src/auth/claude_code_oauth.rs` | **New.** Read Claude Code credentials (keychain + file), check expiry, refresh tokens, write back. Public API: `get_valid_access_token()`, `force_refresh()` |
| `src/auth/anthropic_token.rs` | **Modify.** Add `sk-ant-oat01-` prefix detection to `detect_auth_kind()` |
| `src/auth/mod.rs` | **Modify.** Add `pub mod claude_code_oauth;` |
| `src/providers/anthropic.rs` | **Modify.** Update `resolve_credential()` to try OAuth bridge as fallback, replace `is_setup_token()` with `detect_auth_kind()`, update error messages |
| `src/providers/mod.rs` | **Modify.** Remove `ANTHROPIC_OAUTH_TOKEN` from env var candidates |

---

### Task 1: Update `detect_auth_kind()` to recognize OAuth tokens

**Files:**
- Modify: `src/auth/anthropic_token.rs:31-50` (add `sk-ant-oat01-` check)

- [ ] **Step 1: Write the failing test**

Add to `src/auth/anthropic_token.rs` in the `mod tests` block (after line 85):

```rust
#[test]
fn detect_setup_token_as_authorization() {
    let kind = detect_auth_kind("sk-ant-oat01-dFxyz123abc", None);
    assert_eq!(kind, AnthropicAuthKind::Authorization);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib auth::anthropic_token::tests::detect_setup_token_as_authorization -- --exact`
Expected: FAIL — currently returns `ApiKey` because `sk-ant-oat01-` doesn't match any existing pattern.

- [ ] **Step 3: Add the prefix check to `detect_auth_kind()`**

In `src/auth/anthropic_token.rs`, insert after line 45 (`if trimmed.starts_with("sk-ant-api")` block):

```rust
    // OAuth setup tokens (from Claude Code subscriptions).
    if trimmed.starts_with("sk-ant-oat01-") {
        return AnthropicAuthKind::Authorization;
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib auth::anthropic_token::tests`
Expected: All 4 tests pass (3 existing + 1 new).

- [ ] **Step 5: Commit**

```bash
git add src/auth/anthropic_token.rs
git commit -m "feat(auth): detect sk-ant-oat01- tokens as Authorization in detect_auth_kind"
```

---

### Task 2: Create `claude_code_oauth.rs` — structs and credential reading

**Files:**
- Create: `src/auth/claude_code_oauth.rs`
- Modify: `src/auth/mod.rs:1` (add module declaration)

- [ ] **Step 1: Write failing tests for JSON parsing**

Create `src/auth/claude_code_oauth.rs` with the test module and struct definitions. The tests come first, the public functions will be stubs:

```rust
//! Claude Code OAuth credential bridge.
//!
//! Reads Claude Code's OAuth credentials from macOS Keychain or
//! `~/.claude/.credentials.json`, refreshes expired tokens, and writes
//! back to the source. This enables RustyClaw to use Claude Max subscriptions.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

// ── Constants ───────────────────────────────────────────────────────

const REFRESH_ENDPOINT: &str = "https://console.anthropic.com/v1/oauth/token";
const REFRESH_TIMEOUT_SECS: u64 = 15;
const EXPIRY_SKEW_MS: f64 = 5_000.0; // 5s pre-expiry buffer
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const KEYCHAIN_ACCOUNT: &str = "Claude Code";

// ── Types ───────────────────────────────────────────────────────────

/// Which source the credentials were read from (for write-back).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialSource {
    Keychain,
    File,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct CredentialsFile {
    #[serde(default)]
    claude_ai_oauth: Option<OAuthEntry>,
    #[serde(default, rename = "claudeAiOauth")]
    claude_ai_oauth_camel: Option<OAuthEntry>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct OAuthEntry {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken", default)]
    refresh_token: Option<String>,
    #[serde(rename = "expiresAt", default)]
    expires_at: Option<f64>,
    #[serde(default)]
    scopes: Option<Vec<String>>,
    #[serde(rename = "rateLimitTier", default)]
    rate_limit_tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<f64>,
}

impl CredentialsFile {
    /// Return the first non-null OAuth entry (snake_case preferred).
    fn oauth_entry(&self) -> Option<&OAuthEntry> {
        self.claude_ai_oauth
            .as_ref()
            .or(self.claude_ai_oauth_camel.as_ref())
    }

    /// Return a mutable ref to the active OAuth entry.
    fn oauth_entry_mut(&mut self) -> Option<&mut OAuthEntry> {
        if self.claude_ai_oauth.is_some() {
            self.claude_ai_oauth.as_mut()
        } else {
            self.claude_ai_oauth_camel.as_mut()
        }
    }
}

impl OAuthEntry {
    fn is_expired(&self) -> bool {
        let Some(expires_at) = self.expires_at else {
            return false; // no expiry info → assume valid
        };
        if expires_at <= 0.0 {
            return false;
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as f64;
        now_ms >= (expires_at - EXPIRY_SKEW_MS)
    }
}

// ── Credential file path ────────────────────────────────────────────

fn credentials_file_path() -> Option<PathBuf> {
    directories::UserDirs::new().map(|u| u.home_dir().join(".claude").join(".credentials.json"))
}

// ── Public API (stubs for now) ──────────────────────────────────────

/// Returns a valid Anthropic OAuth access token by reading Claude Code's
/// credentials and refreshing if expired. Returns None if unavailable.
pub async fn get_valid_access_token() -> Option<String> {
    None // stub — implemented in Task 3
}

/// Force-refresh regardless of expiry (for 401 recovery).
pub async fn force_refresh() -> Option<String> {
    None // stub — implemented in Task 4
}

// ── Read credentials ────────────────────────────────────────────────

fn read_credentials_file() -> Option<(CredentialsFile, PathBuf)> {
    let path = credentials_file_path()?;
    let data = std::fs::read(&path).ok()?;
    if data.is_empty() {
        return None;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                tracing::warn!(
                    path = %path.display(),
                    mode = format!("{mode:04o}"),
                    "Claude Code credentials file has overly permissive permissions (expected 0600)"
                );
            }
        }
    }

    let creds: CredentialsFile = serde_json::from_slice(&data).ok()?;
    if creds.oauth_entry().is_some() {
        Some((creds, path))
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn read_keychain() -> Option<CredentialsFile> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    // Try JSON parse first; fall back to bare token
    match serde_json::from_str::<CredentialsFile>(raw) {
        Ok(creds) if creds.oauth_entry().is_some() => Some(creds),
        _ => {
            // Treat raw value as a bare access token
            Some(CredentialsFile {
                claude_ai_oauth: Some(OAuthEntry {
                    access_token: raw.to_string(),
                    refresh_token: None,
                    expires_at: None,
                    scopes: None,
                    rate_limit_tier: None,
                }),
                claude_ai_oauth_camel: None,
            })
        }
    }
}

/// Read credentials from the best available source.
fn read_credentials() -> Option<(CredentialsFile, CredentialSource)> {
    #[cfg(target_os = "macos")]
    {
        if let Some(creds) = read_keychain() {
            return Some((creds, CredentialSource::Keychain));
        }
    }
    let (creds, _path) = read_credentials_file()?;
    Some((creds, CredentialSource::File))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_snake_case_credentials() {
        let json = r#"{
            "claude_ai_oauth": {
                "accessToken": "sk-ant-oat01-test",
                "refreshToken": "sk-ant-ort01-refresh",
                "expiresAt": 1711234567890
            }
        }"#;
        let creds: CredentialsFile = serde_json::from_str(json).unwrap();
        let entry = creds.oauth_entry().unwrap();
        assert_eq!(entry.access_token, "sk-ant-oat01-test");
        assert_eq!(
            entry.refresh_token.as_deref(),
            Some("sk-ant-ort01-refresh")
        );
        assert_eq!(entry.expires_at, Some(1711234567890.0));
    }

    #[test]
    fn parse_camel_case_credentials() {
        let json = r#"{
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-camel",
                "refreshToken": "sk-ant-ort01-camel-refresh",
                "expiresAt": 9999999999999
            }
        }"#;
        let creds: CredentialsFile = serde_json::from_str(json).unwrap();
        let entry = creds.oauth_entry().unwrap();
        assert_eq!(entry.access_token, "sk-ant-oat01-camel");
    }

    #[test]
    fn parse_both_present_prefers_snake_case() {
        let json = r#"{
            "claude_ai_oauth": {
                "accessToken": "snake-wins",
                "expiresAt": 9999999999999
            },
            "claudeAiOauth": {
                "accessToken": "camel-loses",
                "expiresAt": 9999999999999
            }
        }"#;
        let creds: CredentialsFile = serde_json::from_str(json).unwrap();
        assert_eq!(creds.oauth_entry().unwrap().access_token, "snake-wins");
    }

    #[test]
    fn parse_empty_oauth_returns_none() {
        let json = r#"{}"#;
        let creds: CredentialsFile = serde_json::from_str(json).unwrap();
        assert!(creds.oauth_entry().is_none());
    }

    #[test]
    fn parse_null_oauth_returns_none() {
        let json = r#"{"claude_ai_oauth": null, "claudeAiOauth": null}"#;
        let creds: CredentialsFile = serde_json::from_str(json).unwrap();
        assert!(creds.oauth_entry().is_none());
    }

    #[test]
    fn expiry_missing_assumes_valid() {
        let entry = OAuthEntry {
            access_token: "token".into(),
            refresh_token: None,
            expires_at: None,
            scopes: None,
            rate_limit_tier: None,
        };
        assert!(!entry.is_expired());
    }

    #[test]
    fn expiry_zero_assumes_valid() {
        let entry = OAuthEntry {
            access_token: "token".into(),
            refresh_token: None,
            expires_at: Some(0.0),
            scopes: None,
            rate_limit_tier: None,
        };
        assert!(!entry.is_expired());
    }

    #[test]
    fn expiry_far_future_not_expired() {
        let entry = OAuthEntry {
            access_token: "token".into(),
            refresh_token: None,
            expires_at: Some(9_999_999_999_999.0), // year 2286
            scopes: None,
            rate_limit_tier: None,
        };
        assert!(!entry.is_expired());
    }

    #[test]
    fn expiry_in_past_is_expired() {
        let entry = OAuthEntry {
            access_token: "token".into(),
            refresh_token: None,
            expires_at: Some(1_000_000_000_000.0), // 2001
            scopes: None,
            rate_limit_tier: None,
        };
        assert!(entry.is_expired());
    }

    #[test]
    fn refresh_response_parses_full() {
        let json = r#"{
            "access_token": "new-token",
            "refresh_token": "new-refresh",
            "expires_in": 3600
        }"#;
        let resp: RefreshResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.access_token, "new-token");
        assert_eq!(resp.refresh_token.as_deref(), Some("new-refresh"));
        assert_eq!(resp.expires_in, Some(3600.0));
    }

    #[test]
    fn refresh_response_parses_minimal() {
        let json = r#"{"access_token": "just-the-token"}"#;
        let resp: RefreshResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.access_token, "just-the-token");
        assert!(resp.refresh_token.is_none());
        assert!(resp.expires_in.is_none());
    }

    #[test]
    fn oauth_entry_mut_follows_snake_case_priority() {
        let mut creds = CredentialsFile {
            claude_ai_oauth: Some(OAuthEntry {
                access_token: "original".into(),
                refresh_token: None,
                expires_at: None,
                scopes: None,
                rate_limit_tier: None,
            }),
            claude_ai_oauth_camel: None,
        };
        creds.oauth_entry_mut().unwrap().access_token = "updated".into();
        assert_eq!(
            creds.claude_ai_oauth.as_ref().unwrap().access_token,
            "updated"
        );
    }

    #[test]
    fn credentials_file_path_returns_expected() {
        let path = credentials_file_path();
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains(".claude"));
        assert!(path.to_string_lossy().ends_with(".credentials.json"));
    }
}
```

- [ ] **Step 2: Add module declaration to `src/auth/mod.rs`**

Insert at line 1 of `src/auth/mod.rs` (before `pub mod anthropic_token;`):

```rust
pub mod claude_code_oauth;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib auth::claude_code_oauth`
Expected: All 12 tests pass. The public stubs return `None` but aren't tested yet.

- [ ] **Step 4: Commit**

```bash
git add src/auth/claude_code_oauth.rs src/auth/mod.rs
git commit -m "feat(auth): add claude_code_oauth module with credential parsing and tests"
```

---

### Task 3: Implement `get_valid_access_token()` with refresh and write-back

**Files:**
- Modify: `src/auth/claude_code_oauth.rs` (replace stubs, add refresh + write-back)

- [ ] **Step 1: Write the failing test for graceful degradation**

Add to the `mod tests` block in `src/auth/claude_code_oauth.rs`:

```rust
#[tokio::test]
async fn get_valid_access_token_returns_none_without_credentials() {
    // No keychain, no file → should return None gracefully
    let result = get_valid_access_token().await;
    // On CI/machines without Claude Code, this returns None
    // On machines with Claude Code, this returns a valid token
    // Either way, it should not panic
    let _ = result;
}
```

- [ ] **Step 2: Implement the refresh lock, refresh function, and write-back**

Replace the `get_valid_access_token` and `force_refresh` stubs in `src/auth/claude_code_oauth.rs` with:

```rust
// ── Module-level refresh lock ───────────────────────────────────────

fn refresh_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
}

// ── Public API ──────────────────────────────────────────────────────

/// Returns a valid Anthropic OAuth access token by reading Claude Code's
/// credentials and refreshing if expired. Returns None if unavailable.
pub async fn get_valid_access_token() -> Option<String> {
    let _guard = refresh_lock().lock().await;
    let (mut creds, source) = read_credentials()?;
    let entry = creds.oauth_entry()?.clone();

    if !entry.is_expired() {
        return Some(entry.access_token);
    }

    // Token expired — try refresh
    match do_refresh(&entry, &mut creds, source).await {
        Some(new_token) => Some(new_token),
        None => {
            tracing::warn!("Claude Code OAuth refresh failed, returning existing token");
            Some(entry.access_token)
        }
    }
}

/// Force-refresh regardless of expiry (for 401 recovery).
pub async fn force_refresh() -> Option<String> {
    let _guard = refresh_lock().lock().await;
    let (mut creds, source) = read_credentials()?;
    let entry = creds.oauth_entry()?.clone();

    match do_refresh(&entry, &mut creds, source).await {
        Some(new_token) => Some(new_token),
        None => {
            tracing::warn!("Claude Code OAuth force-refresh failed");
            None // force-refresh failure is not gracefully degraded
        }
    }
}

// ── Refresh implementation ──────────────────────────────────────────

async fn do_refresh(
    entry: &OAuthEntry,
    creds: &mut CredentialsFile,
    source: CredentialSource,
) -> Option<String> {
    let refresh_token = entry.refresh_token.as_deref()?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(REFRESH_TIMEOUT_SECS))
        .build()
        .ok()?;

    let payload = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
    });

    let resp = client
        .post(REFRESH_ENDPOINT)
        .header("content-type", "application/json")
        .json(&payload)
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!(
            status = %status,
            "Claude Code OAuth refresh request failed: {body}"
        );
        return None;
    }

    let refreshed: RefreshResponse = resp.json().await.ok()?;

    // Update the credential entry
    if let Some(oauth) = creds.oauth_entry_mut() {
        oauth.access_token = refreshed.access_token.clone();
        if let Some(new_refresh) = &refreshed.refresh_token {
            oauth.refresh_token = Some(new_refresh.clone());
        }
        if let Some(expires_in) = refreshed.expires_in {
            oauth.expires_at = Some(now_ms() + expires_in * 1000.0);
        }
    }

    // Write back
    write_credentials(creds, source);

    tracing::info!("Claude Code OAuth token refreshed successfully");
    Some(refreshed.access_token)
}

// ── Write-back ──────────────────────────────────────────────────────

fn write_credentials(creds: &CredentialsFile, source: CredentialSource) {
    match source {
        CredentialSource::File => write_credentials_file(creds),
        CredentialSource::Keychain => {
            #[cfg(target_os = "macos")]
            write_keychain(creds);
            #[cfg(not(target_os = "macos"))]
            write_credentials_file(creds);
        }
    }
}

fn write_credentials_file(creds: &CredentialsFile) {
    let Some(path) = credentials_file_path() else {
        return;
    };
    let Ok(data) = serde_json::to_string_pretty(creds) else {
        return;
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
        {
            use std::io::Write;
            let _ = file.write_all(data.as_bytes());
        }
    }

    #[cfg(not(unix))]
    {
        let _ = std::fs::write(&path, data);
    }
}

#[cfg(target_os = "macos")]
fn write_keychain(creds: &CredentialsFile) {
    let Ok(data) = serde_json::to_string(creds) else {
        return;
    };
    let _ = std::process::Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            KEYCHAIN_ACCOUNT,
            "-w",
            &data,
        ])
        .output();
}
```

- [ ] **Step 3: Run all module tests**

Run: `cargo test --lib auth::claude_code_oauth`
Expected: All 13 tests pass.

- [ ] **Step 4: Run full test suite to check nothing breaks**

Run: `cargo test --quiet`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/auth/claude_code_oauth.rs
git commit -m "feat(auth): implement Claude Code OAuth refresh and write-back"
```

---

### Task 4: Wire OAuth bridge into `AnthropicProvider`

**Files:**
- Modify: `src/providers/anthropic.rs:196-234` (resolve_credential, apply_auth, error messages, 401 retry)

- [ ] **Step 1: Update `resolve_credential()` to return credential source**

In `src/providers/anthropic.rs`, change `resolve_credential()` to return whether the credential came from the OAuth bridge. Replace lines 196-216:

```rust
    /// Resolve the best available credential.
    /// Returns (credential, from_oauth_bridge) to enable 401 force-refresh.
    /// Resolution order:
    /// 1. Directly provided credential (from env var or config)
    /// 2. Auth profile from auth-profiles.json (via AuthService)
    /// 3. Claude Code OAuth bridge (auto-discovery fallback)
    async fn resolve_credential(&self) -> Option<(String, bool)> {
        // Direct credential takes priority (ANTHROPIC_API_KEY or config)
        if let Some(ref cred) = self.credential {
            return Some((cred.clone(), false));
        }

        // Try auth service for managed profiles
        if let Some(auth_service) = &self.auth_service {
            if let Ok(Some(token)) = auth_service
                .get_provider_bearer_token("anthropic", self.auth_profile_override.as_deref())
                .await
            {
                return Some((token, false));
            }
        }

        // Auto-discover Claude Code OAuth credentials
        let token = crate::auth::claude_code_oauth::get_valid_access_token().await?;
        Some((token, true))
    }
```

- [ ] **Step 2: Update all callers of `resolve_credential()` to destructure the tuple**

In each `chat_with_system()`, `chat()`, `chat_with_tools()`, and `warmup()` method, update the credential retrieval from:
```rust
let credential = self.resolve_credential().await.ok_or_else(|| { ... })?;
```
to:
```rust
let (credential, from_oauth_bridge) = self.resolve_credential().await.ok_or_else(|| { ... })?;
```

For `warmup()` which uses `if let Some(credential)`, change to:
```rust
if let Some((credential, _from_oauth_bridge)) = self.resolve_credential().await {
```

- [ ] **Step 3: Add 401 retry with force-refresh to `chat()` and `chat_with_system()`**

After the `response.status().is_success()` check in each chat method, add 401 retry logic. For example in `chat_with_system()`, replace:

```rust
if !response.status().is_success() {
    return Err(super::api_error("Anthropic", response).await);
}
```

with:

```rust
if !response.status().is_success() {
    // On 401 with OAuth bridge credentials, try force-refresh once
    if response.status() == reqwest::StatusCode::UNAUTHORIZED && from_oauth_bridge {
        if let Some(new_token) = crate::auth::claude_code_oauth::force_refresh().await {
            let retry_request = ChatRequest {
                model: model.to_string(),
                max_tokens: 4096,
                system: system_prompt.map(ToString::to_string),
                messages: vec![Message {
                    role: "user".to_string(),
                    content: message.to_string(),
                }],
                temperature,
            };
            let mut retry = self
                .http_client()
                .post(format!("{}/v1/messages", self.base_url))
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&retry_request);
            retry = self.apply_auth(retry, &new_token);
            let retry_response = retry.send().await?;
            if retry_response.status().is_success() {
                let chat_response: ChatResponse = retry_response.json().await?;
                return Self::parse_text_response(chat_response);
            }
            return Err(super::api_error("Anthropic", retry_response).await);
        }
    }
    return Err(super::api_error("Anthropic", response).await);
}
```

Apply similar logic to `chat()` (note: `chat()` uses `NativeChatRequest` not `ChatRequest`, so the retry must reconstruct the native request — copy the request-building code from above the original send). `chat_with_tools()` delegates to `chat()` so it inherits the retry automatically.

- [ ] **Step 4: Replace `is_setup_token()` with `detect_auth_kind()` in `apply_auth()`**

Replace lines 218-234 with:

```rust
    fn apply_auth(
        &self,
        request: reqwest::RequestBuilder,
        credential: &str,
    ) -> reqwest::RequestBuilder {
        use crate::auth::anthropic_token::{detect_auth_kind, AnthropicAuthKind};
        match detect_auth_kind(credential, None) {
            AnthropicAuthKind::Authorization => request
                .header("Authorization", format!("Bearer {credential}"))
                .header("anthropic-beta", "oauth-2025-04-20"),
            AnthropicAuthKind::ApiKey => request.header("x-api-key", credential),
        }
    }
```

Remove the now-unused `is_setup_token()` method (lines 218-220) and its test `setup_token_detection_works` (lines 690-694). The `apply_auth_uses_bearer_and_beta_for_setup_tokens` test (line 696+) should still pass since `detect_auth_kind` handles the same token format.

- [ ] **Step 5: Update error messages**

Replace the two error messages at lines 476-478 and 518-520:

From:
```
"Anthropic credentials not set. Set ANTHROPIC_API_KEY, ANTHROPIC_OAUTH_TOKEN, or run `rustyclaw auth login --provider anthropic`."
```

To:
```
"Anthropic credentials not set. Set ANTHROPIC_API_KEY, install Claude Code for OAuth auto-discovery, or run `rustyclaw auth login --provider anthropic`."
```

- [ ] **Step 6: Add test for credential resolution order**

Add to the test module in `src/providers/anthropic.rs`:

```rust
#[tokio::test]
async fn resolve_credential_prefers_direct_over_bridge() {
    // Provider with explicit credential should return it (not from bridge)
    let provider = AnthropicProvider::new(Some("sk-ant-api03-test"));
    let result = provider.resolve_credential().await;
    assert!(result.is_some());
    let (cred, from_bridge) = result.unwrap();
    assert_eq!(cred, "sk-ant-api03-test");
    assert!(!from_bridge);
}

#[tokio::test]
async fn resolve_credential_falls_through_to_bridge() {
    // Provider with no credential — bridge returns what it can (may be None on CI)
    let provider = AnthropicProvider::new(None);
    let result = provider.resolve_credential().await;
    // On machines with Claude Code: Some((token, true))
    // On CI without Claude Code: None
    if let Some((_token, from_bridge)) = result {
        assert!(from_bridge);
    }
}
```

- [ ] **Step 7: Run clippy and tests**

Run: `cargo clippy --all-targets -- -D warnings && cargo test --quiet`
Expected: Zero warnings, all tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/providers/anthropic.rs
git commit -m "feat(providers): wire Claude Code OAuth bridge into AnthropicProvider with 401 retry"
```

---

### Task 5: Remove `ANTHROPIC_OAUTH_TOKEN` from env var candidates and docstrings

**Files:**
- Modify: `src/providers/mod.rs:782-786,812`

- [ ] **Step 1: Update the env var candidate list**

In `src/providers/mod.rs`, change line 812:

From:
```rust
        "anthropic" => vec!["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"],
```

To:
```rust
        "anthropic" => vec!["ANTHROPIC_API_KEY"],
```

- [ ] **Step 2: Update docstring references**

In `src/providers/mod.rs`, update the docstring near lines 782-786. Change references from `ANTHROPIC_OAUTH_TOKEN` to reflect the new OAuth bridge approach. Replace:

```
/// 2. Provider-specific environment variable (e.g., `ANTHROPIC_OAUTH_TOKEN`, `OPENROUTER_API_KEY`)
```

with:

```
/// 2. Provider-specific environment variable (e.g., `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`)
```

And update the Anthropic-specific doc line from:

```
/// For Anthropic, the provider-specific env var is `ANTHROPIC_OAUTH_TOKEN` (for setup-tokens)
/// followed by `ANTHROPIC_API_KEY` (for regular API keys).
```

to:

```
/// For Anthropic, the provider-specific env var is `ANTHROPIC_API_KEY`.
/// Claude Code OAuth credentials are auto-discovered separately via `claude_code_oauth` module.
```

- [ ] **Step 3: Run clippy and tests**

Run: `cargo clippy --all-targets -- -D warnings && cargo test --quiet`
Expected: Zero warnings, all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/providers/mod.rs
git commit -m "chore(providers): remove ANTHROPIC_OAUTH_TOKEN references, update docs for OAuth bridge"
```

---

### Task 6: Full validation

- [ ] **Step 1: Run `cargo fmt`**

Run: `cargo fmt --all -- --check`
Expected: No formatting issues.

- [ ] **Step 2: Run `cargo clippy`**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: Zero warnings.

- [ ] **Step 3: Run full test suite**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 4: Manual smoke test (if Claude Code is installed)**

Run: `ANTHROPIC_API_KEY= rustyclaw gateway` (unset API key to force OAuth bridge)
Expected: Gateway starts, can send a message via webhook, gets a response from Anthropic using Claude Code's credentials.

- [ ] **Step 5: Final commit if any formatting fixes were needed**

```bash
git add src/auth/claude_code_oauth.rs src/auth/anthropic_token.rs src/auth/mod.rs src/providers/anthropic.rs src/providers/mod.rs
git commit -m "chore: format and final cleanup for Claude Code OAuth bridge"
```
