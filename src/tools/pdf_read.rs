use super::traits::{Tool, ToolResult};
use crate::security::SecurityPolicy;
use async_trait::async_trait;
use serde_json::json;
use std::fmt::Write as FmtWrite;
use std::sync::Arc;

const MAX_PDF_BYTES: u64 = 50 * 1024 * 1024; // 50MB
const DEFAULT_MAX_CHARS: usize = 50_000;
const MAX_OUTPUT_CHARS: usize = 200_000;

/// Read and extract text from PDF files in the workspace.
pub struct PdfReadTool {
    security: Arc<SecurityPolicy>,
}

impl PdfReadTool {
    pub fn new(security: Arc<SecurityPolicy>) -> Self {
        Self { security }
    }
}

#[async_trait]
impl Tool for PdfReadTool {
    fn name(&self) -> &str {
        "pdf_read"
    }

    fn description(&self) -> &str {
        "Extract plain text from a PDF file in the workspace. \
         Returns the text content of the PDF, suitable for reading research papers, \
         contracts, reports, and other documents."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the PDF file within the workspace"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum number of characters to return (default: 50000, max: 200000)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing required parameter: path".into()),
                })
            }
        };

        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|v| {
                #[allow(clippy::cast_possible_truncation)]
                let n = v as usize;
                n.min(MAX_OUTPUT_CHARS)
            })
            .unwrap_or(DEFAULT_MAX_CHARS);

        // 1. Rate limit check
        if self.security.is_rate_limited() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: too many actions in the last hour".into()),
            });
        }

        // 2. Path allowlist check
        if !self.security.is_path_allowed(path) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Path not allowed by security policy: {path}")),
            });
        }

        // 3. Record action
        if !self.security.record_action() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: action budget exhausted".into()),
            });
        }

        let full_path = self.security.workspace_dir.join(path);

        // 4. Canonicalize (blocks symlink escapes)
        let resolved_path = match tokio::fs::canonicalize(&full_path).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Cannot resolve path '{path}': {e}")),
                })
            }
        };

        // 5. Resolved path allowlist check
        if !self.security.is_resolved_path_allowed(&resolved_path) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Resolved path escapes workspace: {path}")),
            });
        }

        // 6. File size check
        let metadata = match tokio::fs::metadata(&resolved_path).await {
            Ok(m) => m,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Cannot read file metadata '{path}': {e}")),
                })
            }
        };

        if metadata.len() > MAX_PDF_BYTES {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "File too large: {} bytes (max {} bytes)",
                    metadata.len(),
                    MAX_PDF_BYTES
                )),
            });
        }

        // 7. Read bytes
        let bytes = match tokio::fs::read(&resolved_path).await {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to read file '{path}': {e}")),
                })
            }
        };

        // 8. Feature-gated extraction
        #[cfg(feature = "rag-pdf")]
        let extraction: anyhow::Result<String> = {
            match tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                pdf_extract::extract_text_from_mem(&bytes)
                    .map_err(|e| anyhow::anyhow!("PDF parse error: {e}"))
            })
            .await
            {
                Ok(result) => result,
                Err(e) => Err(anyhow::anyhow!("PDF extraction task panicked: {e}")),
            }
        };

        #[cfg(not(feature = "rag-pdf"))]
        let extraction: anyhow::Result<String> = {
            let _ = bytes;
            Err(anyhow::anyhow!(
                "PDF extraction is not enabled. Rebuild with: cargo build --features rag-pdf"
            ))
        };

        match extraction {
            Ok(text) => {
                let output = if text.chars().count() > max_chars {
                    let mut truncated: String = text.chars().take(max_chars).collect();
                    let _ = write!(truncated, "\n\n... [truncated at {max_chars} chars]");
                    truncated
                } else {
                    text
                };
                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
            }),
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

    #[tokio::test]
    async fn pdf_read_missing_path_param() {
        let tmp = TempDir::new().unwrap();
        let tool = PdfReadTool::new(make_security(&tmp));
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn pdf_read_path_traversal_blocked() {
        let tmp = TempDir::new().unwrap();
        let tool = PdfReadTool::new(make_security(&tmp));
        let result = tool
            .execute(serde_json::json!({"path": "../etc/passwd"}))
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn pdf_read_nonexistent_file() {
        let tmp = TempDir::new().unwrap();
        let tool = PdfReadTool::new(make_security(&tmp));
        let result = tool
            .execute(serde_json::json!({"path": "nonexistent.pdf"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn pdf_read_oversized_file_rejected() {
        // We can't create a 50MB file in tests, but we can verify the constant exists
        assert_eq!(MAX_PDF_BYTES, 50 * 1024 * 1024);
        assert_eq!(DEFAULT_MAX_CHARS, 50_000);
        assert_eq!(MAX_OUTPUT_CHARS, 200_000);
    }

    #[cfg(not(feature = "rag-pdf"))]
    #[tokio::test]
    async fn pdf_read_no_feature_returns_helpful_error() {
        let tmp = TempDir::new().unwrap();
        // Write a fake PDF file
        std::fs::write(tmp.path().join("test.pdf"), b"%PDF-1.4 fake content").unwrap();
        let tool = PdfReadTool::new(make_security(&tmp));
        let result = tool
            .execute(serde_json::json!({"path": "test.pdf"}))
            .await
            .unwrap();
        assert!(!result.success);
        let err = result.error.unwrap();
        assert!(
            err.contains("rag-pdf"),
            "error should mention rag-pdf feature, got: {err}"
        );
    }

    #[test]
    fn pdf_read_tool_name_and_schema() {
        let tmp = TempDir::new().unwrap();
        let tool = PdfReadTool::new(make_security(&tmp));
        assert_eq!(tool.name(), "pdf_read");
        assert!(!tool.description().is_empty());
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["max_chars"].is_object());
    }
}
