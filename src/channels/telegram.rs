use async_trait::async_trait;
use std::sync::Arc;
use tracing::{error, info};

use crate::bus::events::OutboundMessage;
use crate::bus::queue::MessageBus;
use crate::config::schema::TelegramConfig;

/// Telegram channel using long polling (no webhooks needed).
pub struct TelegramChannel {
    config: TelegramConfig,
    bus: Arc<MessageBus>,
}

impl TelegramChannel {
    pub fn new(config: TelegramConfig, bus: Arc<MessageBus>) -> Self {
        Self { config, bus }
    }
}

#[async_trait]
impl super::base::Channel for TelegramChannel {
    fn name(&self) -> &str {
        "telegram"
    }

    async fn start(&self) -> crate::error::Result<()> {
        info!("Telegram channel starting (long polling)");

        // TODO: Implement using teloxide or direct Bot API
        // 1. Call getUpdates in a loop with offset tracking
        // 2. Handle text messages, voice (with transcription), media
        // 3. Send typing indicators while processing
        // 4. Register /start, /reset, /help commands

        // Placeholder: long poll loop
        let client = reqwest::Client::new();
        let base_url = format!("https://api.telegram.org/bot{}", self.config.token);
        let mut offset: i64 = 0;

        loop {
            let url = format!("{}/getUpdates?offset={}&timeout=30", base_url, offset);

            match client.get(&url).send().await {
                Ok(resp) => {
                    if let Ok(data) = resp.json::<serde_json::Value>().await {
                        if let Some(updates) = data["result"].as_array() {
                            for update in updates {
                                if let Some(uid) = update["update_id"].as_i64() {
                                    offset = uid + 1;
                                }
                                // Process message
                                if let Some(msg) = update.get("message") {
                                    let chat_id = msg["chat"]["id"].to_string();
                                    let sender_id = format!(
                                        "{}|{}",
                                        msg["from"]["id"],
                                        msg["from"]["username"].as_str().unwrap_or("")
                                    );
                                    let text = msg["text"].as_str().unwrap_or("");

                                    super::base::handle_message(
                                        &self.bus,
                                        "telegram",
                                        &sender_id,
                                        &chat_id,
                                        text,
                                        &self.config.allow_from,
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Telegram poll error: {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn stop(&self) -> crate::error::Result<()> {
        info!("Telegram channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> crate::error::Result<()> {
        let client = reqwest::Client::new();
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.config.token
        );

        // Convert markdown to Telegram HTML
        let html = markdown_to_telegram_html(&msg.content);

        let body = serde_json::json!({
            "chat_id": msg.chat_id,
            "text": html,
            "parse_mode": "HTML",
        });

        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::error::NanobotError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            // Fallback to plain text on HTML parse error
            let fallback = serde_json::json!({
                "chat_id": msg.chat_id,
                "text": msg.content,
            });
            client.post(&url).json(&fallback).send().await.ok();
        }

        Ok(())
    }
}

/// Convert markdown to Telegram-compatible HTML.
fn markdown_to_telegram_html(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Bold: **text**
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(end) = find_closing(&chars, i + 2, &['*', '*']) {
                result.push_str("<b>");
                let inner: String = chars[i + 2..end].iter().collect();
                result.push_str(&inner);
                result.push_str("</b>");
                i = end + 2;
                continue;
            }
        }
        // Italic: __text__
        if i + 1 < len && chars[i] == '_' && chars[i + 1] == '_' {
            if let Some(end) = find_closing(&chars, i + 2, &['_', '_']) {
                result.push_str("<i>");
                let inner: String = chars[i + 2..end].iter().collect();
                result.push_str(&inner);
                result.push_str("</i>");
                i = end + 2;
                continue;
            }
        }
        // Code: `text`
        if chars[i] == '`' && !(i + 1 < len && chars[i + 1] == '`') {
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '`') {
                let end = i + 1 + end;
                result.push_str("<code>");
                let inner: String = chars[i + 1..end].iter().collect();
                result.push_str(&inner);
                result.push_str("</code>");
                i = end + 1;
                continue;
            }
        }
        // Escape HTML special chars
        match chars[i] {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            c => result.push(c),
        }
        i += 1;
    }

    result
}

/// Find closing marker (e.g., ** or __) starting from position `from`.
fn find_closing(chars: &[char], from: usize, marker: &[char]) -> Option<usize> {
    let mlen = marker.len();
    if from + mlen > chars.len() {
        return None;
    }
    for i in from..chars.len() - mlen + 1 {
        if chars[i..i + mlen] == *marker {
            return Some(i);
        }
    }
    None
}
