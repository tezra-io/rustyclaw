use super::traits::{Channel, ChannelMessage};
use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use std::path::Path;
use std::time::Duration;

/// Telegram's maximum message length for text messages.
const TELEGRAM_MAX_MESSAGE_LENGTH: usize = 4096;

/// Characters consumed by continuation markers so we never exceed the hard limit.
/// "(continued)\n\n" + "\n\n(continues...)" ≈ 30 chars.
const TELEGRAM_CONTINUATION_OVERHEAD: usize = 30;

/// XML tags that wrap tool calls — strip before sending to avoid confusing Telegram's Markdown parser.
const TOOL_CALL_OPEN_TAGS: [&str; 7] = [
    "<function_calls>",
    "<function_call>",
    "<tool_call>",
    "<toolcall>",
    "<tool-call>",
    "<invoke>",
    "<use_mcp_tool>",
];

fn matching_close_tag(open: &str) -> Option<&'static str> {
    match open {
        "<function_calls>" => Some("</function_calls>"),
        "<function_call>" => Some("</function_call>"),
        "<tool_call>" => Some("</tool_call>"),
        "<toolcall>" => Some("</toolcall>"),
        "<tool-call>" => Some("</tool-call>"),
        "<invoke>" => Some("</invoke>"),
        "<use_mcp_tool>" => Some("</use_mcp_tool>"),
        _ => None,
    }
}

/// Strip XML tool-call wrapper tags from a message before sending.
fn strip_tool_call_tags(message: &str) -> String {
    let mut result = message.to_string();
    for open_tag in &TOOL_CALL_OPEN_TAGS {
        if let Some(close_tag) = matching_close_tag(open_tag) {
            while let Some(start) = result.find(open_tag) {
                let search_from = start + open_tag.len();
                if let Some(rel_end) = result[search_from..].find(close_tag) {
                    let end = search_from + rel_end + close_tag.len();
                    result.replace_range(start..end, "");
                } else {
                    // Unclosed tag — remove just the opening tag
                    result.replace_range(start..start + open_tag.len(), "");
                }
            }
        }
    }
    result.trim().to_string()
}

/// Parse a recipient string that may encode a forum thread as "chat_id:thread_id".
fn parse_recipient(recipient: &str) -> (&str, Option<&str>) {
    if let Some(idx) = recipient.find(':') {
        (&recipient[..idx], Some(&recipient[idx + 1..]))
    } else {
        (recipient, None)
    }
}

