use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{Duration, Instant};

const ELIXIR_HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
const ELIXIR_STARTUP_GRACE: Duration = Duration::from_secs(10);
const ELIXIR_SYNTH_API_PORT: u16 = 4001;
const ELIXIR_PLUGIN_API_PORT: u16 = 4002;

/// Status of the Elixir orchestrator subprocess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestratorStatus {
    /// Running and healthy
    Running,
    /// Starting up (within grace period)
    Starting,
    /// Not running (exited or never started)
    Stopped,
    /// Not available (Elixir not installed or build failed)
    Unavailable(String),
    /// Explicitly disabled via --no-elixir
    Disabled,
}

impl std::fmt::Display for OrchestratorStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Starting => write!(f, "starting"),
            Self::Stopped => write!(f, "stopped"),
            Self::Unavailable(reason) => write!(f, "unavailable ({reason})"),
            Self::Disabled => write!(f, "disabled"),
        }
    }
}

/// Manages the Elixir orchestrator as a supervised child process.
pub struct ElixirOrchestrator {
    child: Option<Child>,
    project_dir: PathBuf,
    status: OrchestratorStatus,
    rust_bridge_port: u16,
    bridge_socket_path: Option<PathBuf>,
    synth_port: u16,
    plugin_port: u16,
}

impl ElixirOrchestrator {
    /// Resolve the Elixir orchestrator project directory.
    /// Looks for: <binary_dir>/../elixir/rustyclaw_orchestrator, then fallback
    /// to the compiled-in project root.
    fn resolve_project_dir() -> Option<PathBuf> {
        // Try relative to the source repo root
        let candidates = [
            // Development: project root / elixir / rustyclaw_orchestrator
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("elixir")
                .join("rustyclaw_orchestrator"),
            // Installed: next to binary
            std::env::current_exe()
                .ok()?
                .parent()?
                .join("elixir")
                .join("rustyclaw_orchestrator"),
        ];

        candidates.into_iter().find(|p| p.join("mix.exs").exists())
    }

    /// Check if Elixir is installed and meets minimum version requirements.
    pub fn check_elixir_installed() -> Result<String> {
        let output = std::process::Command::new("elixir")
            .arg("--version")
            .output()
            .context("Elixir is not installed or not in PATH")?;

        if !output.status.success() {
            anyhow::bail!("elixir --version returned non-zero exit code");
        }

        let version_output = String::from_utf8_lossy(&output.stdout).to_string();

        // Parse Elixir version from output like "Elixir 1.17.0 ..."
        let elixir_version = version_output
            .lines()
            .find(|line| line.starts_with("Elixir"))
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("unknown");

        // Validate >= 1.17
        let parts: Vec<u32> = elixir_version
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();

        if parts.len() >= 2 && (parts[0] < 1 || (parts[0] == 1 && parts[1] < 17)) {
            anyhow::bail!(
                "Elixir version {elixir_version} is below minimum required 1.17. \
                 Please upgrade Elixir."
            );
        }

        Ok(elixir_version.to_string())
    }

