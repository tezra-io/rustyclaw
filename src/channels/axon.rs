//! Axon channel — native agent-to-agent messaging over the Axon local mesh.
//!
//! Uses the Axon v2 two-socket architecture:
//! - **Command socket** (`mode: "cmd"`): synchronous send + delivery ack
//! - **Event socket** (`mode: "event"`): long-running listener + ping/pong
//!
//! Wire protocol: 4-byte big-endian length-prefixed JSON frames over Unix domain socket.

use crate::channels::traits::{Channel, ChannelMessage, SendMessage};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const MAX_MESSAGE_SIZE: u32 = 65_536;
const MAX_BACKOFF_MS: u64 = 30_000;
const SEND_TIMEOUT_SECS: u64 = 5;

// ── Wire protocol types ─────────────────────────────────────────

/// Axon envelope matching the agent-mesh wire format (§2 of protocol spec).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AxonEnvelope {
    pub v: u8,
    pub id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub body: serde_json::Value,
}

impl AxonEnvelope {
    fn new(msg_type: &str, body: serde_json::Value) -> Self {
        Self {
            v: 1,
            id: String::new(),
            msg_type: msg_type.to_string(),
            from: None,
            to: None,
            ts: None,
            thread_id: None,
            body,
        }
    }
}

// ── Framing (length-prefixed JSON over UDS) ─────────────────────

/// Read a 4-byte big-endian length-prefixed frame. Returns None on clean EOF.
async fn read_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> anyhow::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len_buf);
    if len == 0 {
        anyhow::bail!("zero-length frame");
    }
    if len > MAX_MESSAGE_SIZE {
        anyhow::bail!("frame too large: {} bytes (max {})", len, MAX_MESSAGE_SIZE);
    }
    let mut buf = vec![0u8; len as usize];
    reader.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

/// Write a 4-byte big-endian length-prefixed frame.
async fn write_frame<W: AsyncWriteExt + Unpin>(writer: &mut W, data: &[u8]) -> anyhow::Result<()> {
    if data.len() > MAX_MESSAGE_SIZE as usize {
        anyhow::bail!(
            "payload too large: {} bytes (max {})",
            data.len(),
            MAX_MESSAGE_SIZE
        );
    }
    // Safe: length validated against MAX_MESSAGE_SIZE (u32) above.
    let len = u32::try_from(data.len()).expect("length already validated");

    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(data).await?;
    writer.flush().await?;
    Ok(())
}

/// Read one envelope from a framed stream.
async fn read_envelope<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> anyhow::Result<Option<AxonEnvelope>> {
    match read_frame(reader).await? {
        Some(data) => Ok(Some(serde_json::from_slice(&data)?)),
        None => Ok(None),
    }
}

/// Write one envelope to a framed stream.
async fn write_envelope<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    envelope: &AxonEnvelope,
) -> anyhow::Result<()> {
    let data = serde_json::to_vec(envelope)?;
    write_frame(writer, &data).await
}

// ── Channel ─────────────────────────────────────────────────────

pub struct AxonChannel {
    identity: String,
    broker_socket: PathBuf,
    keys_dir: PathBuf,
    reconnect_initial_delay_ms: u64,
    allowed_from: Vec<String>,
    connected: Arc<AtomicBool>,
}

impl AxonChannel {
    pub fn new(
        identity: String,
        broker_socket: PathBuf,
        keys_dir: PathBuf,
        reconnect_initial_delay_ms: u64,
        allowed_from: Vec<String>,
    ) -> Self {
        Self {
            identity,
            broker_socket,
            keys_dir,
            reconnect_initial_delay_ms,
            allowed_from,
            connected: Arc::new(AtomicBool::new(false)),
        }
    }

    // ── Pure mapping functions ───────────────────────────────────

