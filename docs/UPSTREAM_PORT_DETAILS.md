# Linear Issues: Upstream Zeroclaw Sync

Generated from diffs against commits in the zeroclaw upstream.
Each section describes what changed and how to reimplement it in RustyClaw.

---

## Issue 1: Security — Shell Injection Hardening (Priority 1)

**Title:** Security: harden shell injection vectors in policy, git, screenshot, and http tools

**Description:**

Six upstream commits close distinct shell injection attack surfaces. None touch provider code. All should be ported to RustyClaw as-is.

---

### What changed and why

#### a) `9ec1106` — Argument injection in `is_command_allowed` (`src/security/policy.rs`)

**Problem:** `is_command_allowed` validated the base command against an allowlist but did not inspect arguments. An attacker could pass `find . -exec rm -rf {} +` or `git config core.editor "rm -rf /"` — both allowed commands with dangerous argument-level sub-execution.

**Fix:** After extracting `base_cmd`, keep the remaining `words` iterator as `args: Vec<String>` (lowercased). Add `is_args_safe(base, args)` method gated on the `match base` pattern:

```rust
fn is_args_safe(&self, base: &str, args: &[String]) -> bool {
    let base = base.to_ascii_lowercase();
    match base.as_str() {
        "find" => {
            // -exec and -ok allow arbitrary command execution
            !args.iter().any(|arg| arg == "-exec" || arg == "-ok")
        }
        "git" => {
            // config/alias/-c allow setting dangerous options
            !args.iter().any(|arg| {
                arg == "config"
                    || arg.starts_with("config.")
                    || arg == "alias"
                    || arg.starts_with("alias.")
                    || arg == "-c"
            })
        }
        _ => true,
    }
}
```

Replace the old one-liner that discarded `words` after extracting `base_cmd`:

```rust
// BEFORE
let base_cmd = cmd_part.split_whitespace().next().unwrap_or("").rsplit('/').next().unwrap_or("");

// AFTER
let mut words = cmd_part.split_whitespace();
let base_raw = words.next().unwrap_or("");
let base_cmd = base_raw.rsplit('/').next().unwrap_or("");
// ... allowlist check ...
let args: Vec<String> = words.map(|w| w.to_ascii_lowercase()).collect();
if !self.is_args_safe(base_cmd, &args) { return false; }
```

Tests to add: `find . -exec rm -rf {} +` → blocked; `find / -ok cat {} \;` → blocked; `git config core.editor "rm -rf /"` → blocked; `git -c core.editor=calc.exe commit` → blocked; `find . -name '*.txt'` → allowed; `git status` → allowed.

---

#### b) `a2986db` — Process substitution and `tee` blocking (`src/security/policy.rs`)

**Problem:** The subshell block list missed `<(...)` and `>(...)` (process substitution). `tee` can write to arbitrary paths, bypassing the `>` redirect check.

**Fix 1:** Extend the existing `contains('`')` check:

```rust
// BEFORE
if command.contains('`') || command.contains("$(") || command.contains("${") {
    return false;
}

