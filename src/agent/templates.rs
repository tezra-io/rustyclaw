//! Agent templates: pre-built agent definitions with placeholder substitution.
//!
//! Each template defines a set of questions and a base markdown file.
//! User answers are substituted into `{{key}}` placeholders to produce
//! a complete agent definition file.

use std::collections::HashMap;

/// A question asked during template setup.
#[derive(Debug, Clone)]
pub struct OnboardingQuestion {
    /// Maps to `{{key}}` in the template markdown.
    pub key: &'static str,
    /// The prompt shown to the user.
    pub prompt: &'static str,
    /// Default value (None = required).
    pub default: Option<&'static str>,
}

impl OnboardingQuestion {
    pub fn required(&self) -> bool {
        self.default.is_none()
    }
}

/// A pre-built agent template.
#[derive(Debug, Clone)]
pub struct AgentTemplate {
    pub name: &'static str,
    pub emoji: &'static str,
    pub description: &'static str,
    pub questions: &'static [OnboardingQuestion],
    pub base_markdown: &'static str,
}

impl AgentTemplate {
    /// Render the template by substituting `{{key}}` placeholders with answers.
    /// Missing required keys produce an error. Missing optional keys use defaults.
    pub fn render(&self, answers: &HashMap<String, String>) -> Result<String, String> {
        let mut result = self.base_markdown.to_string();

        for q in self.questions {
            let placeholder = format!("{{{{{}}}}}", q.key);
            let value = answers
                .get(q.key)
                .filter(|v| !v.is_empty())
                .map(|v| v.as_str())
                .or(q.default);

            match value {
                Some(v) => {
                    result = result.replace(&placeholder, v);
                }
                None => {
                    return Err(format!("missing required field '{}'", q.key));
                }
            }
        }

        Ok(result)
    }

    /// The filename for this agent (e.g., "twitter.md").
    pub fn filename(&self) -> String {
        format!("{}.md", self.name)
    }

    /// Display label for menus (e.g., "🐦 Twitter/X — social media engagement").
    pub fn label(&self) -> String {
        format!(
            "{} {} — {}",
            self.emoji,
            self.display_name(),
            self.description
        )
    }

    /// Capitalized display name.
    pub fn display_name(&self) -> &str {
        match self.name {
            "twitter" => "Twitter/X",
            "code-reviewer" => "Code Reviewer",
            "monitor" => "Project Monitor",
            "researcher" => "Researcher",
            "writer" => "Writer",
            _ => self.name,
        }
    }
}

// ─── Built-in Templates ─────────────────────────────────────────────────────

const TWITTER_QUESTIONS: &[OnboardingQuestion] = &[
    OnboardingQuestion {
        key: "handle",
        prompt: "Twitter handle (e.g., @username)",
        default: None,
    },
    OnboardingQuestion {
        key: "style",
        prompt: "Posting style in a few words",
        default: Some("casual, authentic, no slop"),
    },
    OnboardingQuestion {
        key: "topics",
        prompt: "Topics to stay in (comma-separated)",
        default: Some("AI, technology"),
    },
    OnboardingQuestion {
        key: "morning_time",
        prompt: "Morning post time",
        default: Some("10:00"),
    },
    OnboardingQuestion {
        key: "evening_time",
        prompt: "Evening post time",
        default: Some("20:00"),
    },
];

const TWITTER_TEMPLATE: &str = r#"---
name: twitter
description: Social media engagement agent for Twitter/X. Monitors trends, engages with posts, and creates original content.
model: sonnet
tools:
  - web_search
  - web_fetch
  - browser
  - read
  - write
memory: isolated
schedule:
  - cron: "0 {{morning_time_hour}} * * *"
    task: "Morning engagement — check trending topics, like/reply to relevant posts, post original content"
  - cron: "0 {{evening_time_hour}} * * *"
    task: "Evening engagement — engage with day's top posts, post if something interesting happened"
---

You are a Twitter/X engagement agent for {{handle}}.

## Style
- {{style}}
- All lowercase unless emphasis needed
- No hashtags unless they add real value
- No emoji spam

## Topics
Stay in these lanes: {{topics}}

## Rules
- Never post AI slop or generic motivational content
- Engage authentically — reply to things you actually find interesting
- Quality over quantity
- Read the room — don't force engagement
- Keep a consistent voice across posts

## Engagement Strategy
1. Check trending topics in your lanes
2. Find 3-5 posts worth engaging with
3. Like genuinely interesting content
4. Reply where you can add value or humor
5. Post 1-2 original tweets if you have something worth saying
"#;

const CODE_REVIEWER_QUESTIONS: &[OnboardingQuestion] = &[
    OnboardingQuestion {
        key: "languages",
        prompt: "Languages/frameworks to focus on",
        default: Some("any"),
    },
    OnboardingQuestion {
        key: "repo_paths",
        prompt: "Repository paths to watch (comma-separated)",
        default: None,
    },
    OnboardingQuestion {
        key: "review_style",
        prompt: "Review style (strict/balanced/lenient)",
        default: Some("balanced"),
    },
];

