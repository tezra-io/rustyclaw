use crate::agent::bus::AgentBus;
use crate::config::Config;
use crate::cron::{due_jobs, reschedule_after_run, CronJob};
use crate::security::SecurityPolicy;
use anyhow::Result;
use chrono::Utc;
use futures_util::{stream, StreamExt};
use std::sync::Arc;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{self, Duration};

const MIN_POLL_SECONDS: u64 = 5;
const AGENT_DELEGATE_TIMEOUT_SECS: u64 = 120;
/// Maximum wall-clock time a shell cron job may run before being killed.
const SHELL_JOB_TIMEOUT_SECS: u64 = 120;

pub async fn run(config: Config, bus: Option<Arc<AgentBus>>) -> Result<()> {
    if !config.scheduler.enabled {
        tracing::info!("Scheduler disabled by config");
        crate::health::mark_component_ok("scheduler");
        loop {
            time::sleep(Duration::from_secs(3600)).await;
        }
    }

    let poll_secs = config.reliability.scheduler_poll_secs.max(MIN_POLL_SECONDS);
    let mut interval = time::interval(Duration::from_secs(poll_secs));
    let security = Arc::new(SecurityPolicy::from_config(
        &config.autonomy,
        &config.workspace_dir,
    ));

    crate::health::mark_component_ok("scheduler");

    loop {
        interval.tick().await;

        let jobs = match due_jobs(&config, Utc::now()) {
            Ok(jobs) => jobs,
            Err(e) => {
                crate::health::mark_component_error("scheduler", e.to_string());
                tracing::warn!("Scheduler query failed: {e}");
                continue;
            }
        };

        if !jobs.is_empty() {
            process_due_jobs(&config, &security, jobs, bus.clone()).await;
        }
    }
}

/// Run all due jobs concurrently (up to `max_concurrent` at once) and persist results.
async fn process_due_jobs(
    config: &Config,
    security: &Arc<SecurityPolicy>,
    jobs: Vec<CronJob>,
    bus: Option<Arc<AgentBus>>,
) {
    let max_concurrent = config.scheduler.max_concurrent.max(1);

    // Clone the Arc<AgentBus> into each future so all jobs (including agent: jobs)
    // run concurrently with full bus access.
    let mut in_flight = stream::iter(jobs.into_iter().map(|job| {
        let config = config.clone();
        let security = Arc::clone(security);
        let bus = bus.clone();
        async move {
            let job_id = job.id.clone();
            let (success, output) =
                execute_job_with_retry(&config, &security, &job, bus.as_deref()).await;
            if let Err(e) = reschedule_after_run(&config, &job, success, &output) {
                tracing::warn!("Failed to persist run result for {job_id}: {e}");
            }
            (job_id, success)
        }
    }))
    .buffer_unordered(max_concurrent);

    while let Some((job_id, success)) = in_flight.next().await {
        if success {
            crate::health::mark_component_ok("scheduler");
        } else {
            crate::health::mark_component_error("scheduler", format!("job {job_id} failed"));
        }
    }
}

async fn execute_job_with_retry(
    config: &Config,
    security: &SecurityPolicy,
    job: &CronJob,
    bus: Option<&AgentBus>,
) -> (bool, String) {
    let mut last_output = String::new();
    let retries = config.reliability.scheduler_retries;
    let mut backoff_ms = config.reliability.provider_backoff_ms.max(200);

    for attempt in 0..=retries {
        let (success, output) = run_job_command(config, security, job, bus).await;
        last_output = output;

        if success {
            return (true, last_output);
        }

        if last_output.starts_with("blocked by security policy:") {
            // Deterministic policy violations are not retryable.
            return (false, last_output);
        }

        if attempt < retries {
            let jitter_ms = u64::from(Utc::now().timestamp_subsec_millis() % 250);
            time::sleep(Duration::from_millis(backoff_ms + jitter_ms)).await;
            backoff_ms = (backoff_ms.saturating_mul(2)).min(30_000);
        }
    }

    (false, last_output)
}

fn is_env_assignment(word: &str) -> bool {
    word.contains('=')
        && word
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
}

fn strip_wrapping_quotes(token: &str) -> &str {
    token.trim_matches(|c| c == '"' || c == '\'')
}

fn forbidden_path_argument(security: &SecurityPolicy, command: &str) -> Option<String> {
    let mut normalized = command.to_string();
    for sep in ["&&", "||"] {
        normalized = normalized.replace(sep, "\x00");
    }
    for sep in ['\n', ';', '|'] {
        normalized = normalized.replace(sep, "\x00");
    }

    for segment in normalized.split('\x00') {
        let tokens: Vec<&str> = segment.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }

        // Skip leading env assignments and executable token.
        let mut idx = 0;
        while idx < tokens.len() && is_env_assignment(tokens[idx]) {
            idx += 1;
        }
        if idx >= tokens.len() {
            continue;
        }
        idx += 1;

        for token in &tokens[idx..] {
            let candidate = strip_wrapping_quotes(token);
            if candidate.is_empty() || candidate.starts_with('-') || candidate.contains("://") {
                continue;
            }

            let looks_like_path = candidate.starts_with('/')
                || candidate.starts_with("./")
                || candidate.starts_with("../")
                || candidate.starts_with("~/")
                || candidate.contains('/');

            if looks_like_path && !security.is_path_allowed(candidate) {
                return Some(candidate.to_string());
            }
        }
    }

    None
}

