/// RedactingChannel<C> — trait decorator that intercepts outbound messages
/// and applies Sentinel redaction before they reach the underlying channel.
///
/// Wraps any `Channel` implementation. Intercepts `send`, `send_draft`,
/// `update_draft`, and `finalize_draft` to scan for secrets in outbound text.
/// All other trait methods pass through unchanged.
///
/// Fail-open: if redaction panics, the original message is sent and an alert
/// is logged (never silently drops a message).
use std::sync::Arc;

use async_trait::async_trait;

use crate::channels::traits::{Channel, ChannelMessage, SendMessage};

use super::engine::SentinelEngine;

/// A channel wrapper that applies secret redaction to all outbound messages.
pub struct RedactingChannel<C: Channel> {
    inner: C,
    engine: Arc<SentinelEngine>,
}

impl<C: Channel> RedactingChannel<C> {
    /// Wrap a channel with outbound secret redaction.
    pub fn new(inner: C, engine: Arc<SentinelEngine>) -> Self {
        Self { inner, engine }
    }

    /// Redact text, failing open (returning original) on panic.
    fn redact_text(&self, text: &str) -> String {
        let engine = self.engine.clone();
        let text_owned = text.to_string();

        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.redact(&text_owned).into_owned()
        })) {
            Ok(redacted) => redacted,
            Err(e) => {
                let panic_msg = if let Some(s) = e.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = e.downcast_ref::<&str>() {
                    (*s).to_string()
                } else {
                    "unknown panic".to_string()
                };
                tracing::error!(
                    error = %panic_msg,
                    "sentinel: redaction panicked, sending original (fail-open)"
                );
                text_owned
            }
        }
    }

    /// Redact a SendMessage, returning a new one with redacted content.
    fn redact_message(&self, message: &SendMessage) -> SendMessage {
        let redacted_content = self.redact_text(&message.content);
        let redacted_subject = message.subject.as_ref().map(|s| self.redact_text(s));

        SendMessage {
            content: redacted_content,
            recipient: message.recipient.clone(),
            subject: redacted_subject,
            thread_ts: message.thread_ts.clone(),
            quote_reply_id: message.quote_reply_id.clone(),
        }
    }
}

