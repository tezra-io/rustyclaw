# Hermes Memory Enhancement — Design Document

> LLM extraction + confidence scoring + loop detection for RustyClaw
>
> Author: Sujeeth (with Claude)
> Date: 2026-03-28
> Status: Draft
> Prerequisite: [Memory Architecture Analysis](./memory-architecture-analysis.md)

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [Architecture Overview](#2-architecture-overview)
3. [LLM Extraction Hook](#3-llm-extraction-hook)
4. [Confidence Scoring](#4-confidence-scoring)
5. [Loop Detection](#5-loop-detection)
6. [Heartbeat Integration](#6-heartbeat-integration)
7. [Memory Trait Changes](#7-memory-trait-changes)
8. [SQLite Schema Migration](#8-sqlite-schema-migration)
9. [Config Changes](#9-config-changes)
10. [The Complete Hermes Loop](#10-the-complete-hermes-loop)
11. [Testing Strategy](#11-testing-strategy)
12. [Implementation Order](#12-implementation-order)

---

## 1. Problem Statement

RustyClaw's memory system has strong infrastructure — hybrid vector+keyword search, multi-backend storage, security scanning, hygiene, snapshots — but it relies entirely on the agent explicitly calling `memory_store` to persist knowledge. This has three consequences:

**Memory quality depends on agent initiative.** The agent must decide what's worth remembering during the conversation. Insights that emerge from multi-turn exchanges, subtle user preferences, and implicit context are lost unless the agent happens to call `memory_store`.

**No quality signal on stored memories.** All memories are treated equally in recall scoring. A user-stated fact ("I prefer TypeScript") gets the same weight as an auto-saved daily log entry. The `min_relevance_score` threshold in `DefaultMemoryLoader` (`src/agent/memory_loader.rs:13`) filters by search relevance (how well a query matches), not memory quality (how confident we are the memory is correct or important).

**No protection against agent loops.** A malfunctioning agent can make the same tool calls indefinitely. The hook system (`src/hooks/traits.rs`) has `before_tool_call` but no built-in handler that detects repetition.

**Hermes** addresses all three by adding:
- **LLM-driven memory extraction** — automatic fact/preference/context distillation from conversations
- **Confidence scoring** — quality-weighted recall so high-confidence memories surface first
- **Loop detection** — hash-based repetition detection with configurable thresholds

These three features close the gap between "agent stores explicitly" and "agent learns implicitly," completing the self-learning loop outlined in the [architecture analysis](./memory-architecture-analysis.md#3-hermes-self-learning-loop).

---

## 2. Architecture Overview

### Current State

```
User ──→ Channel ──→ Agent Loop ──→ LLM Provider
                        │
                        ├──→ Tool Execution (incl. memory_store)
                        │         │
                        │         ▼
                        │    Memory Trait ──→ SQLite (hybrid search)
                        │                     ├── FTS5 (keyword)
                        │                     ├── Embeddings (vector)
                        │                     └── Security scanning
                        │
                        └──→ Hooks (HookRunner)
                              ├── CommandLoggerHook
                              └── SessionBridgeHook
```

### With Hermes

```
User ──→ Channel ──→ Agent Loop ──→ LLM Provider
                        │
                        ├──→ Tool Execution (incl. memory_store)
                        │         │
                        │         ▼
                        │    Memory Trait ──→ SQLite (hybrid search + confidence)
                        │         ▲              ├── FTS5 (keyword)
                        │         │              ├── Embeddings (vector)
                        │         │              ├── Confidence weighting ←── NEW
                        │         │              └── Security scanning
                        │         │
                        │    Extraction results stored via Memory::store()
                        │         │
                        └──→ Hooks (HookRunner)
                              ├── CommandLoggerHook
                              ├── SessionBridgeHook
                              ├── HermesExtractionHook ←────────── NEW
                              │     ├── on_llm_output: buffer conversation
                              │     ├── on_heartbeat_tick: run extraction
                              │     └── on_heartbeat_tick: run consolidation (daily)
                              └── LoopDetectionHook ←──────────── NEW
                                    └── before_tool_call: hash + detect loops
```

### Data Flow — Extraction Cycle

```
┌─────────────────────────────────────────────────────────────────┐
│                    HermesExtractionHook                          │
│                                                                  │
│  on_llm_output(response)                                        │
│    └─→ Append to conversation_buffer (Arc<Mutex<Vec<String>>>)  │
│         Track turn_count += 1                                   │
│                                                                  │
│  on_heartbeat_tick()                                            │
│    ├─→ IF turn_count >= extraction_turn_threshold (default 3):  │
│    │     1. Drain conversation_buffer                           │
│    │     2. Build extraction prompt (HERMES_EXTRACT_PROMPT)     │
│    │     3. Call configured LLM (default_provider/model or      │
│    │        hermes.extraction_model override)                   │
│    │     4. Parse JSON response → Vec<ExtractedFact>            │
│    │     5. For each fact:                                      │
│    │        a. Security scan (ContentScanner)                   │
│    │        b. memory.store(key, content, Core, session_id)     │
│    │           with confidence from LLM                         │
│    │     6. Reset turn_count and buffer                         │
│    │                                                            │
│    └─→ IF last_consolidation > consolidation_interval (24h):    │
│          1. memory.list(Core, None)                             │
│          2. Build consolidation prompt (HERMES_CONSOLIDATE)     │
│          3. Call LLM → merge/update/decay recommendations       │
│          4. Apply: update confidence, merge entries, forget stale│
│          5. Update last_consolidation timestamp                 │
└─────────────────────────────────────────────────────────────────┘
```

### Data Flow — Loop Detection

```
┌─────────────────────────────────────────────────────────────────┐
│                    LoopDetectionHook                             │
│                                                                  │
│  before_tool_call(name, args)                                   │
│    1. hash = SHA-256(name + canonical_json(args))               │
│    2. Push hash into sliding_window (VecDeque, max = window_size)│
│    3. Count occurrences of this hash in window                  │
│    4. IF count >= warn_threshold (default 3):                   │
│         tracing::warn!("Loop detected: {name} repeated {count}x")│
│    5. IF count >= hard_stop_threshold (default 5):              │
│         return HookResult::Cancel("Loop detected: ...")         │
│    6. ELSE: return HookResult::Continue((name, args))           │
└─────────────────────────────────────────────────────────────────┘
```

---

## 3. LLM Extraction Hook

### 3.1 Location

`src/hooks/builtin/hermes/mod.rs` — New submodule under `src/hooks/builtin/`.

Module structure:
```
src/hooks/builtin/hermes/
├── mod.rs              # HermesExtractionHook impl
├── prompts.rs          # Extraction and consolidation prompt templates
└── types.rs            # ExtractedFact, ConsolidationAction structs
```

Register in `src/hooks/builtin/mod.rs` alongside `CommandLoggerHook` and the session bridge.

### 3.2 Trigger

The hook implements two `HookHandler` trait methods:

**`on_llm_output(&self, response: &ChatResponse)`** — Buffers the assistant's response text. Increments the internal turn counter. This is a void hook (fire-and-forget, parallel dispatch via `HookRunner::fire_llm_output`).

**`on_heartbeat_tick(&self)`** — Checks whether extraction or consolidation should run. This piggybacks on the existing heartbeat cycle (`HookRunner::fire_heartbeat_tick` called from `src/daemon/mod.rs:283`). No new timers or cron jobs.

### 3.3 Debouncing Strategy

Extraction does NOT run on every `on_llm_output`. Instead:

- `on_llm_output` buffers text and increments `turn_count`
- `on_heartbeat_tick` checks `turn_count >= extraction_turn_threshold` (default: 3)
- If threshold not met, extraction is skipped for this tick
- After extraction runs, `turn_count` resets to 0

This means extraction runs at most once per heartbeat interval (default 30min from `HeartbeatConfig::interval_minutes`), and only if there have been at least N conversation turns since the last extraction. For high-traffic agents, the heartbeat interval is the rate limiter. For low-traffic agents, the turn threshold is the rate limiter.

**Why not debounce by wall-clock time like DeerFlow (30s)?** DeerFlow's debounce works because it runs as a synchronous middleware in the agent's request path. RustyClaw's hook system is fire-and-forget — `on_llm_output` returns immediately. The heartbeat is the natural periodic trigger already wired into the daemon loop.

### 3.4 Prompt Design

#### Extraction Prompt (`HERMES_EXTRACT_PROMPT`)

```
You are a memory extraction assistant for an AI agent named RustyClaw.

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
]
```

#### Consolidation Prompt (`HERMES_CONSOLIDATE_PROMPT`)

```
You are a memory consolidation assistant. Review the following memories stored
by an AI agent and recommend maintenance actions.

For each memory, you may recommend:
- "keep": No change needed
- "update_confidence": Adjust the confidence score (provide new value + reason)
- "merge": Merge with another memory (provide target key)
- "forget": This memory is outdated, contradicted, or no longer useful

Rules:
- Contradictory memories: keep the more recent one, forget the older
- Nearly identical memories: merge into one with higher confidence
- Memories with very low confidence (< 0.3) that haven't been recalled: forget
- Time-sensitive context that is clearly outdated: forget

Memories:
{all_core_memories_with_timestamps_and_confidence}

Respond with ONLY a JSON array:
[
  {"key": "...", "action": "keep|update_confidence|merge|forget",
   "new_confidence": 0.0, "merge_into": "other_key", "reason": "..."}
]
```

### 3.5 Model Selection

The extraction LLM call uses RustyClaw's configured provider infrastructure:

1. **Default:** Uses `config.default_provider` + `config.default_model` — the same LLM the agent uses for everything else. No hardcoded model.

2. **Optional override:** `config.memory.hermes.extraction_model` (format: `"provider/model"`, e.g. `"openrouter/anthropic/claude-haiku-4-5"`) allows routing extraction to a cheaper model. Falls back to the default if not set.

3. **Resolution path:** The extraction call goes through the same `Provider` trait (`src/providers/traits.rs`) and provider factory that all other LLM calls use. This means it gets retry/fallback from `ReliabilityConfig`, model routing from `model_routes`, and cost tracking from the cost module.

### 3.6 Hook State

```rust
pub struct HermesExtractionHook {
    // Conversation buffer (thread-safe, append on on_llm_output)
    conversation_buffer: Arc<Mutex<Vec<String>>>,
    turn_count: Arc<AtomicU32>,

    // Extraction config
    extraction_turn_threshold: u32,       // default: 3
    max_extractions_per_tick: usize,      // default: 10
    extraction_model: Option<String>,     // optional cheaper model

    // Consolidation config
    consolidation_interval: Duration,     // default: 24h
    last_consolidation: Arc<Mutex<Instant>>,

    // Dependencies (injected at construction)
    memory: Arc<dyn Memory>,
    provider_factory: Arc<dyn ProviderFactory>,
    scanner: ContentScanner,
    config: HermesConfig,
}
```

---

## 4. Confidence Scoring

### 4.1 Schema Changes

Add `confidence` field to `MemoryEntry` (`src/memory/traits.rs`):

```rust
#[derive(Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub key: String,
    pub content: String,
    pub category: MemoryCategory,
    pub timestamp: String,
    pub session_id: Option<String>,
    pub score: Option<f64>,
    pub confidence: f64,           // NEW — 0.0 to 1.0, default 1.0
}
```

### 4.2 Default Values

| Source | Default Confidence |
|--------|-------------------|
| Explicit `memory_store` tool call | `1.0` (agent chose to remember) |
| Auto-save (raw user input) | `0.5` (useful but unprocessed) |
| Hermes extraction | LLM-assigned (`0.3` – `1.0`) |
| Hermes consolidation update | Adjusted by LLM recommendation |
| Snapshot hydration | Preserve original confidence from snapshot |

### 4.3 Scoring Formula

Current recall scoring in `SqliteMemory` (`src/memory/sqlite.rs`):

```
final_score = vector_weight × vec_score + keyword_weight × kw_score
```

With confidence weighting:

```
final_score = (vector_weight × vec_score + keyword_weight × kw_score) × confidence_boost

where confidence_boost = 0.7 + 0.3 × confidence
```

This means:
- `confidence = 1.0` → `boost = 1.0` (no change, full score)
- `confidence = 0.5` → `boost = 0.85` (slight penalty)
- `confidence = 0.0` → `boost = 0.7` (30% penalty, but still searchable)

The boost is multiplicative so it modulates relevance without dominating it. A highly relevant low-confidence memory still outranks an irrelevant high-confidence one.

The `0.7` floor and `0.3` range are configurable via `config.memory.hermes.confidence_weight` (default: `0.3`). The formula becomes:

```
confidence_boost = (1.0 - confidence_weight) + confidence_weight × confidence
```

### 4.4 Confidence Decay (Optional)

If `config.memory.hermes.confidence_decay_enabled` is true (default: false):

- On each consolidation pass, memories that haven't been recalled since the last consolidation have their confidence reduced:
  ```
  new_confidence = confidence × decay_factor
  ```
  where `decay_factor` defaults to `0.95` (5% decay per consolidation cycle).

- Memories recalled since the last consolidation are boosted:
  ```
  new_confidence = min(1.0, confidence × 1.05)
  ```

- Memories below `decay_floor` (default: `0.1`) are candidates for automatic cleanup.

Implementation: add `last_recalled_at` column to the SQLite schema. Update on every `recall()` hit. The consolidation pass reads this to decide decay vs. boost.

---

## 5. Loop Detection

### 5.1 Location

`src/hooks/builtin/loop_detection.rs` — Standalone hook, not part of the Hermes submodule (loop detection is a safety feature, not a memory feature).

Register in `src/hooks/builtin/mod.rs`.

### 5.2 Hook Design

```rust
pub struct LoopDetectionHook {
    /// Sliding window of recent tool call hashes
    window: Arc<Mutex<VecDeque<u64>>>,
    /// Max window size
    window_size: usize,         // default: 20
    /// Warn after this many repetitions
    warn_threshold: u32,        // default: 3
    /// Hard stop after this many repetitions
    hard_stop_threshold: u32,   // default: 5
}
```

### 5.3 Hash Strategy

```rust
fn hash_tool_call(name: &str, args: &serde_json::Value) -> u64 {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(b"|");
    // Canonical JSON: sorted keys, no whitespace
    let canonical = serde_json::to_string(args).unwrap_or_default();
    hasher.update(canonical.as_bytes());
    let hash = hasher.finalize();
    u64::from_be_bytes(hash[..8].try_into().unwrap())
}
```

Using the first 8 bytes of SHA-256 for the hash (matching the pattern in `SqliteMemory::content_hash` at `src/memory/sqlite.rs:211`). Collision probability is negligible for a 20-element sliding window.

**Note on `serde_json::to_string`:** This produces deterministic output for the same `Value`, but key ordering depends on whether the `Value` was parsed from a map (insertion-ordered) or constructed programmatically. For the purposes of loop detection, this is sufficient — we're hashing the exact `Value` the agent produced, and a loop means producing the exact same `Value` repeatedly.

### 5.4 Thresholds

| Threshold | Default | Action |
|-----------|---------|--------|
| `warn_threshold` | 3 | `tracing::warn!` log entry |
| `hard_stop_threshold` | 5 | `HookResult::Cancel` — tool call blocked |

The `Cancel` message is explicit and actionable:

```
Loop detected: tool '{tool_name}' called {count} times with identical arguments
in the last {window_size} tool calls. Stopping to prevent infinite loop.
Please try a different approach.
```

### 5.5 Hook Implementation

```rust
#[async_trait]
impl HookHandler for LoopDetectionHook {
    fn name(&self) -> &str { "loop_detection" }
    fn priority(&self) -> i32 { 100 }  // High priority — run before other hooks

    async fn before_tool_call(
        &self,
        name: String,
        args: Value,
    ) -> HookResult<(String, Value)> {
        let hash = hash_tool_call(&name, &args);
        let mut window = self.window.lock();

        window.push_back(hash);
        if window.len() > self.window_size {
            window.pop_front();
        }

        let count = window.iter().filter(|&&h| h == hash).count();

        if count >= self.hard_stop_threshold as usize {
            return HookResult::Cancel(format!(
                "Loop detected: tool '{}' called {} times with identical arguments \
                 in the last {} tool calls. Stopping to prevent infinite loop. \
                 Please try a different approach.",
                name, count, self.window_size
            ));
        }

        if count >= self.warn_threshold as usize {
            tracing::warn!(
                tool = %name,
                count,
                window_size = self.window_size,
                "Possible loop: repeated tool call detected"
            );
        }

        HookResult::Continue((name, args))
    }
}
```

Priority `100` ensures loop detection runs before other `before_tool_call` hooks. This matches the pattern used by `HookRunner` — handlers sorted by descending priority (`src/hooks/runner.rs:34`).

---

## 6. Heartbeat Integration

### 6.1 Current Heartbeat Architecture

The heartbeat system consists of:

1. **`HeartbeatConfig`** (`src/config/schema.rs:2456`) — `enabled`, `interval_minutes` (default 30), optional `message`, `target`, `to`
2. **`HeartbeatEngine`** (`src/heartbeat/engine.rs`) — Reads `HEARTBEAT.md`, parses `- ` prefixed tasks, runs on interval
3. **`HookHandler::on_heartbeat_tick`** (`src/hooks/traits.rs:40`) — Void hook fired on each tick
4. **`HookRunner::fire_heartbeat_tick`** (`src/hooks/runner.rs:110`) — Parallel dispatch to all registered handlers
5. **Daemon wiring** (`src/daemon/mod.rs:283`) — The daemon's heartbeat loop calls `engine.collect_tasks()` and fires hooks

### 6.2 How Hermes Hooks In

Hermes extraction and consolidation are triggered **solely** by `on_heartbeat_tick`. No new timers, no new cron jobs, no new daemon loops.

```
Daemon heartbeat loop (every interval_minutes)
  │
  ├── engine.collect_tasks()       ← existing: reads HEARTBEAT.md
  ├── hooks.fire_heartbeat_tick()  ← existing: fires all handlers
  │     │
  │     ├── HermesExtractionHook::on_heartbeat_tick()   ← NEW
  │     │     ├── Check turn_count >= threshold → extract
  │     │     └── Check last_consolidation age → consolidate
  │     │
  │     ├── LoopDetectionHook::on_heartbeat_tick()      ← no-op
  │     ├── CommandLoggerHook::on_heartbeat_tick()       ← no-op
  │     └── SessionBridgeHook::on_heartbeat_tick()       ← no-op
  │
  └── Process heartbeat tasks      ← existing: send to channel
```

### 6.3 Extraction Frequency

With the default `HeartbeatConfig::interval_minutes = 30` and `extraction_turn_threshold = 3`:

- Extraction runs at most every 30 minutes
- Only runs if 3+ conversation turns have occurred since the last extraction
- In practice, active conversations will trigger extraction every 30min; idle agents will never trigger

### 6.4 Consolidation Frequency

Consolidation runs during `on_heartbeat_tick` but has its own interval gate:

- Default: every 24 hours (`consolidation_interval_hours = 24`)
- Tracked via `last_consolidation: Instant` in hook state
- Wall-clock independent — survives daemon restarts via state file (same pattern as `memory_hygiene_state.json` at `src/memory/hygiene.rs:32`)

### 6.5 Heartbeat Must Be Enabled

Hermes extraction requires `[heartbeat] enabled = true`. If the heartbeat is disabled, extraction and consolidation do not run. This is intentional — the agent operates in explicit-only memory mode.

The config validation will warn if `[memory.hermes] enabled = true` but `[heartbeat] enabled = false`.

---

## 7. Memory Trait Changes

### 7.1 MemoryEntry

```rust
// src/memory/traits.rs

#[derive(Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub key: String,
    pub content: String,
    pub category: MemoryCategory,
    pub timestamp: String,
    pub session_id: Option<String>,
    pub score: Option<f64>,
    pub confidence: f64,                // NEW
    pub source: MemorySource,           // NEW
}

/// How this memory was created — affects default confidence and display
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    /// Agent explicitly called memory_store tool
    Explicit,
    /// Auto-saved from user input
    AutoSave,
    /// Extracted by Hermes LLM analysis
    Extracted,
    /// Restored from MEMORY_SNAPSHOT.md
    Hydrated,
}

impl Default for MemorySource {
    fn default() -> Self {
        Self::Explicit
    }
}
```

### 7.2 Memory Trait

The `Memory` trait (`src/memory/traits.rs:56`) does NOT change its method signatures. The `store()` method signature remains:

```rust
async fn store(
    &self,
    key: &str,
    content: &str,
    category: MemoryCategory,
    session_id: Option<&str>,
) -> anyhow::Result<()>;
```

Confidence and source are passed through a new optional method with a default implementation:

```rust
#[async_trait]
pub trait Memory: Send + Sync {
    // ... existing methods unchanged ...

    /// Store with explicit confidence and source metadata.
    /// Default: delegates to store() with confidence=1.0, source=Explicit.
    async fn store_with_metadata(
        &self,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
        confidence: f64,
        source: MemorySource,
    ) -> anyhow::Result<()> {
        // Default implementation ignores metadata — backwards compatible
        self.store(key, content, category, session_id).await
    }
}
```

This preserves backward compatibility: all existing `Memory` implementations continue to work. Only SQLite (and later Postgres/Qdrant if desired) need to override `store_with_metadata` to persist the new fields.

### 7.3 Recall Changes

No signature changes to `recall()`. The confidence weighting is applied **inside** the SQLite backend's `recall()` implementation, in the hybrid merge step. Other backends that don't store confidence simply return entries with `confidence: 1.0` (the default).

### 7.4 MemoryCategory

No changes. Extracted memories use `MemoryCategory::Core`. The extraction category from the LLM response ("preference", "knowledge", etc.) is stored in the `content` field as metadata or as a tag prefix, not as a new `MemoryCategory` variant. This avoids proliferating categories that only Hermes understands.

---

## 8. SQLite Schema Migration

### 8.1 Migration SQL

Applied in `SqliteMemory::init_schema()` (`src/memory/sqlite.rs:128`), using the same safe migration pattern as the existing `session_id` migration (line 174):

```sql
-- Migration: add confidence column (default 1.0 for all existing memories)
ALTER TABLE memories ADD COLUMN confidence REAL NOT NULL DEFAULT 1.0;

-- Migration: add source column (default 'explicit' for all existing memories)
ALTER TABLE memories ADD COLUMN source TEXT NOT NULL DEFAULT 'explicit';

-- Migration: add last_recalled_at for confidence decay tracking
ALTER TABLE memories ADD COLUMN last_recalled_at TEXT;

-- Index for consolidation queries (find low-confidence + stale memories)
CREATE INDEX IF NOT EXISTS idx_memories_confidence ON memories(confidence);
```

### 8.2 Migration Detection

Same pattern as the existing `session_id` migration:

```rust
let has_confidence: bool = conn
    .prepare("SELECT sql FROM sqlite_master WHERE type='table' AND name='memories'")?
    .query_row([], |row| row.get::<_, String>(0))?
    .contains("confidence");

if !has_confidence {
    conn.execute_batch(
        "ALTER TABLE memories ADD COLUMN confidence REAL NOT NULL DEFAULT 1.0;
         ALTER TABLE memories ADD COLUMN source TEXT NOT NULL DEFAULT 'explicit';
         ALTER TABLE memories ADD COLUMN last_recalled_at TEXT;
         CREATE INDEX IF NOT EXISTS idx_memories_confidence ON memories(confidence);",
    )?;
}
```

### 8.3 Snapshot Format Update

`src/memory/snapshot.rs` — `export_snapshot()` and `hydrate_from_snapshot()` need to include confidence and source in the markdown format:

```markdown
## favorite_language
- **Content:** Rust
- **Confidence:** 0.95
- **Source:** extracted
- **Created:** 2026-03-15T12:00:00Z
- **Updated:** 2026-03-28T08:00:00Z
```

Parsing must handle both old format (no confidence/source lines) and new format. Missing fields default to `confidence: 1.0`, `source: hydrated`.

---

## 9. Config Changes

### 9.1 New Config Section

Add to `MemoryConfig` in `src/config/schema.rs`:

```rust
/// Hermes memory enhancement configuration
#[serde(default)]
pub hermes: HermesConfig,
```

### 9.2 HermesConfig Struct

```rust
/// Configuration for the Hermes memory enhancement system.
/// Requires `[heartbeat] enabled = true` for extraction and consolidation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HermesConfig {
    /// Enable Hermes LLM-driven memory extraction
    pub enabled: bool,

    /// Optional model for extraction (format: "provider/model").
    /// Falls back to default_provider + default_model if not set.
    /// Use a cheaper model here to reduce cost (e.g., "openrouter/anthropic/claude-haiku-4-5").
    #[serde(default)]
    pub extraction_model: Option<String>,

    /// Minimum conversation turns before extraction runs (default: 3)
    #[serde(default = "default_extraction_turn_threshold")]
    pub extraction_turn_threshold: u32,

    /// Max memories to extract per heartbeat tick (default: 10)
    #[serde(default = "default_max_extractions_per_tick")]
    pub max_extractions_per_tick: usize,

    /// Hours between consolidation passes (default: 24)
    #[serde(default = "default_consolidation_interval_hours")]
    pub consolidation_interval_hours: u32,

    /// Weight of confidence in recall scoring (0.0–1.0, default: 0.3)
    /// Higher values make confidence more influential in search results.
    #[serde(default = "default_confidence_weight")]
    pub confidence_weight: f64,

    /// Enable confidence decay for unreinforced memories (default: false)
    #[serde(default)]
    pub confidence_decay_enabled: bool,

    /// Decay factor per consolidation cycle (default: 0.95)
    #[serde(default = "default_decay_factor")]
    pub decay_factor: f64,

    /// Minimum confidence before memory is eligible for auto-cleanup (default: 0.1)
    #[serde(default = "default_decay_floor")]
    pub decay_floor: f64,
}

impl Default for HermesConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            extraction_model: None,
            extraction_turn_threshold: 3,
            max_extractions_per_tick: 10,
            consolidation_interval_hours: 24,
            confidence_weight: 0.3,
            confidence_decay_enabled: false,
            decay_factor: 0.95,
            decay_floor: 0.1,
        }
    }
}
```

### 9.3 Loop Detection Config

Add to `HooksConfig` (which already has `builtin: BuiltinHooksConfig`):

```rust
/// Built-in hook toggles
pub struct BuiltinHooksConfig {
    pub command_logger: bool,
    pub session_bridge: bool,
    pub loop_detection: bool,       // NEW — default: true
}
```

Loop detection config:

```rust
/// Loop detection hook configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LoopDetectionConfig {
    /// Sliding window size for tracking recent tool calls (default: 20)
    #[serde(default = "default_loop_window_size")]
    pub window_size: usize,

    /// Log a warning after this many identical calls (default: 3)
    #[serde(default = "default_loop_warn_threshold")]
    pub warn_threshold: u32,

    /// Hard-stop (cancel) after this many identical calls (default: 5)
    #[serde(default = "default_loop_hard_stop_threshold")]
    pub hard_stop_threshold: u32,
}

impl Default for LoopDetectionConfig {
    fn default() -> Self {
        Self {
            window_size: 20,
            warn_threshold: 3,
            hard_stop_threshold: 5,
        }
    }
}
```

### 9.4 Example config.toml

```toml
[heartbeat]
enabled = true
interval_minutes = 30

[memory]
backend = "sqlite"

[memory.hermes]
enabled = true
# extraction_model = "openrouter/anthropic/claude-haiku-4-5"  # optional cheap model
extraction_turn_threshold = 3
max_extractions_per_tick = 10
consolidation_interval_hours = 24
confidence_weight = 0.3
confidence_decay_enabled = false

[agent.hooks]
enabled = true

[agent.hooks.builtin]
loop_detection = true

[agent.hooks.loop_detection]
window_size = 20
warn_threshold = 3
hard_stop_threshold = 5
```

---

## 10. The Complete Hermes Loop

All pieces wired together in a single cycle:

```
                          ┌──────────────────────┐
                          │   1. EXPERIENCE       │
                          │   User ↔ Agent chat   │
                          └──────────┬───────────┘
                                     │
                     on_llm_output() │ (buffer conversation turns)
                                     ▼
                          ┌──────────────────────┐
                          │   2. CAPTURE          │
                          │                       │
                          │ a) Explicit:          │
                          │    memory_store tool  │
                          │    confidence = 1.0   │
                          │                       │
                          │ b) Extracted (Hermes): │
                          │    on_heartbeat_tick  │
                          │    LLM extracts facts │
                          │    confidence = LLM   │
                          │                       │
                          │ Both → Memory::store  │
                          │ → Security scan       │
                          │ → Embedding           │
                          │ → FTS5 index          │
                          └──────────┬───────────┘
                                     │
                          ┌──────────▼───────────┐
                          │   3. REFLECT          │
                          │   (daily, heartbeat)  │
                          │                       │
                          │ Load all Core memories│
                          │ LLM consolidation:    │
                          │ - Merge duplicates    │
                          │ - Resolve conflicts   │
                          │ - Decay stale entries │
                          │ - Boost reinforced    │
                          └──────────┬───────────┘
                                     │
                          ┌──────────▼───────────┐
                          │   4. PRUNE            │
                          │                       │
                          │ a) Hygiene (existing):│
                          │    Archive old files  │
                          │    Purge old archives │
                          │    Prune conversations│
                          │                       │
                          │ b) Confidence decay:  │
                          │    Unreinforced → 0.95│
                          │    Below floor → purge│
                          └──────────┬───────────┘
                                     │
                          ┌──────────▼───────────┐
                          │   5. SNAPSHOT         │
                          │   (existing, on       │
                          │    hygiene cycle)      │
                          │                       │
                          │ Export Core memories   │
                          │ to MEMORY_SNAPSHOT.md  │
                          │ Now includes:          │
                          │ - confidence scores    │
                          │ - source attribution   │
                          └──────────┬───────────┘
                                     │
                          ┌──────────▼───────────┐
                          │   6. RECALL           │
                          │   (existing, enhanced)│
                          │                       │
                          │ Hybrid search:        │
                          │ score = (vec*w + kw*w)│
                          │        × conf_boost   │
                          │                       │
                          │ High-confidence facts  │
                          │ surface first          │
                          └──────────┬───────────┘
                                     │
                          ┌──────────▼───────────┐
                          │   7. APPLY            │
                          │   (existing)          │
                          │                       │
                          │ DefaultMemoryLoader   │
                          │ injects recalled      │
                          │ memories into prompt  │
                          │                       │
                          │ Agent uses knowledge  │
                          │ to improve response   │
                          └──────────┬───────────┘
                                     │
                                     └─────────────→ Back to 1. EXPERIENCE
```

### Safety Rail: Loop Detection

Orthogonal to the memory loop, the `LoopDetectionHook` runs on every `before_tool_call`:

```
Agent decides to call tool
  │
  ▼
HookRunner::run_before_tool_call()
  │
  ├── LoopDetectionHook (priority 100, runs first)
  │     ├── Hash (tool_name, args)
  │     ├── Check sliding window
  │     ├── count >= 5 → Cancel (hard stop)
  │     ├── count >= 3 → Warn (log)
  │     └── else → Continue
  │
  └── Other hooks...
        └── Tool executes (or is cancelled)
```

---

## 11. Testing Strategy

### 11.1 Unit Tests

**HermesExtractionHook:**
- `on_llm_output` correctly buffers and increments turn count
- `on_heartbeat_tick` skips extraction when `turn_count < threshold`
- `on_heartbeat_tick` runs extraction when threshold met, resets counter
- Extraction prompt includes existing memories (dedup check)
- Consolidation skipped when interval not elapsed
- Consolidation runs when interval elapsed, updates state
- Malformed LLM JSON response → logged, not crashed
- Empty extraction results → no store calls

**LoopDetectionHook:**
- Unique calls pass through (window fills without repeats)
- Warn threshold triggers log at correct count
- Hard stop threshold returns `HookResult::Cancel`
- Window evicts oldest entries beyond `window_size`
- Different args for same tool name = different hashes
- Same args for different tool names = different hashes

**Confidence scoring:**
- `MemoryEntry` round-trip serialization with confidence field
- Confidence boost formula: verify `confidence_boost(1.0) = 1.0`, `confidence_boost(0.0) = 0.7`
- Recall results ordered by boosted score
- Default confidence `1.0` for explicit store, `0.5` for auto-save

**Schema migration:**
- Fresh DB creates schema with confidence column
- Existing DB without confidence → migration adds it with default 1.0
- Existing DB with confidence → migration is idempotent

### 11.2 Integration Tests

- End-to-end extraction: buffer 3 turns → heartbeat tick → verify extracted entries in SQLite
- Consolidation: store conflicting memories → consolidate → verify merge/forget
- Loop detection in agent loop: mock tool that always returns same result → verify hard stop after 5 calls
- Confidence in recall: store two memories with different confidence → recall → verify ordering
- Snapshot round-trip: store with confidence → export → hydrate → verify confidence preserved

### 11.3 Mock Strategy

- **LLM calls**: Mock the `Provider` trait to return canned JSON responses for extraction/consolidation prompts
- **Memory backend**: Use `SqliteMemory::new()` with `tempfile::TempDir` (same pattern as existing tests in `src/memory/hygiene.rs:383`)
- **ContentScanner**: Real scanner (it's stateless, no external deps)
- **Time**: Use `tokio::time::pause()` for heartbeat interval testing

### 11.4 Test File Locations

| Component | Test Location |
|-----------|--------------|
| `HermesExtractionHook` | `src/hooks/builtin/hermes/mod.rs` (inline `#[cfg(test)]`) |
| `LoopDetectionHook` | `src/hooks/builtin/loop_detection.rs` (inline `#[cfg(test)]`) |
| `ExtractedFact` parsing | `src/hooks/builtin/hermes/types.rs` (inline) |
| Confidence in `MemoryEntry` | `src/memory/traits.rs` (extend existing tests) |
| Confidence in `SqliteMemory` | `src/memory/sqlite.rs` (extend existing tests) |
| Schema migration | `src/memory/sqlite.rs` (new migration test) |
| Snapshot with confidence | `src/memory/snapshot.rs` (extend existing tests) |
| Config parsing | `src/config/schema.rs` (extend existing `#[test]` block) |
| Integration: extraction e2e | `tests/hermes_integration.rs` (new file) |

---

## 12. Implementation Order

### Phase 1: Foundation (confidence + schema)

**What:** Add `confidence`, `source`, `last_recalled_at` to `MemoryEntry` and SQLite schema. Update `store()`, `recall()`, and snapshot to handle the new fields.

**Files:**
- `src/memory/traits.rs` — Add fields to `MemoryEntry`, add `MemorySource` enum, add `store_with_metadata()` default method
- `src/memory/sqlite.rs` — Schema migration, update `store()` to write confidence/source, update recall to apply confidence boost
- `src/memory/snapshot.rs` — Export/import confidence+source
- `src/memory/scanning.rs` — Pass through `store_with_metadata` to inner backend
- `src/memory/lucid.rs` — Pass through `store_with_metadata`
- `src/memory/markdown.rs` — Default impl (ignore metadata)
- `src/memory/none.rs` — Default impl
- `src/config/schema.rs` — Add `HermesConfig` to `MemoryConfig`
- `src/tools/memory_store.rs` — Set `confidence: 1.0`, `source: Explicit` on explicit stores

**Validates:** `cargo test` (all existing tests pass, new confidence tests pass)

### Phase 2: Loop Detection

**What:** Implement `LoopDetectionHook`. Wire into `HookRunner` registration.

**Files:**
- `src/hooks/builtin/loop_detection.rs` — New file, full hook implementation
- `src/hooks/builtin/mod.rs` — Export `LoopDetectionHook`
- `src/config/schema.rs` — Add `loop_detection: bool` to `BuiltinHooksConfig`, add `LoopDetectionConfig`
- `src/channels/mod.rs` — Register hook in `HookRunner` setup (around line 3401)

**Validates:** `cargo test`, manual test with a loop-inducing prompt

### Phase 3: Extraction Hook

**What:** Implement `HermesExtractionHook` with conversation buffering and LLM extraction on heartbeat tick.

**Files:**
- `src/hooks/builtin/hermes/mod.rs` — Hook implementation
- `src/hooks/builtin/hermes/prompts.rs` — Prompt templates
- `src/hooks/builtin/hermes/types.rs` — `ExtractedFact`, `ConsolidationAction` structs
- `src/hooks/builtin/mod.rs` — Export hermes module
- `src/channels/mod.rs` — Register hook in `HookRunner` setup

**Validates:** `cargo test`, manual test with daemon running (heartbeat enabled)

### Phase 4: Consolidation

**What:** Add the consolidation pass to `on_heartbeat_tick` — periodic review and maintenance of Core memories.

**Files:**
- `src/hooks/builtin/hermes/mod.rs` — Add consolidation logic to `on_heartbeat_tick`
- `src/hooks/builtin/hermes/prompts.rs` — Add `HERMES_CONSOLIDATE_PROMPT`
- State persistence: write `hermes_state.json` alongside `memory_hygiene_state.json`

**Validates:** `cargo test`, integration test with mock LLM

### Phase 5: Confidence Decay (Optional)

**What:** If enabled, decay confidence for unreinforced memories during consolidation. Boost for recalled memories.

**Files:**
- `src/memory/sqlite.rs` — Update `last_recalled_at` on recall hits
- `src/hooks/builtin/hermes/mod.rs` — Decay/boost logic in consolidation pass

**Validates:** `cargo test`, time-based integration test

---

## Appendix A: Hermes State File

`{workspace}/state/hermes_state.json`:

```json
{
  "last_extraction_at": "2026-03-28T20:30:00Z",
  "last_consolidation_at": "2026-03-28T08:00:00Z",
  "total_extractions": 42,
  "total_consolidations": 7,
  "total_extracted_memories": 156
}
```

## Appendix B: Observability

New Prometheus metrics (extend `src/observability/prometheus.rs`):

| Metric | Type | Description |
|--------|------|-------------|
| `rustyclaw_hermes_extractions_total` | Counter | Total extraction runs |
| `rustyclaw_hermes_extracted_memories_total` | Counter | Total memories extracted |
| `rustyclaw_hermes_consolidations_total` | Counter | Total consolidation runs |
| `rustyclaw_hermes_extraction_duration_seconds` | Histogram | Extraction LLM call duration |
| `rustyclaw_loop_detections_total` | Counter | Loop detections (warn + hard_stop) |
| `rustyclaw_loop_hard_stops_total` | Counter | Hard stop cancellations |

## Appendix C: Cost Estimation

Per extraction call (assuming Claude Haiku-class model):
- Input: ~500 tokens (prompt) + ~2000 tokens (conversation buffer) = ~2500 tokens
- Output: ~200 tokens (JSON array of facts)
- Cost at $0.25/M input, $1.25/M output: ~$0.0009 per extraction

With default settings (every 30min, only when active):
- Active 8 hours/day → ~16 extractions/day → ~$0.014/day
- Monthly: ~$0.42

Consolidation (once daily):
- Input: ~500 tokens (prompt) + ~5000 tokens (all Core memories) = ~5500 tokens
- Output: ~500 tokens (JSON actions)
- Cost: ~$0.002/day → ~$0.06/month

**Total incremental cost: ~$0.50/month** with a Haiku-class extraction model.