    /// Map an inbound Axon envelope to a ChannelMessage.
    /// Returns None for non-"send" types or envelopes missing required fields.
    fn envelope_to_channel_message(envelope: &AxonEnvelope) -> Option<ChannelMessage> {
        if envelope.msg_type != "send" {
            return None;
        }
        let from = envelope.from.as_deref()?;
        let text = envelope.body.get("text")?.as_str()?;
        if text.is_empty() {
            return None;
        }
        let timestamp = envelope.ts.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        });
        Some(ChannelMessage {
            id: envelope.id.clone(),
            sender: from.to_string(),
            reply_target: from.to_string(),
            content: text.to_string(),
            channel: "axon".to_string(),
            timestamp,
            thread_ts: envelope.thread_id.clone(),
        })
    }

    /// Map an outbound SendMessage to an Axon envelope.
    /// Note: `from` is broker-populated — not set here.
    /// Note: `subject` and `quote_reply_id` have no Axon equivalent — dropped.
    fn send_message_to_envelope(msg: &SendMessage) -> AxonEnvelope {
        let body = serde_json::json!({ "text": msg.content });
        let mut envelope = AxonEnvelope::new("send", body);
        envelope.to = Some(msg.recipient.clone());
        envelope.thread_id = msg.thread_ts.clone();
        envelope
    }

    /// Check whether a sender is in the allowed_from list.
    /// Empty list means allow all senders. Comparison is case-insensitive.
    fn is_sender_allowed(sender: &str, allowed_from: &[String]) -> bool {
        if allowed_from.is_empty() {
            return true;
        }
        let sender_lower = sender.to_lowercase();
        allowed_from
            .iter()
            .any(|a| a.to_lowercase() == sender_lower)
    }

    /// Calculate exponential backoff: `initial_ms * 2^attempt`, capped at 30s.
    fn calculate_backoff(attempt: u32, initial_ms: u64) -> u64 {
        let delay = initial_ms.saturating_mul(1u64 << attempt.min(31));
        delay.min(MAX_BACKOFF_MS)
    }

    /// Sign a challenge nonce with an Ed25519 key. Returns base64-encoded signature.
    fn sign_challenge(signing_key: &SigningKey, nonce: &str) -> String {
        let sig = signing_key.sign(nonce.as_bytes());
        BASE64.encode(sig.to_bytes())
    }

    /// Encode the public key as `"ed25519:<base64>"`.
    fn encode_public_key(signing_key: &SigningKey) -> String {
        let vk = signing_key.verifying_key();
        format!("ed25519:{}", BASE64.encode(vk.as_bytes()))
    }

    /// Load a 32-byte Ed25519 signing key from `{keys_dir}/{identity}.key`.
    fn load_signing_key(keys_dir: &Path, identity: &str) -> anyhow::Result<SigningKey> {
        // Validate identity to prevent path traversal
        if !identity
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || identity.is_empty()
        {
            anyhow::bail!("invalid Axon identity '{identity}': must match [a-zA-Z0-9_-]+");
        }

        let key_path = keys_dir.join(format!("{identity}.key"));
        // Single fs::read avoids TOCTOU between exists() and read()
        let bytes = match std::fs::read(&key_path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!(
                    "Axon identity '{identity}' not found at {} \
                     — run `axon identity create {identity}` to create it",
                    key_path.display(),
                );
            }
            Err(e) => return Err(e.into()),
        };
        if bytes.len() != 32 {
            anyhow::bail!(
                "invalid key file at {}: expected 32 bytes, got {}",
                key_path.display(),
                bytes.len(),
            );
        }
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&bytes);
        Ok(SigningKey::from_bytes(&key_bytes))
    }

    // ── Connection helpers ───────────────────────────────────────

    /// Connect to broker, run challenge-response auth, register with mode.
    async fn connect_and_register(&self, mode: &str) -> anyhow::Result<UnixStream> {
        let mut stream = UnixStream::connect(&self.broker_socket).await?;
        let signing_key = Self::load_signing_key(&self.keys_dir, &self.identity)?;

        let (mut reader, mut writer) = tokio::io::split(&mut stream);

        // Read challenge — verify msg_type before trusting payload
        let challenge = read_envelope(&mut reader)
            .await?
            .ok_or_else(|| anyhow::anyhow!("connection closed before challenge"))?;
        if challenge.msg_type != "challenge" {
            anyhow::bail!(
                "expected 'challenge' from broker, got '{}'",
                challenge.msg_type
            );
        }
        let nonce = challenge.body["nonce"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing nonce in challenge"))?;

        // Build + send register envelope
        let signature = Self::sign_challenge(&signing_key, nonce);
        let public_key = Self::encode_public_key(&signing_key);
        let register = AxonEnvelope::new(
            "register",
            serde_json::json!({
                "name": self.identity,
                "runtime": "rustyclaw",
                "capabilities": [],
                "public_key": public_key,
                "signature": signature,
                "nonce": nonce,
                "mode": mode,
            }),
        );
        write_envelope(&mut writer, &register).await?;

        // Read response
        let response = read_envelope(&mut reader)
            .await?
            .ok_or_else(|| anyhow::anyhow!("connection closed before registration response"))?;

        if response.msg_type == "error" {
            let code = response.body["code"].as_str().unwrap_or("unknown");
            let msg = response.body["message"].as_str().unwrap_or("unknown error");
            anyhow::bail!("registration failed: {code} - {msg}");
        }

        Ok(stream)
    }

    /// Inner listen loop: connect event socket, read messages, handle ping/pong.
    /// Returns `true` if the receiver was dropped (caller should stop reconnecting).
    async fn connect_and_listen(
        &self,
        tx: &tokio::sync::mpsc::Sender<ChannelMessage>,
    ) -> anyhow::Result<bool> {
        let mut stream = self.connect_and_register("event").await?;
        self.connected.store(true, Ordering::Relaxed);
        tracing::info!("[axon] connected as '{}' on event socket", self.identity);

        let (mut reader, mut writer) = tokio::io::split(&mut stream);

        loop {
            match read_envelope(&mut reader).await {
                Ok(Some(envelope)) => {
                    if envelope.msg_type == "ping" {
                        let pong = AxonEnvelope::new("pong", serde_json::json!({}));
                        write_envelope(&mut writer, &pong).await?;
                        continue;
                    }

                    if envelope.msg_type != "send" {
                        tracing::debug!("[axon] ignoring message type: {}", envelope.msg_type);
                        continue;
                    }

                    match envelope.from {
                        Some(ref from) => {
                            if !Self::is_sender_allowed(from, &self.allowed_from) {
                                tracing::debug!("[axon] filtered message from: {}", from);
                                continue;
                            }
                        }
                        None if !self.allowed_from.is_empty() => {
                            tracing::debug!(
                                "[axon] rejected message with no sender (allowed_from is set)"
                            );
                            continue;
                        }
                        None => {}
                    }

                    if let Some(msg) = Self::envelope_to_channel_message(&envelope) {
                        if tx.send(msg).await.is_err() {
                            return Ok(true); // receiver dropped
                        }
                    }
                }
                Ok(None) => return Ok(false), // broker disconnected
                Err(e) => return Err(e),
            }
        }
    }
}

