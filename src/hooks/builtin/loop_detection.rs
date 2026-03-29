use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use crate::config::schema::LoopDetectionConfig;
use crate::hooks::traits::{HookHandler, HookResult};

/// Hash of a (tool_name, sorted_args_json) pair.
type CallHash = [u8; 32];

/// In-memory sliding window that detects repetitive tool calls.
pub struct LoopDetectionHook {
    config: LoopDetectionConfig,
    state: Mutex<WindowState>,
}

struct WindowState {
    window: VecDeque<CallHash>,
    counts: HashMap<CallHash, usize>,
}

impl LoopDetectionHook {
    pub fn new(config: LoopDetectionConfig) -> Self {
        Self {
            config,
            state: Mutex::new(WindowState {
                window: VecDeque::new(),
                counts: HashMap::new(),
            }),
        }
    }
}

fn hash_tool_call(name: &str, args: &Value) -> CallHash {
    let sorted = canonicalize_value(args);
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(b"\0");
    hasher.update(sorted.as_bytes());
    hasher.finalize().into()
}

/// Produce a deterministic JSON string by sorting object keys recursively.
fn canonicalize_value(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let entries: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap(),
                        canonicalize_value(&map[*k])
                    )
                })
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(canonicalize_value).collect();
            format!("[{}]", items.join(","))
        }
        _ => serde_json::to_string(value).unwrap(),
    }
}

#[async_trait]
impl HookHandler for LoopDetectionHook {
    fn name(&self) -> &str {
        "loop-detection"
    }

    fn priority(&self) -> i32 {
        100
    }

    async fn before_tool_call(&self, name: String, args: Value) -> HookResult<(String, Value)> {
        let hash = hash_tool_call(&name, &args);

        let count = {
            let mut state = self.state.lock().unwrap();

            // Evict oldest if window is full.
            if state.window.len() >= self.config.window_size {
                if let Some(old) = state.window.pop_front() {
                    let c = state.counts.get_mut(&old).unwrap();
                    *c -= 1;
                    if *c == 0 {
                        state.counts.remove(&old);
                    }
                }
            }

            state.window.push_back(hash);
            let count = state.counts.entry(hash).or_insert(0);
            *count += 1;
            *count
        };

        if count >= self.config.hard_stop_threshold {
            return HookResult::Cancel(format!(
                "loop detected: {} called {} times in last {} turns",
                name, count, self.config.window_size,
            ));
        }

        if count >= self.config.warn_threshold {
            tracing::warn!(
                hook = "loop-detection",
                tool = %name,
                count,
                window = self.config.window_size,
                "repetitive tool call detected",
            );
        }

        HookResult::Continue((name, args))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(window: usize, warn: usize, stop: usize) -> LoopDetectionConfig {
        LoopDetectionConfig {
            window_size: window,
            warn_threshold: warn,
            hard_stop_threshold: stop,
        }
    }

    #[tokio::test]
    async fn unique_calls_pass_through() {
        let hook = LoopDetectionHook::new(cfg(5, 2, 3));
        for i in 0..5 {
            let result = hook
                .before_tool_call(format!("tool_{i}"), serde_json::json!({"x": i}))
                .await;
            assert!(!result.is_cancel(), "unique call {i} should pass");
        }
    }

    #[tokio::test]
    async fn warns_at_threshold() {
        let hook = LoopDetectionHook::new(cfg(10, 3, 5));
        // Call same tool 3 times — should warn but continue.
        for _ in 0..3 {
            let result = hook
                .before_tool_call("shell".into(), serde_json::json!({"cmd": "ls"}))
                .await;
            assert!(!result.is_cancel());
        }
    }

    #[tokio::test]
    async fn hard_stops_at_threshold() {
        let hook = LoopDetectionHook::new(cfg(10, 3, 5));
        for i in 0..5 {
            let result = hook
                .before_tool_call("shell".into(), serde_json::json!({"cmd": "ls"}))
                .await;
            if i < 4 {
                assert!(!result.is_cancel(), "call {i} should pass");
            } else {
                assert!(result.is_cancel(), "call {i} should be cancelled");
                if let HookResult::Cancel(msg) = result {
                    assert!(msg.contains("loop detected"));
                    assert!(msg.contains("shell"));
                    assert!(msg.contains("5 times"));
                }
            }
        }
    }

    #[tokio::test]
    async fn different_args_different_hashes() {
        let hook = LoopDetectionHook::new(cfg(10, 2, 3));
        // Same tool, different args — should never trigger.
        for i in 0..5 {
            let result = hook
                .before_tool_call(
                    "shell".into(),
                    serde_json::json!({"cmd": format!("cmd_{i}")}),
                )
                .await;
            assert!(!result.is_cancel(), "different args call {i} should pass");
        }
    }

    #[tokio::test]
    async fn window_evicts_oldest() {
        let hook = LoopDetectionHook::new(cfg(3, 2, 3));
        // Fill window: A, A, B → A count = 2
        hook.before_tool_call("A".into(), serde_json::json!({}))
            .await;
        hook.before_tool_call("A".into(), serde_json::json!({}))
            .await;
        hook.before_tool_call("B".into(), serde_json::json!({}))
            .await;
        // Window is [A, A, B], full. Next push evicts first A.
        // Push C → window becomes [A, B, C], A count = 1
        hook.before_tool_call("C".into(), serde_json::json!({}))
            .await;
        // Push A → window becomes [B, C, A], A count = 1 (evicted one earlier)
        // This should NOT warn (count=1 < warn=2) — not hard-stop either.
        let result = hook
            .before_tool_call("A".into(), serde_json::json!({}))
            .await;
        assert!(!result.is_cancel(), "eviction should have reduced A count");
    }

    #[tokio::test]
    async fn canonicalize_sorts_keys() {
        // Same keys, different insertion order → same hash.
        let args_a = serde_json::json!({"b": 2, "a": 1});
        let args_b = serde_json::json!({"a": 1, "b": 2});
        assert_eq!(hash_tool_call("t", &args_a), hash_tool_call("t", &args_b));
    }

    #[tokio::test]
    async fn canonicalize_nested_objects() {
        let args_a = serde_json::json!({"outer": {"z": 1, "a": 2}});
        let args_b = serde_json::json!({"outer": {"a": 2, "z": 1}});
        assert_eq!(hash_tool_call("t", &args_a), hash_tool_call("t", &args_b));
    }

    #[tokio::test]
    async fn default_config_values() {
        let cfg = LoopDetectionConfig::default();
        assert_eq!(cfg.window_size, 20);
        assert_eq!(cfg.warn_threshold, 3);
        assert_eq!(cfg.hard_stop_threshold, 5);
    }
}
