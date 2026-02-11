use std::path::PathBuf;
use tracing::debug;

/// A loaded skill with metadata from YAML frontmatter.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,
    pub always_load: bool,
    pub required_bins: Vec<String>,
    pub required_env: Vec<String>,
}

/// Loads markdown skills from workspace and builtin directories.
pub struct SkillsLoader {
    workspace_dir: PathBuf,
    builtin_dir: Option<PathBuf>,
}

impl SkillsLoader {
    pub fn new(workspace: PathBuf) -> Self {
        let skills_dir = workspace.join("skills");
        Self {
            workspace_dir: skills_dir,
            builtin_dir: None,
        }
    }

    pub fn with_builtin(mut self, dir: PathBuf) -> Self {
        self.builtin_dir = Some(dir);
        self
    }

    /// Load all available skills.
    pub fn load_all(&self) -> Vec<Skill> {
        let mut skills = Vec::new();

        for dir in self.skill_dirs() {
            if dir.is_dir() {
                skills.extend(self.load_from_dir(&dir));
            }
        }

        debug!("Loaded {} skills", skills.len());
        skills
    }

    /// Get skills that should always be included in context.
    pub fn always_loaded(&self) -> Vec<Skill> {
        self.load_all()
            .into_iter()
            .filter(|s| s.always_load)
            .collect()
    }

    /// Get a summary of all skills (name + description).
    pub fn summary(&self) -> String {
        self.load_all()
            .iter()
            .map(|s| format!("- **{}**: {}", s.name, s.description))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn skill_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.workspace_dir.clone()];
        if let Some(b) = &self.builtin_dir {
            dirs.push(b.clone());
        }
        dirs
    }

    fn load_from_dir(&self, dir: &PathBuf) -> Vec<Skill> {
        let mut skills = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return skills,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                if let Some(skill) = self.parse_skill(&path) {
                    if self.check_requirements(&skill) {
                        skills.push(skill);
                    }
                }
            }
        }
        skills
    }

    fn parse_skill(&self, path: &PathBuf) -> Option<Skill> {
        let content = std::fs::read_to_string(path).ok()?;
        let name = path.file_stem()?.to_string_lossy().to_string();

        // Parse YAML frontmatter (between --- markers)
        let mut description = String::new();
        let mut always_load = false;
        let required_bins = Vec::new();
        let required_env = Vec::new();
        let mut body = content.as_str();

        if let Some(stripped) = content.strip_prefix("---") {
            if let Some(end) = stripped.find("---") {
                let frontmatter = &stripped[..end];
                body = &stripped[end + 3..];

                for line in frontmatter.lines() {
                    let line = line.trim();
                    if let Some(val) = line.strip_prefix("description:") {
                        description = val.trim().to_string();
                    } else if let Some(val) = line.strip_prefix("always_load:") {
                        always_load = val.trim() == "true";
                    }
                    // Simplified — full YAML parsing would use serde_yaml
                }
            }
        }

        Some(Skill {
            name,
            description,
            content: body.trim().to_string(),
            always_load,
            required_bins,
            required_env,
        })
    }

    fn check_requirements(&self, skill: &Skill) -> bool {
        for bin in &skill.required_bins {
            if which::which(bin).is_err() {
                debug!("Skill {} requires missing binary: {}", skill.name, bin);
                return false;
            }
        }
        for env in &skill.required_env {
            if std::env::var(env).is_err() {
                debug!("Skill {} requires missing env var: {}", skill.name, env);
                return false;
            }
        }
        true
    }
}