#[async_trait]
impl Channel for AxonChannel {
    fn name(&self) -> &str {
        "axon"
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        let envelope = Self::send_message_to_envelope(message);

        // Open a fresh cmd socket for this send
        let mut stream = self.connect_and_register("cmd").await?;
        let (mut reader, mut writer) = tokio::io::split(&mut stream);

        write_envelope(&mut writer, &envelope).await?;

        // Read delivery ack/nack with timeout
        let response = tokio::time::timeout(
            Duration::from_secs(SEND_TIMEOUT_SECS),
            read_envelope(&mut reader),
        )
        .await
        .map_err(|_| anyhow::anyhow!("send timed out waiting for delivery ack"))??
        .ok_or_else(|| anyhow::anyhow!("connection closed before delivery response"))?;

        match response.msg_type.as_str() {
            "delivery_ack" => Ok(()),
            "delivery_nack" => {
                let reason = response.body["reason"].as_str().unwrap_or("unknown");
                anyhow::bail!("delivery failed: {reason}")
            }
            "error" => {
                let code = response.body["code"].as_str().unwrap_or("unknown");
                let msg = response.body["message"].as_str().unwrap_or("unknown error");
                anyhow::bail!("send error: {code} - {msg}")
            }
            other => anyhow::bail!("unexpected response type: {other}"),
        }
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        let mut attempt: u32 = 0;

        loop {
            match self.connect_and_listen(&tx).await {
                Ok(true) => {
                    // Receiver dropped — no one is consuming messages, stop reconnecting
                    tracing::info!("[axon] message receiver dropped, stopping listener");
                    self.connected.store(false, Ordering::Relaxed);
                    return Ok(());
                }
                Ok(false) => {
                    attempt = 0;
                    tracing::info!("[axon] broker disconnected, reconnecting...");
                }
                Err(e) => {
                    if attempt == 0 {
                        tracing::warn!("[axon] connection failed: {e}");
                    } else {
                        tracing::debug!("[axon] reconnect failed: {e}");
                    }
                }
            }

            self.connected.store(false, Ordering::Relaxed);

            let delay_ms = Self::calculate_backoff(attempt, self.reconnect_initial_delay_ms);
            tracing::info!(
                "[axon] broker not available at {} — retrying in {delay_ms}ms",
                self.broker_socket.display(),
            );
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            attempt = attempt.saturating_add(1);
        }
    }

