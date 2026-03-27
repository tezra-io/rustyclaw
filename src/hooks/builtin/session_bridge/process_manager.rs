use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing;
use uuid::Uuid;

use crate::channels::traits::{Channel, SendMessage};
use crate::config::SessionBridgeAgentConfig;

use super::binding_table::BindingTable;

/// Maximum characters per Telegram message.
const TELEGRAM_CHUNK_LIMIT: usize = 4096;

/// Delay between sending chunked messages to avoid Telegram rate limits.
const INTER_CHUNK_DELAY_MS: u64 = 100;

/// Buffer size for the stdin mpsc channel.
const STDIN_CHANNEL_BUFFER: usize = 32;

/// Maximum queued messages before warning the user.
pub const QUEUE_WARN_THRESHOLD: usize = 3;

/// Maximum crash count within the cooldown window before refusing reconnect.
const CRASH_COOLDOWN_MAX: usize = 3;

/// Cooldown window in seconds.
const CRASH_COOLDOWN_SECS: u64 = 60;

/// Key for the process table: (channel_name, sender_id).
type ProcessKey = (String, String);

/// A managed child process for a session bridge session.
struct SessionProcess {
    /// The spawned child process handle.
    child: Child,
    /// Sender side of the stdin writer channel.
    stdin_tx: mpsc::Sender<String>,
    /// Handle for the stdout reader task.
    stdout_handle: JoinHandle<()>,
    /// Handle for the stdin writer task.
    stdin_handle: JoinHandle<()>,
    /// Unique session ID for this process.
    session_id: String,
    /// Number of messages currently queued (sent but not yet responded to).
    pending_messages: Arc<std::sync::atomic::AtomicUsize>,
}

/// Tracks recent crash times for a (channel, sender) pair.
#[derive(Debug, Default, Clone)]
pub struct CrashRecord {
    times: Vec<std::time::Instant>,
}

impl CrashRecord {
    /// Record a crash at the current instant.
    pub fn record_crash(&mut self) {
        self.times.push(std::time::Instant::now());
    }

    /// Returns true if there have been >= `max` crashes within the last `window` seconds.
    pub fn in_cooldown(&self, max: usize, window_secs: u64) -> bool {
        let cutoff = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(window_secs))
            .unwrap_or(std::time::Instant::now());
        let recent = self.times.iter().filter(|t| **t >= cutoff).count();
        recent >= max
    }
}

/// Manages spawned Claude Code child processes for the session bridge.
///
/// Thread-safe via `Arc<Mutex<...>>` interior mutability.
/// Implements `Drop` to kill all child processes on shutdown.
pub struct ProcessManager {
    inner: Arc<Mutex<ProcessManagerInner>>,
}

struct ProcessManagerInner {
    processes: HashMap<ProcessKey, SessionProcess>,
    crash_records: HashMap<ProcessKey, CrashRecord>,
}

