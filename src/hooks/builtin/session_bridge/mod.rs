pub mod binding_table;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::channels::traits::{Channel, ChannelMessage};
use crate::config::SessionBridgeConfig;
use crate::hooks::traits::{HookHandler, HookResult};

use binding_table::BindingTable;

/// Runtime commands that the session bridge does NOT intercept.
/// These pass through to the normal RustyClaw command handling.
const PASSTHROUGH_COMMANDS: &[&str] = &["/models", "/model", "/new"];

/// Channels supported by the session bridge. Messages from other channels
/// pass through untouched — the bridge commands won't accidentally swallow
/// Slack/Discord/etc. slash commands.
const SUPPORTED_CHANNELS: &[&str] = &["telegram"];

/// Hook that intercepts messages to route bound conversations to Claude Code
/// CLI processes instead of the normal LLM agent loop.
///
/// Priority 100 ensures this runs before other hooks so it can cancel
/// message processing for bound sessions.
pub struct SessionBridgeHook {
    #[allow(dead_code)]
    channels_by_name: Arc<HashMap<String, Arc<dyn Channel>>>,
    #[allow(dead_code)]
    config: SessionBridgeConfig,
    #[allow(dead_code)]
    allowed_users: Vec<String>,
    binding_table: BindingTable,
}

impl SessionBridgeHook {
    pub async fn new(
        channels_by_name: Arc<HashMap<String, Arc<dyn Channel>>>,
        config: SessionBridgeConfig,
        allowed_users: Vec<String>,
        config_dir: &Path,
    ) -> Result<Self> {
        let binding_table = BindingTable::load(config_dir).await?;
        Ok(Self {
            channels_by_name,
            config,
            allowed_users,
            binding_table,
        })
    }
}

#[async_trait]
impl HookHandler for SessionBridgeHook {
    fn name(&self) -> &str {
        "session-bridge"
    }

    fn priority(&self) -> i32 {
        100
    }

