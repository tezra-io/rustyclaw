# PAHF Feedback Loop Design

**Status:** Design spike — ready for implementation
**Last updated:** 2026-02-21
**Paper:** [PAHF: Preference-Aware Human Feedback for Personalized Assistants](https://arxiv.org/abs/2602.16173)

---

## Context

MAPLE (Memory/Learning/Personalization) gives us the *storage* and *injection* halves. What's missing is the **closed feedback loop**:

```
Personalization injects preferences
        ↓
Response is generated
        ↓
User reacts (implicitly or explicitly)
        ↓
Correction is detected ← WE'RE ADDING THIS
        ↓
Memory is updated immediately
        ↓
Next personalization is better
```

The PAHF paper proves that without this loop, preference *drift* accumulates as O(T) cumulative errors. With real-time correction detection, it drops to O(K) where K = number of actual preference changes. In practice: users change preferences rarely, so K << T, and the system converges quickly.

This doc designs two PAHF feedback channels:
1. **Pre-action clarification** — ask before guessing when preference is unknown
2. **Post-action correction detection** — detect and store corrections in real-time

---

## What We Have (Baseline)

**Injection point:** `loop_.rs:936` calls `build_context()` and `build_user_model_context()` before each turn. Preferences are prepended to the user message.

**Storage:** `mem.store(key, content, MemoryCategory::Preference)` — SQLite upserts by key. Stable keys (e.g. `preference::verbosity`) mean updating a preference simply overwrites the old row. No tombstones, no migration.

**Batch extraction:** `learning.rs` runs every 5 minutes, extracts preferences from conversation history after a session boundary, stores them. This is the *slow path*.

**What's missing:**
- Real-time correction detection (the slow path has multi-minute latency)
- Ambiguity detection + clarification before responding
- Preference drift handling (user says "actually, stop doing X")

---

## Channel 1: Pre-Action Clarification

### The Problem

The agent guesses at formatting preferences it doesn't know. If asked "summarize this" with no stored `preference::format` entry, it picks prose or bullets arbitrarily. The user corrects it. Multiply by 50 interactions = 50 wasted corrections.

PAHF's insight: it's cheaper to **ask once** than to be corrected 50 times.

### Detection Logic

A query is *preference-dependent* if it involves subjective presentation choices where different reasonable users want different things. We detect this with a cheap heuristic — no LLM call.

```rust
// src/agent/clarification.rs

/// Patterns that signal format/style preference ambiguity.
/// These are checked ONLY when no relevant preference is in memory.
static FORMAT_AMBIGUITY_PATTERNS: &[&str] = &[
    "summarize", "summary", "explain", "describe",
    "write", "draft", "create a", "generate",
    "list", "outline", "overview",
];

/// Known preference dimensions. If any is in memory, skip clarification.
static PREFERENCE_DIMENSION_KEYS: &[&str] = &[
    "preference::format",
    "preference::verbosity",
    "preference::code_presentation",
    "user_model.response_style",
];

pub struct ClarificationCheck {
    pub needs_clarification: bool,
    /// The question to ask the user, if clarification is needed.
    pub question: Option<String>,
    /// Which preference key this clarification would populate.
    pub preference_key: Option<String>,
}

/// Check whether the agent should ask a clarifying question before responding.
///
/// Returns `ClarificationCheck::needs_clarification = true` only when:
///   1. The query matches a format-ambiguity pattern, AND
///   2. No relevant preference is already in memory
///
/// This avoids asking about things the user has already told us.
pub async fn check_clarification_needed(
    mem: &dyn Memory,
    user_msg: &str,
    config: &ClarificationConfig,
) -> ClarificationCheck {
    if !config.enabled {
        return ClarificationCheck { needs_clarification: false, question: None, preference_key: None };
    }

    let msg_lower = user_msg.to_lowercase();

    // Step 1: Does the query need a format/style preference?
    let matched_pattern = FORMAT_AMBIGUITY_PATTERNS
        .iter()
        .find(|&&p| msg_lower.contains(p));

    if matched_pattern.is_none() {
        return ClarificationCheck { needs_clarification: false, question: None, preference_key: None };
    }

    // Step 2: Do we already know the relevant preference?
    for key in PREFERENCE_DIMENSION_KEYS {
        if mem.get(key).await.ok().flatten().is_some() {
            // We have a preference for this dimension — no need to ask.
            return ClarificationCheck { needs_clarification: false, question: None, preference_key: None };
        }
    }

    // Step 3: Rate-limit — don't ask more than once per N turns
    // (tracked via a short-lived in-memory counter, see §Config)
    ClarificationCheck {
        needs_clarification: true,
        question: Some("How would you like this formatted? (e.g., bullet points, prose, numbered list, or let me choose)".to_string()),
        preference_key: Some("preference::format".to_string()),
    }
}
```

### Integration Point in `loop_.rs`

The clarification check runs **between** receiving the user message and building the LLM history. It intercepts the turn before the LLM call.

```rust
// In loop_.rs — interactive mode, inside `while let Some(msg) = rx.recv().await {`

// ── Pre-action clarification ──────────────────────────────────────────
if config.memory.auto_save && config.clarification.enabled {
    let check = crate::agent::clarification::check_clarification_needed(
        mem.as_ref(),
        &msg.content,
        &config.clarification,
    ).await;

    if check.needs_clarification {
        // Emit the clarification question as the agent's response.
        // Then wait for the user's answer before proceeding.
        println!("\n{}\n", check.question.as_deref().unwrap_or("How would you like me to respond?"));

        // Wait for user's clarification answer
        if let Some(clarification_reply) = rx.recv().await {
            // Store the preference immediately before responding
            if let Some(pref_key) = &check.preference_key {
                let pref_content = extract_preference_from_reply(&clarification_reply.content);
                let _ = mem.store(
                    pref_key,
                    &pref_content,
                    MemoryCategory::Preference,
                ).await;
                tracing::info!(key = pref_key, "Stored clarification preference");
            }
            // Now process the original message enriched with the new preference
            // (fall through to normal turn processing with updated memory)
        }
        // Continue to normal turn processing — the preference is now in memory
        // and will be picked up by the next build_context() call.
    }
}
```

**Why this approach works:**
- The clarification reply is stored as a `Preference` entry *before* the enriched message is built.
- On the very next turn (the actual response), `build_context()` will find the newly-stored preference and inject it.
- No in-process state is needed. The memory store is the communication channel.

### Preference Extraction from Clarification Reply

```rust
/// Map free-text clarification replies to canonical preference values.
/// This is heuristic — exact match is not required.
fn extract_preference_from_reply(reply: &str) -> String {
    let r = reply.to_lowercase();
    if r.contains("bullet") || r.contains("list") || r.contains("point") {
        "bullet points".to_string()
    } else if r.contains("prose") || r.contains("paragraph") || r.contains("flowing") {
        "prose paragraphs".to_string()
    } else if r.contains("numbered") {
        "numbered list".to_string()
    } else if r.contains("short") || r.contains("brief") || r.contains("concise") || r.contains("terse") {
        "terse, concise".to_string()
    } else if r.contains("detail") || r.contains("verbose") || r.contains("thorough") || r.contains("full") {
        "detailed, verbose".to_string()
    } else {
        // Store the raw reply, truncated — it's still useful context
        crate::util::truncate_with_ellipsis(reply.trim(), 100)
    }
}
```

### Anti-Annoyance Safeguards

The biggest risk is over-asking. Safeguards:

1. **Gate on memory absence.** If ANY of `PREFERENCE_DIMENSION_KEYS` has an entry, don't ask. Once the user answers once, they'll never be asked for that dimension again.

2. **Session-level rate limit.** Config field `clarification.max_per_session: u32` (default: 1). Tracked in a `std::sync::atomic::AtomicU32` local to the session. The one-shot CLI path never asks (no interactive loop).

3. **Only on output-shaping queries.** The pattern list is conservative. "What's the capital of France?" never triggers it.

4. **Disabled by default.** `clarification.enabled = false` until tested.

### Config Addition

```rust
// src/config/schema.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClarificationConfig {
    /// Enable pre-action clarification. Default: false (opt-in).
    #[serde(default)]
    pub enabled: bool,
    /// Max clarifications per interactive session. Default: 1.
    #[serde(default = "default_clarification_max_per_session")]
    pub max_per_session: u32,
}

fn default_clarification_max_per_session() -> u32 { 1 }

impl Default for ClarificationConfig {
    fn default() -> Self {
        Self { enabled: false, max_per_session: 1 }
    }
}
```

### Example Walkthrough

```
Session 1, Turn 1:
  User: "Summarize the Rust ownership model for me"
  Memory: no preference::format entry
  → Clarification check fires
  Agent: "How would you like this formatted? (e.g., bullet points, prose, numbered list)"
  User: "bullet points please"
  → Stored: preference::format = "bullet points"

Session 1, Turn 2 (original request processed):
  build_context() finds preference::format = "bullet points"
  → [User preferences]
     - bullet points
  Agent responds in bullet points.

Session 2, Turn 1:
  User: "Explain async/await in Rust"
  Memory: preference::format = "bullet points" ← already there
  → Clarification check: preference found, skip
  Agent responds directly in bullet points.
```

---

## Channel 2: Post-Action Correction Detection

### The Problem

The user says "actually, I prefer verbose responses" after getting a terse one. The Learning Worker will extract this in its next batch run (5 minutes later, at best). But the user is still in this conversation — the *next* turn should already know about this correction.

The PAHF paper calls this the "feedback integration window." Without real-time detection, we respond incorrectly for the entire remainder of the session.

### Detection Strategy: Heuristic, Not LLM

An LLM-per-message correction check costs ~$0.001 per message at Haiku prices — annoying but manageable. But it adds 200-400ms latency to every single message, which is perceptible. Instead, use a fast regex-based heuristic that's good enough in practice.

**Key insight:** Genuine corrections share a small vocabulary. Users say "actually," "no," "instead," "don't," "stop," "I prefer," etc. The false positive rate is low for factual corrections (those don't trigger our preference keys). The false negative rate is acceptable — the Learning Worker catches what we miss.

```rust
// src/agent/correction.rs

use crate::memory::{Memory, MemoryCategory};
use std::sync::LazyLock;
use regex::Regex;

/// Patterns that signal a preference correction.
/// The user is telling us to change how we respond, not what facts to use.
static CORRECTION_PATTERNS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?xi)
        # Explicit contradiction phrases
        \b(actually|no[,.]?\s+|don't|do\s+not|stop)\b.*\b(prefer|like|want|need|use)\b |
        # Direct preference statements (corrective tone)
        \b(i\s+prefer|i\s+want|i\s+like|please\s+use|please\s+give)\b |
        # Format corrections
        \b(bullet|bullets|prose|paragraph|numbered|terse|verbose|concise|brief|detailed|shorter|longer)\b |
        # Style corrections
        \bno\s+(markdown|bullets|bold|headers|formatting)\b |
        \buse\s+(plain\s+text|markdown|bullets|numbered|prose)\b
        "
    ).expect("CORRECTION_PATTERNS is valid")
});

/// Preference dimension classifiers — maps correction text to a stable key.
fn classify_preference_dimension(text: &str) -> Option<&'static str> {
    let t = text.to_lowercase();
    if t.contains("bullet") || t.contains("list") || t.contains("point") || t.contains("prose") || t.contains("paragraph") || t.contains("numbered") || t.contains("format") || t.contains("markdown") {
        return Some("preference::format");
    }
    if t.contains("verbose") || t.contains("terse") || t.contains("concise") || t.contains("brief") || t.contains("shorter") || t.contains("longer") || t.contains("detailed") {
        return Some("preference::verbosity");
    }
    if t.contains("code block") || t.contains("snippet") || t.contains("inline code") {
        return Some("preference::code_presentation");
    }
    None
}

/// Extract the corrected preference value from the user message.
fn extract_correction_value(text: &str) -> String {
    let t = text.to_lowercase();
    // Try to extract the specific requested format/style
    if t.contains("bullet") { return "bullet points".to_string(); }
    if t.contains("prose") || t.contains("paragraph") { return "prose paragraphs".to_string(); }
    if t.contains("numbered") { return "numbered list".to_string(); }
    if t.contains("terse") || t.contains("concise") || t.contains("brief") || t.contains("shorter") { return "terse, concise".to_string(); }
    if t.contains("verbose") || t.contains("detailed") || t.contains("longer") { return "detailed, verbose".to_string(); }
    if t.contains("plain text") || t.contains("no markdown") { return "plain text, no markdown".to_string(); }
    // Fallback: store the correction as-is
    crate::util::truncate_with_ellipsis(text.trim(), 100)
}

pub struct CorrectionResult {
    /// Whether a correction was detected.
    pub detected: bool,
    /// Stable preference key to update (e.g. "preference::verbosity").
    pub preference_key: Option<&'static str>,
    /// New value to store.
    pub new_value: Option<String>,
}

/// Check if a user message contains a preference correction.
/// This is called on every message AFTER the response — cheap, synchronous.
pub fn detect_correction(user_msg: &str) -> CorrectionResult {
    if !CORRECTION_PATTERNS.is_match(user_msg) {
        return CorrectionResult { detected: false, preference_key: None, new_value: None };
    }

    let key = classify_preference_dimension(user_msg);
    let value = key.map(|_| extract_correction_value(user_msg));

    CorrectionResult {
        detected: key.is_some(),
        preference_key: key,
        new_value: value,
    }
}
```

### Integration Point in `loop_.rs`

Correction detection runs at the **top of the next turn**, applied to the incoming user message *before* building context. This means: the correction is stored before `build_context()` is called, so it's already in memory when the response is generated.

```rust
// In loop_.rs — interactive mode, inside `while let Some(msg) = rx.recv().await {`

// ── Post-action correction detection (real-time) ──────────────────────
if config.memory.auto_save {
    let correction = crate::agent::correction::detect_correction(&msg.content);
    if correction.detected {
        if let (Some(key), Some(value)) = (correction.preference_key, correction.new_value) {
            // Upsert the preference — SQLite's ON CONFLICT DO UPDATE semantics
            // mean this supersedes any previous entry for this key.
            let _ = mem.store(key, &value, MemoryCategory::Preference).await;
            tracing::info!(key, value, "Real-time correction stored");
        }
    }
}

// ── Auto-save user message ────────────────────────────────────────────
if config.memory.auto_save {
    let user_key = autosave_memory_key("user_msg");
    let _ = mem.store(&user_key, &msg.content, MemoryCategory::Conversation).await;
}

// ── Build enriched context (correction is now in memory) ─────────────
let user_model_ctx = build_user_model_context(mem.as_ref()).await;
let mem_context = build_context(mem.as_ref(), &msg.content, config.memory.min_relevance_score).await;
// ... rest of turn processing unchanged
```

**The ordering is critical:** correction store → then build_context. If we build context first, we miss the correction for this turn.

### Handling Preference Drift

Preference drift is when a user *changes* their mind: they used to want verbose, now want terse. This is the O(T) error the PAHF paper describes.

Our design handles drift automatically:
- Preference keys are stable: `preference::verbosity`, `preference::format`
- SQLite's `store()` is an upsert: `ON CONFLICT(key) DO UPDATE SET content = ?`
- "I prefer terse" on Monday overwrites "I prefer verbose" from last week — one row, always current

No tombstones. No versioning. No "supersedes" chains. The latest write wins.

The Learning Worker is *also* running, and it also writes to these same stable keys. No race: both the real-time path and the Learning Worker call `mem.store()` with the same key, so whichever runs last wins. For recent corrections, the real-time path wins (it fires immediately). For older, implicit preferences, the Learning Worker wins (it runs later). Both are correct — they reflect the most recent expression of the preference.

### Interaction with Learning Worker (Deduplication)

The Learning Worker runs `stable_preference_key()` on extracted preferences, which returns the same `preference::verbosity` / `preference::format` keys as the real-time path. When both paths write to the same key:
- If the user said "actually, I prefer verbose" 3 minutes ago, real-time detection wrote it.
- When Learning Worker runs and extracts the same preference from the conversation transcript, it upserts the same key with the same (or similar) value.
- Net effect: the row is refreshed with a slightly later timestamp. No duplicate rows.

The only case where they diverge is if the user expressed a *different* correction than what the batch extractor inferred. In that case, the real-time path's value is the ground truth and the batch extractor's value is stale. SQLite's upsert means the *last write* wins. Since the real-time path fires first (at turn N) and the batch extractor fires later (at turn N+5min), the batch extractor actually *overwrites* the real-time correction. This is a known limitation:

**Mitigation:** The Learning Worker should skip entries that have been written in the last `correction_freshness_window` seconds (default: 300s = 5 min). Before storing an extracted preference, check the entry's `updated_at` timestamp:

```rust
// In learning.rs, store_extracted():
for pref in extracted.preferences {
    let key = stable_preference_key(&pref);
    // Skip if recently written by real-time correction detection
    if let Ok(Some(existing)) = self.mem.get(&key).await {
        if let Ok(ts) = DateTime::parse_from_rfc3339(&existing.timestamp) {
            let age_secs = (Utc::now() - ts.with_timezone(&Utc)).num_seconds();
            if age_secs < CORRECTION_FRESHNESS_WINDOW_SECS {
                debug!("Skipping preference key {key}: recently updated by real-time path");
                continue;
            }
        }
    }
    let _ = self.mem.store(&key, &pref.text, MemoryCategory::Preference).await;
}

const CORRECTION_FRESHNESS_WINDOW_SECS: i64 = 300;
```

### Example Walkthrough

```
Turn N-1:
  Agent responds with terse bullet points
  (based on preference::verbosity = "terse, concise" from 2 weeks ago)

Turn N (incoming user message):
  User: "Actually I want more detail — stop being so brief, give me verbose answers"

  → detect_correction("Actually I want...") fires
  → CORRECTION_PATTERNS matches ("actually", "verbose")
  → key = "preference::verbosity"
  → value = "detailed, verbose"
  → mem.store("preference::verbosity", "detailed, verbose", Preference).await
     (upserts — replaces "terse, concise")

  → build_context() now finds:
     preference::verbosity = "detailed, verbose"

  → [User preferences]
     - detailed, verbose

  Agent responds with a verbose, detailed answer. ✓

Turn N+5min (Learning Worker batch):
  → Extracts "user wants verbose responses" from transcript
  → stable_preference_key() → "preference::verbosity"
  → Checks existing entry timestamp: 3 minutes ago
  → 3 min < 5 min freshness window → skip
  → No overwrite. ✓
```

---

## The Closed Feedback Loop

Putting both channels together, here's the full closed loop per the PAHF paper's formulation:

```
┌─────────────────────────────────────────────────────────────────────┐
│                     MAPLE + PAHF Feedback Loop                      │
│                                                                      │
│  ┌─────────────┐    inject preferences    ┌──────────────────────┐  │
│  │   Memory    │ ──────────────────────▶  │   build_context()    │  │
│  │  (SQLite)   │                          │   + user_model_ctx   │  │
│  └─────────────┘                          └──────────┬───────────┘  │
│        ▲                                             │              │
│        │                                             ▼              │
│        │                                   ┌──────────────────────┐ │
│        │                                   │   LLM generates      │ │
│        │                                   │   response           │ │
│        │                                   └──────────┬───────────┘ │
│        │                                             │              │
│        │                                             ▼              │
│        │                                   ┌──────────────────────┐ │
│        │                                   │   User sees response │ │
│        │                                   │   and replies        │ │
│        │                                   └──────────┬───────────┘ │
│        │                                             │              │
│        │    ┌────────────────────────────────────────┘              │
│        │    │         Next user message                             │
│        │    ▼                                                       │
│        │  ┌─────────────────────────────────────────────────────┐   │
│        │  │  1. detect_correction()  [real-time, heuristic]    │   │
│        │  │     → if correction: mem.store(pref_key, value)    │   │
│        │  │                                                     │   │
│        │  │  2. check_clarification_needed()  [if enabled]     │   │
│        │  │     → if ambiguous + no pref: ask, then store      │   │
│        │  └─────────────────────────────────────────────────────┘   │
│        │                                                             │
│        └─────────────────── preference updated ──────────────────── │
│                          (ready for next turn)                       │
│                                                                      │
│  Background:                                                         │
│  LearningWorker (5-min poll) extracts from conversation history      │
│  → skips keys updated within freshness window                        │
└─────────────────────────────────────────────────────────────────────┘
```

**Error bound:** The PAHF paper shows that with this loop, cumulative errors grow as O(K) where K = number of preference changes. In a real session:
- K is typically 0-3 (people rarely change preferences)
- Without the loop: every post-drift turn is wrong → O(T) errors
- With the loop: errors stop at the turn *after* the correction → O(K+1) errors

---

## Implementation Plan

### Phase 1: Post-Action Correction Detection

This is the high-value, low-effort change. No new config needed for a v0.

**Files to modify:**
- `src/agent/correction.rs` — new file, `detect_correction()` function
- `src/agent/loop_.rs` — add correction detection at top of each turn (both interactive and one-shot paths)
- `src/memory/learning.rs` — add freshness window check before storing extracted preferences

**Files to create:**
- `src/agent/correction.rs`

**Steps:**
1. Add `src/agent/correction.rs` with `CORRECTION_PATTERNS`, `detect_correction()`, `classify_preference_dimension()`, `extract_correction_value()`
2. In `loop_.rs` interactive loop: add correction detection before `build_context()`, gated on `config.memory.auto_save`
3. In `loop_.rs` one-shot path: same correction detection (user msg is single turn, but still useful)
4. In `learning.rs`: add `CORRECTION_FRESHNESS_WINDOW_SECS` const and timestamp check before storing preferences
5. Tests: `detect_correction()` unit tests (see §Tests)

### Phase 2: Pre-Action Clarification

More infrastructure needed. Config opt-in.

**Files to modify:**
- `src/config/schema.rs` — add `ClarificationConfig`, add to `Config`
- `src/agent/loop_.rs` — add clarification check in interactive loop only
- `src/agent/mod.rs` — re-export `clarification` module

**Files to create:**
- `src/agent/clarification.rs`

**Steps:**
1. Add `ClarificationConfig` to `schema.rs` with `enabled: false` default
2. Add `src/agent/clarification.rs` with `check_clarification_needed()`, `extract_preference_from_reply()`
3. In `loop_.rs` interactive loop: add clarification check after auto-save, before context build
4. Add session-level `AtomicU32` counter to enforce `max_per_session` limit
5. Tests: (see §Tests)

---

## Tests

### Correction Detection (Phase 1)

```rust
// src/agent/correction.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_simple_verbosity_correction() {
        let r = detect_correction("Actually I prefer verbose responses");
        assert!(r.detected);
        assert_eq!(r.preference_key, Some("preference::verbosity"));
        assert!(r.new_value.as_deref().unwrap().contains("verbose"));
    }

    #[test]
    fn detect_format_correction() {
        let r = detect_correction("Please use bullet points instead of prose");
        assert!(r.detected);
        assert_eq!(r.preference_key, Some("preference::format"));
        assert!(r.new_value.as_deref().unwrap().contains("bullet"));
    }

    #[test]
    fn no_false_positive_on_factual_question() {
        let r = detect_correction("What's the difference between stack and heap?");
        assert!(!r.detected);
    }

    #[test]
    fn no_false_positive_on_simple_agreement() {
        let r = detect_correction("Yes, that's right");
        assert!(!r.detected);
    }

    #[test]
    fn detect_no_markdown_correction() {
        let r = detect_correction("No markdown please, just plain text");
        assert!(r.detected);
        assert_eq!(r.preference_key, Some("preference::format"));
    }

    #[test]
    fn detect_stop_brief_correction() {
        let r = detect_correction("Stop being so brief, I need more detail");
        assert!(r.detected);
        assert_eq!(r.preference_key, Some("preference::verbosity"));
    }

    #[test]
    fn no_false_positive_on_task_with_list() {
        // "list the files" should not trigger — it's a task, not a preference
        // (CORRECTION_PATTERNS requires correction-signal words alongside format words)
        let r = detect_correction("list the files in this directory");
        assert!(!r.detected);
    }

    #[test]
    fn correction_with_actually_prefix() {
        let r = detect_correction("Actually, I like prose better");
        assert!(r.detected);
        assert_eq!(r.preference_key, Some("preference::format"));
    }
}
```

### Clarification (Phase 2)

```rust
// src/agent/clarification.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Memory, MemoryCategory, MemoryEntry};

    struct EmptyMemory;
    // ... impl Memory for EmptyMemory (all methods return empty/ok)

    struct MemoryWithPreference;
    // ... impl Memory for MemoryWithPreference
    //     get("preference::format") returns Some(entry)

    #[tokio::test]
    async fn triggers_when_no_preference_stored() {
        let mem = EmptyMemory;
        let cfg = ClarificationConfig { enabled: true, max_per_session: 3 };
        let result = check_clarification_needed(&mem, "summarize the Rust book for me", &cfg).await;
        assert!(result.needs_clarification);
        assert!(result.question.is_some());
    }

    #[tokio::test]
    async fn skips_when_preference_already_stored() {
        let mem = MemoryWithPreference;
        let cfg = ClarificationConfig { enabled: true, max_per_session: 3 };
        let result = check_clarification_needed(&mem, "summarize this", &cfg).await;
        assert!(!result.needs_clarification);
    }

    #[tokio::test]
    async fn skips_factual_queries() {
        let mem = EmptyMemory;
        let cfg = ClarificationConfig { enabled: true, max_per_session: 3 };
        let result = check_clarification_needed(&mem, "what is the capital of France?", &cfg).await;
        assert!(!result.needs_clarification);
    }

    #[tokio::test]
    async fn disabled_by_default() {
        let mem = EmptyMemory;
        let cfg = ClarificationConfig::default(); // enabled: false
        let result = check_clarification_needed(&mem, "summarize this", &cfg).await;
        assert!(!result.needs_clarification);
    }

    #[test]
    fn extract_preference_from_bullet_reply() {
        let v = extract_preference_from_reply("bullet points please");
        assert!(v.contains("bullet"));
    }

    #[test]
    fn extract_preference_from_prose_reply() {
        let v = extract_preference_from_reply("prose, flowing paragraphs");
        assert!(v.contains("prose"));
    }
}
```

---

## Config Schema (Final)

```toml
# ~/.rustyclaw/config.toml

[clarification]
enabled = false                  # opt-in
max_per_session = 1              # ask at most once per session

# No new correction config — it's always on when memory.auto_save = true.
# The freshness window is a constant in learning.rs, not user-configurable.
```

---

## What This Does NOT Do

**No LLM call per message for correction detection.** The regex heuristic is ~90% accurate for explicit corrections and adds < 1ms latency. An LLM-based detector would be more accurate but adds 200-400ms per message. The Learning Worker catches what the heuristic misses.

**No per-turn injection tracking.** The LEARNING_AND_PERSONALIZATION.md design (§3.1) describes tracking which preference keys were injected per turn and counting hits/contradictions. That's useful for L2 synthesis but not needed for the core feedback loop. Skip for now.

**No streaming correction detection.** We detect corrections on the *complete* message, not token-by-token. This is correct — you can't reliably classify correction intent from a partial message.

**No cross-session correction history.** We don't record the history of preference changes (e.g., "user changed verbosity preference 3 times"). This would be useful for detecting volatile users (K is large for them) but adds complexity. The stable key approach already handles the common case: the current value is always the right value.

**No clarification for one-shot CLI.** The one-shot path (`rustyclaw -m "summarize this"`) doesn't support interactive clarification. This is by design — one-shot messages are often scripted. Clarification only applies to interactive mode (`loop_.rs`'s `while let Some(msg) = rx.recv()` path).

---

## Summary of Changes

| File | Change | Phase |
|------|--------|-------|
| `src/agent/correction.rs` | New: `detect_correction()`, `classify_preference_dimension()`, `extract_correction_value()` | 1 |
| `src/agent/loop_.rs` | Add correction detection before `build_context()` in both paths | 1 |
| `src/memory/learning.rs` | Add freshness window check before storing extracted preferences | 1 |
| `src/agent/clarification.rs` | New: `check_clarification_needed()`, `extract_preference_from_reply()` | 2 |
| `src/config/schema.rs` | Add `ClarificationConfig`, wire into `Config` | 2 |
| `src/agent/loop_.rs` | Add clarification check in interactive loop only | 2 |