impl Drop for ProcessManagerInner {
    fn drop(&mut self) {
        for (key, mut proc) in self.processes.drain() {
            tracing::info!(
                hook = "session-bridge",
                channel = %key.0,
                sender = %key.1,
                session_id = %proc.session_id,
                "killing child process on shutdown"
            );
            // Best-effort kill — we're being dropped, can't do much if it fails
            let _ = proc.child.start_kill();
            proc.stdout_handle.abort();
            proc.stdin_handle.abort();
        }
    }
}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProcessManagerInner {
                processes: HashMap::new(),
                crash_records: HashMap::new(),
            })),
        }
    }

    /// Check if a (channel, sender) pair is in crash cooldown.
    pub async fn in_cooldown(&self, channel: &str, sender: &str) -> bool {
        let inner = self.inner.lock().await;
        if let Some(record) = inner
            .crash_records
            .get(&(channel.to_string(), sender.to_string()))
        {
            record.in_cooldown(CRASH_COOLDOWN_MAX, CRASH_COOLDOWN_SECS)
        } else {
            false
        }
    }

    /// Spawn a new child process for the given (channel, sender) pair.
    ///
    /// The process runs the agent command with stream-json I/O. A stdout reader
    /// task sends responses back to the user via `Channel::send()`. On process
    /// exit, the binding is auto-removed and the user is notified.
    pub async fn spawn(
        &self,
        channel: &str,
        sender: &str,
        reply_target: &str,
        thread_ts: Option<String>,
        agent_config: &SessionBridgeAgentConfig,
        working_dir: &Path,
        output_buffer_ms: u64,
        channel_ref: Arc<dyn Channel>,
        binding_table: &BindingTable,
    ) -> anyhow::Result<String> {
        let key = (channel.to_string(), sender.to_string());
        let mut inner = self.inner.lock().await;

        if inner.processes.contains_key(&key) {
            anyhow::bail!("process already running for ({}, {})", channel, sender);
        }

        let session_id = Uuid::new_v4().to_string();

        let mut cmd = Command::new(&agent_config.command);
        cmd.args(&agent_config.args)
            .arg("--print")
            .arg("--permission-mode")
            .arg("bypassPermissions")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--session-id")
            .arg(&session_id)
            .current_dir(working_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn '{}': {}", agent_config.command, e))?;

        let child_stdin = child.stdin.take().expect("stdin was piped");
        let child_stdout = child.stdout.take().expect("stdout was piped");

        // Stdin writer task
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(STDIN_CHANNEL_BUFFER);
        let stdin_handle = tokio::spawn(async move {
            let mut writer = child_stdin;
            while let Some(msg) = stdin_rx.recv().await {
                if let Err(e) = writer.write_all(msg.as_bytes()).await {
                    tracing::error!(
                        hook = "session-bridge",
                        error = %e,
                        "stdin write failed"
                    );
                    break;
                }
                if let Err(e) = writer.flush().await {
                    tracing::error!(
                        hook = "session-bridge",
                        error = %e,
                        "stdin flush failed"
                    );
                    break;
                }
            }
        });

        // Pending message counter (shared between message routing and stdout reader)
        let pending_messages = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // Stdout reader task
        let reader_channel = channel.to_string();
        let reader_sender = sender.to_string();
        let reader_reply_target = reply_target.to_string();
        let reader_thread_ts = thread_ts;
        let reader_session_id = session_id.clone();
        let reader_binding_table = binding_table.clone();
        let reader_pending = pending_messages.clone();
        let process_manager_inner = self.inner.clone();

        let stdout_handle = tokio::spawn(async move {
            let reader = BufReader::new(child_stdout);
            let mut lines = reader.lines();

            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }

                        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&line);
                        let json = match parsed {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        // Check for result type (signals turn completion)
                        let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");

                        if msg_type == "result" {
                            // Turn complete — extract final result text if present
                            let result_text = json
                                .get("result")
                                .and_then(|r| r.as_str())
                                .unwrap_or("")
                                .to_string();

                            if !result_text.is_empty() {
                                // Buffer before sending
                                if output_buffer_ms > 0 {
                                    tokio::time::sleep(tokio::time::Duration::from_millis(
                                        output_buffer_ms,
                                    ))
                                    .await;
                                }
                                send_chunked(
                                    &channel_ref,
                                    &reader_reply_target,
                                    reader_thread_ts.as_deref(),
                                    &result_text,
                                )
                                .await;
                            }

                            // Decrement pending counter
                            reader_pending.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                            continue;
                        }

                        if msg_type == "assistant" {
                            // Extract assistant message content
                            let content_text = extract_assistant_text(&json);
                            if content_text.is_empty() {
                                continue;
                            }

                            // Buffer before sending
                            if output_buffer_ms > 0 {
                                tokio::time::sleep(tokio::time::Duration::from_millis(
                                    output_buffer_ms,
                                ))
                                .await;
                            }

                            send_chunked(
                                &channel_ref,
                                &reader_reply_target,
                                reader_thread_ts.as_deref(),
                                &content_text,
                            )
                            .await;

                            // Decrement pending counter
                            reader_pending.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Ok(None) => {
                        // EOF — process exited
                        break;
                    }
                    Err(e) => {
                        tracing::error!(
                            hook = "session-bridge",
                            error = %e,
                            "stdout read error"
                        );
                        break;
                    }
                }
            }

            // Process exited — auto-unbind and notify user
            tracing::info!(
                hook = "session-bridge",
                channel = %reader_channel,
                sender = %reader_sender,
                session_id = %reader_session_id,
                "child process exited (stdout EOF)"
            );

            // Record crash
            {
                let mut pm_inner = process_manager_inner.lock().await;
                let key = (reader_channel.clone(), reader_sender.clone());
                pm_inner.processes.remove(&key);
                pm_inner
                    .crash_records
                    .entry(key)
                    .or_default()
                    .record_crash();
            }

            // Auto-unbind
            if let Err(e) = reader_binding_table
                .unbind(&reader_channel, &reader_sender)
                .await
            {
                tracing::error!(
                    hook = "session-bridge",
                    error = %e,
                    "failed to auto-unbind after process exit"
                );
            }

            // Notify user
            let reply = SendMessage::new(
                "Claude Code session ended unexpectedly. Back to normal chat.",
                &reader_reply_target,
            )
            .in_thread(reader_thread_ts);
            if let Err(e) = channel_ref.send(&reply).await {
                tracing::error!(
                    hook = "session-bridge",
                    error = %e,
                    "failed to notify user of process exit"
                );
            }
        });

        inner.processes.insert(
            key,
            SessionProcess {
                child,
                stdin_tx,
                stdout_handle,
                stdin_handle,
                session_id: session_id.clone(),
                pending_messages,
            },
        );

        Ok(session_id)
    }

    /// Send a message to a running child process via its stdin channel.
    ///
    /// Returns `Ok(pending_count)` with the number of messages queued (including this one).
    pub async fn send_message(
        &self,
        channel: &str,
        sender: &str,
        text: &str,
    ) -> anyhow::Result<usize> {
        let inner = self.inner.lock().await;
        let key = (channel.to_string(), sender.to_string());

        let proc = inner
            .processes
            .get(&key)
            .ok_or_else(|| anyhow::anyhow!("no running process for ({}, {})", channel, sender))?;

        let json_msg = format_stdin_message(text);

        // Increment pending counter
        let prev = proc
            .pending_messages
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pending_count = prev + 1;

        proc.stdin_tx.try_send(json_msg).map_err(|e| {
            proc.pending_messages
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            anyhow::anyhow!("stdin channel full or closed: {}", e)
        })?;

        Ok(pending_count)
    }

    /// Kill a specific child process. Returns the session_id if it was running.
    pub async fn kill(&self, channel: &str, sender: &str) -> Option<String> {
        let mut inner = self.inner.lock().await;
        let key = (channel.to_string(), sender.to_string());

        if let Some(mut proc) = inner.processes.remove(&key) {
            let session_id = proc.session_id.clone();
            let _ = proc.child.start_kill();
            proc.stdout_handle.abort();
            proc.stdin_handle.abort();
            tracing::info!(
                hook = "session-bridge",
                channel = channel,
                sender = sender,
                session_id = %session_id,
                "killed child process"
            );
            Some(session_id)
        } else {
            None
        }
    }

    /// Kill all child processes. Used for graceful shutdown.
    pub async fn kill_all(&self) {
        let mut inner = self.inner.lock().await;
        for (key, mut proc) in inner.processes.drain() {
            let _ = proc.child.start_kill();
            proc.stdout_handle.abort();
            proc.stdin_handle.abort();
            tracing::info!(
                hook = "session-bridge",
                channel = %key.0,
                sender = %key.1,
                session_id = %proc.session_id,
                "killed child process (shutdown)"
            );
        }
    }

    /// Check if a process is running for the given key.
    pub async fn has_process(&self, channel: &str, sender: &str) -> bool {
        let inner = self.inner.lock().await;
        inner
            .processes
            .contains_key(&(channel.to_string(), sender.to_string()))
    }

    /// Get the number of pending (unresponded) messages for a session.
    pub async fn pending_count(&self, channel: &str, sender: &str) -> usize {
        let inner = self.inner.lock().await;
        let key = (channel.to_string(), sender.to_string());
        inner
            .processes
            .get(&key)
            .map(|p| {
                p.pending_messages
                    .load(std::sync::atomic::Ordering::Relaxed)
            })
            .unwrap_or(0)
    }
}

