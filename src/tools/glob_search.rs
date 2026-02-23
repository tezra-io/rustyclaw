use super::traits::{Tool, ToolResult};
use crate::security::SecurityPolicy;
use async_trait::async_trait;
use serde_json::json;
use std::fmt::Write as FmtWrite;
use std::sync::Arc;

const MAX_RESULTS: usize = 1000;

/// Search for files matching a glob pattern within the workspace.
pub struct GlobSearchTool {
    security: Arc<SecurityPolicy>,
}

impl GlobSearchTool {
    pub fn new(security: Arc<SecurityPolicy>) -> Self {
        Self { security }
    }
}

#[async_trait]
impl Tool for GlobSearchTool {
    fn name(&self) -> &str {
        "glob_search"
    }

    fn description(&self) -> &str {
        "Search for files matching a glob pattern within the workspace. \
         Returns a sorted list of matching file paths relative to the workspace root. \
         Examples: '**/*.rs', 'src/**/mod.rs'."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match against workspace files (e.g. '**/*.rs', 'src/**/mod.rs')"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing required parameter: pattern".into()),
                })
            }
        };

        // 1. Rate limit check
        if self.security.is_rate_limited() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: too many actions in the last hour".into()),
            });
        }

        // 2. No absolute paths
        if pattern.starts_with('/') || pattern.starts_with('\\') {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Absolute paths are not allowed in glob patterns".into()),
            });
        }

        // 3. No path traversal
        if pattern.contains("../") || pattern.contains("..\\") || pattern == ".." {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Path traversal is not allowed in glob patterns".into()),
            });
        }

        // 4. Record action
        if !self.security.record_action() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: action budget exhausted".into()),
            });
        }

        let workspace = self.security.workspace_dir.clone();
        let pattern = pattern.to_string();

        // Run the glob on a blocking thread (filesystem I/O)
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(Vec<String>, usize)> {
            let full_pattern = workspace.join(&pattern).to_string_lossy().to_string();
            let workspace_canon = std::fs::canonicalize(&workspace)?;

            let entries = glob::glob(&full_pattern)
                .map_err(|e| anyhow::anyhow!("Invalid glob pattern '{pattern}': {e}"))?;

            let mut paths: Vec<String> = Vec::new();
            let mut total_matches: usize = 0;
            let mut truncated = false;

            for entry in entries {
                let entry_path = match entry {
                    Ok(p) => p,
                    Err(_) => continue, // skip unreadable entries
                };

                // Skip directories — files only
                if entry_path.is_dir() {
                    continue;
                }

                // Canonicalize to resolve symlinks
                let resolved = match std::fs::canonicalize(&entry_path) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // Security: silently filter paths that escape the workspace
                if !resolved.starts_with(&workspace_canon) {
                    continue;
                }

                total_matches += 1;

                if paths.len() >= MAX_RESULTS {
                    truncated = true;
                    continue; // count total but don't add to results
                }

                // Strip workspace prefix for relative path
                if let Ok(rel) = resolved.strip_prefix(&workspace_canon) {
                    paths.push(rel.to_string_lossy().to_string());
                }
            }

            paths.sort();
            let count = if truncated { total_matches } else { paths.len() };
            Ok((paths, count))
        })
        .await
        .map_err(|e| anyhow::anyhow!("Glob search task panicked: {e}"))?;

        match result {
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
            }),
            Ok((paths, total)) => {
                let mut output = paths.join("\n");

                if total > MAX_RESULTS {
                    let _ = write!(
                        output,
                        "\n\n[Results truncated: showing first {MAX_RESULTS} of more matches]"
                    );
                }

                let _ = write!(output, "\n\nTotal: {} files", paths.len());

                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::{AutonomyLevel, SecurityPolicy};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn make_security(tmp: &TempDir) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            workspace_dir: tmp.path().to_path_buf(),
            ..SecurityPolicy::default()
        })
    }

    #[test]
    fn glob_search_name_and_schema() {
        let tmp = TempDir::new().unwrap();
        let tool = GlobSearchTool::new(make_security(&tmp));
        assert_eq!(tool.name(), "glob_search");
        assert!(!tool.description().is_empty());
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["pattern"].is_object());
        assert!(schema["required"].as_array().unwrap().contains(&serde_json::json!("pattern")));
    }

    #[tokio::test]
    async fn glob_search_missing_pattern() {
        let tmp = TempDir::new().unwrap();
        let tool = GlobSearchTool::new(make_security(&tmp));
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn glob_search_absolute_path_blocked() {
        let tmp = TempDir::new().unwrap();
        let tool = GlobSearchTool::new(make_security(&tmp));
        let result = tool
            .execute(serde_json::json!({"pattern": "/etc/passwd"}))
            .await
            .unwrap();
        assert!(!result.success);
        let err = result.error.unwrap();
        assert!(err.contains("Absolute paths"), "got: {err}");
    }

    #[tokio::test]
    async fn glob_search_path_traversal_blocked() {
        let tmp = TempDir::new().unwrap();
        let tool = GlobSearchTool::new(make_security(&tmp));
        let result = tool
            .execute(serde_json::json!({"pattern": "../../../etc/passwd"}))
            .await
            .unwrap();
        assert!(!result.success);
        let err = result.error.unwrap();
        assert!(err.contains("traversal"), "got: {err}");
    }

    #[tokio::test]
    async fn glob_search_finds_files() {
        let tmp = TempDir::new().unwrap();
        // Create some test files
        std::fs::write(tmp.path().join("foo.txt"), "foo").unwrap();
        std::fs::write(tmp.path().join("bar.txt"), "bar").unwrap();
        std::fs::write(tmp.path().join("baz.rs"), "baz").unwrap();

        let tool = GlobSearchTool::new(make_security(&tmp));
        let result = tool
            .execute(serde_json::json!({"pattern": "*.txt"}))
            .await
            .unwrap();
        assert!(result.success, "error: {:?}", result.error);
        assert!(result.output.contains("foo.txt"), "got: {}", result.output);
        assert!(result.output.contains("bar.txt"), "got: {}", result.output);
        assert!(!result.output.contains("baz.rs"), "got: {}", result.output);
    }

    #[tokio::test]
    async fn glob_search_recursive_pattern() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("src");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(sub.join("lib.rs"), "// lib").unwrap();

        let tool = GlobSearchTool::new(make_security(&tmp));
        let result = tool
            .execute(serde_json::json!({"pattern": "**/*.rs"}))
            .await
            .unwrap();
        assert!(result.success, "error: {:?}", result.error);
        assert!(
            result.output.contains("main.rs"),
            "got: {}",
            result.output
        );
        assert!(result.output.contains("lib.rs"), "got: {}", result.output);
    }

    #[tokio::test]
    async fn glob_search_empty_workspace() {
        let tmp = TempDir::new().unwrap();
        let tool = GlobSearchTool::new(make_security(&tmp));
        let result = tool
            .execute(serde_json::json!({"pattern": "*.rs"}))
            .await
            .unwrap();
        assert!(result.success, "error: {:?}", result.error);
        assert!(result.output.contains("Total: 0 files"), "got: {}", result.output);
    }

    #[tokio::test]
    async fn glob_search_max_results_constant() {
        assert_eq!(MAX_RESULTS, 1000);
    }
}
