# Tool Synthesis — BEAM Hot-Loaded Agent Tool Generation

*Branch: `feature/tool-synthesis` | Author: Aira | Created: 2026-03-14*
*Revised: 2026-03-14 — incorporated review feedback, made my calls*

---

## Revision Log

**Rev 2 (2026-03-14):** Both reviewers (Codex, Claude Code) said REVISE. I agree on the security changes, disagree on scope reduction.

**What I'm taking:**
- Switch from blocklist to **allowlist-primary** static analysis. Both reviewers flagged `apply/3`, `:erlang` escape hatches, `import/use/require` bypasses. They're right — blocklist is a game of whack-a-mole.
- Block `import`, `use`, `require`, `defmacro`, `defmacrop` — no metaprogramming in synthesized tools
- Add `apply/3`, `Kernel.apply/3`, `:erlang.apply/3`, `Function.capture/3` to hard blocks
- Block `@on_load` callbacks (execute during compilation)
- Block `:ets`, `:file`, `:os`, `Module.create/3`, `:code.load_binary/3` (Erlang-level escapes)
- Add `String.to_atom/1` block (atom exhaustion — atoms aren't GC'd)
- Add output validation + size cap at execution layer
- Add synthesis rate limiting (max 3 per agent per hour)
- Specify Rust-side tool discovery mechanism

**What I'm rejecting:**
- Codex says "avoid compiling arbitrary Elixir entirely" and suggests DSL. No. The whole point is real Elixir modules. If we wanted a DSL, we'd build a DSL — this is about leverage. Allowlist makes it safe enough.
- Codex says compile in separate VM/container. Overkill for Phase 1-3. Sacrificial Erlang node is a good Phase 5 idea (Claude Code suggested this too) but not needed when allowlist + no metaprogramming + hard timeouts are in place.
- Codex says drop probation for v1. Disagree — it's not that much complexity and it prevents garbage accumulation. But I'll simplify it.
- Codex says "require human approval before first execution." Compromise: opt-in. Default is `auto_compile = false` (preview mode), but users can set `auto_compile = true` if they trust the system.

See reviews: `docs/CODEX_SYNTHESIS_REVIEW.md`, `docs/CLAUDE_CODE_SYNTHESIS_REVIEW.md`

---

## The Idea

Agents that write, compile, and deploy their own tools at runtime. No restart, no redeploy. The BEAM was built for hot code loading — we're using it for what it was designed to do.

When an agent encounters a task where no existing tool fits, it synthesizes a new one. The tool compiles in the running BEAM, registers in the tool registry, and is immediately available — to the agent that created it and to every other agent in the system. Tools that work well get persisted. Tools that crash get unloaded by OTP supervision. Over time, the system evolves capabilities custom-fit to its owner's workflows.

---

## Why This Is Only Possible Here

| Framework | Runtime compilation? | Safe? | Hot reload? |
|-----------|---------------------|-------|-------------|
| LangGraph (Python) | `exec()` — yes but unsafe | No | No — restart required |
| CrewAI (Python) | Same | No | No |
| AutoGen (Python) | Same | No | No |
| OpenClaw (Node.js) | `eval()` — yes but unsafe | No | No |
| Pure Rust | No runtime compilation | N/A | No |
| **RustyClaw (Elixir/BEAM)** | `Code.compile_string/1` | **Yes — OTP isolation** | **Yes — native** |

The BEAM compiles Elixir to bytecode at runtime. Erlang has done this in production telecom systems for 30+ years. We're not inventing hot code loading — we're applying it to AI tool generation.

---

## Architecture

```
Agent encounters task with no suitable tool
    │
    ▼
┌─────────────────────────────────────────────┐
│  ToolSynthesizer (Elixir GenServer)         │
│                                             │
│  1. Agent describes needed capability       │
│  2. Synthesizer generates Elixir module     │
│     via LLM call through RustBridge         │
│  3. Static analysis + validation            │
│  4. Compile in sandboxed BEAM process       │
│  5. Register in SynthesizedToolRegistry     │
│  6. Return tool spec to requesting agent    │
└──────────────────┬──────────────────────────┘
                   │
    ┌──────────────┼──────────────────┐
    ▼              ▼                  ▼
┌────────┐  ┌──────────┐  ┌───────────────┐
│ Use it │  │ Persist  │  │ Probation     │
│ now    │  │ if good  │  │ monitoring    │
└────────┘  └──────────┘  └───────────────┘
```