/// Format a user message as stream-json for Claude Code stdin.
pub fn format_stdin_message(text: &str) -> String {
    let msg = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": text
        }
    });
    format!("{}\n", msg)
}

/// Extract text content from an assistant-type stream-json message.
///
/// The content field can be either a string or an array of content blocks.
pub fn extract_assistant_text(json: &serde_json::Value) -> String {
    let message = match json.get("message") {
        Some(m) => m,
        None => return String::new(),
    };

    let content = match message.get("content") {
        Some(c) => c,
        None => return String::new(),
    };

    // Content can be a string directly
    if let Some(s) = content.as_str() {
        return s.to_string();
    }

    // Or an array of content blocks
    if let Some(arr) = content.as_array() {
        let mut text = String::new();
        for block in arr {
            if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
            }
        }
        return text;
    }

    String::new()
}

/// Split a long message into chunks suitable for Telegram (<=4096 chars each).
/// Prefers splitting on paragraph boundaries ("\n\n"), falls back to hard split.
pub fn chunk_message(text: &str, limit: usize) -> Vec<String> {
    if text.len() <= limit {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= limit {
            chunks.push(remaining.to_string());
            break;
        }

        // Try to split on paragraph boundary
        let search_region = &remaining[..limit];
        let split_pos = if let Some(pos) = search_region.rfind("\n\n") {
            // Split after the paragraph break
            pos + 2
        } else if let Some(pos) = search_region.rfind('\n') {
            // Fall back to line boundary
            pos + 1
        } else {
            // Hard split at limit
            limit
        };

        // Avoid empty chunks from boundary splits
        let split_pos = if split_pos == 0 { limit } else { split_pos };

        chunks.push(remaining[..split_pos].to_string());
        remaining = &remaining[split_pos..];
    }

    chunks
}

