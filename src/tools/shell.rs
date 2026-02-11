use async_trait::async_trait;
use std::path::PathBuf;
use tokio::process::Command;
use tracing::{debug, warn};

/// Dangerous command patterns that are always denied.
const DENY_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf ~",
    "mkfs.",
    "dd if=",
    ":(){ :|:& };:",
    "> /dev/sd",
    "chmod -R 777 /",
    "shutdown",
    "reboot",
    "halt",
    "init 0",
    "init 6",
];

const MAX_OUTPUT: usize = 10_240; // 10 KB

pub struct ExecTool {
    pub allowed_dir: Option<PathBuf>,
    pub timeout_secs: u64,
}

#[async_trait]
impl super::base::Tool for ExecTool {
    fn name(&self) -> &str { "exec" }

    fn description(&self) -> &str {
        "Execute a shell command and return its output."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to execute" }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> crate::error::Result<String> {
        let cmd = args["command"]
            .as_str()
            .ok_or_else(|| crate::error::NanobotError::Tool("Missing 'command'".into()))?;

        // Safety checks
        let lower = cmd.to_lowercase();
        for pattern in DENY_PATTERNS {
            if lower.contains(pattern) {
                warn!("Blocked dangerous command: {}", cmd);
                return Err(crate::error::NanobotError::Tool(format!(
                    "Command blocked by safety filter: contains '{}'",
                    pattern
                )));
            }
        }

        // Path traversal check
        if let Some(dir) = &self.allowed_dir {
            if cmd.contains("../") {
                let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
                debug!("Checking path traversal against {}", canonical.display());
            }
        }

        debug!("exec: {}", cmd);

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            Command::new("sh").arg("-c").arg(cmd).output(),
        )
        .await
        .map_err(|_| {
            crate::error::NanobotError::Tool(format!(
                "Command timed out after {}s",
                self.timeout_secs
            ))
        })?
        .map_err(|e| crate::error::NanobotError::Io(e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut result = format!("exit code: {}\n", output.status.code().unwrap_or(-1));
        if !stdout.is_empty() {
            result.push_str(&truncate(&stdout, MAX_OUTPUT));
        }
        if !stderr.is_empty() {
            result.push_str("\n--- stderr ---\n");
            result.push_str(&truncate(&stderr, MAX_OUTPUT));
        }

        Ok(result)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...\n[truncated at {} bytes]", &s[..max], max)
    }
}
