pub mod elixir;

use crate::config::Config;
use anyhow::Result;
use chrono::Utc;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::Duration;

const STATUS_FLUSH_SECONDS: u64 = 5;
const ELIXIR_HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(15);
const ELIXIR_RESTART_BACKOFF: Duration = Duration::from_secs(10);
const ELIXIR_MAX_RESTART_BACKOFF: Duration = Duration::from_secs(120);

pub async fn run(config: Config, host: String, port: u16, enable_elixir: bool) -> Result<()> {
    let initial_backoff = config.reliability.channel_initial_backoff_secs.max(1);
    let max_backoff = config
        .reliability
        .channel_max_backoff_secs
        .max(initial_backoff);

    crate::health::mark_component_ok("daemon");

    if config.heartbeat.enabled {
        let _ =
            crate::heartbeat::engine::HeartbeatEngine::ensure_heartbeat_file(&config.workspace_dir)
                .await;
    }

    let mut handles: Vec<JoinHandle<()>> = vec![spawn_state_writer(config.clone())];

    {
        let gateway_cfg = config.clone();
        let gateway_host = host.clone();
        handles.push(spawn_component_supervisor(
            "gateway",
            initial_backoff,
            max_backoff,
            move || {
                let cfg = gateway_cfg.clone();
                let host = gateway_host.clone();
                async move { crate::gateway::run_gateway(&host, port, cfg).await }
            },
        ));
    }

    {
        if has_supervised_channels(&config) {
            let channels_cfg = config.clone();
            handles.push(spawn_component_supervisor(
                "channels",
                initial_backoff,
                max_backoff,
                move || {
                    let cfg = channels_cfg.clone();
                    async move { crate::channels::start_channels(cfg).await }
                },
            ));
        } else {
            crate::health::mark_component_ok("channels");
            tracing::warn!(
                config_path = %config.config_path.display(),
                "No channels configured — the daemon will run without messaging. \
                 Run `rustyclaw onboard` or add a channel to config.toml."
            );
        }
    }

    if config.heartbeat.enabled {
        let heartbeat_cfg = config.clone();
        handles.push(spawn_component_supervisor(
            "heartbeat",
            initial_backoff,
            max_backoff,
            move || {
                let cfg = heartbeat_cfg.clone();
                async move { Box::pin(run_heartbeat_worker(cfg)).await }
            },
        ));
    }

    if config.cron.enabled {
        let scheduler_cfg = config.clone();
        handles.push(spawn_component_supervisor(
            "scheduler",
            initial_backoff,
            max_backoff,
            move || {
                let cfg = scheduler_cfg.clone();
                async move { crate::cron::scheduler::run(cfg).await }
            },
        ));
    } else {
        crate::health::mark_component_ok("scheduler");
        tracing::info!("Cron disabled; scheduler supervisor not started");
    }

    // Elixir orchestrator subprocess
    let orchestrator = Arc::new(Mutex::new(elixir::ElixirOrchestrator::new(port)));
    if enable_elixir {
        handles.push(spawn_elixir_supervisor(orchestrator.clone()));
    } else {
        // Don't mark elixir as "ok" when disabled — it's not running.
        // Just log the status and set the disabled flag on the orchestrator.
        tracing::info!("Elixir orchestrator disabled (--no-elixir); running in degraded mode");
        orchestrator.lock().await.set_disabled();
    }

    let elixir_status = if enable_elixir {
        "enabled"
    } else {
        "disabled (--no-elixir)"
    };

    println!();
    println!("  ┌──────────────────────────────────────────────────┐");
    println!(
        "  │          🧠 RustyClaw Daemon — v{}{}│",
        env!("CARGO_PKG_VERSION"),
        " ".repeat(24_usize.saturating_sub(env!("CARGO_PKG_VERSION").len()))
    );
    println!("  ├──────────────────────────────────────────────────┤");
    println!("  │  Services                                       │");
    println!(
        "  │    Rust Core:    ✅ running (PID {:<16})│",
        std::process::id()
    );
    println!("  │    Gateway:      http://{host}:{port:<24}│");
    println!("  │    Elixir OTP:   {:<31}│", elixir_status);
    println!(
        "  │    BTW Bridge:   {:<31}│",
        if enable_elixir {
            "localhost:4001-4002"
        } else {
            "n/a (elixir disabled)"
        }
    );
    println!("  ├──────────────────────────────────────────────────┤");
    println!("  │  Components                                     │");
    print_component_line("Gateway", true);
    print_component_line("Channels", has_supervised_channels(&config));
    print_component_line("Heartbeat", config.heartbeat.enabled);
    print_component_line("Scheduler", config.cron.enabled);
    print_component_line("Elixir", enable_elixir);
    println!("  ├──────────────────────────────────────────────────┤");
    println!("  │  Ctrl+C to stop                                 │");
    println!("  └──────────────────────────────────────────────────┘");
    println!();

    tokio::signal::ctrl_c().await?;
    println!();
    println!("  Shutting down...");
    crate::health::mark_component_error("daemon", "shutdown requested");

    // Stop Elixir orchestrator gracefully first
    {
        let mut orch = orchestrator.lock().await;
        orch.stop().await;
    }

    for handle in &handles {
        handle.abort();
    }
    for handle in handles {
        let _ = handle.await;
    }

    Ok(())
}

