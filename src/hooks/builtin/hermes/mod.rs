pub mod prompts;
pub mod types;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::schema::HermesConfig;
use crate::hooks::traits::HookHandler;
use crate::memory::traits::{Memory, MemoryCategory, MemorySource};
use crate::providers::traits::{ChatMessage, ChatResponse, Provider};

use prompts::build_extraction_prompt;
use types::ExtractedFact;

const STATE_FILE: &str = "hermes_state.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HermesState {
    last_extraction_at: Option<String>,
    total_extracted_memories: u64,
}

/// Hermes extraction hook — buffers conversation turns and runs LLM extraction
/// on heartbeat ticks when enough turns have accumulated.
pub struct HermesExtractionHook {
    config: HermesConfig,
    /// Buffered (user_message, assistant_message) pairs since last extraction.
    conversation_buffer: Arc<Mutex<Vec<(String, String)>>>,
    /// Last user message captured by on_llm_input, paired on next on_llm_output.
    pending_user_msg: Arc<Mutex<Option<String>>>,
    /// Number of conversation turns since last extraction.
    turn_count: Arc<AtomicU32>,
    /// Memory backend for storing extracted facts and querying existing memories.
    memory: Arc<dyn Memory>,
    /// LLM provider for extraction calls.
    provider: Arc<dyn Provider>,
    /// Model name for extraction (resolved from config or agent default).
    model: String,
    /// Workspace directory for state file persistence.
    workspace_dir: PathBuf,
    /// Session ID to scope extracted memories.
    session_id: Option<String>,
}

impl HermesExtractionHook {
    pub fn new(
        config: HermesConfig,
        memory: Arc<dyn Memory>,
        provider: Arc<dyn Provider>,
        default_model: String,
        workspace_dir: PathBuf,
        session_id: Option<String>,
    ) -> Self {
        let model = config
            .extraction_model
            .as_ref()
            .and_then(|m| m.split_once('/').map(|(_, model)| model.to_string()))
            .unwrap_or(default_model);

        Self {
            config,
            conversation_buffer: Arc::new(Mutex::new(Vec::new())),
            pending_user_msg: Arc::new(Mutex::new(None)),
            turn_count: Arc::new(AtomicU32::new(0)),
            memory,
            provider,
            model,
            workspace_dir,
            session_id,
        }
    }

    /// Run LLM extraction on the buffered conversation.
    async fn run_extraction(&self) {
        let turns: Vec<(String, String)> = {
            let mut buf = self.conversation_buffer.lock().unwrap();
            buf.drain(..).collect()
        };
        self.turn_count.store(0, Ordering::SeqCst);

        if turns.is_empty() {
            return;
        }

        // Build conversation text from buffered turns.
        let conversation_text: String = turns
            .iter()
            .map(|(user, assistant)| format!("User: {user}\nAssistant: {assistant}"))
            .collect::<Vec<_>>()
            .join("\n\n");

        // Fetch existing Core memories for dedup context.
        let existing = match self.memory.list(Some(&MemoryCategory::Core), None).await {
            Ok(entries) => entries
                .iter()
                .map(|e| format!("{}: {}", e.key, e.content))
                .collect::<Vec<_>>()
                .join("\n"),
            Err(e) => {
                tracing::warn!(
                    hook = "hermes-extraction",
                    "failed to list existing memories: {e}"
                );
                String::new()
            }
        };

        let prompt = build_extraction_prompt(&existing, &conversation_text);

        // Call LLM for extraction.
        let response = match self
            .provider
            .chat_with_system(
                Some(&prompt),
                "Extract memories from the conversation above.",
                &self.model,
                0.3,
            )
            .await
        {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!(
                    hook = "hermes-extraction",
                    "LLM extraction call failed: {e}"
                );
                return;
            }
        };

