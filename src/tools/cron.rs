use std::sync::Arc;

use async_trait::async_trait;

use crate::scheduler::AgentScheduler;

/// Tool for querying scheduled agent tasks.
pub struct CronTool {
    scheduler: Option<Arc<AgentScheduler>>,
}

impl CronTool {
    pub fn new() -> Self {
        Self { scheduler: None }
    }

    /// Create with a live scheduler reference.
    pub fn with_scheduler(scheduler: Arc<AgentScheduler>) -> Self {
        Self {
            scheduler: Some(scheduler),
        }
    }
}

impl Default for CronTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl super::base::Tool for CronTool {
    fn name(&self) -> &str {
        "cron"
    }

    fn description(&self) -> &str {
        "View scheduled agent tasks: list all schedules or show upcoming runs."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "upcoming"],
                    "description": "Action: 'list' all schedules, 'upcoming' next N runs"
                },
                "count": {
                    "type": "integer",
                    "description": "Number of upcoming runs to show (default 10)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> crate::error::Result<String> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| crate::error::RustyClawError::Tool("Missing 'action'".into()))?;

        let scheduler = self.scheduler.as_ref().ok_or_else(|| {
            crate::error::RustyClawError::Tool("Scheduler not initialized".into())
        })?;

        match action {
            "list" => {
                let snapshot = scheduler.snapshot().await;
                if snapshot.is_empty() {
                    return Ok("No scheduled tasks.".into());
                }
                let mut lines = vec!["Scheduled tasks:".to_string()];
                for (agent, idx, next_run) in &snapshot {
                    lines.push(format!(
                        "  • {}[{}] — next run: {}",
                        agent,
                        idx,
                        next_run.format("%Y-%m-%d %H:%M:%S UTC")
                    ));
                }
                Ok(lines.join("\n"))
            }
            "upcoming" => {
                let count = args["count"].as_u64().unwrap_or(10) as usize;
                let upcoming = scheduler.upcoming(count).await;
                if upcoming.is_empty() {
                    return Ok("No upcoming scheduled runs.".into());
                }
                let mut lines = vec![format!("Next {} scheduled runs:", upcoming.len())];
                for (agent, task, time) in &upcoming {
                    lines.push(format!(
                        "  • {} — \"{}\" at {}",
                        agent,
                        task,
                        time.format("%Y-%m-%d %H:%M:%S UTC")
                    ));
                }
                Ok(lines.join("\n"))
            }
            _ => Err(crate::error::RustyClawError::Tool(format!(
                "Unknown action: {}",
                action
            ))),
        }
    }
}
