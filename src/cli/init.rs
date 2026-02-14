//! Interactive onboarding wizard (`rustyclaw init`).
//!
//! Guides first-time users through setup: user info, assistant personality,
//! agent template selection, and LLM provider configuration.

use std::collections::HashMap;

use dialoguer::{Confirm, Input, MultiSelect, Select};

use crate::agent::templates::{
    builtin_templates, generate_soul_md, generate_user_md, AgentTemplate,
};

/// Run the full interactive onboarding wizard.
pub fn run_init() -> anyhow::Result<()> {
    println!("🦀 Welcome to RustyClaw!\n");
    println!("Let's set you up. This takes about 2 minutes.\n");

    let config_path = crate::config::get_config_path();
    let is_reinit = config_path.exists();
    if is_reinit {
        let proceed = Confirm::new()
            .with_prompt("Config already exists. Re-run setup?")
            .default(false)
            .interact()?;
        if !proceed {
            println!("Aborted.");
            return Ok(());
        }
    }

    let mut config = crate::config::load_config();

    // ─── About You ───────────────────────────────────────────────────
    println!("─── About You ───\n");

    let user_name: String = Input::new().with_prompt("Your name").interact_text()?;

    let detected_tz = detect_timezone();
    let timezone: String = Input::new()
        .with_prompt("Timezone")
        .default(detected_tz)
        .interact_text()?;

    let channel_options = &["telegram", "discord", "cli"];
    let channel_idx = Select::new()
        .with_prompt("Primary channel")
        .items(channel_options)
        .default(0)
        .interact()?;
    let primary_channel = channel_options[channel_idx];

    // ─── Your Assistant ──────────────────────────────────────────────
    println!("\n─── Your Assistant ───\n");

    let assistant_name: String = Input::new()
        .with_prompt("Give your assistant a name")
        .default("Claw".to_string())
        .interact_text()?;

    let personality: String = Input::new()
        .with_prompt("Personality in a few words (e.g., \"direct, witty, no BS\")")
        .default("helpful, concise, friendly".to_string())
        .interact_text()?;

    let interests: String = Input::new()
        .with_prompt("Topics/interests (comma-separated)")
        .default("AI, technology".to_string())
        .interact_text()?;

    // ─── Agents ──────────────────────────────────────────────────────
    println!("\n─── Agents ───\n");
    println!("Want to set up specialized agents? You can always add more later.\n");

    let templates = builtin_templates();
    let template_labels: Vec<String> = templates
        .iter()
        .map(|t| t.label())
        .chain(std::iter::once("⚡ Custom — define your own".to_string()))
        .chain(std::iter::once(
            "⏭  Skip — just the main agent for now".to_string(),
        ))
        .collect();

    let selections = MultiSelect::new()
        .with_prompt("Select agents (space to toggle, enter to confirm)")
        .items(&template_labels)
        .interact()?;

    // Collect agent files to write
    let mut agent_files: Vec<(String, String)> = Vec::new();

    let skip_idx = template_labels.len() - 1;
    let custom_idx = template_labels.len() - 2;

    if !selections.contains(&skip_idx) {
        for &idx in &selections {
            if idx == skip_idx {
                continue;
            }
            if idx == custom_idx {
                // Custom agent
                if let Some((name, content)) = prompt_custom_agent()? {
                    agent_files.push((format!("{}.md", name), content));
                }
                continue;
            }
            if idx < templates.len() {
                let template = &templates[idx];
                println!("\n─── {} Setup ───\n", template.display_name());
                let answers = prompt_template_questions(template)?;
                match template.render(&answers) {
                    Ok(content) => {
                        agent_files.push((template.filename(), content));
                        println!("✨ Configured {}", template.display_name());
                    }
                    Err(e) => {
                        eprintln!("  Error rendering template: {}", e);
                    }
                }
            }
        }
    }

    // ─── LLM Provider ────────────────────────────────────────────────
    println!("\n─── LLM Provider ───\n");

    let provider_options = &[
        "openrouter",
        "anthropic",
        "openai",
        "deepseek",
        "groq",
        "gemini",
        "other",
    ];
    let provider_idx = Select::new()
        .with_prompt("Provider")
        .items(provider_options)
        .default(0)
        .interact()?;
    let provider_name = provider_options[provider_idx];

    if provider_name != "other" {
        let current_key = config
            .providers
            .by_name(provider_name)
            .map(|p| &p.api_key)
            .filter(|k| !k.is_empty());

        let prompt_text = if current_key.is_some() {
            format!("{} API key (already set, Enter to keep)", provider_name)
        } else {
            format!("{} API key", provider_name)
        };

        let key: String = Input::new()
            .with_prompt(&prompt_text)
            .allow_empty(current_key.is_some())
            .interact_text()?;

        if !key.is_empty() {
            super::set_provider_key(&mut config, provider_name, &key);
        }
    }

    let model: String = Input::new()
        .with_prompt("Default model")
        .default(config.agents.defaults.model.clone())
        .interact_text()?;
    config.agents.defaults.model = model;

    // ─── Workspace ───────────────────────────────────────────────────

    let workspace: String = Input::new()
        .with_prompt("Workspace path")
        .default(config.agents.defaults.workspace.clone())
        .interact_text()?;
    config.agents.defaults.workspace = workspace;

    // ─── Write Everything ────────────────────────────────────────────
    println!("\n─── Done! ───\n");

    let ws_path = config.workspace_path();
    std::fs::create_dir_all(&ws_path)?;

    // SOUL.md
    let soul_path = ws_path.join("SOUL.md");
    if !soul_path.exists() || is_reinit {
        let soul = generate_soul_md(&assistant_name, &personality);
        std::fs::write(&soul_path, &soul)?;
        println!("  Created SOUL.md");
    }

    // USER.md
    let user_path = ws_path.join("USER.md");
    if !user_path.exists() || is_reinit {
        let user = generate_user_md(&user_name, &timezone, &interests, primary_channel);
        std::fs::write(&user_path, &user)?;
        println!("  Created USER.md");
    }

    // AGENTS.md (only on first init)
    let agents_md_path = ws_path.join("AGENTS.md");
    if !agents_md_path.exists() {
        std::fs::write(
            &agents_md_path,
            "# Agent Instructions\n\n\
             Your workspace. Add conventions, rules, and guidelines here.\n",
        )?;
        println!("  Created AGENTS.md");
    }

    // Memory dir
    let memory_dir = ws_path.join("memory");
    std::fs::create_dir_all(&memory_dir)?;

    // Agent files
    let agents_dir = dirs::home_dir()
        .map(|h| h.join(".rustyclaw").join("agents"))
        .unwrap_or_else(|| ws_path.join("agents"));
    std::fs::create_dir_all(&agents_dir)?;

    for (filename, content) in &agent_files {
        let path = agents_dir.join(filename);
        std::fs::write(&path, content)?;
        println!("  Created agents/{}", filename);
    }

    // Config
    crate::config::save_config(&config)?;
    println!("  Saved {}", config_path.display());

    println!("\nCreated:");
    println!("  {}", config_path.display());
    println!("  {}/SOUL.md", ws_path.display());
    println!("  {}/USER.md", ws_path.display());
    for (filename, _) in &agent_files {
        println!("  {}/{}", agents_dir.display(), filename);
    }

    println!("\nStart with: rustyclaw run");

    Ok(())
}

