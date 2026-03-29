# RustyClaw Memory Architecture & DeerFlow Comparative Analysis

> Research document — March 28, 2026
> Covers: memory subsystem internals, DeerFlow 2.0 evaluation, Hermes self-learning loop, fit assessment

---

## Table of Contents

1. [Current Memory Architecture](#1-current-memory-architecture)
2. [DeerFlow Analysis](#2-deerflow-analysis)
3. [Hermes Self-Learning Loop](#3-hermes-self-learning-loop)
4. [Fit Assessment](#4-fit-assessment)
5. [Recommendations](#5-recommendations)

---

## 1. Current Memory Architecture

### 1.1 Overview

RustyClaw's memory system is a trait-driven, multi-backend architecture with hybrid search (vector + keyword), lifecycle management (hygiene, snapshots, hydration), security scanning, and an optional self-learning bridge (Lucid). All backends are interchangeable at config time via the `Memory` trait.

```
┌──────────────────────────────────────────────────────────────────┐
│                      Agent Runtime                                │
│  memory_store / memory_recall / memory_forget (tools)            │
│  DefaultMemoryLoader (context injection)                         │
└──────────────┬───────────────────────────────────────────────────┘
               │
┌──────────────▼───────────────────────────────────────────────────┐
│                    Memory Trait (traits.rs)                        │
│  store() / recall() / get() / list() / forget() / count()        │
│  health_check()                                                   │
└──────────────┬───────────────────────────────────────────────────┘
               │
       ┌───────┼───────┬───────┬───────┬───────┬───────┐
       │       │       │       │       │       │       │
    SQLite  Lucid  Postgres  Qdrant  Markdown  None  Scanned
    (hybrid  (CLI    (keyword  (vector  (file    (noop) (security
     search)  sync)   search)  search)  search)         decorator)
```

### 1.2 Core Trait: `Memory`

Defined in `src/memory/traits.rs`:

```rust
#[async_trait]
pub trait Memory: Send + Sync {
    fn name(&self) -> &str;
    async fn store(&self, key: &str, content: &str, category: MemoryCategory,
                   session_id: Option<&str>) -> anyhow::Result<()>;
    async fn recall(&self, query: &str, limit: usize,
                    session_id: Option<&str>) -> anyhow::Result<Vec<MemoryEntry>>;
    async fn get(&self, key: &str) -> anyhow::Result<Option<MemoryEntry>>;
    async fn list(&self, category: Option<&MemoryCategory>,
                  session_id: Option<&str>) -> anyhow::Result<Vec<MemoryEntry>>;
    async fn forget(&self, key: &str) -> anyhow::Result<bool>;
    async fn count(&self) -> anyhow::Result<usize>;
    async fn health_check(&self) -> bool;
}
```

**MemoryEntry:**
- `id`, `key`, `content`, `category: MemoryCategory`, `timestamp`, `session_id`, `score`

**MemoryCategory:**
- `Core` — Long-term facts, preferences, decisions
- `Daily` — Daily session logs
- `Conversation` — Conversation context
- `Custom(String)` — User-defined

### 1.3 Backend Registry

`src/memory/backend.rs` defines the backend type system:

| Backend | Key | Hybrid Search | Auto-Save | SQLite-Based | Optional Dep |
|---------|-----|--------------|-----------|-------------|-------------|
| SQLite | `sqlite` | Vector + FTS5 BM25 | true | yes | no |
| Lucid | `lucid` | Via SQLite fallback + CLI | true | yes | yes |
| PostgreSQL | `postgres` | Keyword (ILIKE) only | true | no | no |
| Qdrant | `qdrant` | Vector (cosine) only | true | no | no |
| Markdown | `markdown` | Keyword (ratio) only | true | no | no |
| None | `none` | N/A | false | no | no |

Default backend: **sqlite** (recommended).

Each backend has a `MemoryBackendProfile` with metadata: `key`, `label`, `auto_save_default`, `uses_sqlite_hygiene`, `sqlite_based`, `optional_dependency`.

### 1.4 SQLite Backend — The Full-Stack Engine

`src/memory/sqlite.rs` — The most capable backend, providing hybrid vector + keyword search.

**Schema:**
- `memories` table: `id` (PK), `key` (UNIQUE), `content`, `category`, `embedding` (BLOB), `created_at`, `updated_at`, `session_id`
- `memories_fts` (FTS5 virtual table): Full-text index on `key + content` with BM25 scoring
- Auto-sync triggers (INSERT/UPDATE/DELETE) keep FTS5 in sync
- `embedding_cache` table: SHA-256 content hash (PK), `embedding` (BLOB), `created_at`, `accessed_at`
- Index on `accessed_at` for LRU eviction

**Performance tuning (PRAGMAs):**
- `journal_mode = WAL` — Concurrent reads during writes
- `synchronous = NORMAL` — 2x write speed
- `mmap_size = 8MB` — OS page-cache for hot reads
- `cache_size = -2000` — ~500 hot pages
- `temp_store = MEMORY` — Temp tables in memory

**Hybrid search algorithm:**

1. **FTS5 BM25 path** — Quotes and joins query keywords with OR; scores: key_match(2.0) + content_match(1.0)
2. **Vector similarity path** — Computes query embedding, calculates cosine similarity against all stored vectors
3. **Merge** — Normalizes keyword scores by max, computes `final_score = vector_weight × vec_score + keyword_weight × kw_score`, deduplicates by id, returns top N

**Store path:**
1. Compute embedding (async HTTP to provider)
2. Check embedding cache (SHA-256 content hash)
3. Compute if cache miss
4. LRU evict on cache overflow
5. INSERT OR REPLACE into memories (blocking thread)
6. FTS5 auto-synced via trigger

**Key struct fields:**
- `conn: Arc<Mutex<Connection>>` — Single shared connection
- `embedder: Arc<dyn EmbeddingProvider>` — Embedding service
- `vector_weight: f32` (default 0.7), `keyword_weight: f32` (default 0.3)
- `cache_max: usize` (default 10,000)

### 1.5 PostgreSQL Backend

`src/memory/postgres.rs` — Remote keyword-only backend.

**Schema:** Standard table with `id`, `key` (UNIQUE), `content`, `category`, `created_at`, `updated_at`, `session_id`. Indexes on category, session_id, updated_at DESC.

**Recall:** ILIKE keyword matching only:
```sql
CASE WHEN key ILIKE '%query%' THEN 2.0 ELSE 0.0 END +
CASE WHEN content ILIKE '%query%' THEN 1.0 ELSE 0.0 END AS score
```

No vector search — users wanting vectors should use Qdrant or SQLite. Connection timeout capped at 300s. Identifier validation (schema/table names must be alphanumeric + underscore).

### 1.6 Qdrant Backend

`src/memory/qdrant.rs` — Vector-only backend via REST API.

- `base_url`, `collection` (default: "rustyclaw_memories"), optional `api_key`
- Lazy initialization (`OnceCell`) — creates collection on first operation
- Collection uses cosine distance
- Store: POST /points with embedding
- Recall: POST /search with query embedding
- If noop embedder (dims = 0), operates as metadata-only storage

### 1.7 Markdown Backend

`src/memory/markdown.rs` — Human-readable file storage.

- `workspace/MEMORY.md` — Core memories (append-only)
- `workspace/memory/YYYY-MM-DD.md` — Daily logs
- Recall: Reads all files, scores by keyword match ratio, returns top N
- No vector search, no FTS5 — pure keyword matching

### 1.8 Lucid Backend — Self-Learning Bridge

`src/memory/lucid.rs` — Bridges to the external `lucid-memory` CLI for federated/extended memory, with SQLite as fallback.

**Architecture:**
- `local: SqliteMemory` — Always-available fallback
- `lucid_cmd: String` — CLI command (env: `RUSTYCLAW_LUCID_CMD`, default: "lucid")
- Configurable timeouts: recall (500ms), store (800ms)
- Failure cooldown: 15s after lucid failure (prevents thrashing)

**Category mapping:**
- `Core` ↔ "decision", `Daily` ↔ "context", `Conversation` ↔ "conversation", `Custom` ↔ "learning"

**Recall strategy:**
1. Try local SQLite first (fast path)
2. If < `local_hit_threshold` (default 3) results, invoke lucid CLI with timeout
3. Parse lucid output (looks for `<lucid-context>...</lucid-context>` XML block)
4. Merge results, deduplicate by (key + content signature)
5. On lucid failure → enter cooldown, use local-only for 15s

**Store strategy:**
- Write to local SQLite first (always)
- Async sync to lucid CLI with timeout
- Lucid failure doesn't block local write

**Environment variables:**
- `RUSTYCLAW_LUCID_CMD`, `RUSTYCLAW_LUCID_BUDGET` (default 200 tokens)
- `RUSTYCLAW_LUCID_RECALL_TIMEOUT_MS`, `RUSTYCLAW_LUCID_STORE_TIMEOUT_MS`
- `RUSTYCLAW_LUCID_LOCAL_HIT_THRESHOLD`, `RUSTYCLAW_LUCID_FAILURE_COOLDOWN_MS`

### 1.9 None Backend

`src/memory/none.rs` — Explicit no-op. All operations return empty/success. `health_check()` returns true.

### 1.10 Scanned Memory — Security Decorator

`src/memory/scanning.rs` — Wraps any Memory backend with `ContentScanner` validation.

- On `store()`: scans key + content for injection/exfiltration patterns
- Rejects High/Medium severity findings
- All other operations pass through to inner backend
- Uses `ContentScanner` from the security module

### 1.11 Embedding System

`src/memory/embeddings.rs`:

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;
    fn dimensions(&self) -> usize;
    async fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>>;
    async fn embed_one(&self, text: &str) -> anyhow::Result<Vec<f32>>;
}
```

**Implementations:**
1. **NoopEmbedding** — Returns 0 dimensions, empty vectors (keyword-only fallback)
2. **OpenAiEmbedding** — OpenAI-compatible API supporting:
   - `openai` (api.openai.com)
   - `openrouter` (openrouter.ai/api/v1)
   - `custom:URL` (any OpenAI-compatible endpoint)
   - Auto-detects endpoint path (/v1/embeddings vs /embeddings)

**Factory:** `create_embedding_provider(provider, api_key, model, dims)` → `Box<dyn EmbeddingProvider>`

**Embedding route resolution (`mod.rs`):**
- `resolve_embedding_config()` routes `hint:semantic`-style model specs to dedicated `EmbeddingRouteConfig` entries
- Falls back to base `[memory]` config values

### 1.12 Vector Math

`src/memory/vector.rs`:

- `cosine_similarity(a, b) -> f32` — Returns 0.0–1.0, handles NaN/infinity/zero vectors, uses f64 intermediate
- `vec_to_bytes(v) -> Vec<u8>` — Little-endian serialization
- `bytes_to_vec(bytes) -> Vec<f32>` — Deserialize from little-endian
- `hybrid_merge(vector_results, keyword_results, vector_weight, keyword_weight, limit) -> Vec<ScoredResult>` — Normalizes keyword scores, computes weighted sum, deduplicates by id

### 1.13 Text Chunking

`src/memory/chunker.rs`:

```rust
pub fn chunk_markdown(text: &str, max_tokens: usize) -> Vec<Chunk>
```

Strategy:
1. Split on markdown headings (#, ##, ###)
2. If section > max_chars (max_tokens × 4), split on blank lines (paragraphs)
3. If paragraph still too big, split on line boundaries
4. Preserves heading context in all sub-chunks

### 1.14 Snapshots — Cold Boot Recovery

`src/memory/snapshot.rs`:

**Export (`export_snapshot()`):**
- Queries all Core memories from SQLite
- Writes `MEMORY_SNAPSHOT.md` as human-readable markdown with key, content, timestamps
- Git-visible "soul" backup

**Hydration (`hydrate_from_snapshot()`):**
- Triggered if `brain.db` < 4096 bytes AND `MEMORY_SNAPSHOT.md` exists
- Parses markdown, inserts as Core memories into fresh SQLite DB
- Cold boot recovery from markdown backup

### 1.15 Memory Hygiene

`src/memory/hygiene.rs` — Runs every 12 hours:

1. **Archive daily memory files** older than `archive_after_days` (default 7) → `memory/archive/`
2. **Archive session files** older than `archive_after_days` → `sessions/archive/`
3. **Purge memory archives** older than `purge_after_days` (default 30)
4. **Purge session archives** older than `purge_after_days`
5. **Prune conversation rows** in SQLite older than `conversation_retention_days` (default 30)

State tracked in `memory_hygiene_state.json` (last_run_at + report).

### 1.16 Response Cache

`src/memory/response_cache.rs` — Token-saving LLM response cache.

**Storage:** Separate SQLite DB at `workspace/memory/response_cache.db`

**Schema:**
```sql
CREATE TABLE response_cache (
    prompt_hash TEXT PRIMARY KEY,  -- SHA-256 of (model | system_prompt | user_prompt)
    model TEXT NOT NULL,
    response TEXT NOT NULL,
    token_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    accessed_at TEXT NOT NULL,
    hit_count INTEGER NOT NULL DEFAULT 0
);
```

**Operations:**
- `cache_key()` — SHA-256 of (model | system_prompt | user_prompt)
- `get()` — Lookup by key, check TTL (default 60 min), bump hit_count on hit
- `put()` — INSERT OR REPLACE, evict expired, LRU evict if over max_entries (default 5,000)
- `stats()` → (count, total_hits, tokens_saved)

Disabled by default (`response_cache_enabled = false`).

### 1.17 Memory Tools (Agent-Facing)

Three tools expose memory operations to the agent:

**`memory_store`** (`src/tools/memory_store.rs`):
```json
{ "key": "string", "content": "string", "category": "core|daily|conversation|custom_name" }
```
- Parses category (defaults to Core)
- Security check: `enforce_tool_operation(ToolOperation::Act)`
- Scans key + content via ContentScanner
- Calls `memory.store()`

**`memory_recall`** (`src/tools/memory_recall.rs`):
```json
{ "query": "string", "limit": "integer (default 5)" }
```
- Calls `memory.recall(query, limit)`
- Formats: `Found N memories: - [category] key: content [score%]`

**`memory_forget`** (`src/tools/memory_forget.rs`):
```json
{ "key": "string" }
```
- Security check: `enforce_tool_operation(ToolOperation::Act)`
- Blocked in ReadOnly mode; respects rate limiting
- Calls `memory.forget(key)`

### 1.18 Agent Memory Loading

`src/agent/memory_loader.rs` — Context injection into agent prompts.

```rust
#[async_trait]
pub trait MemoryLoader: Send + Sync {
    async fn load_context(&self, memory: &dyn Memory, user_message: &str) -> anyhow::Result<String>;
}
```

**DefaultMemoryLoader** (limit=5, min_relevance_score=0.4):
1. Recalls top `limit` results for user_message
2. Skips `assistant_resp_*` legacy autosave entries (untrusted)
3. Filters out scores < `min_relevance_score`
4. Formats as `[Memory context]\n- key: content\n- key: content`
5. Returns empty string if nothing passes threshold

### 1.19 Configuration

`src/config/schema.rs` — `MemoryConfig`:

```rust
pub struct MemoryConfig {
    // Backend
    pub backend: String,                      // "sqlite"|"lucid"|"postgres"|"qdrant"|"markdown"|"none"
    pub auto_save: bool,                      // Auto-save user input

    // Hygiene
    pub hygiene_enabled: bool,                // Default: true
    pub archive_after_days: u32,              // Default: 7
    pub purge_after_days: u32,                // Default: 30
    pub conversation_retention_days: u32,     // Default: 30

    // Embeddings
    pub embedding_provider: String,           // "none"|"openai"|"openrouter"|"custom:URL"
    pub embedding_model: String,              // Default: "text-embedding-3-small"
    pub embedding_dimensions: usize,          // Default: 1536
    pub vector_weight: f64,                   // Default: 0.7
    pub keyword_weight: f64,                  // Default: 0.3
    pub min_relevance_score: f64,             // Default: 0.4
    pub embedding_cache_size: usize,          // Default: 10,000
    pub chunk_max_tokens: usize,              // Default: 512

    // Response Cache
    pub response_cache_enabled: bool,         // Default: false
    pub response_cache_ttl_minutes: u32,      // Default: 60
    pub response_cache_max_entries: usize,    // Default: 5,000

    // Snapshot
    pub snapshot_enabled: bool,               // Default: false
    pub snapshot_on_hygiene: bool,            // Default: false
    pub auto_hydrate: bool,                   // Default: true

    // Backend-specific
    pub sqlite_open_timeout_secs: Option<u64>,
    pub qdrant: QdrantConfig,                 // url, collection, api_key
}
```

### 1.20 Factory & Startup Lifecycle

`src/memory/mod.rs` — Memory creation follows this sequence:

```
create_memory() / create_memory_with_storage()
    │
    ├─ 1. Resolve backend name (with optional storage override)
    ├─ 2. Route embedding config (hint: lookup via EmbeddingRouteConfig)
    ├─ 3. Run hygiene if due (12-hour interval check)
    ├─ 4. Export snapshot if enabled + sqlite-based
    ├─ 5. Auto-hydrate if brain.db empty + snapshot exists
    └─ 6. Initialize backend (create embedder, open DB, apply PRAGMAs/migrations)
```

Public factory functions:
- `create_memory()` — Standard path
- `create_memory_with_storage()` — With optional storage provider override
- `create_memory_with_storage_and_routes()` — With embedding route configs
- `create_response_cache()` — Optional response cache factory
- `effective_memory_backend_name()` — Resolves backend with storage override
- `is_assistant_autosave_key()` — Detects legacy untrusted entries

### 1.21 Elixir Layer

The Elixir orchestration layer (`elixir/rustyclaw_orchestrator/`) has **no memory-related modules**. Per the architecture design (`docs/ELIXIR_ORCHESTRATION_DESIGN.md`), memory stays entirely in the Rust core — Elixir owns agent lifecycle, registry, coordination, and sessions, but all memory operations route through Rust via the bridge.

---

## 2. DeerFlow Analysis

### 2.1 What Is DeerFlow?

**DeerFlow** (Deep Exploration and Efficient Research Flow) is ByteDance's open-source agent harness — a full-stack AI agent system built on **LangGraph + LangChain** (Python 3.12+) with a Next.js frontend and Nginx reverse proxy. Version 2.0 is a ground-up rewrite sharing no code with v1.

It positions itself as a batteries-included runtime for complex multi-step tasks: research reports, slide decks, data analysis, and code generation.

**Stack:**
```
Nginx (:2026) — unified entry
  ├── /api/langgraph/* → LangGraph Server (:2024) — agent runtime
  ├── /api/*           → Gateway API (:8001) — FastAPI REST
  └── /                → Frontend (:3000) — Next.js
```

### 2.2 Architecture

DeerFlow uses a **single lead agent with a middleware-based cross-cutting concern chain**, not a multi-graph LangGraph workflow.

**Core runtime:** A single LangGraph agent (`lead_agent`), created via `make_lead_agent(config)`. This calls `create_agent()` with a dynamically assembled tool list and system prompt.

**Middleware chain (12 middlewares, strict order):**
1. ThreadData — workspace/uploads/outputs path setup
2. Uploads — file upload handling
3. Sandbox — Docker/local sandbox lifecycle
4. DanglingToolCall — orphaned tool call cleanup
5. Guardrails — pre-tool-call authorization
6. Summarization — older message compression
7. TodoList — task tracking
8. Title — auto-generated conversation title
9. Memory — persistent context extraction
10. ViewImage — image content injection
11. SubagentLimit — concurrent sub-agent cap
12. LoopDetection — repetitive tool call detection
13. Clarification — ambiguity resolution

**Subagent delegation:** The lead agent can call a `task()` tool to spawn sub-agents (general-purpose or bash specialist) running in background thread pools (max 3 concurrent, 15-minute timeout).

**Key distinction from RustyClaw:** There is no explicit DAG, state machine, or workflow graph. The LLM itself decides task decomposition via prompt engineering (a large `<subagent_system>` prompt section). This is fundamentally different from RustyClaw's structured agent loop.

### 2.3 DeerFlow's Memory System

This is the most relevant comparison point.

**Architecture:**
```
MemoryMiddleware (after_agent hook)
    → MemoryUpdateQueue (debounced, 30s)
        → MemoryUpdater (LLM-driven extraction)
            → FileMemoryStorage (single JSON file)
```

**Storage format** (`backend/.deer-flow/memory.json`):
```json
{
  "version": "1.0",
  "lastUpdated": "...",
  "user": {
    "workContext": {"summary": "...", "updatedAt": "..."},
    "personalContext": {"summary": "...", "updatedAt": "..."},
    "topOfMind": {"summary": "...", "updatedAt": "..."}
  },
  "history": {
    "recentMonths": {"summary": "...", "updatedAt": "..."},
    "earlierContext": {"summary": "...", "updatedAt": "..."},
    "longTermBackground": {"summary": "...", "updatedAt": "..."}
  },
  "facts": [
    {
      "id": "fact_xxx",
      "content": "...",
      "category": "preference|knowledge|context|behavior|goal",
      "confidence": 0.0-1.0,
      "createdAt": "...",
      "source": "thread_id"
    }
  ]
}
```

**Update flow:**
1. `MemoryMiddleware.after_agent()` filters messages (keeps user inputs + final AI responses, strips tool calls, strips `<uploaded_files>` blocks)
2. Queues filtered conversation into `MemoryUpdateQueue`
3. Queue debounces (30s default), deduplicates per-thread
4. Background thread calls `MemoryUpdater.update_memory()`:
   - Loads current memory JSON
   - Sends conversation + current memory to LLM via `MEMORY_UPDATE_PROMPT`
   - LLM returns JSON update (add/remove facts, update summaries)
   - Applies updates, strips upload mentions, deduplicates facts
   - Enforces `max_facts` limit (100, sorted by confidence)
   - Atomic save (temp file + rename)
5. On next interaction, `format_memory_for_injection()` injects top 15 facts + context summaries into `<memory>` tags (2000 token budget)

**Storage abstraction:** `MemoryStorage` ABC with `load()`, `reload()`, `save()`. Default: `FileMemoryStorage`. Configurable via `config.yaml`, but no alternative implementations are shipped.

### 2.4 DeerFlow State Management

**ThreadState** extends LangChain's AgentState:
- `sandbox` — SandboxState with sandbox_id
- `thread_data` — workspace/uploads/outputs paths
- `title` — auto-generated conversation title
- `artifacts` — list of artifact paths
- `todos` — task list
- `uploaded_files`, `viewed_images`

**Persistence:** LangGraph's built-in checkpointer (memory, SQLite, or PostgreSQL). Per-thread filesystem isolation.

### 2.5 DeerFlow Tool System

Tools assembled dynamically per invocation:
- **Config-defined** (via reflection from config.yaml)
- **MCP tools** (multi-server, lazy-loaded, stdio/SSE/HTTP transports)
- **Built-in:** `present_files`, `ask_clarification`, `view_image`
- **Sandbox:** `bash`, `ls`, `read_file`, `write_file`, `str_replace`
- **Community:** Tavily, Jina AI, Firecrawl, DuckDuckGo, InfoQuest
- **Subagent:** `task` (spawns sub-agents)
- **ACP:** `invoke_acp_agent` (Agent Communication Protocol)

### 2.6 DeerFlow Provider Support

Model-agnostic via LangChain class resolution:
- `langchain_openai:ChatOpenAI` (OpenAI, OpenRouter, any compatible gateway)
- `langchain_anthropic:ChatAnthropic` (Claude)
- `langchain_google_genai:ChatGoogleGenerativeAI` (Gemini)
- Custom patched providers for DeepSeek, Volcengine Doubao
- Claude Code OAuth flow, Codex CLI wrappers

### 2.7 DeerFlow Strengths

1. **Full-stack polish** — Frontend + backend + reverse proxy + Docker compose, ready to deploy
2. **LangGraph/LangChain ecosystem** — Mature model abstraction, tool calling, LangSmith tracing
3. **Middleware architecture** — Clean separation of cross-cutting concerns
4. **Skills system** — Progressive loading of domain-specific workflows via Markdown files
5. **Sandbox isolation** — Per-thread Docker containers with virtual path translation
6. **IM channel integration** — Telegram, Slack, Feishu out of the box
7. **Guardrails** — Pluggable pre-tool-call authorization
8. **Loop detection** — Hash-based detection of repetitive tool calls (sliding window, warn threshold, hard stop)
9. **Debounced memory queue** — Per-thread deduplication with configurable debounce
10. **Upload scrubbing** — Regex filtering strips file paths from long-term memory

### 2.8 DeerFlow Weaknesses

1. **Primitive memory** — Single JSON file, no vector search, no semantic recall, 100-fact cap with confidence sorting
2. **No autonomous reflection** — Memory only updates reactively from conversations
3. **No planning graph** — Research workflows are entirely prompt-driven, no formal verification/rollback
4. **Single-process** — No distributed execution, sub-agent thread pools limited to 3 workers
5. **LangGraph lock-in** — Tight version pinning on LangGraph 1.0.6–1.0.10 and LangChain 1.2.3+
6. **No multi-user isolation** — Memory is global (or per-agent, not per-user)
7. **Isolated subagent context** — Sub-agents cannot see parent or sibling context
8. **No response caching** — Every LLM call is fresh
9. **Coarse summarization** — Keeps N most recent messages, summarizes the rest (no semantic importance weighting)
10. **Static skills** — Markdown instruction templates that can't execute code themselves

---

## 3. Hermes Self-Learning Loop

### 3.1 How Lucid/Hygiene/Scanning Relate to Self-Improvement

RustyClaw's memory system contains the building blocks of a self-learning loop, though they are currently wired as independent subsystems rather than a unified cycle. Here's how each module contributes:

#### Lucid Memory (`lucid.rs`) — External Knowledge Bridge

The Lucid backend bridges to an external `lucid-memory` CLI, enabling a form of federated memory:
- **Store:** Writes locally first, then syncs to lucid CLI asynchronously
- **Recall:** Queries local first, augments with lucid results if local is sparse
- **Category mapping:** Maps Core→"decision", Daily→"context", Conversation→"conversation", Custom→"learning"
- **Self-learning potential:** The `lucid` CLI can run its own reflection/consolidation cycles independently. RustyClaw doesn't control this — it simply syncs in both directions. This is the closest thing to autonomous self-improvement in the current system.

The **learning** category mapping for Custom memories is telling — it suggests the system was designed with the intent that agents would classify their acquired knowledge distinctly from facts and preferences.

#### Memory Hygiene (`hygiene.rs`) — Forgetting Curve

Hygiene implements a time-based forgetting curve:
- Daily files archived after 7 days, purged after 30
- Session files follow the same lifecycle
- Conversation rows pruned from SQLite after 30 days
- Core memories are never touched by hygiene (they're permanent)

This is a **passive** self-improvement mechanism: by removing stale context, the system prevents recall from being diluted by irrelevant old data. It doesn't actively consolidate or re-evaluate memories — it just applies retention policies.

#### Content Scanning (`scanning.rs`) — Quality Gate

The ScannedMemory decorator acts as a quality gate:
- Rejects injection/exfiltration patterns on store
- Prevents prompt injection from persisting in memory
- This is a **defensive** self-improvement mechanism — it prevents the memory from being poisoned

#### Memory Snapshots (`snapshot.rs`) — Soul Persistence

Snapshots serve two self-learning purposes:
1. **Cold boot recovery** — The agent can restart from a checkpoint, preserving accumulated knowledge
2. **Git visibility** — MEMORY_SNAPSHOT.md makes the agent's "soul" visible, auditable, and version-controlled

This isn't self-learning per se, but it ensures that learned knowledge survives infrastructure failures.

#### Response Cache (`response_cache.rs`) — Behavioral Memoization

The response cache is a form of behavioral learning:
- If the agent has answered a similar question before (same model + system + user prompt hash), it can reuse the response
- TTL-based expiry ensures stale responses don't persist
- This is **implicit** learning — the system doesn't reason about what it cached, it just avoids redundant work

### 3.2 The Hermes Loop (Conceptual)

The term "Hermes-style self-learning loop" refers to a cycle where:

```
┌─────────────────────────────────────────────────┐
│                                                  │
│  1. Experience → Agent interacts with users      │
│                                                  │
│  2. Capture → memory_store saves knowledge       │
│                                                  │
│  3. Reflect → Lucid CLI consolidates/reviews     │
│               memories externally                │
│                                                  │
│  4. Prune → Hygiene removes stale context        │
│                                                  │
│  5. Recall → memory_recall retrieves relevant    │
│              knowledge for new interactions       │
│                                                  │
│  6. Apply → Agent uses recalled knowledge        │
│             to improve future responses           │
│                                                  │
└─────────────────────────────────────────────────┘
```

**What exists today:**
- Steps 1, 2, 5, 6 are fully operational
- Step 4 (Prune) is operational via hygiene
- Step 3 (Reflect) is partially operational — the Lucid bridge syncs with an external tool, but RustyClaw doesn't drive the reflection cycle itself

**What's missing for a complete Hermes loop:**
- **Active consolidation** — No mechanism to periodically review Core memories, identify contradictions, merge related entries, or increase/decrease confidence scores
- **Importance scoring** — No dynamic relevance weighting beyond the static `min_relevance_score` threshold
- **Cross-session learning** — No mechanism to identify patterns across multiple sessions and promote recurring insights to Core
- **Self-evaluation** — No mechanism for the agent to assess whether recalled memories actually improved its responses

### 3.3 Current Wiring vs. Unified Loop

Today, these modules operate independently:
- Hygiene runs on a 12-hour timer, not triggered by memory quality signals
- Scanning is a passive decorator, not an active quality reviewer
- Lucid syncs bi-directionally but doesn't initiate reflection
- Snapshots export on hygiene cycles, not on knowledge milestones
- Response cache has no connection to memory (different DB, different lifecycle)

A unified Hermes loop would wire these together: experience → capture → (background) reflect → prune → snapshot, with feedback signals between stages.

---

## 4. Fit Assessment

### 4.1 Does DeerFlow Complement, Replace, or Conflict?

| Aspect | Relationship | Detail |
|--------|-------------|--------|
| Memory storage | **RustyClaw far superior** | Multi-backend (SQLite/Postgres/Qdrant) vs. single JSON file |
| Semantic recall | **RustyClaw far superior** | Hybrid vector+FTS5 vs. no search at all |
| Memory hygiene | **RustyClaw superior** | Time-based archival/purge vs. 100-fact confidence cap |
| Self-reflection | **Neither has it fully** | RustyClaw has the Lucid bridge; DeerFlow has nothing |
| LLM-driven extraction | **DeerFlow has a pattern** | Auto-extracts facts from conversations; RustyClaw relies on explicit `memory_store` calls |
| Debounced updates | **DeerFlow has a pattern** | Batches memory updates efficiently |
| Context injection | **Comparable** | Both inject memory into system prompt |
| Response caching | **RustyClaw superior** | SQLite-backed cache vs. nothing |
| Cold boot recovery | **RustyClaw superior** | Snapshot/hydration vs. nothing |
| Security | **RustyClaw superior** | Content scanning vs. nothing |

**Verdict:** DeerFlow does not complement, replace, or conflict with RustyClaw's memory system at a backend level. Its storage and recall are strictly inferior. However, it has two patterns worth studying:

1. **LLM-driven memory extraction** — Automatically identifying facts/preferences/knowledge from conversations without requiring the agent to explicitly call `memory_store`
2. **Structured memory schema** — The user/history/facts taxonomy with confidence scoring provides a richer semantic structure than flat key-value entries

### 4.2 What DeerFlow Gets Right That We Don't

**Automatic memory extraction:**
DeerFlow's `MemoryMiddleware` runs after every agent turn, feeding the conversation to an LLM with a `MEMORY_UPDATE_PROMPT` that asks: "What facts, preferences, and context should be remembered from this conversation?" This produces structured fact extraction without the agent needing to explicitly decide what to store.

RustyClaw's current approach relies on either:
- The agent calling `memory_store` explicitly (requires agent awareness)
- Auto-save of raw user input (produces noisy, unstructured entries)

The extraction approach produces higher-quality memories because the LLM can distill meaning from multi-turn conversations.

**Confidence scoring:**
DeerFlow assigns confidence scores to facts, enabling quality-based filtering. RustyClaw's `min_relevance_score` filters by search relevance (how well a query matches), not by memory quality (how confident we are the memory is correct).

**Temporal layering:**
DeerFlow's `recentMonths` / `earlierContext` / `longTermBackground` hierarchy explicitly models temporal decay. RustyClaw's hygiene achieves a similar effect (archive after 7 days, purge after 30) but at the file/row level rather than the semantic level.

### 4.3 What RustyClaw Gets Right That DeerFlow Doesn't

**Everything else.** The multi-backend architecture, hybrid search, embedding cache, security scanning, snapshot recovery, Lucid bridge, CLI tooling, and the fact that it's built in Rust (performance, type safety) make RustyClaw's memory system production-grade in ways DeerFlow's is not.

DeerFlow's memory is adequate for a personal assistant. RustyClaw's memory is designed for a long-running autonomous agent that accumulates significant knowledge over time.

---

## 5. Recommendations

### 5.1 Adopt: LLM-Driven Memory Extraction

**What:** Add an after-turn memory extraction step that feeds the conversation to an LLM and produces structured memory updates.

**Why:** This closes the gap between "agent explicitly stores" and "agent implicitly learns." Currently, memory quality depends on the agent deciding to call `memory_store`. An extraction step would capture insights that the agent didn't think to save.

**How it fits:**
- New middleware/hook: `MemoryExtractionHook` in `src/hooks/builtin/`
- Triggers after agent response (like DeerFlow's `MemoryMiddleware.after_agent`)
- Uses a focused prompt: "Given this conversation, what facts/preferences/decisions should be remembered?"
- Debounced (configurable, default 30s) to batch multi-turn conversations
- Produces `MemoryEntry` values with category = Core and a confidence score
- Feeds into existing `memory.store()` path (gets security scanning, embedding, FTS5 for free)
- Respects `response_cache` (same conversation shouldn't produce duplicate extractions)

**Cost:** One additional LLM call per conversation (debounced). Use a cheap model (Haiku-class) for extraction.

### 5.2 Adopt: Confidence Scoring for Memory Entries

**What:** Add a `confidence: f64` field to `MemoryEntry` and incorporate it into recall scoring.

**Why:** Not all memories are equal. A fact stated directly by the user ("I prefer TypeScript") should outrank an inference from context. Confidence scoring enables quality-based filtering alongside relevance-based filtering.

**How it fits:**
- Add `confidence` column to SQLite `memories` table (default 1.0 for explicit stores)
- Extraction-produced memories start at LLM-assigned confidence (0.0–1.0)
- Recall scoring becomes: `final_score = vector_weight × vec + keyword_weight × kw + confidence_weight × conf`
- Hygiene can optionally decay confidence over time (if a memory hasn't been reinforced by recall or re-extraction)

### 5.3 Adopt: Loop Detection Pattern

**What:** Port DeerFlow's `LoopDetectionMiddleware` concept — hash-based detection of repetitive tool calls with sliding window and hard stop.

**Why:** Runaway agents making the same tool calls repeatedly is a real failure mode. This is a cheap safety mechanism.

**How it fits:** New hook in `src/hooks/builtin/` that monitors tool call history, hashes the (tool_name, arguments) tuple, and breaks loops after configurable thresholds.

### 5.4 Consider: Temporal Memory Layering

**What:** Add explicit temporal tiers to the memory schema — "recent" (this week), "established" (this month), "long-term" (older).

**Why:** DeerFlow's `recentMonths` / `earlierContext` / `longTermBackground` structure makes temporal relevance explicit. Currently, RustyClaw's hygiene achieves temporal pruning but doesn't tier the remaining memories.

**How it fits:** Could be implemented as MemoryCategory variants or as metadata on existing entries. The recall algorithm would boost recent entries. This is lower priority than extraction and confidence scoring — only worth doing if we see recall quality degrade as memory volume grows.

### 5.5 Skip: DeerFlow's Storage Architecture

DeerFlow's single JSON file with entire-memory-load-on-every-read is not worth adopting. RustyClaw's database-backed approach is strictly superior for any non-trivial memory volume.

### 5.6 Skip: DeerFlow's Middleware Chain Pattern

DeerFlow's 12-middleware chain is a LangChain pattern. RustyClaw already has the hook system (`HookHandler` trait) which serves the same purpose in a Rust-native way. No need to adopt.

### 5.7 Complete: The Hermes Self-Learning Loop

The highest-value improvement is wiring the existing modules into a unified self-learning cycle:

```
1. Experience → Agent interacts (existing)
2. Capture → memory_store (existing) + LLM extraction (§5.1, new)
3. Reflect → Periodic consolidation hook (new):
   - Load all Core memories
   - Ask LLM: "Are any contradictory? Can any be merged? Which are outdated?"
   - Update/remove memories accordingly
   - Adjust confidence scores based on reinforcement
4. Prune → Hygiene (existing) + confidence decay (§5.2, new)
5. Snapshot → Export (existing), now captures confidence + temporal tier
6. Recall → Hybrid search (existing) + confidence weighting (§5.2, new)
7. Apply → Agent uses memories (existing)
8. Evaluate → Track whether recalled memories were useful (new):
   - If agent ignores a recalled memory, decay its confidence
   - If agent explicitly references it, boost its confidence
```

Steps 1, 4 (partial), 5, 6, 7 exist today. Steps 2 (extraction), 3 (consolidation), 4 (confidence decay), and 8 (evaluation) are new.

**Priority order:**
1. LLM-driven extraction (§5.1) — biggest quality improvement, relatively easy
2. Confidence scoring (§5.2) — enables the rest of the loop
3. Consolidation hook (§5.7 step 3) — periodic memory review
4. Usage-based feedback (§5.7 step 8) — closes the learning loop
5. Temporal layering (§5.4) — nice-to-have refinement
6. Loop detection (§5.3) — safety, not memory-related but worth grabbing

---

## Appendix A: Complete Memory Configuration Reference

```toml
[memory]
backend = "sqlite"                        # sqlite|lucid|postgres|qdrant|markdown|none
auto_save = true                          # Auto-save user input to memory

# Hygiene
hygiene_enabled = true
archive_after_days = 7
purge_after_days = 30
conversation_retention_days = 30

# Embeddings
embedding_provider = "none"               # none|openai|openrouter|custom:URL
embedding_model = "text-embedding-3-small"
embedding_dimensions = 1536
vector_weight = 0.7
keyword_weight = 0.3
min_relevance_score = 0.4
embedding_cache_size = 10000
chunk_max_tokens = 512

# Response Cache
response_cache_enabled = false
response_cache_ttl_minutes = 60
response_cache_max_entries = 5000

# Snapshot
snapshot_enabled = false
snapshot_on_hygiene = false
auto_hydrate = true

# SQLite-specific
sqlite_open_timeout_secs = 5

# Qdrant-specific
[memory.qdrant]
url = "http://localhost:6333"
collection = "rustyclaw_memories"
api_key = ""
```

## Appendix B: DeerFlow Memory Schema

```json
{
  "version": "1.0",
  "lastUpdated": "ISO-8601",
  "user": {
    "workContext": {"summary": "string", "updatedAt": "ISO-8601"},
    "personalContext": {"summary": "string", "updatedAt": "ISO-8601"},
    "topOfMind": {"summary": "string", "updatedAt": "ISO-8601"}
  },
  "history": {
    "recentMonths": {"summary": "string", "updatedAt": "ISO-8601"},
    "earlierContext": {"summary": "string", "updatedAt": "ISO-8601"},
    "longTermBackground": {"summary": "string", "updatedAt": "ISO-8601"}
  },
  "facts": [
    {
      "id": "fact_UUID",
      "content": "string",
      "category": "preference|knowledge|context|behavior|goal",
      "confidence": 0.0-1.0,
      "createdAt": "ISO-8601",
      "source": "thread_id"
    }
  ]
}
```

## Appendix C: Memory Backend Selection Guide

| Use Case | Recommended Backend | Why |
|----------|-------------------|-----|
| Default / general purpose | `sqlite` | Hybrid search, zero external deps, production-tested |
| Multi-machine / shared | `postgres` | Centralized, keyword search, scales horizontally |
| Semantic-heavy recall | `qdrant` + `sqlite` | Vector search for semantics, SQLite for structured |
| Human-auditable | `markdown` | Git-friendly, readable, but no vector search |
| External reflection tool | `lucid` | Bridges to lucid-memory CLI, SQLite fallback |
| Disabled | `none` | Explicit opt-out, no storage overhead |
