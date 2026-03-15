# Claude Code Review: Elixir Delegation Design
*Reviewer: Claude Code | Date: 2026-03-14*

---

## STRENGTHS

**S1. Clear problem decomposition.** The design correctly identifies the core tension: Rust DelegateTool runs sub-agents inline as blocking function calls with no supervision, while Elixir has the orchestration primitives (GenServer, DynamicSupervisor, Registry) that should own lifecycle management. The "what moves vs. what stays" tables are precise.

**S2. Defense-in-depth tool isolation.** The design enforces tool allowlists at two layers: Elixir strips `delegate` from allowed tools before calling Rust, and Rust's SecurityPolicy applies on top. This prevents re-entrant delegation bypassing Elixir and preserves existing security guarantees.

**S3. Feature-flagged migration path.** The `delegation_mode: "rust" | "elixir"` flag in Phase 2 is the right call. It allows parallel testing and safe rollback. The current Rust DelegateTool is well-tested (22 tests covering depth limits, tool filtering, provider failures, security policy), so having a fallback is critical.

**S4. Provenance propagation design.** Carrying `MessageProvenance` through both HTTP hops gives end-to-end tracing. The existing `TraceStore` + `MessageProvenance` modules in Elixir are already wired into `AgentServer` and `AgentCoordinator`, so this isn't speculative — it's ready.

**S5. Correct separation of concerns.** Keeping `run_tool_call_loop` in Rust is the right call. That function has 16 parameters and deep integration with providers, tools, multimodal handling, approval, hooks, credential scrubbing, and history compaction. Rewriting it in Elixir would be a multi-month effort for no architectural gain.

---

## WEAKNESSES

**W1. AgentDefinition is missing critical fields for execution.** The design's biggest data-model gap. AgentDefinition has no `provider`, `api_key`, or `agentic` field. Without `provider`, Elixir cannot construct the `/api/agent/run` request. Phase 1 is blocked until AgentDefinition is extended.

**W2. No HTTP server exists in the Elixir app.** The design proposes `POST /api/delegate` but the Elixir app starts no web server. No Bandit, no Plug.Router, no Cowboy. This is "add an HTTP framework from scratch," not "wire up existing code."

**W3. The `/api/agent/run` endpoint doesn't exist in Rust either.** The gateway has no such route. Adding it requires a new Axum handler, integration with the 16-parameter `run_tool_call_loop`, tool registry filtering, and provider creation from JSON. Significant Rust work not acknowledged in the design.

**W4. `run_tool_call_loop` is not designed for external invocation.** It takes in-process references (trait objects, mutable vectors, Arc pointers) that can't be serialized over HTTP. The handler must replicate what DelegateTool does internally.

**W5. No idle timeout implementation.** Described in design but AgentServer has no timeout logic. Doesn't address what happens if agent exits while RustBridge call is in-flight.

---

## RISKS

**R1. API key transmission over HTTP (CRITICAL).** The design sends `api_key` in plaintext JSON over localhost HTTP on every delegation. Security regression from current in-process credential handling.

**R2. Bidirectional HTTP dependency creates fragile coupling.** Either process restart kills all in-flight delegations. No circuit breaker on either side.

**R3. Fanout strategy multiplies failure probability.** N agents × 2 hops = 2N HTTP requests. No cancellation propagation from Elixir to Rust for killed tasks.

**R4. Agent discovery requires agents to already be spawned.** `find_matching_agents` queries running processes, not definition files. Breaks lazy spawning.

**R5. Config format split will cause confusion.** TOML vs YAML with different fields, no reconciliation strategy.

---

## RECOMMENDATIONS

1. **Don't send API keys over the wire.** Rust resolves credentials by agent name from its own config.
2. **Add Phase 0:** Scaffold Elixir HTTP server + extend AgentDefinition with missing fields.
3. **Consider one-hop alternative:** Elixir returns routing decision to Rust, Rust executes. Simpler, more secure, 80% of the value.
4. **Fix agent discovery** to read definition files, not just running processes.
5. **Add cancellation propagation** for fanout/timeout.
6. **Extract `SubAgentRunner` struct** from the 16-parameter function.

---

## ELIXIR CODE READINESS

| Module | Status | Ready? |
|--------|--------|--------|
| AgentServer | Scaffolding (returns `:pending_bridge`) | No |
| AgentCoordinator | Functional for routing | Partially |
| AgentSupervisor | Functional | Yes |
| AgentDefinition | Missing fields | No |
| RustBridge | Targets nonexistent endpoint | No |
| SubAgentSession | Functional | Yes |
| MessageProvenance | Functional | Yes |
| ResourceLock | Functional | Yes |
| DelegationRouter | Does not exist | No |

**~40% of Elixir code is ready.**

---

## VERDICT: REVISE

Architecture is sound in principle but overstates readiness and underestimates implementation gap. Fix credential exposure, add Phase 0, consider one-hop alternative, fix agent discovery, add cancellation.
