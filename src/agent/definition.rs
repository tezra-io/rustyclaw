use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

use super::{AgentDefinition, MemoryMode};

/// Canonical tool names that agents can reference.
const KNOWN_TOOLS: &[&str] = &[
    "read",
    "write",
    "edit",
    "list_dir",
    "exec",
    "web_search",
    "web_fetch",
    "message",
    "bash",
    "browser",
    "grep",
    "glob",
];

/// Model shorthand aliases.
const MODEL_SHORTHANDS: &[(&str, &str)] = &[
    ("sonnet", "anthropic/claude-sonnet-4-5"),
    ("haiku", "anthropic/claude-haiku-4-5"),
    ("opus", "anthropic/claude-opus-4-6"),
];

/// YAML frontmatter parsed from agent markdown files.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
struct AgentFrontmatter {
    name: Option<String>,
    description: Option<String>,
    model: Option<String>,
    tools: Option<Vec<String>>,
    disallowed_tools: Option<Vec<String>>,
    memory: Option<String>,
    context_files: Option<Vec<String>>,
    schedule: Option<serde_yaml::Value>,
    trigger: Option<serde_yaml::Value>,
    max_turns: Option<u32>,
    channels: Option<Vec<String>>,
    hooks: Option<serde_yaml::Value>,
}

/// Validation warning (non-fatal).
#[derive(Debug)]
pub struct ValidationWarning {
    pub file: String,
    pub message: String,
}

impl std::fmt::Display for ValidationWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} — {}", self.file, self.message)
    }
}

/// Result of parsing an agent definition file.
#[derive(Debug)]
pub struct ParseResult {
    pub definition: AgentDefinition,
    pub warnings: Vec<ValidationWarning>,
}

/// Parse a single agent markdown file into an AgentDefinition.
pub fn parse_agent_file(path: &Path) -> Result<ParseResult, String> {
    let filename = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());

    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("{} — cannot read file: {}", filename, e))?;

    parse_agent_content(&content, &filename)
}

/// Parse agent definition from content string (testable without filesystem).
pub fn parse_agent_content(content: &str, filename: &str) -> Result<ParseResult, String> {
    let mut warnings = Vec::new();

    // Extract YAML frontmatter between --- markers
    let (frontmatter_str, body) = extract_frontmatter(content).ok_or_else(|| {
        format!(
            "{} — no YAML frontmatter found (expected --- delimiters)",
            filename
        )
    })?;

    // Parse YAML
    let fm: AgentFrontmatter = serde_yaml::from_str(&frontmatter_str)
        .map_err(|e| format!("{} — invalid YAML: {}", filename, e))?;

    // Validate required fields
    let name = fm
        .name
        .filter(|n| !n.is_empty())
        .ok_or_else(|| format!("{} — missing required field 'name'", filename))?;

    let description = fm
        .description
        .filter(|d| !d.is_empty())
        .ok_or_else(|| format!("{} — missing required field 'description'", filename))?;

    // Validate + resolve model shorthand
    let model = fm
        .model
        .map(|m| resolve_model_shorthand(&m, filename, &mut warnings));

    // Validate tool names
    let tools = fm.tools.map(|tools| {
        validate_tool_names(&tools, filename, &mut warnings);
        tools
    });

    // Handle disallowed_tools (just validate names)
    if let Some(ref disallowed) = fm.disallowed_tools {
        validate_tool_names(disallowed, filename, &mut warnings);
    }

    // Validate memory mode
    let memory_mode = match fm.memory.as_deref() {
        Some("isolated") | None => MemoryMode::Isolated,
        Some("shared") => MemoryMode::Shared,
        Some(other) => {
            warnings.push(ValidationWarning {
                file: filename.to_string(),
                message: format!("unknown memory mode '{}', defaulting to 'isolated'", other),
            });
            MemoryMode::Isolated
        }
    };

    // Validate cron expressions in schedule
    if let Some(ref schedule) = fm.schedule {
        validate_schedule(schedule, filename, &mut warnings);
    }

    let context_files = fm.context_files.unwrap_or_default();

    let definition = AgentDefinition {
        name,
        description,
        system_prompt: body.trim().to_string(),
        model,
        tools,
        context_files,
        memory_mode,
    };

    Ok(ParseResult {
        definition,
        warnings,
    })
}

/// Load all agent definitions from global (~/.rustyclaw/agents/) and project (.rustyclaw/agents/).
/// Project-level definitions override global ones with the same name.
pub fn load_all_agents() -> (Vec<AgentDefinition>, Vec<ValidationWarning>) {
    let mut agents: HashMap<String, AgentDefinition> = HashMap::new();
    let mut all_warnings = Vec::new();

    // Global agents directory
    let global_dir = dirs::home_dir()
        .map(|h| h.join(".rustyclaw").join("agents"))
        .unwrap_or_else(|| PathBuf::from("~/.rustyclaw/agents"));

    if global_dir.is_dir() {
        load_agents_from_dir(&global_dir, &mut agents, &mut all_warnings);
    }

    // Project-level agents directory (overrides global)
    let project_dir = PathBuf::from(".rustyclaw/agents");
    if project_dir.is_dir() {
        load_agents_from_dir(&project_dir, &mut agents, &mut all_warnings);
    }

    let definitions: Vec<AgentDefinition> = agents.into_values().collect();
    debug!("Loaded {} agent definitions", definitions.len());

    (definitions, all_warnings)
}

