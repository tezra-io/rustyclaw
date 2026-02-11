use tokio::sync::mpsc;
use tracing::error;

use super::events::{InboundMessage, OutboundMessage};

/// Async message bus that decouples chat channels from the agent core.
pub struct MessageBus {
    inbound_tx: mpsc::Sender<InboundMessage>,
    inbound_rx: tokio::sync::Mutex<mpsc::Receiver<InboundMessage>>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    outbound_rx: tokio::sync::Mutex<mpsc::Receiver<OutboundMessage>>,
}

impl MessageBus {
    pub fn new(buffer_size: usize) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(buffer_size);
        let (outbound_tx, outbound_rx) = mpsc::channel(buffer_size);
        Self {
            inbound_tx,
            inbound_rx: tokio::sync::Mutex::new(inbound_rx),
            outbound_tx,
            outbound_rx: tokio::sync::Mutex::new(outbound_rx),
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
}
