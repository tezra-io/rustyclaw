# Learning and Personalization — Deep Design

**Status:** Revised after code-level review (implementation-ready for phased delivery)
**Last updated:** 2026-02-19
**Scope:** Learning (background extraction) + Personalization (request-path adaptation)

---

## Overview

RustyClaw already has a strong memory substrate (`Memory` trait + SQLite/Markdown/Lucid backends). What is missing is:

- **Learning**: extract durable user facts/preferences from stored interaction data.
- **Personalization**: inject the right subset of those memories into each request.

This design keeps trait boundaries intact and avoids schema churn by using `MemoryCategory::Custom(...)` values.

---

## Code Reality Check (Current Repository)

These are the constraints this design must respect:

1. `MemoryCategory` currently has `Core`, `Daily`, `Conversation`, and `Custom(String)` only (`src/memory/traits.rs`).
2. `build_context()` exists in `src/agent/loop_.rs` and is used by CLI/single-message flows.
3. Persistent agents do **not** use `build_context()`; they use `DefaultMemoryLoader::load_context()` in `src/agent/memory_loader.rs` via `src/agent/agent.rs`.
4. `loop_.rs` auto-saves user turns to `Conversation` with unique keys (`user_msg_<uuid>`), and assistant replies to `Daily` with truncated summaries.
5. `Agent::turn()` currently auto-saves with fixed keys (`user_msg`, `assistant_resp`), which upsert in SQLite and overwrite older rows (`src/agent/agent.rs`).
6. Daemon (`src/daemon/mod.rs`) has a component supervisor, but it does **not** hold a shared `Arc<dyn Memory>` or provider instance to hand to a Learning worker.
7. `recall()` returns scored entries; `list()` returns entries with `score: None` (SQLite and Markdown behavior).

Implication: a realistic LP design must handle two agent paths, backend differences, and current autosave shapes.

---

## Part 1: Learning (L)

### 1.1 Learning Scope (V1)

Ship **L1 only** first:

- Extract durable facts and explicit preferences from recent conversation memory.
- Store in custom categories.
- Maintain a watermark so extraction is idempotent enough for crash/retry.

Defer L2/L3 synthesis until L1 quality is stable.

### 1.2 Categories and Keys (No Trait Changes)

Use existing `Custom(String)`:

- `Custom("fact")`
- `Custom("preference")`
- `Custom("user_model")`
- `Custom("learning_meta")`
- `Custom("personalization_feedback")` (for the closed loop)

Use deterministic key namespaces:

- `fact::<sha256_16>`
- `preference::<sha256_16>`
- `learning_meta::watermark`
- `learning_meta::session_end::<uuid>`
- `personalization_feedback::<preference_key>`

This avoids enum churn and works on SQLite/Lucid immediately.

### 1.3 Session Boundary Detection

#### CLI / one-shot flow (`src/agent/loop_.rs::run`)

Write a session-end marker after response handling, when `auto_save` is enabled:

```rust
let key = format!("learning_meta::session_end::{}", Uuid::new_v4());
let payload = format!(r#"{{"ended_at":"{}"}}"#, chrono::Utc::now().to_rfc3339());
let _ = mem
    .store(&key, &payload, MemoryCategory::Custom("learning_meta".into()))
    .await;
```

#### Channel/persistent agent flow

There is no natural process exit boundary. Use idle-gap boundary (`session_idle_secs`) based on latest `Conversation` timestamp after autosave key behavior is fixed (see prerequisite below).

### 1.4 Critical Prerequisite for Persistent Agents

Before daemon-mode learning is reliable, fix `src/agent/agent.rs::turn` autosave keys:

- Today: `store("user_msg", ...)` and `store("assistant_resp", ...)` overwrite in SQLite.
- Needed: unique keys (same `autosave_memory_key()` pattern as `loop_.rs`) so learning has a real corpus.

Without this, background learning for persistent SQLite agents only sees the latest row.

### 1.5 Worker Placement and Wiring

Add `src/learning/worker.rs` with a polling loop.

For daemon wiring (`src/daemon/mod.rs`): create memory/provider **inside** the supervised closure from `Config`; do not assume an existing shared `mem` object.

```rust
spawn_component_supervisor("learning", initial_backoff, max_backoff, move || {
    let cfg = learning_cfg.clone();
    async move {
        let mem: Arc<dyn Memory> = Arc::from(crate::memory::create_memory(
            &cfg.memory,
            &cfg.workspace_dir,
            cfg.api_key.as_deref(),
        )?);
        crate::learning::worker::run(cfg, mem).await
    }
});
```