    async fn health_check(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Envelope → ChannelMessage mapping ───────────────────────

    fn make_send_envelope(from: &str, text: &str) -> AxonEnvelope {
        AxonEnvelope {
            v: 1,
            id: "msg-001".to_string(),
            msg_type: "send".to_string(),
            from: Some(from.to_string()),
            to: Some("rusty".to_string()),
            ts: Some(1_700_000_000),
            thread_id: None,
            body: serde_json::json!({ "text": text }),
        }
    }

    #[test]
    fn envelope_to_channel_message_valid() {
        let env = make_send_envelope("aira", "hello rusty");
        let msg = AxonChannel::envelope_to_channel_message(&env).unwrap();
        assert_eq!(msg.id, "msg-001");
        assert_eq!(msg.sender, "aira");
        assert_eq!(msg.reply_target, "aira");
        assert_eq!(msg.content, "hello rusty");
        assert_eq!(msg.channel, "axon");
        assert_eq!(msg.timestamp, 1_700_000_000);
        assert!(msg.thread_ts.is_none());
    }

    #[test]
    fn envelope_to_channel_message_with_thread() {
        let mut env = make_send_envelope("aira", "threaded msg");
        env.thread_id = Some("thread-42".to_string());
        let msg = AxonChannel::envelope_to_channel_message(&env).unwrap();
        assert_eq!(msg.thread_ts.as_deref(), Some("thread-42"));
    }

    #[test]
    fn envelope_to_channel_message_missing_from() {
        let mut env = make_send_envelope("aira", "hello");
        env.from = None;
        assert!(AxonChannel::envelope_to_channel_message(&env).is_none());
    }

    #[test]
    fn envelope_to_channel_message_missing_body_text() {
        let mut env = make_send_envelope("aira", "hello");
        env.body = serde_json::json!({});
        assert!(AxonChannel::envelope_to_channel_message(&env).is_none());
    }

    #[test]
    fn envelope_to_channel_message_empty_text() {
        let env = make_send_envelope("aira", "");
        assert!(AxonChannel::envelope_to_channel_message(&env).is_none());
    }

    #[test]
    fn envelope_to_channel_message_non_send_type() {
        let mut env = make_send_envelope("aira", "hello");
        env.msg_type = "ping".to_string();
        assert!(AxonChannel::envelope_to_channel_message(&env).is_none());
    }

    #[test]
    fn envelope_to_channel_message_no_timestamp_uses_now() {
        let mut env = make_send_envelope("aira", "hello");
        env.ts = None;
        let msg = AxonChannel::envelope_to_channel_message(&env).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(msg.timestamp <= now && msg.timestamp >= now - 5);
    }

    // ── SendMessage → Envelope mapping ──────────────────────────

    #[test]
    fn send_message_to_envelope_basic() {
        let msg = SendMessage::new("sup", "aira");
        let env = AxonChannel::send_message_to_envelope(&msg);
        assert_eq!(env.msg_type, "send");
        assert_eq!(env.to.as_deref(), Some("aira"));
        assert_eq!(env.body["text"].as_str(), Some("sup"));
        assert_eq!(env.v, 1);
        assert!(env.from.is_none());
        assert!(env.thread_id.is_none());
    }

    #[test]
    fn send_message_to_envelope_with_thread() {
        let msg = SendMessage::new("reply", "aira").in_thread(Some("thread-7".to_string()));
        let env = AxonChannel::send_message_to_envelope(&msg);
        assert_eq!(env.thread_id.as_deref(), Some("thread-7"));
    }

    #[test]
    fn send_message_to_envelope_drops_unsupported_fields() {
        let msg = SendMessage::with_subject("hello", "aira", "Important")
            .with_quote_reply(Some("msg-99".to_string()));
        let env = AxonChannel::send_message_to_envelope(&msg);
        assert!(env.body.get("subject").is_none());
        assert!(env.body.get("quote_reply_id").is_none());
        assert_eq!(env.body["text"].as_str(), Some("hello"));
    }

    // ── Sender filter ───────────────────────────────────────────

    #[test]
    fn allowed_from_empty_allows_all() {
        assert!(AxonChannel::is_sender_allowed("anyone", &[]));
    }

    #[test]
    fn allowed_from_specific_match() {
        let allowed = vec!["aira".to_string(), "codex".to_string()];
        assert!(AxonChannel::is_sender_allowed("aira", &allowed));
        assert!(AxonChannel::is_sender_allowed("codex", &allowed));
    }

    #[test]
    fn allowed_from_denies_unlisted() {
        let allowed = vec!["aira".to_string()];
        assert!(!AxonChannel::is_sender_allowed("unknown", &allowed));
    }

    #[test]
    fn allowed_from_case_insensitive() {
        let allowed = vec!["Aira".to_string()];
        assert!(AxonChannel::is_sender_allowed("aira", &allowed));
        assert!(AxonChannel::is_sender_allowed("AIRA", &allowed));
    }

    // ── Backoff calculation ─────────────────────────────────────

    #[test]
    fn backoff_attempt_zero() {
        assert_eq!(AxonChannel::calculate_backoff(0, 1000), 1000);
    }

    #[test]
    fn backoff_exponential() {
        assert_eq!(AxonChannel::calculate_backoff(1, 1000), 2000);
        assert_eq!(AxonChannel::calculate_backoff(2, 1000), 4000);
        assert_eq!(AxonChannel::calculate_backoff(3, 1000), 8000);
    }

    #[test]
    fn backoff_caps_at_max() {
        assert_eq!(AxonChannel::calculate_backoff(5, 1000), MAX_BACKOFF_MS);
        assert_eq!(AxonChannel::calculate_backoff(10, 1000), MAX_BACKOFF_MS);
        assert_eq!(AxonChannel::calculate_backoff(31, 1000), MAX_BACKOFF_MS);
    }

    // ── Ed25519 signing ─────────────────────────────────────────

    #[test]
    fn sign_challenge_produces_valid_base64() {
        let key = SigningKey::from_bytes(&[1u8; 32]);
        let sig = AxonChannel::sign_challenge(&key, "test-nonce-123");
        let decoded = BASE64.decode(&sig).unwrap();
        assert_eq!(decoded.len(), 64); // Ed25519 signature is 64 bytes
    }

    #[test]
    fn sign_challenge_different_nonces_differ() {
        let key = SigningKey::from_bytes(&[1u8; 32]);
        let sig1 = AxonChannel::sign_challenge(&key, "nonce-a");
        let sig2 = AxonChannel::sign_challenge(&key, "nonce-b");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn sign_challenge_verifiable() {
        let key = SigningKey::from_bytes(&[2u8; 32]);
        let nonce = "verify-me";
        let sig_b64 = AxonChannel::sign_challenge(&key, nonce);
        let sig_bytes = BASE64.decode(&sig_b64).unwrap();
        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes);
        let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);
        use ed25519_dalek::Verifier;
        assert!(key.verifying_key().verify(nonce.as_bytes(), &sig).is_ok());
    }

