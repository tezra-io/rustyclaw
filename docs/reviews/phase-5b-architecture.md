# Architecture Review — Phase 5b: AgentCoordinator (TEZ-70–74)

**Reviewed:** 2026-02-26  
**Commit:** 596f49d  
**Reviewer:** aira-bot (architecture agent)  
**Scope:** `src/agent/capability_registry.rs`, `src/agent/coordinator.rs`, `src/agent/definition.rs` (capabilities field), `src/agent/mod.rs`, `src/daemon/mod.rs`

---

## Summary

The AgentCoordinator adds a capability-based routing layer above the existing `AgentBus`. The implementation is purely additive, correctly follows zeroclaw's layered architecture (config → definition → bus → coordinator), and uses existing async patterns throughout.

---

## Findings

### MEDIUM: `_coordinator` is unused in daemon — coordination not accessible to DelegateTool

**File:** `src/daemon/mod.rs`  
**Severity:** MEDIUM  
**Type:** Architectural gap

The daemon creates `_coordinator` but never surfaces it for use. The `DelegateTool` still routes by hardcoded agent name only. The coordinator exists but nothing actually uses it for routing.

**Impact:** The capability routing feature is implemented but not exercised in the live system. Any caller wanting capability-based routing must construct their own coordinator.

**Recommendation:** In a follow-up (TEZ-74 follow-up or new ticket), pass `Arc<AgentCoordinator>` into `run_persistent_agent` and inject it as a tool (e.g., `delegate_by_capability` tool), or wire it into `DelegateTool` when no explicit agent name is given.

---

### LOW: `CapabilityRegistry` is not synced with `AgentBus` lifecycle

**File:** `src/daemon/mod.rs`, `src/agent/runner.rs`  
**Severity:** LOW  
**Type:** Lifecycle gap

When a persistent agent crashes and is restarted by the supervisor loop, `AgentBus` re-registers the agent (via `bus.register()` in `run_persistent_agent`), but `CapabilityRegistry` is not updated. The registry entry from startup persists correctly in normal operation, but if capabilities change (e.g., agent definition file edited + hot-reload), they would drift.

**Recommendation:** Pass `Arc<CapabilityRegistry>` into `run_persistent_agent` and call `registry.register(name, capabilities)` alongside `bus.register()`, and `registry.unregister(name)` at the end. This keeps the two registries in sync across restarts.

---

### LOW: `agents_for_capability` ordering is non-deterministic

**File:** `src/agent/capability_registry.rs:43`  
**Severity:** LOW  
**Type:** Determinism

`agents_for_capability` iterates `HashMap` entries, which are unordered. The sequential coordinator will try agents in arbitrary order on each call. This is acceptable for now but may surprise users who expect consistent routing.

**Recommendation:** Consider sorting by agent name (or priority field if added later) before returning candidates. Document the non-deterministic behavior in the function's doc comment.

---

### INFO: `DelegationPolicy` is not config-driven

**File:** `src/agent/coordinator.rs`  
**Severity:** INFO  
**Type:** Extensibility

`DelegationPolicy::default()` is hardcoded (120s timeout, 0 retries, no fanout). There's no config schema entry for coordinator settings. This is acceptable for the initial implementation but limits operator customization.

**Recommendation:** Track as follow-up. Add `CoordinatorConfig` to `src/config/schema.rs` with serde defaults matching the current defaults.

---

### INFO: No capability validation in `AgentDefinition::validate()`

**File:** `src/agent/definition.rs`  
**Severity:** INFO  
**Type:** Validation gap

The `validate()` method doesn't check capability strings for format. Free-form strings are flexible but could lead to typos (e.g., `"webSearch"` vs `"web_search"`).

**Recommendation:** Either document the naming convention (snake_case, no spaces) or add a validation rule. A warning (not error) for non-snake-case capability names would be appropriate.

---

## Positive Findings

- ✅ Module boundaries are clean — `CapabilityRegistry` has no knowledge of `AgentBus`, coordinator owns both
- ✅ Additive pattern is correct — existing `AgentBus` API unchanged
- ✅ `Arc<RwLock<...>>` is the right concurrency primitive for this access pattern
- ✅ `Default` impls follow zeroclaw conventions
- ✅ Module exports in `mod.rs` follow existing `#[allow(unused_imports)]` pattern

---

## Verdict

**No P0/CRITICAL blockers.** Two LOW findings recommended as follow-up tickets. Architecture is sound.