    /// Check OTP version.
    pub fn check_otp_version() -> Result<String> {
        let output = std::process::Command::new("elixir")
            .arg("-e")
            .arg("IO.puts(:erlang.system_info(:otp_release))")
            .output()
            .context("Failed to query OTP version")?;

        if !output.status.success() {
            anyhow::bail!("Failed to determine OTP version");
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Create a new orchestrator manager. Does not start the process.
    pub fn new(rust_bridge_port: u16, bridge_socket_path: Option<PathBuf>) -> Self {
        let synth_port = std::env::var("RUSTYCLAW_ELIXIR_SYNTH_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(ELIXIR_SYNTH_API_PORT);
        let plugin_port = std::env::var("RUSTYCLAW_ELIXIR_PLUGIN_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(ELIXIR_PLUGIN_API_PORT);

        let project_dir = Self::resolve_project_dir().unwrap_or_default();
        Self {
            child: None,
            project_dir,
            status: OrchestratorStatus::Stopped,
            rust_bridge_port,
            bridge_socket_path,
            synth_port,
            plugin_port,
        }
    }

    /// Port the Elixir synth API listens on.
    pub fn synth_port(&self) -> u16 {
        self.synth_port
    }

    /// Port the Elixir plugin API listens on.
    pub fn plugin_port(&self) -> u16 {
        self.plugin_port
    }

    /// Attempt to start the Elixir orchestrator as a child process.
    /// Returns Ok on success, or the reason for degraded mode on failure.
    pub async fn start(&mut self) -> Result<Instant> {
        // Verify Elixir is installed (blocking shell command — run off async runtime)
        let elixir_check = tokio::task::spawn_blocking(Self::check_elixir_installed)
            .await
            .context("Elixir check task panicked")?;
        if let Err(e) = elixir_check {
            let reason = format!("Elixir not available: {e}");
            self.status = OrchestratorStatus::Unavailable(reason.clone());
            anyhow::bail!(reason);
        }

        // Verify project directory exists
        if !self.project_dir.join("mix.exs").exists() {
            let reason = format!(
                "Elixir orchestrator project not found at {}",
                self.project_dir.display()
            );
            self.status = OrchestratorStatus::Unavailable(reason.clone());
            anyhow::bail!(reason);
        }

        // Ensure deps are compiled (use spawn_blocking to avoid blocking the async runtime)
        if !self.project_dir.join("_build").exists() {
            tracing::info!("Elixir orchestrator: running initial mix deps.get + compile...");
            let project_dir = self.project_dir.clone();
            let compile_result = tokio::task::spawn_blocking(move || {
                let deps_status = std::process::Command::new("mix")
                    .arg("deps.get")
                    .current_dir(&project_dir)
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .status()
                    .context("Failed to run mix deps.get")?;

                if !deps_status.success() {
                    anyhow::bail!("mix deps.get failed");
                }

                let compile_status = std::process::Command::new("mix")
                    .arg("compile")
                    .current_dir(&project_dir)
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .status()
                    .context("Failed to run mix compile")?;

                if !compile_status.success() {
                    anyhow::bail!("mix compile failed");
                }

                Ok(())
            })
            .await
            .context("compile task panicked")?;

            if let Err(e) = compile_result {
                let reason = e.to_string();
                self.status = OrchestratorStatus::Unavailable(reason.clone());
                anyhow::bail!(reason);
            }
        }

        // Start the Elixir application
        let mut cmd = Command::new("elixir");
        cmd.arg("--no-halt")
            .arg("-S")
            .arg("mix")
            .arg("run")
            .current_dir(&self.project_dir)
            .env("RUSTYCLAW_BRIDGE_PORT", self.rust_bridge_port.to_string())
            .env("RUSTYCLAW_ELIXIR_SYNTH_PORT", self.synth_port.to_string())
            .env("RUSTYCLAW_ELIXIR_PLUGIN_PORT", self.plugin_port.to_string())
            .env("MIX_ENV", "prod")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if let Some(ref socket_path) = self.bridge_socket_path {
            cmd.env("RUSTYCLAW_BRIDGE_SOCKET", socket_path.as_os_str());
        }

        let mut child = cmd
            .spawn()
            .context("Failed to spawn Elixir orchestrator process")?;

        // Drain stdout/stderr to prevent pipe buffer deadlock (~64KB OS limit).
        // Forward to tracing at info (stdout) and warn (stderr) levels.
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::info!(target: "elixir.stdout", "{}", line);
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(target: "elixir.stderr", "{}", line);
                }
            });
        }

        let started_at = Instant::now();
        self.child = Some(child);
        self.status = OrchestratorStatus::Starting;

        Ok(started_at)
    }

    /// Check if the orchestrator process is still alive.
    pub fn is_alive(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(None) => true, // still running
                Ok(Some(_)) | Err(_) => {
                    self.status = OrchestratorStatus::Stopped;
                    false
                }
            }
        } else {
            false
        }
    }

    /// Probe the Elixir HTTP APIs for health.
    pub async fn health_check(&mut self) -> bool {
        if !self.is_alive() {
            return false;
        }

        // Try the synth API health endpoint
        let client = reqwest::Client::builder()
            .timeout(ELIXIR_HEALTH_TIMEOUT)
            .build();

        let Ok(client) = client else {
            return false;
        };

        let url = format!("http://127.0.0.1:{}/health", self.synth_port);
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                self.status = OrchestratorStatus::Running;
                true
            }
            _ => false,
        }
    }

    /// Get the current status.
    pub fn status(&self) -> &OrchestratorStatus {
        &self.status
    }

    /// Gracefully stop the Elixir process.
    pub async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Send SIGTERM first on Unix
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                if pid != 0 {
                    // SAFETY: `pid` is a valid, non-zero process ID obtained from
                    // `child.id()` (the child we just spawned). Sending SIGTERM to
                    // a specific positive PID is safe. We guard against pid == 0 to
                    // avoid signaling the entire process group.
                    unsafe {
                        libc::kill(pid as i32, libc::SIGTERM);
                    }
                }
            }

            // Give it a few seconds to shut down gracefully
            let timeout = tokio::time::sleep(Duration::from_secs(5));
            tokio::select! {
                _ = child.wait() => {},
                () = timeout => {
                    let _ = child.kill().await;
                }
            }

            self.status = OrchestratorStatus::Stopped;
        }
    }

    /// Get the project directory path.
    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    /// Mark the orchestrator as explicitly disabled.
    pub fn set_disabled(&mut self) {
        self.status = OrchestratorStatus::Disabled;
    }
}

