use std::path::PathBuf;
use tracing::{debug, info};

/// Periodic service that checks HEARTBEAT.md for agent wake-up prompts.
pub struct HeartbeatService {
    workspace: PathBuf,
    interval_secs: u64,
}

impl HeartbeatService {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            interval_secs: 60,
        }
    }

    pub fn with_interval(mut self, secs: u64) -> Self {
        self.interval_secs = secs;
        self
    }

    fn heartbeat_path(&self) -> PathBuf {
        self.workspace.join("HEARTBEAT.md")
    }

    /// Run the heartbeat check loop.
    pub async fn run(&self) {
        info!(
            "Heartbeat service started (interval: {}s)",
            self.interval_secs
        );

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(self.interval_secs)).await;

            let path = self.heartbeat_path();
            if path.exists() {
                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                    let content = content.trim();
                    if !content.is_empty() && content != "HEARTBEAT_OK" {
                        debug!("Heartbeat prompt found: {}", content);
                        // TODO: Send prompt to agent via process_direct
                        // Then check response for HEARTBEAT_OK token
                        // If found, clear the file
                    }
                }
            }
        }
    }
}
