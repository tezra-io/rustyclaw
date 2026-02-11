use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tracing::debug;

/// Resolve and validate a path within an allowed directory.
fn resolve_path(path: &str, allowed_dir: Option<&Path>) -> crate::error::Result<PathBuf> {
    let expanded = shellexpand::tilde(path);
    let resolved = PathBuf::from(expanded.as_ref())
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(expanded.as_ref()));

    if let Some(dir) = allowed_dir {
        if !resolved.starts_with(dir) {
            return Err(crate::error::NanobotError::Tool(format!(
                "Path {} is outside allowed directory {}",
                resolved.display(),
                dir.display()
            )));
        }
    }

    Ok(resolved)
}

// --- ReadFile ---

pub struct ReadFileTool {
    pub allowed_dir: Option<PathBuf>,
}

#[async_trait]
impl super::base::Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path to read" }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> crate::error::Result<String> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| crate::error::NanobotError::Tool("Missing 'path'".into()))?;
        let path = resolve_path(path_str, self.allowed_dir.as_deref())?;
        debug!("read_file: {}", path.display());
        tokio::fs::read_to_string(&path)
            .await
            .map_err(crate::error::NanobotError::Io)
    }
}

// --- WriteFile ---

pub struct WriteFileTool {
    pub allowed_dir: Option<PathBuf>,
}

#[async_trait]
impl super::base::Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating parent directories as needed."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path to write" },
                "content": { "type": "string", "description": "Content to write" }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> crate::error::Result<String> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| crate::error::NanobotError::Tool("Missing 'path'".into()))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| crate::error::NanobotError::Tool("Missing 'content'".into()))?;
        let path = resolve_path(path_str, self.allowed_dir.as_deref())?;
        debug!("write_file: {}", path.display());

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, content).await?;
        Ok(format!(
            "Wrote {} bytes to {}",
            content.len(),
            path.display()
        ))
    }
}

// --- EditFile ---

pub struct EditFileTool {
    pub allowed_dir: Option<PathBuf>,
}

#[async_trait]
impl super::base::Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Replace a unique string in a file with new content."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old_text": { "type": "string", "description": "Exact text to find (must be unique)" },
                "new_text": { "type": "string", "description": "Replacement text" }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> crate::error::Result<String> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| crate::error::NanobotError::Tool("Missing 'path'".into()))?;
        let old_text = args["old_text"]
            .as_str()
            .ok_or_else(|| crate::error::NanobotError::Tool("Missing 'old_text'".into()))?;
        let new_text = args["new_text"]
            .as_str()
            .ok_or_else(|| crate::error::NanobotError::Tool("Missing 'new_text'".into()))?;
        let path = resolve_path(path_str, self.allowed_dir.as_deref())?;

        let content = tokio::fs::read_to_string(&path).await?;
        let count = content.matches(old_text).count();
        if count == 0 {
            return Err(crate::error::NanobotError::Tool(
                "old_text not found in file".into(),
            ));
        }
        if count > 1 {
            return Err(crate::error::NanobotError::Tool(format!(
                "old_text found {} times — must be unique",
                count
            )));
        }

        let updated = content.replacen(old_text, new_text, 1);
        tokio::fs::write(&path, updated).await?;
        Ok("Edit applied.".into())
    }
}

// --- ListDir ---

pub struct ListDirTool {
    pub allowed_dir: Option<PathBuf>,
}

#[async_trait]
impl super::base::Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List files and directories at a given path."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory path to list" }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> crate::error::Result<String> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| crate::error::NanobotError::Tool("Missing 'path'".into()))?;
        let path = resolve_path(path_str, self.allowed_dir.as_deref())?;

        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&path).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            let ft = entry.file_type().await?;
            let suffix = if ft.is_dir() { "/" } else { "" };
            entries.push(format!("{}{}", name, suffix));
        }
        entries.sort();
        Ok(entries.join("\n"))
    }
}
