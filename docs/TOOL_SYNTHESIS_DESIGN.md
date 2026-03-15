# Tool Synthesis — BEAM Hot-Loaded Agent Tool Generation

*Branch: `feature/tool-synthesis` | Author: Aira | Created: 2026-03-14*

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

## Security Model

This is the section that matters most. Generated code is inherently risky. Here's how we make it safe:

### Layer 1: Static Analysis (before compilation)

Before `Code.compile_string/1` ever runs:

**Hard blocks (compilation rejected):**
- `System.cmd`, `System.shell`, `:os.cmd` — no shell access
- `File.write`, `File.rm`, `File.rename` — no filesystem writes
- `Port.open`, `:erlang.open_port` — no port/process spawning
- `Node.connect`, `:net_kernel` — no distributed Erlang
- `Application.put_env`, `System.put_env` — no env mutation
- `Code.eval_string`, `Code.compile_string` — no meta-compilation (no turtles)
- `Process.exit`, `Process.flag` — no process manipulation
- `send/2` to arbitrary pids — no message injection
- Any module attribute with `@external_resource`
- Network calls: `:httpc`, `:gen_tcp`, `:ssl` — must go through sanctioned HTTP tool
- `__ENV__`, `__CALLER__`, `__STACKTRACE__` — no compiler introspection
- Kernel functions: `spawn`, `spawn_link`, `spawn_monitor`

**Soft blocks (require human approval):**
- `File.read`, `File.ls` — filesystem reads (may be needed, but gated)
- `Req.get`, `Req.post` — HTTP calls (useful but network access)
- `DateTime`, `System.monotonic_time` — time access (usually fine)

**Allowed freely:**
- Pure computation (math, string manipulation, data transformation)
- Pattern matching, guards, comprehensions
- Jason/JSON encoding/decoding
- Map/List/Enum/Stream operations
- Regex
- Binary pattern matching

### Layer 2: Compilation Sandbox

- Compile in a short-lived process with `:kill` timeout (5s max)
- Module name forced to `RustyclawOrchestrator.Synth.<ToolName>` namespace
- If compilation fails, error returned to agent — no retry spam
- Compiled module's exports validated against the `SynthesizedTool` behaviour

### Layer 3: Runtime Sandbox

- Synthesized tools execute under `ToolSandbox` DynamicSupervisor
- Each execution wrapped in `Task.async` with hard timeout (30s default, configurable)
- Process memory monitored — killed if exceeds limit (50MB default)
- No access to other GenServer state — tools are pure functions with I/O restrictions
- All tool output passes through Rust's credential scrubbing before reaching the agent

### Layer 4: Probation System

Every new tool starts on probation:

```
PROBATION (first 10 invocations)
  │
  ├── Success rate ≥ 80% → PROMOTED (persisted, available to all agents)
  │
  ├── Success rate < 50% → DEPRECATED (unloaded, source kept for analysis)
  │
  └── Any crash/timeout → SUSPENDED (human review required)
      │
      ├── Human approves fix → back to PROBATION
      └── Human rejects → DELETED
```

### Layer 5: Human Override

- `rustyclaw synth list` — show all synthesized tools with status
- `rustyclaw synth inspect <name>` — show source code + metrics
- `rustyclaw synth approve <name>` — manually promote
- `rustyclaw synth suspend <name>` — take offline
- `rustyclaw synth delete <name>` — remove permanently
- Config flag: `tool_synthesis.enabled = true/false` (default: false — opt-in)
- Config flag: `tool_synthesis.auto_promote = true/false` (default: false — require human approval for promotion)

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

Synthesized tools need to be callable from Rust's agent loop. Two approaches:

### Approach A: Bridge call per invocation (simple, start here)

When Rust's agent loop encounters a tool call for a synthesized tool:
1. Tool name matches prefix `synth.*` or is in the synthesized tool list
2. Rust sends `POST /api/synth/execute` to Elixir: `{tool: "csv_column_extractor", params: {...}}`
3. Elixir executes in ToolSandbox, returns result
4. Rust passes result back to agent loop

This adds one HTTP hop per synthesized tool call. Fine for now — synthesized tools are typically called once or twice per agent run, not in tight loops.

### Approach B: Rust-side stub generation (future optimization)

For promoted tools with stable interfaces, auto-generate a Rust `Tool` impl that caches the schema and calls Elixir only for execution. Eliminates schema lookup overhead.

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
auto_promote = false               # Require human approval for promotion
max_tools = 50                     # Cap on total synthesized tools
max_code_lines = 500               # Per-tool source limit
compilation_timeout_ms = 5000      # Kill compilation after 5s
execution_timeout_ms = 30000       # Kill execution after 30s
memory_limit_mb = 50               # Per-execution memory ceiling
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

*This is my feature. Reviews welcome, but I'll make the calls.*
