# Option B Orchestration Design — Implementation Review

**Reviewer**: Claude Sonnet 4.5
**Date**: 2026-03-30
**Design Document**: `docs/OPTION_B_ORCHESTRATION_DESIGN.md`
**Status**: ⚠️ Multiple blockers identified

---

## Executive Summary

The Option B design proposes routing all messages through Elixir orchestration while keeping Rust as the I/O layer. This review verifies technical claims against the actual codebase and identifies gaps between design assumptions and current implementation.

**Key Finding**: The design assumes infrastructure that doesn't yet exist. Critical pieces like `run_task` bridge integration (TEZ-146), Main Agent identity, and all-message forwarding are unimplemented. Phase 1 cannot proceed without addressing these blockers.

---

## ✅ Verified: What Exists

### 1. Bridge Endpoints (Partial)

**Location**: `src/gateway/api.rs:559-694`

**Verified**:
- ✅ `AgentRunBody` struct exists (lines 562-567)
- ✅ `ChannelSendBody` struct exists (lines 616-625)
- ✅ `POST /api/agent/run` endpoint exists (lines 570-612)
- ✅ `POST /api/channel/send` endpoint exists (lines 627-694)
- ✅ UDS and TCP variants both supported
- ✅ Auth via `require_auth_or_loopback` for TCP endpoints

**AgentRunBody Fields**:
```rust
pub struct AgentRunBody {
    pub agent: Option<String>,
    pub task: String,
    pub model: Option<String>,
    pub temperature: Option<f64>,
}
```

**ChannelSendBody Fields**:
```rust
pub struct ChannelSendBody {
    pub text: String,
    pub channel: String,
    pub chat_id: Option<String>,
    #[serde(rename = "reply_to_message_id")]
    pub quote_message_id: Option<serde_json::Value>,
    pub btw: Option<bool>,
}
```

### 2. AgentServer State Persistence

**Location**: `elixir/rustyclaw_orchestrator/lib/rustyclaw_orchestrator/agent_server.ex:450-506`

**Verified**:
- ✅ Snapshot system exists (`.snapshot.etf` files in `~/.rustyclaw/snapshots/`)
- ✅ Restores `accumulated_state` on startup
- ✅ Restores last 20 `history` entries
- ✅ Persists `last_active_at` timestamp
- ✅ Safe term deserialization with `:safe` flag
- ✅ Graceful fallback to fresh state on corrupt snapshot

### 3. BtwRouter Message Routing

**Location**: `elixir/rustyclaw_orchestrator/lib/rustyclaw_orchestrator/btw_router.ex:1-139`

**Verified**:
- ✅ Splits `/btw` vs normal messages correctly
- ✅ Routes `/btw` → BtwSupervisor (ephemeral side-channel)
- ✅ Routes other → `AgentServer.send_message(agent_name, message)`
- ✅ Case-insensitive `/btw ` matching (space required)
- ✅ Fetches agent context for BTW tasks (`accumulated_state`, `definition`, `session_id`)

**Current Limitation**: Requires explicit `agent_name` parameter. Does NOT resolve Main Agent by role.

### 4. Conversation History Tracking

**Location**: `src/channels/mod.rs:222,278-284,1644`

**Verified**:
- ✅ `conversation_histories: ConversationHistoryMap` in ChannelContext (line 222)
- ✅ Per-sender history keyed by `{channel}_{thread_ts}_{sender}` (lines 278-284)
- ✅ HashMap wrapped in `Arc<Mutex<>>` for concurrent access
- ✅ Thread-aware isolation for forum groups (thread_ts support)

**Critical Gap**: History is Rust-side only. Not transferred to Elixir when forwarding messages.

### 5. Graceful Elixir Fallback

**Location**: `src/channels/mod.rs:1632-1642`

**Verified**:
- ✅ Health check before forwarding to Elixir
- ✅ Fallback to local Rust agent loop if Elixir unavailable
- ✅ Strips `/btw` prefix before local processing
- ✅ Logs degraded mode: "Elixir orchestrator unavailable, processing locally"

**Current Scope**: Only applies to `/btw` messages. Does NOT cover all-message forwarding.

### 6. RustBridge HTTP Client