Guardrails:

- If backend is `markdown` or `none`: log once and skip worker start.
- If persistent-agent autosave prerequisite is not met: do not claim daemon learning is fully supported.

### 1.6 Watermark Contract

Watermark key: `learning_meta::watermark` in `Custom("learning_meta")`.

Payload:

```json
{
  "last_processed_ts": "2026-02-19T14:00:00Z",
  "last_l1_run_ts": "2026-02-19T14:01:12Z"
}
```

### 1.7 Extraction Input Assembly

Input source for L1:

- `mem.list(Some(&MemoryCategory::Conversation))`
- filter to entries newer than watermark
- drop session marker keys (`learning_meta::session_end::*`)
- drop very short/noisy entries

Important current limitation:

- `Conversation` currently contains mostly user messages.
- assistant outputs are in `Daily` and truncated.

So V1 learning should focus on explicit user-stated facts/preferences, not dialogue-structure inference.

### 1.8 L1 Extraction Prompt (Structured, Parse-Friendly)

Use strict JSON output with fixed schema and numeric confidence.

```text
System:
You extract durable user facts and interaction preferences from chat logs.

Only use USER-authored content.
Ignore assistant text, tool outputs, and temporary task instructions.
If uncertain, omit.

Return JSON only, no markdown:
{
  "facts": [
    {"text": "...", "confidence": 0.0}
  ],
  "preferences": [
    {"text": "...", "kind": "explicit", "confidence": 0.0}
  ]
}

Rules:
- confidence must be 0.0 to 1.0
- kind must be "explicit" or "implicit"
- max 8 facts and 8 preferences
- if none: {"facts":[],"preferences":[]}

User:
<conversation_transcript>
```

Post-parse validation:

- reject non-JSON / invalid schema
- clamp confidence to `[0.0, 1.0]`
- discard entries below threshold (e.g. `< 0.7` for V1)
- sanitize control tokens before store

### 1.9 Dedup and Contradictions (Pragmatic V1)

Do not add an extra LLM call for contradiction checks in V1.

Use deterministic heuristics:

1. Canonicalize text (lowercase, trim, collapse whitespace).
2. Check exact/near duplicate against existing `fact`/`preference` entries.
3. For known dimensions (verbosity, format, language preference), replace by stable key.

Example stable keys:

- `preference::verbosity`
- `preference::format`
- `preference::code_presentation`

For unknown preference dimensions, use hash-based keys and allow multiple entries.

### 1.10 Learning Config

