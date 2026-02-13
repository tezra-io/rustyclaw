use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::bus::events::{AgentMessage, AgentMessageType};
use crate::bus::queue::MessageBus;

/// Agent lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Initializing,
    Idle,
    Running,
    Paused,
    Failed,
    Stopped,
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentState::Initializing => write!(f, "initializing"),
            AgentState::Idle => write!(f, "idle"),
            AgentState::Running => write!(f, "running"),
            AgentState::Paused => write!(f, "paused"),
            AgentState::Failed => write!(f, "failed"),
            AgentState::Stopped => write!(f, "stopped"),
        }
    }
}

/// Tracks an individual agent's runtime state.
pub struct AgentHandle {
    pub name: String,
    pub state: AgentState,
    pub join_handle: Option<JoinHandle<()>>,
    pub last_active: chrono::DateTime<chrono::Utc>,
    pub error_count: u64,
    pub consecutive_failures: u32,
}

/// Maximum consecutive panics before an agent is stopped permanently.
const MAX_CONSECUTIVE_PANICS: u32 = 5;

/// Base backoff duration for restart (doubles each time, max 60s).
const BASE_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Supervises worker agents: tracks state, restarts on panic, enforces backoff.
pub struct AgentSupervisor {
    handles: Arc<Mutex<HashMap<String, AgentHandle>>>,
    bus: Arc<MessageBus>,
}

impl AgentSupervisor {
    pub fn new(bus: Arc<MessageBus>) -> Self {
        Self {
            handles: Arc::new(Mutex::new(HashMap::new())),
            bus,
        }
    }

    /// Register an agent with its initial spawned task handle.
    pub async fn register(&self, name: &str, handle: JoinHandle<()>) {
        let agent_handle = AgentHandle {
            name: name.to_string(),
            state: AgentState::Running,
            join_handle: Some(handle),
            last_active: chrono::Utc::now(),
            error_count: 0,
            consecutive_failures: 0,
        };
        self.handles
            .lock()
            .await
            .insert(name.to_string(), agent_handle);
        info!(agent = name, "Agent registered with supervisor");
    }

    /// Get the current state of an agent.
    pub async fn get_state(&self, name: &str) -> Option<AgentState> {
        self.handles.lock().await.get(name).map(|h| h.state)
    }

    /// Get a snapshot of all agent states.
    pub async fn all_states(&self) -> Vec<(String, AgentState, u64, u32)> {
        self.handles
            .lock()
            .await
            .values()
            .map(|h| {
                (
                    h.name.clone(),
                    h.state,
                    h.error_count,
                    h.consecutive_failures,
                )
            })
            .collect()
    }

    /// Pause an agent (marks as paused, does not kill the task).
    pub async fn pause(&self, name: &str) -> Result<(), String> {
        let mut handles = self.handles.lock().await;
        let handle = handles
            .get_mut(name)
            .ok_or_else(|| format!("Agent '{}' not found", name))?;

        match handle.state {
            AgentState::Running | AgentState::Idle => {
                handle.state = AgentState::Paused;
                info!(agent = name, "Agent paused");
                Ok(())
            }
            other => Err(format!(
                "Cannot pause agent '{}' in state '{}'",
                name, other
            )),
        }
    }

    /// Resume a paused agent.
    pub async fn resume(&self, name: &str) -> Result<(), String> {
        let mut handles = self.handles.lock().await;
        let handle = handles
            .get_mut(name)
            .ok_or_else(|| format!("Agent '{}' not found", name))?;

        if handle.state == AgentState::Paused {
            handle.state = AgentState::Idle;
            info!(agent = name, "Agent resumed");
            Ok(())
        } else {
            Err(format!(
                "Cannot resume agent '{}' — not paused (state: '{}')",
                name, handle.state
            ))
        }
    }

    /// Shutdown a specific agent.
    pub async fn shutdown(&self, name: &str) -> Result<(), String> {
        let mut handles = self.handles.lock().await;
        let handle = handles
            .get_mut(name)
            .ok_or_else(|| format!("Agent '{}' not found", name))?;

        handle.state = AgentState::Stopped;
        if let Some(jh) = handle.join_handle.take() {
            jh.abort();
        }
        info!(agent = name, "Agent shut down");
        Ok(())
    }

