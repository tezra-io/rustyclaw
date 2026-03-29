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
}
