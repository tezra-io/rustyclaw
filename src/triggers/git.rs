use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{info, warn};

/// Event emitted when a git ref changes.
#[derive(Debug, Clone)]
pub struct GitPushEvent {
    pub repo_path: PathBuf,
    pub branch: String,
    pub old_hash: String,
    pub new_hash: String,
}

/// Configuration for watching a git repository.
#[derive(Debug, Clone)]
pub struct GitWatchConfig {
    pub repo_path: PathBuf,
    pub branches: Vec<String>,
    pub agent_name: String,
    pub task: String,
}

/// Watches git repositories for ref changes by polling.
pub struct GitTrigger {
    configs: Vec<GitWatchConfig>,
    poll_interval: Duration,
}

impl GitTrigger {
    pub fn new(configs: Vec<GitWatchConfig>, poll_interval: Duration) -> Self {
        Self {
            configs,
            poll_interval,
        }
    }

    /// Run the git watcher loop, emitting events when refs change.
    pub async fn watch(&self, tx: mpsc::Sender<(String, String, GitPushEvent)>) {
        if self.configs.is_empty() {
            return;
        }

        info!("Git trigger watching {} repositories", self.configs.len());

        // Track last known HEAD per (repo, branch)
        let mut known_refs: HashMap<(PathBuf, String), String> = HashMap::new();

        // Initialize with current refs
        for config in &self.configs {
            for branch in &config.branches {
                if let Some(hash) = read_git_ref(&config.repo_path, branch) {
                    known_refs.insert((config.repo_path.clone(), branch.clone()), hash);
                }
            }
        }

        loop {
            tokio::time::sleep(self.poll_interval).await;

            for config in &self.configs {
                for branch in &config.branches {
                    let key = (config.repo_path.clone(), branch.clone());

                    if let Some(new_hash) = read_git_ref(&config.repo_path, branch) {
                        let old_hash = known_refs.get(&key).cloned().unwrap_or_default();

                        if !old_hash.is_empty() && old_hash != new_hash {
                            info!(
                                repo = %config.repo_path.display(),
                                branch = %branch,
                                old = &old_hash[..8.min(old_hash.len())],
                                new = &new_hash[..8.min(new_hash.len())],
                                "Git ref changed"
                            );

                            let event = GitPushEvent {
                                repo_path: config.repo_path.clone(),
                                branch: branch.clone(),
                                old_hash,
                                new_hash: new_hash.clone(),
                            };

                            if tx
                                .send((config.agent_name.clone(), config.task.clone(), event))
                                .await
                                .is_err()
                            {
                                warn!("Git trigger channel closed");
                                return;
                            }
                        }

                        known_refs.insert(key, new_hash);
                    }
                }
            }
        }
    }
}

/// Read the current commit hash for a branch from a git repo.
fn read_git_ref(repo_path: &Path, branch: &str) -> Option<String> {
    // Try packed-refs first, then loose refs
    let ref_path = repo_path.join(".git/refs/heads").join(branch);
    if ref_path.exists() {
        return std::fs::read_to_string(&ref_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    }

    // Try packed-refs
    let packed_path = repo_path.join(".git/packed-refs");
    if packed_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&packed_path) {
            let suffix = format!("refs/heads/{}", branch);
            for line in content.lines() {
                if line.starts_with('#') {
                    continue;
                }
                if line.ends_with(&suffix) {
                    if let Some(hash) = line.split_whitespace().next() {
                        return Some(hash.to_string());
                    }
                }
            }
        }
    }

    // Try git rev-parse as fallback
    std::process::Command::new("git")
        .args(["rev-parse", &format!("refs/heads/{}", branch)])
        .current_dir(repo_path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_git_ref_nonexistent_repo() {
        let result = read_git_ref(Path::new("/tmp/nonexistent_repo_xyz"), "main");
        assert!(result.is_none());
    }

    #[test]
    fn read_git_ref_current_repo() {
        // This test repo should have a main branch
        let repo = std::env::current_dir().unwrap();
        let result = read_git_ref(&repo, "main");
        // May or may not exist depending on branch name, but shouldn't panic
        if let Some(hash) = result {
            assert!(!hash.is_empty());
            assert!(hash.len() >= 7); // at least short hash
        }
    }
}
