# Code Review — Phase 5b: AgentCoordinator (TEZ-70–74)

**Reviewed:** 2026-02-26  
**Commit:** 596f49d  
**Reviewer:** aira-bot (staff-level code review)  
**Scope:** `src/agent/capability_registry.rs`, `src/agent/coordinator.rs`, `src/agent/definition.rs` (capabilities field), `src/agent/mod.rs`, `src/daemon/mod.rs`

---

## Summary

Quality is consistent with the existing codebase. No new clippy warnings introduced. Test coverage is good (10 tests for `CapabilityRegistry`, 8 for `AgentCoordinator`, 3 new tests for `definition.rs`). A few minor idiom and ergonomics improvements are noted.

---

## Findings

### LOW: `sequential_delegate` fallthrough test is weak

**File:** `src/agent/coordinator.rs:tests::sequential_falls_through_to_second_agent_on_failure`  
**Severity:** LOW  
**Type:** Test quality

The test comment says "acceptable if agent-bad was tried last" — the test passes even if the coordinator never reached `agent-good`. This is because `HashMap` iteration order is non-deterministic, so `agent-bad` might not be tried first. The test should force ordering to verify fallthrough behavior.

**Recommendation:**
```rust
// Force deterministic ordering by using a single-capability registry 
// where we control which agent is listed first.
// Or: use separate coordinators with a Vec of known order.
```
Consider refactoring the test to use a custom ordering or add a separate test that explicitly constructs the candidates list.

---

### LOW: Missing `tracing::instrument` on public async methods

**File:** `src/agent/coordinator.rs`, `src/agent/capability_registry.rs`  
**Severity:** LOW  
**Type:** Observability

Other async methods in the codebase (e.g., in `bus.rs`) use structured logging but not `#[tracing::instrument]`. The new coordinator and registry don't have structured spans either — consistent with existing style, but `delegate_to_capable` is a high-value observability point.

**Recommendation:** Add `tracing::debug!("delegating to capable agent", capability = %capability, candidates = ?candidates)` at the start of `delegate_to_capable`. Low priority but useful for debugging in production.

---

### LOW: `spawn_failing_agent` test helper has a logic bug

**File:** `src/agent/coordinator.rs:tests::spawn_failing_agent`  
**Severity:** LOW  
**Type:** Test correctness

```rust
drop(name.clone()); // suppress unused warning
```

This drops a clone of `name`, not `name` itself. The `name` variable is actually used only in the closure capture, so this is fine, but `drop(name.clone())` is misleading. The `#[allow(unused)]` attribute or a `let _ = name;` would be clearer.

---

### INFO: `CapabilityRegistry` inner `Arc` is redundant with `self`

**File:** `src/agent/capability_registry.rs`  
**Severity:** INFO  
**Type:** Unnecessary indirection

```rust
pub struct CapabilityRegistry {
    inner: Arc<RwLock<HashMap<String, HashSet<String>>>>,
}
```

The outer `Arc` in `inner` is unnecessary — `CapabilityRegistry` is itself wrapped in `Arc` at the call sites. The double-Arc means cloning `CapabilityRegistry` (if ever done) would share the inner lock, which is correct but the outer `Arc` could be removed to simplify:

```rust
pub struct CapabilityRegistry {
    inner: RwLock<HashMap<String, HashSet<String>>>,
}
```

This matches how `AgentBus` structures its inner state (uses `Arc<RwLock<...>>` because `AgentBus` itself isn't always wrapped in Arc at all call sites, but for `CapabilityRegistry` it is).

**Note:** This is a style point. Both implementations are correct and the double-Arc has zero runtime overhead beyond a single extra reference count allocation.

---

### INFO: `agents_for_capability` allocates a `Vec<String>` on every call

**File:** `src/agent/capability_registry.rs:43`  
**Severity:** INFO  
**Type:** Minor allocation

On each call, agent names are cloned into a new `Vec<String>`. This is correct and idiomatic, but for hot paths (e.g., called on every tool invocation), consider returning `impl Iterator` or `Vec<&str>` if the lock guard can be held. For the current usage (infrequent routing decisions), this is fine.

---

## Positive Findings

- ✅ All new code is `cargo fmt` clean
- ✅ No new clippy warnings in new files
- ✅ `Default` impls are correct and follow existing patterns
- ✅ Error messages are descriptive (include capability name and candidate list)
- ✅ Fanout channel drop pattern correctly prevents channel leaks
- ✅ `retry_count: u32` overflow is impossible (loop condition is `attempts > retry_count`, both u32)
- ✅ Tests use `#[tokio::test]` correctly — no sync-in-async blocking
- ✅ `spawn_mock_agent` helper is reusable and well-structured
- ✅ Doc comments explain the "why" not just the "what"
- ✅ Commit message follows conventional commits format

---

## Verdict

**No P0/CRITICAL or P1/HIGH findings.** Three LOW findings are minor test/style improvements. Code quality matches the existing codebase bar. Ready to merge.