/// Send a message to a channel, chunking if necessary for Telegram limits.
async fn send_chunked(
    channel: &Arc<dyn Channel>,
    reply_target: &str,
    thread_ts: Option<&str>,
    text: &str,
) {
    let chunks = chunk_message(text, TELEGRAM_CHUNK_LIMIT);

    for (i, chunk) in chunks.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(INTER_CHUNK_DELAY_MS)).await;
        }

        let msg =
            SendMessage::new(chunk.as_str(), reply_target).in_thread(thread_ts.map(String::from));
        if let Err(e) = channel.send(&msg).await {
            tracing::error!(
                hook = "session-bridge",
                error = %e,
                chunk = i + 1,
                total_chunks = chunks.len(),
                "failed to send chunked message"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── format_stdin_message tests ──────────────────────

    #[test]
    fn stdin_message_format() {
        let msg = format_stdin_message("hello world");
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed["type"], "user");
        assert_eq!(parsed["message"]["role"], "user");
        assert_eq!(parsed["message"]["content"], "hello world");
    }

    #[test]
    fn stdin_message_escapes_special_chars() {
        let msg = format_stdin_message("line1\nline2\t\"quoted\"");
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed["message"]["content"], "line1\nline2\t\"quoted\"");
    }

    #[test]
    fn stdin_message_ends_with_newline() {
        let msg = format_stdin_message("test");
        assert!(msg.ends_with('\n'));
    }

    // ── extract_assistant_text tests ────────────────────

    #[test]
    fn extract_text_from_string_content() {
        let json: serde_json::Value = serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": "Hello from Claude"
            }
        });
        assert_eq!(extract_assistant_text(&json), "Hello from Claude");
    }

    #[test]
    fn extract_text_from_array_content() {
        let json: serde_json::Value = serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "First block"},
                    {"type": "text", "text": "Second block"}
                ]
            }
        });
        assert_eq!(extract_assistant_text(&json), "First block\nSecond block");
    }

    #[test]
    fn extract_text_skips_non_text_blocks() {
        let json: serde_json::Value = serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "t1"},
                    {"type": "text", "text": "only this"}
                ]
            }
        });
        assert_eq!(extract_assistant_text(&json), "only this");
    }

    #[test]
    fn extract_text_empty_on_missing_message() {
        let json: serde_json::Value = serde_json::json!({"type": "system"});
        assert_eq!(extract_assistant_text(&json), "");
    }

    #[test]
    fn extract_text_empty_on_missing_content() {
        let json: serde_json::Value = serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant"}
        });
        assert_eq!(extract_assistant_text(&json), "");
    }

    // ── chunk_message tests ─────────────────────────────

    #[test]
    fn short_message_single_chunk() {
        let chunks = chunk_message("short", 4096);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "short");
    }

    #[test]
    fn exact_limit_single_chunk() {
        let text = "a".repeat(4096);
        let chunks = chunk_message(&text, 4096);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn splits_on_paragraph_boundary() {
        let text = format!("{}\n\n{}", "a".repeat(100), "b".repeat(100));
        let chunks = chunk_message(&text, 150);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].ends_with("\n\n"));
        assert!(chunks[1].starts_with('b'));
    }

    #[test]
    fn splits_on_line_boundary_when_no_paragraph() {
        let text = format!("{}\n{}", "a".repeat(100), "b".repeat(100));
        let chunks = chunk_message(&text, 150);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].ends_with('\n'));
    }

    #[test]
    fn hard_split_when_no_boundaries() {
        let text = "a".repeat(200);
        let chunks = chunk_message(&text, 100);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 100);
        assert_eq!(chunks[1].len(), 100);
    }

    #[test]
    fn multiple_chunks() {
        let text = "a".repeat(300);
        let chunks = chunk_message(&text, 100);
        assert_eq!(chunks.len(), 3);
        for chunk in &chunks {
            assert_eq!(chunk.len(), 100);
        }
    }

    #[test]
    fn empty_message() {
        let chunks = chunk_message("", 4096);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "");
    }

    #[test]
    fn all_chunks_within_limit() {
        // A realistic test with paragraph-separated content
        let paragraphs: Vec<String> = (0..20)
            .map(|i| format!("Paragraph {} with some content here.", i))
            .collect();
        let text = paragraphs.join("\n\n");
        let chunks = chunk_message(&text, 200);
        for chunk in &chunks {
            assert!(chunk.len() <= 200, "chunk too long: {} chars", chunk.len());
        }
        // Reassembled text matches original
        let reassembled: String = chunks.concat();
        assert_eq!(reassembled, text);
    }

    // ── CrashRecord / cooldown tests ────────────────────

    #[test]
    fn no_crashes_no_cooldown() {
        let record = CrashRecord::default();
        assert!(!record.in_cooldown(3, 60));
    }

    #[test]
    fn fewer_than_max_no_cooldown() {
        let mut record = CrashRecord::default();
        record.record_crash();
        record.record_crash();
        assert!(!record.in_cooldown(3, 60));
    }

    #[test]
    fn at_max_triggers_cooldown() {
        let mut record = CrashRecord::default();
        record.record_crash();
        record.record_crash();
        record.record_crash();
        assert!(record.in_cooldown(3, 60));
    }

    #[test]
    fn old_crashes_expire() {
        let mut record = CrashRecord::default();
        // Manually insert old timestamps
        let old = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(120))
            .expect("120s subtraction should not underflow");
        record.times.push(old);
        record.times.push(old);
        record.times.push(old);
        // Old crashes should not trigger cooldown
        assert!(!record.in_cooldown(3, 60));
    }

    // ── ProcessManager unit tests (no actual process spawn) ──

    #[tokio::test]
    async fn has_process_false_when_empty() {
        let pm = ProcessManager::new();
        assert!(!pm.has_process("telegram", "alice").await);
    }

    #[tokio::test]
    async fn in_cooldown_false_when_empty() {
        let pm = ProcessManager::new();
        assert!(!pm.in_cooldown("telegram", "alice").await);
    }

    #[tokio::test]
    async fn send_message_fails_no_process() {
        let pm = ProcessManager::new();
        let result = pm.send_message("telegram", "alice", "hello").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn kill_returns_none_when_empty() {
        let pm = ProcessManager::new();
        assert!(pm.kill("telegram", "alice").await.is_none());
    }

    #[tokio::test]
    async fn queue_warn_threshold_value() {
        // Ensure our threshold constant is reasonable
        assert_eq!(QUEUE_WARN_THRESHOLD, 3);
    }

    // ── ProcessManager integration tests (spawns real processes) ──

    use super::super::binding_table::BindingTable;
    use crate::channels::traits::{Channel, ChannelMessage, SendMessage};
    use std::sync::Mutex;
    use tempfile::TempDir;

    struct MockChannel {
        sent: Arc<Mutex<Vec<SendMessage>>>,
    }

    #[async_trait::async_trait]
    impl Channel for MockChannel {
        fn name(&self) -> &str {
            "telegram"
        }
        async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push(message.clone());
            Ok(())
        }
        async fn listen(
            &self,
            _tx: tokio::sync::mpsc::Sender<ChannelMessage>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn mock_channel() -> (Arc<dyn Channel>, Arc<Mutex<Vec<SendMessage>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let channel = Arc::new(MockChannel { sent: sent.clone() });
        (channel as Arc<dyn Channel>, sent)
    }

    fn cat_agent_config() -> SessionBridgeAgentConfig {
        SessionBridgeAgentConfig {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "cat".to_string()],
        }
    }

    #[tokio::test]
    async fn spawn_and_has_process() {
        let pm = ProcessManager::new();
        let tmp = TempDir::new().unwrap();
        let binding_table = BindingTable::load(tmp.path()).await.unwrap();
        let (channel, _sent) = mock_channel();

        let result = pm
            .spawn(
                "telegram",
                "alice",
                "chat123",
                None,
                &cat_agent_config(),
                tmp.path(),
                0,
                channel,
                &binding_table,
            )
            .await;

        assert!(result.is_ok());
        let session_id = result.unwrap();
        assert!(!session_id.is_empty());
        assert!(pm.has_process("telegram", "alice").await);

        // Cleanup
        pm.kill("telegram", "alice").await;
    }

    #[tokio::test]
    async fn spawn_duplicate_fails() {
        let pm = ProcessManager::new();
        let tmp = TempDir::new().unwrap();
        let binding_table = BindingTable::load(tmp.path()).await.unwrap();
        let (channel1, _) = mock_channel();
        let (channel2, _) = mock_channel();

        pm.spawn(
            "telegram",
            "alice",
            "chat123",
            None,
            &cat_agent_config(),
            tmp.path(),
            0,
            channel1,
            &binding_table,
        )
        .await
        .unwrap();

        let result = pm
            .spawn(
                "telegram",
                "alice",
                "chat123",
                None,
                &cat_agent_config(),
                tmp.path(),
                0,
                channel2,
                &binding_table,
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already running"));

        pm.kill("telegram", "alice").await;
    }

    #[tokio::test]
    async fn kill_returns_session_id() {
        let pm = ProcessManager::new();
        let tmp = TempDir::new().unwrap();
        let binding_table = BindingTable::load(tmp.path()).await.unwrap();
        let (channel, _) = mock_channel();

        let session_id = pm
            .spawn(
                "telegram",
                "alice",
                "chat123",
                None,
                &cat_agent_config(),
                tmp.path(),
                0,
                channel,
                &binding_table,
            )
            .await
            .unwrap();

        let killed = pm.kill("telegram", "alice").await;
        assert_eq!(killed, Some(session_id));
        assert!(!pm.has_process("telegram", "alice").await);
    }

    #[tokio::test]
    async fn send_message_to_running_process() {
        let pm = ProcessManager::new();
        let tmp = TempDir::new().unwrap();
        let binding_table = BindingTable::load(tmp.path()).await.unwrap();
        let (channel, _) = mock_channel();

        pm.spawn(
            "telegram",
            "alice",
            "chat123",
            None,
            &cat_agent_config(),
            tmp.path(),
            0,
            channel,
            &binding_table,
        )
        .await
        .unwrap();

        let result = pm.send_message("telegram", "alice", "hello world").await;
        assert!(result.is_ok());
        let pending = result.unwrap();
        assert!(pending >= 1);

        pm.kill("telegram", "alice").await;
    }

    #[tokio::test]
    async fn kill_all_cleans_up() {
        let pm = ProcessManager::new();
        let tmp = TempDir::new().unwrap();
        let binding_table = BindingTable::load(tmp.path()).await.unwrap();
        let (ch1, _) = mock_channel();
        let (ch2, _) = mock_channel();

        pm.spawn(
            "telegram",
            "alice",
            "chat1",
            None,
            &cat_agent_config(),
            tmp.path(),
            0,
            ch1,
            &binding_table,
        )
        .await
        .unwrap();
        pm.spawn(
            "telegram",
            "bob",
            "chat2",
            None,
            &cat_agent_config(),
            tmp.path(),
            0,
            ch2,
            &binding_table,
        )
        .await
        .unwrap();

        assert!(pm.has_process("telegram", "alice").await);
        assert!(pm.has_process("telegram", "bob").await);

        pm.kill_all().await;

        assert!(!pm.has_process("telegram", "alice").await);
        assert!(!pm.has_process("telegram", "bob").await);
    }

    #[tokio::test]
    async fn process_exit_records_crash_and_unbinds() {
        let pm = ProcessManager::new();
        let tmp = TempDir::new().unwrap();
        let binding_table = BindingTable::load(tmp.path()).await.unwrap();
        let (channel, sent) = mock_channel();

        // Create a binding first
        binding_table
            .bind(super::super::binding_table::SessionBinding {
                channel: "telegram".to_string(),
                sender: "alice".to_string(),
                agent_name: "test".to_string(),
                working_dir: tmp.path().to_path_buf(),
                bound_at: chrono::Utc::now(),
                stale: false,
            })
            .await
            .unwrap();

        // Use a command that exits immediately
        let exit_config = SessionBridgeAgentConfig {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "exit 0".to_string()],
        };

        pm.spawn(
            "telegram",
            "alice",
            "chat123",
            None,
            &exit_config,
            tmp.path(),
            0,
            channel,
            &binding_table,
        )
        .await
        .unwrap();

        // Wait for the stdout reader to detect EOF and process exit
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Process should be gone
        assert!(!pm.has_process("telegram", "alice").await);

        // Binding should be auto-unbound
        assert!(binding_table.lookup("telegram", "alice").await.is_none());

        // User should be notified
        let messages = sent.lock().unwrap();
        assert!(
            messages
                .iter()
                .any(|m| m.content.contains("session ended unexpectedly")),
            "user should be notified of process exit"
        );
    }

    #[tokio::test]
    async fn crash_cooldown_after_multiple_exits() {
        let pm = ProcessManager::new();
        let tmp = TempDir::new().unwrap();
        let binding_table = BindingTable::load(tmp.path()).await.unwrap();

        let exit_config = SessionBridgeAgentConfig {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "exit 1".to_string()],
        };

        // Spawn and let crash 3 times
        for i in 0..3 {
            let sender = format!("crash_user_{i}");
            // Use same channel+sender key each time — rebind for each spawn
            let (ch, _) = mock_channel();
            binding_table
                .bind(super::super::binding_table::SessionBinding {
                    channel: "telegram".to_string(),
                    sender: "crasher".to_string(),
                    agent_name: "test".to_string(),
                    working_dir: tmp.path().to_path_buf(),
                    bound_at: chrono::Utc::now(),
                    stale: false,
                })
                .await
                .ok(); // May fail if binding still exists; that's fine

            let _ = pm
                .spawn(
                    "telegram",
                    "crasher",
                    &sender,
                    None,
                    &exit_config,
                    tmp.path(),
                    0,
                    ch,
                    &binding_table,
                )
                .await;

            // Wait for crash detection
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        }

        // Should now be in cooldown
        assert!(pm.in_cooldown("telegram", "crasher").await);
    }

    #[tokio::test]
    async fn spawn_with_invalid_command_fails() {
        let pm = ProcessManager::new();
        let tmp = TempDir::new().unwrap();
        let binding_table = BindingTable::load(tmp.path()).await.unwrap();
        let (channel, _) = mock_channel();

        let bad_config = SessionBridgeAgentConfig {
            command: "/nonexistent/binary/that/does/not/exist".to_string(),
            args: vec![],
        };

        let result = pm
            .spawn(
                "telegram",
                "alice",
                "chat123",
                None,
                &bad_config,
                tmp.path(),
                0,
                channel,
                &binding_table,
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("failed to spawn"));
        assert!(!pm.has_process("telegram", "alice").await);
    }

    #[tokio::test]
    async fn pending_count_tracks_messages() {
        let pm = ProcessManager::new();
        let tmp = TempDir::new().unwrap();
        let binding_table = BindingTable::load(tmp.path()).await.unwrap();
        let (channel, _) = mock_channel();

        pm.spawn(
            "telegram",
            "alice",
            "chat123",
            None,
            &cat_agent_config(),
            tmp.path(),
            0,
            channel,
            &binding_table,
        )
        .await
        .unwrap();

        assert_eq!(pm.pending_count("telegram", "alice").await, 0);

        pm.send_message("telegram", "alice", "msg1").await.unwrap();
        assert!(pm.pending_count("telegram", "alice").await >= 1);

        pm.kill("telegram", "alice").await;
    }
}