    /// Shutdown all agents.
    pub async fn shutdown_all(&self) {
        let mut handles = self.handles.lock().await;
        for (name, handle) in handles.iter_mut() {
            handle.state = AgentState::Stopped;
            if let Some(jh) = handle.join_handle.take() {
                jh.abort();
            }
            info!(agent = %name, "Agent shut down");
        }
    }

    /// Run the supervision loop: monitors all agent JoinHandles for panics,
    /// auto-restarts with exponential backoff, alerts master after max failures.
    ///
    /// The `spawn_fn` callback is used to re-spawn an agent by name.
    pub async fn supervise<F>(&self, spawn_fn: Arc<F>)
    where
        F: Fn(String) -> JoinHandle<()> + Send + Sync + 'static,
    {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;

            let names: Vec<String> = {
                let handles = self.handles.lock().await;
                handles.keys().cloned().collect()
            };

            for name in names {
                let mut handles = self.handles.lock().await;
                let handle = match handles.get_mut(&name) {
                    Some(h) => h,
                    None => continue,
                };

                // Only check Running agents with a JoinHandle
                if handle.state != AgentState::Running {
                    continue;
                }

                let finished = handle
                    .join_handle
                    .as_ref()
                    .map(|jh| jh.is_finished())
                    .unwrap_or(true);

                if !finished {
                    continue;
                }

                // Task finished — check if it was clean or a panic
                if let Some(jh) = handle.join_handle.take() {
                    match jh.await {
                        Ok(()) => {
                            // Clean shutdown
                            handle.state = AgentState::Idle;
                            handle.consecutive_failures = 0;
                            handle.last_active = chrono::Utc::now();
                            info!(agent = %name, "Agent task completed cleanly");
                        }
                        Err(e) => {
                            handle.error_count += 1;
                            handle.consecutive_failures += 1;
                            handle.last_active = chrono::Utc::now();

                            if e.is_panic() {
                                error!(
                                    agent = %name,
                                    consecutive = handle.consecutive_failures,
                                    "Agent panicked: {:?}", e
                                );
                            } else {
                                warn!(agent = %name, "Agent task cancelled: {:?}", e);
                            }

                            if handle.consecutive_failures >= MAX_CONSECUTIVE_PANICS {
                                handle.state = AgentState::Stopped;
                                error!(
                                    agent = %name,
                                    "Agent stopped after {} consecutive failures",
                                    MAX_CONSECUTIVE_PANICS
                                );

                                // Alert master
                                let alert = AgentMessage::new(
                                    &name,
                                    "master",
                                    AgentMessageType::Alert,
                                    &format!(
                                        "Agent '{}' stopped after {} consecutive failures",
                                        name, MAX_CONSECUTIVE_PANICS
                                    ),
                                );
                                let bus = self.bus.clone();
                                // Drop lock before async operation
                                drop(handles);
                                if let Err(e) = bus.send_to_master(alert).await {
                                    error!("Failed to alert master about stopped agent: {}", e);
                                }
                                continue;
                            }

                            // Calculate backoff
                            let backoff = calculate_backoff(handle.consecutive_failures);
                            handle.state = AgentState::Failed;

                            let agent_name = name.clone();
                            let spawn_fn = spawn_fn.clone();
                            let handles_ref = self.handles.clone();

                            // Drop lock before spawning restart timer
                            drop(handles);

                            info!(
                                agent = %agent_name,
                                backoff_secs = backoff.as_secs(),
                                "Scheduling agent restart"
                            );

                            tokio::spawn(async move {
                                tokio::time::sleep(backoff).await;

                                let mut handles = handles_ref.lock().await;
                                if let Some(h) = handles.get_mut(&agent_name) {
                                    // Only restart if still in Failed state (not manually stopped)
                                    if h.state == AgentState::Failed {
                                        let new_handle = spawn_fn(agent_name.clone());
                                        h.join_handle = Some(new_handle);
                                        h.state = AgentState::Running;
                                        info!(agent = %agent_name, "Agent restarted");
                                    }
                                }
                            });
                        }
                    }
                }
            }
        }
    }

    /// Mark an agent as actively running (called when it picks up a task).
    pub async fn mark_running(&self, name: &str) {
        let mut handles = self.handles.lock().await;
        if let Some(h) = handles.get_mut(name) {
            if h.state == AgentState::Idle || h.state == AgentState::Initializing {
                h.state = AgentState::Running;
                h.last_active = chrono::Utc::now();
            }
        }
    }

    /// Mark an agent as idle (called when it finishes a task and waits).
    pub async fn mark_idle(&self, name: &str) {
        let mut handles = self.handles.lock().await;
        if let Some(h) = handles.get_mut(name) {
            if h.state == AgentState::Running {
                h.state = AgentState::Idle;
                h.last_active = chrono::Utc::now();
                h.consecutive_failures = 0;
            }
        }
    }
}