**Location**: `elixir/rustyclaw_orchestrator/lib/rustyclaw_orchestrator/rust_bridge.ex:1-283`

**Verified**:
- ✅ GenServer with async Task workers for concurrency
- ✅ Exponential backoff retry on 5xx errors and connection failures
- ✅ Max 3 retries with 500ms initial backoff
- ✅ 360s timeout for `run_task`, 60s for `send_to_channel`
- ✅ UDS and TCP transport support
- ✅ Startup health check (fire-and-forget, non-blocking)
- ✅ Provenance metadata passthrough

---

## ❌ Issues: Design vs Implementation Gaps

### 1. AgentRunBody Missing Context Overrides

**Design Claim** (line 122):
> `system_prompt: state.definition.system_prompt, memory_context: build_memory_context(state)`

**Reality**:
```rust
// src/gateway/api.rs:562-567
pub struct AgentRunBody {
    pub agent: Option<String>,
    pub task: String,
    pub model: Option<String>,
    pub temperature: Option<f64>,
    // ❌ NO system_prompt field
    // ❌ NO memory_context field
}
```

**Impact**: Elixir cannot inject system prompts or memory gists when calling Rust. All agents use global system prompt from Rust config.

**Fix Required**: Add optional fields to `AgentRunBody`:
```rust
pub system_prompt: Option<String>,
pub memory_context: Option<Vec<String>>, // or serialized ChatMessage vec
```

### 2. RustBridge.run_task Limited Payload

**Design Assumption** (line 129):
> RustBridge calls with system_prompt and memory_context overrides

**Reality**:
```elixir
# elixir/.../rust_bridge.ex:128-138
body = %{
  agent: agent_name,
  task: task,
  model: Keyword.get(opts, :model),
  temperature: Keyword.get(opts, :temperature)
}
# ❌ system_prompt not in opts
# ❌ memory_context not in opts
```

**Impact**: Even if Rust endpoint supported overrides, Elixir bridge doesn't send them.

**Fix Required**: Accept and forward `system_prompt` and `memory_context` in opts.

### 3. Channel Send Supports Only telegram/whatsapp

**Design Claim** (line 23):
> "Channels: Telegram, WhatsApp, Discord, IRC, etc"

**Reality**:
```rust
// src/gateway/api.rs:668-683
match body.channel.as_str() {
    "telegram" => { /* ... */ }
    "whatsapp" => { /* ... */ }
    other => Err(anyhow::anyhow!("Unsupported channel for send: {other}"))
}
```

**Impact**: Main Agent cannot respond via Discord/Slack/Signal/IRC channels even though Rust supports receiving from them.

**Fix Required**: Add channel dispatch cases for all 19 supported channels, or implement generic channel lookup from AppState.

### 4. Only /btw Messages Forwarded to Elixir

**Design Goal** (lines 76-87):
> "After (all messages forwarded, graceful fallback)"

**Reality**:
```rust
// src/channels/mod.rs:1632-1642
if msg.content.to_ascii_lowercase().starts_with("/btw ") {
    if let Some(channel) = target_channel.as_ref() {
        if try_forward_to_elixir(&msg, channel.as_ref()).await {
            return;
        }
    }
    // fallback to local processing
}
// ❌ All other messages go straight to Rust agent loop
```

**Impact**: Main Agent never receives normal messages. Elixir orchestration layer is bypassed for 99% of traffic.

**Fix Required**: Remove `/btw` guard, forward ALL messages, add graceful fallback for Elixir unavailability (as design specifies).

---

## 🚫 Blockers: Missing Infrastructure

### 1. TEZ-146: run_task Bridge Unimplemented (CRITICAL)

**Location**: `elixir/.../agent_server.ex:155-173`

**Status**: Placeholder only

```elixir
def handle_call({:run_task, task, provenance}, _from, state) do
  # ...

  # Task execution will be routed through RustBridge in TEZ-146.
  # For now, record the task and return a placeholder.
  result = {:ok, %{task: task, status: :pending_bridge}}

  state = state
    |> Map.put(:status, :idle)
    |> append_history(:task_executed, %{task: task, result: result})

  {:reply, result, state}
end
```

