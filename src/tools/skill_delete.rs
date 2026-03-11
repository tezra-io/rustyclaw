use super::skill_create::is_valid_skill_name;
use super::traits::{Tool, ToolResult};
use crate::security::policy::ToolOperation;
use crate::security::SecurityPolicy;
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

/// Tool that lets the agent remove an existing skill directory.
///
/// High-risk operation: blocked at `ReadOnly` autonomy level.
/// Only operates within the workspace skills directory (path validation
/// ensures the target is under `<workspace>/skills/`).
pub struct SkillDeleteTool {
    workspace_dir: PathBuf,
    security: Arc<SecurityPolicy>,
}

impl SkillDeleteTool {
    pub fn new(workspace_dir: PathBuf, security: Arc<SecurityPolicy>) -> Self {
        Self {
            workspace_dir,
            security,
        }
    }
}

#[async_trait]
impl Tool for SkillDeleteTool {
    fn name(&self) -> &str {
        "skill_delete"
    }

    fn description(&self) -> &str {
        "Delete an existing skill and its entire directory. \
         This is a destructive operation that cannot be undone."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the skill to delete"
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        // Permission check
        if let Err(error) = self
            .security
            .enforce_tool_operation(ToolOperation::Act, "skill_delete")
        {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(error),
            });
        }

        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'name' parameter"))?;

        // 1. Validate name format
        if !is_valid_skill_name(name) {
            return Ok(ToolResult {
                success: false,
                output: "Skill name must be 1-64 chars, lowercase alphanumeric with hyphens, \
                         no leading/trailing/double hyphens."
                    .into(),
                error: None,
            });
        }

        // 2. Resolve and validate path is under workspace/skills/
        let skills_dir = self.workspace_dir.join("skills");
        let skill_dir = skills_dir.join(name);

        // Canonicalize to prevent path traversal via symlinks
        // (skill_dir may not exist yet, so canonicalize the parent)
        if skill_dir.exists() {
            let canonical = skill_dir.canonicalize()?;
            let canonical_skills = if skills_dir.exists() {
                skills_dir.canonicalize()?
            } else {
                // Skills dir doesn't exist — skill can't be under it
                return Ok(ToolResult {
                    success: false,
                    output: format!("Skill '{name}' not found at {}.", skill_dir.display()),
                    error: None,
                });
            };
            if !canonical.starts_with(&canonical_skills) {
                return Ok(ToolResult {
                    success: false,
                    output:
                        "Path validation failed: target is not within workspace skills directory."
                            .into(),
                    error: None,
                });
            }
        }

        // 3. Verify skill exists
        if !skill_dir.exists() {
            return Ok(ToolResult {
                success: false,
                output: format!("Skill '{name}' not found at {}.", skill_dir.display()),
                error: None,
            });
        }

        // 4. Remove entire skill directory
        std::fs::remove_dir_all(&skill_dir)?;

        Ok(ToolResult {
            success: true,
            output: format!("Skill '{name}' deleted from {}.", skill_dir.display()),
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::{AutonomyLevel, SecurityPolicy};
    use tempfile::TempDir;

    fn setup() -> (TempDir, SkillDeleteTool) {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().to_path_buf();
        let security = Arc::new(SecurityPolicy::default());
        let tool = SkillDeleteTool::new(workspace, security);
        (tmp, tool)
    }

    fn create_skill(tmp: &TempDir, name: &str) {
        let dir = tmp.path().join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: test\ndescription: test\n---\n\nTest.\n",
        )
        .unwrap();
    }

    // ----------------------------------------------------------------
    // Tool metadata
    // ----------------------------------------------------------------

    #[test]
    fn name_and_schema() {
        let (_tmp, tool) = setup();
        assert_eq!(tool.name(), "skill_delete");
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["name"].is_object());
        assert_eq!(schema["required"], json!(["name"]));
    }

    // ----------------------------------------------------------------
    // Happy path
    // ----------------------------------------------------------------

    #[tokio::test]
    async fn deletes_skill_successfully() {
        let (tmp, tool) = setup();
        create_skill(&tmp, "doomed-skill");

        let skill_dir = tmp.path().join("skills").join("doomed-skill");
        assert!(skill_dir.exists());

        let result = tool.execute(json!({"name": "doomed-skill"})).await.unwrap();

        assert!(result.success, "Expected success: {:?}", result);
        assert!(result.output.contains("doomed-skill"));
        assert!(result.output.contains("deleted"));
        assert!(!skill_dir.exists());
    }

    // ----------------------------------------------------------------
    // Skill not found
    // ----------------------------------------------------------------

    #[tokio::test]
    async fn rejects_nonexistent_skill() {
        let (_tmp, tool) = setup();
        let result = tool
            .execute(json!({"name": "no-such-skill"}))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.output.contains("not found"));
    }

    // ----------------------------------------------------------------
    // Name validation
    // ----------------------------------------------------------------

    #[tokio::test]
    async fn rejects_bad_name() {
        let (_tmp, tool) = setup();
        let result = tool.execute(json!({"name": "../escape"})).await.unwrap();

        assert!(!result.success);
        assert!(result.output.contains("Skill name must be"));
    }

    // ----------------------------------------------------------------
    // Path traversal prevention
    // ----------------------------------------------------------------

    #[tokio::test]
    async fn prevents_symlink_escape() {
        let (tmp, tool) = setup();

        // Create a target directory outside workspace
        let outside = TempDir::new().unwrap();
        let outside_target = outside.path().join("precious");
        std::fs::create_dir_all(&outside_target).unwrap();
        std::fs::write(outside_target.join("SKILL.md"), "data").unwrap();

        // Create skills dir and a symlink pointing outside
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_target, skills_dir.join("escape-link")).unwrap();

        #[cfg(unix)]
        {
            let result = tool.execute(json!({"name": "escape-link"})).await.unwrap();

            assert!(!result.success);
            assert!(result.output.contains("Path validation failed"));

            // Outside target must still exist
            assert!(outside_target.exists());
        }
    }

    // ----------------------------------------------------------------
    // Security policy enforcement
    // ----------------------------------------------------------------

    #[tokio::test]
    async fn blocked_in_readonly_mode() {
        let tmp = TempDir::new().unwrap();
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::ReadOnly,
            ..SecurityPolicy::default()
        });
        let tool = SkillDeleteTool::new(tmp.path().to_path_buf(), security);

        create_skill_at(tmp.path(), "blocked");

        let result = tool.execute(json!({"name": "blocked"})).await.unwrap();

        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("read-only mode"));

        // Skill must still exist
        assert!(tmp.path().join("skills").join("blocked").exists());
    }

    // ----------------------------------------------------------------
    // Missing parameters
    // ----------------------------------------------------------------

    #[tokio::test]
    async fn rejects_missing_name() {
        let (_tmp, tool) = setup();
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }

    // ----------------------------------------------------------------
    // Helpers
    // ----------------------------------------------------------------

    fn create_skill_at(workspace: &std::path::Path, name: &str) {
        let dir = workspace.join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: test\ndescription: test\n---\n\nTest.\n",
        )
        .unwrap();
    }
}