pub fn state_file_path(config: &Config) -> PathBuf {
    config
        .config_path
        .parent()
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join("daemon_state.json")
}

fn spawn_state_writer(config: Config) -> JoinHandle<()> {
    tokio::spawn(async move {
        let path = state_file_path(&config);
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        let mut interval = tokio::time::interval(Duration::from_secs(STATUS_FLUSH_SECONDS));
        loop {
            interval.tick().await;
            let mut json = crate::health::snapshot_json();
            if let Some(obj) = json.as_object_mut() {
                obj.insert(
                    "written_at".into(),
                    serde_json::json!(Utc::now().to_rfc3339()),
                );
            }
            let data = serde_json::to_vec_pretty(&json).unwrap_or_else(|_| b"{}".to_vec());
            let _ = tokio::fs::write(&path, data).await;
        }
    })
}

fn spawn_component_supervisor<F, Fut>(
    name: &'static str,
    initial_backoff_secs: u64,
    max_backoff_secs: u64,
    mut run_component: F,
) -> JoinHandle<()>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    tokio::spawn(async move {
        let mut backoff = initial_backoff_secs.max(1);
        let max_backoff = max_backoff_secs.max(backoff);

        loop {
            crate::health::mark_component_ok(name);
            match run_component().await {
                Ok(()) => {
                    crate::health::mark_component_error(name, "component exited unexpectedly");
                    tracing::warn!("Daemon component '{name}' exited unexpectedly");
                    // Clean exit — reset backoff since the component ran successfully
                    backoff = initial_backoff_secs.max(1);
                }
                Err(e) => {
                    crate::health::mark_component_error(name, e.to_string());
                    tracing::error!("Daemon component '{name}' failed: {e}");
                }
            }

            crate::health::bump_component_restart(name);
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            // Double backoff AFTER sleeping so first error uses initial_backoff
            backoff = backoff.saturating_mul(2).min(max_backoff);
        }
    })
}

