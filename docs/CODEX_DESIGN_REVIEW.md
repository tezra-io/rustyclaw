# Design Review: Elixir Delegation Migration

## Verdict
**REQUEST_CHANGES**. The proposal has good intent (separate orchestration from execution), but the current design and code shape introduce bottlenecks and failure risk that will hurt reliability fast.

## 1) Two-hop HTTP (Rust -> Elixir -> Rust): right call or overengineered?
Short answer: **acceptable as a migration bridge, overengineed as the target architecture**.

- It is a valid transitional boundary while ownership is split.
- It is not cheap operationally once you add retries/timeouts/correlation across two RPC hops.
- The “~5–15ms overhead” claim in the design is too optimistic as a system-level estimate once retries, queueing, and overloaded GenServers are involved ([ELIXIR_DELEGATION_DESIGN.md:463](ELIXIR_DELEGATION_DESIGN.md:463)).

Bigger issue: current Elixir implementation shape already serializes throughput, so fanout gains are mostly illusion.

- Bridge calls are synchronous in a single GenServer and include `Process.sleep` backoff in the callback ([rust_bridge.ex:73](rust_bridge.ex:73), [rust_bridge.ex:117](rust_bridge.ex:117), [rust_bridge.ex:129](rust_bridge.ex:129)).
- Coordinator strategy execution also runs inside a GenServer call path ([agent_coordinator.ex:63](agent_coordinator.ex:63), [agent_coordinator.ex:146](agent_coordinator.ex:146)).

So yes: **this is currently overengineered for the value delivered** unless you first fix concurrency architecture.

## 2) Missing failure modes
Major gaps:

1. **Duplicate execution on retry (idempotency missing)**
- Retry logic exists ([rust_bridge.ex:117](rust_bridge.ex:117)) but there is no request id/idempotency key.
- If Rust executed but response was lost, Elixir retries and can re-run side-effecting tools.

2. **Timeout mismatch and partial failure ambiguity**
- Coordinator/AgentServer call timeout is 30s ([agent_coordinator.ex:20](agent_coordinator.ex:20), [agent_server.ex:17](agent_server.ex:17)); RustBridge call timeout is 60s ([rust_bridge.ex:18](rust_bridge.ex:18)).
- This creates “caller timed out, worker still running” behavior with no cancellation semantics.

3. **No cancellation propagation**
- If parent aborts, there is no end-to-end cancel token from Rust->Elixir->Rust task.

4. **Backpressure/concurrency control not defined where it matters**
- Fanout exists, but no global/per-agent queue limits, and bridge is single-process bottleneck.

5. **Secret handling risk is underspecified**
- Design sends `api_key` in request payload ([ELIXIR_DELEGATION_DESIGN.md:290](ELIXIR_DELEGATION_DESIGN.md:290)).
- No concrete plan for secret redaction in logs, memory lifecycle, or transport hardening.

6. **Health model does not represent real health**
- Health state is based on `status/recovery_attempts`, not bridge/provider outcomes ([agent_server.ex:323](agent_server.ex:323)).

7. **Capability routing can miss valid agents**
- Routing checks currently-running agents, not full definitions ([agent_coordinator.ex:104](agent_coordinator.ex:104)).
- This conflicts with “lazy spawn by capability.”

8. **Fallback behavior is hand-wavy**
- Design says fallback to Rust mode if Elixir fails ([ELIXIR_DELEGATION_DESIGN.md:473](ELIXIR_DELEGATION_DESIGN.md:473)), but no concrete failure detector/switchover policy is defined.

## 3) Is migration realistic for a 2-person team?
**Not as written.** Phase scope is too broad.

- Phase 2 already requires: new routing path, bridge hardening, real execution wiring, parity testing across two modes.
- Phase 3 adds dual config formats + compatibility plumbing.
- Phase 5 is another project (fanout aggregation policy, persistent snapshots, async inter-agent messaging, dynamic agent creation, lock manager).

For two people, realistic near-term scope is:

- Phase A: `first_available` only, no fanout/sequential.
- Phase B: one config source of truth (pick YAML or TOML, not both long-term).
- Phase C: idempotency, cancellation, timeout alignment, and observability parity.
- Defer persistent snapshots/resource locking/async agent messaging until after stable production usage.

## 4) What I would do differently
1. **Keep Elixir as control plane first, not full execution loop orchestrator**
- Let Elixir choose target + policy.
- Return an execution plan to Rust.
- Rust performs the sub-agent run directly in first cut.
- This removes one RPC hop immediately and keeps existing stable loop.

2. **If keeping two hops, harden protocol before feature expansion**
- Add `request_id`, idempotency key, and cancel endpoint.
- Align all timeouts and define ownership for timeout.
- Add explicit error taxonomy (`retryable`, `non_retryable`, `partial_unknown`).

3. **Fix concurrency model before fanout claims**
- Do not block GenServer callbacks on network + sleeps.
- Move HTTP calls to supervised tasks or a connection pool worker model.

4. **Use one agent-definition source of truth now**
- Dual-format compatibility should be short-lived with a hard deprecation date.

5. **Cut Phase 5 entirely from migration doc**
- Those are separate epics, not part of safe delegation migration.

## 5) Elixir/OTP anti-patterns in proposed/current design
Yes, several:

1. **Blocking work inside GenServer callbacks**
- Network call + retry sleeps in `handle_call` ([rust_bridge.ex:73](rust_bridge.ex:73), [rust_bridge.ex:129](rust_bridge.ex:129)).

2. **Coordinator as bottleneck GenServer**
- Long-running strategy execution inside `handle_call` ([agent_coordinator.ex:63](agent_coordinator.ex:63)).

3. **Unlinked fire-and-forget Task with manual reply**
- `Task.start` in `handle_call` + `GenServer.reply` can strand callers on task crash ([agent_server.ex:190](agent_server.ex:190)).

4. **Health checks not tied to external dependencies**
- `evaluate_health/1` ignores Rust bridge/provider outcomes ([agent_server.ex:323](agent_server.ex:323)).

5. **Snapshot restore behavior ignores persistence intent**
- Snapshot restore runs unconditionally at init ([agent_server.ex:116](agent_server.ex:116)); non-persistent agents can inherit stale state.

6. **“Fanout” currently bottlenecked by architecture**
- Even with `Task.async_stream`, central GenServers serialize critical segments.

## Highest-priority fixes before rollout
1. Remove blocking retry/sleep from GenServer callbacks (`RustBridge`, `AgentCoordinator`).
2. Add idempotency + cancellation + unified timeout policy.
3. Route capabilities from agent definitions, not only running processes.
4. Implement actual `AgentServer -> RustBridge` execution path and production-grade failure handling (not `:pending_bridge`) ([agent_server.ex:153](agent_server.ex:153)).
5. Reduce migration scope to one strategy (`first_available`) until stable.
