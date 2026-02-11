use async_trait::async_trait;
use tracing::warn;

use crate::bus::events::{InboundMessage, OutboundMessage};
use crate::bus::queue::MessageBus;

/// Trait for chat channel implementations.
#[async_trait]
pub trait Channel: Send + Sync {
    /// Channel name (e.g., "telegram", "discord").
    fn name(&self) -> &str;

    /// Start the channel (long-running, connects to platform).
    async fn start(&self) -> crate::error::Result<()>;

    /// Stop the channel gracefully.
    async fn stop(&self) -> crate::error::Result<()>;

    /// Send a message to the platform.
    async fn send(&self, msg: OutboundMessage) -> crate::error::Result<()>;
}

/// Check if a sender is allowed based on the allowlist.
///
/// An empty allowlist permits all senders. Supports pipe-delimited
/// composite identifiers (e.g., "123|username" matches either "123" or "username").
pub fn is_allowed(sender_id: &str, allow_from: &[String]) -> bool {
    if allow_from.is_empty() {
        return true;
    }

    // Check direct match
    if allow_from.iter().any(|a| a == sender_id) {
        return true;
    }

    // Check pipe-delimited parts
    for part in sender_id.split('|') {
        if allow_from.iter().any(|a| a == part) {
            return true;
        }
    }

    false
}

/// Common message handling: check permissions and forward to bus.
pub async fn handle_message(
    bus: &MessageBus,
    channel_name: &str,
    sender_id: &str,
    chat_id: &str,
    content: &str,
    allow_from: &[String],
) {
    if !is_allowed(sender_id, allow_from) {
        warn!(
            "Denied message from {} on {}. Add to allowFrom to grant access.",
            sender_id, channel_name
        );
        return;
    }

    let msg = InboundMessage::new(channel_name, sender_id, chat_id, content);
    bus.publish_inbound(msg).await;
}
