use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Event emitted when watched files change.
#[derive(Debug, Clone)]
pub struct FileChangeEvent {
    pub path: PathBuf,
    pub agent_name: String,
    pub task: String,
}

/// Configuration for watching files/directories.
#[derive(Debug, Clone)]
pub struct FileWatchConfig {
    pub paths: Vec<PathBuf>,
    pub agent_name: String,
    pub task: String,
}

/// Watches files for changes using polling (no `notify` crate dependency).
/// Uses mtime-based change detection with debouncing.
pub struct FileTrigger {
    configs: Vec<FileWatchConfig>,
    poll_interval: Duration,
    debounce: Duration,
}

impl FileTrigger {
    pub fn new(configs: Vec<FileWatchConfig>) -> Self {
        Self {
            configs,
            poll_interval: Duration::from_secs(5),
            debounce: Duration::from_millis(500),
        }
    }

    /// Run the file watcher loop.
    pub async fn watch(&self, tx: mpsc::Sender<FileChangeEvent>) {
        if self.configs.is_empty() {
            return;
        }

        info!(
            "File trigger watching {} configurations",
            self.configs.len()
        );

        // Track last known mtime per path
        let mut known_mtimes: HashMap<PathBuf, std::time::SystemTime> = HashMap::new();
        // Track last fire time per agent to debounce
        let mut last_fired: HashMap<String, Instant> = HashMap::new();

        // Initialize mtimes
        for config in &self.configs {
            for path in &config.paths {
                if let Some(mtime) = get_mtime(path) {
                    known_mtimes.insert(path.clone(), mtime);
                }
            }
        }

        loop {
            tokio::time::sleep(self.poll_interval).await;

            for config in &self.configs {
                let mut changed = false;

                for path in &config.paths {
                    if let Some(new_mtime) = get_mtime(path) {
                        let old_mtime = known_mtimes.get(path);
                        if old_mtime.map(|o| *o != new_mtime).unwrap_or(true) {
                            known_mtimes.insert(path.clone(), new_mtime);
                            changed = true;
                            debug!(path = %path.display(), "File changed");
                        }
                    }
                }

                if changed {
                    // Debounce: skip if we fired for this agent recently
                    if let Some(last) = last_fired.get(&config.agent_name) {
                        if last.elapsed() < self.debounce {
                            continue;
                        }
                    }

                    info!(
                        agent = %config.agent_name,
                        "File change detected, firing trigger"
                    );

                    let event = FileChangeEvent {
                        path: config.paths.first().cloned().unwrap_or_default(),
                        agent_name: config.agent_name.clone(),
                        task: config.task.clone(),
                    };

                    last_fired.insert(config.agent_name.clone(), Instant::now());

                    if tx.send(event).await.is_err() {
                        warn!("File trigger channel closed");
                        return;
                    }
                }
            }
        }
    }
}

/// Get the modification time of a path (file or directory).
fn get_mtime(path: &PathBuf) -> Option<std::time::SystemTime> {
    // For directories, check the most recent mtime of direct children
    if path.is_dir() {
        let mut latest = std::fs::metadata(path).ok()?.modified().ok()?;
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(mtime) = meta.modified() {
                        if mtime > latest {
                            latest = mtime;
                        }
                    }
                }
            }
        }
        Some(latest)
    } else {
        std::fs::metadata(path).ok()?.modified().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_mtime_existing_file() {
        let mtime = get_mtime(&PathBuf::from("Cargo.toml"));
        assert!(mtime.is_some());
    }

    #[test]
    fn get_mtime_nonexistent() {
        let mtime = get_mtime(&PathBuf::from("/tmp/nonexistent_file_xyz"));
        assert!(mtime.is_none());
    }

    #[test]
    fn get_mtime_directory() {
        let mtime = get_mtime(&PathBuf::from("src"));
        assert!(mtime.is_some());
    }
}