        // Parse JSON response — strip markdown fences if present.
        let json_str = strip_json_fences(&response);
        let facts: Vec<ExtractedFact> = match serde_json::from_str(json_str) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(
                    hook = "hermes-extraction",
                    response_preview = &response[..response.len().min(200)],
                    "malformed extraction JSON: {e}"
                );
                return;
            }
        };

        // Store each extracted fact, respecting max_extractions_per_tick and dedup.
        let mut stored = 0u64;
        for fact in facts.iter().take(self.config.max_extractions_per_tick) {
            if fact.confidence < 0.3 {
                continue;
            }

            // Dedup: skip if existing memory has higher or equal confidence.
            if let Ok(Some(existing_entry)) = self.memory.get(&fact.key).await {
                if existing_entry.confidence >= fact.confidence {
                    continue;
                }
            }

            if let Err(e) = self
                .memory
                .store_with_metadata(
                    &fact.key,
                    &fact.content,
                    MemoryCategory::Core,
                    self.session_id.as_deref(),
                    fact.confidence,
                    MemorySource::Extracted,
                )
                .await
            {
                tracing::warn!(
                    hook = "hermes-extraction",
                    key = %fact.key,
                    "failed to store extracted memory: {e}"
                );
            } else {
                stored += 1;
            }
        }

        if stored > 0 {
            tracing::info!(
                hook = "hermes-extraction",
                extracted = facts.len(),
                stored,
                "extraction cycle complete"
            );
        }

        // Update state file.
        if let Err(e) = self.write_state(stored) {
            tracing::warn!(hook = "hermes-extraction", "failed to write state: {e}");
        }
    }

    fn write_state(&self, newly_stored: u64) -> anyhow::Result<()> {
        let path = state_path(&self.workspace_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut state = read_state(&self.workspace_dir);
        state.last_extraction_at = Some(chrono::Utc::now().to_rfc3339());
        state.total_extracted_memories += newly_stored;

        let json = serde_json::to_vec_pretty(&state)?;
        std::fs::write(path, json)?;
        Ok(())
    }
}

fn state_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("state").join(STATE_FILE)
}

fn read_state(workspace_dir: &Path) -> HermesState {
    let path = state_path(workspace_dir);
    if !path.exists() {
        return HermesState::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => HermesState::default(),
    }
}

/// Strip markdown code fences (```json ... ```) from LLM response.
fn strip_json_fences(s: &str) -> &str {
    let trimmed = s.trim();
    let without_opening = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    let without_closing = without_opening
        .trim()
        .strip_suffix("```")
        .unwrap_or(without_opening);
    without_closing.trim()
}

#[async_trait]
impl HookHandler for HermesExtractionHook {
    fn name(&self) -> &str {
        "hermes-extraction"
    }

    fn priority(&self) -> i32 {
        -100 // Low priority — runs after other hooks.
    }

    async fn on_llm_input(&self, messages: &[ChatMessage], _model: &str) {
        if !self.config.enabled {
            return;
        }
        // Capture the last user message for pairing with the next on_llm_output.
        if let Some(last_user) = messages.iter().rfind(|m| m.role == "user") {
            *self.pending_user_msg.lock().unwrap() = Some(last_user.content.clone());
        }
    }

