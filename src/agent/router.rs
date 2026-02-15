use std::collections::HashMap;
use tracing::debug;

use super::AgentDefinition;
use crate::bus::events::MAX_DELEGATION_DEPTH;

/// Result of routing a message to an agent.
#[derive(Debug)]
pub struct RouteResult {
    pub agent_name: String,
    pub confidence: f64,
    pub method: RouteMethod,
}

/// How the route was determined.
#[derive(Debug, PartialEq)]
pub enum RouteMethod {
    /// User explicitly named the agent ("ask X to...", "tell X to...").
    Explicit,
    /// Matched by keyword scoring against agent descriptions.
    KeywordMatch,
}

/// Confidence threshold: only delegate if score exceeds this.
const CONFIDENCE_THRESHOLD: f64 = 0.5;

/// Routes incoming messages to the appropriate agent.
pub struct AgentRouter {
    /// Agent name → lowercase keywords extracted from description.
    agent_keywords: HashMap<String, Vec<String>>,
}

impl AgentRouter {
    pub fn new() -> Self {
        Self {
            agent_keywords: HashMap::new(),
        }
    }

    /// Register an agent's description for keyword matching.
    pub fn register_agent(&mut self, agent: &AgentDefinition) {
        let keywords = extract_keywords(&agent.description);
        debug!(
            agent = %agent.name,
            keywords = ?keywords,
            "Registered agent for routing"
        );
        self.agent_keywords.insert(agent.name.clone(), keywords);
    }

    /// Route a message to the best matching agent, if any.
    pub fn route(&self, message: &str, agents: &[AgentDefinition]) -> Option<RouteResult> {
        self.route_with_depth(message, agents, 0)
    }

    /// Route with delegation depth tracking.
    pub fn route_with_depth(
        &self,
        message: &str,
        agents: &[AgentDefinition],
        depth: u8,
    ) -> Option<RouteResult> {
        // Guard against infinite recursion
        if depth >= MAX_DELEGATION_DEPTH {
            debug!(
                depth,
                "Delegation depth limit reached, master handles directly"
            );
            return None;
        }

        // 1. Try explicit routing: "ask X to...", "tell X to..."
        if let Some(result) = self.try_explicit_route(message, agents) {
            return Some(result);
        }

        // 2. Try keyword matching
        if let Some(result) = self.try_keyword_route(message) {
            return Some(result);
        }

        // 3. No match → master handles
        None
    }

    /// Check for explicit agent naming patterns.
    fn try_explicit_route(&self, message: &str, agents: &[AgentDefinition]) -> Option<RouteResult> {
        let lower = message.to_lowercase();

        // Patterns: "ask X to...", "tell X to...", "@X ..."
        let prefixes = ["ask ", "tell "];

        for prefix in prefixes {
            if let Some(rest) = lower.strip_prefix(prefix) {
                // Find the agent name at the start of the rest
                for agent in agents {
                    let name_lower = agent.name.to_lowercase();
                    if rest.starts_with(&name_lower) {
                        let after = &rest[name_lower.len()..];
                        // Must be followed by whitespace or "to"
                        if after.starts_with(' ') || after.starts_with(" to ") {
                            return Some(RouteResult {
                                agent_name: agent.name.clone(),
                                confidence: 1.0,
                                method: RouteMethod::Explicit,
                            });
                        }
                    }
                }
            }
        }

        // Pattern: "@agent-name ..."
        if let Some(rest) = lower.strip_prefix('@') {
            for agent in agents {
                let name_lower = agent.name.to_lowercase();
                if rest.starts_with(&name_lower)
                    && rest[name_lower.len()..].starts_with(|c: char| c.is_whitespace())
                {
                    return Some(RouteResult {
                        agent_name: agent.name.clone(),
                        confidence: 1.0,
                        method: RouteMethod::Explicit,
                    });
                }
            }
        }

        None
    }

    /// Score each agent's keywords against the message content.
    fn try_keyword_route(&self, message: &str) -> Option<RouteResult> {
        let message_words = extract_keywords(message);

        if message_words.is_empty() {
            return None;
        }

        let mut best_name: Option<String> = None;
        let mut best_score: f64 = 0.0;

        for (name, agent_keywords) in &self.agent_keywords {
            if agent_keywords.is_empty() {
                continue;
            }

            // Count how many agent keywords appear in the message
            let matches: usize = agent_keywords
                .iter()
                .filter(|kw| message_words.contains(kw))
                .count();

            // Score = fraction of agent keywords that matched
            let score = matches as f64 / agent_keywords.len() as f64;

            if score > best_score {
                best_score = score;
                best_name = Some(name.clone());
            }
        }

        if best_score >= CONFIDENCE_THRESHOLD {
            best_name.map(|name| {
                debug!(
                    agent = %name,
                    score = best_score,
                    "Keyword match"
                );
                RouteResult {
                    agent_name: name,
                    confidence: best_score,
                    method: RouteMethod::KeywordMatch,
                }
            })
        } else {
            None
        }
    }
}

