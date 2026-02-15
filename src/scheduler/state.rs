use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Unique key for a scheduled entry: (agent_name, schedule_index).
pub type ScheduleKey = (String, usize);

/// Persistent state for a single scheduled entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryState {
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: DateTime<Utc>,
    pub run_count: u64,
}

/// Persistent scheduler state, saved to disk between restarts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SchedulerState {
    /// Key: "agent_name:index" → entry state.
    pub entries: HashMap<String, EntryState>,
}

impl SchedulerState {
    /// Load state from disk, or return default if missing/corrupt.
    pub fn load(path: &PathBuf) -> Self {
        if path.exists() {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Self::default()
        }
    }

    /// Save state to disk.
    pub fn save(&self, path: &PathBuf) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// Get the state key string for an agent + schedule index.
    pub fn key(agent_name: &str, index: usize) -> String {
        format!("{}:{}", agent_name, index)
    }

    /// Get entry state, if it exists.
    pub fn get(&self, agent_name: &str, index: usize) -> Option<&EntryState> {
        self.entries.get(&Self::key(agent_name, index))
    }

    /// Set entry state.
    pub fn set(&mut self, agent_name: &str, index: usize, entry: EntryState) {
        self.entries.insert(Self::key(agent_name, index), entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_key_format() {
        assert_eq!(SchedulerState::key("twitter", 0), "twitter:0");
        assert_eq!(SchedulerState::key("monitor", 2), "monitor:2");
    }

    #[test]
    fn get_set_entry() {
        let mut state = SchedulerState::default();
        let entry = EntryState {
            last_run: None,
            next_run: Utc::now(),
            run_count: 0,
        };
        state.set("agent", 0, entry.clone());
        let got = state.get("agent", 0).unwrap();
        assert_eq!(got.run_count, 0);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let path = PathBuf::from("/tmp/rustyclaw_test_nonexistent_scheduler.json");
        let state = SchedulerState::load(&path);
        assert!(state.entries.is_empty());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let path = PathBuf::from("/tmp/rustyclaw_test_scheduler_state.json");
        let mut state = SchedulerState::default();
        state.set(
            "test",
            0,
            EntryState {
                last_run: None,
                next_run: Utc::now(),
                run_count: 5,
            },
        );
        state.save(&path).unwrap();

        let loaded = SchedulerState::load(&path);
        assert_eq!(loaded.get("test", 0).unwrap().run_count, 5);

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }
}
