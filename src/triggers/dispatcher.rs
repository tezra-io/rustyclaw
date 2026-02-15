use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{error, info};

use crate::agent::{AgentDefinition, TriggerEvent};
use crate::bus::events::{AgentMessage, AgentMessageType};
use crate::bus::queue::MessageBus;

use super::file::{FileChangeEvent, FileTrigger, FileWatchConfig};
use super::git::{GitPushEvent, GitTrigger, GitWatchConfig};
use super::message::{MessageMatchConfig, MessageTrigger};

/// Central dispatcher that collects trigger configurations from agents
/// and runs all trigger watchers.
pub struct TriggerDispatcher {
    bus: Arc<MessageBus>,
    git_configs: Vec<GitWatchConfig>,
    file_configs: Vec<FileWatchConfig>,
    message_trigger: MessageTrigger,
}

impl TriggerDispatcher {
    /// Build a dispatcher from agent definitions.
    pub fn from_agents(agents: &[AgentDefinition], bus: Arc<MessageBus>) -> Self {
        let mut git_configs = Vec::new();
        let mut file_configs = Vec::new();
        let mut message_configs = Vec::new();

        for agent in agents {
            if let Some(ref trigger) = agent.trigger {
                match trigger.on {
                    TriggerEvent::GitPush => {
                        // Infer repo path from context or use current dir
                        let repo_path = PathBuf::from(".");
                        let branches = trigger
                            .branches
                            .clone()
                            .unwrap_or_else(|| vec!["main".to_string()]);

                        git_configs.push(GitWatchConfig {
                            repo_path,
                            branches,
                            agent_name: agent.name.clone(),
                            task: trigger.task.clone(),
                        });
                    }
                    TriggerEvent::FileChange => {
                        let paths = trigger
                            .paths
                            .as_ref()
                            .map(|p| p.iter().map(PathBuf::from).collect())
                            .unwrap_or_else(|| vec![PathBuf::from(".")]);

                        file_configs.push(FileWatchConfig {
                            paths,
                            agent_name: agent.name.clone(),
                            task: trigger.task.clone(),
                        });
                    }
                    TriggerEvent::MessageMatch => {
                        if let Some(ref pattern) = trigger.pattern {
                            message_configs.push(MessageMatchConfig {
                                pattern: pattern.clone(),
                                agent_name: agent.name.clone(),
                                task: trigger.task.clone(),
                            });
                        }
                    }
                }
            }
        }

        let message_trigger = MessageTrigger::new(message_configs);

        Self {
            bus,
            git_configs,
            file_configs,
            message_trigger,
        }
    }

    /// Start all background trigger watchers. Returns a handle for the message trigger.
    pub async fn start(&self) -> MessageTriggerHandle {
        let bus = self.bus.clone();

        // Git trigger
        if !self.git_configs.is_empty() {
            let git_trigger = GitTrigger::new(self.git_configs.clone(), Duration::from_secs(30));
            let (tx, mut rx) = mpsc::channel::<(String, String, GitPushEvent)>(16);
            let bus_clone = bus.clone();

            tokio::spawn(async move {
                git_trigger.watch(tx).await;
            });

            tokio::spawn(async move {
                while let Some((agent_name, task, event)) = rx.recv().await {
                    let content = format!(
                        "{}\n\nContext: branch '{}' updated ({} → {})",
                        task,
                        event.branch,
                        &event.old_hash[..8.min(event.old_hash.len())],
                        &event.new_hash[..8.min(event.new_hash.len())],
                    );

                    let msg = AgentMessage::new(
                        "trigger:git",
                        &agent_name,
                        AgentMessageType::Task,
                        &content,
                    );

                    if let Err(e) = bus_clone.send_to_agent(&agent_name, msg).await {
                        error!(agent = %agent_name, "Failed to dispatch git trigger: {}", e);
                    }
                }
            });

            info!("Git triggers started ({} repos)", self.git_configs.len());
        }

        // File trigger
        if !self.file_configs.is_empty() {
            let file_trigger = FileTrigger::new(self.file_configs.clone());
            let (tx, mut rx) = mpsc::channel::<FileChangeEvent>(16);
            let bus_clone = bus.clone();

            tokio::spawn(async move {
                file_trigger.watch(tx).await;
            });

            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    let content = format!(
                        "{}\n\nContext: file change detected at {}",
                        event.task,
                        event.path.display()
                    );

                    let msg = AgentMessage::new(
                        "trigger:file",
                        &event.agent_name,
                        AgentMessageType::Task,
                        &content,
                    );

                    if let Err(e) = bus_clone.send_to_agent(&event.agent_name, msg).await {
                        error!(
                            agent = %event.agent_name,
                            "Failed to dispatch file trigger: {}", e
                        );
                    }
                }
            });

            info!(
                "File triggers started ({} configs)",
                self.file_configs.len()
            );
        }

        // Return message trigger handle for synchronous checking in message pipeline
        MessageTriggerHandle {
            trigger: self.message_trigger.clone(),
            bus: self.bus.clone(),
        }
    }

    /// Check if any triggers are configured.
    pub fn has_triggers(&self) -> bool {
        !self.git_configs.is_empty()
            || !self.file_configs.is_empty()
            || !self.message_trigger.is_empty()
    }
}

