use std::collections::HashMap;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::agent::loop_::scrub_credentials;
use crate::providers::traits::ConversationMessage;

use super::rotation::RotatingWriter;
use super::sharegpt::{ShareGptConversation, ShareGptTurn, TrajectoryMetadata};

/// Events sent from the agent loop to the background writer task.
pub enum TrajectoryEvent {
    /// A single conversation message to accumulate.
    Turn {
        conversation_id: String,
        message: ConversationMessage,
    },
    /// Signal that a conversation is finished and should be serialized.
    Finish {
        conversation_id: String,
        status: ConversationStatus,
        metadata: TrajectoryMetadata,
    },
}

/// Outcome of a recorded conversation.
#[derive(Debug, Clone)]
pub enum ConversationStatus {
    /// Conversation completed successfully with a final assistant response.
    Completed,
    /// Conversation failed with an error.
    Failed(String),
    /// Conversation was truncated (hit max_tool_iterations).
    Truncated,
}

impl ConversationStatus {
    fn as_str(&self) -> &str {
        match self {
            Self::Completed => "completed",
            Self::Failed(_) => "failed",
            Self::Truncated => "truncated",
        }
    }
}

/// Configuration for trajectory collection.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TrajectoryConfig {
    /// Enable trajectory collection (default: false).
    #[serde(default)]
    pub enabled: bool,
    /// Output directory for JSONL files.
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,
    /// Maximum JSONL file size in bytes before rotation (default: 50 MB).
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    /// Scrub sensitive patterns (API keys, tokens, passwords) before writing.
    #[serde(default = "default_scrub_secrets")]
    pub scrub_secrets: bool,
}

fn default_output_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|u| u.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".rustyclaw")
        .join("trajectories")
}

fn default_max_file_bytes() -> u64 {
    50 * 1024 * 1024
}

fn default_scrub_secrets() -> bool {
    true
}

impl Default for TrajectoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            output_dir: default_output_dir(),
            max_file_bytes: default_max_file_bytes(),
            scrub_secrets: default_scrub_secrets(),
        }
    }
}

/// Non-blocking trajectory collector that records conversation turns via an async channel
/// and writes completed conversations to JSONL files in a background task.
pub struct TrajectoryCollector {
    tx: mpsc::Sender<TrajectoryEvent>,
}

impl TrajectoryCollector {
    /// Spawn a new collector with a background writer task.
    pub fn new(config: TrajectoryConfig) -> Self {
        let (tx, rx) = mpsc::channel(1024);
        tokio::spawn(Self::writer_loop(rx, config));
        Self { tx }
    }

    /// Record a conversation turn. Never blocks the agent loop — drops the event
    /// if the channel is full.
    pub fn record_turn(&self, conversation_id: &str, message: &ConversationMessage) {
        let _ = self.tx.try_send(TrajectoryEvent::Turn {
            conversation_id: conversation_id.to_string(),
            message: message.clone(),
        });
    }

    /// Signal that a conversation has finished. The background task will serialize
    /// and write the accumulated turns.
    pub fn finish_conversation(
        &self,
        conversation_id: &str,
        status: ConversationStatus,
        metadata: TrajectoryMetadata,
    ) {
        let _ = self.tx.try_send(TrajectoryEvent::Finish {
            conversation_id: conversation_id.to_string(),
            status,
            metadata,
        });
    }

