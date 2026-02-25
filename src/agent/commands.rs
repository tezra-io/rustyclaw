use super::definition::{AgentDefinition, MemoryIsolation};
use super::registry::AgentRegistry;
use crate::config::Config;
use anyhow::Result;

#[allow(clippy::too_many_lines)]
pub async fn handle_command(command: crate::AgentSubCommands, config: &Config) -> Result<()> {
    let registry = AgentRegistry::from_config(config);

    match command {
        crate::AgentSubCommands::List => {
            let agents = registry.list();
            if agents.is_empty() {
                println!("No agents defined.");
                println!();
                println!("Create one:");
                println!("  rustyclaw agents create --name my-agent --persistent --skills twitter");
            } else {
                println!("Agents ({}):", agents.len());
                println!();
                for agent in &agents {
                    let kind = if agent.persistent {
                        "persistent"
                    } else {
                        "ephemeral"
                    };
                    let schedule = agent.schedule.as_deref().unwrap_or("none");
                    println!(
                        "  {} [{}] skills={:?} schedule={} memory={:?}",
                        console::style(&agent.name).white().bold(),
                        kind,
                        agent.skills,
                        schedule,
                        agent.memory
                    );
                    if !agent.allowed_tools.is_empty() {
                        println!("    allowed_tools: {:?}", agent.allowed_tools);
                    }
                }
            }
            Ok(())
        }

        crate::AgentSubCommands::Create {
            name,
            persistent,
            skills,
            memory,
            schedule,
            allowed_tools,
            from_description,
        } => {
            if let Some(desc) = from_description {
                println!(
                    "  {} Generating agent from: \"{desc}\"",
                    console::style("⟳").cyan().bold()
                );
                let definition = super::generator::generate_definition(&desc, config).await?;

                println!();
                println!("Generated agent definition:");
                println!("{}", console::style("─".repeat(40)).dim());
                println!("{}", definition.to_markdown()?);
                println!("{}", console::style("─".repeat(40)).dim());

                registry.create(&definition)?;
                println!(
                    "  {} Created agent '{}'",
                    console::style("✓").green().bold(),
                    definition.name
                );
                return Ok(());
            }

            let Some(agent_name) = name else {
                anyhow::bail!(
                    "Agent name is required. Use --name <name> or --from-description \"...\""
                );
            };

            let definition = AgentDefinition {
                name: agent_name.clone(),
                persistent,
                skills: skills.unwrap_or_default(),
                memory: match memory.as_deref() {
                    Some("shared-read") => MemoryIsolation::SharedRead,
                    Some("shared") => MemoryIsolation::Shared,
                    _ => MemoryIsolation::Isolated,
                },
                schedule,
                allowed_tools: allowed_tools.unwrap_or_default(),
                ..AgentDefinition::default()
            };

            // Validate before creating
            let available_skills: Vec<String> = crate::skills::load_skills(&config.workspace_dir)
                .iter()
                .map(|s| s.name.clone())
                .collect();
            let warnings = definition.validate(&available_skills)?;
            for w in &warnings {
                println!("  {} {w}", console::style("warning:").yellow().bold());
            }

            registry.create(&definition)?;
            println!(
                "  {} Created agent '{agent_name}'",
                console::style("✓").green().bold()
            );
            Ok(())
        }

        crate::AgentSubCommands::Edit { name } => {
            if !registry.exists(&name) {
                anyhow::bail!("Agent '{name}' not found");
            }

            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
            let path = super::definition::agents_dir_from_config(config).join(format!("{name}.md"));
            let status = std::process::Command::new(&editor).arg(&path).status()?;

            if status.success() {
                // Validate the edited file
                match AgentDefinition::from_file(&path) {
                    Ok(def) => {
                        let available_skills: Vec<String> =
                            crate::skills::load_skills(&config.workspace_dir)
                                .iter()
                                .map(|s| s.name.clone())
                                .collect();
                        match def.validate(&available_skills) {
                            Ok(warnings) => {
                                for w in &warnings {
                                    println!(
                                        "  {} {w}",
                                        console::style("warning:").yellow().bold()
                                    );
                                }
                                println!(
                                    "  {} Agent '{name}' updated",
                                    console::style("✓").green().bold()
                                );
                            }
                            Err(e) => {
                                println!(
                                    "  {} Validation error: {e}",
                                    console::style("✗").red().bold()
                                );
                                println!("  The file was saved but may have issues.");
                            }
                        }
                    }
                    Err(e) => {
                        println!("  {} Parse error: {e}", console::style("✗").red().bold());
                        println!("  The file was saved but may have issues.");
                    }
                }
            } else {
                println!("Editor exited with non-zero status");
            }
            Ok(())
        }

        crate::AgentSubCommands::Remove { name } => {
            registry.remove(&name)?;
            println!(
                "  {} Removed agent '{name}'",
                console::style("✓").green().bold()
            );
            Ok(())
        }

        crate::AgentSubCommands::Status { name } => {
            let Some(agent) = registry.get(&name) else {
                anyhow::bail!("Agent '{name}' not found");
            };

            println!("Agent: {}", console::style(&agent.name).white().bold());
            println!(
                "  Type:           {}",
                if agent.persistent {
                    "persistent"
                } else {
                    "ephemeral"
                }
            );
            println!("  Memory:         {:?}", agent.memory);
            println!("  Memory backend: {}", agent.memory_backend);
            println!("  Skills:         {:?}", agent.skills);
            println!(
                "  Schedule:       {}",
                agent.schedule.as_deref().unwrap_or("none")
            );
            println!(
                "  Model:          {}",
                agent.model.as_deref().unwrap_or("(default)")
            );
            println!(
                "  Temperature:    {}",
                agent
                    .temperature
                    .map_or("(default)".to_string(), |t| format!("{t}"))
            );
            println!("  Max tools/turn: {}", agent.max_tools_per_turn);
            if !agent.allowed_tools.is_empty() {
                println!("  Allowed tools:  {:?}", agent.allowed_tools);
            }
            if !agent.delegates_to.is_empty() {
                println!("  Delegates to:   {:?}", agent.delegates_to);
            }
            if !agent.channels.is_empty() {
                println!("  Channels:       {:?}", agent.channels);
            }
            if !agent.personality.is_empty() {
                println!();
                println!("Personality:");
                println!("{}", agent.personality);
            }
            Ok(())
        }

        crate::AgentSubCommands::SkillAdd { agent, skill } => {
            let Some(mut def) = registry.get(&agent) else {
                anyhow::bail!("Agent '{agent}' not found");
            };

            if def.skills.contains(&skill) {
                println!("Agent '{agent}' already has skill '{skill}'");
                return Ok(());
            }

            def.skills.push(skill.clone());
            registry.update(&def)?;
            println!(
                "  {} Added skill '{skill}' to agent '{agent}'",
                console::style("✓").green().bold()
            );
            Ok(())
        }

        crate::AgentSubCommands::SkillRemove { agent, skill } => {
            let Some(mut def) = registry.get(&agent) else {
                anyhow::bail!("Agent '{agent}' not found");
            };

            let before = def.skills.len();
            def.skills.retain(|s| s != &skill);
            if def.skills.len() == before {
                println!("Agent '{agent}' does not have skill '{skill}'");
                return Ok(());
            }

            registry.update(&def)?;
            println!(
                "  {} Removed skill '{skill}' from agent '{agent}'",
                console::style("✓").green().bold()
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a Config pointing at a temp workspace so the registry resolves correctly.
    fn test_config(tmp: &TempDir) -> Config {
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        // agents dir is a sibling of workspace
        let agents = tmp.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        Config {
            workspace_dir: workspace,
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn list_empty() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        handle_command(crate::AgentSubCommands::List, &config)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_and_list() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        handle_command(
            crate::AgentSubCommands::Create {
                name: Some("test-bot".into()),
                persistent: false,
                skills: None,
                memory: None,
                schedule: None,
                allowed_tools: None,
                from_description: None,
            },
            &config,
        )
        .await
        .unwrap();

        let registry = AgentRegistry::from_config(&config);
        assert!(registry.exists("test-bot"));
    }

    #[tokio::test]
    async fn create_requires_name() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let result = handle_command(
            crate::AgentSubCommands::Create {
                name: None,
                persistent: false,
                skills: None,
                memory: None,
                schedule: None,
                allowed_tools: None,
                from_description: None,
            },
            &config,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn remove_existing() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        handle_command(
            crate::AgentSubCommands::Create {
                name: Some("removable".into()),
                persistent: false,
                skills: None,
                memory: None,
                schedule: None,
                allowed_tools: None,
                from_description: None,
            },
            &config,
        )
        .await
        .unwrap();

        handle_command(
            crate::AgentSubCommands::Remove {
                name: "removable".into(),
            },
            &config,
        )
        .await
        .unwrap();

        let registry = AgentRegistry::from_config(&config);
        assert!(!registry.exists("removable"));
    }

    #[tokio::test]
    async fn remove_nonexistent_fails() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let result = handle_command(
            crate::AgentSubCommands::Remove {
                name: "ghost".into(),
            },
            &config,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn status_existing() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        handle_command(
            crate::AgentSubCommands::Create {
                name: Some("statusable".into()),
                persistent: true,
                skills: None,
                memory: None,
                schedule: None,
                allowed_tools: None,
                from_description: None,
            },
            &config,
        )
        .await
        .unwrap();

        handle_command(
            crate::AgentSubCommands::Status {
                name: "statusable".into(),
            },
            &config,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn status_nonexistent_fails() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let result = handle_command(
            crate::AgentSubCommands::Status {
                name: "nope".into(),
            },
            &config,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn skill_add_and_remove() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        handle_command(
            crate::AgentSubCommands::Create {
                name: Some("skilled".into()),
                persistent: false,
                skills: None,
                memory: None,
                schedule: None,
                allowed_tools: None,
                from_description: None,
            },
            &config,
        )
        .await
        .unwrap();

        handle_command(
            crate::AgentSubCommands::SkillAdd {
                agent: "skilled".into(),
                skill: "twitter".into(),
            },
            &config,
        )
        .await
        .unwrap();

        let registry = AgentRegistry::from_config(&config);
        let def = registry.get("skilled").unwrap();
        assert!(def.skills.contains(&"twitter".to_string()));

        handle_command(
            crate::AgentSubCommands::SkillRemove {
                agent: "skilled".into(),
                skill: "twitter".into(),
            },
            &config,
        )
        .await
        .unwrap();

        let def = registry.get("skilled").unwrap();
        assert!(!def.skills.contains(&"twitter".to_string()));
    }

    #[tokio::test]
    async fn skill_add_duplicate_is_noop() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        handle_command(
            crate::AgentSubCommands::Create {
                name: Some("dup-skill".into()),
                persistent: false,
                skills: Some(vec!["web".into()]),
                memory: None,
                schedule: None,
                allowed_tools: None,
                from_description: None,
            },
            &config,
        )
        .await
        .unwrap();

        // Adding the same skill again should succeed (noop)
        handle_command(
            crate::AgentSubCommands::SkillAdd {
                agent: "dup-skill".into(),
                skill: "web".into(),
            },
            &config,
        )
        .await
        .unwrap();

        let registry = AgentRegistry::from_config(&config);
        let def = registry.get("dup-skill").unwrap();
        assert_eq!(def.skills.iter().filter(|s| *s == "web").count(), 1);
    }

    #[tokio::test]
    async fn edit_nonexistent_fails() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let result = handle_command(
            crate::AgentSubCommands::Edit {
                name: "nope".into(),
            },
            &config,
        )
        .await;
        assert!(result.is_err());
    }
}
