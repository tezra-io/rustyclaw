## Code Review Summary

**Files reviewed**: 5 (`TOOL_SYNTHESIS_DESIGN.md`, `traits.rs`, `application.ex`, `rust_bridge.ex`, `resource_lock.ex`)
**Overall assessment**: REQUEST_CHANGES

This design is ambitious, but in its current form it is not safe to ship. The security model overestimates what AST blocklists and OTP supervision can guarantee for runtime-compiled untrusted Elixir.

---

## Findings

### P0 - Critical

1. **[TOOL_SYNTHESIS_DESIGN.md:118] Blocklist-based static analysis is not enforceable against Elixir metaprogramming and indirection**
- The design relies on blocking specific function names (`System.cmd`, `File.write`, `send/2`, etc.), but this is bypassable via indirection (`apply/3`, `:erlang.apply/3`, captured functions, aliases), alternate modules (`:file`, other HTTP clients), and macro expansion.
- `Code.string_to_quoted/1` analysis (`TOOL_SYNTHESIS_DESIGN.md:232`) sees pre-expansion AST; macros can generate blocked calls after analysis.
- Compile-time hooks/macros are a major gap (`require`, `use`, top-level expressions, `@on_load`) and are not addressed.
- Suggested fix: switch from blocklist to a strict allowlist AST subset (no macros/import/require/alias, no remote calls except approved modules/functions), or avoid compiling arbitrary Elixir entirely.

2. **[TOOL_SYNTHESIS_DESIGN.md:149] “Compilation sandbox” is not an actual isolation boundary**
- `Code.compile_string/1` executes compilation in the same BEAM and loads modules globally; a short-lived process wrapper does not isolate side effects.
- Compiled code can execute compile-time code before runtime safeguards apply.
- The claim "Yes — OTP isolation" (`TOOL_SYNTHESIS_DESIGN.md:24`) is incorrect for untrusted code execution.
- Suggested fix: compile/execute in a separate OS-isolated runtime (separate VM/container with no network/filesystem privileges), and treat it as untrusted plugin execution.

3. **[TOOL_SYNTHESIS_DESIGN.md:156] Runtime limits are described as hard guarantees but are soft and bypassable**
- Per-process memory "ceiling" via monitoring is not a hard cap in BEAM; memory can spike before monitors react, binaries/ETS accounting is tricky, and VM-wide pressure is still possible.
- Timeout wrappers cannot prevent side effects that already happened.
- Generated code can still create VM pressure (hot loops, process storms through unblocked APIs) even if killed later.
- Suggested fix: enforce limits at OS boundary; BEAM-only controls should be documented as best-effort, not security boundaries.

### P1 - High

4. **[TOOL_SYNTHESIS_DESIGN.md:104] Tool interface mapping is underspecified and currently mismatched to Rust contracts**
- Elixir behavior returns `{:ok, String.t()} | {:error, String.t()}` (`TOOL_SYNTHESIS_DESIGN.md:96`), while Rust expects `ToolResult { success, output, error }` and `anyhow::Result` (`traits.rs:6`, `traits.rs:33`).
- No concrete adapter contract is defined for schema validation, error normalization, or key conventions (string vs atom keys in params).
- Suggested fix: define one canonical wire contract and validate every synthesized tool against it before registration.

5. **[TOOL_SYNTHESIS_DESIGN.md:151] `Code.compile_string/1` edge cases are not handled**
- It can compile multiple modules from one source; your sample logic only accepts exactly one (`TOOL_SYNTHESIS_DESIGN.md:241`).
- Module redefinition/hot-upgrade semantics (old/new code versions, purge/delete) are not addressed.
- Unbounded unique module names consume atoms/code server memory over time.
- Suggested fix: enforce single-module AST precondition, explicit unload flow with `:code.purge/:code.delete`, and strict limits on synthesis/version churn.

6. **[TOOL_SYNTHESIS_DESIGN.md:255] LLM reliability assumptions are optimistic for production tooling**
- One example-based test is easy to overfit and not a correctness signal.
- LLM-generated Elixir will frequently be structurally valid but semantically brittle (schema drift, edge-case failures, wrong return shapes).
- Suggested fix: start with constrained template/DSL generation and deterministic validators, not free-form modules.

### P2 - Medium

7. **[TOOL_SYNTHESIS_DESIGN.md:164] Probation system adds state-machine complexity without solving main risk**
- Probation measures operational success rate, not safety.
- A malicious or unsafe tool can pass probation if it returns `{:ok, ...}`.
- Crash-based suspension overlaps with supervisor behavior and may not justify lifecycle complexity in v1.
- Suggested fix: replace with simple manual approval + basic telemetry first.

8. **[application.ex:12] Integration scope is much larger than design framing implies**
- Current supervision tree has none of the synthesis components yet (`application.ex:12`).
- Bridge currently supports `/api/agent/run` and `/api/health` only (`rust_bridge.ex:9`, `rust_bridge.ex:85`), so synth execution paths are net-new operational surface.
- Existing lock primitive (`resource_lock.ex`) does not directly mitigate synthesis runtime abuse.
- Suggested fix: treat as a platform feature with phased hardening, not a small extension.

---

## Direct Answers To Requested Focus Areas

1. **Security / blocklist enforceability**: Not enforceable as proposed. It is bypassable via macros, indirection, alternate modules, and compile-time execution paths.
2. **BEAM specifics / `Code.compile_string`**: Works technically, but not as a safe sandbox. It loads global code, has hot-code/versioning edge cases, and can execute compile-time side effects.
3. **Practical viability of LLM-generated modules**: Viable only for narrow pure transformations under tight constraints. Not reliable enough for broad autonomous tool authoring.
4. **Probation system**: Mostly complexity at this stage. It helps operations/quality tracking, not security or correctness guarantees.
5. **Minimum viable version**: Start much smaller (see below).

---

## Minimum Viable Version (Recommended)

1. Keep `tool_synthesis.enabled = false` by default; require explicit operator enablement.
2. Remove free-form Elixir generation in v1. Generate a constrained declarative plan/DSL that a trusted interpreter executes.
3. If you insist on codegen, enforce strict allowlist AST (single `defmodule`, no macros/import/alias/require, no remote calls outside approved pure modules).
4. Require human approval before first execution (not just before promotion).
5. Disable persistence and cross-agent sharing in v1; keep synthesized artifacts ephemeral per session.
6. Drop probation state machine for v1; keep only invocation count, latency, and explicit pass/fail logs.

Current design direction is conceptually interesting, but the security claims are stronger than what the mechanisms can guarantee. As written, this is a research prototype design, not a production-safe architecture.