Add to `Config` (`src/config/schema.rs`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningConfig {
    #[serde(default)]
    pub enabled: bool, // default false
    #[serde(default = "default_learning_poll_secs")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_session_idle_secs")]
    pub session_idle_secs: u64,
    #[serde(default)]
    pub extraction_model: Option<String>,
    #[serde(default = "default_learning_daily_limit")]
    pub max_extractions_per_day: u32,
}
```

Default recommendation:

- `enabled = false` (opt-in)
- `poll_interval_secs = 300`
- `session_idle_secs = 600`
- `max_extractions_per_day = 3`

---

## Part 2: Personalization (P)

### 2.1 One Engine, Two Call Sites

Personalization logic should live once (new module), then be called from both paths:

1. `src/agent/loop_.rs` (CLI, one-shot, `process_message`)
2. `src/agent/memory_loader.rs` (persistent agents via `Agent::turn`)

If only `loop_.rs` is updated, daemon-persistent agents will not get personalization.

### 2.2 Retrieval Plan Aligned with Current Memory APIs

Use a hybrid retrieval strategy that matches actual trait behavior:

1. `mem.get("user_model_summary")` from `Custom("user_model")` if present.
2. `mem.recall(user_msg, 8)` for query-relevant entries (`score` available).
3. `mem.list(Some(&MemoryCategory::Custom("preference".into())))` for always-on preferences.

Notes:

- `list()` entries do not have scores.
- On SQLite, `list()` is already ordered by `updated_at DESC`; use returned order for preference priority.
- `MemoryEntry.timestamp` from SQLite maps to `created_at`, not `updated_at`, so pure timestamp-decay on updated preference rows is misleading.

### 2.3 Realistic Relevance Scoring

Only apply semantic scoring to entries from `recall()`.

```rust
fn rank(entry: &MemoryEntry, source: Source, mode: Mode) -> f64 {
    let base = match source {
        Source::Recall => entry.score.unwrap_or(0.3),
        Source::ListPreference => 0.6,
        Source::UserModel => 0.9,
    };

    let category_boost = match entry.category {
        MemoryCategory::Custom(ref c) if c == "preference" => 1.2,
        MemoryCategory::Custom(ref c) if c == "fact" => 1.0,
        _ => 0.9,
    };

    let mode_boost = match mode {
        Mode::Debugging => 1.1,
        Mode::Learning => 1.0,
        Mode::Implementation => 1.05,
        Mode::Default => 1.0,
    };

    base * category_boost * mode_boost
}
```

Keep this deterministic and cheap.

### 2.4 Context Assembly and Budget

Prefer a char-budget (already used in code patterns) over estimated token math.

Recommended structure:

- `[User preferences]` (always-on, top 3-5)
- `[Relevant facts]` (query-relevant)
- `[Recent memory]` (fallback)

If `compact_context` is true, reduce limits and skip fallback block.

### 2.5 Prompt/Style Mapping

Keep style adaptation deterministic:

- map preference text patterns to concrete directives
- deduplicate conflicting directives
- cap directive count

Do not call an LLM for style mapping in request path.

---

## Part 3: Closing the Feedback Loop

Current gap: extraction writes preferences, but nothing measures whether they helped.

### 3.1 Minimal Closed Loop (V1.5)

After building personalization context for a turn:

1. record which preference keys were injected (`turn::<id> -> [preference keys]` in memory or in-process cache)
2. on next user message, check for explicit contradiction phrases (`"too verbose"`, `"don't use bullets"`, `"show reasoning"`, etc.)
3. update per-preference counters in `Custom("personalization_feedback")`

Feedback payload example:

```json
{
  "hits": 12,
  "contradictions": 2,
  "last_seen": "2026-02-19T14:10:00Z"
}
```

### 3.2 Using Feedback in L2

When L2 is added, weight preferences by observed utility:

- high hit / low contradiction => promote into `user_model_summary`
- low hit / high contradiction => demote or archive

This closes the loop without modifying `MemoryEntry` schema.

---

## Part 4: Implementation Sequence

### Phase 0 (must-do prerequisites)

1. Fix persistent autosave keys in `src/agent/agent.rs` to avoid SQLite upsert overwrite.
2. Extend `memory_store` (`src/tools/memory_store.rs`) to accept `fact`, `preference`, and `user_model` category strings (mapped to `Custom(...)`).

### Phase 1 (Personalization first)

1. Add `src/agent/personalization.rs`.
2. Route both `loop_.rs` and `DefaultMemoryLoader` through it.
3. Add tests for ordering and budget behavior with mixed categories.

### Phase 2 (L1 Learning worker)

1. Add `LearningConfig` in `src/config/schema.rs` and `Config`.
2. Add `src/learning/worker.rs` (watermark, polling, extraction call, storage).
3. Add session-end marker writes in `loop_.rs` paths.
4. Wire daemon supervisor for workspace-level learning only.

### Phase 3 (feedback)

1. Add injected-preference tracking.
2. Add contradiction detection heuristics.
3. Persist feedback counters.

### Phase 4 (optional L2)

1. Add cross-session synthesis job from extracted `fact`/`preference` + feedback counters.
2. Maintain `user_model_summary` entry.

---

## Test Matrix (By Risk)

### Medium risk (Personalization)

- Unit: mixed `MemoryEntry` ranking and section formatting
- Unit: compact mode budget trimming
- Integration: `loop_.rs` and `DefaultMemoryLoader` both include personalization

### High risk (Learning + daemon wiring)

- Unit: watermark read/write and idempotent re-run
- Unit: strict JSON parse and schema validation
- Integration: worker skips unsupported backends
- Integration: daemon learning supervisor restart/backoff behavior

### Regression checks

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

---

## What We Are Intentionally Not Doing (Now)

- No new `Memory` trait methods.
- No cross-agent learning merge policy in V1.
- No per-turn extraction call in request path.
- No LLM-based contradiction arbiter in V1.
- No Markdown-backend learning guarantees.

---

## Practical Summary

- Use `Custom(...)` categories, not enum expansion.
- Personalization must cover both agent paths (`loop_.rs` and `memory_loader.rs`).
- Learning for persistent agents requires fixing autosave key overwrite first.
- Relevance scoring must reflect real `recall()`/`list()` behavior.
- Feedback loop should persist explicit utility counters, not add fields to `MemoryEntry`.