// AFTER
if command.contains('`')
    || command.contains("$(")
    || command.contains("${")
    || command.contains("<(")
    || command.contains(">(")
{
    return false;
}
```

**Fix 2:** After the existing `&` check, add a tee block:

```rust
if command.split_whitespace().any(|w| w == "tee" || w.ends_with("/tee")) {
    return false;
}
```

Tests: `echo secret | tee /etc/crontab` → blocked; `ls | /usr/bin/tee outfile` → blocked; `cat <(echo pwned)` → blocked; `ls >(cat /etc/passwd)` → blocked.

---

#### c) `87dcd7a` — Git argument sanitization expansion (`src/tools/git_operations.rs`)

**Problem:** `sanitize_git_args` blocked `--exec=`, `--upload-pack=`, `--receive-pack=`, `$(`, `` ` ``, `|`, `;` — but not `--pager=`, `--editor=`, `--no-verify`, `>` in args, or `-c` config injection. Also, `git_add()` and `git_diff()` didn't call `sanitize_git_args` on their inputs.

**Fix:** Add to the blocklist in `sanitize_git_args`:
- `arg_lower.starts_with("--pager=")`
- `arg_lower.starts_with("--editor=")`
- `arg_lower == "--no-verify"`
- `arg.contains('>')`

Then add exact-match `-c` check (avoids false-positive on `--cached`):
```rust
if arg_lower == "-c" || arg_lower.starts_with("-c=") {
    anyhow::bail!("Blocked potentially dangerous git argument: {arg}");
}
```

Call `self.sanitize_git_args(files)?` at the start of `git_diff()` and `self.sanitize_git_args(paths)?` at the start of `git_add()`.

Tests: `--pager=less`, `--editor=vim`, `--no-verify`, `-c core.sshCommand=evil`, `file.txt > /tmp/out` all blocked; `--cached`, `-cached`, `src/main.rs`, `.` all allowed.

---

#### d) `290d971` — Shell injection via screenshot filename (`src/tools/screenshot.rs`)

**Problem:** On Linux, the screenshot path is interpolated into `sh -c '...'` with single-quote wrapping. A filename containing `'` or other shell metacharacters breaks the quoting and injects shell tokens.

**Fix:** Before computing `output_path`, add:

```rust
const SHELL_UNSAFE: &[char] =
    &['\'', '"', '`', '$', '\\', ';', '|', '&', '\n', '\0', '(', ')'];
if safe_name.contains(SHELL_UNSAFE) {
    return Ok(ToolResult {
        success: false,
        output: String::new(),
        error: Some("Filename contains characters unsafe for shell execution".into()),
    });
}
```

Test: `execute(json!({"filename": "test'injection.png"}))` → `!result.success`, error contains "unsafe for shell execution".

---

#### e) `bbe5530` — SSRF via redirect chain in `http_request` tool (`src/tools/http_request.rs`)

**Problem:** `reqwest::Client` follows redirects by default. The initial URL was validated, but redirect hops were not, enabling SSRF: attacker-controlled server returns a 302 to `http://169.254.169.254/`.

**Fix:** Add `.redirect(reqwest::redirect::Policy::none())` to the client builder in `execute_request`:

```rust
let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(self.timeout_secs))
    .redirect(reqwest::redirect::Policy::none())
    .build()?;
```

Callers that need to follow redirects must issue a new request, which re-runs `validate_url`.

---

#### f) `ac33121` — Config file permission hardening (`src/config/schema.rs`)

**Problem:** On multi-user systems, `config.toml` (containing API keys) may be world-readable.

**Fix:** In `Config::load_or_init()`:

1. When loading an existing config, warn if world-readable:
```rust
#[cfg(unix)]
{
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(&config_path) {
        if meta.permissions().mode() & 0o004 != 0 {
            tracing::warn!(
                "Config file {:?} is world-readable (mode {:o}). Consider: chmod 600 {:?}",
                config_path, meta.permissions().mode() & 0o777, config_path,
            );
        }
    }
}
```

2. After creating a new config file:
```rust
#[cfg(unix)]
{
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600));
}
```

---

**Our Files Affected:**
- `src/security/policy.rs` — issues a, b (no conflict with MAPLE)
- `src/tools/git_operations.rs` — issue c (no conflict)
- `src/tools/screenshot.rs` — issue d (no conflict)
- `src/tools/http_request.rs` — issue e (no conflict)
- `src/config/schema.rs` — issue f (check for MAPLE config additions — don't disturb new MAPLE fields)

**Conflict Notes:** None of these touch agent orchestration or memory. Safe to port in isolation. Our `src/config/schema.rs` has MAPLE additions — review the diff context carefully to insert permission hardening in `load_or_init` without disturbing those fields.

---

## Issue 2: Security — Credential Scrubbing in Tool Output (Priority 1)

**Title:** Security: scrub credentials from tool output before adding to conversation history

**Description:**

**Commit:** `df31359`
**Files:** `src/agent/loop_.rs`, `Cargo.toml`

### What changed and why

Tool output (file reads, shell commands, HTTP responses) is added verbatim to conversation history and echoed to connected channels. If a tool reads a `.env` file or `config.toml` containing API keys, those keys appear in the LLM context and potentially in the channel response.

**Fix:** Add a `scrub_credentials()` function that uses a regex to redact `key=value` and `"key": "value"` patterns where the key matches common credential field names. Apply it to successful tool output before it enters history.

**New dependency:** `regex = "1.10"` in `Cargo.toml` (already used in `loop_.rs` after the MAPLE changes — verify before adding again).

**New statics** (using `LazyLock`):

```rust
static SENSITIVE_KEY_PATTERNS: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new([
        r"(?i)token",
        r"(?i)api[_-]?key",
        r"(?i)password",
        r"(?i)secret",
        r"(?i)user[_-]?key",
        r"(?i)bearer",
        r"(?i)credential",
    ]).unwrap()
});

static SENSITIVE_KV_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(token|api[_-]?key|password|secret|user[_-]?key|bearer|credential)["']?\s*[:=]\s*(?:"([^"]{8,})"|'([^']{8,})'|([a-zA-Z0-9_\-\.]{8,}))"#).unwrap()
});
```

**`scrub_credentials(input: &str) -> String`** — runs `SENSITIVE_KV_REGEX.replace_all`, preserving the first 4 chars of the value for context:

```rust
fn scrub_credentials(input: &str) -> String {
    SENSITIVE_KV_REGEX.replace_all(input, |caps: &regex::Captures| {
        let full_match = &caps[0];
        let key = &caps[1];
        let val = caps.get(2).or(caps.get(3)).or(caps.get(4))
            .map(|m| m.as_str()).unwrap_or("");
        let prefix = if val.len() > 4 { &val[..4] } else { "" };
        if full_match.contains(':') {
            if full_match.contains('"') {
                format!("\"{}\": \"{}*[REDACTED]\"", key, prefix)
            } else {
                format!("{}: {}*[REDACTED]", key, prefix)
            }
        } else if full_match.contains('=') {
            if full_match.contains('"') {
                format!("{}=\"{}*[REDACTED]\"", key, prefix)
            } else {
                format!("{}={}*[REDACTED]", key, prefix)
            }
        } else {
            format!("{}: {}*[REDACTED]", key, prefix)
        }
    }).to_string()
}
```

**Insertion point** in `run_tool_call_loop` — in the branch that processes successful tool results:

```rust
// BEFORE
if r.success {
    r.output
}

// AFTER
if r.success {
    scrub_credentials(&r.output)
}
```

**Tests:**
- `API_KEY=sk-1234567890abcdef` → `API_KEY=sk-1*[REDACTED]`
- `token: 1234567890` → `token: 1234*[REDACTED]`
- `password="secret123456"` → `password="secr*[REDACTED]"`
- `{"api_key": "sk-1234567890", "other": "public"}` → api_key redacted, "public" preserved

**Conflict Notes:** `loop_.rs` has heavy MAPLE modifications in our codebase. The insertion point is specifically in the `run_tool_call_loop` success branch where `r.output` is returned. Find this branch carefully — the `regex` crate may already be a dep from MAPLE work; check `Cargo.toml` before adding.

---

## Issue 3: Agent Loop — Native Tool-Call History Fixes (Priority 2)

**Title:** Agent: fix native tool-call history format, markdown fallback parsing, and config-driven loop limits

**Description:**

**Commits:** `f75f73a`, `0e5a785`, `63602a2`, `3467d34`
**Files:** `src/agent/loop_.rs`

### What changed and why

Four bugs related to how `run_tool_call_loop` handles native (OpenAI-style) tool calls:

---

#### a) `63602a2` — Config-driven `max_tool_iterations` and `max_history_messages`

The constants `MAX_TOOL_ITERATIONS` (10) and `MAX_HISTORY_MESSAGES` (50) were hardcoded and ignored the `agent.max_tool_iterations` / `agent.max_history_messages` config values.

**Fix:**
- Rename `MAX_TOOL_ITERATIONS` → `DEFAULT_MAX_TOOL_ITERATIONS`
- Rename `MAX_HISTORY_MESSAGES` → `DEFAULT_MAX_HISTORY_MESSAGES`
- Add `max_tool_iterations: usize` parameter to `run_tool_call_loop`
- Add `max_history: usize` parameter to `trim_history` and `auto_compact_history`
- Thread `config.agent.max_tool_iterations` through all call sites
- Thread `config.agent.max_history_messages` through `trim_history`/`auto_compact_history`

**Signature changes:**
```rust
pub(crate) async fn run_tool_call_loop(
    // ... existing params ...
    max_tool_iterations: usize,  // NEW
) -> Result<String>

fn trim_history(history: &mut Vec<ChatMessage>, max_history: usize)  // was no param

async fn auto_compact_history(
    history: &mut Vec<ChatMessage>,
    provider: &dyn Provider,
    model: &str,
    max_history: usize,  // NEW
) -> Result<bool>
```

**Note:** Check if our MAPLE changes already thread `max_tool_iterations` — the `8a1e7cc` commit (see Issue 6) overlaps here.

---

#### b) `0e5a785` — Native tool result history uses proper format

**Problem:** When `use_native_tools = true`, tool results were appended to history as `ChatMessage::user` with XML `<tool_result>` tags. OpenRouter/Gemini reject this — they expect `role: "tool"` messages with `tool_call_id`.

**Fix:** Add `build_native_assistant_history(text, tool_calls) -> String` which serializes tool calls as JSON (matching what `convert_messages()` in the provider expects):

```rust
fn build_native_assistant_history(text: &str, tool_calls: &[ToolCall]) -> String {
    let calls_json: Vec<serde_json::Value> = tool_calls.iter().map(|tc| {
        serde_json::json!({
            "id": tc.id,
            "name": tc.name,
            "arguments": tc.arguments,
        })
    }).collect();
    serde_json::json!({
        "content": text,
        "tool_calls": calls_json,
    }).to_string()
}
```

When `use_native_tools && !resp.tool_calls.is_empty()`:
- Use `build_native_assistant_history` instead of `build_assistant_history_with_tool_calls`
- Push each tool result as `ChatMessage { role: "tool".into(), content: json!({"tool_call_id": tc_id, "content": result}).to_string() }` instead of a single `ChatMessage::user` with concatenated XML

Also: add markdown ```` ```tool_call ... ``` ```` fallback parsing in `parse_tool_calls`. Models (e.g. via OpenRouter) sometimes output tool calls wrapped in code blocks instead of XML or structured API calls:

```rust
// In parse_tool_calls, after XML parsing:
if calls.is_empty() {
    static MD_TOOL_CALL_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)```tool[_-]?call\s*\n(.*?)(?:```|</tool[_-]?call>|</toolcall>)").unwrap()
    });
    // parse captures, extract JSON, extend calls
}
```

---

#### c) `f75f73a` — Preserve native tool-call fallbacks

**Problem:** In the `use_native_tools` branch, if `resp.tool_calls.is_empty()` and the response text contained XML tool calls, `parsed_text` was discarded (left as empty string). The fallback `parse_tool_calls(&response_text)` was called but `parsed_text` from it was not threaded to the display/history path.

**Fix:** Reformatted the deeply-nested `if use_native_tools { match ... }` block to properly set `parsed_text` from the fallback when `calls.is_empty()`. The logic was correct but the variable wasn't assigned. This is primarily a formatting/indentation fix that makes the code match what was intended. Also adds tests for `convert_messages` round-trip fidelity in OpenRouter provider.

---

#### d) `3467d34` — Avoid duplicate text in markdown tool_call fallback

**Problem:** When the markdown code block fallback (from `0e5a785`) extracted tool calls, text before/after blocks was duplicated — once from the surrounding `response_text` and once from the `md_text_parts` accumulator.

**Fix:** Track `text_parts` from the markdown path separately; only use them when markdown calls were actually found.

---

**Conflict Notes:** Our `loop_.rs` has significant MAPLE modifications. The `8a1e7cc` commit in Issue 6 overlaps with `63602a2` — both thread `max_tool_iterations` through the same call sites. Implement Issue 6 first (or together). The tool result history format change (`0e5a785`) touches the same area as MAPLE's tool loop modifications — carefully diff against our current `loop_.rs` before applying.

---

## Issue 4: Anthropic Prompt Caching (Priority 2)

**Title:** Provider: add prompt caching support to Anthropic provider

**Description:**

**Commit:** `455eb3b`
**Files:** `src/providers/anthropic.rs`

### What changed and why

Anthropic's API supports a `cache_control: {"type": "ephemeral"}` field on content blocks, system prompts, and tool specs. When set, Anthropic caches the prefix up to that point for 5 minutes, reducing cost by ~90% and latency by ~85% for repeated requests with identical leading content (e.g., long system prompts + tool lists).

### New types

```rust
#[derive(Debug, Clone, Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    cache_type: String,
}
impl CacheControl {
    fn ephemeral() -> Self { Self { cache_type: "ephemeral".to_string() } }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum SystemPrompt {
    String(String),
    Blocks(Vec<SystemBlock>),
}

#[derive(Debug, Serialize)]
struct SystemBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}
```

### Modified types

`NativeChatRequest.system` changes from `Option<String>` to `Option<SystemPrompt>`.

All `NativeContentOut` variants and `NativeToolSpec` gain `#[serde(skip_serializing_if = "Option::is_none")] cache_control: Option<CacheControl>`.

### New methods on `AnthropicProvider`

```rust
fn should_cache_system(text: &str) -> bool { text.len() > 3072 }
fn should_cache_conversation(messages: &[ChatMessage]) -> bool {
    messages.iter().filter(|m| m.role != "system").count() > 4
}
fn apply_cache_to_last_message(messages: &mut [NativeMessage]) {
    if let Some(last_msg) = messages.last_mut() {
        if let Some(last_content) = last_msg.content.last_mut() {
            match last_content {
                NativeContentOut::Text { cache_control, .. } => *cache_control = Some(CacheControl::ephemeral()),
                NativeContentOut::ToolResult { cache_control, .. } => *cache_control = Some(CacheControl::ephemeral()),
                _ => {}
            }
        }
    }
}
```

### Modified methods

**`convert_tools`:** After building `native_tools: Vec<NativeToolSpec>`, set `cache_control = Some(CacheControl::ephemeral())` on the last tool only. This caches all preceding tools too (Anthropic's cache covers the prefix up to the marked point).

**`convert_messages`:** Return `Option<SystemPrompt>` instead of `Option<String>`. After extracting `system_text`, convert:
```rust
let system_prompt = system_text.map(|text| {
    if Self::should_cache_system(&text) {
        SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text,
            cache_control: Some(CacheControl::ephemeral()),
        }])
    } else {
        SystemPrompt::String(text)
    }
});
```

**`chat_with_tools` / chat method:** After `convert_messages`, if `should_cache_conversation(request.messages)`, call `apply_cache_to_last_message(&mut messages)`.

All `NativeContentOut` constructors need `cache_control: None` added.

### Backward compatibility

All `cache_control: None` values serialize with `skip_serializing_if = "Option::is_none"`, so the wire format is identical to before when caching doesn't apply.

### Tests (comprehensive — ~350 lines)

Cover: `CacheControl` serialization; `SystemPrompt` both variants; all `NativeContentOut` variants with/without cache; `NativeToolSpec` with/without cache; `should_cache_system` boundary at 3072/3073 bytes; `should_cache_conversation` boundary at 4/5 non-system messages; `apply_cache_to_last_message` for Text/ToolResult/ToolUse/empty; `convert_tools` cache on last only; `convert_messages` small/large system prompt; backward-compatibility serialization (no `cache_control` field emitted when None).

**Conflict Notes:** Our `src/providers/anthropic.rs` may not have this. Check if our Anthropic provider is the upstream `compatible.rs`-based version or has its own native implementation. The `NativeChatRequest` struct must exist for this to apply. If we use the OpenAI-compatible provider for Anthropic, this is not applicable (and a native Anthropic provider would need to be created first).

---

## Issue 5: Memory — `spawn_blocking` for SQLite (Priority 2)

**Title:** Performance: wrap all blocking SQLite calls in `tokio::task::spawn_blocking`

**Description:**

**Commits:** `4e528dd` (original), `dd45417` (fold-in optimizations)
**Files:** `src/memory/sqlite.rs`

### What changed and why

`SqliteMemory` held a `Mutex<Connection>` and ran synchronous rusqlite queries directly on the Tokio async executor thread. This blocks the executor — other tasks (channel message processing, tool execution) stall while memory operations run. Under concurrent load, this causes latency spikes.

**Fix:** Change `conn: Mutex<Connection>` to `conn: Arc<Mutex<Connection>>` so it can be moved into `spawn_blocking` closures. Wrap every method that touches the database.

### Key pattern

```rust
// BEFORE
pub async fn store(&self, ...) {
    let conn = self.conn.lock();
    conn.execute(...)?;
}

// AFTER
pub async fn store(&self, ...) {
    let conn = self.conn.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let conn = conn.lock();
        conn.execute(...)?;
        Ok(())
    }).await??;
}
```

### `get_or_compute_embedding` split

This method does: (1) cache check (blocking), (2) embedding computation (async HTTP), (3) cache store (blocking). The lock must not be held across the async await in step 2.

```rust
async fn get_or_compute_embedding(&self, text: &str) -> anyhow::Result<Option<Vec<f32>>> {
    // Phase 1: cache check (spawn_blocking)
    let conn = self.conn.clone();
    let cached = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Vec<f32>>> {
        let conn = conn.lock();
        // SELECT + UPDATE accessed_at
    }).await??;

    if cached.is_some() { return Ok(cached); }

    // Phase 2: async embedding
    let embedding = self.embedder.embed_one(text).await?;

    // Phase 3: cache store (spawn_blocking)
    let conn = self.conn.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let conn = conn.lock();
        // INSERT + LRU eviction DELETE
    }).await??;

    Ok(Some(embedding))
}
```

Apply `spawn_blocking` to: `store`, `recall`, `get`, `list`, `forget`, `count`, `health_check`, `reindex` (each step), `vector_search` caller.

### `dd45417` additions (fold-in)

Bundled with the spawn_blocking refactor:

**`vector_search` gains `category` and `session_id` filter params:**
```rust
fn vector_search(conn: &Connection, query_embedding: &[f32], limit: usize,
                  category: Option<&str>, session_id: Option<&str>) -> anyhow::Result<Vec<(String, f32)>>
```
Builds a dynamic SQL query appending `AND category = ?N` and/or `AND session_id = ?N` to reduce full-table scan.

**Batch fetch (N+1 fix):** Replace the per-entry `SELECT ... WHERE id = ?1` loop with a single `SELECT ... WHERE id IN (?,?,...)` query, returning all entries at once via a `HashMap<String, ...>`.

**`MAX_LIKE_KEYWORDS = 8`:** Cap keyword count in the LIKE fallback to improve prepared statement cache efficiency.

**`DEFAULT_LIST_LIMIT = 1000`:** Add `LIMIT ?` to both `list()` queries to prevent unbounded result sets.

**Conflict Notes:** Our MAPLE changes heavily modify `sqlite.rs` — new memory categories, personalization fields, learning worker. The `conn: Arc<Mutex<Connection>>` change is the most invasive part and must be reconciled with our field additions. The batch fetch and keyword cap are additive and should merge cleanly. Check if MAPLE already uses `spawn_blocking`.

---

## Issue 6: Memory — Relevance Threshold + Search Weight Rebalance (Priority 2)

**Title:** Memory: add `min_relevance_score` filtering and rebalance default hybrid search weights

**Description:**

**Commits:** `8a1e7cc`, `decea53`
**Files:** `src/memory/sqlite.rs`, `src/config/schema.rs`, `src/agent/loop_.rs`, `src/agent/memory_loader.rs`, `src/agent/agent.rs`

### What changed and why

Two problems with memory recall quality:

1. **Cross-topic bleed:** Unrelated memories with low hybrid scores were included in context, confusing the agent. No filtering threshold existed.

2. **Weight mismatch:** Default hybrid weights were 70% vector / 30% keyword (as of the `decea53` commit reverting a prior change). The `8a1e7cc` commit had set them to 40/60, but `decea53` reverted to 70/30 while keeping the filtering threshold. **Net result:** weights are 70/30, threshold is 0.4.

### Config changes (`src/config/schema.rs`)

Add `min_relevance_score` to `MemoryConfig`:
```rust
pub struct MemoryConfig {
    // ... existing fields ...
    #[serde(default = "default_min_relevance_score")]
    pub min_relevance_score: f64,
}
fn default_min_relevance_score() -> f64 { 0.4 }
```

Default weights (70/30 vector/keyword):
```rust
fn default_vector_weight() -> f64 { 0.7 }
fn default_keyword_weight() -> f64 { 0.3 }
```

### `DefaultMemoryLoader` changes (`src/agent/memory_loader.rs`)

Add `min_relevance_score: f64` field to `DefaultMemoryLoader`:
```rust
pub struct DefaultMemoryLoader {
    limit: usize,
    min_relevance_score: f64,
}
impl DefaultMemoryLoader {
    pub fn new(limit: usize, min_relevance_score: f64) -> Self { ... }
}
impl Default for DefaultMemoryLoader {
    fn default() -> Self { Self { limit: 5, min_relevance_score: 0.4 } }
}
```

In `MemoryLoader for DefaultMemoryLoader`, filter entries:
```rust
for entry in entries {
    if let Some(score) = entry.score {
        if score < self.min_relevance_score { continue; }
    }
    // add to context
}
```

### `build_context` changes (`src/agent/loop_.rs`)

Add `min_relevance_score: f64` parameter:
```rust
async fn build_context(mem: &dyn Memory, user_msg: &str, min_relevance_score: f64) -> String {
    if let Ok(entries) = mem.recall(user_msg, 5, None).await {
        let relevant: Vec<_> = entries.iter()
            .filter(|e| match e.score {
                Some(score) => score >= min_relevance_score,
                None => true,  // include entries with no score
            }).collect();
        // build context from relevant only
    }
}
```

Call sites: `build_context(mem.as_ref(), &msg, config.memory.min_relevance_score)`.

Also: `max_tool_iterations` threading (overlaps with commit `63602a2` from Issue 3):
- `agent_turn` signature gains `max_tool_iterations: usize` param
- `run_tool_call_loop` gains `max_tool_iterations: usize` param
- Zero value falls back to `DEFAULT_MAX_TOOL_ITERATIONS`

### `agent.rs` changes

```rust
.memory_loader(Box::new(DefaultMemoryLoader::new(5, config.memory.min_relevance_score)))
```

**Conflict Notes:** MAPLE adds new memory categories and a personalization engine. The `min_relevance_score` field in `MemoryConfig` needs to coexist with MAPLE's additions. Check if MAPLE's `MemoryConfig` already has this field. The `build_context` function signature change will conflict with MAPLE modifications to the same function — merge carefully.

---

## Issue 7: New Tool — `web_search` (Priority 2)

**Title:** Tool: add `web_search_tool` with DuckDuckGo and Brave providers

**Description:**

**Commit:** `1757add`
**Files:** `src/tools/web_search_tool.rs` (new), `src/tools/mod.rs`, `src/config/schema.rs`, `Cargo.toml`

### What changed and why

Agents have no way to access current information from the web. This is especially important for GLM/Z.AI models that don't reliably support native tool calling — they need a simple tool with a clear XML tag.

### New `WebSearchConfig` in `src/config/schema.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_web_search_provider")]
    pub provider: String,             // "duckduckgo" or "brave"
    #[serde(default)]
    pub brave_api_key: Option<String>,
    #[serde(default = "default_web_search_max_results")]
    pub max_results: usize,           // default 5
    #[serde(default = "default_web_search_timeout_secs")]
    pub timeout_secs: u64,            // default 15
}
```

Add to `Config` struct: `pub web_search: WebSearchConfig` with `#[serde(default)]`.

### New `src/tools/web_search_tool.rs`

```rust
pub struct WebSearchTool {
    provider: String,
    brave_api_key: Option<String>,
    max_results: usize,
    timeout_secs: u64,
}

impl WebSearchTool {
    pub fn new(provider: String, brave_api_key: Option<String>,
               max_results: usize, timeout_secs: u64) -> Self { ... }
}
```

**Tool name:** `web_search_tool`
**Description:** "Search the web for information. Returns relevant search results with titles, URLs, and descriptions..."
**Parameter schema:** `{ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }`

**DuckDuckGo implementation:**
```rust
async fn search_duckduckgo(&self, query: &str) -> anyhow::Result<String> {
    let encoded_query = urlencoding::encode(query);
    let url = format!("https://html.duckduckgo.com/html/?q={}", encoded_query);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(self.timeout_secs))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) ...")
        .build()?;
    let html = client.get(&url).send().await?.text().await?;
    self.parse_duckduckgo_results(&html, query)
}
```

Parser extracts `<a class="result__a" href="...">Title</a>` links and `<a class="result__snippet">` text using regex. DDG redirect URLs (containing `uddg=`) are decoded via `urlencoding::decode`. Strip HTML tags with `fn strip_tags(content: &str) -> String` using `Regex::new(r"<[^>]+>")`.

**Brave implementation:**
- URL: `https://api.search.brave.com/res/v1/web/search?q={encoded}&count={max_results}`
- Headers: `Accept: application/json`, `X-Subscription-Token: {api_key}`
- Parse JSON: `json["web"]["results"]` array, extract `title`, `url`, `description`

**Registration in `src/tools/mod.rs`:**
```rust
if root_config.web_search.enabled {
    tools.push(Box::new(WebSearchTool::new(
        root_config.web_search.provider.clone(),
        root_config.web_search.brave_api_key.clone(),
        root_config.web_search.max_results,
        root_config.web_search.timeout_secs,
    )));
}
```

**New dependency:** `urlencoding = "2.1"` in `Cargo.toml`. The `regex` crate is already a dep.

**Conflict Notes:** Purely additive. No conflicts with MAPLE or multi-agent. The `WebSearchConfig` field in `Config` needs to be added after MAPLE's fields — check `Config` struct ordering in `schema.rs`. Export `WebSearchConfig` from `src/config/mod.rs` alongside other config types.

---

## Issue 8: New Tool — `pdf_read` (Priority 3)

**Title:** Tool: add `pdf_read` tool for PDF text extraction from workspace

**Description:**

**Commit:** `a5521ab`
**Files:** `src/tools/pdf_read.rs` (new), `src/tools/mod.rs`

### What changed and why

Agents can use `file_read` for text files but have no way to extract content from PDFs. Common use case: reading research papers, contracts, reports placed in the workspace.

### New `src/tools/pdf_read.rs`

```rust
pub struct PdfReadTool {
    security: Arc<SecurityPolicy>,
}
impl PdfReadTool {
    pub fn new(security: Arc<SecurityPolicy>) -> Self { ... }
}
```

**Tool name:** `pdf_read`
**Description:** "Extract plain text from a PDF file in the workspace..."
**Parameters:** `{ path: string (required), max_chars: integer (optional, default 50000, max 200000) }`

**Constants:**
```rust
const MAX_PDF_BYTES: u64 = 50 * 1024 * 1024;   // 50MB
const DEFAULT_MAX_CHARS: usize = 50_000;
const MAX_OUTPUT_CHARS: usize = 200_000;
```

**Security checks (in order):**
1. `security.is_rate_limited()` → return error
2. `security.is_path_allowed(path)` → return error if false
3. `security.record_action()` → return error if false
4. `tokio::fs::canonicalize` → return error on failure
5. `security.is_resolved_path_allowed(&resolved_path)` → return error if escapes workspace
6. Check file size via `tokio::fs::metadata` → return error if > `MAX_PDF_BYTES`
7. Read bytes via `tokio::fs::read`

**Extraction (feature-gated):**
```rust
#[cfg(feature = "rag-pdf")]
{
    let text = tokio::task::spawn_blocking(move || {
        pdf_extract::extract_text_from_mem(&bytes)
    }).await??;
    // truncate to max_chars, return
}

#[cfg(not(feature = "rag-pdf"))]
{
    // Return clear error: "Rebuild with: cargo build --features rag-pdf"
}
```

**Truncation:**
```rust
let output = if text.chars().count() > max_chars {
    let mut truncated: String = text.chars().take(max_chars).collect();
    write!(truncated, "\n\n... [truncated at {max_chars} chars]")?;
    truncated
} else {
    text
};
```

**Registration in `src/tools/mod.rs`:**
```rust
tools.push(Box::new(PdfReadTool::new(security.clone())));
```

The tool is always registered (even without `rag-pdf`) so the LLM gets a clear error rather than a "tool not found" confusion.

**Conflict Notes:** Purely additive. No conflicts with MAPLE or multi-agent. The `rag-pdf` feature likely needs to be added to `Cargo.toml` with `pdf_extract` as an optional dependency.

---

## Issue 9: New Tool — `glob_search` (Priority 3)

**Title:** Tool: add `glob_search` for workspace file pattern search

**Description:**

**Commit:** `955ae92`
**Files:** `src/tools/glob_search.rs` (new), `src/tools/mod.rs`

### What changed and why

Agents use `shell` + `find` for file search: fragile syntax, raw output, broad permissions. A dedicated `glob_search` tool gives structured output and tighter security.

### New `src/tools/glob_search.rs`

```rust
pub struct GlobSearchTool {
    security: Arc<SecurityPolicy>,
}
impl GlobSearchTool {
    pub fn new(security: Arc<SecurityPolicy>) -> Self { ... }
}
```

**Constants:** `const MAX_RESULTS: usize = 1000;`

**Tool name:** `glob_search`
**Description:** "Search for files matching a glob pattern within the workspace. Returns a sorted list of matching file paths relative to the workspace root. Examples: '**/*.rs', 'src/**/mod.rs'."
**Parameters:** `{ pattern: string (required) }`

**Security checks (in `execute`):**
1. `is_rate_limited()` → error
2. Pattern starts with `/` or `\` → error ("Absolute paths are not allowed")
3. Pattern contains `../` or `..\` or is `..` → error ("Path traversal not allowed")
4. `record_action()` → error

**Glob execution:**
```rust
let full_pattern = workspace.join(pattern).to_string_lossy().to_string();
let entries = glob::glob(&full_pattern)?;  // crate: glob = "0.3"
let workspace_canon = std::fs::canonicalize(workspace)?;
```

**Per-entry processing:**
- Skip unreadable entries
- `canonicalize` each path (resolves symlinks)
- `security.is_resolved_path_allowed(&resolved)` — silently filter escapes
- Skip directories (files only)
- `strip_prefix(&workspace_canon)` to get relative path
- Stop at `MAX_RESULTS`, set `truncated = true`

**Output format:**
```
src/tools/mod.rs
src/tools/shell.rs
...
[Results truncated: showing first 1000 of more matches]

Total: 847 files
```

**Registration in `src/tools/mod.rs`:**
```rust
pub mod glob_search;
pub use glob_search::GlobSearchTool;
// In all_tools_with_runtime:
tools.push(Box::new(GlobSearchTool::new(security.clone())));
```

**Dependency:** `glob = "0.3"` — already in `Cargo.toml` (check before adding).

**Conflict Notes:** Purely additive. No conflicts. The `glob` crate is already a dependency per `Cargo.toml`.

---

## Issue 10: Cron — Reliability Fixes (Priority 2)

**Title:** Cron: add timeout, concurrent execution, transaction safety, and composite index

**Description:**

**Commits:** `7de052c`, `5f5cb27`, `21c5f58`, `9967eeb`
**Files:** `src/cron/scheduler.rs`, `src/cron/store.rs`

### What changed and why

Four reliability issues in the cron subsystem:

---

#### a) `7de052c` — Job timeout + concurrent execution (`src/cron/scheduler.rs`)

**Problem 1:** Shell jobs could run indefinitely, blocking the scheduler loop. No timeout was set on `Command::output()`.

**Problem 2:** Jobs ran sequentially (one at a time). Multiple due jobs blocked each other.

**Fix — Timeout:**
```rust
const SHELL_JOB_TIMEOUT_SECS: u64 = 120;
```

Replace `Command::output().await` with `spawn()` + `kill_on_drop(true)` + `time::timeout`:

```rust
async fn run_job_command_with_timeout(
    config: &Config, security: &SecurityPolicy, job: &CronJob, timeout: Duration,
) -> (bool, String) {
    let child = match Command::new("sh")
        .arg("-lc").arg(&job.command)
        .current_dir(&config.workspace_dir)
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return (false, format!("spawn error: {e}")),
    };
    match time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => { /* format stdout+stderr */ (output.status.success(), combined) }
        Ok(Err(e)) => (false, format!("spawn error: {e}")),
        Err(_) => (false, format!("job timed out after {}s", timeout.as_secs_f64())),
    }
}
```

`run_job_command` becomes a thin wrapper calling `run_job_command_with_timeout` with `Duration::from_secs(SHELL_JOB_TIMEOUT_SECS)`.

**Fix — Concurrency:** Extract a `process_due_jobs` function using `futures_util::stream::buffer_unordered`:

```rust
async fn process_due_jobs(config: &Config, security: &Arc<SecurityPolicy>, jobs: Vec<CronJob>) {
    let max_concurrent = config.scheduler.max_concurrent.max(1);
    let mut in_flight = stream::iter(jobs.into_iter().map(|job| {
        let config = config.clone();
        let security = Arc::clone(security);
        async move { execute_and_persist_job(&config, security.as_ref(), &job).await }
    })).buffer_unordered(max_concurrent);

    while let Some((job_id, success)) = in_flight.next().await {
        if !success {
            crate::health::mark_component_error("scheduler", format!("job {job_id} failed"));
        }
    }
}
```

`SecurityPolicy` in `run()` must become `Arc<SecurityPolicy>` to be shared across concurrent futures.

Also add `due_jobs` LIMIT clause: `LIMIT ?1` using `config.scheduler.max_tasks` cast to `i64`.

Add `MAX_CRON_OUTPUT_BYTES = 16 * 1024` and `TRUNCATED_OUTPUT_MARKER = "\n...[truncated]"` in `store.rs` for output size bounding.

**New deps in `scheduler.rs`:** `futures_util::{stream, StreamExt}`, `std::sync::Arc`.

---

#### b) `5f5cb27` — ALTER TABLE race condition (`src/cron/store.rs`)

**Problem:** `add_column_if_missing` checks `PRAGMA table_info` then issues `ALTER TABLE ADD COLUMN`. Concurrent processes can both pass the check, then both issue ALTER — the second fails with "duplicate column name".

**Fix:** Drop stmt/rows handles before ALTER (releases locks), then catch the specific error:

```rust
drop(rows);
drop(stmt);

match conn.execute(&format!("ALTER TABLE cron_jobs ADD COLUMN {name} {sql_type}"), []) {
    Ok(_) => Ok(()),
    Err(rusqlite::Error::SqliteFailure(err, Some(ref msg)))
        if msg.contains("duplicate column name") =>
    {
        tracing::debug!("Column cron_jobs.{name} already exists (concurrent migration): {err}");
        Ok(())
    }
    Err(e) => Err(e).with_context(|| format!("Failed to add cron_jobs.{name}")),
}
```

---

#### c) `21c5f58` — Wrap INSERT+DELETE in transaction (`src/cron/store.rs`)

**Problem:** `record_run()` ran an INSERT followed by a pruning DELETE as separate implicit transactions. If DELETE failed, the run table grew unboundedly.

**Fix:**
```rust
let tx = conn.unchecked_transaction()?;
tx.execute("INSERT INTO cron_runs ...", params![...])?;
tx.execute("DELETE FROM cron_runs WHERE ...", params![...])?;
tx.commit().context("Failed to commit cron run transaction")?;
```

---

#### d) `9967eeb` — Composite index (`src/cron/store.rs`)

**Problem:** The pruning query uses `WHERE job_id = ?1 ORDER BY started_at DESC` — two single-column indexes require SQLite to scan + sort separately.

**Fix:** Add to schema init:
```sql
CREATE INDEX IF NOT EXISTS idx_cron_runs_job_started ON cron_runs(job_id, started_at);
```

---

**Conflict Notes:** Our `src/cron/scheduler.rs` is modified (git status shows unstaged changes). Review our current state before applying. The `Arc<SecurityPolicy>` change is the most invasive for our concurrency model. `SchedulerConfig.max_concurrent` needs to exist in our config schema — check if MAPLE added it.

---

## Issue 11: Channel — Telegram Improvements (Priority 2)

**Title:** Channel: Telegram typing indicator, forum topic support, message length fix, function_calls tag stripping

**Description:**

**Commits:** `b3b1679`, `36062fb`, `c17a0ad` (not found — may be unlisted), `0f8fc72`
**Files:** `src/channels/telegram.rs`

### What changed and why

Four improvements to the Telegram channel:

---

#### a) `b3b1679` — Typing indicator

Telegram's typing indicator expires after 5 seconds. Add persistent typing loop using the same `parking_lot::Mutex<Option<JoinHandle<()>>>` pattern as Discord.

**Add to `TelegramChannel` struct:**
```rust
typing_handle: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
```

Initialize in constructor: `typing_handle: parking_lot::Mutex::new(None)`.

**New methods:**
```rust
async fn start_typing(&self, recipient: &str) -> anyhow::Result<()> {
    self.stop_typing(recipient).await?;
    let client = self.client.clone();
    let url = self.api_url("sendChatAction");
    let chat_id = recipient.to_string();
    let handle = tokio::spawn(async move {
        loop {
            let body = json!({"chat_id": &chat_id, "action": "typing"});
            let _ = client.post(&url).json(&body).send().await;
            tokio::time::sleep(Duration::from_secs(4)).await; // Expires at 5s
        }
    });
    *self.typing_handle.lock() = Some(handle);
    Ok(())
}

async fn stop_typing(&self, _recipient: &str) -> anyhow::Result<()> {
    if let Some(handle) = self.typing_handle.lock().take() {
        handle.abort();
    }
    Ok(())
}
```

---

#### b) `36062fb` — Forum topic support

Telegram supergroups can have "forum mode" with topic threads. Replies must include `message_thread_id` to stay in the right topic.

**In `parse_update_message`:** Extract `message_thread_id` from the message JSON and encode into `reply_target` as `"chat_id:thread_id"` format:

```rust
let thread_id = message.get("message_thread_id")
    .and_then(serde_json::Value::as_i64)
    .map(|id| id.to_string());

let reply_target = if let Some(tid) = thread_id {
    format!("{}:{}", chat_id, tid)
} else {
    chat_id.clone()
};
```

**In `send()` / `send_text_chunks` / `send_attachment` / all media send methods:** Parse `recipient` to extract optional `thread_id`:

```rust
let (chat_id, thread_id) = if let Some(idx) = recipient.find(':') {
    (&recipient[..idx], Some(&recipient[idx+1..]))
} else {
    (recipient, None)
};
```

Pass `thread_id: Option<&str>` to all send sub-methods. When `Some(tid)`, add `"message_thread_id": tid` to all JSON bodies and `form.text("message_thread_id", tid)` to multipart forms.

Affected signatures: `send_text_chunks(message, chat_id, thread_id)`, `send_attachment(chat_id, thread_id, attachment)`, `send_document(chat_id, thread_id, path, caption)`, `send_document_bytes(chat_id, thread_id, bytes, filename, caption)`, and all `send_photo`, `send_video`, `send_audio`, `send_voice` variants.

---

#### c) `0f8fc72` — Message length fix + `function_calls` tag stripping

**Problem 1:** `split_message_for_telegram` splits at exactly 4096 chars, but `send_text_chunks` appends `"(continued)\n\n"` and `"\n\n(continues...)"` (~30 chars), pushing sent messages over Telegram's limit.

**Fix:** Add `const TELEGRAM_CONTINUATION_OVERHEAD: usize = 30;`. Use `chunk_limit = TELEGRAM_MAX_MESSAGE_LENGTH - TELEGRAM_CONTINUATION_OVERHEAD` as the hard split point for non-final chunks. Final chunk (when remainder fits in 4096) takes the full limit.

```rust
let chunk_limit = TELEGRAM_MAX_MESSAGE_LENGTH - TELEGRAM_CONTINUATION_OVERHEAD;
// For non-final chunks, split at chunk_limit instead of TELEGRAM_MAX_MESSAGE_LENGTH
// Early exit: if remaining.chars().count() <= TELEGRAM_MAX_MESSAGE_LENGTH, take all
```

**Problem 2:** `strip_tool_call_tags` didn't handle `<function_calls>` / `<function_call>` wrapper tags. Unstripped angle brackets break Telegram's Markdown parser.

**Fix:** Add to `TOOL_CALL_OPEN_TAGS`:
```rust
const TOOL_CALL_OPEN_TAGS: [&str; 7] = [
    "<function_calls>",
    "<function_call>",
    "<tool_call>",
    "<toolcall>",
    "<tool-call>",
    // ... existing entries
];
```

Add to `matching_close_tag`:
```rust
"<function_calls>" => Some("</function_calls>"),
"<function_call>" => Some("</function_call>"),
```

---

**Conflict Notes:** Our `src/channels/telegram.rs` is modified (git status shows unstaged changes). The typing indicator uses `parking_lot::Mutex` — check if already in our Cargo.toml (it's used in Discord). Forum topic parsing changes `reply_target` format from a plain string to a `chat_id:thread_id` format — any code that reads `reply_target` and passes it back to Telegram must handle the colon-split.

---

## Issue 12: Channel — Discord `mention_only` Mode (Priority 3)

**Title:** Channel: Discord `mention_only` config — only respond to @-mention messages

**Description:**

**Commit:** `5b5d9fe`
**Files:** `src/channels/discord.rs`, `src/channels/mod.rs`

### What changed and why

In guild channels, the bot receives every message. By default it processes all of them. Many deployments want the bot to only respond when explicitly @-mentioned, silently ignoring other traffic.

### Changes

**`DiscordConfig` in `src/config/schema.rs`:** Add `mention_only: bool` field (default `false`).

**`DiscordChannel::new` signature gains `mention_only: bool` param:**
```rust
pub fn new(
    bot_token: String,
    guild_id: Option<String>,
    allowed_users: Vec<String>,
    listen_to_bots: bool,
    mention_only: bool,   // NEW
) -> Self
```

**In the WebSocket message polling loop** (where messages are processed), after the existing bot/allowlist checks:

```rust
if self.mention_only {
    let mention_tag = format!("<@{bot_user_id}>");
    if !content.contains(&mention_tag) {
        continue;  // silently ignore
    }
}

let clean_content = if self.mention_only {
    let mention_tag = format!("<@{bot_user_id}>");
    content.replace(&mention_tag, "").trim().to_string()
} else {
    content.to_string()
};
```

Use `clean_content` instead of `content.to_string()` when building `ChannelMessage`.

**In `src/channels/mod.rs`** (where `DiscordChannel::new` is called with config): Add `dc.mention_only` to the constructor call.

**All existing tests** need `false` added as the fifth arg to `DiscordChannel::new(...)`.

**Conflict Notes:** Our `src/channels/discord.rs` is unmodified. Straightforward addition. Check if `DiscordConfig` in `schema.rs` already has this field — it was part of the upstream analysis in session #1143.

---

## Issue 13: Memory — Lucid Recall Timeout Fix (Priority 2)

**Title:** Memory: increase Lucid recall timeout from 120ms to 500ms and add cold-start test

**Description:**

**Commit:** `bc18b8d`
**Files:** `src/memory/lucid.rs`

### What changed and why

The `LucidMemory` backend calls the external `lucid` CLI. The timeout was 120ms — insufficient for CLI cold starts (JVM initialization, process startup) on slower machines or when the system is under load. When the timeout fires prematurely, `LucidMemory::recall()` falls back to local-only SQLite memory and records a failure, starting a 15-second cooldown that prevents further Lucid calls. This causes users to lose access to their Lucid memories silently.

### Fix

Change the constant:

```rust
// BEFORE
const DEFAULT_RECALL_TIMEOUT_MS: u64 = 120;

// AFTER
// Lucid CLI cold start can exceed 120ms on slower machines, which causes
// avoidable fallback to local-only memory and premature cooldown.
const DEFAULT_RECALL_TIMEOUT_MS: u64 = 500;
```

Update all test instantiations that pass the timeout explicitly from `Duration::from_millis(120)` to `Duration::from_millis(500)`.

### New test: cold-start delay within timeout

Add a `write_delayed_lucid_script` helper that writes a shell script simulating a 200ms delay on the `context` subcommand:

```bash
if [[ "${1:-}" == "context" ]]; then
    sleep 0.2  # Simulate cold start slower than 120ms but below 500ms
    cat <<'EOF'
<lucid-context>
- [decision] Delayed token refresh guidance
</lucid-context>
EOF
    exit 0
fi
```

Test: store a local entry, recall with delay script as the `lucid` binary, assert both the local entry and the Lucid delayed entry appear in results.

**Conflict Notes:** Our MAPLE changes may have modified `src/memory/lucid.rs` — the session context mentions MAPLE touched composite memory. Check our current `DEFAULT_RECALL_TIMEOUT_MS` value before applying.

---

*End of issues. Total: 13 issues across security, agent loop, memory, tools, channels.*
