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
#[cfg(target_os = "macos")]
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
            None
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
        oauth.access_token.clone_from(&refreshed.access_token);
        if let Some(ref new_refresh) = refreshed.refresh_token {
            oauth.refresh_token = Some(new_refresh.clone());
        }
        if let Some(expires_in) = refreshed.expires_in {
            oauth.expires_at = Some(now_ms() + expires_in * 1000.0);
        }
    }

    // Write back to source
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
        assert_eq!(entry.refresh_token.as_deref(), Some("sk-ant-ort01-refresh"));
        assert_eq!(entry.expires_at, Some(1_711_234_567_890.0));
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
            expires_at: Some(9_999_999_999_999.0),
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
            expires_at: Some(1_000_000_000_000.0),
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

    #[tokio::test]
    async fn get_valid_access_token_does_not_panic() {
        // On machines without Claude Code, returns None
        // On machines with Claude Code, returns a token
        // Either way, should not panic
        let _ = get_valid_access_token().await;
    }
}