**Impact**: AgentServer cannot execute LLM tasks. The entire orchestration layer is non-functional without this.

**What Design Expects** (lines 122-154):
```elixir
def handle_call({:run_task, task, provenance}, from, state) do
  task_ref = Task.Supervisor.async_nolink(@task_supervisor, fn ->
    RustBridge.run_task(state.definition.name, task,
      provenance: provenance,
      system_prompt: state.definition.system_prompt,
      model: state.definition.model,
      memory_context: build_memory_context(state)
    )
  end)

  {:noreply, %{state |
    status: :running,
    pending_task: {from, task_ref},
    last_active_at: DateTime.utc_now()
  }}
end

def handle_info({ref, result}, %{pending_task: {from, %Task{ref: ^ref}}} = state) do
  Process.demonitor(ref, [:flush])
  GenServer.reply(from, result)

  state = %{state |
    status: :idle,
    pending_task: nil,
    last_active_at: DateTime.utc_now()
  }
  |> append_history(:task_completed, result)

  {:noreply, state}
end
```

**Work Required**:
1. Implement async Task.Supervisor pattern in AgentServer
2. Add `pending_task: {from, task_ref}` to state
3. Handle `{ref, result}` messages for task completion
4. Handle `{:DOWN, ref, ...}` for task crashes
5. Reply to original caller with final result

### 2. No Main Agent Identity

**Design Claim** (line 101):
> `resolve_main_agent/0` to find agent with `role: :main`

**Reality**: Neither exists.

**AgentDefinition Fields** (`agent_definition.ex:20-55`):
```elixir
@type t :: %__MODULE__{
  name: String.t(),
  persistent: boolean(),
  parent: String.t() | nil,
  # ... 13 more fields
  # ❌ NO role field
}
```

**Registry Lookups**:
- `Registry.lookup(AgentRegistry, agent_name)` — by name only
- `Registry.select(AgentRegistry, ...)` — no role-based queries exist

**Grep Results**:
- `resolve_main_agent` → 0 matches in entire Elixir codebase
- `role:` → 0 matches in AgentDefinition

**Impact**:
- BtwRouter requires explicit agent_name, can't route to "the Main Agent"
- No way to boot a special `:permanent` Main Agent on startup
- No distinction between Main Agent and sub-agents in supervisor strategy

**Fix Required**:
1. Add `role: :main | :sub` to AgentDefinition schema
2. Store role in Registry metadata: `{:via, Registry, {AgentRegistry, name, role}}`
3. Implement `resolve_main_agent/0`:
```elixir
def resolve_main_agent do
  Registry.select(AgentRegistry, [
    {{:"$1", :"$2", :"$3"}, [{:==, :"$3", :main}], [:"$1"]}
  ])
  |> List.first()
end
```
4. Update BtwRouter to call `resolve_main_agent()` instead of requiring agent_name param

### 3. No Conversation State Transfer to Elixir

**Design Question** (line 290):
> "how should per-chat conversation state be tracked"

**Rust Answer** (`src/channels/mod.rs:278-284`):
```rust
fn conversation_history_key(msg: &traits::ChannelMessage) -> String {
    match &msg.thread_ts {
        Some(tid) => format!("{}_{}_{}", msg.channel, tid, msg.sender),
        None => format!("{}_{}", msg.channel, msg.sender),
    }
}
```

**Elixir State** (`agent_server.ex:120-141`):
```elixir
%{
  definition: %AgentDefinition{},
  session_id: nil,
  status: :idle,
  health: :healthy,
  history: [],  # ← Event history (spawn, delegate, etc), NOT chat history
  accumulated_state: %{},
  # ❌ NO conversation_history field
  # ❌ NO conversation_key tracking
}
```

**Impact**: Conversation history stays in Rust's HashMap. Each `RustBridge.run_task` call is stateless — Rust's `process_message` function reconstructs context fresh every time (see `src/agent/loop_.rs:3213`). Main Agent has no way to maintain cross-message conversation context through Elixir.

**Design Assumption** (lines 158-160):
> "Main Agent receives all messages and maintains conversation history across calls"

