pub mod binding_table;
pub mod process_manager;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::channels::traits::{Channel, ChannelMessage, SendMessage};
use crate::config::SessionBridgeConfig;
use crate::hooks::traits::{HookHandler, HookResult};

use binding_table::BindingTable;
use process_manager::ProcessManager;

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
    channels_by_name: Arc<HashMap<String, Arc<dyn Channel>>>,
    config: SessionBridgeConfig,
    allowed_users: Vec<String>,
    binding_table: BindingTable,
    process_manager: ProcessManager,
}

impl SessionBridgeHook {
    pub async fn new(
        channels_by_name: Arc<HashMap<String, Arc<dyn Channel>>>,
        config: SessionBridgeConfig,
        allowed_users: Vec<String>,
        config_dir: &Path,
    ) -> Result<Self> {
        let binding_table = BindingTable::load(config_dir).await?;
        let process_manager = ProcessManager::new();
        Ok(Self {
            channels_by_name,
            config,
            allowed_users,
            binding_table,
            process_manager,
        })
    }

    async fn send_reply(&self, message: &ChannelMessage, content: &str) {
        if let Some(channel) = self.channels_by_name.get(&message.channel) {
            let reply = SendMessage::new(content, &message.reply_target)
                .in_thread(message.thread_ts.clone())
                .with_quote_reply(Some(message.id.clone()));
            if let Err(e) = channel.send(&reply).await {
                tracing::error!(
                    hook = "session-bridge",
                    error = %e,
                    "failed to send reply"
                );
            }
        }
    }

    async fn handle_connect(&self, message: &ChannelMessage) -> HookResult<ChannelMessage> {
        let content = message.content.trim();
        let args: Vec<&str> = content.split_whitespace().skip(1).collect();

        if args.is_empty() {
            self.send_reply(message, "Usage: /connect <agent> [path]")
                .await;
            return HookResult::Cancel("connect: no args".to_string());
        }

        let agent_name = args[0];

        // Validate agent exists in configuration
        if !self.config.agents.contains_key(agent_name) {
            let mut available: Vec<&str> = self.config.agents.keys().map(|s| s.as_str()).collect();
            available.sort_unstable();
            self.send_reply(
                message,
                &format!(
                    "Unknown agent: {}. Available: {}",
                    agent_name,
                    available.join(", ")
                ),
            )
            .await;
            return HookResult::Cancel("connect: unknown agent".to_string());
        }

        // Resolve working directory
        let work_dir = if args.len() > 1 {
            PathBuf::from(args[1])
        } else if let Some(ref default) = self.config.default_workspace {
            default.clone()
        } else {
            self.send_reply(message, "Usage: /connect <agent> [path]")
                .await;
            return HookResult::Cancel("connect: no workspace".to_string());
        };

        // Validate directory exists
        match tokio::fs::metadata(&work_dir).await {
            Ok(meta) if meta.is_dir() => {}
            _ => {
                self.send_reply(
                    message,
                    &format!("Directory not found: {}", work_dir.display()),
                )
                .await;
                return HookResult::Cancel("connect: invalid path".to_string());
            }
        }

        // Check if already bound (non-stale)
        if let Some(existing) = self
            .binding_table
            .lookup(&message.channel, &message.sender)
            .await
        {
            if !existing.stale {
                self.send_reply(
                    message,
                    &format!(
                        "Already connected to {} in {}. Send /disconnect first.",
                        existing.agent_name,
                        existing.working_dir.display()
                    ),
                )
                .await;
                return HookResult::Cancel("connect: already bound".to_string());
            }
        }

        // Check crash cooldown
        if self
            .process_manager
            .in_cooldown(&message.channel, &message.sender)
            .await
        {
            self.send_reply(
                message,
                "Too many recent crashes. Please wait before reconnecting.",
            )
            .await;
            return HookResult::Cancel("connect: cooldown".to_string());
        }

        // Check max sessions
        let active = self.binding_table.count_active().await;
        if active >= self.config.max_sessions {
            self.send_reply(
                message,
                &format!(
                    "Too many active sessions ({}/{})",
                    active, self.config.max_sessions
                ),
            )
            .await;
            return HookResult::Cancel("connect: max sessions".to_string());
        }

        // Create binding
        let binding = binding_table::SessionBinding {
            channel: message.channel.clone(),
            sender: message.sender.clone(),
            agent_name: agent_name.to_string(),
            working_dir: work_dir.clone(),
            bound_at: chrono::Utc::now(),
            stale: false,
        };

        if let Err(e) = self.binding_table.bind(binding).await {
            tracing::error!(hook = "session-bridge", error = %e, "bind failed");
            self.send_reply(message, &format!("Failed to connect: {e}"))
                .await;
            return HookResult::Cancel("connect: bind failed".to_string());
        }

        // Spawn child process
        let agent_config = &self.config.agents[agent_name];
        let channel_ref = match self.channels_by_name.get(&message.channel) {
            Some(ch) => ch.clone(),
            None => {
                // Rollback binding
                let _ = self
                    .binding_table
                    .unbind(&message.channel, &message.sender)
                    .await;
                self.send_reply(message, "Internal error: channel not found.")
                    .await;
                return HookResult::Cancel("connect: no channel".to_string());
            }
        };

        match self
            .process_manager
            .spawn(
                &message.channel,
                &message.sender,
                &message.reply_target,
                message.thread_ts.clone(),
                agent_config,
                &work_dir,
                self.config.output_buffer_ms,
                channel_ref,
                &self.binding_table,
            )
            .await
        {
            Ok(session_id) => {
                self.send_reply(
                    message,
                    &format!("Connected to {} in {}", agent_name, work_dir.display()),
                )
                .await;

                tracing::info!(
                    hook = "session-bridge",
                    channel = %message.channel,
                    sender = %message.sender,
                    agent = agent_name,
                    session_id = %session_id,
                    "session connected"
                );
            }
            Err(e) => {
                // Rollback binding on spawn failure
                let _ = self
                    .binding_table
                    .unbind(&message.channel, &message.sender)
                    .await;
                tracing::error!(hook = "session-bridge", error = %e, "spawn failed");
                self.send_reply(message, &format!("Failed to start session: {e}"))
                    .await;
                return HookResult::Cancel("connect: spawn failed".to_string());
            }
        }

        HookResult::Cancel("connect: success".to_string())
    }