/// Load agent definitions from a specific directory.
fn load_agents_from_dir(
    dir: &Path,
    agents: &mut HashMap<String, AgentDefinition>,
    warnings: &mut Vec<ValidationWarning>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!("Cannot read agents directory {}: {}", dir.display(), e);
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "md").unwrap_or(false) {
            match parse_agent_file(&path) {
                Ok(result) => {
                    let name = result.definition.name.clone();
                    if agents.contains_key(&name) {
                        debug!("Agent '{}' overridden by {}", name, path.display());
                    }
                    agents.insert(name, result.definition);
                    warnings.extend(result.warnings);
                }
                Err(e) => {
                    warn!("Skipping agent file: {}", e);
                    warnings.push(ValidationWarning {
                        file: path.display().to_string(),
                        message: e,
                    });
                }
            }
        }
    }
}

/// Extract YAML frontmatter and body from markdown content.
fn extract_frontmatter(content: &str) -> Option<(String, String)> {
    let trimmed = content.trim_start();
    let stripped = trimmed.strip_prefix("---")?;

    // Find the closing ---
    let end = stripped.find("\n---")?;
    let frontmatter = stripped[..end].to_string();
    let body = stripped[end + 4..].to_string();

    Some((frontmatter, body))
}

/// Resolve model shorthand (e.g., "sonnet" → "anthropic/claude-sonnet-4-5").
fn resolve_model_shorthand(
    model: &str,
    filename: &str,
    warnings: &mut Vec<ValidationWarning>,
) -> String {
    if model == "inherit" {
        return "inherit".to_string();
    }

    for (shorthand, full) in MODEL_SHORTHANDS {
        if model == *shorthand {
            return full.to_string();
        }
    }

    // If it contains a slash, assume it's already a full model ID
    if model.contains('/') {
        return model.to_string();
    }

    // Unknown shorthand — warn but keep as-is
    warnings.push(ValidationWarning {
        file: filename.to_string(),
        message: format!(
            "unknown model shorthand '{}'. Known: sonnet, haiku, opus, inherit",
            model
        ),
    });
    model.to_string()
}

/// Validate tool names against the canonical list.
fn validate_tool_names(tools: &[String], filename: &str, warnings: &mut Vec<ValidationWarning>) {
    for tool in tools {
        if !KNOWN_TOOLS.contains(&tool.as_str()) {
            let suggestion = find_closest_tool(tool);
            let msg = if let Some(closest) = suggestion {
                format!("unknown tool '{}'. Did you mean '{}'?", tool, closest)
            } else {
                format!(
                    "unknown tool '{}'. Known tools: {}",
                    tool,
                    KNOWN_TOOLS.join(", ")
                )
            };
            warnings.push(ValidationWarning {
                file: filename.to_string(),
                message: msg,
            });
        }
    }
}

/// Find the closest matching tool name (simple edit distance).
fn find_closest_tool(input: &str) -> Option<&'static str> {
    let input_lower = input.to_lowercase();
    KNOWN_TOOLS
        .iter()
        .filter(|t| {
            // Simple heuristic: share at least half the characters
            let common = input_lower.chars().filter(|c| t.contains(*c)).count();
            common * 2 >= input_lower.len()
        })
        .min_by_key(|t| levenshtein(&input_lower, t))
        .copied()
}

/// Simple Levenshtein distance for tool name suggestions.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];

    for (i, row) in dp.iter_mut().enumerate().take(a.len() + 1) {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate().take(b.len() + 1) {
        *cell = j;
    }

    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }

    dp[a.len()][b.len()]
}

/// Validate schedule entries (check cron expressions).
fn validate_schedule(
    schedule: &serde_yaml::Value,
    filename: &str,
    warnings: &mut Vec<ValidationWarning>,
) {
    let entries = match schedule {
        serde_yaml::Value::Sequence(seq) => seq.clone(),
        serde_yaml::Value::Mapping(_) => vec![schedule.clone()],
        _ => {
            warnings.push(ValidationWarning {
                file: filename.to_string(),
                message: "schedule should be a list of schedule entries".to_string(),
            });
            return;
        }
    };

    for entry in &entries {
        if let Some(cron_expr) = entry.get("cron").and_then(|v| v.as_str()) {
            // The `cron` crate expects 6-field expressions (with seconds).
            // User-facing cron is 5-field, so prepend "0 " for seconds.
            let full_expr = format!("0 {}", cron_expr);
            if full_expr.parse::<cron::Schedule>().is_err() {
                warnings.push(ValidationWarning {
                    file: filename.to_string(),
                    message: format!("invalid cron expression '{}'", cron_expr),
                });
            }
        }
        // `every` format (e.g., "4h") is accepted without validation for now
    }
}