/// Returns the startup grace duration for the Elixir orchestrator.
/// Health-check failures within this window after start are ignored.
pub fn startup_grace_duration() -> Duration {
    ELIXIR_STARTUP_GRACE
}

/// Resolve the synth API port from env or default.
pub fn resolve_synth_port() -> u16 {
    std::env::var("RUSTYCLAW_ELIXIR_SYNTH_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(ELIXIR_SYNTH_API_PORT)
}

/// Resolve the plugin API port from env or default.
pub fn resolve_plugin_port() -> u16 {
    std::env::var("RUSTYCLAW_ELIXIR_PLUGIN_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(ELIXIR_PLUGIN_API_PORT)
}

/// Check if the Elixir orchestrator is reachable via its HTTP APIs.
/// This is a standalone check that doesn't require a running ElixirOrchestrator instance.
pub async fn probe_orchestrator_health() -> Result<serde_json::Value> {
    let client = reqwest::Client::builder()
        .timeout(ELIXIR_HEALTH_TIMEOUT)
        .build()
        .context("Failed to build HTTP client")?;

    let synth_port = resolve_synth_port();
    let plugin_port = resolve_plugin_port();

    let synth_url = format!("http://127.0.0.1:{synth_port}/health");
    let synth_ok = client
        .get(&synth_url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    let plugin_url = format!("http://127.0.0.1:{plugin_port}/health");
    let plugin_ok = client
        .get(&plugin_url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    Ok(serde_json::json!({
        "synth_api": if synth_ok { "ok" } else { "unreachable" },
        "plugin_api": if plugin_ok { "ok" } else { "unreachable" },
        "synth_api_port": synth_port,
        "plugin_api_port": plugin_port,
    }))
}

/// Query the orchestrator for active agents, plugins, and synthesized tools counts.
pub async fn query_orchestrator_stats() -> OrchestratorStats {
    let client = match reqwest::Client::builder()
        .timeout(ELIXIR_HEALTH_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(_) => return OrchestratorStats::default(),
    };

    let synth_port = resolve_synth_port();
    let plugin_port = resolve_plugin_port();

    let agents = client
        .get(format!("http://127.0.0.1:{synth_port}/api/agents"))
        .send()
        .await
        .ok()
        .and_then(|r| {
            if r.status().is_success() {
                Some(r)
            } else {
                None
            }
        });

    let synth_tools = client
        .get(format!("http://127.0.0.1:{synth_port}/api/tools"))
        .send()
        .await
        .ok()
        .and_then(|r| {
            if r.status().is_success() {
                Some(r)
            } else {
                None
            }
        });

    let plugins = client
        .get(format!("http://127.0.0.1:{plugin_port}/api/plugins"))
        .send()
        .await
        .ok()
        .and_then(|r| {
            if r.status().is_success() {
                Some(r)
            } else {
                None
            }
        });

    // Parse counts from responses (best-effort)
    let agent_count = parse_list_count(agents).await;
    let synth_tool_count = parse_list_count(synth_tools).await;
    let plugin_count = parse_list_count(plugins).await;

    OrchestratorStats {
        active_agents: agent_count,
        synth_tools: synth_tool_count,
        active_plugins: plugin_count,
    }
}

async fn parse_list_count(response: Option<reqwest::Response>) -> Option<usize> {
    let resp = response?;
    let body: serde_json::Value = resp.json().await.ok()?;
    body.as_array().map(Vec::len)
}

/// Summary stats from the Elixir orchestrator.
#[derive(Debug, Clone, Default)]
pub struct OrchestratorStats {
    pub active_agents: Option<usize>,
    pub synth_tools: Option<usize>,
    pub active_plugins: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orchestrator_status_display() {
        assert_eq!(OrchestratorStatus::Running.to_string(), "running");
        assert_eq!(OrchestratorStatus::Starting.to_string(), "starting");
        assert_eq!(OrchestratorStatus::Stopped.to_string(), "stopped");
        assert_eq!(OrchestratorStatus::Disabled.to_string(), "disabled");
        assert_eq!(
            OrchestratorStatus::Unavailable("no elixir".into()).to_string(),
            "unavailable (no elixir)"
        );
    }

    #[test]
    fn orchestrator_new_sets_stopped() {
        let orch = ElixirOrchestrator::new(4200, None);
        assert_eq!(*orch.status(), OrchestratorStatus::Stopped);
    }

    #[test]
    fn orchestrator_stats_default() {
        let stats = OrchestratorStats::default();
        assert!(stats.active_agents.is_none());
        assert!(stats.synth_tools.is_none());
        assert!(stats.active_plugins.is_none());
    }
}