/// Prompt the user for template question answers.
fn prompt_template_questions(template: &AgentTemplate) -> anyhow::Result<HashMap<String, String>> {
    let mut answers = HashMap::new();

    for q in template.questions {
        let value: String = if let Some(default) = q.default {
            Input::new()
                .with_prompt(q.prompt)
                .default(default.to_string())
                .interact_text()?
        } else {
            Input::new().with_prompt(q.prompt).interact_text()?
        };
        answers.insert(q.key.to_string(), value);
    }

    Ok(answers)
}

/// Prompt for a custom agent definition.
fn prompt_custom_agent() -> anyhow::Result<Option<(String, String)>> {
    println!("\n─── Custom Agent ───\n");

    let name: String = Input::new()
        .with_prompt("Agent name (lowercase, hyphens ok)")
        .interact_text()?;

    let description: String = Input::new()
        .with_prompt("What should this agent do?")
        .interact_text()?;

    let tools_str: String = Input::new()
        .with_prompt("Tools needed (comma-separated, or 'all')")
        .default("all".to_string())
        .interact_text()?;

    let model: String = Input::new()
        .with_prompt("Model (haiku/sonnet/opus/inherit)")
        .default("sonnet".to_string())
        .interact_text()?;

    let schedule: String = Input::new()
        .with_prompt("Schedule (e.g., 'every 4h', 'cron: 0 10 * * *', or 'on-demand')")
        .default("on-demand".to_string())
        .interact_text()?;

    // Build tools YAML
    let tools_yaml = if tools_str.trim() == "all" {
        String::new()
    } else {
        let tools: Vec<&str> = tools_str.split(',').map(|t| t.trim()).collect();
        let mut yaml = "tools:\n".to_string();
        for tool in &tools {
            yaml.push_str(&format!("  - {}\n", tool));
        }
        yaml
    };

    // Build schedule YAML
    let schedule_yaml = if schedule.trim() == "on-demand" {
        String::new()
    } else {
        format!("schedule:\n  - {}\n", schedule.trim())
    };

    let content = format!(
        "---\nname: {name}\ndescription: {description}\nmodel: {model}\n{tools}{schedule}memory: isolated\n---\n\n{description}\n",
        name = name,
        description = description,
        model = model,
        tools = tools_yaml,
        schedule = schedule_yaml,
    );

    Ok(Some((name, content)))
}

