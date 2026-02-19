# MAPLE Memory Design — Second-Pass Review

**Document under review:** `docs/MAPLE_MEMORY_DESIGN.md`  
**First review validated:** `docs/MAPLE_DESIGN_REVIEW.md`  
**Date:** 2026-02-19  
**Verdict:** **No-go as written.** Phase 1 should not start until design corrections below are made.

## 1) Validation of First Review C1/C2/C3

### C1 (`Mutex<Connection>` deadlock) — **Partially agree, diagnosis overstated**

**What is correct:**
- Concurrency design is currently underspecified and risky for Learning Worker writes.
- `SqliteMemory` is synchronous `rusqlite` behind `std::sync::Mutex` in async methods (`src/memory/sqlite.rs:20`, `src/memory/sqlite.rs:358`, `src/memory/sqlite.rs:392`). This can block Tokio worker threads under contention.

**What is incorrect/overstated:**
- The first review says this “will deadlock under concurrent writes.” Current code does not hold the mutex across `.await` in `store()`/`recall()`; lock sections are synchronous (`src/memory/sqlite.rs:364-373`, `src/memory/sqlite.rs:397-403`). Thread blocking risk is real; guaranteed deadlock is not proven.
- A `clone_for_background_writer()` API is not strictly required. A shared `Arc<dyn Memory>` can be passed to worker tasks today; separate connection strategy is an architecture choice, not mandatory API shape.

**Disposition:** keep as **Critical**, but rewrite as **runtime blocking + multi-writer policy gap**, not guaranteed deadlock.

### C2 (`supersedes` semantics incomplete) — **Agree, and this is worse than stated**

- `Memory` write API cannot persist proposed metadata at all; `store()` only accepts `(key, content, category)` (`src/memory/traits.rs:48`). Design claims `supersedes`, `source`, `confidence`, etc. are written via `memory.store()` (`docs/MAPLE_MEMORY_DESIGN.md:381-385`), which is impossible without trait/API change.
- SQLite currently upserts by key (`ON CONFLICT(key) DO UPDATE`) (`src/memory/sqlite.rs:379-385`), which directly conflicts with append-only supersession unless key policy is redesigned.
- Markdown backend does not support per-entry metadata and does not preserve stable keys from `store()` (`src/memory/markdown.rs:66-97`, `src/memory/markdown.rs:141-152`).

**Disposition:** keep as **Critical**.

### C3 (prompt injection via learned facts) — **Agree**

- Existing memory injection already inserts raw recalled text into prompt context (`src/agent/loop_.rs:127-136`).
- Design adds higher-authority system prompt injection via `UserModelSection` (`docs/MAPLE_MEMORY_DESIGN.md:524-555`) plus learned memory blocks (`docs/MAPLE_MEMORY_DESIGN.md:561-571`) without hard trust boundaries.

**Disposition:** keep as **Critical**.

## 2) Critical findings both documents missed

### N1 — “Append-only tamper-proof” is not true under current contracts

- `store()` mutates existing rows in SQLite (`src/memory/sqlite.rs:381-385`).
- `forget()` hard-deletes rows in SQLite (`src/memory/sqlite.rs:581-587`).
- Design claims tamper-proof preservation (`docs/MAPLE_MEMORY_DESIGN.md:184`, `docs/MAPLE_MEMORY_DESIGN.md:703-720`) without addressing existing mutability.

### N2 — Trait evolution is underspecified and currently insufficient

- Adding fields to `MemoryEntry` alone is not enough. There is no write path for metadata or provenance in trait methods (`src/memory/traits.rs:43-68`).
- `get()/list()/recall()` currently map only legacy columns and set `session_id`/`score` manually (`src/memory/sqlite.rs:443-553`).

### N3 — Markdown backend has correctness issues beyond frontmatter

- Non-`Core` categories collapse into daily file path (`src/memory/markdown.rs:148-151`), so new categories are not preserved.
- Stored key is not recoverable as stable key; parser generates `filename:index` keys (`src/memory/markdown.rs:87-89`).
- Write path is read-modify-write without lock/append syscall (`src/memory/markdown.rs:41-63`), so concurrent stores can lose entries.