async fn run_heartbeat_worker(config: Config) -> Result<()> {
    let observer: std::sync::Arc<dyn crate::observability::Observer> =
        std::sync::Arc::from(crate::observability::create_observer(&config.observability));
    let engine = crate::heartbeat::engine::HeartbeatEngine::new(
        config.heartbeat.clone(),
        config.workspace_dir.clone(),
        observer,
    );
    let delivery = heartbeat_delivery_target(&config)?;

    let interval_mins = config.heartbeat.interval_minutes.max(5);
    let mut interval = tokio::time::interval(Duration::from_secs(u64::from(interval_mins) * 60));

    loop {
        interval.tick().await;

        let file_tasks = engine.collect_tasks().await?;
        let tasks = heartbeat_tasks_for_tick(file_tasks, config.heartbeat.message.as_deref());
        if tasks.is_empty() {
            continue;
        }

        for task in tasks {
            let prompt = format!("[Heartbeat Task] {task}");
            let temp = config.default_temperature;
            match crate::agent::run(
                config.clone(),
                Some(prompt),
                None,
                None,
                temp,
                vec![],
                false,
            )
            .await
            {
                Ok(output) => {
                    crate::health::mark_component_ok("heartbeat");
                    let announcement = if output.trim().is_empty() {
                        "heartbeat task executed".to_string()
                    } else {
                        output
                    };
                    if let Some((channel, target)) = &delivery {
                        if let Err(e) = crate::cron::scheduler::deliver_announcement(
                            &config,
                            channel,
                            target,
                            &announcement,
                        )
                        .await
                        {
                            crate::health::mark_component_error(
                                "heartbeat",
                                format!("delivery failed: {e}"),
                            );
                            tracing::warn!("Heartbeat delivery failed: {e}");
                        }
                    }
                }
                Err(e) => {
                    crate::health::mark_component_error("heartbeat", e.to_string());
                    tracing::warn!("Heartbeat task failed: {e}");
                }
            }
        }
    }
}

fn heartbeat_tasks_for_tick(
    file_tasks: Vec<String>,
    fallback_message: Option<&str>,
) -> Vec<String> {
    if !file_tasks.is_empty() {
        return file_tasks;
    }

    fallback_message
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(|message| vec![message.to_string()])
        .unwrap_or_default()
}

fn heartbeat_delivery_target(config: &Config) -> Result<Option<(String, String)>> {
    let channel = config
        .heartbeat
        .target
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let target = config
        .heartbeat
        .to
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match (channel, target) {
        (None, None) => Ok(None),
        (Some(_), None) => anyhow::bail!("heartbeat.to is required when heartbeat.target is set"),
        (None, Some(_)) => anyhow::bail!("heartbeat.target is required when heartbeat.to is set"),
        (Some(channel), Some(target)) => {
            validate_heartbeat_channel_config(config, channel)?;
            Ok(Some((channel.to_string(), target.to_string())))
        }
    }
}

fn validate_heartbeat_channel_config(config: &Config, channel: &str) -> Result<()> {
    match channel.to_ascii_lowercase().as_str() {
        "telegram" => {
            if config.channels_config.telegram.is_none() {
                anyhow::bail!(
                    "heartbeat.target is set to telegram but channels_config.telegram is not configured"
                );
            }
        }
        "discord" => {
            if config.channels_config.discord.is_none() {
                anyhow::bail!(
                    "heartbeat.target is set to discord but channels_config.discord is not configured"
                );
            }
        }
        "slack" => {
            if config.channels_config.slack.is_none() {
                anyhow::bail!(
                    "heartbeat.target is set to slack but channels_config.slack is not configured"
                );
            }
        }
        "mattermost" => {
            if config.channels_config.mattermost.is_none() {
                anyhow::bail!(
                    "heartbeat.target is set to mattermost but channels_config.mattermost is not configured"
                );
            }
        }
        other => anyhow::bail!("unsupported heartbeat.target channel: {other}"),
    }

    Ok(())
}

fn print_component_line(name: &str, active: bool) {
    let status = if active { "✅ active" } else { "⬚ inactive" };
    println!("  │    {:<14}{:<32}│", name, status);
}