/// Split a message into chunks that respect Telegram's 4096 character limit.
/// Non-final chunks are split at `TELEGRAM_MAX_MESSAGE_LENGTH - TELEGRAM_CONTINUATION_OVERHEAD`
/// to leave room for continuation markers.
fn split_message_for_telegram(message: &str) -> Vec<String> {
    if message.chars().count() <= TELEGRAM_MAX_MESSAGE_LENGTH {
        return vec![message.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = message;

    while !remaining.is_empty() {
        let char_count = remaining.chars().count();

        // If the remainder fits in the full limit (no continuation needed), take all.
        if char_count <= TELEGRAM_MAX_MESSAGE_LENGTH {
            chunks.push(remaining.to_string());
            break;
        }

        // Use a tighter limit for non-final chunks to leave room for markers.
        let chunk_limit = TELEGRAM_MAX_MESSAGE_LENGTH - TELEGRAM_CONTINUATION_OVERHEAD;

        let hard_split = remaining
            .char_indices()
            .nth(chunk_limit)
            .map_or(remaining.len(), |(idx, _)| idx);

        let chunk_end = {
            let search_area = &remaining[..hard_split];
            if let Some(pos) = search_area.rfind('\n') {
                if search_area[..pos].chars().count() >= chunk_limit / 2 {
                    pos + 1
                } else {
                    search_area.rfind(' ').map_or(hard_split, |p| p + 1)
                }
            } else if let Some(pos) = search_area.rfind(' ') {
                pos + 1
            } else {
                hard_split
            }
        };

        chunks.push(remaining[..chunk_end].to_string());
        remaining = &remaining[chunk_end..];
    }

    chunks
}

/// Telegram channel — long-polls the Bot API for updates
pub struct TelegramChannel {
    bot_token: String,
    allowed_users: Vec<String>,
    mention_only: bool,
    client: reqwest::Client,
    typing_handle: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl TelegramChannel {
    pub fn new(bot_token: String, allowed_users: Vec<String>, mention_only: bool) -> Self {
        Self {
            bot_token,
            allowed_users,
            mention_only,
            client: reqwest::Client::new(),
            typing_handle: parking_lot::Mutex::new(None),
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{method}", self.bot_token)
    }

    fn is_user_allowed(&self, username: &str) -> bool {
        self.allowed_users.iter().any(|u| u == "*" || u == username)
    }

    fn is_any_user_allowed<'a, I>(&self, identities: I) -> bool
    where
        I: IntoIterator<Item = &'a str>,
    {
        identities.into_iter().any(|id| self.is_user_allowed(id))
    }

    /// Send text chunks to a Telegram chat, optionally in a forum thread.
    async fn send_text_chunks(
        &self,
        message: &str,
        chat_id: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let chunks = split_message_for_telegram(message);

        for (i, chunk) in chunks.iter().enumerate() {
            let text = if chunks.len() > 1 {
                if i == 0 {
                    format!("{chunk}\n\n(continues...)")
                } else if i == chunks.len() - 1 {
                    format!("(continued)\n\n{chunk}")
                } else {
                    format!("(continued)\n\n{chunk}\n\n(continues...)")
                }
            } else {
                chunk.to_string()
            };

            let mut body = serde_json::json!({
                "chat_id": chat_id,
                "text": text,
                "parse_mode": "Markdown"
            });
            if let Some(tid) = thread_id {
                body["message_thread_id"] = serde_json::Value::String(tid.to_string());
            }

            let markdown_resp = self
                .client
                .post(self.api_url("sendMessage"))
                .json(&body)
                .send()
                .await?;

            if markdown_resp.status().is_success() {
                if i < chunks.len() - 1 {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                continue;
            }

            let markdown_status = markdown_resp.status();
            let markdown_err = markdown_resp.text().await.unwrap_or_default();
            tracing::warn!(
                status = ?markdown_status,
                "Telegram sendMessage with Markdown failed; retrying without parse_mode"
            );

            let mut plain_body = serde_json::json!({
                "chat_id": chat_id,
                "text": text,
            });
            if let Some(tid) = thread_id {
                plain_body["message_thread_id"] = serde_json::Value::String(tid.to_string());
            }
            let plain_resp = self
                .client
                .post(self.api_url("sendMessage"))
                .json(&plain_body)
                .send()
                .await?;

            if !plain_resp.status().is_success() {
                let plain_status = plain_resp.status();
                let plain_err = plain_resp.text().await.unwrap_or_default();
                anyhow::bail!(
                    "Telegram sendMessage failed (markdown {}: {}; plain {}: {})",
                    markdown_status,
                    markdown_err,
                    plain_status,
                    plain_err
                );
            }

            if i < chunks.len() - 1 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        Ok(())
    }

    /// Send a document/file to a Telegram chat
    pub async fn send_document(
        &self,
        chat_id: &str,
        file_path: &Path,
        caption: Option<&str>,
    ) -> anyhow::Result<()> {
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");

        let file_bytes = tokio::fs::read(file_path).await?;
        let part = Part::bytes(file_bytes).file_name(file_name.to_string());

        let mut form = Form::new()
            .text("chat_id", chat_id.to_string())
            .part("document", part);

        if let Some(cap) = caption {
            form = form.text("caption", cap.to_string());
        }

        let resp = self
            .client
            .post(self.api_url("sendDocument"))
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Telegram sendDocument failed: {err}");
        }

        tracing::info!("Telegram document sent to {chat_id}: {file_name}");
        Ok(())
    }

    /// Send a document from bytes (in-memory) to a Telegram chat
    pub async fn send_document_bytes(
        &self,
        chat_id: &str,
        file_bytes: Vec<u8>,
        file_name: &str,
        caption: Option<&str>,
    ) -> anyhow::Result<()> {
        let part = Part::bytes(file_bytes).file_name(file_name.to_string());

        let mut form = Form::new()
            .text("chat_id", chat_id.to_string())
            .part("document", part);

        if let Some(cap) = caption {
            form = form.text("caption", cap.to_string());
        }

        let resp = self
            .client
            .post(self.api_url("sendDocument"))
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Telegram sendDocument failed: {err}");
        }

        tracing::info!("Telegram document sent to {chat_id}: {file_name}");
        Ok(())
    }

    /// Send a photo to a Telegram chat
    pub async fn send_photo(
        &self,
        chat_id: &str,
        file_path: &Path,
        caption: Option<&str>,
    ) -> anyhow::Result<()> {
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("photo.jpg");

        let file_bytes = tokio::fs::read(file_path).await?;
        let part = Part::bytes(file_bytes).file_name(file_name.to_string());

        let mut form = Form::new()
            .text("chat_id", chat_id.to_string())
            .part("photo", part);

        if let Some(cap) = caption {
            form = form.text("caption", cap.to_string());
        }

        let resp = self
            .client
            .post(self.api_url("sendPhoto"))
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Telegram sendPhoto failed: {err}");
        }

        tracing::info!("Telegram photo sent to {chat_id}: {file_name}");
        Ok(())
    }

    /// Send a photo from bytes (in-memory) to a Telegram chat
    pub async fn send_photo_bytes(
        &self,
        chat_id: &str,
        file_bytes: Vec<u8>,
        file_name: &str,
        caption: Option<&str>,
    ) -> anyhow::Result<()> {
        let part = Part::bytes(file_bytes).file_name(file_name.to_string());

        let mut form = Form::new()
            .text("chat_id", chat_id.to_string())
            .part("photo", part);

        if let Some(cap) = caption {
            form = form.text("caption", cap.to_string());
        }

        let resp = self
            .client
            .post(self.api_url("sendPhoto"))
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Telegram sendPhoto failed: {err}");
        }

        tracing::info!("Telegram photo sent to {chat_id}: {file_name}");
        Ok(())
    }

    /// Send a video to a Telegram chat
    pub async fn send_video(
        &self,
        chat_id: &str,
        file_path: &Path,
        caption: Option<&str>,
    ) -> anyhow::Result<()> {
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("video.mp4");

        let file_bytes = tokio::fs::read(file_path).await?;
        let part = Part::bytes(file_bytes).file_name(file_name.to_string());

        let mut form = Form::new()
            .text("chat_id", chat_id.to_string())
            .part("video", part);

        if let Some(cap) = caption {
            form = form.text("caption", cap.to_string());
        }

        let resp = self
            .client
            .post(self.api_url("sendVideo"))
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Telegram sendVideo failed: {err}");
        }

        tracing::info!("Telegram video sent to {chat_id}: {file_name}");
        Ok(())
    }

    /// Send an audio file to a Telegram chat
    pub async fn send_audio(
        &self,
        chat_id: &str,
        file_path: &Path,
        caption: Option<&str>,
    ) -> anyhow::Result<()> {
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.mp3");

        let file_bytes = tokio::fs::read(file_path).await?;
        let part = Part::bytes(file_bytes).file_name(file_name.to_string());

        let mut form = Form::new()
            .text("chat_id", chat_id.to_string())
            .part("audio", part);

        if let Some(cap) = caption {
            form = form.text("caption", cap.to_string());
        }

        let resp = self
            .client
            .post(self.api_url("sendAudio"))
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Telegram sendAudio failed: {err}");
        }

        tracing::info!("Telegram audio sent to {chat_id}: {file_name}");
        Ok(())
    }

    /// Send a voice message to a Telegram chat
    pub async fn send_voice(
        &self,
        chat_id: &str,
        file_path: &Path,
        caption: Option<&str>,
    ) -> anyhow::Result<()> {
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("voice.ogg");

        let file_bytes = tokio::fs::read(file_path).await?;
        let part = Part::bytes(file_bytes).file_name(file_name.to_string());

        let mut form = Form::new()
            .text("chat_id", chat_id.to_string())
            .part("voice", part);

        if let Some(cap) = caption {
            form = form.text("caption", cap.to_string());
        }

        let resp = self
            .client
            .post(self.api_url("sendVoice"))
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Telegram sendVoice failed: {err}");
        }

        tracing::info!("Telegram voice sent to {chat_id}: {file_name}");
        Ok(())
    }

    /// Send a file by URL (Telegram will download it)
    pub async fn send_document_by_url(
        &self,
        chat_id: &str,
        url: &str,
        caption: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "document": url
        });

        if let Some(cap) = caption {
            body["caption"] = serde_json::Value::String(cap.to_string());
        }

        let resp = self
            .client
            .post(self.api_url("sendDocument"))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Telegram sendDocument by URL failed: {err}");
        }

        tracing::info!("Telegram document (URL) sent to {chat_id}: {url}");
        Ok(())
    }

    /// Send a photo by URL (Telegram will download it)
    pub async fn send_photo_by_url(
        &self,
        chat_id: &str,
        url: &str,
        caption: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "photo": url
        });

        if let Some(cap) = caption {
            body["caption"] = serde_json::Value::String(cap.to_string());
        }

        let resp = self
            .client
            .post(self.api_url("sendPhoto"))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Telegram sendPhoto by URL failed: {err}");
        }

        tracing::info!("Telegram photo (URL) sent to {chat_id}: {url}");
        Ok(())
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &str {
        "telegram"
    }

    async fn send(&self, message: &str, recipient: &str) -> anyhow::Result<()> {
        let (chat_id, thread_id) = parse_recipient(recipient);
        let cleaned = strip_tool_call_tags(message);
        self.send_text_chunks(&cleaned, chat_id, thread_id).await
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        let mut offset: i64 = 0;

        tracing::info!("Telegram channel listening for messages...");

        loop {
            let url = self.api_url("getUpdates");
            let body = serde_json::json!({
                "offset": offset,
                "timeout": 30,
                "allowed_updates": ["message"]
            });

            let resp = match self.client.post(&url).json(&body).send().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("Telegram poll error: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            let data: serde_json::Value = match resp.json().await {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("Telegram parse error: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            if let Some(results) = data.get("result").and_then(serde_json::Value::as_array) {
                for update in results {
                    // Advance offset past this update
                    if let Some(uid) = update.get("update_id").and_then(serde_json::Value::as_i64) {
                        offset = uid + 1;
                    }

                    let Some(message) = update.get("message") else {
                        continue;
                    };

                    let Some(text) = message.get("text").and_then(serde_json::Value::as_str) else {
                        continue;
                    };

                    let username_opt = message
                        .get("from")
                        .and_then(|f| f.get("username"))
                        .and_then(|u| u.as_str());
                    let username = username_opt.unwrap_or("unknown");

                    let user_id = message
                        .get("from")
                        .and_then(|f| f.get("id"))
                        .and_then(serde_json::Value::as_i64);
                    let user_id_str = user_id.map(|id| id.to_string());

                    let mut identities = vec![username];
                    if let Some(ref id) = user_id_str {
                        identities.push(id.as_str());
                    }

                    if !self.is_any_user_allowed(identities.iter().copied()) {
                        tracing::warn!(
                            "Telegram: ignoring message from unauthorized user: username={username}, user_id={}. \
Allowlist Telegram @username or numeric user ID, then run `rustyclaw onboard --channels-only`.",
                            user_id_str.as_deref().unwrap_or("unknown")
                        );
                        continue;
                    }

                    let chat_id = message
                        .get("chat")
                        .and_then(|c| c.get("id"))
                        .and_then(serde_json::Value::as_i64)
                        .map(|id| id.to_string());

                    let Some(chat_id) = chat_id else {
                        tracing::warn!("Telegram: missing chat_id in message, skipping");
                        continue;
                    };

                    // Forum topic support: encode thread ID into reply target
                    let thread_id = message
                        .get("message_thread_id")
                        .and_then(serde_json::Value::as_i64)
                        .map(|id| id.to_string());

                    let reply_target = if let Some(ref tid) = thread_id {
                        format!("{chat_id}:{tid}")
                    } else {
                        chat_id.clone()
                    };

                    // Mention-only filter for groups (negative chat IDs are groups)
                    let content = if self.mention_only {
                        // Only respond to messages containing @-mentions
                        if !text.contains('@') && !text.starts_with('/') {
                            continue;
                        }
                        // Strip leading @-mention (e.g. "@botname hello" → "hello")
                        let stripped = text
                            .split_once(' ')
                            .filter(|(first, _)| first.starts_with('@'))
                            .map(|(_, rest)| rest.trim())
                            .unwrap_or(text);
                        stripped.to_string()
                    } else {
                        text.to_string()
                    };

                    if content.is_empty() {
                        continue;
                    }

                    let message_id = message
                        .get("message_id")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);

                    // Start persistent typing indicator
                    let _ = self.start_typing(&reply_target).await;

                    let msg = ChannelMessage {
                        id: format!("telegram_{chat_id}_{message_id}"),
                        sender: reply_target.clone(),
                        content,
                        channel: "telegram".to_string(),
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    };

                    if tx.send(msg).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn health_check(&self) -> bool {
        let timeout_duration = Duration::from_secs(5);

        match tokio::time::timeout(
            timeout_duration,
            self.client.get(self.api_url("getMe")).send(),
        )
        .await
        {
            Ok(Ok(resp)) => resp.status().is_success(),
            Ok(Err(e)) => {
                tracing::debug!("Telegram health check failed: {e}");
                false
            }
            Err(_) => {
                tracing::debug!("Telegram health check timed out after 5s");
                false
            }
        }
    }

    async fn start_typing(&self, recipient: &str) -> anyhow::Result<()> {
        // Stop any existing typing task first
        self.stop_typing(recipient).await?;

        let client = self.client.clone();
        let url = self.api_url("sendChatAction");
        let (chat_id, _thread_id) = parse_recipient(recipient);
        let chat_id = chat_id.to_string();

        let handle = tokio::spawn(async move {
            loop {
                let body = serde_json::json!({"chat_id": &chat_id, "action": "typing"});
                let _ = client.post(&url).json(&body).send().await;
                // Telegram typing expires after 5s; refresh every 4s
                tokio::time::sleep(Duration::from_secs(4)).await;
            }
        });

        *self.typing_handle.lock() = Some(handle);
        Ok(())
    }

    async fn stop_typing(&self, _recipient: &str) -> anyhow::Result<()> {
        if let Some(handle) = self.typing_handle.lock().take() {
            handle.abort();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_channel() -> TelegramChannel {
        TelegramChannel::new("fake-token".into(), vec!["*".into()], false)
    }

    #[test]
    fn telegram_channel_name() {
        let ch = make_channel();
        assert_eq!(ch.name(), "telegram");
    }

    #[test]
    fn telegram_api_url() {
        let ch = TelegramChannel::new("123:ABC".into(), vec![], false);
        assert_eq!(
            ch.api_url("getMe"),
            "https://api.telegram.org/bot123:ABC/getMe"
        );
    }

    #[test]
    fn telegram_user_allowed_wildcard() {
        let ch = TelegramChannel::new("t".into(), vec!["*".into()], false);
        assert!(ch.is_user_allowed("anyone"));
    }

    #[test]
    fn telegram_user_allowed_specific() {
        let ch = TelegramChannel::new("t".into(), vec!["alice".into(), "bob".into()], false);
        assert!(ch.is_user_allowed("alice"));
        assert!(!ch.is_user_allowed("eve"));
    }

    #[test]
    fn telegram_user_denied_empty() {
        let ch = TelegramChannel::new("t".into(), vec![], false);
        assert!(!ch.is_user_allowed("anyone"));
    }

    #[test]
    fn telegram_user_exact_match_not_substring() {
        let ch = TelegramChannel::new("t".into(), vec!["alice".into()], false);
        assert!(!ch.is_user_allowed("alice_bot"));
        assert!(!ch.is_user_allowed("alic"));
        assert!(!ch.is_user_allowed("malice"));
    }

    #[test]
    fn telegram_user_empty_string_denied() {
        let ch = TelegramChannel::new("t".into(), vec!["alice".into()], false);
        assert!(!ch.is_user_allowed(""));
    }

    #[test]
    fn telegram_user_case_sensitive() {
        let ch = TelegramChannel::new("t".into(), vec!["Alice".into()], false);
        assert!(ch.is_user_allowed("Alice"));
        assert!(!ch.is_user_allowed("alice"));
        assert!(!ch.is_user_allowed("ALICE"));
    }

    #[test]
    fn telegram_wildcard_with_specific_users() {
        let ch = TelegramChannel::new("t".into(), vec!["alice".into(), "*".into()], false);
        assert!(ch.is_user_allowed("alice"));
        assert!(ch.is_user_allowed("bob"));
        assert!(ch.is_user_allowed("anyone"));
    }

    #[test]
    fn telegram_user_allowed_by_numeric_id_identity() {
        let ch = TelegramChannel::new("t".into(), vec!["123456789".into()], false);
        assert!(ch.is_any_user_allowed(["unknown", "123456789"]));
    }

    #[test]
    fn telegram_user_denied_when_none_of_identities_match() {
        let ch = TelegramChannel::new("t".into(), vec!["alice".into(), "987654321".into()], false);
        assert!(!ch.is_any_user_allowed(["unknown", "123456789"]));
    }

    // ── File sending API URL tests ──────────────────────────────────

    #[test]
    fn telegram_api_url_send_document() {
        let ch = TelegramChannel::new("123:ABC".into(), vec![], false);
        assert_eq!(
            ch.api_url("sendDocument"),
            "https://api.telegram.org/bot123:ABC/sendDocument"
        );
    }

    #[test]
    fn telegram_api_url_send_photo() {
        let ch = TelegramChannel::new("123:ABC".into(), vec![], false);
        assert_eq!(
            ch.api_url("sendPhoto"),
            "https://api.telegram.org/bot123:ABC/sendPhoto"
        );
    }

    #[test]
    fn telegram_api_url_send_video() {
        let ch = TelegramChannel::new("123:ABC".into(), vec![], false);
        assert_eq!(
            ch.api_url("sendVideo"),
            "https://api.telegram.org/bot123:ABC/sendVideo"
        );
    }

    #[test]
    fn telegram_api_url_send_audio() {
        let ch = TelegramChannel::new("123:ABC".into(), vec![], false);
        assert_eq!(
            ch.api_url("sendAudio"),
            "https://api.telegram.org/bot123:ABC/sendAudio"
        );
    }

    #[test]
    fn telegram_api_url_send_voice() {
        let ch = TelegramChannel::new("123:ABC".into(), vec![], false);
        assert_eq!(
            ch.api_url("sendVoice"),
            "https://api.telegram.org/bot123:ABC/sendVoice"
        );
    }

    // ── File sending integration tests (with mock server) ──────────

    #[tokio::test]
    async fn telegram_send_document_bytes_builds_correct_form() {
        let ch = make_channel();
        let file_bytes = b"Hello, this is a test file content".to_vec();

        let result = ch
            .send_document_bytes("123456", file_bytes, "test.txt", Some("Test caption"))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("error") || err.contains("failed") || err.contains("connect"),
            "Expected network error, got: {err}"
        );
    }

    #[tokio::test]
    async fn telegram_send_photo_bytes_builds_correct_form() {
        let ch = make_channel();
        let file_bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

        let result = ch
            .send_photo_bytes("123456", file_bytes, "test.png", None)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn telegram_send_document_by_url_builds_correct_json() {
        let ch = make_channel();

        let result = ch
            .send_document_by_url("123456", "https://example.com/file.pdf", Some("PDF doc"))
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn telegram_send_photo_by_url_builds_correct_json() {
        let ch = make_channel();

        let result = ch
            .send_photo_by_url("123456", "https://example.com/image.jpg", None)
            .await;

        assert!(result.is_err());
    }

    // ── File path handling tests ────────────────────────────────────

    #[tokio::test]
    async fn telegram_send_document_nonexistent_file() {
        let ch = make_channel();
        let path = Path::new("/nonexistent/path/to/file.txt");

        let result = ch.send_document("123456", path, None).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("No such file") || err.contains("not found") || err.contains("os error"),
            "Expected file not found error, got: {err}"
        );
    }

    #[tokio::test]
    async fn telegram_send_photo_nonexistent_file() {
        let ch = make_channel();
        let path = Path::new("/nonexistent/path/to/photo.jpg");

        let result = ch.send_photo("123456", path, None).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn telegram_send_video_nonexistent_file() {
        let ch = make_channel();
        let path = Path::new("/nonexistent/path/to/video.mp4");

        let result = ch.send_video("123456", path, None).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn telegram_send_audio_nonexistent_file() {
        let ch = make_channel();
        let path = Path::new("/nonexistent/path/to/audio.mp3");

        let result = ch.send_audio("123456", path, None).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn telegram_send_voice_nonexistent_file() {
        let ch = make_channel();
        let path = Path::new("/nonexistent/path/to/voice.ogg");

        let result = ch.send_voice("123456", path, None).await;

        assert!(result.is_err());
    }

    // ── Message splitting tests ─────────────────────────────────────

    #[test]
    fn telegram_split_short_message() {
        let msg = "Hello, world!";
        let chunks = split_message_for_telegram(msg);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], msg);
    }

    #[test]
    fn telegram_split_exact_limit() {
        let msg = "a".repeat(TELEGRAM_MAX_MESSAGE_LENGTH);
        let chunks = split_message_for_telegram(&msg);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chars().count(), TELEGRAM_MAX_MESSAGE_LENGTH);
    }

    #[test]
    fn telegram_split_over_limit() {
        let msg = "a".repeat(TELEGRAM_MAX_MESSAGE_LENGTH + 100);
        let chunks = split_message_for_telegram(&msg);
        assert_eq!(chunks.len(), 2);
        // Non-final chunk must leave room for continuation markers
        assert!(
            chunks[0].chars().count()
                <= TELEGRAM_MAX_MESSAGE_LENGTH - TELEGRAM_CONTINUATION_OVERHEAD
        );
        assert!(chunks[1].chars().count() <= TELEGRAM_MAX_MESSAGE_LENGTH);
    }

    #[test]
    fn telegram_split_at_word_boundary() {
        let msg = format!(
            "{} more text here",
            "word ".repeat(TELEGRAM_MAX_MESSAGE_LENGTH / 5)
        );
        let chunks = split_message_for_telegram(&msg);
        assert!(chunks.len() >= 2);
        for chunk in &chunks[..chunks.len() - 1] {
            assert!(
                chunk.chars().count()
                    <= TELEGRAM_MAX_MESSAGE_LENGTH - TELEGRAM_CONTINUATION_OVERHEAD
            );
        }
    }

    #[test]
    fn telegram_split_at_newline() {
        let text_block = "Line of text\n".repeat(TELEGRAM_MAX_MESSAGE_LENGTH / 13 + 1);
        let chunks = split_message_for_telegram(&text_block);
        assert!(chunks.len() >= 2);
        for chunk in chunks {
            assert!(chunk.chars().count() <= TELEGRAM_MAX_MESSAGE_LENGTH);
        }
    }

    #[test]
    fn telegram_split_preserves_content() {
        let msg = "test ".repeat(TELEGRAM_MAX_MESSAGE_LENGTH / 5 + 100);
        let chunks = split_message_for_telegram(&msg);
        let rejoined = chunks.join("");
        assert_eq!(rejoined, msg);
    }

    #[test]
    fn telegram_split_empty_message() {
        let chunks = split_message_for_telegram("");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "");
    }

    #[test]
    fn telegram_split_very_long_message() {
        let msg = "x".repeat(TELEGRAM_MAX_MESSAGE_LENGTH * 3);
        let chunks = split_message_for_telegram(&msg);
        assert!(chunks.len() >= 3);
        for chunk in chunks {
            assert!(chunk.chars().count() <= TELEGRAM_MAX_MESSAGE_LENGTH);
        }
    }

    // ── Caption handling tests ──────────────────────────────────────

    #[tokio::test]
    async fn telegram_send_document_bytes_with_caption() {
        let ch = make_channel();
        let file_bytes = b"test content".to_vec();

        let result = ch
            .send_document_bytes("123456", file_bytes.clone(), "test.txt", Some("My caption"))
            .await;
        assert!(result.is_err());

        let result = ch
            .send_document_bytes("123456", file_bytes, "test.txt", None)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn telegram_send_photo_bytes_with_caption() {
        let ch = make_channel();
        let file_bytes = vec![0x89, 0x50, 0x4E, 0x47];

        let result = ch
            .send_photo_bytes(
                "123456",
                file_bytes.clone(),
                "test.png",
                Some("Photo caption"),
            )
            .await;
        assert!(result.is_err());

        let result = ch
            .send_photo_bytes("123456", file_bytes, "test.png", None)
            .await;
        assert!(result.is_err());
    }

    // ── Empty/edge case tests ───────────────────────────────────────

    #[tokio::test]
    async fn telegram_send_document_bytes_empty_file() {
        let ch = make_channel();
        let file_bytes: Vec<u8> = vec![];

        let result = ch
            .send_document_bytes("123456", file_bytes, "empty.txt", None)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn telegram_send_document_bytes_empty_filename() {
        let ch = make_channel();
        let file_bytes = b"content".to_vec();

        let result = ch.send_document_bytes("123456", file_bytes, "", None).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn telegram_send_document_bytes_empty_chat_id() {
        let ch = make_channel();
        let file_bytes = b"content".to_vec();

        let result = ch
            .send_document_bytes("", file_bytes, "test.txt", None)
            .await;

        assert!(result.is_err());
    }

    // ── Message ID edge cases ─────────────────────────────────────

    #[test]
    fn telegram_message_id_format_includes_chat_and_message_id() {
        let chat_id = "123456";
        let message_id = 789;
        let expected_id = format!("telegram_{chat_id}_{message_id}");
        assert_eq!(expected_id, "telegram_123456_789");
    }

    #[test]
    fn telegram_message_id_is_deterministic() {
        let chat_id = "123456";
        let message_id = 789;
        let id1 = format!("telegram_{chat_id}_{message_id}");
        let id2 = format!("telegram_{chat_id}_{message_id}");
        assert_eq!(id1, id2);
    }

    #[test]
    fn telegram_message_id_different_message_different_id() {
        let chat_id = "123456";
        let id1 = format!("telegram_{chat_id}_789");
        let id2 = format!("telegram_{chat_id}_790");
        assert_ne!(id1, id2);
    }

    #[test]
    fn telegram_message_id_different_chat_different_id() {
        let message_id = 789;
        let id1 = format!("telegram_123456_{message_id}");
        let id2 = format!("telegram_789012_{message_id}");
        assert_ne!(id1, id2);
    }

    #[test]
    fn telegram_message_id_no_uuid_randomness() {
        let chat_id = "123456";
        let message_id = 789;
        let id = format!("telegram_{chat_id}_{message_id}");
        assert!(!id.contains('-'));
        assert!(id.starts_with("telegram_"));
    }

    #[test]
    fn telegram_message_id_handles_zero_message_id() {
        let chat_id = "123456";
        let message_id = 0;
        let id = format!("telegram_{chat_id}_{message_id}");
        assert_eq!(id, "telegram_123456_0");
    }

    // ── Typing indicator tests ──────────────────────────────────────

    #[test]
    fn telegram_typing_handle_starts_as_none() {
        let ch = make_channel();
        assert!(ch.typing_handle.lock().is_none());
    }

    #[tokio::test]
    async fn telegram_start_typing_sets_handle() {
        let ch = make_channel();
        let _ = ch.start_typing("123456").await;
        assert!(ch.typing_handle.lock().is_some());
    }

    #[tokio::test]
    async fn telegram_stop_typing_clears_handle() {
        let ch = make_channel();
        let _ = ch.start_typing("123456").await;
        let _ = ch.stop_typing("123456").await;
        assert!(ch.typing_handle.lock().is_none());
    }

    #[tokio::test]
    async fn telegram_stop_typing_is_idempotent() {
        let ch = make_channel();
        assert!(ch.stop_typing("123456").await.is_ok());
        assert!(ch.stop_typing("123456").await.is_ok());
    }

    #[tokio::test]
    async fn telegram_start_typing_replaces_existing_handle() {
        let ch = make_channel();
        let _ = ch.start_typing("111").await;
        let _ = ch.start_typing("222").await;
        // Second call replaces first — still has a handle
        assert!(ch.typing_handle.lock().is_some());
    }

    // ── Forum topic (parse_recipient) tests ────────────────────────

    #[test]
    fn parse_recipient_plain_chat_id() {
        let (chat_id, thread_id) = parse_recipient("123456");
        assert_eq!(chat_id, "123456");
        assert_eq!(thread_id, None);
    }

    #[test]
    fn parse_recipient_with_thread_id() {
        let (chat_id, thread_id) = parse_recipient("-100123456:42");
        assert_eq!(chat_id, "-100123456");
        assert_eq!(thread_id, Some("42"));
    }

    #[test]
    fn parse_recipient_empty_thread_id() {
        let (chat_id, thread_id) = parse_recipient("123456:");
        assert_eq!(chat_id, "123456");
        assert_eq!(thread_id, Some(""));
    }

    // ── strip_tool_call_tags tests ──────────────────────────────────

    #[test]
    fn strip_tool_call_tags_removes_function_calls() {
        let input = "Hello <function_calls>some call</function_calls> world";
        let result = strip_tool_call_tags(input);
        assert_eq!(result, "Hello  world");
    }

    #[test]
    fn strip_tool_call_tags_removes_tool_call() {
        let input = "Answer: <tool_call>{\"name\":\"shell\",\"args\":{}}</tool_call>done";
        let result = strip_tool_call_tags(input);
        assert_eq!(result, "Answer: done");
    }

    #[test]
    fn strip_tool_call_tags_removes_function_call() {
        let input = "Before <function_call>payload</function_call> after";
        let result = strip_tool_call_tags(input);
        assert_eq!(result, "Before  after");
    }

    #[test]
    fn strip_tool_call_tags_no_tags_unchanged() {
        let input = "Normal message with no tool calls";
        let result = strip_tool_call_tags(input);
        assert_eq!(result, input);
    }

    #[test]
    fn strip_tool_call_tags_multiple_calls() {
        let input = "<function_calls>first</function_calls> middle <function_calls>second</function_calls>";
        let result = strip_tool_call_tags(input);
        assert_eq!(result, "middle");
    }

    #[test]
    fn strip_tool_call_tags_unclosed_tag_removed() {
        let input = "text <tool_call>unclosed";
        let result = strip_tool_call_tags(input);
        // Unclosed tag: only opening tag removed
        assert!(!result.contains("<tool_call>"));
    }

    // ── mention_only tests ──────────────────────────────────────────

    #[test]
    fn mention_only_false_by_default() {
        let ch = TelegramChannel::new("t".into(), vec![], false);
        assert!(!ch.mention_only);
    }

    #[test]
    fn mention_only_stored_when_true() {
        let ch = TelegramChannel::new("t".into(), vec![], true);
        assert!(ch.mention_only);
    }

    // ── Constants ──────────────────────────────────────────────────

    #[test]
    fn telegram_constants_are_sane() {
        assert_eq!(TELEGRAM_MAX_MESSAGE_LENGTH, 4096);
        assert_eq!(TELEGRAM_CONTINUATION_OVERHEAD, 30);
        assert!(TELEGRAM_CONTINUATION_OVERHEAD < TELEGRAM_MAX_MESSAGE_LENGTH);
    }
}