**Reality**: Rust maintains it, not Elixir. If messages route through Elixir, conversation history breaks unless:
1. Elixir manages conversation_histories HashMap (port from Rust), OR
2. Rust endpoint accepts `conversation_history: Vec<ChatMessage>` in AgentRunBody payload, OR
3. Messages continue to flow through Rust channels layer (defeats the orchestration design)

**Recommended Fix**: Option 2 — extend AgentRunBody with optional `conversation_history` field, let Elixir manage per-chat state and inject it on each RustBridge call.

### 4. process_message is Completely Stateless

**Design Assumption**: Elixir can call Rust and have conversation context persist.

**Reality** (`src/agent/loop_.rs:3213-3252`):
```rust
pub async fn process_message(config: Config, message: &str) -> Result<String> {
    let observer: Arc<dyn Observer> =
        Arc::from(observability::create_observer(&config.observability));
    let runtime: Arc<dyn runtime::RuntimeAdapter> =
        Arc::from(runtime::create_runtime(&config.runtime)?);
    let security = Arc::new(SecurityPolicy::from_config(...));
    let mem: Arc<dyn Memory> = Arc::from(memory::create_memory_with_storage(...));
    let tools_registry = tools::all_tools_with_runtime(...);
    // ... 40 more lines of initialization

    // Single turn, no state carried forward
}
```

**Impact**: Every `POST /api/agent/run` call reconstructs the entire agent context from scratch:
- Observer
- Runtime adapter
- Security policy
- Memory backend
- Tools registry (37 tools)
- Peripherals (hardware boards)
- System prompt
- Provider

This works for single-turn stateless execution (current BtwServer usage), but cannot support multi-turn conversations or agent state evolution.

**Fix Required**: Either:
1. Accept `conversation_history` and `accumulated_state` in `AgentRunBody` and thread through `process_message`, OR
2. Port conversation management to Elixir entirely (breaking change — would need to reconstruct conversation from Elixir state on each call)

---

## 🔧 Required Changes for Phase 1 MVP

Phase 1 goal: "All messages flow through Elixir Main Agent. No delegation yet."

### Priority 0: Implement TEZ-146 (BLOCKING)

**File**: `elixir/.../agent_server.ex`

Replace placeholder `run_task` handler with async Task.Supervisor pattern (see design lines 122-154).

Estimated LOC: ~40

### Priority 1: Add system_prompt to Bridge Payload

**Files**:
- `src/gateway/api.rs` — add field to `AgentRunBody`
- `elixir/.../rust_bridge.ex` — forward in opts
- `src/agent/loop_.rs` — consume override in `process_message`

Estimated LOC: ~15 (Rust), ~10 (Elixir)

### Priority 2: Add Main Agent Identity

**Files**:
- `elixir/.../agent_definition.ex` — add `role` field to schema
- `elixir/.../agent_supervisor.ex` — `:permanent` for role=:main
- `elixir/.../btw_router.ex` — add `resolve_main_agent/0`
- `elixir/.../application.ex` — boot Main Agent on startup

Estimated LOC: ~60

### Priority 3: Forward All Messages to Elixir

**File**: `src/channels/mod.rs`

Remove `/btw` guard from `try_forward_to_elixir` call (line 1632). Add conversation_key to payload. Keep graceful fallback.

Estimated LOC: ~20

### Priority 4: Conversation History Transfer

**Option A** (Minimal): Pass history in payload
- Add `conversation_history: Option<Vec<ChatMessage>>` to `AgentRunBody`
- Elixir maintains per-chat history HashMap
- Forward on each `run_task` call

**Option B** (Larger refactor): Port conversation_histories to Elixir entirely
- Elixir BtwRouter extracts conversation_key from channel_info
- Stores in ETS table or AgentServer state
- Rust `process_message` becomes truly stateless

Recommended: **Option A** for Phase 1 (less intrusive).

Estimated LOC: ~40 (Option A), ~150 (Option B)

### Priority 5: Expand Channel Send Support

**File**: `src/gateway/api.rs:668-683`

Replace hardcoded telegram/whatsapp match with generic lookup from `state.channels_by_name`.

Estimated LOC: ~15

---

## 📊 Implementation Readiness Matrix