/// Parse and execute an `agent:name` or `agent:name:message` command via the bus.
async fn run_agent_job(bus: &AgentBus, command: &str) -> (bool, String) {
    // Strip the "agent:" prefix; remainder is "name" or "name:message"
    let rest = &command["agent:".len()..];
    let (agent_name, message) = match rest.find(':') {
        Some(idx) => (&rest[..idx], &rest[idx + 1..]),
        None => (rest, ""),
    };

    if agent_name.is_empty() {
        return (false, "agent command missing agent name".to_string());
    }

    if !bus.is_registered(agent_name).await {
        return (
            false,
            format!("agent '{agent_name}' is not registered on the bus"),
        );
    }

    let timeout = std::time::Duration::from_secs(AGENT_DELEGATE_TIMEOUT_SECS);
    match bus.delegate("cron", agent_name, message, timeout).await {
        Ok(response) => (true, response),
        Err(e) => (false, format!("agent delegation failed: {e}")),
    }
}

async fn run_job_command(
    config: &Config,
    security: &SecurityPolicy,
    job: &CronJob,
    bus: Option<&AgentBus>,
) -> (bool, String) {
    if !security.can_act() {
        return (
            false,
            "blocked by security policy: autonomy is read-only".to_string(),
        );
    }

    if security.is_rate_limited() {
        return (
            false,
            "blocked by security policy: rate limit exceeded".to_string(),
        );
    }

    if !security.record_action() {
        return (
            false,
            "blocked by security policy: action budget exhausted".to_string(),
        );
    }

    // Delegate to agent via bus if command starts with "agent:"
    // (skip shell-specific security checks — agent commands don't run shell)
    if job.command.starts_with("agent:") {
        return match bus {
            Some(bus) => run_agent_job(bus, &job.command).await,
            None => (
                false,
                "agent command requires agent bus (not available)".to_string(),
            ),
        };
    }

    if !security.is_command_allowed(&job.command) {
        return (
            false,
            format!(
                "blocked by security policy: command not allowed: {}",
                job.command
            ),
        );
    }

    if let Some(path) = forbidden_path_argument(security, &job.command) {
        return (
            false,
            format!("blocked by security policy: forbidden path argument: {path}"),
        );
    }

    run_job_command_with_timeout(
        config,
        security,
        job,
        Duration::from_secs(SHELL_JOB_TIMEOUT_SECS),
    )
    .await
}