fn spawn_elixir_supervisor(orchestrator: Arc<Mutex<elixir::ElixirOrchestrator>>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff = ELIXIR_RESTART_BACKOFF;

        loop {
            let started_at;
            {
                let mut orch = orchestrator.lock().await;
                match orch.start().await {
                    Ok(instant) => {
                        crate::health::mark_component_ok("elixir");
                        tracing::info!("Elixir orchestrator started");
                        backoff = ELIXIR_RESTART_BACKOFF;
                        started_at = instant;
                    }
                    Err(e) => {
                        let msg = format!("Elixir orchestrator failed to start: {e}");
                        crate::health::mark_component_error("elixir", &msg);
                        tracing::warn!("{msg}");
                        tracing::warn!(
                            "Daemon running in degraded mode (no orchestrator). \
                             Will retry in {}s",
                            backoff.as_secs()
                        );
                        drop(orch);
                        tokio::time::sleep(backoff).await;
                        backoff = backoff.saturating_mul(2).min(ELIXIR_MAX_RESTART_BACKOFF);
                        continue;
                    }
                }
            }

            // Health-check loop while running.
            // Skip health-check failures during ELIXIR_STARTUP_GRACE period
            // after start to allow the orchestrator time to initialize.
            let startup_grace = elixir::startup_grace_duration();
            let mut health_interval = tokio::time::interval(ELIXIR_HEALTH_CHECK_INTERVAL);
            loop {
                health_interval.tick().await;
                let mut orch = orchestrator.lock().await;
                if !orch.is_alive() {
                    crate::health::mark_component_error(
                        "elixir",
                        "orchestrator process exited unexpectedly",
                    );
                    crate::health::bump_component_restart("elixir");
                    tracing::warn!(
                        "Elixir orchestrator exited unexpectedly; restarting in {}s",
                        backoff.as_secs()
                    );
                    break;
                }
                if orch.health_check().await {
                    crate::health::mark_component_ok("elixir");
                } else {
                    let within_grace = started_at.elapsed() < startup_grace;
                    if within_grace {
                        tracing::debug!(
                            "Elixir health check failed during startup grace period; ignoring"
                        );
                    } else {
                        crate::health::mark_component_error(
                            "elixir",
                            "orchestrator health check failed",
                        );
                    }
                }
            }

            tokio::time::sleep(backoff).await;
            backoff = backoff.saturating_mul(2).min(ELIXIR_MAX_RESTART_BACKOFF);
        }
    })
}

