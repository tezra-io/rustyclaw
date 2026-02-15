use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::agent::{AgentDefinition, ScheduleEntry};
use crate::bus::events::{AgentMessage, AgentMessageType};
use crate::bus::queue::MessageBus;

use super::state::{EntryState, SchedulerState};

/// A resolved scheduled item ready for the run loop.
#[derive(Debug, Clone)]
struct ScheduledItem {
    agent_name: String,
    schedule_index: usize,
    entry: ScheduleEntry,
    next_run: DateTime<Utc>,
}

/// Internal cron scheduler that fires agent tasks on schedule.
pub struct AgentScheduler {
    bus: Arc<MessageBus>,
    state: Arc<Mutex<SchedulerState>>,
    state_path: PathBuf,
    items: Arc<Mutex<Vec<ScheduledItem>>>,
}

impl AgentScheduler {
    /// Create a new scheduler with persisted state.
    pub fn new(bus: Arc<MessageBus>, data_dir: PathBuf) -> Self {
        let state_path = data_dir.join("scheduler_state.json");
        let state = SchedulerState::load(&state_path);

        Self {
            bus,
            state: Arc::new(Mutex::new(state)),
            state_path,
            items: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Register all schedules from a set of agent definitions.
    pub async fn register_agents(&self, agents: &[AgentDefinition]) {
        let mut items = self.items.lock().await;
        let state = self.state.lock().await;
        let now = Utc::now();

        items.clear();

        for agent in agents {
            for (idx, entry) in agent.schedule.iter().enumerate() {
                let next_run = if let Some(es) = state.get(&agent.name, idx) {
                    // If stored next_run is in the past, compute fresh
                    if es.next_run <= now {
                        compute_next_run(entry, now)
                    } else {
                        es.next_run
                    }
                } else {
                    compute_next_run(entry, now)
                };

                items.push(ScheduledItem {
                    agent_name: agent.name.clone(),
                    schedule_index: idx,
                    entry: entry.clone(),
                    next_run,
                });

                debug!(
                    agent = %agent.name,
                    index = idx,
                    next_run = %next_run,
                    "Registered schedule entry"
                );
            }
        }

        info!("Scheduler loaded {} schedule entries", items.len());
    }

    /// Run the scheduler loop. Checks for due jobs every second (sleeps up to 30s between checks).
    pub async fn run(&self) {
        info!("Agent scheduler started");

        loop {
            let sleep_duration = self.tick().await;
            tokio::time::sleep(sleep_duration).await;
        }
    }

    /// Single tick: fire all due jobs, return how long to sleep until next.
    async fn tick(&self) -> Duration {
        let now = Utc::now();
        let mut items = self.items.lock().await;
        let mut state = self.state.lock().await;

        let mut min_wait = Duration::from_secs(30);

        for item in items.iter_mut() {
            if item.next_run <= now {
                // Fire the job
                let task = match &item.entry {
                    ScheduleEntry::Cron { task, .. } => task.clone(),
                    ScheduleEntry::Every { task, .. } => task.clone(),
                };

                info!(
                    agent = %item.agent_name,
                    task = %task,
                    "Firing scheduled task"
                );

                let msg =
                    AgentMessage::new("scheduler", &item.agent_name, AgentMessageType::Task, &task);

                if let Err(e) = self.bus.send_to_agent(&item.agent_name, msg).await {
                    warn!(
                        agent = %item.agent_name,
                        error = %e,
                        "Failed to dispatch scheduled task"
                    );
                }

                // Update state
                let new_next = compute_next_run(&item.entry, now);
                item.next_run = new_next;

                let entry_state = EntryState {
                    last_run: Some(now),
                    next_run: new_next,
                    run_count: state
                        .get(&item.agent_name, item.schedule_index)
                        .map(|e| e.run_count + 1)
                        .unwrap_or(1),
                };
                state.set(&item.agent_name, item.schedule_index, entry_state);
            }

            // Calculate sleep time until nearest due item
            if item.next_run > now {
                let wait = (item.next_run - now)
                    .to_std()
                    .unwrap_or(Duration::from_secs(30));
                if wait < min_wait {
                    min_wait = wait;
                }
            }
        }

        // Persist state after processing
        if let Err(e) = state.save(&self.state_path) {
            error!("Failed to save scheduler state: {}", e);
        }

        // Don't sleep less than 100ms to avoid tight loops
        min_wait.max(Duration::from_millis(100))
    }

    /// Get a snapshot of all scheduled items (for status display).
    pub async fn snapshot(&self) -> Vec<(String, usize, DateTime<Utc>)> {
        let items = self.items.lock().await;
        items
            .iter()
            .map(|i| (i.agent_name.clone(), i.schedule_index, i.next_run))
            .collect()
    }

    /// Get the next N upcoming scheduled runs, sorted by time.
    pub async fn upcoming(&self, n: usize) -> Vec<(String, String, DateTime<Utc>)> {
        let items = self.items.lock().await;
        let mut upcoming: Vec<_> = items
            .iter()
            .map(|i| {
                let task = match &i.entry {
                    ScheduleEntry::Cron { task, .. } => task.clone(),
                    ScheduleEntry::Every { task, .. } => task.clone(),
                };
                (i.agent_name.clone(), task, i.next_run)
            })
            .collect();
        upcoming.sort_by_key(|(_, _, t)| *t);
        upcoming.truncate(n);
        upcoming
    }
}

/// Compute the next run time for a schedule entry after `after`.
pub fn compute_next_run(entry: &ScheduleEntry, after: DateTime<Utc>) -> DateTime<Utc> {
    match entry {
        ScheduleEntry::Cron { expression, .. } => {
            // Prepend seconds field for the cron crate
            let full_expr = format!("0 {}", expression);
            match cron::Schedule::from_str(&full_expr) {
                Ok(schedule) => schedule
                    .after(&after)
                    .next()
                    .unwrap_or_else(|| after + chrono::Duration::hours(1)),
                Err(_) => {
                    // Shouldn't happen since we validated during parsing
                    after + chrono::Duration::hours(1)
                }
            }
        }
        ScheduleEntry::Every { interval, .. } => {
            let duration = chrono::Duration::from_std(*interval)
                .unwrap_or_else(|_| chrono::Duration::hours(1));
            after + duration
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ScheduleEntry;

    #[test]
    fn compute_next_run_cron() {
        let entry = ScheduleEntry::Cron {
            expression: "0 10 * * *".to_string(),
            task: "morning".to_string(),
        };
        let now = Utc::now();
        let next = compute_next_run(&entry, now);
        assert!(next > now);
    }

    #[test]
    fn compute_next_run_every() {
        let entry = ScheduleEntry::Every {
            interval: Duration::from_secs(3600),
            task: "hourly".to_string(),
        };
        let now = Utc::now();
        let next = compute_next_run(&entry, now);
        let diff = (next - now).num_seconds();
        assert!((3599..=3601).contains(&diff));
    }

    #[tokio::test]
    async fn register_agents_populates_items() {
        let bus = Arc::new(MessageBus::new(16));
        let dir = tempfile::tempdir().unwrap();
        let scheduler = AgentScheduler::new(bus, dir.path().to_path_buf());

        let agents = vec![AgentDefinition {
            name: "twitter".to_string(),
            description: "tweets".to_string(),
            system_prompt: String::new(),
            model: None,
            tools: None,
            context_files: Vec::new(),
            memory_mode: crate::agent::MemoryMode::Isolated,
            schedule: vec![
                ScheduleEntry::Cron {
                    expression: "0 10 * * *".to_string(),
                    task: "morning".to_string(),
                },
                ScheduleEntry::Every {
                    interval: Duration::from_secs(7200),
                    task: "check".to_string(),
                },
            ],
            trigger: None,
        }];

        scheduler.register_agents(&agents).await;
        let snap = scheduler.snapshot().await;
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].0, "twitter");
        assert_eq!(snap[1].0, "twitter");
    }

    #[tokio::test]
    async fn tick_fires_due_jobs() {
        let bus = Arc::new(MessageBus::new(16));
        // Register an agent channel so send_to_agent works
        let _rx = bus.register_agent("test-agent").await;

        let dir = tempfile::tempdir().unwrap();
        let scheduler = AgentScheduler::new(bus.clone(), dir.path().to_path_buf());

        // Set up an item that's already due (next_run in the past)
        {
            let mut items = scheduler.items.lock().await;
            items.push(ScheduledItem {
                agent_name: "test-agent".to_string(),
                schedule_index: 0,
                entry: ScheduleEntry::Every {
                    interval: Duration::from_secs(3600),
                    task: "do stuff".to_string(),
                },
                next_run: Utc::now() - chrono::Duration::seconds(10),
            });
        }

        // Tick should fire it
        let _sleep = scheduler.tick().await;

        // Verify state was updated
        let state = scheduler.state.lock().await;
        let entry = state.get("test-agent", 0).unwrap();
        assert_eq!(entry.run_count, 1);
        assert!(entry.last_run.is_some());
    }

    #[tokio::test]
    async fn upcoming_sorted() {
        let bus = Arc::new(MessageBus::new(16));
        let dir = tempfile::tempdir().unwrap();
        let scheduler = AgentScheduler::new(bus, dir.path().to_path_buf());

        let now = Utc::now();
        {
            let mut items = scheduler.items.lock().await;
            items.push(ScheduledItem {
                agent_name: "b".to_string(),
                schedule_index: 0,
                entry: ScheduleEntry::Every {
                    interval: Duration::from_secs(3600),
                    task: "later".to_string(),
                },
                next_run: now + chrono::Duration::hours(2),
            });
            items.push(ScheduledItem {
                agent_name: "a".to_string(),
                schedule_index: 0,
                entry: ScheduleEntry::Every {
                    interval: Duration::from_secs(1800),
                    task: "sooner".to_string(),
                },
                next_run: now + chrono::Duration::hours(1),
            });
        }

        let upcoming = scheduler.upcoming(5).await;
        assert_eq!(upcoming.len(), 2);
        assert_eq!(upcoming[0].0, "a");
        assert_eq!(upcoming[1].0, "b");
    }
}