    async fn on_llm_output(&self, response: &ChatResponse) {
        if !self.config.enabled {
            return;
        }
        let Some(assistant_text) = response.text.as_ref().filter(|t| !t.is_empty()) else {
            return;
        };

        let user_msg = self.pending_user_msg.lock().unwrap().take();
        let Some(user_text) = user_msg else {
            return;
        };

        self.conversation_buffer
            .lock()
            .unwrap()
            .push((user_text, assistant_text.clone()));
        self.turn_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_heartbeat_tick(&self) {
        if !self.config.enabled {
            return;
        }
        if self.turn_count.load(Ordering::SeqCst) < self.config.extraction_turn_threshold {
            return;
        }
        self.run_extraction().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::traits::MemoryEntry;
    use crate::providers::traits::ChatResponse;
    use std::sync::Mutex as StdMutex;

    // ── Mock Provider ──────────────────────────────────────────────

    struct MockProvider {
        response: StdMutex<String>,
    }

    impl MockProvider {
        fn new(response: &str) -> Self {
            Self {
                response: StdMutex::new(response.into()),
            }
        }

        fn set_response(&self, response: &str) {
            *self.response.lock().unwrap() = response.into();
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        async fn chat_with_system(
            &self,
            _system: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            Ok(self.response.lock().unwrap().clone())
        }
    }

    // ── Mock Memory ────────────────────────────────────────────────

    struct MockMemory {
        entries: Arc<StdMutex<Vec<MemoryEntry>>>,
    }

    impl MockMemory {
        fn new() -> Self {
            Self {
                entries: Arc::new(StdMutex::new(Vec::new())),
            }
        }

        fn entries(&self) -> Vec<MemoryEntry> {
            self.entries.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Memory for MockMemory {
        fn name(&self) -> &str {
            "mock"
        }

        async fn store(
            &self,
            key: &str,
            content: &str,
            category: MemoryCategory,
            session_id: Option<&str>,
        ) -> anyhow::Result<()> {
            self.store_with_metadata(
                key,
                content,
                category,
                session_id,
                1.0,
                MemorySource::Explicit,
            )
            .await
        }

        async fn store_with_metadata(
            &self,
            key: &str,
            content: &str,
            category: MemoryCategory,
            session_id: Option<&str>,
            confidence: f64,
            source: MemorySource,
        ) -> anyhow::Result<()> {
            let mut entries = self.entries.lock().unwrap();
            // Upsert: remove existing entry with same key.
            entries.retain(|e| e.key != key);
            let next_id = entries.len();
            entries.push(MemoryEntry {
                id: format!("mock-{next_id}"),
                key: key.into(),
                content: content.into(),
                category,
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_id: session_id.map(String::from),
                score: None,
                confidence,
                source,
                last_recalled_at: None,
            });
            Ok(())
        }

        async fn recall(
            &self,
            _query: &str,
            _limit: usize,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(self.entries.lock().unwrap().clone())
        }

        async fn get(&self, key: &str) -> anyhow::Result<Option<MemoryEntry>> {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .iter()
                .find(|e| e.key == key)
                .cloned())
        }

        async fn list(
            &self,
            _category: Option<&MemoryCategory>,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(self.entries.lock().unwrap().clone())
        }

        async fn forget(&self, key: &str) -> anyhow::Result<bool> {
            let mut entries = self.entries.lock().unwrap();
            let before = entries.len();
            entries.retain(|e| e.key != key);
            Ok(entries.len() < before)
        }

        async fn count(&self) -> anyhow::Result<usize> {
            Ok(self.entries.lock().unwrap().len())
        }

        async fn health_check(&self) -> bool {
            true
        }
    }

    // ── Helpers ────────────────────────────────────────────────────

    fn default_config() -> HermesConfig {
        HermesConfig {
            enabled: true,
            extraction_turn_threshold: 3,
            max_extractions_per_tick: 10,
            ..HermesConfig::default()
        }
    }

    fn make_hook(
        config: HermesConfig,
        memory: Arc<dyn Memory>,
        provider: Arc<dyn Provider>,
    ) -> HermesExtractionHook {
        let tmp = tempfile::TempDir::new().unwrap();
        HermesExtractionHook::new(
            config,
            memory,
            provider,
            "test-model".into(),
            tmp.path().to_path_buf(),
            Some("test-session".into()),
        )
    }

    fn chat_response(text: &str) -> ChatResponse {
        ChatResponse {
            text: Some(text.into()),
            tool_calls: vec![],
            usage: None,
            reasoning_content: None,
        }
    }

    fn user_messages(content: &str) -> Vec<ChatMessage> {
        vec![
            ChatMessage::system("You are helpful."),
            ChatMessage::user(content),
        ]
    }

    // ── Tests ──────────────────────────────────────────────────────

    #[test]
    fn strip_json_fences_plain() {
        assert_eq!(strip_json_fences(r#"[{"key": "a"}]"#), r#"[{"key": "a"}]"#);
    }

    #[test]
    fn strip_json_fences_with_markers() {
        let input = "```json\n[{\"key\": \"a\"}]\n```";
        assert_eq!(strip_json_fences(input), r#"[{"key": "a"}]"#);
    }

    #[test]
    fn strip_json_fences_bare_backticks() {
        let input = "```\n[]\n```";
        assert_eq!(strip_json_fences(input), "[]");
    }

    #[tokio::test]
    async fn disabled_hook_does_not_buffer() {
        let memory = Arc::new(MockMemory::new());
        let provider = Arc::new(MockProvider::new("[]"));
        let config = HermesConfig {
            enabled: false,
            ..default_config()
        };
        let hook = make_hook(config, memory, provider);

        hook.on_llm_input(&user_messages("hello"), "model").await;
        hook.on_llm_output(&chat_response("hi there")).await;

        assert_eq!(hook.turn_count.load(Ordering::SeqCst), 0);
        assert!(hook.conversation_buffer.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn buffers_conversation_turns() {
        let memory = Arc::new(MockMemory::new());
        let provider = Arc::new(MockProvider::new("[]"));
        let hook = make_hook(default_config(), memory, provider);

        hook.on_llm_input(&user_messages("hello"), "model").await;
        hook.on_llm_output(&chat_response("hi there")).await;

        assert_eq!(hook.turn_count.load(Ordering::SeqCst), 1);
        let buf = hook.conversation_buffer.lock().unwrap();
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].0, "hello");
        assert_eq!(buf[0].1, "hi there");
    }

    #[tokio::test]
    async fn skips_empty_assistant_response() {
        let memory = Arc::new(MockMemory::new());
        let provider = Arc::new(MockProvider::new("[]"));
        let hook = make_hook(default_config(), memory, provider);

        hook.on_llm_input(&user_messages("hello"), "model").await;
        hook.on_llm_output(&ChatResponse {
            text: None,
            tool_calls: vec![],
            usage: None,
            reasoning_content: None,
        })
        .await;

        assert_eq!(hook.turn_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn heartbeat_skips_below_threshold() {
        let memory = Arc::new(MockMemory::new());
        let provider = Arc::new(MockProvider::new("[]"));
        let hook = make_hook(default_config(), memory, provider);

        // Only 2 turns, threshold is 3.
        for _ in 0..2 {
            hook.on_llm_input(&user_messages("q"), "model").await;
            hook.on_llm_output(&chat_response("a")).await;
        }

        hook.on_heartbeat_tick().await;

        // Buffer should NOT be drained.
        assert_eq!(hook.conversation_buffer.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn heartbeat_runs_extraction_at_threshold() {
        let memory: Arc<MockMemory> = Arc::new(MockMemory::new());
        let llm_response = r#"[
            {"key": "user_name", "content": "User is Alice", "category": "knowledge", "confidence": 0.95}
        ]"#;
        let provider = Arc::new(MockProvider::new(llm_response));
        let hook = make_hook(
            default_config(),
            memory.clone() as Arc<dyn Memory>,
            provider,
        );

        // Push 3 turns to meet threshold.
        for i in 0..3 {
            hook.on_llm_input(&user_messages(&format!("question {i}")), "model")
                .await;
            hook.on_llm_output(&chat_response(&format!("answer {i}")))
                .await;
        }
        assert_eq!(hook.turn_count.load(Ordering::SeqCst), 3);

        hook.on_heartbeat_tick().await;

        // Buffer should be drained and turn count reset.
        assert_eq!(hook.turn_count.load(Ordering::SeqCst), 0);
        assert!(hook.conversation_buffer.lock().unwrap().is_empty());

        // Memory should have the extracted fact.
        let entries = memory.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "user_name");
        assert_eq!(entries[0].content, "User is Alice");
        assert!((entries[0].confidence - 0.95).abs() < f64::EPSILON);
        assert_eq!(entries[0].source, MemorySource::Extracted);
    }

    #[tokio::test]
    async fn malformed_json_does_not_crash() {
        let memory = Arc::new(MockMemory::new());
        let provider = Arc::new(MockProvider::new("this is not json"));
        let hook = make_hook(
            default_config(),
            memory.clone() as Arc<dyn Memory>,
            provider,
        );

        for _ in 0..3 {
            hook.on_llm_input(&user_messages("q"), "model").await;
            hook.on_llm_output(&chat_response("a")).await;
        }

        // Should not panic.
        hook.on_heartbeat_tick().await;

        // Buffer drained, no memories stored.
        assert!(hook.conversation_buffer.lock().unwrap().is_empty());
        assert!(memory.entries().is_empty());
    }

    #[tokio::test]
    async fn dedup_skips_lower_confidence() {
        let memory: Arc<MockMemory> = Arc::new(MockMemory::new());
        // Pre-populate with a high-confidence memory.
        memory
            .store_with_metadata(
                "user_lang",
                "User prefers Rust",
                MemoryCategory::Core,
                None,
                0.95,
                MemorySource::Explicit,
            )
            .await
            .unwrap();

        // LLM tries to extract same key with lower confidence.
        let llm_response = r#"[
            {"key": "user_lang", "content": "User likes Rust", "category": "preference", "confidence": 0.7}
        ]"#;
        let provider = Arc::new(MockProvider::new(llm_response));
        let hook = make_hook(
            default_config(),
            memory.clone() as Arc<dyn Memory>,
            provider,
        );

        for _ in 0..3 {
            hook.on_llm_input(&user_messages("q"), "model").await;
            hook.on_llm_output(&chat_response("a")).await;
        }

        hook.on_heartbeat_tick().await;

        // Original memory should be preserved (not overwritten).
        let entry = memory.entries();
        assert_eq!(entry.len(), 1);
        assert_eq!(entry[0].content, "User prefers Rust");
        assert!((entry[0].confidence - 0.95).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn dedup_overwrites_with_higher_confidence() {
        let memory: Arc<MockMemory> = Arc::new(MockMemory::new());
        // Pre-populate with a low-confidence memory.
        memory
            .store_with_metadata(
                "user_lang",
                "User maybe likes Rust",
                MemoryCategory::Core,
                None,
                0.5,
                MemorySource::Extracted,
            )
            .await
            .unwrap();

        // LLM extracts same key with higher confidence.
        let llm_response = r#"[
            {"key": "user_lang", "content": "User definitely prefers Rust", "category": "preference", "confidence": 0.95}
        ]"#;
        let provider = Arc::new(MockProvider::new(llm_response));
        let hook = make_hook(
            default_config(),
            memory.clone() as Arc<dyn Memory>,
            provider,
        );

        for _ in 0..3 {
            hook.on_llm_input(&user_messages("q"), "model").await;
            hook.on_llm_output(&chat_response("a")).await;
        }

        hook.on_heartbeat_tick().await;

        let entry = memory.entries();
        assert_eq!(entry.len(), 1);
        assert_eq!(entry[0].content, "User definitely prefers Rust");
        assert!((entry[0].confidence - 0.95).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn respects_max_extractions_per_tick() {
        let memory: Arc<MockMemory> = Arc::new(MockMemory::new());
        // LLM returns 5 facts, but max is 2.
        let llm_response = r#"[
            {"key": "a", "content": "fact a", "category": "knowledge", "confidence": 0.9},
            {"key": "b", "content": "fact b", "category": "knowledge", "confidence": 0.9},
            {"key": "c", "content": "fact c", "category": "knowledge", "confidence": 0.9},
            {"key": "d", "content": "fact d", "category": "knowledge", "confidence": 0.9},
            {"key": "e", "content": "fact e", "category": "knowledge", "confidence": 0.9}
        ]"#;
        let provider = Arc::new(MockProvider::new(llm_response));
        let config = HermesConfig {
            max_extractions_per_tick: 2,
            ..default_config()
        };
        let hook = make_hook(config, memory.clone() as Arc<dyn Memory>, provider);

        for _ in 0..3 {
            hook.on_llm_input(&user_messages("q"), "model").await;
            hook.on_llm_output(&chat_response("a")).await;
        }

        hook.on_heartbeat_tick().await;

        assert_eq!(memory.entries().len(), 2);
    }

    #[tokio::test]
    async fn skips_low_confidence_facts() {
        let memory: Arc<MockMemory> = Arc::new(MockMemory::new());
        let llm_response = r#"[
            {"key": "strong", "content": "high conf", "category": "knowledge", "confidence": 0.8},
            {"key": "weak", "content": "too low", "category": "knowledge", "confidence": 0.2}
        ]"#;
        let provider = Arc::new(MockProvider::new(llm_response));
        let hook = make_hook(
            default_config(),
            memory.clone() as Arc<dyn Memory>,
            provider,
        );

        for _ in 0..3 {
            hook.on_llm_input(&user_messages("q"), "model").await;
            hook.on_llm_output(&chat_response("a")).await;
        }

        hook.on_heartbeat_tick().await;

        let entries = memory.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "strong");
    }

    #[tokio::test]
    async fn state_file_written_after_extraction() {
        let memory: Arc<MockMemory> = Arc::new(MockMemory::new());
        let llm_response =
            r#"[{"key": "k", "content": "v", "category": "knowledge", "confidence": 0.9}]"#;
        let provider = Arc::new(MockProvider::new(llm_response));
        let tmp = tempfile::TempDir::new().unwrap();

        let hook = HermesExtractionHook::new(
            default_config(),
            memory.clone() as Arc<dyn Memory>,
            provider,
            "test-model".into(),
            tmp.path().to_path_buf(),
            None,
        );

        for _ in 0..3 {
            hook.on_llm_input(&user_messages("q"), "model").await;
            hook.on_llm_output(&chat_response("a")).await;
        }

        hook.on_heartbeat_tick().await;

        let state = read_state(tmp.path());
        assert!(state.last_extraction_at.is_some());
        assert_eq!(state.total_extracted_memories, 1);
    }

    #[tokio::test]
    async fn state_file_accumulates_across_cycles() {
        let memory: Arc<MockMemory> = Arc::new(MockMemory::new());
        let provider = Arc::new(MockProvider::new(
            r#"[{"key": "k1", "content": "v1", "category": "knowledge", "confidence": 0.9}]"#,
        ));
        let tmp = tempfile::TempDir::new().unwrap();

        let hook = HermesExtractionHook::new(
            default_config(),
            memory.clone() as Arc<dyn Memory>,
            provider.clone() as Arc<dyn Provider>,
            "test-model".into(),
            tmp.path().to_path_buf(),
            None,
        );

        // First cycle.
        for _ in 0..3 {
            hook.on_llm_input(&user_messages("q"), "model").await;
            hook.on_llm_output(&chat_response("a")).await;
        }
        hook.on_heartbeat_tick().await;

        // Second cycle with different key.
        provider.set_response(
            r#"[{"key": "k2", "content": "v2", "category": "knowledge", "confidence": 0.9}]"#,
        );
        for _ in 0..3 {
            hook.on_llm_input(&user_messages("q2"), "model").await;
            hook.on_llm_output(&chat_response("a2")).await;
        }
        hook.on_heartbeat_tick().await;

        let state = read_state(tmp.path());
        assert_eq!(state.total_extracted_memories, 2);
    }

    #[tokio::test]
    async fn extraction_with_fenced_json_response() {
        let memory: Arc<MockMemory> = Arc::new(MockMemory::new());
        let llm_response = "```json\n[\n  {\"key\": \"lang\", \"content\": \"Rust\", \"category\": \"preference\", \"confidence\": 0.9}\n]\n```";
        let provider = Arc::new(MockProvider::new(llm_response));
        let hook = make_hook(
            default_config(),
            memory.clone() as Arc<dyn Memory>,
            provider,
        );

        for _ in 0..3 {
            hook.on_llm_input(&user_messages("q"), "model").await;
            hook.on_llm_output(&chat_response("a")).await;
        }

        hook.on_heartbeat_tick().await;

        let entries = memory.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "lang");
    }

    #[tokio::test]
    async fn extraction_model_override_strips_provider_prefix() {
        let config = HermesConfig {
            extraction_model: Some("openrouter/anthropic/claude-haiku-4-5".into()),
            ..default_config()
        };
        let memory = Arc::new(MockMemory::new());
        let provider = Arc::new(MockProvider::new("[]"));
        let tmp = tempfile::TempDir::new().unwrap();

        let hook = HermesExtractionHook::new(
            config,
            memory as Arc<dyn Memory>,
            provider,
            "default-model".into(),
            tmp.path().to_path_buf(),
            None,
        );

        assert_eq!(hook.model, "anthropic/claude-haiku-4-5");
    }

    #[test]
    fn hook_name_and_priority() {
        let memory = Arc::new(MockMemory::new());
        let provider = Arc::new(MockProvider::new("[]"));
        let hook = make_hook(default_config(), memory as Arc<dyn Memory>, provider);
        assert_eq!(hook.name(), "hermes-extraction");
        assert_eq!(hook.priority(), -100);
    }
}