### Key Components

**1. ToolSynthesizer** — Elixir GenServer
- Receives synthesis requests from agents (via tool call or AgentCoordinator)
- Calls RustBridge to get LLM to generate the Elixir module code
- Runs static analysis before compilation
- Compiles via `Code.compile_string/1`
- Registers in SynthesizedToolRegistry
- Manages tool lifecycle (probation → promoted → deprecated → unloaded)

**2. SynthesizedToolRegistry** — ETS table
- Maps tool name → module + metadata (author agent, creation time, success rate, invocation count)
- Separate from the static tool registry (defense in depth)
- Queryable by capability tags for discovery

**3. ToolSandbox** — Supervision subtree
- All synthesized tools run under a dedicated DynamicSupervisor
- Crash isolation: a bad tool can't take down the rest of the system
- Resource limits: per-tool memory ceiling via process monitoring
- Timeout enforcement: every tool call has a hard deadline

**4. ToolPersistence** — File-backed storage
- Promoted tools saved to `~/.rustyclaw/synthesized_tools/`
- Each tool: `<name>.ex` (source) + `<name>.meta.json` (metadata)
- Loaded on startup if validation passes
- Version history for rollback

---

## The Tool Interface

Synthesized tools implement a simple behaviour:

```elixir
defmodule RustyclawOrchestrator.SynthesizedTool do
  @doc "Behaviour that all synthesized tools must implement."

  @callback name() :: String.t()
  @callback description() :: String.t()
  @callback parameters_schema() :: map()
  @callback execute(params :: map()) :: {:ok, String.t()} | {:error, String.t()}

  # Optional
  @callback capabilities() :: [String.t()]
  @optional_callbacks [capabilities: 0]
end
```

This maps 1:1 to Rust's `Tool` trait:
- `name()` → `fn name(&self) -> &str`
- `description()` → `fn description(&self) -> &str`
- `parameters_schema()` → `fn parameters_schema(&self) -> serde_json::Value`
- `execute(params)` → `async fn execute(&self, args: Value) -> Result<ToolResult>`

The Elixir tool gets exposed to the Rust side via RustBridge — Rust calls Elixir to execute synthesized tools, same as it calls Elixir for routing.

---

## Security Model (Revised — Allowlist-Primary)

This is the section that matters most. Generated code is inherently risky. The original design used a blocklist — reviewers correctly identified that blocklists lose to `apply/3`, Erlang escape hatches, and metaprogramming. Revised to allowlist-primary.

### Layer 1: Static Analysis — Allowlist-Primary (before compilation)

Before `Code.compile_string/1` ever runs, the AST is walked. The fundamental principle: **if it's not explicitly allowed, it's rejected.**

**Allowed modules (the allowlist — exhaustive):**
```elixir
@allowed_modules [
  Enum, Map, List, String, Regex, Jason, Integer, Float,
  Tuple, Keyword, MapSet, Stream, Range, Access, URI,
  Base, Bitwise, Date, Time, DateTime, NaiveDateTime,
  IO  # Only IO.inspect for debugging — IO.cmd blocked separately
]
```

**Allowed language constructs:**
- `defmodule` (exactly one, in `RustyclawOrchestrator.Synth.*` namespace)
- `def`, `defp` (regular function definitions)
- Pattern matching, guards, `case`, `cond`, `if`, `with`, `for`
- Pipe operator, comprehensions, `fn` lambdas
- Binary/string patterns, sigils (`~r`, `~s`, `~w`)
- `@doc`, `@moduledoc`, `@behaviour`, `@type`, `@spec` (documentation/types only)
- `Logger.debug/info/warning/error` (observability)