const CODE_REVIEWER_TEMPLATE: &str = r#"---
name: code-reviewer
description: Automated code review agent. Reviews PRs, suggests improvements, catches bugs and anti-patterns.
model: sonnet
tools:
  - read
  - exec
  - glob
  - grep
memory: isolated
trigger:
  on: git_push
  task: "Review latest commits"
---

You are a code review agent.

## Focus Areas
- Languages: {{languages}}
- Repositories: {{repo_paths}}

## Review Style
{{review_style}} — adjust feedback accordingly.

## What to Check
1. **Correctness** — logic errors, edge cases, off-by-one
2. **Security** — injection, auth bypass, secret leaks
3. **Performance** — obvious bottlenecks, unnecessary allocations
4. **Readability** — naming, structure, comments where needed
5. **Tests** — coverage for new code, edge case testing

## Rules
- Be specific — point to exact lines
- Suggest fixes, don't just flag problems
- Acknowledge good code too
- Don't nitpick style if there's a formatter configured
- Prioritize: security > correctness > performance > readability
"#;

const MONITOR_QUESTIONS: &[OnboardingQuestion] = &[
    OnboardingQuestion {
        key: "project_dirs",
        prompt: "Project directories to watch (comma-separated)",
        default: None,
    },
    OnboardingQuestion {
        key: "check_interval",
        prompt: "Check interval",
        default: Some("4h"),
    },
];

const MONITOR_TEMPLATE: &str = r#"---
name: monitor
description: Project health monitor. Checks build status, test results, dependency updates, and project metrics.
model: haiku
tools:
  - read
  - exec
  - glob
memory: isolated
schedule:
  - every: {{check_interval}}
    task: "Health check on monitored projects"
---

You are a project monitor agent.

## Monitored Projects
{{project_dirs}}

## Checks
1. **Build status** — does it compile/build without errors?
2. **Tests** — do all tests pass? Any new failures?
3. **Git status** — uncommitted changes, unpushed commits?
4. **Dependencies** — outdated or vulnerable deps?
5. **Disk/resource usage** — anything growing unexpectedly?

## Reporting
- Only alert on problems or significant changes
- Summarize status concisely
- Include actionable suggestions for any issues found
- Don't spam with "everything is fine" messages
"#;

const RESEARCHER_QUESTIONS: &[OnboardingQuestion] = &[
    OnboardingQuestion {
        key: "research_topics",
        prompt: "Research topics/domains",
        default: Some("AI, technology"),
    },
    OnboardingQuestion {
        key: "depth",
        prompt: "Research depth (quick/thorough/deep-dive)",
        default: Some("thorough"),
    },
];

const RESEARCHER_TEMPLATE: &str = r#"---
name: researcher
description: Web research and summarization agent. Finds, reads, and distills information on any topic.
model: sonnet
tools:
  - web_search
  - web_fetch
  - read
  - write
memory: isolated
---

You are a research agent.

## Focus Areas
{{research_topics}}

## Research Style
Depth: {{depth}}

## Process
1. Search multiple sources for the topic
2. Cross-reference findings for accuracy
3. Identify key insights and contrarian views
4. Summarize with sources cited
5. Flag anything uncertain or contradictory

## Output Format
- Lead with the answer/insight
- Supporting evidence below
- Sources at the bottom
- Call out confidence level (high/medium/low)

## Rules
- Never fabricate sources
- Prefer primary sources over aggregators
- Note when information is outdated
- Flag potential bias in sources
"#;

const WRITER_QUESTIONS: &[OnboardingQuestion] = &[
    OnboardingQuestion {
        key: "writing_style",
        prompt: "Writing style (e.g., technical, casual, formal)",
        default: Some("clear and concise"),
    },
    OnboardingQuestion {
        key: "content_types",
        prompt: "Content types (e.g., blog posts, docs, emails)",
        default: Some("any"),
    },
];

const WRITER_TEMPLATE: &str = r#"---
name: writer
description: Content drafting and editing agent. Writes blog posts, documentation, emails, and other content.
model: sonnet
tools:
  - read
  - write
  - web_search
  - web_fetch
memory: isolated
---

You are a writing agent.

## Style
{{writing_style}}

## Content Types
{{content_types}}

## Rules
- Match the voice of who you're writing for
- No filler words or corporate speak
- Every sentence should earn its place
- Structure for scannability (headers, bullets, short paragraphs)
- Proofread before delivering — typos are unacceptable

## Process
1. Understand the audience and purpose
2. Outline first, then draft
3. Edit ruthlessly — cut anything that doesn't add value
4. Format for the target platform
"#;