fn has_supervised_channels(config: &Config) -> bool {
    config
        .channels_config
        .channels_except_webhook()
        .iter()
        .any(|(_, ok)| *ok)
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn state_file_path_uses_config_directory() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let path = state_file_path(&config);
        assert_eq!(path, tmp.path().join("daemon_state.json"));
    }

    #[tokio::test]
    async fn supervisor_marks_error_and_restart_on_failure() {
        let handle = spawn_component_supervisor("daemon-test-fail", 1, 1, || async {
            anyhow::bail!("boom")
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        let _ = handle.await;

        let snapshot = crate::health::snapshot_json();
        let component = &snapshot["components"]["daemon-test-fail"];
        assert_eq!(component["status"], "error");
        assert!(component["restart_count"].as_u64().unwrap_or(0) >= 1);
        assert!(component["last_error"]
            .as_str()
            .unwrap_or("")
            .contains("boom"));
    }

    #[tokio::test]
    async fn supervisor_marks_unexpected_exit_as_error() {
        let handle = spawn_component_supervisor("daemon-test-exit", 1, 1, || async { Ok(()) });

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        let _ = handle.await;

        let snapshot = crate::health::snapshot_json();
        let component = &snapshot["components"]["daemon-test-exit"];
        assert_eq!(component["status"], "error");
        assert!(component["restart_count"].as_u64().unwrap_or(0) >= 1);
        assert!(component["last_error"]
            .as_str()
            .unwrap_or("")
            .contains("component exited unexpectedly"));
    }

    #[test]
    fn detects_no_supervised_channels() {
        let config = Config::default();
        assert!(!has_supervised_channels(&config));
    }

    #[test]
    fn detects_supervised_channels_present() {
        let mut config = Config::default();
        config.channels_config.telegram = Some(crate::config::TelegramConfig {
            bot_token: "token".into(),
            allowed_users: vec![],
            stream_mode: crate::config::StreamMode::default(),
            draft_update_interval_ms: 1000,
            interrupt_on_new_message: false,
            mention_only: false,
        });
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn detects_dingtalk_as_supervised_channel() {
        let mut config = Config::default();
        config.channels_config.dingtalk = Some(crate::config::schema::DingTalkConfig {
            client_id: "client_id".into(),
            client_secret: "client_secret".into(),
            allowed_users: vec!["*".into()],
        });
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn detects_mattermost_as_supervised_channel() {
        let mut config = Config::default();
        config.channels_config.mattermost = Some(crate::config::schema::MattermostConfig {
            url: "https://mattermost.example.com".into(),
            bot_token: "token".into(),
            channel_id: Some("channel-id".into()),
            allowed_users: vec!["*".into()],
            thread_replies: Some(true),
            mention_only: Some(false),
        });
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn detects_qq_as_supervised_channel() {
        let mut config = Config::default();
        config.channels_config.qq = Some(crate::config::schema::QQConfig {
            app_id: "app-id".into(),
            app_secret: "app-secret".into(),
            allowed_users: vec!["*".into()],
        });
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn detects_nextcloud_talk_as_supervised_channel() {
        let mut config = Config::default();
        config.channels_config.nextcloud_talk = Some(crate::config::schema::NextcloudTalkConfig {
            base_url: "https://cloud.example.com".into(),
            app_token: "app-token".into(),
            webhook_secret: None,
            allowed_users: vec!["*".into()],
        });
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn heartbeat_tasks_use_file_tasks_when_available() {
        let tasks =
            heartbeat_tasks_for_tick(vec!["From file".to_string()], Some("Fallback from config"));
        assert_eq!(tasks, vec!["From file".to_string()]);
    }

    #[test]
    fn heartbeat_tasks_fall_back_to_config_message() {
        let tasks = heartbeat_tasks_for_tick(vec![], Some("  check london time  "));
        assert_eq!(tasks, vec!["check london time".to_string()]);
    }

    #[test]
    fn heartbeat_tasks_ignore_empty_fallback_message() {
        let tasks = heartbeat_tasks_for_tick(vec![], Some("   "));
        assert!(tasks.is_empty());
    }

    #[test]
    fn heartbeat_delivery_target_none_when_unset() {
        let config = Config::default();
        let target = heartbeat_delivery_target(&config).unwrap();
        assert!(target.is_none());
    }

    #[test]
    fn heartbeat_delivery_target_requires_to_field() {
        let mut config = Config::default();
        config.heartbeat.target = Some("telegram".into());
        let err = heartbeat_delivery_target(&config).unwrap_err();
        assert!(err
            .to_string()
            .contains("heartbeat.to is required when heartbeat.target is set"));
    }

    #[test]
    fn heartbeat_delivery_target_requires_target_field() {
        let mut config = Config::default();
        config.heartbeat.to = Some("123456".into());
        let err = heartbeat_delivery_target(&config).unwrap_err();
        assert!(err
            .to_string()
            .contains("heartbeat.target is required when heartbeat.to is set"));
    }

    #[test]
    fn heartbeat_delivery_target_rejects_unsupported_channel() {
        let mut config = Config::default();
        config.heartbeat.target = Some("email".into());
        config.heartbeat.to = Some("ops@example.com".into());
        let err = heartbeat_delivery_target(&config).unwrap_err();
        assert!(err
            .to_string()
            .contains("unsupported heartbeat.target channel"));
    }

    #[test]
    fn heartbeat_delivery_target_requires_channel_configuration() {
        let mut config = Config::default();
        config.heartbeat.target = Some("telegram".into());
        config.heartbeat.to = Some("123456".into());
        let err = heartbeat_delivery_target(&config).unwrap_err();
        assert!(err
            .to_string()
            .contains("channels_config.telegram is not configured"));
    }

    #[test]
    fn heartbeat_delivery_target_accepts_telegram_configuration() {
        let mut config = Config::default();
        config.heartbeat.target = Some("telegram".into());
        config.heartbeat.to = Some("123456".into());
        config.channels_config.telegram = Some(crate::config::TelegramConfig {
            bot_token: "bot-token".into(),
            allowed_users: vec![],
            stream_mode: crate::config::StreamMode::default(),
            draft_update_interval_ms: 1000,
            interrupt_on_new_message: false,
            mention_only: false,
        });

        let target = heartbeat_delivery_target(&config).unwrap();
        assert_eq!(target, Some(("telegram".to_string(), "123456".to_string())));
    }
}