**Hard blocks (anything not in the allowlist, plus explicit blocks for defense-in-depth):**
- ALL `import`, `use`, `require` statements (no module importing = no capability escalation)
- `defmacro`, `defmacrop` (no metaprogramming — macros execute at compile time)
- `apply/3`, `Kernel.apply/3`, `:erlang.apply/3` (dynamic dispatch bypass)
- `Function.capture/3`, `&Module.fun/arity` for non-allowed modules
- `@on_load` (executes during compilation, before validation)
- `Module.create/3`, `:code.load_binary/3`, `:code.load_abs/1` (runtime module/bytecode loading)
- `Code.*` (no meta-compilation)
- `EEx.*` (template execution)
- `String.to_atom/1` (atom exhaustion — atoms aren't GC'd; use `String.to_existing_atom/1`)
- ALL Erlang module calls (`:file`, `:os`, `:erlang`, `:ets`, `:net_kernel`, `:gen_tcp`, `:ssl`, `:httpc`, etc.)
- `Process.*`, `spawn*`, `send/2`, `Port.*`, `Node.*`, `System.*`
- `File.*`, `Path.*` (filesystem access)
- Any remote call `Module.function` where `Module` is not in `@allowed_modules`

**How the AST walk works:**
```elixir
# Parse without compilation
{:ok, ast} = Code.string_to_quoted(source)

# Walk every node
Macro.prewalk(ast, fn
  # Reject any remote call to non-allowed module
  {{:., _, [{:__aliases__, _, module_parts}, _fun]}, _, _} = node ->
    module = Module.concat(module_parts)
    if module not in @allowed_modules, do: raise("blocked: #{module}")
    node

  # Reject Erlang module calls (:atom.function)
  {{:., _, [atom, _fun]}, _, _} = node when is_atom(atom) ->
    raise("blocked: Erlang module #{atom}")

  # Reject import/use/require
  {directive, _, _} = node when directive in [:import, :use, :require] ->
    raise("blocked: #{directive}")

  # Reject defmacro
  {macro_def, _, _} = node when macro_def in [:defmacro, :defmacrop] ->
    raise("blocked: #{macro_def}")

  # Reject apply
  {:apply, _, _} = node -> raise("blocked: apply")

  node -> node
end)
```

**Why allowlist > blocklist:** We don't need to predict what the LLM tries. If it uses any module/construct outside the allowlist, it's rejected. New Elixir features, obscure stdlib modules, Erlang interop — all blocked by default. We only open doors, never close them after the fact.

### Layer 2: Compilation Sandbox

- Compile in a short-lived process with `:kill` timeout (5s max)
- Module name **enforced** to `RustyclawOrchestrator.Synth.<ToolName>` namespace (not just a convention — reject if AST shows different module name)
- Single module per compilation (reject if AST contains multiple `defmodule`)
- If compilation fails, error returned to agent — no retry spam
- Compiled module's exports validated against the `SynthesizedTool` behaviour
- Module IS globally visible on the BEAM (this is how the BEAM works — accepted tradeoff, mitigated by namespace + allowlist)
- `@on_load` blocked in Layer 1, so no compile-time code execution

### Layer 3: Runtime Sandbox

- Synthesized tools execute under `ToolSandbox` DynamicSupervisor
- Each execution wrapped in `Task.async` with hard timeout (30s default, configurable)
- Process memory monitored — killed if exceeds limit (50MB default). Note: this is best-effort, not a hard BEAM guarantee. Acceptable because allowlisted functions can't do memory-intensive operations (no process spawning, no ETS, no file I/O)
- **Output validation:** Must return `{:ok, String.t()} | {:error, String.t()}`. Anything else = failure.
- **Output size cap:** 1MB max. Larger outputs truncated.
- All tool output passes through Rust's credential scrubbing before reaching the agent

### Layer 4: Probation System (Simplified)

Every new tool starts on probation. Simplified from original — fewer states, clearer triggers:

```
PROBATION (first 10 invocations)
  │
  ├── Success rate ≥ 80% AND human approves → PROMOTED
  │   (auto_promote=true skips human approval)
  │
  ├── Success rate < 50% after 10 runs → DEPRECATED (unloaded)
  │
  └── Any crash/timeout/blocked-output → SUSPENDED
      │
      └── Human reviews → PROMOTE / DEPRECATE / DELETE
```

**Metrics tracked beyond success rate:**
- Average latency per invocation
- Output size distribution
- Input diversity (are the same inputs being recycled?)
- Failure patterns (same error repeated = systematic bug, not flaky)

**Rate limiting:** Max 3 synthesis attempts per agent per hour. Prevents synthesis spam loops.

### Layer 5: Human Override

- `rustyclaw synth list` — show all synthesized tools with status
- `rustyclaw synth inspect <name>` — show source code + metrics + static analysis results
- `rustyclaw synth preview <request>` — dry run: generate code but don't compile (NEW)
- `rustyclaw synth approve <name>` — manually promote
- `rustyclaw synth suspend <name>` — take offline
- `rustyclaw synth delete <name>` — remove permanently
- Config: `tool_synthesis.enabled = false` (default — opt-in)
- Config: `tool_synthesis.auto_compile = false` (default — preview mode, human approves before compilation)
- Config: `tool_synthesis.auto_promote = false` (default — human approves before promotion)

### Layer 6: Future — Sacrificial Node (Phase 5)

For maximum isolation: compile and execute synthesized tools on a connected-but-separate Erlang node started with restricted OS capabilities (no file access, no network, capped memory). This is how Erlang was designed for telecom fault isolation. Communicate via `:erpc`. Not needed for Phases 1-4 but the right move for hardened production deployments.

---

## Synthesis Flow (Detailed)

### Step 1: Agent requests synthesis

Agent calls the `synthesize_tool` tool (a meta-tool):

```json
{
  "name": "synthesize_tool",
  "arguments": {
    "capability": "Parse CSV files and extract specific columns by header name",
    "suggested_name": "csv_column_extractor",
    "input_example": {"csv_text": "name,age,city\nAlice,30,NYC", "columns": ["name", "city"]},
    "expected_output": "name,city\nAlice,NYC"
  }
}
```

### Step 2: ToolSynthesizer generates code

Calls RustBridge → LLM with a tightly constrained prompt:

```
Generate an Elixir module that implements the SynthesizedTool behaviour.

The module MUST:
- Be named RustyclawOrchestrator.Synth.CsvColumnExtractor
- Implement name/0, description/0, parameters_schema/0, execute/1
- execute/1 takes a map and returns {:ok, string} or {:error, string}
- Use ONLY: Enum, Map, List, String, Regex, Jason, binary matching
- NO: File, System, Port, Process, Network, Code, spawn, send

Capability needed: Parse CSV files and extract specific columns by header name
Input example: %{"csv_text" => "name,age,city\nAlice,30,NYC", "columns" => ["name", "city"]}
Expected output: "name,city\nAlice,NYC"
```

### Step 3: Static analysis

AST-level scan using `Code.string_to_quoted/1`:
- Walk the AST looking for blocked function calls
- Verify module name matches expected namespace
- Verify all behaviour callbacks are implemented
- Check code size (max 500 lines — tools should be focused)

### Step 4: Compile + validate

```elixir
case Code.compile_string(source) do
  [{module, _bytecode}] ->
    # Verify behaviour
    if function_exported?(module, :name, 0) and
       function_exported?(module, :execute, 1) do
      {:ok, module}
    else
      {:error, :missing_callbacks}
    end

  _ -> {:error, :compilation_failed}
end
```

### Step 5: Test with provided example

If `input_example` and `expected_output` were provided:
```elixir
case module.execute(input_example) do
  {:ok, output} when output == expected_output -> :pass
  {:ok, output} -> {:warn, "Output differs: #{output}"}
  {:error, reason} -> {:fail, reason}
end
```

### Step 6: Register + announce