### N4 — Design still references JSONL backend assumptions that do not match runtime backend registry

- Agent default is `jsonl` (`src/agent/definition.rs:31-34`, `src/agent/definition.rs:64-66`), but backend classifier does not recognize it (`src/memory/backend.rs:80-87`) and factory falls back to markdown (`src/memory/mod.rs:53-58`).
- `docs/MAPLE_MEMORY_DESIGN.md` repeatedly treats JSONL as a first-class backend (`docs/MAPLE_MEMORY_DESIGN.md:240`, `docs/MAPLE_MEMORY_DESIGN.md:641`).

### N5 — Learning idempotency/atomicity model is missing

- Design mentions watermarking (`docs/MAPLE_MEMORY_DESIGN.md:299`) but does not define transactional behavior for “extract -> store many entries -> mark processed.” Crash mid-run can duplicate or partially apply outputs.

## 3) Tamper-proofing assessment (append-only + supersedes)

**Current proposal is not sound as tamper-proofing.**

Why:
- Append-only is a convention, not enforced by storage contracts.
- Supersedes pointers can be added, omitted, or forged without integrity checks.
- Mutable side channels remain (`forget`, upsert, editable markdown files, mutable `USER_MODEL.yaml`).

What would make it sound:
1. Define an immutable event log model (`memory_events`) with one-way inserts only.
2. Separate read model/projection for fast recall (mutable/indexed), derived from event log.
3. Represent correction/supersession as new events referencing immutable event IDs, not mutable keys.
4. Add optional integrity chain (`prev_hash`, event hash/signature) if “tamper-proof” is a hard claim.
5. Reclassify `USER_MODEL.yaml` as a mutable cache/projection, not authoritative memory.

## 4) Rust implementation assessment

### Ownership model
- `Arc<dyn Memory>` is compatible with daemon/agent architecture and can support a background worker.
- Provider ownership for learning (`Box<dyn Provider>`) is fine if lifecycle is daemon-scoped.

### Async/runtime model
- Synchronous DB work on async paths is acceptable at low load but risky once Learning adds periodic write bursts.
- Before Learning phase, design must pick one model:
1. Dedicated DB actor task + channel (single writer), or
2. `spawn_blocking` for SQLite operations + busy timeout/backoff policy, or
3. Async DB stack/pool migration.

### Trait evolution
- Required. Minimal safe path:
1. Add `store_entry(&MemoryWrite)` API with metadata/provenance fields.
2. Add retrieval API with category/source filters and supersession behavior (`include_superseded` or `latest_only`).
3. Keep legacy `store()` as compatibility shim that populates minimal `MemoryWrite`.

## 5) Required changes to `MAPLE_MEMORY_DESIGN.md` before implementation

1. Replace tamper-proof claim with explicit threat model and current exceptions.
2. Add a concrete memory data model section that separates immutable events from retrieval projections.
3. Add trait/API evolution section (not just struct-field additions) with migration path for all memory backends.
4. Define supersession semantics using immutable IDs, not key-prefix heuristics.
5. Declare backend support matrix by phase:
- Phase 1-3 full support: SQLite only.
- Markdown: degraded/manual mode or explicitly unsupported for Learning.
- JSONL: either remove from doc or add real runtime alias/implementation.
6. Define SQLite concurrency policy (single writer vs multi-connection) and failure handling (`SQLITE_BUSY`, retries, backoff).
7. Define Learning transaction/idempotency semantics (batch unit, watermark persistence, replay safety).
8. Add explicit session boundary strategy (event markers), not just inactivity heuristics.
9. Add hard prompt-safety controls for learned content (classification, sanitization, trust levels, injection boundaries).
10. Update isolation model in closed-loop diagram and text to match `Isolated`/`SharedRead`/`Shared` behavior.
11. Rework timeline estimates into capability gates with exit criteria and rollback notes.
12. Update CLI/tool impacts (`memory_store`, `memory_recall`, `memory_forget`) to reflect new categories and append-only policy.

## Final call

- **C1/C2/C3 remain Critical** (C1 wording needs correction).
- The design is directionally good, but key contracts are missing or contradictory to current code.
- Revise `MAPLE_MEMORY_DESIGN.md` with the required changes above, then proceed.