    async fn on_message_received(&self, message: ChannelMessage) -> HookResult<ChannelMessage> {
        // Only process messages from supported channels
        if !SUPPORTED_CHANNELS.contains(&message.channel.as_str()) {
            return HookResult::Continue(message);
        }

        let content = message.content.trim();

        // 1. /btw → passthrough to Elixir orchestrator
        if content.starts_with("/btw ") || content == "/btw" {
            return HookResult::Continue(message);
        }

        // 2. Known RustyClaw runtime commands → passthrough
        let base_command = content.split_whitespace().next().unwrap_or("");
        if PASSTHROUGH_COMMANDS.contains(&base_command) {
            return HookResult::Continue(message);
        }

        // 3. /connect → handle connect (stub)
        if base_command == "/connect" {
            tracing::info!(
                hook = "session-bridge",
                channel = %message.channel,
                sender = %message.sender,
                "connect command received (stub)"
            );
            return HookResult::Cancel("Session bridge connect not yet implemented".to_string());
        }

        // 4. /disconnect → handle disconnect (stub)
        if base_command == "/disconnect" {
            tracing::info!(
                hook = "session-bridge",
                channel = %message.channel,
                sender = %message.sender,
                "disconnect command received (stub)"
            );
            return HookResult::Cancel("Session bridge disconnect not yet implemented".to_string());
        }

        // 5. /status → handle status (stub)
        if base_command == "/status" {
            tracing::info!(
                hook = "session-bridge",
                channel = %message.channel,
                sender = %message.sender,
                "status command received (stub)"
            );
            return HookResult::Cancel("Session bridge status not yet implemented".to_string());
        }

        // 6. Check binding table
        if let Some(binding) = self
            .binding_table
            .lookup(&message.channel, &message.sender)
            .await
        {
            // 6a. Stale binding → auto-unbind and let message through
            if binding.stale {
                tracing::info!(
                    hook = "session-bridge",
                    channel = %message.channel,
                    sender = %message.sender,
                    agent = %binding.agent_name,
                    "cleaning up stale binding from previous run"
                );
                if let Err(e) = self
                    .binding_table
                    .unbind(&message.channel, &message.sender)
                    .await
                {
                    tracing::error!(
                        hook = "session-bridge",
                        error = %e,
                        "failed to clean up stale binding"
                    );
                }
                // Let the message through to normal agent loop
                // TODO: notify user their session ended when RustyClaw restarted
                return HookResult::Continue(message);
            }

            // 6b. Active binding → route to child process (stub)
            tracing::info!(
                hook = "session-bridge",
                channel = %message.channel,
                sender = %message.sender,
                agent = %binding.agent_name,
                "routing message to bound session (stub)"
            );
            return HookResult::Cancel(format!(
                "Session bridge: message would be routed to agent '{}' (not yet implemented)",
                binding.agent_name
            ));
        }

        // 7. No binding → continue normal flow
        HookResult::Continue(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::traits::ChannelMessage;
    use tempfile::TempDir;

    fn test_message(content: &str) -> ChannelMessage {
        ChannelMessage {
            id: "1".to_string(),
            sender: "alice".to_string(),
            reply_target: "chat123".to_string(),
            content: content.to_string(),
            channel: "telegram".to_string(),
            timestamp: 0,
            thread_ts: None,
        }
    }

    fn test_message_on_channel(content: &str, channel: &str) -> ChannelMessage {
        ChannelMessage {
            id: "1".to_string(),
            sender: "alice".to_string(),
            reply_target: "chat123".to_string(),
            content: content.to_string(),
            channel: channel.to_string(),
            timestamp: 0,
            thread_ts: None,
        }
    }

    async fn make_hook(config_dir: &Path) -> SessionBridgeHook {
        SessionBridgeHook::new(
            Arc::new(HashMap::new()),
            SessionBridgeConfig::default(),
            vec!["alice".to_string()],
            config_dir,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn btw_passes_through() {
        let tmp = TempDir::new().unwrap();
        let hook = make_hook(tmp.path()).await;
        let msg = test_message("/btw hello elixir");
        assert!(!hook.on_message_received(msg).await.is_cancel());
    }

    #[tokio::test]
    async fn runtime_commands_pass_through() {
        let tmp = TempDir::new().unwrap();
        let hook = make_hook(tmp.path()).await;

        for cmd in &["/models", "/model gpt-4", "/new"] {
            let msg = test_message(cmd);
            assert!(
                !hook.on_message_received(msg).await.is_cancel(),
                "{cmd} should pass through"
            );
        }
    }

    #[tokio::test]
    async fn connect_cancels() {
        let tmp = TempDir::new().unwrap();
        let hook = make_hook(tmp.path()).await;
        let msg = test_message("/connect claude ~/projects");
        assert!(hook.on_message_received(msg).await.is_cancel());
    }

    #[tokio::test]
    async fn disconnect_cancels() {
        let tmp = TempDir::new().unwrap();
        let hook = make_hook(tmp.path()).await;
        let msg = test_message("/disconnect");
        assert!(hook.on_message_received(msg).await.is_cancel());
    }

    #[tokio::test]
    async fn status_cancels() {
        let tmp = TempDir::new().unwrap();
        let hook = make_hook(tmp.path()).await;
        let msg = test_message("/status");
        assert!(hook.on_message_received(msg).await.is_cancel());
    }

    #[tokio::test]
    async fn unbound_message_continues() {
        let tmp = TempDir::new().unwrap();
        let hook = make_hook(tmp.path()).await;
        let msg = test_message("hello world");
        assert!(!hook.on_message_received(msg).await.is_cancel());
    }

    #[tokio::test]
    async fn bound_message_cancels() {
        let tmp = TempDir::new().unwrap();
        let hook = make_hook(tmp.path()).await;

        // Create a binding
        hook.binding_table
            .bind(binding_table::SessionBinding {
                channel: "telegram".to_string(),
                sender: "alice".to_string(),
                agent_name: "claude".to_string(),
                working_dir: std::path::PathBuf::from("/tmp"),
                bound_at: chrono::Utc::now(),
                stale: false,
            })
            .await
            .unwrap();

        let msg = test_message("hello world");
        assert!(hook.on_message_received(msg).await.is_cancel());
    }

    #[tokio::test]
    async fn stale_binding_auto_unbinds_and_continues() {
        let tmp = TempDir::new().unwrap();

        // Create a binding, persist, then reload (marks stale)
        {
            let hook = make_hook(tmp.path()).await;
            hook.binding_table
                .bind(binding_table::SessionBinding {
                    channel: "telegram".to_string(),
                    sender: "alice".to_string(),
                    agent_name: "claude".to_string(),
                    working_dir: std::path::PathBuf::from("/tmp"),
                    bound_at: chrono::Utc::now(),
                    stale: false,
                })
                .await
                .unwrap();
        }

        // Reload — binding is now stale
        let hook = make_hook(tmp.path()).await;
        let found = hook
            .binding_table
            .lookup("telegram", "alice")
            .await
            .unwrap();
        assert!(found.stale);

        // Message should pass through (stale binding auto-cleaned)
        let msg = test_message("hello world");
        assert!(!hook.on_message_received(msg).await.is_cancel());

        // Binding should be removed
        assert!(hook
            .binding_table
            .lookup("telegram", "alice")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn non_telegram_channel_passes_through() {
        let tmp = TempDir::new().unwrap();
        let hook = make_hook(tmp.path()).await;

        // /connect on Discord should NOT be intercepted
        let msg = test_message_on_channel("/connect claude ~/projects", "discord");
        assert!(!hook.on_message_received(msg).await.is_cancel());

        // /status on Slack should NOT be intercepted
        let msg = test_message_on_channel("/status", "slack");
        assert!(!hook.on_message_received(msg).await.is_cancel());
    }
}