- Add to SynthesizedToolRegistry (ETS)
- Status: `:probation`
- Notify requesting agent: tool available
- Expose to Rust via bridge (synthesized tools appear in tool list)

---

## Rust Integration

Synthesized tools need to be callable from Rust's agent loop.

### SynthToolProxy (Rust-side)

A single Rust struct implementing the `Tool` trait that proxies all synthesized tool calls to Elixir:

```rust
struct SynthToolProxy {
    name: String,
    description: String,
    schema: serde_json::Value,
    elixir_url: String,
}

#[async_trait]
impl Tool for SynthToolProxy {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { &self.description }
    fn parameters_schema(&self) -> serde_json::Value { self.schema.clone() }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let resp = reqwest::Client::new()
            .post(format!("{}/api/synth/execute", self.elixir_url))
            .json(&json!({"tool": self.name, "params": args}))
            .timeout(Duration::from_secs(35)) // slightly above Elixir's 30s
            .send().await?;
        // Parse Elixir response → ToolResult
        let body: SynthResponse = resp.json().await?;
        Ok(ToolResult {
            success: body.ok,
            output: body.output,
            error: body.error,
        })
    }
}
```

### Tool Discovery (NEW — from review feedback)

Rust needs to know which synthesized tools exist so it can include them in the LLM's tool list. Mechanism:

1. **On agent run start:** Rust calls `GET /api/synth/tools` → Elixir returns list of `{name, description, schema, status}` for all active synthesized tools
2. **Rust builds `SynthToolProxy` instances** for each and adds them to the tool registry for that agent run
3. **Cache with TTL:** Rust caches the tool list for 60s. Synthesis doesn't happen often enough to need real-time updates.
4. **Cache invalidation:** When a new tool is synthesized, Elixir can optionally POST a notification to Rust to bust the cache. If missed, TTL handles it.

This is lightweight — one HTTP call per agent run start, cached. Synthesized tools show up alongside built-in tools in the LLM's function list.

---

## File Layout

```
elixir/rustyclaw_orchestrator/lib/rustyclaw_orchestrator/
├── tool_synthesis/
│   ├── synthesizer.ex          # GenServer — orchestrates synthesis flow
│   ├── static_analyzer.ex      # AST-walking security scanner
│   ├── sandbox.ex              # DynamicSupervisor for safe execution
│   ├── registry.ex             # ETS-backed tool registry + metadata
│   ├── persistence.ex          # File-backed storage for promoted tools
│   ├── probation.ex            # Lifecycle state machine
│   └── bridge_handler.ex       # Plug handler for Rust bridge calls
├── synthesized_tool.ex         # Behaviour definition

~/.rustyclaw/synthesized_tools/
├── csv_column_extractor.ex     # Persisted source
├── csv_column_extractor.meta.json  # Metadata + metrics
└── ...
```

---

## Config

```toml
[tool_synthesis]
enabled = false                    # Opt-in (safe default)
auto_compile = false               # Preview mode by default — human approves before compilation
auto_promote = false               # Require human approval for promotion
max_tools = 50                     # Cap on total synthesized tools
max_tools_per_agent = 10           # Per-agent cap (prevents spam)
max_synthesis_per_hour = 3         # Rate limit per agent
max_code_lines = 500               # Per-tool source limit
compilation_timeout_ms = 5000      # Kill compilation after 5s
execution_timeout_ms = 30000       # Kill execution after 30s
execution_output_max_bytes = 1048576  # 1MB output cap
memory_limit_mb = 50               # Per-execution memory ceiling (best-effort)
probation_invocations = 10         # Runs before promotion eligible
min_success_rate = 0.8             # Required for auto-promotion
model = "anthropic/claude-sonnet"  # LLM used for code generation
```

---

## Migration / Rollout Plan

### Phase 1: Foundation
- `SynthesizedTool` behaviour
- `StaticAnalyzer` — AST scanner with blocked/allowed function lists
- `ToolSandbox` — DynamicSupervisor with resource limits
- `SynthesizedToolRegistry` — ETS table
- Unit tests for all of the above
- **No LLM integration yet** — tools are created programmatically in tests