/// Calculate exponential backoff duration from consecutive failure count.
fn calculate_backoff(failures: u32) -> Duration {
    let secs = BASE_BACKOFF.as_secs() * 2u64.saturating_pow(failures.saturating_sub(1));
    Duration::from_secs(secs.min(MAX_BACKOFF.as_secs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_display() {
        assert_eq!(AgentState::Running.to_string(), "running");
        assert_eq!(AgentState::Paused.to_string(), "paused");
        assert_eq!(AgentState::Stopped.to_string(), "stopped");
    }

    #[test]
    fn backoff_calculation() {
        assert_eq!(calculate_backoff(1), Duration::from_secs(1));
        assert_eq!(calculate_backoff(2), Duration::from_secs(2));
        assert_eq!(calculate_backoff(3), Duration::from_secs(4));
        assert_eq!(calculate_backoff(4), Duration::from_secs(8));
        assert_eq!(calculate_backoff(5), Duration::from_secs(16));
        // Should cap at MAX_BACKOFF (60s)
        assert_eq!(calculate_backoff(10), Duration::from_secs(60));
    }

    #[tokio::test]
    async fn register_and_get_state() {
        let bus = Arc::new(MessageBus::new(16));
        let supervisor = AgentSupervisor::new(bus);

        let handle = tokio::spawn(async {});
        supervisor.register("test-agent", handle).await;

        // Wait for the trivial task to finish
        tokio::time::sleep(Duration::from_millis(50)).await;

        let state = supervisor.get_state("test-agent").await;
        assert!(state.is_some());
    }

    #[tokio::test]
    async fn pause_resume() {
        let bus = Arc::new(MessageBus::new(16));
        let supervisor = AgentSupervisor::new(bus);

        let handle = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        supervisor.register("agent", handle).await;

        // Pause
        supervisor.pause("agent").await.unwrap();
        assert_eq!(
            supervisor.get_state("agent").await,
            Some(AgentState::Paused)
        );

        // Resume
        supervisor.resume("agent").await.unwrap();
        assert_eq!(supervisor.get_state("agent").await, Some(AgentState::Idle));

        // Cleanup
        supervisor.shutdown("agent").await.unwrap();
    }

    #[tokio::test]
    async fn pause_nonexistent_fails() {
        let bus = Arc::new(MessageBus::new(16));
        let supervisor = AgentSupervisor::new(bus);
        let err = supervisor.pause("ghost").await.unwrap_err();
        assert!(err.contains("not found"));
    }

    #[tokio::test]
    async fn shutdown_sets_stopped() {
        let bus = Arc::new(MessageBus::new(16));
        let supervisor = AgentSupervisor::new(bus);

        let handle = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        supervisor.register("agent", handle).await;

        supervisor.shutdown("agent").await.unwrap();
        assert_eq!(
            supervisor.get_state("agent").await,
            Some(AgentState::Stopped)
        );
    }

    #[tokio::test]
    async fn all_states() {
        let bus = Arc::new(MessageBus::new(16));
        let supervisor = AgentSupervisor::new(bus);

        let h1 = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        let h2 = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        supervisor.register("a", h1).await;
        supervisor.register("b", h2).await;

        let states = supervisor.all_states().await;
        assert_eq!(states.len(), 2);

        supervisor.shutdown_all().await;
    }
}