    async fn writer_loop(mut rx: mpsc::Receiver<TrajectoryEvent>, config: TrajectoryConfig) {
        let mut conversations: HashMap<String, Vec<ShareGptTurn>> = HashMap::new();
        let mut completed_writer =
            RotatingWriter::new(config.output_dir.join("completed"), config.max_file_bytes);
        let mut failed_writer =
            RotatingWriter::new(config.output_dir.join("failed"), config.max_file_bytes);

        while let Some(event) = rx.recv().await {
            match event {
                TrajectoryEvent::Turn {
                    conversation_id,
                    message,
                } => {
                    let mut turns: Vec<ShareGptTurn> = (&message).into();
                    if config.scrub_secrets {
                        for turn in &mut turns {
                            turn.value = scrub_credentials(&turn.value);
                        }
                    }
                    conversations
                        .entry(conversation_id)
                        .or_default()
                        .extend(turns);
                }
                TrajectoryEvent::Finish {
                    conversation_id,
                    status,
                    mut metadata,
                } => {
                    if let Some(turns) = conversations.remove(&conversation_id) {
                        metadata.status = status.as_str().to_string();
                        let conv = ShareGptConversation {
                            id: conversation_id,
                            conversations: turns,
                            metadata,
                        };
                        let line = match serde_json::to_string(&conv) {
                            Ok(l) => l,
                            Err(e) => {
                                tracing::error!("trajectory serialization failed: {e}");
                                continue;
                            }
                        };
                        let writer = match &status {
                            ConversationStatus::Completed => &mut completed_writer,
                            _ => &mut failed_writer,
                        };
                        if let Err(e) = writer.write_line(&line) {
                            tracing::warn!("trajectory write failed: {e}");
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::traits::{ChatMessage, ToolCall, ToolResultMessage};
    use tempfile::TempDir;

    /// Helper: create a collector with a temp dir and return (collector, config, tmpdir).
    fn setup(scrub: bool) -> (TrajectoryCollector, TrajectoryConfig, TempDir) {
        let tmp = TempDir::new().unwrap();
        let config = TrajectoryConfig {
            enabled: true,
            output_dir: tmp.path().to_path_buf(),
            max_file_bytes: 10 * 1024 * 1024,
            scrub_secrets: scrub,
        };
        let collector = TrajectoryCollector::new(config.clone());
        (collector, config, tmp)
    }

    #[tokio::test]
    async fn channel_send_receive_basic() {
        let (tx, mut rx) = mpsc::channel::<TrajectoryEvent>(16);

        let msg = ConversationMessage::Chat(ChatMessage::user("hello"));
        tx.try_send(TrajectoryEvent::Turn {
            conversation_id: "conv_1".into(),
            message: msg,
        })
        .unwrap();

        let event = rx.recv().await.unwrap();
        match event {
            TrajectoryEvent::Turn {
                conversation_id,
                message,
            } => {
                assert_eq!(conversation_id, "conv_1");
                match message {
                    ConversationMessage::Chat(chat) => {
                        assert_eq!(chat.content, "hello");
                    }
                    _ => panic!("expected Chat message"),
                }
            }
            _ => panic!("expected Turn event"),
        }
    }

    #[tokio::test]
    async fn accumulates_turns_and_writes_completed() {
        let (collector, _config, tmp) = setup(false);
        let conv_id = "conv_accumulate";

        collector.record_turn(
            conv_id,
            &ConversationMessage::Chat(ChatMessage::system("System prompt")),
        );
        collector.record_turn(
            conv_id,
            &ConversationMessage::Chat(ChatMessage::user("What is 2+2?")),
        );
        collector.record_turn(
            conv_id,
            &ConversationMessage::Chat(ChatMessage::assistant("4")),
        );

        collector.finish_conversation(
            conv_id,
            ConversationStatus::Completed,
            TrajectoryMetadata {
                model: "test-model".into(),
                provider: "test-provider".into(),
                timestamp: "2026-03-11T00:00:00Z".into(),
                duration_ms: 100,
                tool_calls_count: 0,
                turns: 3,
                status: String::new(), // filled by finish
                tokens: None,
            },
        );

        // Drop sender to close channel, let writer loop finish
        drop(collector);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let completed_dir = tmp.path().join("completed");
        assert!(completed_dir.exists(), "completed/ dir should exist");

        let files: Vec<_> = std::fs::read_dir(&completed_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
            .collect();
        assert_eq!(files.len(), 1);

        let content = std::fs::read_to_string(files[0].path()).unwrap();
        let conv: ShareGptConversation = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(conv.id, conv_id);
        assert_eq!(conv.conversations.len(), 3);
        assert_eq!(conv.metadata.status, "completed");
        assert_eq!(conv.conversations[0].from, "system");
        assert_eq!(conv.conversations[1].from, "human");
        assert_eq!(conv.conversations[2].from, "gpt");
    }

    #[tokio::test]
    async fn failed_conversation_goes_to_failed_dir() {
        let (collector, _config, tmp) = setup(false);
        let conv_id = "conv_fail";

        collector.record_turn(
            conv_id,
            &ConversationMessage::Chat(ChatMessage::user("trigger error")),
        );

        collector.finish_conversation(
            conv_id,
            ConversationStatus::Failed("provider timeout".into()),
            TrajectoryMetadata {
                model: "test".into(),
                provider: "test".into(),
                timestamp: "2026-03-11T00:00:00Z".into(),
                duration_ms: 5000,
                tool_calls_count: 0,
                turns: 1,
                status: String::new(),
                tokens: None,
            },
        );

        drop(collector);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let failed_dir = tmp.path().join("failed");
        assert!(failed_dir.exists(), "failed/ dir should exist");

        let files: Vec<_> = std::fs::read_dir(&failed_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
            .collect();
        assert_eq!(files.len(), 1);

        let content = std::fs::read_to_string(files[0].path()).unwrap();
        let conv: ShareGptConversation = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(conv.metadata.status, "failed");
    }

    #[tokio::test]
    async fn truncated_conversation_goes_to_failed_dir() {
        let (collector, _config, tmp) = setup(false);
        let conv_id = "conv_trunc";

        collector.record_turn(
            conv_id,
            &ConversationMessage::Chat(ChatMessage::user("loop forever")),
        );

        collector.finish_conversation(
            conv_id,
            ConversationStatus::Truncated,
            TrajectoryMetadata {
                model: "test".into(),
                provider: "test".into(),
                timestamp: "2026-03-11T00:00:00Z".into(),
                duration_ms: 30000,
                tool_calls_count: 50,
                turns: 1,
                status: String::new(),
                tokens: None,
            },
        );

        drop(collector);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let failed_dir = tmp.path().join("failed");
        let content = std::fs::read_to_string(
            std::fs::read_dir(&failed_dir)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        let conv: ShareGptConversation = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(conv.metadata.status, "truncated");
    }

    #[tokio::test]
    async fn scrub_secrets_redacts_api_keys() {
        let (collector, _config, tmp) = setup(true);
        let conv_id = "conv_scrub";

        collector.record_turn(
            conv_id,
            &ConversationMessage::Chat(ChatMessage::user(
                "My api_key=sk-proj-abcdefghijklmnop123456",
            )),
        );

        collector.finish_conversation(
            conv_id,
            ConversationStatus::Completed,
            TrajectoryMetadata {
                model: "test".into(),
                provider: "test".into(),
                timestamp: "2026-03-11T00:00:00Z".into(),
                duration_ms: 50,
                tool_calls_count: 0,
                turns: 1,
                status: String::new(),
                tokens: None,
            },
        );

        drop(collector);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let completed_dir = tmp.path().join("completed");
        let content = std::fs::read_to_string(
            std::fs::read_dir(&completed_dir)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        let conv: ShareGptConversation = serde_json::from_str(content.trim()).unwrap();

        // The original secret should NOT appear in the output
        assert!(
            !conv.conversations[0]
                .value
                .contains("sk-proj-abcdefghijklmnop123456"),
            "secret should be redacted"
        );
        assert!(
            conv.conversations[0].value.contains("REDACTED"),
            "redaction marker should be present"
        );
    }

    #[tokio::test]
    async fn scrub_secrets_disabled_preserves_content() {
        let (collector, _config, tmp) = setup(false);
        let conv_id = "conv_no_scrub";

        collector.record_turn(
            conv_id,
            &ConversationMessage::Chat(ChatMessage::user(
                "My api_key=sk-proj-abcdefghijklmnop123456",
            )),
        );

        collector.finish_conversation(
            conv_id,
            ConversationStatus::Completed,
            TrajectoryMetadata {
                model: "test".into(),
                provider: "test".into(),
                timestamp: "2026-03-11T00:00:00Z".into(),
                duration_ms: 50,
                tool_calls_count: 0,
                turns: 1,
                status: String::new(),
                tokens: None,
            },
        );

        drop(collector);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let completed_dir = tmp.path().join("completed");
        let content = std::fs::read_to_string(
            std::fs::read_dir(&completed_dir)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        let conv: ShareGptConversation = serde_json::from_str(content.trim()).unwrap();
        assert!(conv.conversations[0]
            .value
            .contains("sk-proj-abcdefghijklmnop123456"));
    }

    #[tokio::test]
    async fn tool_calls_expand_correctly() {
        let (collector, _config, tmp) = setup(false);
        let conv_id = "conv_tools";

        collector.record_turn(
            conv_id,
            &ConversationMessage::Chat(ChatMessage::user("List files")),
        );
        collector.record_turn(
            conv_id,
            &ConversationMessage::AssistantToolCalls {
                text: Some("Sure, let me check.".into()),
                tool_calls: vec![ToolCall {
                    id: "tc1".into(),
                    name: "shell".into(),
                    arguments: r#"{"command":"ls"}"#.into(),
                }],
                reasoning_content: None,
            },
        );
        collector.record_turn(
            conv_id,
            &ConversationMessage::ToolResults(vec![ToolResultMessage {
                tool_call_id: "tc1".into(),
                content: "file1.txt\nfile2.txt".into(),
            }]),
        );
        collector.record_turn(
            conv_id,
            &ConversationMessage::Chat(ChatMessage::assistant("Found 2 files.")),
        );

        collector.finish_conversation(
            conv_id,
            ConversationStatus::Completed,
            TrajectoryMetadata {
                model: "test".into(),
                provider: "test".into(),
                timestamp: "2026-03-11T00:00:00Z".into(),
                duration_ms: 200,
                tool_calls_count: 1,
                turns: 4,
                status: String::new(),
                tokens: None,
            },
        );

        drop(collector);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let completed_dir = tmp.path().join("completed");
        let content = std::fs::read_to_string(
            std::fs::read_dir(&completed_dir)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        let conv: ShareGptConversation = serde_json::from_str(content.trim()).unwrap();

        assert_eq!(conv.conversations.len(), 5); // human, gpt, tool_call, tool_response, gpt
        let roles: Vec<&str> = conv.conversations.iter().map(|t| t.from.as_str()).collect();
        assert_eq!(
            roles,
            vec!["human", "gpt", "tool_call", "tool_response", "gpt"]
        );
    }

    #[tokio::test]
    async fn finish_without_turns_is_noop() {
        let (collector, _config, tmp) = setup(false);

        // Finish a conversation that was never started
        collector.finish_conversation(
            "nonexistent",
            ConversationStatus::Completed,
            TrajectoryMetadata {
                model: "test".into(),
                provider: "test".into(),
                timestamp: "2026-03-11T00:00:00Z".into(),
                duration_ms: 0,
                tool_calls_count: 0,
                turns: 0,
                status: String::new(),
                tokens: None,
            },
        );

        drop(collector);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Neither directory should be created
        assert!(!tmp.path().join("completed").exists());
        assert!(!tmp.path().join("failed").exists());
    }
}
