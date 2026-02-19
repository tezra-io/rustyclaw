# LP Review Summary

## What I Fixed in `docs/LEARNING_AND_PERSONALIZATION.md`

- Corrected architecture assumptions to match real code paths:
  - included both personalization injection points: `src/agent/loop_.rs` and `src/agent/memory_loader.rs`
  - removed assumptions that daemon already has shared memory/provider instances
- Corrected memory-model assumptions:
  - kept category plan on `MemoryCategory::Custom(...)` (no enum expansion required)
  - updated relevance logic to reflect that `recall()` has scores and `list()` does not
  - documented that SQLite `MemoryEntry.timestamp` is `created_at` (not `updated_at`)
- Corrected learning-data assumptions:
  - highlighted current autosave shape (user conversation entries vs truncated assistant daily summaries)
  - called out `Agent::turn` fixed-key overwrite as a blocking prerequisite for persistent-agent learning
- Reworked extraction prompt for structured output quality:
  - strict JSON schema
  - explicit limits and confidence normalization
  - post-parse validation steps
- Closed the feedback-loop gap with a concrete persisted counter design (`Custom("personalization_feedback")`) instead of adding fields to `MemoryEntry`.
- Replaced brittle line-number references with stable function/file integration points.

## What Looks Good in the Original Direction

- Good separation of concerns: Learning (background) vs Personalization (request path).
- Correct instinct to keep extraction async and avoid per-request LLM calls for routing/style.
- Strong focus on deterministic, auditable behavior and bounded budgets.

## What Still Needs Attention (Implementation Risk)

- Persistent-agent autosave key overwrite in `src/agent/agent.rs` must be fixed before daemon learning can be reliable on SQLite.
- `memory_store` category parser in `src/tools/memory_store.rs` still only supports `core|daily|conversation`; needs extension for LP categories.
- Daemon learning scope should be explicit in initial rollout (workspace-level first, then persistent-agent support after autosave fix).
