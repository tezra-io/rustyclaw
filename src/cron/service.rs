use std::path::PathBuf;
use tracing::info;

use super::types::{CronJob, CronStore};

/// Service that manages and executes scheduled cron jobs.
pub struct CronService {
    store_path: PathBuf,
    store: CronStore,
}

impl CronService {
    pub fn new(data_dir: PathBuf) -> Self {
        let store_path = data_dir.join("cron.json");
        let store = Self::load_store(&store_path);
        Self { store_path, store }
    }

    fn load_store(path: &PathBuf) -> CronStore {
        if path.exists() {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            CronStore::default()
        }
    }

    fn save_store(&self) -> crate::error::Result<()> {
        let json =
            serde_json::to_string_pretty(&self.store).map_err(crate::error::NanobotError::Json)?;
        std::fs::write(&self.store_path, json)?;
        Ok(())
    }

    /// Add a new cron job.
    pub fn add(&mut self, job: CronJob) -> crate::error::Result<()> {
        self.store.jobs.push(job);
        self.save_store()
    }

    /// Remove a cron job by name.
    pub fn remove(&mut self, name: &str) -> crate::error::Result<bool> {
        let before = self.store.jobs.len();
        self.store.jobs.retain(|j| j.name != name);
        let removed = self.store.jobs.len() < before;
        if removed {
            self.save_store()?;
        }
        Ok(removed)
    }

    /// List all cron jobs.
    pub fn list(&self) -> &[CronJob] {
        &self.store.jobs
    }

    /// Get a job by name.
    pub fn get(&self, name: &str) -> Option<&CronJob> {
        self.store.jobs.iter().find(|j| j.name == name)
    }

    /// Run the cron scheduler loop (checks and fires jobs).
    pub async fn run(&mut self) {
        info!("Cron service started");
        loop {
            // TODO: Check each job's next_run time, fire if due,
            // update state, and save store.
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    }
}
