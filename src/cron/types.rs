use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Schedule type for a cron job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CronSchedule {
    /// Run at a specific time daily (HH:MM).
    At { time: String },
    /// Run every N minutes/hours.
    Every { interval: String },
    /// Standard cron expression.
    Cron { expression: String },
}

/// Payload to execute when a cron job fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronPayload {
    pub prompt: String,
    pub channel: Option<String>,
    pub chat_id: Option<String>,
}

/// State of a cron job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobState {
    pub enabled: bool,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: Option<DateTime<Utc>>,
    pub run_count: u64,
}

impl Default for CronJobState {
    fn default() -> Self {
        Self {
            enabled: true,
            last_run: None,
            next_run: None,
            run_count: 0,
        }
    }
}

/// A scheduled cron job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub name: String,
    pub schedule: CronSchedule,
    pub payload: CronPayload,
    pub state: CronJobState,
}

/// Persistent store of all cron jobs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CronStore {
    pub jobs: Vec<CronJob>,
}
