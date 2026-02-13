use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::error;

use super::events::{AgentMessage, InboundMessage, OutboundMessage};

/// Async message bus that decouples chat channels from the agent core.
pub struct MessageBus {
    inbound_tx: mpsc::Sender<InboundMessage>,
    inbound_rx: tokio::sync::Mutex<mpsc::Receiver<InboundMessage>>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    outbound_rx: tokio::sync::Mutex<mpsc::Receiver<OutboundMessage>>,

    // Inter-agent messaging (mpsc everywhere — many producers, single consumer per agent)
    agent_channels: tokio::sync::Mutex<HashMap<String, mpsc::Sender<AgentMessage>>>,
    master_tx: mpsc::Sender<AgentMessage>,
    master_rx: tokio::sync::Mutex<mpsc::Receiver<AgentMessage>>,
}

impl MessageBus {
    pub fn new(buffer_size: usize) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(buffer_size);
        let (outbound_tx, outbound_rx) = mpsc::channel(buffer_size);
        let (master_tx, master_rx) = mpsc::channel(buffer_size);
        Self {
            inbound_tx,
            inbound_rx: tokio::sync::Mutex::new(inbound_rx),
            outbound_tx,
            outbound_rx: tokio::sync::Mutex::new(outbound_rx),
            agent_channels: tokio::sync::Mutex::new(HashMap::new()),
            master_tx,
            master_rx: tokio::sync::Mutex::new(master_rx),
        }
    }

    /// Publish a message from a channel to the agent.
    pub async fn publish_inbound(&self, msg: InboundMessage) {
        if let Err(e) = self.inbound_tx.send(msg).await {
            error!("Failed to publish inbound message: {}", e);
        }
    }

    /// Consume the next inbound message (blocks until available).
    pub async fn consume_inbound(&self) -> Option<InboundMessage> {
        self.inbound_rx.lock().await.recv().await
    }

    /// Publish a response from the agent to channels.
    pub async fn publish_outbound(&self, msg: OutboundMessage) {
        if let Err(e) = self.outbound_tx.send(msg).await {
            error!("Failed to publish outbound message: {}", e);
        }
    }

    /// Consume the next outbound message (blocks until available).
    pub async fn consume_outbound(&self) -> Option<OutboundMessage> {
        self.outbound_rx.lock().await.recv().await
    }

    /// Get a clone of the outbound sender (for tools that need to send messages).
    pub fn outbound_sender(&self) -> mpsc::Sender<OutboundMessage> {
        self.outbound_tx.clone()
    }

    /// Get a clone of the inbound sender (for channels and subagents).
    pub fn inbound_sender(&self) -> mpsc::Sender<InboundMessage> {
        self.inbound_tx.clone()
    }

    // ── Inter-agent messaging ──────────────────────────────────────────

    /// Register a named agent and return its receive channel.
    pub async fn register_agent(&self, name: &str) -> mpsc::Receiver<AgentMessage> {
        let (tx, rx) = mpsc::channel(64);
        self.agent_channels
            .lock()
            .await
            .insert(name.to_string(), tx);
        rx
    }

    /// Send a message to a specific named agent.
    pub async fn send_to_agent(&self, name: &str, msg: AgentMessage) -> crate::error::Result<()> {
        let channels = self.agent_channels.lock().await;
        let tx = channels.get(name).ok_or_else(|| {
            crate::error::RustyClawError::Agent(format!("Unknown agent: {}", name))
        })?;
        tx.send(msg).await.map_err(|e| {
            crate::error::RustyClawError::Agent(format!("Failed to send to agent {}: {}", name, e))
        })
    }

    /// Send a message to master's inbox.
    pub async fn send_to_master(&self, msg: AgentMessage) -> crate::error::Result<()> {
        self.master_tx.send(msg).await.map_err(|e| {
            crate::error::RustyClawError::Agent(format!("Failed to send to master: {}", e))
        })
    }

    /// Broadcast a message to all registered agents.
    pub async fn broadcast(&self, msg: AgentMessage) -> crate::error::Result<()> {
        let channels = self.agent_channels.lock().await;
        for (name, tx) in channels.iter() {
            if let Err(e) = tx.send(msg.clone()).await {
                error!("Failed to broadcast to agent {}: {}", name, e);
            }
        }
        Ok(())
    }

    /// Consume the next message from master's inbox.
    pub async fn consume_master(&self) -> Option<AgentMessage> {
        self.master_rx.lock().await.recv().await
    }

    /// Get a clone of the master sender (for workers to send to master).
    pub fn master_sender(&self) -> mpsc::Sender<AgentMessage> {
        self.master_tx.clone()
    }
}