impl Default for AgentRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract meaningful keywords from text (lowercase, deduplicated, stop words removed).
fn extract_keywords(text: &str) -> Vec<String> {
    let stop_words = [
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can",
        "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "about", "like",
        "through", "after", "over", "between", "out", "against", "during", "without", "before",
        "under", "around", "among", "and", "but", "or", "nor", "not", "so", "yet", "both",
        "either", "neither", "each", "every", "all", "any", "few", "more", "most", "other", "some",
        "such", "no", "only", "own", "same", "than", "too", "very", "just", "because", "if",
        "when", "while", "where", "how", "what", "which", "who", "whom", "this", "that", "these",
        "those", "it", "its", "my", "your", "his", "her", "our", "their", "i", "me", "you", "he",
        "she", "we", "they", "them", "us",
    ];

    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .filter(|w| w.len() > 2 && !stop_words.contains(w))
        .map(|w| w.to_string())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::MemoryMode;

    fn make_agent(name: &str, description: &str) -> AgentDefinition {
        AgentDefinition {
            name: name.to_string(),
            description: description.to_string(),
            system_prompt: String::new(),
            model: None,
            tools: None,
            context_files: Vec::new(),
            memory_mode: MemoryMode::Isolated,
            schedule: Vec::new(),
            trigger: None,
        }
    }

    #[test]
    fn explicit_route_ask() {
        let agents = vec![
            make_agent("twitter", "Manages Twitter presence"),
            make_agent("code-reviewer", "Reviews code changes"),
        ];

        let mut router = AgentRouter::new();
        for a in &agents {
            router.register_agent(a);
        }

        let result = router.route("ask twitter to post a tweet", &agents);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.agent_name, "twitter");
        assert_eq!(r.confidence, 1.0);
        assert_eq!(r.method, RouteMethod::Explicit);
    }

    #[test]
    fn explicit_route_tell() {
        let agents = vec![make_agent("code-reviewer", "Reviews code for security")];
        let mut router = AgentRouter::new();
        for a in &agents {
            router.register_agent(a);
        }

        let result = router.route("tell code-reviewer to check the PR", &agents);
        assert!(result.is_some());
        assert_eq!(result.unwrap().agent_name, "code-reviewer");
    }

    #[test]
    fn explicit_route_at_mention() {
        let agents = vec![make_agent("twitter", "Twitter management")];
        let mut router = AgentRouter::new();
        for a in &agents {
            router.register_agent(a);
        }

        let result = router.route("@twitter post something cool", &agents);
        assert!(result.is_some());
        assert_eq!(result.unwrap().agent_name, "twitter");
    }

    #[test]
    fn keyword_match() {
        let agents = vec![
            make_agent(
                "twitter",
                "Manages Twitter presence, posts tweets, engagement",
            ),
            make_agent(
                "code-reviewer",
                "Reviews code changes for security and performance",
            ),
        ];

        let mut router = AgentRouter::new();
        for a in &agents {
            router.register_agent(a);
        }

        let result = router.route("reviews code changes for security issues", &agents);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.agent_name, "code-reviewer");
        assert_eq!(r.method, RouteMethod::KeywordMatch);
    }

    #[test]
    fn no_match_returns_none() {
        let agents = vec![make_agent("twitter", "Twitter management")];
        let mut router = AgentRouter::new();
        for a in &agents {
            router.register_agent(a);
        }

        let result = router.route("what is the weather today?", &agents);
        assert!(result.is_none());
    }

    #[test]
    fn depth_guard() {
        let agents = vec![make_agent("twitter", "Twitter management")];
        let mut router = AgentRouter::new();
        for a in &agents {
            router.register_agent(a);
        }

        // At max depth, should return None even for explicit routes
        let result = router.route_with_depth("ask twitter to post", &agents, MAX_DELEGATION_DEPTH);
        assert!(result.is_none());
    }

    #[test]
    fn extract_keywords_basic() {
        let kws = extract_keywords("Reviews code changes for security and performance");
        assert!(kws.contains(&"reviews".to_string()));
        assert!(kws.contains(&"code".to_string()));
        assert!(kws.contains(&"security".to_string()));
        assert!(kws.contains(&"performance".to_string()));
        // Stop words filtered out
        assert!(!kws.contains(&"for".to_string()));
        assert!(!kws.contains(&"and".to_string()));
    }

    #[test]
    fn fallback_when_below_threshold() {
        let agents = vec![make_agent(
            "monitor",
            "Watches project health git status test results",
        )];
        let mut router = AgentRouter::new();
        for a in &agents {
            router.register_agent(a);
        }

        // Very unrelated message should not match
        let result = router.route("order pizza for the team", &agents);
        assert!(result.is_none());
    }
}
