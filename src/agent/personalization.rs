use crate::memory::{Memory, MemoryCategory};
use std::fmt::Write;

/// Character budget for the full personalization context block.
/// Entries are truncated (least-relevant first) when this is exceeded.
pub const PERSONALIZATION_BUDGET_CHARS: usize = 2_000;

/// Load all UserModel entries unconditionally (session-start injection).
/// These are always prepended to context regardless of query relevance.
pub async fn build_user_model_context(mem: &dyn Memory) -> String {
    let Ok(entries) = mem.list(Some(&MemoryCategory::UserModel)).await else {
        return String::new();
    };
    if entries.is_empty() {
        return String::new();
    }

    let mut context = String::from("[User model]\n");
    for entry in &entries {
        let _ = writeln!(context, "- {}: {}", entry.key, entry.content);
    }
    context.push('\n');
    context
}

/// Build a category-structured memory context for a user message.
///
/// Ordering (highest signal first):
///   1. [User preferences] — Preference + UserModel entries from recall
///   2. [Known facts about user] — Fact entries from recall
///   3. [Relevant memory] — everything else from recall
///
/// Entries with a score below `min_relevance_score` are filtered out to
/// prevent cross-topic bleed. Entries without a score are always included.
///
/// The combined output is truncated to `PERSONALIZATION_BUDGET_CHARS` if
/// necessary, removing entries from the least-relevant (last) section first.
pub async fn build_context(mem: &dyn Memory, user_msg: &str, min_relevance_score: f64) -> String {
    let Ok(entries) = mem.recall(user_msg, 10).await else {
        return String::new();
    };
    if entries.is_empty() {
        return String::new();
    }

    // Filter low-relevance entries; include entries without a score.
    let entries: Vec<_> = entries
        .into_iter()
        .filter(|e| match e.score {
            Some(score) => score >= min_relevance_score,
            None => true,
        })
        .collect();

    if entries.is_empty() {
        return String::new();
    }

    let mut preferences: Vec<&crate::memory::MemoryEntry> = Vec::new();
    let mut facts: Vec<&crate::memory::MemoryEntry> = Vec::new();
    let mut other: Vec<&crate::memory::MemoryEntry> = Vec::new();

    for entry in &entries {
        match &entry.category {
            MemoryCategory::Preference | MemoryCategory::UserModel => {
                preferences.push(entry);
            }
            MemoryCategory::Fact => {
                facts.push(entry);
            }
            _ => {
                other.push(entry);
            }
        }
    }

    let mut context = String::new();

    if !preferences.is_empty() {
        context.push_str("[User preferences]\n");
        for e in &preferences {
            let _ = writeln!(context, "- {}", e.content);
        }
        context.push('\n');
    }

    if !facts.is_empty() {
        context.push_str("[Known facts about user]\n");
        for e in &facts {
            let _ = writeln!(context, "- {}", e.content);
        }
        context.push('\n');
    }

    if !other.is_empty() {
        context.push_str("[Relevant memory]\n");
        for e in &other {
            let _ = writeln!(context, "- {}: {}", e.key, e.content);
        }
        context.push('\n');
    }

    // Enforce token budget: trim from the end (least relevant) if over limit.
    if context.chars().count() > PERSONALIZATION_BUDGET_CHARS {
        truncate_to_budget(&context, PERSONALIZATION_BUDGET_CHARS)
    } else {
        context
    }
}