/// Get the list of known tool names (for CLI validation output).
pub fn known_tool_names() -> &'static [&'static str] {
    KNOWN_TOOLS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_agent_md() -> &'static str {
        "---\nname: test-agent\ndescription: A test agent for unit testing.\nmodel: sonnet\ntools:\n  - read\n  - write\nmemory: isolated\ncontext_files:\n  - test/CONTEXT.md\n---\n\nYou are a test agent.\n\n## Rules\n- Be helpful\n"
    }

    #[test]
    fn parse_valid_agent() {
        let result = parse_agent_content(valid_agent_md(), "test.md").unwrap();
        assert_eq!(result.definition.name, "test-agent");
        assert_eq!(
            result.definition.description,
            "A test agent for unit testing."
        );
        assert_eq!(
            result.definition.model,
            Some("anthropic/claude-sonnet-4-5".to_string())
        );
        assert_eq!(
            result.definition.tools,
            Some(vec!["read".to_string(), "write".to_string()])
        );
        assert_eq!(result.definition.memory_mode, MemoryMode::Isolated);
        assert!(result
            .definition
            .system_prompt
            .contains("You are a test agent."));
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn parse_missing_name() {
        let md = "---\ndescription: No name agent\n---\nBody\n";
        let err = parse_agent_content(md, "noname.md").unwrap_err();
        assert!(err.contains("missing required field 'name'"));
    }

    #[test]
    fn parse_missing_description() {
        let md = "---\nname: agent\n---\nBody\n";
        let err = parse_agent_content(md, "nodesc.md").unwrap_err();
        assert!(err.contains("missing required field 'description'"));
    }

    #[test]
    fn parse_bad_yaml() {
        let md = "---\nname: [invalid yaml\n---\nBody\n";
        let err = parse_agent_content(md, "bad.md").unwrap_err();
        assert!(err.contains("invalid YAML"));
    }

    #[test]
    fn parse_no_frontmatter() {
        let md = "Just some markdown without frontmatter.";
        let err = parse_agent_content(md, "nofm.md").unwrap_err();
        assert!(err.contains("no YAML frontmatter found"));
    }

    #[test]
    fn unknown_tool_warning() {
        let md =
            "---\nname: agent\ndescription: desc\ntools:\n  - filesystem\n  - read\n---\nBody\n";
        let result = parse_agent_content(md, "tools.md").unwrap();
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0]
            .message
            .contains("unknown tool 'filesystem'"));
    }

    #[test]
    fn invalid_cron_warning() {
        let md = "---\nname: agent\ndescription: desc\nschedule:\n  - cron: \"0 25 * * *\"\n    task: bad\n---\nBody\n";
        let result = parse_agent_content(md, "cron.md").unwrap();
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].message.contains("invalid cron"));
    }

    #[test]
    fn valid_cron_no_warning() {
        let md = "---\nname: agent\ndescription: desc\nschedule:\n  - cron: \"0 10 * * *\"\n    task: morning\n---\nBody\n";
        let result = parse_agent_content(md, "cron.md").unwrap();
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn model_shorthand_resolution() {
        let md = "---\nname: agent\ndescription: desc\nmodel: haiku\n---\nBody\n";
        let result = parse_agent_content(md, "model.md").unwrap();
        assert_eq!(
            result.definition.model,
            Some("anthropic/claude-haiku-4-5".to_string())
        );
    }

    #[test]
    fn model_inherit() {
        let md = "---\nname: agent\ndescription: desc\nmodel: inherit\n---\nBody\n";
        let result = parse_agent_content(md, "model.md").unwrap();
        assert_eq!(result.definition.model, Some("inherit".to_string()));
    }

    #[test]
    fn unknown_model_shorthand_warns() {
        let md = "---\nname: agent\ndescription: desc\nmodel: gpt4\n---\nBody\n";
        let result = parse_agent_content(md, "model.md").unwrap();
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0]
            .message
            .contains("unknown model shorthand"));
    }

    #[test]
    fn full_model_id_no_warning() {
        let md = "---\nname: agent\ndescription: desc\nmodel: openai/gpt-4o\n---\nBody\n";
        let result = parse_agent_content(md, "model.md").unwrap();
        assert_eq!(result.definition.model, Some("openai/gpt-4o".to_string()));
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn unknown_memory_mode_defaults() {
        let md = "---\nname: agent\ndescription: desc\nmemory: broken\n---\nBody\n";
        let result = parse_agent_content(md, "mem.md").unwrap();
        assert_eq!(result.definition.memory_mode, MemoryMode::Isolated);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].message.contains("unknown memory mode"));
    }

    #[test]
    fn shared_memory_mode() {
        let md = "---\nname: agent\ndescription: desc\nmemory: shared\n---\nBody\n";
        let result = parse_agent_content(md, "mem.md").unwrap();
        assert_eq!(result.definition.memory_mode, MemoryMode::Shared);
    }

    #[test]
    fn levenshtein_distance() {
        assert_eq!(levenshtein("read", "read"), 0);
        assert_eq!(levenshtein("raed", "read"), 2);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn tool_suggestion() {
        let suggestion = find_closest_tool("reed");
        assert_eq!(suggestion, Some("read"));
    }
}