    #[test]
    fn encode_public_key_format() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let encoded = AxonChannel::encode_public_key(&key);
        assert!(encoded.starts_with("ed25519:"));
        let b64_part = encoded.strip_prefix("ed25519:").unwrap();
        let decoded = BASE64.decode(b64_part).unwrap();
        assert_eq!(decoded.len(), 32); // Ed25519 public key is 32 bytes
    }

    // ── Key loading ─────────────────────────────────────────────

    #[test]
    fn load_signing_key_missing_file() {
        let result = AxonChannel::load_signing_key(Path::new("/nonexistent"), "ghost");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "error should mention 'not found': {err}"
        );
        assert!(
            err.contains("axon identity create"),
            "error should suggest create command: {err}"
        );
    }

    #[test]
    fn load_signing_key_valid() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("test.key");
        std::fs::write(&key_path, [42u8; 32]).unwrap();
        let key = AxonChannel::load_signing_key(dir.path(), "test").unwrap();
        assert_eq!(key.to_bytes(), [42u8; 32]);
    }

    #[test]
    fn load_signing_key_wrong_size() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("bad.key");
        std::fs::write(&key_path, [0u8; 16]).unwrap();
        let result = AxonChannel::load_signing_key(dir.path(), "bad");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("expected 32 bytes"));
    }

    #[test]
    fn load_signing_key_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let result = AxonChannel::load_signing_key(dir.path(), "../../../etc/passwd");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid Axon identity"), "got: {err}");
    }

    #[test]
    fn load_signing_key_rejects_slash_in_identity() {
        let dir = tempfile::tempdir().unwrap();
        let result = AxonChannel::load_signing_key(dir.path(), "foo/bar");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid Axon identity"), "got: {err}");
    }

    #[test]
    fn load_signing_key_rejects_empty_identity() {
        let dir = tempfile::tempdir().unwrap();
        let result = AxonChannel::load_signing_key(dir.path(), "");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid Axon identity"), "got: {err}");
    }

    #[test]
    fn load_signing_key_accepts_valid_identity_chars() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("my-agent_01.key");
        std::fs::write(&key_path, [7u8; 32]).unwrap();
        let key = AxonChannel::load_signing_key(dir.path(), "my-agent_01").unwrap();
        assert_eq!(key.to_bytes(), [7u8; 32]);
    }

    // ── Wire framing ────────────────────────────────────────────

    #[tokio::test]
    async fn frame_roundtrip() {
        let payload = b"{\"hello\":\"world\"}";
        let mut buf = Vec::new();
        write_frame(&mut buf, payload).await.unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let result = read_frame(&mut cursor).await.unwrap().unwrap();
        assert_eq!(result, payload);
    }

    #[tokio::test]
    async fn frame_eof_returns_none() {
        let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
        assert!(read_frame(&mut cursor).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn frame_too_large_errors() {
        let data = vec![0u8; (MAX_MESSAGE_SIZE + 1) as usize];
        let mut buf = Vec::new();
        assert!(write_frame(&mut buf, &data).await.is_err());
    }

    #[tokio::test]
    async fn envelope_roundtrip() {
        let env = AxonEnvelope {
            v: 1,
            id: "test-123".to_string(),
            msg_type: "send".to_string(),
            from: Some("aira".to_string()),
            to: Some("rusty".to_string()),
            ts: Some(1_700_000_000),
            thread_id: None,
            body: serde_json::json!({ "text": "hello" }),
        };
        let mut buf = Vec::new();
        write_envelope(&mut buf, &env).await.unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let decoded = read_envelope(&mut cursor).await.unwrap().unwrap();
        assert_eq!(decoded.id, "test-123");
        assert_eq!(decoded.msg_type, "send");
        assert_eq!(decoded.from.as_deref(), Some("aira"));
        assert_eq!(decoded.body["text"].as_str(), Some("hello"));
    }

    // ── Envelope serde ──────────────────────────────────────────

    #[test]
    fn envelope_serializes_correctly() {
        let mut env = AxonEnvelope::new("send", serde_json::json!({ "text": "hi" }));
        env.to = Some("aira".to_string());
        let json = serde_json::to_value(&env).unwrap();
        assert_eq!(json["type"], "send");
        assert_eq!(json["to"], "aira");
        assert_eq!(json["v"], 1);
        assert!(json.get("from").is_none());
    }

    #[test]
    fn envelope_deserializes_from_broker() {
        let json = r#"{
            "v": 1,
            "id": "broker-uuid",
            "type": "send",
            "from": "aira",
            "to": "rusty",
            "ts": 1700000000,
            "body": { "text": "hey there" }
        }"#;
        let env: AxonEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(env.id, "broker-uuid");
        assert_eq!(env.msg_type, "send");
        assert_eq!(env.from.as_deref(), Some("aira"));
        assert_eq!(env.body["text"].as_str(), Some("hey there"));
    }

    // ── Channel trait basics ────────────────────────────────────

    #[test]
    fn channel_name_is_axon() {
        let ch = AxonChannel::new(
            "rusty".to_string(),
            PathBuf::from("/tmp/test.sock"),
            PathBuf::from("/tmp/keys"),
            1000,
            vec![],
        );
        assert_eq!(ch.name(), "axon");
    }

    #[tokio::test]
    async fn health_check_starts_false() {
        let ch = AxonChannel::new(
            "rusty".to_string(),
            PathBuf::from("/tmp/test.sock"),
            PathBuf::from("/tmp/keys"),
            1000,
            vec![],
        );
        assert!(!ch.health_check().await);
    }
}
