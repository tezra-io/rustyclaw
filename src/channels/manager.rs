use std::sync::Arc;
use tracing::{error, info, warn};

use crate::bus::queue::MessageBus;
use crate::config::schema::Config;

use super::base::Channel;

/// Manages all active channel instances.
pub struct ChannelManager {
    channels: Vec<Arc<dyn Channel>>,
    bus: Arc<MessageBus>,
    #[allow(dead_code)]
    config: Arc<Config>,
}

impl ChannelManager {
    pub fn new(config: Arc<Config>, bus: Arc<MessageBus>) -> Self {
        let mut channels: Vec<Arc<dyn Channel>> = Vec::new();

        // Register enabled channels
        if config.channels.telegram.enabled {
            info!("Enabling Telegram channel");
            channels.push(Arc::new(super::telegram::TelegramChannel::new(
                config.channels.telegram.clone(),
                bus.clone(),
            )));
        }

        if config.channels.discord.enabled {
            info!("Enabling Discord channel");
            channels.push(Arc::new(super::discord::DiscordChannel::new(
                config.channels.discord.clone(),
                bus.clone(),
            )));
        }

        // TODO: Add other channels (Slack, WhatsApp, Feishu, DingTalk, etc.)

        Self {
            channels,
            bus,
            config,
        }
    }

    /// Start all enabled channels and the outbound dispatcher.
    pub async fn start_all(&self) {
        // Start outbound dispatcher
        let bus = self.bus.clone();
        let channels = self.channels.clone();
        tokio::spawn(async move {
            dispatch_outbound(bus, channels).await;
        });

        // Start each channel concurrently
        let mut handles = Vec::new();
        for ch in &self.channels {
            let ch = ch.clone();
            handles.push(tokio::spawn(async move {
                if let Err(e) = ch.start().await {
                    error!("Channel {} failed to start: {}", ch.name(), e);
                }
            }));
        }

        // Wait for all channels (they run indefinitely)
        for h in handles {
            let _ = h.await;
        }
    }

    /// Stop all channels gracefully.
    pub async fn stop_all(&self) {
        for ch in &self.channels {
            if let Err(e) = ch.stop().await {
                error!("Error stopping channel {}: {}", ch.name(), e);
            }
        }
    }
}

/// Route outbound messages to the correct channel.
async fn dispatch_outbound(bus: Arc<MessageBus>, channels: Vec<Arc<dyn Channel>>) {
    while let Some(msg) = bus.consume_outbound().await {
        let target_channel = &msg.channel;
        let matched = channels.iter().find(|ch| ch.name() == target_channel);

        match matched {
            Some(ch) => {
                if let Err(e) = ch.send(msg).await {
                    error!("Failed to send outbound message: {}", e);
                }
            }
            None => {
                warn!("No channel handler for: {}", target_channel);
            }
        }
    }
}
