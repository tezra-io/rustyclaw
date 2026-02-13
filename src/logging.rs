use std::path::PathBuf;
use tracing_appender::rolling;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Initialize the tracing subscriber with both console and file output.
///
/// - Console: human-readable, filtered by RUST_LOG or default `rustyclaw=info`
/// - File: daily-rotating log at `~/.rustyclaw/logs/rustyclaw.log`
pub fn init_logging() {
    let log_dir = log_base_dir();
    std::fs::create_dir_all(&log_dir).ok();

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "rustyclaw=info".into());

    let console_layer = fmt::layer().with_target(false).compact();

    let file_appender = rolling::daily(&log_dir, "rustyclaw.log");
    let file_layer = fmt::layer()
        .with_writer(file_appender)
        .with_ansi(false)
        .with_target(true);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .init();
}

/// Get the base directory for logs (~/.rustyclaw/logs/).
pub fn log_base_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".rustyclaw")
        .join("logs")
}

/// Get the log directory for a specific agent.
pub fn agent_log_dir(agent_name: &str) -> PathBuf {
    log_base_dir().join(agent_name)
}

/// Read recent log lines for a specific agent from the global log.
/// Filters lines containing the agent name.
pub fn read_agent_logs(
    agent_name: &str,
    max_lines: usize,
    level_filter: Option<&str>,
) -> Vec<String> {
    let log_dir = log_base_dir();
    let mut lines = Vec::new();

    // Read from global log files (most recent first)
    let mut log_files: Vec<PathBuf> = std::fs::read_dir(&log_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with("rustyclaw.log"))
                .unwrap_or(false)
        })
        .collect();

    log_files.sort_by(|a, b| b.cmp(a)); // newest first

    for file in log_files {
        if let Ok(content) = std::fs::read_to_string(&file) {
            for line in content.lines().rev() {
                // Filter by agent name
                if !line.contains(agent_name) {
                    continue;
                }

                // Filter by level if specified
                if let Some(level) = level_filter {
                    let level_upper = level.to_uppercase();
                    if !line.contains(&level_upper) {
                        continue;
                    }
                }

                lines.push(line.to_string());
                if lines.len() >= max_lines {
                    break;
                }
            }

            if lines.len() >= max_lines {
                break;
            }
        }
    }

    lines.reverse(); // chronological order
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_base_dir_exists() {
        let dir = log_base_dir();
        assert!(dir.to_string_lossy().contains("logs"));
    }

    #[test]
    fn agent_log_dir_includes_name() {
        let dir = agent_log_dir("twitter");
        assert!(dir.to_string_lossy().contains("twitter"));
    }
}
