//! Unix Domain Socket bridge for Rust ↔ Elixir communication.
//!
//! Provides:
//! - UDS listener creation with stale socket detection
//! - Socket path resolution from config directory
//! - HTTP-over-UDS client helpers for Rust → Elixir calls
//! - Socket lifecycle management (permissions, cleanup)

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use tokio::time::Duration;

const BRIDGE_SOCKET_NAME: &str = "bridge.sock";
const ELIXIR_SOCKET_NAME: &str = "elixir.sock";

/// Body size limit for UDS bridge endpoints (1 MB).
pub const BRIDGE_BODY_SIZE: usize = 1_048_576;

/// Timeout for UDS bridge endpoints (300s — allows LLM calls with tool use).
pub const BRIDGE_TIMEOUT_SECS: u64 = 300;

/// Global store for the Elixir socket path (set by daemon, read by channels).
static ELIXIR_SOCKET: OnceLock<PathBuf> = OnceLock::new();

// ── Socket Path Resolution ──────────────────────────────────────

/// Resolve the bridge socket path (Rust listens, Elixir connects).
/// Located in the config directory, e.g. `~/.rustyclaw/bridge.sock`.
pub fn bridge_socket_path(config: &crate::config::Config) -> PathBuf {
    config_dir(config).join(BRIDGE_SOCKET_NAME)
}

/// Resolve the Elixir socket path (Elixir listens, Rust connects).
/// Located in the config directory, e.g. `~/.rustyclaw/elixir.sock`.
pub fn elixir_socket_path(config: &crate::config::Config) -> PathBuf {
    config_dir(config).join(ELIXIR_SOCKET_NAME)
}

fn config_dir(config: &crate::config::Config) -> PathBuf {
    config
        .config_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Register the Elixir socket path globally (called by daemon at startup).
pub fn register_elixir_socket(path: PathBuf) {
    let _ = ELIXIR_SOCKET.set(path);
}

/// Get the registered Elixir socket path, if set and the file exists.
pub fn registered_elixir_socket() -> Option<&'static Path> {
    ELIXIR_SOCKET
        .get()
        .map(|p| p.as_path())
        .filter(|p| p.exists())
}

// ── UDS Listener ────────────────────────────────────────────────

/// Create a Unix domain socket listener, cleaning up any stale socket file.
///
/// Sets permissions to `0600` (owner read/write only) for security.
/// Returns an error if another daemon is actively using the socket.
#[cfg(unix)]
pub async fn create_uds_listener(socket_path: &Path) -> Result<tokio::net::UnixListener> {
    if socket_path.exists() {
        // Try connecting — if it succeeds, another process owns this socket.
        match tokio::net::UnixStream::connect(socket_path).await {
            Ok(_) => {
                anyhow::bail!(
                    "Socket {} is in use by another process",
                    socket_path.display()
                );
            }
            Err(_) => {
                tracing::info!(
                    path = %socket_path.display(),
                    "Removing stale socket file"
                );
                tokio::fs::remove_file(socket_path).await.with_context(|| {
                    format!("Failed to remove stale socket {}", socket_path.display())
                })?;
            }
        }
    }

    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }

    let listener = tokio::net::UnixListener::bind(socket_path)
        .with_context(|| format!("Failed to bind UDS at {}", socket_path.display()))?;

    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;

    Ok(listener)
}

/// Remove a socket file if it exists.
pub async fn cleanup_socket(socket_path: &Path) {
    if socket_path.exists() {
        if let Err(e) = tokio::fs::remove_file(socket_path).await {
            tracing::warn!(
                path = %socket_path.display(),
                error = %e,
                "Failed to clean up socket"
            );
        }
    }
}

// ── UDS HTTP Client (Rust → Elixir) ────────────────────────────

/// Health-check an HTTP endpoint over UDS (`GET`, expects 2xx).
#[cfg(unix)]
pub async fn uds_health_check(socket_path: &Path, url_path: &str, timeout: Duration) -> bool {
    uds_request_status(socket_path, "GET", url_path, None, timeout)
        .await
        .map(|status| (200..300).contains(&status))
        .unwrap_or(false)
}

/// POST JSON to an HTTP endpoint over UDS. Returns `true` if response is 2xx.
#[cfg(unix)]
pub async fn uds_post_ok(
    socket_path: &Path,
    url_path: &str,
    body: &serde_json::Value,
    timeout: Duration,
) -> bool {
    uds_request_status(socket_path, "POST", url_path, Some(body), timeout)
        .await
        .map(|status| (200..300).contains(&status))
        .unwrap_or(false)
}