    async fn handle_disconnect(&self, message: &ChannelMessage) -> HookResult<ChannelMessage> {
        match self
            .binding_table
            .lookup(&message.channel, &message.sender)
            .await
        {
            Some(binding) => {
                // Kill the child process if running
                self.process_manager
                    .kill(&message.channel, &message.sender)
                    .await;

                if let Err(e) = self
                    .binding_table
                    .unbind(&message.channel, &message.sender)
                    .await
                {
                    tracing::error!(hook = "session-bridge", error = %e, "unbind failed");
                }

                self.send_reply(
                    message,
                    &format!("Disconnected from {}.", binding.agent_name),
                )
                .await;

                tracing::info!(
                    hook = "session-bridge",
                    channel = %message.channel,
                    sender = %message.sender,
                    agent = %binding.agent_name,
                    stale = binding.stale,
                    "session disconnected"
                );
            }
            None => {
                self.send_reply(message, "No active session to disconnect.")
                    .await;
            }
        }
        HookResult::Cancel("disconnect".to_string())
    }

    async fn handle_status(&self, message: &ChannelMessage) -> HookResult<ChannelMessage> {
        match self
            .binding_table
            .lookup(&message.channel, &message.sender)
            .await
        {
            Some(binding) => {
                let duration = chrono::Utc::now() - binding.bound_at;
                let total_secs = duration.num_seconds().max(0);
                let hours = total_secs / 3600;
                let minutes = (total_secs % 3600) / 60;
                let uptime = if hours > 0 {
                    format!("{}h {}m", hours, minutes)
                } else {
                    format!("{}m", minutes)
                };
                let status = if binding.stale { "stale" } else { "active" };
                self.send_reply(
                    message,
                    &format!(
                        "Session {}:\n  Agent: {}\n  Directory: {}\n  Uptime: {}\n  Status: {}",
                        status,
                        binding.agent_name,
                        binding.working_dir.display(),
                        uptime,
                        status,
                    ),
                )
                .await;
            }
            None => {
                self.send_reply(message, "No active session.").await;
            }
        }
        HookResult::Cancel("status".to_string())
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
        // Strip @bot suffix (Telegram sends "/connect@mybot" in groups)
        let command_token = content.split_whitespace().next().unwrap_or("");
        let base_command = command_token.split('@').next().unwrap_or(command_token);
        if PASSTHROUGH_COMMANDS.contains(&base_command) {
            return HookResult::Continue(message);
        }

        // 3. /connect — only for allowed users
        if base_command == "/connect" {
            if !self.allowed_users.contains(&message.sender) {
                return HookResult::Continue(message);
            }
            return self.handle_connect(&message).await;
        }

        // 4. /disconnect — only for allowed users
        if base_command == "/disconnect" {
            if !self.allowed_users.contains(&message.sender) {
                return HookResult::Continue(message);
            }
            return self.handle_disconnect(&message).await;
        }

        // 5. /status — only for allowed users
        if base_command == "/status" {
            if !self.allowed_users.contains(&message.sender) {
                return HookResult::Continue(message);
            }
            return self.handle_status(&message).await;
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
                return HookResult::Continue(message);
            }

            // 6b. Active binding → route to child process
            match self
                .process_manager
                .send_message(&message.channel, &message.sender, content)
                .await
            {
                Ok(pending) => {
                    if pending >= process_manager::QUEUE_WARN_THRESHOLD {
                        self.send_reply(
                            &message,
                            "Claude Code is still processing. Messages are queued.",
                        )
                        .await;
                    }
                    tracing::debug!(
                        hook = "session-bridge",
                        channel = %message.channel,
                        sender = %message.sender,
                        agent = %binding.agent_name,
                        pending = pending,
                        "routed message to bound session"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        hook = "session-bridge",
                        error = %e,
                        "failed to route message to child process"
                    );
                    self.send_reply(
                        &message,
                        "Failed to send message to Claude Code session. Try /disconnect and reconnect.",
                    )
                    .await;
                }
            }
            return HookResult::Cancel(format!(
                "session-bridge: routed to '{}'",
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
    use crate::config::SessionBridgeAgentConfig;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // ── Test helpers ─────────────────────────────────────

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

    fn test_message_from(content: &str, sender: &str) -> ChannelMessage {
        ChannelMessage {
            id: "1".to_string(),
            sender: sender.to_string(),
            reply_target: "chat123".to_string(),
            content: content.to_string(),
            channel: "telegram".to_string(),
            timestamp: 0,
            thread_ts: None,
        }
    }

    struct MockChannel {
        sent: Arc<Mutex<Vec<SendMessage>>>,
    }

    #[async_trait]
    impl Channel for MockChannel {
        fn name(&self) -> &str {
            "telegram"
        }
        async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push(message.clone());
            Ok(())
        }
        async fn listen(
            &self,
            _tx: tokio::sync::mpsc::Sender<ChannelMessage>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn mock_channel() -> (Arc<dyn Channel>, Arc<Mutex<Vec<SendMessage>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let channel = Arc::new(MockChannel { sent: sent.clone() });
        (channel as Arc<dyn Channel>, sent)
    }

    /// Create a config with agent commands that accept arbitrary arguments
    /// without failing. Uses `/bin/sh -c cat` to simulate a process that
    /// reads stdin and stays alive, ignoring the extra CLI flags added
    /// by the process manager (--print, --input-format, etc.).
    fn config_with_agents() -> SessionBridgeConfig {
        let mut agents = HashMap::new();
        agents.insert(
            "claude".to_string(),
            SessionBridgeAgentConfig {
                command: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), "cat".to_string()],
            },
        );
        agents.insert(
            "codex".to_string(),
            SessionBridgeAgentConfig {
                command: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), "cat".to_string()],
            },
        );
        SessionBridgeConfig {
            agents,
            ..Default::default()
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

    async fn make_hook_with_channel(
        config_dir: &Path,
        config: SessionBridgeConfig,
        channel: Arc<dyn Channel>,
    ) -> SessionBridgeHook {
        let mut channels = HashMap::new();
        channels.insert("telegram".to_string(), channel);
        SessionBridgeHook::new(
            Arc::new(channels),
            config,
            vec!["alice".to_string()],
            config_dir,
        )
        .await
        .unwrap()
    }

    fn last_reply(sent: &Arc<Mutex<Vec<SendMessage>>>) -> String {
        sent.lock().unwrap().last().unwrap().content.clone()
    }

    // ── Passthrough / routing tests ─────────────────────

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
    async fn connect_with_bot_suffix_cancels() {
        let tmp = TempDir::new().unwrap();
        let hook = make_hook(tmp.path()).await;
        // Telegram sends "/connect@botname" in groups
        let msg = test_message("/connect@mybot claude ~/projects");
        assert!(hook.on_message_received(msg).await.is_cancel());
    }

    #[tokio::test]
    async fn runtime_commands_with_bot_suffix_pass_through() {
        let tmp = TempDir::new().unwrap();
        let hook = make_hook(tmp.path()).await;
        let msg = test_message("/models@mybot");
        assert!(
            !hook.on_message_received(msg).await.is_cancel(),
            "/models@bot should pass through"
        );
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

    // ── /connect tests ──────────────────────────────────

    #[tokio::test]
    async fn connect_success() {
        let tmp = TempDir::new().unwrap();
        let work_dir = TempDir::new().unwrap();
        let (channel, sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        let msg = test_message(&format!("/connect claude {}", work_dir.path().display()));
        assert!(hook.on_message_received(msg).await.is_cancel());

        let reply = last_reply(&sent);
        assert!(reply.starts_with("Connected to claude in "));

        let binding = hook
            .binding_table
            .lookup("telegram", "alice")
            .await
            .unwrap();
        assert_eq!(binding.agent_name, "claude");
        assert_eq!(binding.working_dir, work_dir.path());
        assert!(!binding.stale);
    }

    #[tokio::test]
    async fn connect_no_args() {
        let tmp = TempDir::new().unwrap();
        let (channel, sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        let msg = test_message("/connect");
        assert!(hook.on_message_received(msg).await.is_cancel());
        assert_eq!(last_reply(&sent), "Usage: /connect <agent> [path]");
    }

    #[tokio::test]
    async fn connect_unknown_agent() {
        let tmp = TempDir::new().unwrap();
        let (channel, sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        let msg = test_message("/connect nonexistent /tmp");
        assert!(hook.on_message_received(msg).await.is_cancel());

        let reply = last_reply(&sent);
        assert!(reply.starts_with("Unknown agent: nonexistent. Available:"));
        assert!(reply.contains("claude"));
        assert!(reply.contains("codex"));
    }

    #[tokio::test]
    async fn connect_invalid_path() {
        let tmp = TempDir::new().unwrap();
        let (channel, sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        let msg = test_message("/connect claude /nonexistent_test_path_99999");
        assert!(hook.on_message_received(msg).await.is_cancel());
        assert_eq!(
            last_reply(&sent),
            "Directory not found: /nonexistent_test_path_99999"
        );
    }

    #[tokio::test]
    async fn connect_already_bound() {
        let tmp = TempDir::new().unwrap();
        let work_dir = TempDir::new().unwrap();
        let (channel, sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        // First connect succeeds
        let msg = test_message(&format!("/connect claude {}", work_dir.path().display()));
        hook.on_message_received(msg).await;

        // Second connect fails
        let msg = test_message(&format!("/connect codex {}", work_dir.path().display()));
        assert!(hook.on_message_received(msg).await.is_cancel());

        let reply = last_reply(&sent);
        assert!(reply.starts_with("Already connected to claude in "));
        assert!(reply.ends_with(". Send /disconnect first."));
    }

    #[tokio::test]
    async fn connect_max_sessions() {
        let tmp = TempDir::new().unwrap();
        let work_dir = TempDir::new().unwrap();
        let (channel, sent) = mock_channel();
        let mut config = config_with_agents();
        config.max_sessions = 1;
        let hook = make_hook_with_channel(tmp.path(), config, channel).await;

        // Fill the one allowed session with a different user
        hook.binding_table
            .bind(binding_table::SessionBinding {
                channel: "telegram".to_string(),
                sender: "bob".to_string(),
                agent_name: "claude".to_string(),
                working_dir: work_dir.path().to_path_buf(),
                bound_at: chrono::Utc::now(),
                stale: false,
            })
            .await
            .unwrap();

        // Alice tries to connect — should fail
        let msg = test_message(&format!("/connect claude {}", work_dir.path().display()));
        assert!(hook.on_message_received(msg).await.is_cancel());
        assert_eq!(last_reply(&sent), "Too many active sessions (1/1)");
    }

    #[tokio::test]
    async fn connect_unauthorized_passes_through() {
        let tmp = TempDir::new().unwrap();
        let (channel, _sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        let msg = test_message_from("/connect claude /tmp", "stranger");
        assert!(!hook.on_message_received(msg).await.is_cancel());
    }

    #[tokio::test]
    async fn connect_with_default_workspace() {
        let tmp = TempDir::new().unwrap();
        let work_dir = TempDir::new().unwrap();
        let (channel, sent) = mock_channel();
        let mut config = config_with_agents();
        config.default_workspace = Some(work_dir.path().to_path_buf());
        let hook = make_hook_with_channel(tmp.path(), config, channel).await;

        // Connect without path — uses default workspace
        let msg = test_message("/connect claude");
        assert!(hook.on_message_received(msg).await.is_cancel());

        let reply = last_reply(&sent);
        assert!(reply.starts_with("Connected to claude in "));

        let binding = hook
            .binding_table
            .lookup("telegram", "alice")
            .await
            .unwrap();
        assert_eq!(binding.working_dir, work_dir.path());
    }

    // ── /disconnect tests ───────────────────────────────

    #[tokio::test]
    async fn disconnect_success() {
        let tmp = TempDir::new().unwrap();
        let (channel, sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        hook.binding_table
            .bind(binding_table::SessionBinding {
                channel: "telegram".to_string(),
                sender: "alice".to_string(),
                agent_name: "claude".to_string(),
                working_dir: PathBuf::from("/tmp"),
                bound_at: chrono::Utc::now(),
                stale: false,
            })
            .await
            .unwrap();

        let msg = test_message("/disconnect");
        assert!(hook.on_message_received(msg).await.is_cancel());
        assert_eq!(last_reply(&sent), "Disconnected from claude.");

        // Binding should be gone
        assert!(hook
            .binding_table
            .lookup("telegram", "alice")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn disconnect_not_bound() {
        let tmp = TempDir::new().unwrap();
        let (channel, sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        let msg = test_message("/disconnect");
        assert!(hook.on_message_received(msg).await.is_cancel());
        assert_eq!(last_reply(&sent), "No active session to disconnect.");
    }

    #[tokio::test]
    async fn disconnect_stale_binding() {
        let tmp = TempDir::new().unwrap();

        // Create binding, persist, then reload (marks stale)
        {
            let hook = make_hook(tmp.path()).await;
            hook.binding_table
                .bind(binding_table::SessionBinding {
                    channel: "telegram".to_string(),
                    sender: "alice".to_string(),
                    agent_name: "claude".to_string(),
                    working_dir: PathBuf::from("/tmp"),
                    bound_at: chrono::Utc::now(),
                    stale: false,
                })
                .await
                .unwrap();
        }

        // Reload (stale), then disconnect
        let (channel, sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        let msg = test_message("/disconnect");
        assert!(hook.on_message_received(msg).await.is_cancel());
        assert_eq!(last_reply(&sent), "Disconnected from claude.");

        assert!(hook
            .binding_table
            .lookup("telegram", "alice")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn disconnect_unauthorized_passes_through() {
        let tmp = TempDir::new().unwrap();
        let (channel, _sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        let msg = test_message_from("/disconnect", "stranger");
        assert!(!hook.on_message_received(msg).await.is_cancel());
    }

    // ── /status tests ───────────────────────────────────

    #[tokio::test]
    async fn status_bound() {
        let tmp = TempDir::new().unwrap();
        let (channel, sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        hook.binding_table
            .bind(binding_table::SessionBinding {
                channel: "telegram".to_string(),
                sender: "alice".to_string(),
                agent_name: "claude".to_string(),
                working_dir: PathBuf::from("/home/alice/projects"),
                bound_at: chrono::Utc::now(),
                stale: false,
            })
            .await
            .unwrap();

        let msg = test_message("/status");
        assert!(hook.on_message_received(msg).await.is_cancel());

        let reply = last_reply(&sent);
        assert!(reply.contains("Agent: claude"));
        assert!(reply.contains("Directory: /home/alice/projects"));
        assert!(reply.contains("Status: active"));
        assert!(reply.contains("Uptime:"));
    }

    #[tokio::test]
    async fn status_not_bound() {
        let tmp = TempDir::new().unwrap();
        let (channel, sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        let msg = test_message("/status");
        assert!(hook.on_message_received(msg).await.is_cancel());
        assert_eq!(last_reply(&sent), "No active session.");
    }

    #[tokio::test]
    async fn status_unauthorized_passes_through() {
        let tmp = TempDir::new().unwrap();
        let (channel, _sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        let msg = test_message_from("/status", "stranger");
        assert!(!hook.on_message_received(msg).await.is_cancel());
    }

    #[tokio::test]
    async fn status_stale_binding() {
        let tmp = TempDir::new().unwrap();

        // Create binding, persist, reload (marks stale)
        {
            let hook = make_hook(tmp.path()).await;
            hook.binding_table
                .bind(binding_table::SessionBinding {
                    channel: "telegram".to_string(),
                    sender: "alice".to_string(),
                    agent_name: "claude".to_string(),
                    working_dir: PathBuf::from("/home/alice/projects"),
                    bound_at: chrono::Utc::now(),
                    stale: false,
                })
                .await
                .unwrap();
        }

        let (channel, sent) = mock_channel();
        let hook = make_hook_with_channel(tmp.path(), config_with_agents(), channel).await;

        let msg = test_message("/status");
        assert!(hook.on_message_received(msg).await.is_cancel());

        let reply = last_reply(&sent);
        assert!(reply.contains("Status: stale"));
    }
}