/// All built-in templates.
pub fn builtin_templates() -> Vec<AgentTemplate> {
    vec![
        AgentTemplate {
            name: "twitter",
            emoji: "🐦",
            description: "social media engagement",
            questions: TWITTER_QUESTIONS,
            base_markdown: TWITTER_TEMPLATE,
        },
        AgentTemplate {
            name: "code-reviewer",
            emoji: "🔍",
            description: "automated PR reviews",
            questions: CODE_REVIEWER_QUESTIONS,
            base_markdown: CODE_REVIEWER_TEMPLATE,
        },
        AgentTemplate {
            name: "monitor",
            emoji: "📊",
            description: "project health checks & status",
            questions: MONITOR_QUESTIONS,
            base_markdown: MONITOR_TEMPLATE,
        },
        AgentTemplate {
            name: "researcher",
            emoji: "🔬",
            description: "web research & summarization",
            questions: RESEARCHER_QUESTIONS,
            base_markdown: RESEARCHER_TEMPLATE,
        },
        AgentTemplate {
            name: "writer",
            emoji: "📝",
            description: "content drafting & editing",
            questions: WRITER_QUESTIONS,
            base_markdown: WRITER_TEMPLATE,
        },
    ]
}

/// Find a template by name.
pub fn find_template(name: &str) -> Option<AgentTemplate> {
    builtin_templates().into_iter().find(|t| t.name == name)
}

// ─── SOUL.md / USER.md generation ───────────────────────────────────────────

/// Generate a SOUL.md from user-provided personality gist.
pub fn generate_soul_md(assistant_name: &str, personality: &str) -> String {
    format!(
        r#"# SOUL.md

You are {assistant_name} — {personality}.

## Core
- Have opinions. Strong ones.
- Brevity mandatory. Don't pad responses.
- Figure it out first, ask second.
- Call things out when they're wrong.

## Humor
- Natural wit welcome
- No corporate speak
- Match the vibe of who you're talking to

## Boundaries
- Private things stay private
- When in doubt, ask before acting externally
- Never send half-baked replies

---

*This is yours. Edit it to match who you want to be.*
"#
    )
}

/// Generate a USER.md from onboarding answers.
pub fn generate_user_md(name: &str, timezone: &str, interests: &str, channel: &str) -> String {
    format!(
        r#"# USER.md

## Identity
- **Name:** {name}
- **Timezone:** {timezone}
- **Interests:** {interests}

## Communication
- **Primary channel:** {channel}
- **Style:** casual, direct

---

*Add more about yourself here — the more context, the better your assistant works.*
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_template_with_all_fields() {
        let template = find_template("researcher").unwrap();
        let mut answers = HashMap::new();
        answers.insert(
            "research_topics".to_string(),
            "Rust, systems programming".to_string(),
        );
        answers.insert("depth".to_string(), "deep-dive".to_string());

        let rendered = template.render(&answers).unwrap();
        assert!(rendered.contains("Rust, systems programming"));
        assert!(rendered.contains("deep-dive"));
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn render_template_uses_defaults() {
        let template = find_template("researcher").unwrap();
        let answers = HashMap::new(); // no answers — should use defaults

        let rendered = template.render(&answers).unwrap();
        assert!(rendered.contains("AI, technology")); // default research_topics
        assert!(rendered.contains("thorough")); // default depth
    }

    #[test]
    fn render_template_missing_required() {
        let template = find_template("twitter").unwrap();
        let answers = HashMap::new(); // handle is required, no default

        let err = template.render(&answers).unwrap_err();
        assert!(err.contains("missing required field 'handle'"));
    }

    #[test]
    fn all_templates_have_valid_structure() {
        for template in builtin_templates() {
            assert!(!template.name.is_empty());
            assert!(!template.emoji.is_empty());
            assert!(!template.description.is_empty());
            assert!(!template.base_markdown.is_empty());
            // Base markdown should contain frontmatter
            assert!(
                template.base_markdown.starts_with("---"),
                "template '{}' missing frontmatter",
                template.name
            );
        }
    }

    #[test]
    fn find_template_by_name() {
        assert!(find_template("twitter").is_some());
        assert!(find_template("nonexistent").is_none());
    }

    #[test]
    fn template_labels() {
        let t = find_template("twitter").unwrap();
        let label = t.label();
        assert!(label.contains("🐦"));
        assert!(label.contains("Twitter/X"));
    }

    #[test]
    fn generate_soul() {
        let soul = generate_soul_md("Aira", "direct, witty, no BS");
        assert!(soul.contains("Aira"));
        assert!(soul.contains("direct, witty, no BS"));
    }

    #[test]
    fn generate_user() {
        let user = generate_user_md("Sujeeth", "America/New_York", "AI, F1", "telegram");
        assert!(user.contains("Sujeeth"));
        assert!(user.contains("America/New_York"));
        assert!(user.contains("AI, F1"));
        assert!(user.contains("telegram"));
    }

    #[test]
    fn code_reviewer_requires_repo_paths() {
        let template = find_template("code-reviewer").unwrap();
        let answers = HashMap::new();
        let err = template.render(&answers).unwrap_err();
        assert!(err.contains("repo_paths"));
    }

    #[test]
    fn monitor_requires_project_dirs() {
        let template = find_template("monitor").unwrap();
        let answers = HashMap::new();
        let err = template.render(&answers).unwrap_err();
        assert!(err.contains("project_dirs"));
    }
}