/// Truncate `text` to at most `budget` chars, preserving whole lines and
/// appending a truncation notice.
fn truncate_to_budget(text: &str, budget: usize) -> String {
    const NOTICE: &str = "[... memory truncated for budget ...]\n";
    let notice_len = NOTICE.chars().count();
    let effective = budget.saturating_sub(notice_len);

    let mut result = String::new();
    let mut count = 0;
    for line in text.lines() {
        let line_chars = line.chars().count() + 1; // +1 for '\n'
        if count + line_chars > effective {
            break;
        }
        result.push_str(line);
        result.push('\n');
        count += line_chars;
    }
    result.push_str(NOTICE);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Memory, MemoryCategory, MemoryEntry};
    use async_trait::async_trait;

    // ── Helpers ────────────────────────────────────────────────────────────

    fn make_entry(
        key: &str,
        content: &str,
        category: MemoryCategory,
        score: f64,
    ) -> MemoryEntry {
        MemoryEntry {
            id: key.to_string(),
            key: key.to_string(),
            content: content.to_string(),
            category,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            session_id: None,
            score: Some(score),
        }
    }

    struct FixedRecallMemory {
        entries: Vec<MemoryEntry>,
        list_entries: Vec<MemoryEntry>,
    }

    #[async_trait]
    impl Memory for FixedRecallMemory {
        fn name(&self) -> &str {
            "fixed"
        }
        async fn store(
            &self,
            _k: &str,
            _c: &str,
            _cat: MemoryCategory,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn recall(
            &self,
            _query: &str,
            _limit: usize,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(self.entries.clone())
        }
        async fn get(&self, _key: &str) -> anyhow::Result<Option<MemoryEntry>> {
            Ok(None)
        }
        async fn list(
            &self,
            _category: Option<&MemoryCategory>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(self.list_entries.clone())
        }
        async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
        async fn count(&self) -> anyhow::Result<usize> {
            Ok(self.entries.len())
        }
        async fn health_check(&self) -> bool {
            true
        }
    }

    // ── Ordering tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn build_context_preferences_appear_before_facts_and_other() {
        let mem = FixedRecallMemory {
            entries: vec![
                make_entry("conv1", "hello world", MemoryCategory::Conversation, 0.5),
                make_entry("fact1", "user is a backend engineer", MemoryCategory::Fact, 0.8),
                make_entry(
                    "pref1",
                    "prefers terse responses",
                    MemoryCategory::Preference,
                    0.6,
                ),
            ],
            list_entries: vec![],
        };

        let ctx = build_context(&mem, "tell me about rust", 0.0).await;

        let pref_pos = ctx.find("[User preferences]").unwrap();
        let fact_pos = ctx.find("[Known facts about user]").unwrap();
        let other_pos = ctx.find("[Relevant memory]").unwrap();

        assert!(pref_pos < fact_pos, "preferences must come before facts");
        assert!(fact_pos < other_pos, "facts must come before other");
    }

    #[tokio::test]
    async fn build_context_user_model_in_preferences_section() {
        let mem = FixedRecallMemory {
            entries: vec![
                make_entry("um1", "terse", MemoryCategory::UserModel, 0.9),
                make_entry("other", "some old memory", MemoryCategory::Daily, 0.3),
            ],
            list_entries: vec![],
        };

        let ctx = build_context(&mem, "help me", 0.0).await;

        // UserModel should appear under [User preferences]
        assert!(ctx.contains("[User preferences]"));
        assert!(ctx.contains("terse"));
        // Should NOT appear under [Relevant memory] as a separate entry
        let pref_section_end = ctx
            .find("[Known facts about user]")
            .or_else(|| ctx.find("[Relevant memory]"))
            .unwrap_or(ctx.len());
        let pref_section = &ctx[..pref_section_end];
        assert!(pref_section.contains("terse"));
    }

    #[tokio::test]
    async fn build_context_empty_when_no_entries() {
        let mem = FixedRecallMemory {
            entries: vec![],
            list_entries: vec![],
        };
        let ctx = build_context(&mem, "anything", 0.0).await;
        assert!(ctx.is_empty());
    }

    #[tokio::test]
    async fn build_context_only_facts_section_when_no_prefs() {
        let mem = FixedRecallMemory {
            entries: vec![make_entry(
                "f1",
                "user loves coffee",
                MemoryCategory::Fact,
                0.7,
            )],
            list_entries: vec![],
        };
        let ctx = build_context(&mem, "tell me something", 0.0).await;
        assert!(!ctx.contains("[User preferences]"));
        assert!(ctx.contains("[Known facts about user]"));
        assert!(!ctx.contains("[Relevant memory]"));
    }

    // ── UserModel always-on test ──────────────────────────────────────────

    #[tokio::test]
    async fn build_user_model_context_returns_all_user_model_entries() {
        let mem = FixedRecallMemory {
            entries: vec![],
            list_entries: vec![
                make_entry(
                    "user_model.response_style",
                    "terse",
                    MemoryCategory::UserModel,
                    0.0,
                ),
                make_entry(
                    "user_model.expertise",
                    "backend engineer",
                    MemoryCategory::UserModel,
                    0.0,
                ),
            ],
        };

        let ctx = build_user_model_context(&mem).await;

        assert!(ctx.contains("[User model]"));
        assert!(ctx.contains("user_model.response_style"));
        assert!(ctx.contains("terse"));
        assert!(ctx.contains("user_model.expertise"));
        assert!(ctx.contains("backend engineer"));
    }

    #[tokio::test]
    async fn build_user_model_context_empty_when_no_entries() {
        let mem = FixedRecallMemory {
            entries: vec![],
            list_entries: vec![],
        };
        let ctx = build_user_model_context(&mem).await;
        assert!(ctx.is_empty());
    }

    // ── Truncation tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn build_context_truncates_when_over_budget() {
        // Generate many entries to exceed PERSONALIZATION_BUDGET_CHARS
        let long_content = "x".repeat(300);
        let entries: Vec<MemoryEntry> = (0..10)
            .map(|i| {
                make_entry(
                    &format!("conv{i}"),
                    &long_content,
                    MemoryCategory::Conversation,
                    0.5,
                )
            })
            .collect();

        let mem = FixedRecallMemory {
            entries,
            list_entries: vec![],
        };

        let ctx = build_context(&mem, "query", 0.0).await;
        assert!(
            ctx.chars().count() <= PERSONALIZATION_BUDGET_CHARS,
            "context ({} chars) must not exceed budget ({} chars)",
            ctx.chars().count(),
            PERSONALIZATION_BUDGET_CHARS
        );
        assert!(ctx.contains("[... memory truncated for budget ...]"));
    }

    #[tokio::test]
    async fn build_context_within_budget_not_truncated() {
        let mem = FixedRecallMemory {
            entries: vec![
                make_entry("k1", "short content", MemoryCategory::Preference, 0.9),
                make_entry("k2", "another short", MemoryCategory::Fact, 0.8),
            ],
            list_entries: vec![],
        };

        let ctx = build_context(&mem, "query", 0.0).await;
        assert!(!ctx.contains("[... memory truncated for budget ...]"));
    }

    #[test]
    fn truncate_to_budget_respects_whole_lines() {
        let text = "line one\nline two\nline three\n";
        let result = truncate_to_budget(text, 25);
        // Should contain complete lines only
        for line in result.lines() {
            assert!(
                text.contains(line) || line.contains("truncated"),
                "unexpected partial line: {line}"
            );
        }
    }
}