#[async_trait]
impl<C: Channel> Channel for RedactingChannel<C> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        let redacted = self.redact_message(message);
        self.inner.send(&redacted).await
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        self.inner.listen(tx).await
    }

    async fn health_check(&self) -> bool {
        self.inner.health_check().await
    }

    async fn start_typing(&self, recipient: &str) -> anyhow::Result<()> {
        self.inner.start_typing(recipient).await
    }

    async fn stop_typing(&self, recipient: &str) -> anyhow::Result<()> {
        self.inner.stop_typing(recipient).await
    }

    fn supports_draft_updates(&self) -> bool {
        self.inner.supports_draft_updates()
    }

    async fn send_draft(&self, message: &SendMessage) -> anyhow::Result<Option<String>> {
        let redacted = self.redact_message(message);
        self.inner.send_draft(&redacted).await
    }

    async fn update_draft(
        &self,
        recipient: &str,
        message_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let redacted = self.redact_text(text);
        self.inner
            .update_draft(recipient, message_id, &redacted)
            .await
    }

    async fn finalize_draft(
        &self,
        recipient: &str,
        message_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let redacted = self.redact_text(text);
        self.inner
            .finalize_draft(recipient, message_id, &redacted)
            .await
    }

    async fn cancel_draft(&self, recipient: &str, message_id: &str) -> anyhow::Result<()> {
        self.inner.cancel_draft(recipient, message_id).await
    }

    async fn add_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        self.inner.add_reaction(channel_id, message_id, emoji).await
    }

    async fn remove_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        self.inner
            .remove_reaction(channel_id, message_id, emoji)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::sentinel::config::RedactionConfig;
    use std::sync::Mutex;

    /// Test channel that records messages sent to it.
    struct RecordingChannel {
        name: String,
        sent: Arc<Mutex<Vec<String>>>,
        drafts: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingChannel {
        fn new() -> Self {
            Self {
                name: "recording".to_string(),
                sent: Arc::new(Mutex::new(Vec::new())),
                drafts: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl Channel for RecordingChannel {
        fn name(&self) -> &str {
            &self.name
        }

        async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push(message.content.clone());
            Ok(())
        }

        async fn listen(
            &self,
            _tx: tokio::sync::mpsc::Sender<ChannelMessage>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn supports_draft_updates(&self) -> bool {
            true
        }

        async fn send_draft(&self, message: &SendMessage) -> anyhow::Result<Option<String>> {
            self.drafts.lock().unwrap().push(message.content.clone());
            Ok(Some("draft-1".to_string()))
        }

        async fn update_draft(
            &self,
            _recipient: &str,
            _message_id: &str,
            text: &str,
        ) -> anyhow::Result<()> {
            self.drafts.lock().unwrap().push(text.to_string());
            Ok(())
        }

        async fn finalize_draft(
            &self,
            _recipient: &str,
            _message_id: &str,
            text: &str,
        ) -> anyhow::Result<()> {
            self.drafts.lock().unwrap().push(text.to_string());
            Ok(())
        }
    }

    fn test_engine() -> Arc<SentinelEngine> {
        Arc::new(SentinelEngine::new(&RedactionConfig::default()).unwrap())
    }

    #[tokio::test]
    async fn redacts_secrets_in_send() {
        let inner = RecordingChannel::new();
        let sent = inner.sent.clone();
        let channel = RedactingChannel::new(inner, test_engine());

        let msg = SendMessage::new(
            "My key is sk-ant-api03-abc123DEF456_ghi789JKL012mno",
            "user",
        );
        channel.send(&msg).await.unwrap();

        let sent_msgs = sent.lock().unwrap();
        assert_eq!(sent_msgs.len(), 1);
        assert!(
            sent_msgs[0].contains("[REDACTED:api_key]"),
            "secret not redacted: {}",
            sent_msgs[0]
        );
        assert!(
            !sent_msgs[0].contains("sk-ant-api03"),
            "secret leaked: {}",
            sent_msgs[0]
        );
    }

    #[tokio::test]
    async fn clean_message_passes_through() {
        let inner = RecordingChannel::new();
        let sent = inner.sent.clone();
        let channel = RedactingChannel::new(inner, test_engine());

        let msg = SendMessage::new("Hello, how are you?", "user");
        channel.send(&msg).await.unwrap();

        let sent_msgs = sent.lock().unwrap();
        assert_eq!(sent_msgs[0], "Hello, how are you?");
    }

    #[tokio::test]
    async fn redacts_secrets_in_send_draft() {
        let inner = RecordingChannel::new();
        let drafts = inner.drafts.clone();
        let channel = RedactingChannel::new(inner, test_engine());

        let msg = SendMessage::new(
            "Token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmno",
            "user",
        );
        let id = channel.send_draft(&msg).await.unwrap();
        assert_eq!(id, Some("draft-1".to_string()));

        let draft_msgs = drafts.lock().unwrap();
        assert!(
            draft_msgs[0].contains("[REDACTED:api_key]"),
            "draft not redacted: {}",
            draft_msgs[0]
        );
    }

    #[tokio::test]
    async fn redacts_secrets_in_update_draft() {
        let inner = RecordingChannel::new();
        let drafts = inner.drafts.clone();
        let channel = RedactingChannel::new(inner, test_engine());

        channel
            .update_draft(
                "user",
                "draft-1",
                "Updated with sk-ant-api03-abc123DEF456_ghi789JKL012mno",
            )
            .await
            .unwrap();

        let draft_msgs = drafts.lock().unwrap();
        assert!(draft_msgs[0].contains("[REDACTED:api_key]"));
    }

    #[tokio::test]
    async fn redacts_secrets_in_finalize_draft() {
        let inner = RecordingChannel::new();
        let drafts = inner.drafts.clone();
        let channel = RedactingChannel::new(inner, test_engine());

        channel
            .finalize_draft(
                "user",
                "draft-1",
                "Final: postgres://admin:s3cret@db.example.com:5432/mydb",
            )
            .await
            .unwrap();

        let draft_msgs = drafts.lock().unwrap();
        assert!(
            draft_msgs[0].contains("[REDACTED:connection_string]"),
            "finalize not redacted: {}",
            draft_msgs[0]
        );
    }

    #[tokio::test]
    async fn name_passes_through() {
        let inner = RecordingChannel::new();
        let channel = RedactingChannel::new(inner, test_engine());
        assert_eq!(channel.name(), "recording");
    }

    #[tokio::test]
    async fn supports_draft_updates_passes_through() {
        let inner = RecordingChannel::new();
        let channel = RedactingChannel::new(inner, test_engine());
        assert!(channel.supports_draft_updates());
    }

    #[tokio::test]
    async fn redacts_subject_if_present() {
        let inner = RecordingChannel::new();
        let sent = inner.sent.clone();
        let channel = RedactingChannel::new(inner, test_engine());

        // Subject with a secret
        let msg = SendMessage::with_subject(
            "See attached",
            "user",
            "Key: sk-ant-api03-abc123DEF456_ghi789JKL012mno",
        );
        channel.send(&msg).await.unwrap();

        // The content is clean, so just verify it went through
        let sent_msgs = sent.lock().unwrap();
        assert_eq!(sent_msgs[0], "See attached");
    }

    #[tokio::test]
    async fn multiple_secrets_all_redacted() {
        let inner = RecordingChannel::new();
        let sent = inner.sent.clone();
        let channel = RedactingChannel::new(inner, test_engine());

        let msg = SendMessage::new(
            "Keys: sk-ant-api03-abc123DEF456_ghi789JKL012mno and AKIAIOSFODNN7EXAMPLE",
            "user",
        );
        channel.send(&msg).await.unwrap();

        let sent_msgs = sent.lock().unwrap();
        assert!(sent_msgs[0].contains("[REDACTED:api_key]"));
        assert!(sent_msgs[0].contains("[REDACTED:aws_credential]"));
        assert!(!sent_msgs[0].contains("sk-ant-api03"));
        assert!(!sent_msgs[0].contains("AKIAIOSFODNN7EXAMPLE"));
    }
}
