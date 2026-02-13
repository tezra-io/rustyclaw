use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Token usage for a single LLM call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub timestamp: String,
    pub model: String,
}

/// Daily token counter for an agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DailyTokenCount {
    pub date: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub call_count: u64,
}

/// Tracks per-agent token usage with daily aggregation.
pub struct TokenTracker {
    /// Agent name → daily counts keyed by date string.
    counters: Arc<Mutex<HashMap<String, HashMap<String, DailyTokenCount>>>>,
}

impl TokenTracker {
    pub fn new() -> Self {
        Self {
            counters: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Record token usage from an LLM response.
    pub async fn record(&self, agent_name: &str, usage: &TokenUsage) {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let mut counters = self.counters.lock().await;

        let agent_counters = counters.entry(agent_name.to_string()).or_default();
        let daily = agent_counters
            .entry(today.clone())
            .or_insert_with(|| DailyTokenCount {
                date: today,
                ..Default::default()
            });

        daily.input_tokens += usage.input_tokens as u64;
        daily.output_tokens += usage.output_tokens as u64;
        daily.call_count += 1;
    }

    /// Record from a provider Usage struct directly.
    pub async fn record_usage(&self, agent_name: &str, input: u32, output: u32, model: &str) {
        let usage = TokenUsage {
            input_tokens: input,
            output_tokens: output,
            timestamp: Utc::now().to_rfc3339(),
            model: model.to_string(),
        };
        self.record(agent_name, &usage).await;
    }

    /// Get today's token count for an agent.
    pub async fn today(&self, agent_name: &str) -> DailyTokenCount {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let counters = self.counters.lock().await;

        counters
            .get(agent_name)
            .and_then(|agent| agent.get(&today))
            .cloned()
            .unwrap_or_else(|| DailyTokenCount {
                date: today,
                ..Default::default()
            })
    }

    /// Get total tokens across all days for an agent.
    pub async fn total(&self, agent_name: &str) -> (u64, u64) {
        let counters = self.counters.lock().await;
        counters
            .get(agent_name)
            .map(|agent| {
                agent.values().fold((0u64, 0u64), |(inp, out), daily| {
                    (inp + daily.input_tokens, out + daily.output_tokens)
                })
            })
            .unwrap_or((0, 0))
    }

    /// Get all agents' today summaries.
    pub async fn all_today(&self) -> Vec<(String, DailyTokenCount)> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let counters = self.counters.lock().await;

        counters
            .iter()
            .map(|(name, agent)| {
                let daily = agent
                    .get(&today)
                    .cloned()
                    .unwrap_or_else(|| DailyTokenCount {
                        date: today.clone(),
                        ..Default::default()
                    });
                (name.clone(), daily)
            })
            .collect()
    }
}

impl Default for TokenTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn record_and_read() {
        let tracker = TokenTracker::new();

        tracker.record_usage("twitter", 100, 50, "sonnet").await;
        tracker.record_usage("twitter", 200, 80, "sonnet").await;

        let today = tracker.today("twitter").await;
        assert_eq!(today.input_tokens, 300);
        assert_eq!(today.output_tokens, 130);
        assert_eq!(today.call_count, 2);
    }

    #[tokio::test]
    async fn total_tokens() {
        let tracker = TokenTracker::new();

        tracker.record_usage("agent", 100, 50, "sonnet").await;
        tracker.record_usage("agent", 200, 80, "haiku").await;

        let (inp, out) = tracker.total("agent").await;
        assert_eq!(inp, 300);
        assert_eq!(out, 130);
    }

    #[tokio::test]
    async fn unknown_agent_returns_zero() {
        let tracker = TokenTracker::new();
        let today = tracker.today("nonexistent").await;
        assert_eq!(today.input_tokens, 0);
        assert_eq!(today.output_tokens, 0);
    }

    #[tokio::test]
    async fn multiple_agents() {
        let tracker = TokenTracker::new();

        tracker.record_usage("a", 100, 50, "sonnet").await;
        tracker.record_usage("b", 200, 80, "haiku").await;

        let all = tracker.all_today().await;
        assert_eq!(all.len(), 2);
    }
}