| Component | Design | Implementation | Gap | Blocker? |
|-----------|--------|----------------|-----|----------|
| Bridge endpoints | ✅ Specified | ⚠️ Exists (limited payload) | system_prompt, memory_context fields | No |
| AgentServer.run_task | ✅ Specified | ❌ Placeholder only | TEZ-146 unimplemented | **YES** |
| Main Agent identity | ✅ Specified | ❌ No role field, no resolve function | role field + Registry query | **YES** |
| All-message forwarding | ✅ Specified | ❌ Only /btw forwarded | Remove guard in channels/mod.rs | **YES** |
| Conversation history | ⚠️ Implicit assumption | ❌ Lives in Rust, not transferred | Payload extension or state port | No (workaround: local Rust loop) |
| Channel send | ✅ Specified (all 19) | ⚠️ Only telegram/whatsapp | Add dispatch cases | No |
| Graceful fallback | ✅ Specified | ✅ Exists (for /btw) | Expand to all messages | No |
| State snapshots | ✅ Specified | ✅ Implemented | None | No |
| BtwRouter | ✅ Specified | ✅ Implemented | Needs resolve_main_agent | Depends on Main Agent identity |

**Phase 1 Blockers**: 3 (TEZ-146, Main Agent identity, all-message forwarding)

---

## 🚦 Go/No-Go Recommendation

**Status**: 🔴 **NO-GO** for Phase 1 implementation until blockers resolved.

**Rationale**:
1. TEZ-146 is non-negotiable — without it, AgentServer is non-functional
2. Main Agent identity is architectural — can't route to "the Main Agent" without it
3. All-message forwarding is the core design goal — without it, we're just adding overhead to /btw

**Suggested Path Forward**:
1. Complete TEZ-146 implementation (~2-3 days)
2. Add Main Agent role field and resolve function (~1 day)
3. Implement all-message forwarding with conversation_key (~1 day)
4. Extend bridge payload for system_prompt override (~0.5 day)
5. Test Phase 1 MVP: normal message → Elixir → RustBridge → LLM → channel
6. Then proceed to Phase 2 (skill invocation)

**Estimated Timeline**: 4-5 days of focused work to unblock Phase 1.

---

## 📝 Notes on Design vs Reality

### What the Design Got Right

- **Separation of concerns**: Rust for I/O, Elixir for orchestration — clean boundary
- **Graceful degradation**: Fallback to local Rust loop if Elixir unavailable
- **Forward compatibility**: Option B → Option A migration path preserved
- **Minimal Rust changes**: ~50 LOC estimate was accurate for the Rust side
- **BtwRouter pattern**: Already exists and works correctly

### What the Design Underestimated

- **Conversation state**: Design assumes Elixir can "just maintain it", but Rust's channels layer has sophisticated per-chat/per-thread history management that can't be trivially replicated
- **Bridge statefulness**: `process_message` is deeply stateless — wasn't designed for orchestration use case
- **Channel coverage**: Assumed `send_to_channel` worked for all channels (it doesn't)
- **Implementation status**: Assumed TEZ-146 was close to done (it's still a placeholder)

### Biggest Surprise

**Conversation history lives in Rust channels layer, not agent layer.**

The agent's `process_message` is a pure function: `(Config, &str) -> Result<String>`. All statefulness lives upstream in `ChannelContext.conversation_histories`. This works beautifully for the single-agent Rust architecture but creates an impedance mismatch for Option B's "Elixir is the brain" model.

Recommended resolution: Extend `AgentRunBody` to accept `conversation_history`, shift state management to Elixir, and make Rust's agent loop consume history from the payload rather than the context HashMap.

---

## ✅ Conclusion

The Option B design is architecturally sound and well-reasoned. However, it assumes infrastructure that doesn't exist yet:

- ❌ TEZ-146 (run_task bridge) unimplemented
- ❌ Main Agent role/identity missing
- ❌ All-message forwarding disabled
- ⚠️ Conversation state not transferred to Elixir

**Recommendation**: Do not proceed with Phase 1 until all three blockers are resolved. Estimated 4-5 days of work to unblock.

Once unblocked, the implementation is straightforward and the design should work as specified.
