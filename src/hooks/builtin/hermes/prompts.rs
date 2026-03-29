/// System prompt for Hermes memory extraction.
///
/// Placeholders:
/// - `{existing_memories}` — current Core memories (key: content) for dedup
/// - `{conversation_buffer}` — buffered conversation turns
pub const HERMES_EXTRACT_PROMPT: &str = r#"You are a memory extraction assistant for an AI agent named RustyClaw.

Given the following conversation between a user and the agent, extract facts,
preferences, decisions, and context that should be remembered for future
interactions.

For each extracted memory, provide:
- key: A short, unique identifier (snake_case, max 64 chars)
- content: The fact or preference (1-2 sentences max)
- category: One of "preference", "knowledge", "context", "decision", "goal"
- confidence: A score from 0.0 to 1.0 indicating how certain you are that
  this is a genuine, stable fact worth remembering long-term

Confidence guidelines:
- 1.0: User explicitly stated this fact ("I prefer TypeScript", "My name is X")
- 0.8-0.9: Strong inference from repeated behavior or clear context
- 0.6-0.7: Reasonable inference from single interaction
- 0.3-0.5: Tentative inference, may change
- Below 0.3: Do not extract — too uncertain

Rules:
- Do NOT extract ephemeral information (what the user is working on right now)
- Do NOT extract information the agent can derive from code or files
- Do NOT extract secrets, API keys, passwords, or sensitive credentials
- Do NOT duplicate facts that are already in the existing memories below
- Prefer updating an existing memory's key over creating a new duplicate
- Return an empty array if nothing is worth extracting

Existing memories (avoid duplicates):
{existing_memories}

Conversation:
{conversation_buffer}

Respond with ONLY a JSON array:
[
  {"key": "...", "content": "...", "category": "...", "confidence": 0.0}
]"#;

/// Build the extraction prompt with the given existing memories and conversation buffer.
pub fn build_extraction_prompt(existing_memories: &str, conversation_buffer: &str) -> String {
    HERMES_EXTRACT_PROMPT
        .replace("{existing_memories}", existing_memories)
        .replace("{conversation_buffer}", conversation_buffer)
}

/// System prompt for Hermes memory consolidation.
///
/// Placeholder:
/// - `{all_core_memories}` — all Core memories with key, content, confidence, and timestamp
pub const HERMES_CONSOLIDATE_PROMPT: &str = r#"You are a memory consolidation assistant for an AI agent named RustyClaw.

Review the following stored memories and recommend maintenance actions to keep
the memory store clean, consistent, and useful.

For each action, provide:
- action: One of "keep", "forget", "merge", "update"
- keys: Array of memory keys this action applies to
- content: (required for merge and update) The new/merged content
- confidence: (required for merge and update) The confidence score for the result

Action guidelines:
- keep: Memory is accurate and useful. No change needed.
- forget: Memory is outdated, contradicted by a newer memory, or no longer useful.
- merge: Two or more memories are redundant or nearly identical. Combine into one
  entry with the best content and highest justified confidence. All listed keys
  will be forgotten and replaced by a single new entry.
- update: Memory content or confidence needs adjustment (e.g., corrected wording,
  confidence increase from reinforcement, or decrease from staleness).

Rules:
- Contradictory memories: keep the more recent or more confident one, forget the other.
- Nearly identical memories: merge into one with higher confidence.
- Memories with very low confidence (< 0.3) that seem stale: forget.
- Do NOT forget memories that are still accurate and useful, even if old.
- When merging, the content should be the best synthesis of all originals.
- Return an empty array if no changes are needed.

Memories:
{all_core_memories}

Respond with ONLY a JSON array:
[
  {"action": "keep|forget|merge|update", "keys": ["..."], "content": "...", "confidence": 0.0}
]"#;

/// Build the consolidation prompt with the given core memories.
pub fn build_consolidation_prompt(all_core_memories: &str) -> String {
    HERMES_CONSOLIDATE_PROMPT.replace("{all_core_memories}", all_core_memories)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_substitutes_placeholders() {
        let prompt = build_extraction_prompt("key1: value1", "User: hi\nAssistant: hello");
        assert!(prompt.contains("key1: value1"));
        assert!(prompt.contains("User: hi\nAssistant: hello"));
        assert!(!prompt.contains("{existing_memories}"));
        assert!(!prompt.contains("{conversation_buffer}"));
    }

    #[test]
    fn build_prompt_handles_empty_inputs() {
        let prompt = build_extraction_prompt("", "");
        assert!(prompt.contains("Existing memories (avoid duplicates):"));
        assert!(prompt.contains("Conversation:"));
    }

    #[test]
    fn build_consolidation_prompt_substitutes() {
        let prompt = build_consolidation_prompt("user_name (0.95): User is Alice");
        assert!(prompt.contains("user_name (0.95): User is Alice"));
        assert!(!prompt.contains("{all_core_memories}"));
    }

    #[test]
    fn build_consolidation_prompt_handles_empty() {
        let prompt = build_consolidation_prompt("");
        assert!(prompt.contains("Memories:"));
        assert!(!prompt.contains("{all_core_memories}"));
    }
}
