use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::bus::events::OutboundMessage;

/// Tool for the agent to send messages to channels.
pub struct MessageTool {
    sender: mpsc::Sender<OutboundMessage>,
    /// Current context: which channel/chat the agent is responding to.
    current_channel: std::sync::Mutex<Option<String>>,
    current_chat_id: std::sync::Mutex<Option<String>>,
}

impl MessageTool {
    pub fn new(sender: mpsc::Sender<OutboundMessage>) -> Self {
        Self {
            sender,
            current_channel: std::sync::Mutex::new(None),
            current_chat_id: std::sync::Mutex::new(None),
        }
    }

    /// Set the current message context (called before tool execution).
    pub fn set_context(&self, channel: &str, chat_id: &str) {
        *self.current_channel.lock().unwrap() = Some(channel.to_string());
        *self.current_chat_id.lock().unwrap() = Some(chat_id.to_string());
    }
}

#[async_trait]
impl super::base::Tool for MessageTool {
    fn name(&self) -> &str { "message" }

    fn description(&self) -> &str {
        "Send a message to a specific channel and chat."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "channel": { "type": "string", "description": "Target channel (optional, defaults to current)" },
                "chat_id": { "type": "string", "description": "Target chat ID (optional, defaults to current)" },
                "content": { "type": "string", "description": "Message content" }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> crate::error::Result<String> {
        let content = args["content"]
            .as_str()
            .ok_or_else(|| crate::error::NanobotError::Tool("Missing 'content'".into()))?;

        let channel = args["channel"]
            .as_str()
            .map(String::from)
            .or_else(|| self.current_channel.lock().unwrap().clone())
            .ok_or_else(|| crate::error::NanobotError::Tool("No channel context set".into()))?;

        let chat_id = args["chat_id"]
            .as_str()
            .map(String::from)
            .or_else(|| self.current_chat_id.lock().unwrap().clone())
            .ok_or_else(|| crate::error::NanobotError::Tool("No chat_id context set".into()))?;

        let msg = OutboundMessage::new(&channel, &chat_id, content);
        self.sender.send(msg).await.map_err(|e| {
            crate::error::NanobotError::Channel(format!("Failed to send message: {}", e))
        })?;

        Ok("Message sent.".into())
    }
}
