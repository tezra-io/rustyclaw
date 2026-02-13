use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Message received from a chat channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessage {
    pub channel: String,
    pub sender_id: String,
    pub chat_id: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub media: Vec<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl InboundMessage {
    pub fn new(channel: &str, sender_id: &str, chat_id: &str, content: &str) -> Self {
        Self {
            channel: channel.to_string(),
            sender_id: sender_id.to_string(),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
            media: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    /// Unique key for session identification: "channel:chat_id"
    pub fn session_key(&self) -> String {
        format!("{}:{}", self.channel, self.chat_id)
    }
}

/// Message to send to a chat channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundMessage {
    pub channel: String,
    pub chat_id: String,
    pub content: String,
    pub reply_to: Option<String>,
    pub media: Vec<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl OutboundMessage {
    pub fn new(channel: &str, chat_id: &str, content: &str) -> Self {
        Self {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            reply_to: None,
            media: Vec::new(),
            metadata: HashMap::new(),
        }
    }
}

/// Message type for inter-agent communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMessageType {
    Task,      // master -> worker: do this
    Result,    // worker -> master: here's what I found
    Query,     // worker -> master: I need info
    Alert,     // worker -> master: something's wrong
    Broadcast, // master -> all: context update
    Shutdown,  // master -> worker: stop gracefully
}

/// Message exchanged between agents via the internal message bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub from: String,
    pub to: String,
    pub msg_type: AgentMessageType,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub trace_id: String,
    pub delegation_depth: u8,
}

/// Maximum delegation depth to prevent infinite recursion.
pub const MAX_DELEGATION_DEPTH: u8 = 3;

impl AgentMessage {
    pub fn new(from: &str, to: &str, msg_type: AgentMessageType, content: &str) -> Self {
        Self {
            from: from.to_string(),
            to: to.to_string(),
            msg_type,
            content: content.to_string(),
            timestamp: Utc::now(),
            trace_id: Uuid::new_v4().to_string(),
            delegation_depth: 0,
        }
    }

    /// Create a reply message, inheriting trace_id and incrementing delegation_depth.
    pub fn reply(&self, msg_type: AgentMessageType, content: &str) -> Self {
        Self {
            from: self.to.clone(),
            to: self.from.clone(),
            msg_type,
            content: content.to_string(),
            timestamp: Utc::now(),
            trace_id: self.trace_id.clone(),
            delegation_depth: self.delegation_depth.saturating_add(1),
        }
    }
}