/// Send an HTTP/1.1 request over a Unix socket and return the status code.
///
/// Uses `Connection: close` so the server closes the connection after responding.
/// Only the status line is parsed — headers and body are discarded.
#[cfg(unix)]
async fn uds_request_status(
    socket_path: &Path,
    method: &str,
    url_path: &str,
    body: Option<&serde_json::Value>,
    timeout: Duration,
) -> Result<u16> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    tokio::time::timeout(timeout, async {
        let mut stream = UnixStream::connect(socket_path)
            .await
            .with_context(|| format!("Failed to connect to UDS {}", socket_path.display()))?;

        let request_bytes = if let Some(json) = body {
            let body_bytes = serde_json::to_vec(json)?;
            let mut buf = format!(
                "{method} {url_path} HTTP/1.1\r\n\
                 Host: localhost\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n",
                body_bytes.len()
            )
            .into_bytes();
            buf.extend_from_slice(&body_bytes);
            buf
        } else {
            format!(
                "{method} {url_path} HTTP/1.1\r\n\
                 Host: localhost\r\n\
                 Connection: close\r\n\
                 \r\n"
            )
            .into_bytes()
        };

        stream.write_all(&request_bytes).await?;

        // Read status line: "HTTP/1.x NNN reason\r\n"
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).await?;

        status_line
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .context("Invalid HTTP status line in UDS response")
    })
    .await
    .context("UDS request timed out")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_socket_path_uses_config_dir() {
        let config = crate::config::Config {
            config_path: PathBuf::from("/home/user/.rustyclaw/config.toml"),
            ..Default::default()
        };
        assert_eq!(
            bridge_socket_path(&config),
            PathBuf::from("/home/user/.rustyclaw/bridge.sock")
        );
    }

    #[test]
    fn elixir_socket_path_uses_config_dir() {
        let config = crate::config::Config {
            config_path: PathBuf::from("/home/user/.rustyclaw/config.toml"),
            ..Default::default()
        };
        assert_eq!(
            elixir_socket_path(&config),
            PathBuf::from("/home/user/.rustyclaw/elixir.sock")
        );
    }

    #[tokio::test]
    async fn create_uds_listener_creates_socket() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("test.sock");

        let listener = create_uds_listener(&socket_path).await.unwrap();
        assert!(socket_path.exists());
        drop(listener);
    }

    #[tokio::test]
    async fn stale_socket_is_cleaned_up() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("stale.sock");

        // Create a stale socket (bind then drop — no one listening)
        let stale_listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        drop(stale_listener);
        assert!(socket_path.exists());

        // create_uds_listener should clean it up and re-create
        let listener = create_uds_listener(&socket_path).await.unwrap();
        assert!(socket_path.exists());
        drop(listener);
    }

    #[tokio::test]
    async fn active_socket_prevents_second_listener() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("active.sock");

        // Hold the listener open so it's active
        let _listener = create_uds_listener(&socket_path).await.unwrap();

        // Second create should fail
        let result = create_uds_listener(&socket_path).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("in use by another process"));
    }

    #[tokio::test]
    async fn cleanup_socket_removes_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("cleanup.sock");

        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        drop(listener);
        assert!(socket_path.exists());

        cleanup_socket(&socket_path).await;
        assert!(!socket_path.exists());
    }

    #[tokio::test]
    async fn cleanup_socket_noop_for_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("nonexistent.sock");
        cleanup_socket(&socket_path).await;
    }

    #[tokio::test]
    async fn uds_health_check_against_live_server() {
        use axum::{routing::get, Json, Router};

        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("health.sock");

        let app = Router::new().route(
            "/health",
            get(|| async { Json(serde_json::json!({"status": "ok"})) }),
        );

        let listener = create_uds_listener(&socket_path).await.unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .unwrap();
        });

        // Give the server a moment to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(uds_health_check(&socket_path, "/health", Duration::from_secs(2)).await);
        assert!(!uds_health_check(&socket_path, "/nonexistent", Duration::from_secs(2)).await);
    }

    #[tokio::test]
    async fn uds_post_ok_against_live_server() {
        use axum::{routing::post, Json, Router};

        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("post.sock");

        let app = Router::new().route(
            "/api/test",
            post(|Json(body): Json<serde_json::Value>| async move {
                Json(serde_json::json!({"received": body}))
            }),
        );

        let listener = create_uds_listener(&socket_path).await.unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .unwrap();
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let body = serde_json::json!({"message": "hello"});
        assert!(uds_post_ok(&socket_path, "/api/test", &body, Duration::from_secs(2)).await);
    }
}
