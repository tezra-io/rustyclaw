use regex::Regex;
use tracing::{debug, warn};

/// Configuration for a message-match trigger.
#[derive(Debug, Clone)]
pub struct MessageMatchConfig {
    pub pattern: String,
    pub agent_name: String,
    pub task: String,
}

/// Event emitted when an inbound message matches a trigger pattern.
#[derive(Debug, Clone)]
pub struct MessageMatchEvent {
    pub agent_name: String,
    pub task: String,
    pub matched_text: String,
    pub sender_id: String,
    pub chat_id: String,
}

/// Checks inbound messages against configured patterns.
/// Unlike git/file triggers, this is called synchronously from the message pipeline.
pub struct MessageTrigger {
    raw_configs: Vec<MessageMatchConfig>,
    compiled: Vec<CompiledMatch>,
}

struct CompiledMatch {
    regex: Regex,
    agent_name: String,
    task: String,
}

impl Clone for MessageTrigger {
    fn clone(&self) -> Self {
        Self::new(self.raw_configs.clone())
    }
}

impl MessageTrigger {
    pub fn new(configs: Vec<MessageMatchConfig>) -> Self {
        let compiled = configs
            .iter()
            .filter_map(|c| match Regex::new(&c.pattern) {
                Ok(regex) => Some(CompiledMatch {
                    regex,
                    agent_name: c.agent_name.clone(),
                    task: c.task.clone(),
                }),
                Err(e) => {
                    warn!(pattern = %c.pattern, "Invalid trigger regex: {}", e);
                    None
                }
            })
            .collect();

        Self {
            raw_configs: configs,
            compiled,
        }
    }

    /// Check a message against all patterns, returning matching events.
    pub fn check(&self, text: &str, sender_id: &str, chat_id: &str) -> Vec<MessageMatchEvent> {
        let mut events = Vec::new();

        for config in &self.compiled {
            if config.regex.is_match(text) {
                debug!(
                    agent = %config.agent_name,
                    pattern = %config.regex.as_str(),
                    "Message matched trigger"
                );

                events.push(MessageMatchEvent {
                    agent_name: config.agent_name.clone(),
                    task: config.task.clone(),
                    matched_text: text.to_string(),
                    sender_id: sender_id.to_string(),
                    chat_id: chat_id.to_string(),
                });
            }
        }

        events
    }

    /// Whether any message triggers are configured.
    pub fn is_empty(&self) -> bool {
        self.compiled.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_match() {
        let trigger = MessageTrigger::new(vec![MessageMatchConfig {
            pattern: "(?i)help".to_string(),
            agent_name: "helper".to_string(),
            task: "assist user".to_string(),
        }]);

        let events = trigger.check("I need HELP please", "user1", "chat1");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].agent_name, "helper");
        assert_eq!(events[0].matched_text, "I need HELP please");
    }

    #[test]
    fn no_match() {
        let trigger = MessageTrigger::new(vec![MessageMatchConfig {
            pattern: "(?i)deploy".to_string(),
            agent_name: "deployer".to_string(),
            task: "deploy".to_string(),
        }]);

        let events = trigger.check("hello world", "user1", "chat1");
        assert!(events.is_empty());
    }

    #[test]
    fn multiple_matches() {
        let trigger = MessageTrigger::new(vec![
            MessageMatchConfig {
                pattern: "hello".to_string(),
                agent_name: "greeter".to_string(),
                task: "greet".to_string(),
            },
            MessageMatchConfig {
                pattern: "world".to_string(),
                agent_name: "world-agent".to_string(),
                task: "worldly things".to_string(),
            },
        ]);

        let events = trigger.check("hello world", "user1", "chat1");
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn invalid_regex_skipped() {
        let trigger = MessageTrigger::new(vec![
            MessageMatchConfig {
                pattern: "[invalid".to_string(),
                agent_name: "bad".to_string(),
                task: "nope".to_string(),
            },
            MessageMatchConfig {
                pattern: "good".to_string(),
                agent_name: "good-agent".to_string(),
                task: "yes".to_string(),
            },
        ]);

        // Invalid regex is skipped, good one still works
        let events = trigger.check("good stuff", "user1", "chat1");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].agent_name, "good-agent");
    }

    #[test]
    fn empty_trigger() {
        let trigger = MessageTrigger::new(vec![]);
        assert!(trigger.is_empty());
        assert!(trigger.check("anything", "u", "c").is_empty());
    }
}