/// Prompt for adding a single agent (used by `rustyclaw agent add`).
pub fn run_agent_add() -> anyhow::Result<()> {
    let templates = builtin_templates();
    let mut options: Vec<String> = templates.iter().map(|t| t.label()).collect();
    options.push("⚡ Custom — define your own".to_string());

    let idx = Select::new()
        .with_prompt("Choose a template")
        .items(&options)
        .interact()?;

    let agents_dir = dirs::home_dir()
        .map(|h| h.join(".rustyclaw").join("agents"))
        .unwrap_or_else(|| std::path::PathBuf::from(".rustyclaw/agents"));
    std::fs::create_dir_all(&agents_dir)?;

    if idx < templates.len() {
        let template = &templates[idx];
        println!("\n─── {} Setup ───\n", template.display_name());
        let answers = prompt_template_questions(template)?;
        match template.render(&answers) {
            Ok(content) => {
                let path = agents_dir.join(template.filename());
                std::fs::write(&path, &content)?;
                println!("\n✨ Created {}", path.display());
                println!("Edit it anytime, then run: rustyclaw agent reload");
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
    } else {
        // Custom
        if let Some((name, content)) = prompt_custom_agent()? {
            let path = agents_dir.join(format!("{}.md", name));
            std::fs::write(&path, &content)?;
            println!("\n✨ Created {}", path.display());
            println!("Edit it anytime, then run: rustyclaw agent reload");
        }
    }

    Ok(())
}

/// Open an agent file in $EDITOR.
pub fn run_agent_edit(name: &str) -> anyhow::Result<()> {
    let path = find_agent_file(name)?;
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vim".to_string());

    let status = std::process::Command::new(&editor).arg(&path).status()?;

    if status.success() {
        // Validate after edit
        match crate::agent::definition::parse_agent_file(&path) {
            Ok(result) => {
                if result.warnings.is_empty() {
                    println!("✅ Agent '{}' is valid.", name);
                } else {
                    println!("⚠️  Agent '{}' has warnings:", name);
                    for w in &result.warnings {
                        println!("  - {}", w.message);
                    }
                }
            }
            Err(e) => {
                eprintln!("❌ Agent file has errors: {}", e);
            }
        }
        println!("Run `rustyclaw agent reload` to apply changes.");
    }

    Ok(())
}

/// Remove an agent definition file (with confirmation).
pub fn run_agent_remove(name: &str) -> anyhow::Result<()> {
    let path = find_agent_file(name)?;

    let confirm = Confirm::new()
        .with_prompt(format!("Remove agent '{}'? ({})", name, path.display()))
        .default(false)
        .interact()?;

    if confirm {
        std::fs::remove_file(&path)?;
        println!("Removed agent '{}'.", name);
        println!("Run `rustyclaw agent reload` to apply changes.");
    } else {
        println!("Cancelled.");
    }

    Ok(())
}

/// Find an agent's .md file by name.
fn find_agent_file(name: &str) -> anyhow::Result<std::path::PathBuf> {
    let filename = if name.ends_with(".md") {
        name.to_string()
    } else {
        format!("{}.md", name)
    };

    // Check project-level first
    let project_path = std::path::PathBuf::from(".rustyclaw/agents").join(&filename);
    if project_path.exists() {
        return Ok(project_path);
    }

    // Check global
    let global_path = dirs::home_dir()
        .map(|h| h.join(".rustyclaw").join("agents").join(&filename))
        .unwrap_or_default();
    if global_path.exists() {
        return Ok(global_path);
    }

    Err(anyhow::anyhow!(
        "Agent '{}' not found in project or global agents directory.",
        name
    ))
}

/// Auto-detect timezone from system.
fn detect_timezone() -> String {
    // Try TZ env var first
    if let Ok(tz) = std::env::var("TZ") {
        if !tz.is_empty() {
            return tz;
        }
    }

    // macOS: read /etc/localtime symlink
    #[cfg(target_os = "macos")]
    {
        if let Ok(link) = std::fs::read_link("/etc/localtime") {
            let path = link.to_string_lossy();
            if let Some(tz) = path.strip_prefix("/var/db/timezone/zoneinfo/") {
                return tz.to_string();
            }
        }
    }

    // Linux: read /etc/timezone
    #[cfg(target_os = "linux")]
    {
        if let Ok(tz) = std::fs::read_to_string("/etc/timezone") {
            let tz = tz.trim();
            if !tz.is_empty() {
                return tz.to_string();
            }
        }
    }

    "UTC".to_string()
}