async fn run_job_command_with_timeout(
    config: &Config,
    _security: &SecurityPolicy,
    job: &CronJob,
    timeout: Duration,
) -> (bool, String) {
    let child = match Command::new("sh")
        .arg("-lc")
        .arg(&job.command)
        .current_dir(&config.workspace_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return (false, format!("spawn error: {e}")),
    };

    match time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!(
                "status={}\nstdout:\n{}\nstderr:\n{}",
                output.status,
                stdout.trim(),
                stderr.trim()
            );
            (output.status.success(), combined)
        }
        Ok(Err(e)) => (false, format!("spawn error: {e}")),
        Err(_) => (
            false,
            format!("job timed out after {}s", timeout.as_secs()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::security::SecurityPolicy;
    use tempfile::TempDir;

    fn test_config(tmp: &TempDir) -> Config {
        let config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        std::fs::create_dir_all(&config.workspace_dir).unwrap();
        config
    }

    fn test_job(command: &str) -> CronJob {
        CronJob {
            id: "test-job".into(),
            expression: "* * * * *".into(),
            command: command.into(),
            next_run: Utc::now(),
            last_run: None,
            last_status: None,
            paused: false,
            one_shot: false,
        }
    }

    #[tokio::test]
    async fn run_job_command_success() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = test_job("echo scheduler-ok");
        let security = SecurityPolicy::from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job, None).await;
        assert!(success);
        assert!(output.contains("scheduler-ok"));
        assert!(output.contains("status=exit status: 0"));
    }

    #[tokio::test]
    async fn run_job_command_failure() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = test_job("ls definitely_missing_file_for_scheduler_test");
        let security = SecurityPolicy::from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job, None).await;
        assert!(!success);
        assert!(output.contains("definitely_missing_file_for_scheduler_test"));
        assert!(output.contains("status=exit status:"));
    }

    #[tokio::test]
    async fn run_job_command_blocks_disallowed_command() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.autonomy.allowed_commands = vec!["echo".into()];
        let job = test_job("curl https://evil.example");
        let security = SecurityPolicy::from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job, None).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.contains("command not allowed"));
    }

    #[tokio::test]
    async fn run_job_command_blocks_forbidden_path_argument() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.autonomy.allowed_commands = vec!["cat".into()];
        let job = test_job("cat /etc/passwd");
        let security = SecurityPolicy::from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job, None).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.contains("forbidden path argument"));
        assert!(output.contains("/etc/passwd"));
    }

    #[tokio::test]
    async fn run_job_command_blocks_readonly_mode() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.autonomy.level = crate::security::AutonomyLevel::ReadOnly;
        let job = test_job("echo should-not-run");
        let security = SecurityPolicy::from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job, None).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.contains("read-only"));
    }

    #[tokio::test]
    async fn run_job_command_blocks_rate_limited() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.autonomy.max_actions_per_hour = 0;
        let job = test_job("echo should-not-run");
        let security = SecurityPolicy::from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job, None).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.contains("rate limit exceeded"));
    }

    #[tokio::test]
    async fn execute_job_with_retry_recovers_after_first_failure() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.reliability.scheduler_retries = 1;
        config.reliability.provider_backoff_ms = 1;
        config.autonomy.allowed_commands = vec!["sh".into()];
        let security = SecurityPolicy::from_config(&config.autonomy, &config.workspace_dir);

        std::fs::write(
            config.workspace_dir.join("retry-once.sh"),
            "#!/bin/sh\nif [ -f retry-ok.flag ]; then\n  echo recovered\n  exit 0\nfi\ntouch retry-ok.flag\nexit 1\n",
        )
        .unwrap();
        let job = test_job("sh ./retry-once.sh");

        let (success, output) = execute_job_with_retry(&config, &security, &job, None).await;
        assert!(success);
        assert!(output.contains("recovered"));
    }

    #[tokio::test]
    async fn execute_job_with_retry_exhausts_attempts() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.reliability.scheduler_retries = 1;
        config.reliability.provider_backoff_ms = 1;
        let security = SecurityPolicy::from_config(&config.autonomy, &config.workspace_dir);

        let job = test_job("ls always_missing_for_retry_test");

        let (success, output) = execute_job_with_retry(&config, &security, &job, None).await;
        assert!(!success);
        assert!(output.contains("always_missing_for_retry_test"));
    }

    // ── Agent cron integration tests ──

    #[tokio::test]
    async fn agent_job_fails_without_bus() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = test_job("agent:mybot:hello");
        let security = SecurityPolicy::from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job, None).await;
        assert!(!success);
        assert!(output.contains("agent command requires agent bus"));
    }

    #[tokio::test]
    async fn agent_job_fails_unregistered_agent() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = test_job("agent:nonexistent:ping");
        let security = SecurityPolicy::from_config(&config.autonomy, &config.workspace_dir);
        let bus = AgentBus::new();

        let (success, output) = run_job_command(&config, &security, &job, Some(&bus)).await;
        assert!(!success);
        assert!(output.contains("not registered"));
    }

    #[tokio::test]
    async fn agent_job_empty_name_fails() {
        let bus = AgentBus::new();
        let (success, output) = run_agent_job(&bus, "agent:").await;
        assert!(!success);
        assert!(output.contains("missing agent name"));
    }

    #[tokio::test]
    async fn agent_job_parses_name_only() {
        let bus = AgentBus::new();
        // No agent registered, so it should fail with "not registered"
        let (success, output) = run_agent_job(&bus, "agent:mybot").await;
        assert!(!success);
        assert!(output.contains("'mybot' is not registered"));
    }

    #[tokio::test]
    async fn agent_job_parses_name_and_message() {
        let bus = AgentBus::new();
        let (success, output) = run_agent_job(&bus, "agent:helper:do something").await;
        assert!(!success);
        assert!(output.contains("'helper' is not registered"));
    }

    #[tokio::test]
    async fn agent_job_delegates_to_registered_agent() {
        let bus = Arc::new(AgentBus::new());
        let mut rx = bus.register("testbot", 16).await;

        // Spawn a fake agent that responds
        let bus_clone = bus.clone();
        tokio::spawn(async move {
            if let Some(mut msg) = rx.recv().await {
                assert_eq!(msg.from, "cron");
                assert_eq!(msg.payload, "do-work");
                if let Some(tx) = msg.response_tx.take() {
                    let _ = tx.send("work-done".to_string());
                }
            }
        });

        let (success, output) = run_agent_job(&bus_clone, "agent:testbot:do-work").await;
        assert!(success);
        assert_eq!(output, "work-done");
    }

    #[tokio::test]
    async fn agent_job_delegates_empty_message_when_no_payload() {
        let bus = Arc::new(AgentBus::new());
        let mut rx = bus.register("bot2", 16).await;

        tokio::spawn(async move {
            if let Some(mut msg) = rx.recv().await {
                assert_eq!(msg.payload, "");
                if let Some(tx) = msg.response_tx.take() {
                    let _ = tx.send("ack".to_string());
                }
            }
        });

        let (success, output) = run_agent_job(&bus, "agent:bot2").await;
        assert!(success);
        assert_eq!(output, "ack");
    }
}