/// Handle for checking inbound messages against triggers.
/// Returned from `TriggerDispatcher::start()` for use in the message pipeline.
#[derive(Clone)]
pub struct MessageTriggerHandle {
    trigger: MessageTrigger,
    bus: Arc<MessageBus>,
}

impl MessageTriggerHandle {
    /// Check an inbound message and dispatch to matching agents.
    pub async fn check_and_dispatch(&self, text: &str, sender_id: &str, chat_id: &str) {
        let events = self.trigger.check(text, sender_id, chat_id);

        for event in events {
            let content = format!(
                "{}\n\nTriggered by message from {}: \"{}\"",
                event.task, event.sender_id, event.matched_text
            );

            let msg = AgentMessage::new(
                "trigger:message",
                &event.agent_name,
                AgentMessageType::Task,
                &content,
            );

            if let Err(e) = self.bus.send_to_agent(&event.agent_name, msg).await {
                error!(
                    agent = %event.agent_name,
                    "Failed to dispatch message trigger: {}", e
                );
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.trigger.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentDefinition, MemoryMode, TriggerConfig, TriggerEvent};

    fn make_agent_with_trigger(name: &str, trigger: TriggerConfig) -> AgentDefinition {
        AgentDefinition {
            name: name.to_string(),
            description: "test".to_string(),
            system_prompt: String::new(),
            model: None,
            tools: None,
            context_files: Vec::new(),
            memory_mode: MemoryMode::Isolated,
            schedule: Vec::new(),
            trigger: Some(trigger),
        }
    }

    #[test]
    fn dispatcher_from_agents_git() {
        let agents = vec![make_agent_with_trigger(
            "reviewer",
            TriggerConfig {
                on: TriggerEvent::GitPush,
                branches: Some(vec!["main".to_string()]),
                pattern: None,
                paths: None,
                task: "review".to_string(),
            },
        )];

        let bus = Arc::new(MessageBus::new(16));
        let dispatcher = TriggerDispatcher::from_agents(&agents, bus);
        assert!(dispatcher.has_triggers());
        assert_eq!(dispatcher.git_configs.len(), 1);
    }

    #[test]
    fn dispatcher_from_agents_file() {
        let agents = vec![make_agent_with_trigger(
            "watcher",
            TriggerConfig {
                on: TriggerEvent::FileChange,
                branches: None,
                pattern: None,
                paths: Some(vec!["src/".to_string()]),
                task: "check".to_string(),
            },
        )];

        let bus = Arc::new(MessageBus::new(16));
        let dispatcher = TriggerDispatcher::from_agents(&agents, bus);
        assert_eq!(dispatcher.file_configs.len(), 1);
    }

    #[test]
    fn dispatcher_from_agents_message() {
        let agents = vec![make_agent_with_trigger(
            "helper",
            TriggerConfig {
                on: TriggerEvent::MessageMatch,
                branches: None,
                pattern: Some("(?i)help".to_string()),
                paths: None,
                task: "assist".to_string(),
            },
        )];

        let bus = Arc::new(MessageBus::new(16));
        let dispatcher = TriggerDispatcher::from_agents(&agents, bus);
        assert!(!dispatcher.message_trigger.is_empty());
    }

    #[test]
    fn dispatcher_no_triggers() {
        let agents = vec![AgentDefinition {
            name: "basic".to_string(),
            description: "test".to_string(),
            system_prompt: String::new(),
            model: None,
            tools: None,
            context_files: Vec::new(),
            memory_mode: MemoryMode::Isolated,
            schedule: Vec::new(),
            trigger: None,
        }];

        let bus = Arc::new(MessageBus::new(16));
        let dispatcher = TriggerDispatcher::from_agents(&agents, bus);
        assert!(!dispatcher.has_triggers());
    }
}
