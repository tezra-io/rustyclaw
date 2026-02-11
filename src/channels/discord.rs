use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use crate::bus::events::OutboundMessage;
use crate::bus::queue::MessageBus;
use crate::config::schema::DiscordConfig;

/// Discord channel using raw Gateway WebSocket protocol.
pub struct DiscordChannel {
    config: DiscordConfig,
    bus: Arc<MessageBus>,
}

impl DiscordChannel {
    pub fn new(config: DiscordConfig, bus: Arc<MessageBus>) -> Self {
        Self { config, bus }
    }
}

#[async_trait]
impl super::base::Channel for DiscordChannel {
    fn name(&self) -> &str {
        "discord"
    }

    async fn start(&self) -> crate::error::Result<()> {
        info!("Discord channel starting (Gateway WebSocket)");

        loop {
            match self.gateway_loop().await {
                Ok(_) => info!("Discord gateway disconnected, reconnecting..."),
                Err(e) => {
                    error!("Discord gateway error: {}, reconnecting in 5s...", e);
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn stop(&self) -> crate::error::Result<()> {
        info!("Discord channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> crate::error::Result<()> {
        let client = reqwest::Client::new();
        let url = format!(
            "https://discord.com/api/v10/channels/{}/messages",
            msg.chat_id
        );

        let body = serde_json::json!({ "content": msg.content });

        // Retry on rate limit (429)
        for attempt in 0..3 {
            let resp = client
                .post(&url)
                .header("Authorization", format!("Bot {}", self.config.token))
                .json(&body)
                .send()
                .await
                .map_err(|e| crate::error::NanobotError::Http(e.to_string()))?;

            if resp.status().as_u16() == 429 {
                if let Ok(data) = resp.json::<serde_json::Value>().await {
                    let retry_after = data["retry_after"].as_f64().unwrap_or(1.0);
                    warn!("Discord rate limited, retrying in {}s", retry_after);
                    tokio::time::sleep(std::time::Duration::from_secs_f64(retry_after)).await;
                    continue;
                }
            }

            if resp.status().is_success() {
                return Ok(());
            } else {
                let text = resp.text().await.unwrap_or_default();
                return Err(crate::error::NanobotError::Channel(format!(
                    "Discord API error: {}",
                    text
                )));
            }
        }

        Err(crate::error::NanobotError::Channel(
            "Discord send failed after retries".into(),
        ))
    }
}

impl DiscordChannel {
    /// Main gateway WebSocket loop handling Discord protocol.
    async fn gateway_loop(&self) -> crate::error::Result<()> {
        let (ws_stream, _) =
            tokio_tungstenite::connect_async(&self.config.gateway_url)
                .await
                .map_err(|e| crate::error::NanobotError::WebSocket(e.to_string()))?;

        let (mut write, mut read) = ws_stream.split();
        let mut sequence: Option<u64> = None;

        while let Some(msg) = read.next().await {
            let msg = msg.map_err(|e| crate::error::NanobotError::WebSocket(e.to_string()))?;

            if let WsMessage::Text(text) = msg {
                let payload: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|e| crate::error::NanobotError::Json(e.into()))?;

                let op = payload["op"].as_u64().unwrap_or(0);
                if let Some(s) = payload["s"].as_u64() {
                    sequence = Some(s);
                }

                match op {
                    10 => {
                        // HELLO — start heartbeat and identify
                        let interval = payload["d"]["heartbeat_interval"]
                            .as_u64()
                            .unwrap_or(45000);

                        // Identify
                        let identify = serde_json::json!({
                            "op": 2,
                            "d": {
                                "token": self.config.token,
                                "intents": self.config.intents,
                                "properties": {
                                    "os": "linux",
                                    "browser": "nanobot",
                                    "device": "nanobot"
                                }
                            }
                        });

                        write
                            .send(WsMessage::Text(identify.to_string().into()))
                            .await
                            .ok();

                        // Start heartbeat task
                        let seq = sequence;
                        // TODO: Spawn heartbeat task with interval
                    }
                    0 => {
                        // Dispatch event
                        let event = payload["t"].as_str().unwrap_or("");
                        if event == "MESSAGE_CREATE" {
                            let d = &payload["d"];
                            let chat_id = d["channel_id"].as_str().unwrap_or("").to_string();
                            let sender_id = d["author"]["id"].as_str().unwrap_or("").to_string();
                            let content = d["content"].as_str().unwrap_or("").to_string();

                            // Skip bot messages
                            if d["author"]["bot"].as_bool().unwrap_or(false) {
                                continue;
                            }

                            super::base::handle_message(
                                &self.bus,
                                "discord",
                                &sender_id,
                                &chat_id,
                                &content,
                                &self.config.allow_from,
                            )
                            .await;
                        }
                    }
                    7 | 9 => {
                        // RECONNECT or INVALID_SESSION
                        warn!("Discord requested reconnect (op={})", op);
                        break;
                    }
                    11 => {
                        // HEARTBEAT_ACK — no action needed
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }
}