### Phase 2: Synthesis engine
- `ToolSynthesizer` GenServer
- LLM prompt engineering for code generation (via RustBridge)
- Compile → validate → test → register flow
- `synthesize_tool` meta-tool (callable by agents)
- `Persistence` module for promoted tools
- Load persisted tools on startup

### Phase 3: Rust integration
- `POST /api/synth/execute` endpoint in Elixir (requires Bandit — shares Phase 0 work with delegation feature)
- Rust-side `SynthToolProxy` that routes synth tool calls to Elixir
- Synthesized tools appear in agent tool listings
- CLI: `rustyclaw synth list/inspect/approve/suspend/delete`

### Phase 4: Probation + auto-evolution
- Probation state machine
- Metrics tracking (success rate, latency, invocation count)
- Auto-promotion/deprecation
- Agent-to-agent tool discovery (AgentCoordinator exposes synth tool capabilities)

### Phase 5: Self-improvement loop
- When a synthesized tool fails, agent can request a revised version
- Diff-based iteration: "fix this tool — it failed with this input, expected this output"
- Version history with rollback
- Tool composition: synthesized tools that call other synthesized tools

---

## What This Enables

**Day 1 (manual):** Developer writes agent definition, lists tools. Agent can only use what's pre-built.

**Day 30 (with synthesis):** Agent encounters a CSV parsing task. No CSV tool exists. Agent synthesizes `csv_column_extractor`. Tool works, gets promoted. Next week, a different agent needs CSV parsing — it's already there.

**Day 90:** System has 30+ custom tools evolved from real usage. Data transformation, format conversion, domain-specific calculations, API response parsing — all created by agents, validated by the probation system, approved by the human. The tool library is unique to this deployment.

**Day 180:** Agents start composing synthesized tools. A "report generator" tool chains `csv_column_extractor` → `data_summarizer` → `markdown_formatter`. The system has emergent capabilities nobody explicitly programmed.

---

## Open Questions

1. **Should synthesized tools have access to other tools?** — Initially no. Pure functions only. Phase 5 adds composition.

2. **How do we handle tool name collisions?** — Namespace under `synth.*`. If an agent tries to synthesize a tool with a name that matches a built-in, reject with suggestion to use a different name.

3. **Can tools be shared across RustyClaw instances?** — Future: export/import promoted tools. Think npm for agent tools. But not in scope for initial implementation.

4. **What about tools that need state?** — Synthesized tools are stateless functions. If state is needed, the agent should use the memory system. No GenServer-backed synthesized tools (too dangerous for auto-generated code).

---

---

## Review Response Summary

| Reviewer finding | My call |
|-----------------|---------|
| Blocklist bypassable via `apply/3`, `:erlang`, `import` | **ACCEPTED** — switched to allowlist-primary |
| `@on_load` executes during compilation | **ACCEPTED** — blocked in Layer 1 |
| `:ets`, `:file`, `:os` bypass Elixir blocks | **ACCEPTED** — all Erlang modules blocked |
| `String.to_atom/1` atom exhaustion | **ACCEPTED** — blocked |
| Compile in separate VM/container | **DEFERRED** to Phase 5 (sacrificial node) |
| Avoid compiling arbitrary Elixir / use DSL | **REJECTED** — whole point is real Elixir. Allowlist makes it safe enough. |
| Drop probation for v1 | **REJECTED** — keeps garbage out. Simplified instead. |
| Require human approval before first execution | **COMPROMISE** — opt-in via `auto_compile` flag |
| Add output validation + size cap | **ACCEPTED** |
| Rate limit synthesis per agent | **ACCEPTED** — 3/hour |
| Specify Rust-side discovery | **ACCEPTED** — SynthToolProxy + cache with TTL |
| Probation needs better success metrics | **ACCEPTED** — added latency, output size, input diversity tracking |

*This is my feature. Reviews inform, I decide.*
