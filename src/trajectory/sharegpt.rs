use crate::providers::traits::ConversationMessage;
use serde::{Deserialize, Serialize};

/// A single conversation in ShareGPT JSONL format, suitable for SFT/RLHF training.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShareGptConversation {
    pub id: String,
    pub conversations: Vec<ShareGptTurn>,
    pub metadata: TrajectoryMetadata,
}

/// A single turn in a ShareGPT conversation.
///
/// The `from` field uses the ShareGPT role vocabulary:
/// `"system"`, `"human"`, `"gpt"`, `"tool_call"`, `"tool_response"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShareGptTurn {
    pub from: String,
    pub value: String,
}

/// Metadata about a recorded conversation trajectory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrajectoryMetadata {
    pub model: String,
    pub provider: String,
    pub timestamp: String,
    pub duration_ms: u64,
    pub tool_calls_count: usize,
    pub turns: usize,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenCounts>,
}

/// Input/output token counts for a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
}

impl From<&ConversationMessage> for Vec<ShareGptTurn> {
    fn from(msg: &ConversationMessage) -> Vec<ShareGptTurn> {
        match msg {
            ConversationMessage::Chat(chat) => vec![ShareGptTurn {
                from: match chat.role.as_str() {
                    "system" => "system",
                    "user" => "human",
                    "assistant" => "gpt",
                    other => other,
                }
                .into(),
                value: chat.content.clone(),
            }],
            ConversationMessage::AssistantToolCalls {
                text, tool_calls, ..
            } => {
                let mut turns = Vec::new();
                if let Some(t) = text {
                    if !t.is_empty() {
                        turns.push(ShareGptTurn {
                            from: "gpt".into(),
                            value: t.clone(),
                        });
                    }
                }
                for tc in tool_calls {
                    turns.push(ShareGptTurn {
                        from: "tool_call".into(),
                        value: serde_json::json!({
                            "name": tc.name,
                            "arguments": tc.arguments,
                            "id": tc.id,
                        })
                        .to_string(),
                    });
                }
                turns
            }
            ConversationMessage::ToolResults(results) => results
                .iter()
                .map(|r| ShareGptTurn {
                    from: "tool_response".into(),
                    value: r.content.clone(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::traits::{ChatMessage, ToolCall, ToolResultMessage};

    #[test]
    fn system_message_maps_to_system() {
        let msg = ConversationMessage::Chat(ChatMessage::system("You are RustyClaw"));
        let turns: Vec<ShareGptTurn> = (&msg).into();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].from, "system");
        assert_eq!(turns[0].value, "You are RustyClaw");
    }

    #[test]
    fn user_message_maps_to_human() {
        let msg = ConversationMessage::Chat(ChatMessage::user("Hello"));
        let turns: Vec<ShareGptTurn> = (&msg).into();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].from, "human");
        assert_eq!(turns[0].value, "Hello");
    }

    #[test]
    fn assistant_message_maps_to_gpt() {
        let msg = ConversationMessage::Chat(ChatMessage::assistant("Hi there"));
        let turns: Vec<ShareGptTurn> = (&msg).into();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].from, "gpt");
        assert_eq!(turns[0].value, "Hi there");
    }

    #[test]
    fn unknown_role_passes_through() {
        let msg = ConversationMessage::Chat(ChatMessage {
            role: "custom_role".into(),
            content: "test".into(),
        });
        let turns: Vec<ShareGptTurn> = (&msg).into();
        assert_eq!(turns[0].from, "custom_role");
    }

    #[test]
    fn tool_calls_with_text_produces_gpt_plus_tool_call_turns() {
        let msg = ConversationMessage::AssistantToolCalls {
            text: Some("I'll list files".into()),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "shell".into(),
                arguments: r#"{"command":"ls /tmp"}"#.into(),
            }],
            reasoning_content: None,
        };
        let turns: Vec<ShareGptTurn> = (&msg).into();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].from, "gpt");
        assert_eq!(turns[0].value, "I'll list files");
        assert_eq!(turns[1].from, "tool_call");

        let parsed: serde_json::Value = serde_json::from_str(&turns[1].value).unwrap();
        assert_eq!(parsed["name"], "shell");
        assert_eq!(parsed["id"], "call_1");
    }

    #[test]
    fn tool_calls_without_text_skips_gpt_turn() {
        let msg = ConversationMessage::AssistantToolCalls {
            text: None,
            tool_calls: vec![ToolCall {
                id: "call_2".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"/etc/hosts"}"#.into(),
            }],
            reasoning_content: None,
        };
        let turns: Vec<ShareGptTurn> = (&msg).into();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].from, "tool_call");
    }

    #[test]
    fn tool_calls_with_empty_text_skips_gpt_turn() {
        let msg = ConversationMessage::AssistantToolCalls {
            text: Some(String::new()),
            tool_calls: vec![ToolCall {
                id: "call_3".into(),
                name: "web".into(),
                arguments: "{}".into(),
            }],
            reasoning_content: None,
        };
        let turns: Vec<ShareGptTurn> = (&msg).into();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].from, "tool_call");
    }

    #[test]
    fn multiple_tool_calls_produce_multiple_turns() {
        let msg = ConversationMessage::AssistantToolCalls {
            text: None,
            tool_calls: vec![
                ToolCall {
                    id: "c1".into(),
                    name: "shell".into(),
                    arguments: r#"{"command":"ls"}"#.into(),
                },
                ToolCall {
                    id: "c2".into(),
                    name: "read_file".into(),
                    arguments: r#"{"path":"foo.txt"}"#.into(),
                },
            ],
            reasoning_content: None,
        };
        let turns: Vec<ShareGptTurn> = (&msg).into();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].from, "tool_call");
        assert_eq!(turns[1].from, "tool_call");
    }

    #[test]
    fn tool_results_produce_tool_response_turns() {
        let msg = ConversationMessage::ToolResults(vec![
            ToolResultMessage {
                tool_call_id: "c1".into(),
                content: "file1.txt\nfile2.txt".into(),
            },
            ToolResultMessage {
                tool_call_id: "c2".into(),
                content: "hello world".into(),
            },
        ]);
        let turns: Vec<ShareGptTurn> = (&msg).into();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].from, "tool_response");
        assert_eq!(turns[0].value, "file1.txt\nfile2.txt");
        assert_eq!(turns[1].from, "tool_response");
        assert_eq!(turns[1].value, "hello world");
    }

    #[test]
    fn conversation_round_trip_serialization() {
        let conv = ShareGptConversation {
            id: "conv_test_001".into(),
            conversations: vec![
                ShareGptTurn {
                    from: "system".into(),
                    value: "You are RustyClaw".into(),
                },
                ShareGptTurn {
                    from: "human".into(),
                    value: "List files in /tmp".into(),
                },
                ShareGptTurn {
                    from: "gpt".into(),
                    value: "I'll use the shell tool".into(),
                },
                ShareGptTurn {
                    from: "tool_call".into(),
                    value:
                        r#"{"name":"shell","arguments":"{\"command\":\"ls /tmp\"}","id":"call_1"}"#
                            .into(),
                },
                ShareGptTurn {
                    from: "tool_response".into(),
                    value: "file1.txt\nfile2.txt".into(),
                },
                ShareGptTurn {
                    from: "gpt".into(),
                    value: "The /tmp directory contains file1.txt and file2.txt.".into(),
                },
            ],
            metadata: TrajectoryMetadata {
                model: "anthropic/claude-sonnet-4-5".into(),
                provider: "anthropic".into(),
                timestamp: "2026-03-10T18:00:00Z".into(),
                duration_ms: 4200,
                tool_calls_count: 1,
                turns: 3,
                status: "completed".into(),
                tokens: Some(TokenCounts {
                    input: 1200,
                    output: 340,
                }),
            },
        };

        let json = serde_json::to_string(&conv).unwrap();
        let deserialized: ShareGptConversation = serde_json::from_str(&json).unwrap();
        assert_eq!(conv, deserialized);
    }

    #[test]
    fn metadata_without_tokens_round_trips() {
        let meta = TrajectoryMetadata {
            model: "openai/gpt-4o".into(),
            provider: "openai".into(),
            timestamp: "2026-03-11T00:00:00Z".into(),
            duration_ms: 100,
            tool_calls_count: 0,
            turns: 1,
            status: "completed".into(),
            tokens: None,
        };

        let json = serde_json::to_string(&meta).unwrap();
        assert!(!json.contains("tokens"));
        let deserialized: TrajectoryMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, deserialized);
    }

    #[test]
    fn full_conversation_message_conversion() {
        let messages: Vec<ConversationMessage> = vec![
            ConversationMessage::Chat(ChatMessage::system("System prompt")),
            ConversationMessage::Chat(ChatMessage::user("Do something")),
            ConversationMessage::AssistantToolCalls {
                text: Some("Sure, let me run that.".into()),
                tool_calls: vec![ToolCall {
                    id: "tc1".into(),
                    name: "shell".into(),
                    arguments: r#"{"command":"echo hi"}"#.into(),
                }],
                reasoning_content: None,
            },
            ConversationMessage::ToolResults(vec![ToolResultMessage {
                tool_call_id: "tc1".into(),
                content: "hi".into(),
            }]),
            ConversationMessage::Chat(ChatMessage::assistant("Done!")),
        ];

        let turns: Vec<ShareGptTurn> = messages
            .iter()
            .flat_map(Vec::<ShareGptTurn>::from)
            .collect();

        assert_eq!(turns.len(), 6);
        assert_eq!(
            turns.iter().map(|t| t.from.as_str()).collect::<Vec<_>>(),
            vec![
                "system",
                "human",
                "gpt",
                "tool_call",
                "tool_response",
                "gpt"
            ]
        );
    }

    #[test]
    fn jsonl_line_format() {
        let conv = ShareGptConversation {
            id: "conv_jsonl".into(),
            conversations: vec![ShareGptTurn {
                from: "human".into(),
                value: "hello".into(),
            }],
            metadata: TrajectoryMetadata {
                model: "test".into(),
                provider: "test".into(),
                timestamp: "2026-01-01T00:00:00Z".into(),
                duration_ms: 0,
                tool_calls_count: 0,
                turns: 1,
                status: "completed".into(),
                tokens: None,
            },
        };

        let line = serde_json::to_string(&conv).unwrap();
        assert!(!line.contains('\n'));
        // Verify it's valid JSON (single line for JSONL)
        let _: serde_json::Value = serde_json::from_str(&line).unwrap();
    }
}
