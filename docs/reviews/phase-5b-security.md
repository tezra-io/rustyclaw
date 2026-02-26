# Security Review — Phase 5b: AgentCoordinator (TEZ-70–74)

**Reviewed:** 2026-02-26  
**Commit:** 596f49d  
**Reviewer:** aira-bot (security agent)  
**Scope:** `src/agent/capability_registry.rs`, `src/agent/coordinator.rs`, `src/agent/definition.rs` (capabilities field), `src/agent/mod.rs`, `src/daemon/mod.rs`

---

## Summary

The AgentCoordinator introduces a new routing path. Security analysis focuses on: capability string injection, routing bypass, DoS via capability flooding, and interaction with the existing delegation ACL.

---

## Findings

### MEDIUM: Capability strings are user-controlled and unvalidated

**File:** `src/agent/definition.rs`, `src/agent/capability_registry.rs`  
**Severity:** MEDIUM  
**Type:** Input validation

Capability strings come from agent definition YAML files (`capabilities: [...]`). While these files are written by the operator (not end users), a compromised or maliciously crafted definition file could register a capability string designed to shadow or impersonate another agent's capability.

Example: An agent definition claiming `capabilities: ["delegate_all", "admin", "shell_unrestricted"]` would appear in capability lookups, potentially causing the coordinator to route tasks to the wrong agent if caller trusts capability names as identity.

**Current mitigations:** The coordinator only routes to agents already registered on `AgentBus`, which requires the agent to have started successfully via the daemon's supervised loop. This prevents a file-only attack from routing to a non-running agent.

**Risk:** LOW in the current system (coordinator is unused for live routing), but MEDIUM if coordinator is wired into DelegateTool.

**Recommendation:**
- Validate capability strings in `AgentDefinition::validate()`: allow only `[a-z0-9_]` characters
- Log a warning when `agents_for_capability` returns multiple candidates (potential confusion)
- Add ACL checks to `delegate_to_capable` analogous to `AgentBus::set_delegation_acl`

---

### LOW: Fanout spawns unbounded tokio tasks per delegation call

**File:** `src/agent/coordinator.rs:fanout_delegate()`  
**Severity:** LOW  
**Type:** DoS / resource exhaustion

`fanout_delegate` spawns one `tokio::spawn` per online candidate agent. If many agents claim the same capability and `fanout=true`, this spawns many concurrent tasks. In the current implementation this is bounded by the number of registered agents, and with sensible configurations will be small (≤ 10). However, if an adversary can register many agents (requires daemon config write access), this could amplify resource usage.

**Mitigation:** The current implementation is acceptable given that agent registration requires config file write access (already a security boundary). No immediate action needed, but document the behavior.

**Recommendation:** Cap fanout concurrency (e.g., max 5 concurrent fanout delegates) when wiring into production use.

---

### LOW: No rate limiting on `delegate_to_capable`

**File:** `src/agent/coordinator.rs`  
**Severity:** LOW  
**Type:** DoS

The coordinator's `delegate_to_capable` method has no rate limiting. If exposed via a tool, a runaway agent could flood the bus with delegation requests. The bus itself also has no rate limiting.

**Current mitigations:** Agent autonomy limits (`max_actions_per_hour`) in `SecurityPolicy` would constrain tool-call frequency. This is only a concern when coordinator is wired into a tool.

**Recommendation:** Track as follow-up when coordinator is exposed as a tool.

---

### INFO: `CapabilityRegistry` separate from `AgentBus` ACL creates two permission systems

**File:** `src/agent/bus.rs`, `src/agent/coordinator.rs`  
**Severity:** INFO  
**Type:** Defense in depth gap

`AgentBus` enforces delegation ACL (who can delegate to whom). `CapabilityRegistry` has no ACL — any caller of `delegate_to_capable` can route to any agent advertising the capability, regardless of the calling agent's `delegates_to` configuration.

**Consequence:** If coordinator bypasses bus ACL — it does NOT currently, since it calls `bus.delegate()` which enforces ACL — there would be a bypass. Since coordinator goes through `bus.delegate()`, the ACL is still enforced.

**Verdict:** No bypass currently. The ACL chain is: `coordinator.delegate_to_capable()` → `bus.delegate()` → ACL check → agent. ✅

---

## Positive Findings

- ✅ Coordinator goes through `AgentBus.delegate()` — existing ACL enforcement preserved
- ✅ `is_registered` check prevents routing to non-running agents (definition file ≠ running agent)
- ✅ No network I/O, no file system access in new code — attack surface is minimal
- ✅ `CapabilityRegistry` uses `Arc<RwLock>` correctly — no data races
- ✅ Fanout channel drop pattern is correct — no channel leaks

---

## Verdict

**No P0/CRITICAL or P1/HIGH findings.** One MEDIUM finding (capability string validation) recommended as follow-up before coordinator is wired into live tool routing. Two LOW findings are acceptable for current state (coordinator not yet live).
