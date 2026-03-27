# Session Bridge Design: Telegram ↔ Claude Code

## Problem

RustyClaw handles Telegram messages by routing them through an LLM provider (Anthropic, OpenRouter, etc.) via the agent loop. This is good for general chat and tool-driven tasks, but wrong for exploratory development sessions where you need real-time, conversational interaction with Claude Code — the kind of work where you're reading code together, steering direction, and iterating on approaches from your phone.

Today, doing this requires opening a laptop and running `claude` in a terminal. The session bridge eliminates that requirement: bind a Telegram chat to a Claude Code process, and every message routes directly — no RustyClaw LLM in the path, no context burn, no tool-loop overhead.

## Tech Stack

**Already exists in RustyClaw:**
- `HookHandler` trait (`src/hooks/traits.rs`) with `on_message_received` returning `HookResult<ChannelMessage>` — the intercept point
- `HookRunner` (`src/hooks/runner.rs`) — sequential dispatcher with priority ordering, panic recovery, cancel short-circuiting
- `BuiltinHooksConfig` (`src/config/schema.rs:1948`) — pattern for adding config-gated builtin hooks
- Hook registration site in `start_channels()` (`src/channels/mod.rs:3400-3408`) — where the `HookRunner` is assembled
- Slash command parsing in `parse_runtime_command()` (`src/channels/mod.rs:492`) — existing `/models`, `/model`, `/new` handling for Telegram/Discord
- `Channel` trait with `send()` (`src/channels/traits.rs:76`) — how to push responses back to Telegram
- `channels_by_name` map in `ChannelRuntimeContext` (`src/channels/mod.rs:210`) — runtime access to channel instances
- Telegram `allowed_users` auth (`src/channels/telegram.rs:301`) — existing access control

**New:**
- `SessionBridgeHook` — a `HookHandler` implementation (`src/hooks/builtin/session_bridge.rs`)
- Binding table — maps `(channel, sender)` → active child process session
- Process manager — spawns/manages `claude --print --input-format stream-json --output-format stream-json` child processes (persistent, long-running)
- Slash command extensions — `/connect`, `/disconnect`, `/status` added to the existing command parser
- Config section — `[session_bridge]` in config schema

## Approach

The session bridge is a high-priority builtin hook that intercepts messages before the agent loop sees them. For bound conversations, it routes messages to a child process via stdin/stdout and returns `HookResult::Cancel` to prevent the agent loop from processing them. For unbound conversations, it returns `HookResult::Continue` — completely transparent.

**Key trade-off: hook-based vs. routing-layer.** An alternative would be to add a routing check earlier in the message dispatch path (before hooks fire). We chose the hook approach because:
1. It uses existing infrastructure — no new dispatch paths to maintain
2. Priority ordering gives clean precedence over other hooks
3. The `Cancel` semantic is exactly what we need (prevent agent loop consumption)
4. Future extensions (other agents, plugins) follow the same pattern

**Trade-off: child process vs. API.** We use Claude Code's stream-json mode: `--print --input-format stream-json --output-format stream-json`. This spawns a **single persistent process per session** that accepts JSON messages on stdin and emits JSON responses on stdout, maintaining full conversation context across messages. The process lives for the duration of the binding — one spawn per `/connect`, not per message. Fallback mode: if stream-json has issues, `--resume <session-id>` with per-message spawn preserves context via session files, at the cost of per-message startup overhead.

## Scope

**In scope (Phase 1):**
- `/connect claude-code [path]` and `/disconnect` slash commands on Telegram
- Binding table (in-memory with disk persistence for restart awareness)
- Child process lifecycle: spawn, pipe messages, detect exit, cleanup
- Output buffering and chunking for Telegram's 4096-char limit
- Auto-unbind on process crash with user notification
- Auth: only `allowed_users` can `/connect`
- Config: `[session_bridge]` section with defaults, max sessions, buffer timing

**Out of scope:**
- Non-Telegram channels (Discord, Slack, etc.) — architecture supports it, but not wired
- Codex or other agent backends — config is agent-agnostic, but only `claude-code` ships
- Streaming/progressive output (draft message updates)
- Interactive tool approval routing
- Session resume across RustyClaw restarts (we auto-unbind stale entries instead)
- Natural language binding ("start a Claude Code session")

## Data Flow

### Bound conversation (happy path)

```
Telegram Bot API
    │
    ▼
TelegramChannel::listen()        ← long-poll receives update
    │
    ▼
mpsc::Sender<ChannelMessage>     ← message enters dispatch queue
    │
    ▼
run_message_dispatch_loop()      ← src/channels/mod.rs:2195
    │
    ▼
on_message_received hooks        ← HookRunner::run_on_message_received()
    │                               src/channels/mod.rs:1612
    ▼
SessionBridgeHook                ← priority 100 (runs first)
    │
    ├─ Check: message starts with "/btw " → return Continue (passthrough to Elixir)
    ├─ Check: message starts with other RustyClaw command → return Continue
    │
    ├─ Lookup: binding_table.get((msg.channel, msg.sender))
    │  Found → route to persistent child process
    │
    ▼
Child process stdin              ← write JSON message (stream-json format)
    │                               {"type":"user","message":{"role":"user","content":"..."}}
    ▼
claude --print \                 ← persistent process, maintains conversation context
  --input-format stream-json \    spawned once at /connect, lives until /disconnect
  --output-format stream-json     or process exit
    │
    ▼
Child process stdout             ← read JSON response stream
    │                               {"type":"assistant","message":{"role":"assistant","content":"..."}}
    ▼
Channel::send()                  ← SessionBridgeHook sends via stored channel ref
    │                               (chunked if >4096 chars)
    ▼
Telegram Bot API                 ← user sees response
    │
    ▼
return HookResult::Cancel        ← agent loop never sees this message
```

### Slash command (/connect)

```
Telegram message: "/connect claude-code ~/projects/rustyclaw"
    │
    ▼
SessionBridgeHook::on_message_received()
    │
    ├─ Parse: recognized as /connect command
    ├─ Validate: sender is in allowed_users
    ├─ Validate: conversation not already bound
    ├─ Spawn: child process with working dir
    ├─ Store: binding_table.insert((channel, sender), session)
    ├─ Persist: write binding table to disk
    ├─ Reply: "Connected to Claude Code in ~/projects/rustyclaw"
    │
    ▼
return HookResult::Cancel        ← consumed by bridge
```

### Unbound conversation (passthrough)

```
SessionBridgeHook::on_message_received()
    │
    ├─ Check: message starts with "/btw " → return Continue immediately
    ├─ Check: message starts with "/models", "/model", "/new", etc. → return Continue
    │
    ├─ Lookup: binding_table.get((msg.channel, msg.sender))
    │  Not found, and not a /connect command
    │
    ▼
return HookResult::Continue(msg) ← message passes to next hook / agent loop
```

### Bound conversation — RustyClaw command passthrough

```
Telegram message: "/btw summarize the last tool call"
    │
    ▼
SessionBridgeHook::on_message_received()
    │
    ├─ Check: message starts with "/btw " → YES
    │
    ▼
return HookResult::Continue(msg) ← passes through to Elixir router at line 1632
                                    even though conversation is bound
```

## User Scenarios

### Connect and chat

1. User sends `/connect claude-code ~/projects/rustyclaw` in Telegram
2. SessionBridgeHook parses command, validates user, spawns `claude --print --input-format stream-json --output-format stream-json --permission-mode bypassPermissions` with cwd `~/projects/rustyclaw`
3. User sees: "Connected to Claude Code in ~/projects/rustyclaw. Messages route directly. Send /disconnect to end."
4. User sends: "Look at src/hooks/traits.rs and tell me about the HookResult type"
5. SessionBridgeHook writes JSON message to child stdin; persistent process responds on stdout
6. Response appears in Telegram (chunked if long)
7. User sends: "What if we added a Timeout variant?"
8. Same persistent process — Claude Code retains full conversation context across all messages in the session

### Disconnect

1. User sends `/disconnect`
2. SessionBridgeHook kills child process, removes binding, persists table
3. User sees: "Disconnected from Claude Code. Back to normal chat."
4. Next message routes through normal RustyClaw agent loop

### Process crash

1. Claude Code process exits unexpectedly (segfault, OOM, etc.)
2. SessionBridgeHook's process monitor detects exit via stdout EOF
3. Auto-unbinds conversation, persists table
4. User sees: "Claude Code session ended unexpectedly (exit code 137). Conversation unbound."
5. Next message routes through normal agent loop

### RustyClaw restart

1. RustyClaw daemon restarts (deploy, crash, etc.)
2. On startup, SessionBridgeHook loads binding table from disk
3. All entries are stale (child processes are gone) — marks all as dead
4. On next message from a previously-bound user, detects stale binding
5. Auto-unbinds, notifies: "Your Claude Code session ended when RustyClaw restarted. Send /connect to start a new one."

## Integration Points

This is the section that prevents the "built it but forgot to wire it in" problem we hit with the Elixir bridge.

### 1. Config schema — `src/config/schema.rs`

**What exists:** `BuiltinHooksConfig` (line 1948) has a single `command_logger: bool` field. `HooksConfig` (line 1928) wraps it with an `enabled` flag.

**What changes:** Add `session_bridge: bool` to `BuiltinHooksConfig`. Add a new `SessionBridgeConfig` struct with fields: `default_workspace` (PathBuf), `max_sessions` (usize), `output_buffer_ms` (u64), `agents` (HashMap of agent name → command + args). Add `session_bridge_config: SessionBridgeConfig` to the top-level `Config`.

**Why here:** Config-gated means users opt in. The agent command table makes it extensible to Codex later without code changes.

### 2. Hook registration — `src/channels/mod.rs:3400-3408`

**What exists:** Inside `start_channels()`, a `HookRunner` is created and `CommandLoggerHook` is conditionally registered. The runner is stored in `ChannelRuntimeContext.hooks` as `Option<Arc<HookRunner>>`.

**What changes:** After the `command_logger` registration block, add a conditional block that creates `SessionBridgeHook` and registers it. The hook needs:
- A reference to `channels_by_name` (to call `Channel::send()` for responses)
- The `SessionBridgeConfig` (for defaults, agent commands, limits)
- The `allowed_users` list from Telegram config (for auth)

**Critical detail:** `HookRunner` stores `Vec<Box<dyn HookHandler>>` and sorts by priority descending. `SessionBridgeHook` must have priority > 0 (e.g., 100) to run before any default-priority hooks. `CommandLoggerHook` uses priority -50, so there's no conflict.

### 3. Slash command handling — `src/channels/mod.rs:492-531`

**What exists:** `parse_runtime_command()` handles `/models`, `/model`, `/new` for Telegram and Discord. It's called from `handle_runtime_command_if_needed()` (line 1003) which runs *after* the hook pipeline (line 1627). `/btw` messages are routed to the Elixir orchestrator at line 1632, also after hooks.

**What changes:** Handle `/connect`, `/disconnect`, `/status` inside `SessionBridgeHook::on_message_received()` itself. The hook inspects message content, recognizes slash commands, and handles them directly. This keeps all bridge logic in one place and ensures commands are processed before `handle_runtime_command_if_needed()`.

**Critical: explicit passthrough for RustyClaw commands.** Because the hook runs before `handle_runtime_command_if_needed` and the `/btw` router, the hook's decision tree must explicitly pass through commands it doesn't own. For a bound conversation, the hook must check for these prefixes **before** routing to the child process and return `HookResult::Continue` if matched:

- `/btw ` — must reach the Elixir orchestrator at line 1632
- `/models`, `/model`, `/new` — must reach `parse_runtime_command()`
- Any future RustyClaw built-in commands

Decision tree inside `on_message_received`:

```
1. if message starts with "/btw "    → return Continue  (Elixir router)
2. if message starts with known RustyClaw command → return Continue
3. if message is "/connect ..."      → handle connect, return Cancel
4. if message is "/disconnect"       → handle disconnect, return Cancel
5. if message is "/status"           → handle status, return Cancel
6. if binding exists for this sender → route to child process, return Cancel
7. else                              → return Continue  (unbound, normal flow)
```

Steps 1–2 must come first — before any binding check — so bound users can still reach Elixir and other RustyClaw commands.

### 4. Channel access for responses — `src/channels/traits.rs`

**What exists:** `Channel::send()` takes `&SendMessage` and returns `Result<()>`. The `ChannelRuntimeContext` holds `channels_by_name: Arc<HashMap<String, Arc<dyn Channel>>>`.

**What changes:** `SessionBridgeHook` needs an `Arc<HashMap<String, Arc<dyn Channel>>>` to look up the channel by name and call `send()`. This is passed at construction time (during registration in `start_channels()`). No trait changes needed.

**Subtlety:** The hook's `on_message_received` signature takes `&self` — it cannot mutate state. The binding table and process handles must be behind interior mutability (`Arc<Mutex<...>>` or `Arc<RwLock<...>>`).

### 5. Message dispatch interaction — `src/channels/mod.rs:1610-1621`

**What exists:** After `run_on_message_received()`, if the result is `Cancel`, the dispatch function returns early (line 1614-1615). If `Continue`, processing continues to provider resolution, conversation history, agent loop, etc.

**What changes:** Nothing. This is exactly the behavior we need. When `SessionBridgeHook` returns `Cancel` for a bound message, the entire downstream pipeline is skipped. Other hooks in the pipeline that run *after* the bridge (lower priority) also never see the message — that's the `run_on_message_received` short-circuit behavior (line 258-264 in runner.rs).

**Impact on other hooks:** Any hook with priority < 100 (i.e., all current hooks) will never see messages that are bound to a session. This is intentional — bound messages bypass the entire agent pipeline. Void hooks like `on_message_sent` still fire normally when the bridge calls `Channel::send()` (they fire from the channel, not the dispatch path).

### 6. Daemon lifecycle — `src/daemon/mod.rs`

**What exists:** The daemon runs `start_channels()` via `spawn_component_supervisor("channels", ...)` (line 59). Shutdown is Ctrl+C → abort handles (line 188-193). The Elixir orchestrator gets explicit `stop()` before handle abort (line 177-180).

**What changes:** The `SessionBridgeHook` needs graceful shutdown — kill all child processes, notify bound users, persist final state.

**Implement `Drop` on the process manager.** `fire_gateway_stop` exists in `HookRunner` but is never called anywhere in the daemon shutdown sequence (confirmed: grep finds only the definition, no call sites). The actual shutdown path aborts all component handles directly — no hook dispatch. Do not rely on `on_gateway_stop`; it will never fire.

Instead: implement `Drop` on the process manager struct (the `Arc<Mutex<...>>` inner type). When the channels supervisor is aborted, the `SessionBridgeHook` and its owned process manager are dropped. The `Drop` impl calls `Child::kill()` on all live sessions. This is the only reliable cleanup path.

For best-effort user notification on shutdown: attempt a `Channel::send()` in `Drop` with a short timeout for each bound session. Accept that it may fail (network gone, runtime torn down) — persist the final table state synchronously so users see a stale-binding message on next connect.

Additionally: the component supervisor will restart `start_channels()` on crash, which rebuilds the `HookRunner` with a fresh `SessionBridgeHook`. The new hook loads persisted bindings from disk and detects stale entries.

### 7. Auth boundary — Telegram `allowed_users`

**What exists:** `TelegramChannel` has `allowed_users: Arc<RwLock<Vec<String>>>` (line 301). Messages from non-allowed users are filtered before they reach the dispatch queue.

**What changes:** The bridge reuses this — if a message reaches `SessionBridgeHook`, the sender is already authenticated by Telegram's `allowed_users` filter. No additional auth layer needed for `/connect`.

**Consideration:** If we want finer-grained control (e.g., only some allowed users can `/connect`), add a `session_bridge_users` list to config. Phase 1 can skip this — all allowed Telegram users can bridge.

### 8. Persistence path — disk state

**What exists:** `daemon_state.json` is written to the config directory (line 198-204). The workspace dir is at `~/.rustyclaw/workspace`.

**What changes:** Add `session_bridge_bindings.json` alongside `daemon_state.json` in the config directory. Contains: `{ "bindings": [{ "channel": "telegram", "sender": "12345", "agent": "claude-code", "working_dir": "/path", "bound_at": "2026-03-26T..." }] }`. On startup, load and mark all as stale. On any bind/unbind, rewrite.

## Edge Cases & Failure Modes

**Process crash:** Detected via stdout EOF or `Child::try_wait()` polling. Auto-unbind, notify user with exit code. If the process crashes repeatedly (3 times in 60s), refuse re-bind with a cooldown message.

**RustyClaw restart:** Binding table loaded from disk on startup. All entries are stale (PIDs are gone). Lazily cleaned up on next message from that user. No attempt to re-spawn — user must explicitly `/connect` again.

**Telegram disconnect/reconnect:** Telegram long-poll reconnects automatically. If a message arrives while reconnecting, it's queued by Telegram's Bot API. The binding table is unaffected.

**Concurrent sessions:** Binding is keyed on `(channel, sender)`. One user, one session per channel. Multiple users can each have independent sessions. `max_sessions` config caps total concurrent sessions across all users.

**Message ordering:** Messages arrive sequentially per sender (Telegram guarantees ordering within a chat). The bridge processes them sequentially (stdin write → stdout read → send). No reordering risk.

**Large output:** Claude Code's stream-json output streams incrementally. Buffer the full assistant response (accumulate until the response turn closes), then chunk into ≤4096-char Telegram messages. Split on paragraph boundaries where possible, fall back to hard split.

**Rate limits:** Telegram Bot API allows ~30 messages/second. Long Claude Code responses chunked into many messages could hit this. Add a 100ms delay between chunks. For responses >20 chunks (~80KB), warn the user and suggest using the terminal instead.

**stdin/stdout deadlock:** If Claude Code blocks on stdout (full buffer) while we're writing to stdin, deadlock. Prevent by using separate tokio tasks for stdin writer and stdout reader, with an mpsc channel between them.

**Working directory validation:** `/connect claude-code /etc/passwd` should fail. Validate that the path exists, is a directory, and is under an allowed root (default: `$HOME` or configured `allowed_roots`).

## Error UX

| Scenario | User sees |
|----------|-----------|
| `/connect` when already bound | "Already connected to Claude Code in ~/projects/foo. Send /disconnect first." |
| `/connect` with invalid path | "Directory not found: ~/projects/nope" |
| `/connect` when max sessions reached | "Too many active sessions (5/5). Ask another user to /disconnect or wait." |
| `/connect` with unknown agent | "Unknown agent: codex. Available: claude-code" |
| Process crash | "Claude Code session ended unexpectedly (exit code 1). Back to normal chat." |
| Process timeout (no output in 5min) | "Claude Code hasn't responded in 5 minutes. Session still active — send another message or /disconnect." |
| `/disconnect` when not bound | "No active session to disconnect." |
| `/status` when not bound | "No active session." |
| `/status` when bound | "Connected to Claude Code in ~/projects/rustyclaw (uptime: 23m). 47 messages exchanged." |
| Message while process is busy | Queue it. If >3 messages queue, warn: "Claude Code is still processing. Messages are queued." |

## Security

**Auth:** Only Telegram `allowed_users` can send messages at all. The bridge inherits this — no additional auth for Phase 1.

**Process isolation:** Claude Code runs as the same OS user as RustyClaw. `--permission-mode bypassPermissions` gives it full filesystem/shell access within that user context. This is the same trust model as running `claude` in a terminal.

**Working directory:** Validated to exist and be a directory. Optionally restricted to configured allowed roots.

**No secret leakage:** The bridge pipes message text only — no Telegram metadata, no RustyClaw config, no API keys reach the child process.

**Denial of service:** `max_sessions` caps resource usage. Each child process is a full Claude Code instance (memory, CPU). 5 concurrent sessions is a reasonable default.

## Performance

**Latency budget (message in → response visible):**
- Telegram long-poll to message receipt: ~100-500ms (Telegram infrastructure)
- Hook dispatch to SessionBridgeHook: <1ms (in-process function call)
- Binding table lookup: <1ms (HashMap)
- Write JSON message to child stdin: <1ms (persistent process, no spawn overhead)
- Claude Code processing: 2-60s (depends on task complexity, model, API latency)
- Read stdout JSON stream + buffer: <100ms
- Channel::send() to Telegram: ~100-300ms
- **Total overhead added by bridge: <10ms** (dominated by Claude Code processing time; no per-message spawn cost with persistent process)

**Resource usage per session:**
- One persistent OS process (Claude Code CLI in stream-json mode)
- ~50-200MB RSS (Claude Code + Node.js runtime)
- One stdin pipe, one stdout pipe, one stderr pipe
- One entry in binding table (~200 bytes)

**At 5 max sessions:** ~1GB RSS worst case. Acceptable for a single-user server.

## Resolved Questions

1. **Persistent session mode confirmed.** `--print --input-format stream-json --output-format stream-json` keeps a long-running process with streaming JSON I/O over stdin/stdout, maintaining full conversation context across messages. One process per `/connect`, lives until `/disconnect` or crash. This is the primary mode.

2. **Conversation context fallback.** `--resume <session-id>` is available for per-message spawn with session continuity. Use as fallback if stream-json has issues. The binding table stores the session ID returned by the first invocation, and subsequent messages pass `--resume <session-id>`.

3. **`on_gateway_stop` is dead.** `fire_gateway_stop` is defined but never called in the daemon shutdown path. Drop-based cleanup on the process manager is the only viable approach. (See Integration Point 6.)

4. **`/btw` passthrough for bound users.** The hook must check `/btw` prefix before routing bound messages and return `HookResult::Continue` so `/btw` reaches the Elixir router. Same for all other RustyClaw built-in commands. (See Integration Point 3.)

## Open Questions

1. **Stderr handling.** Claude Code writes progress/status to stderr. Do we surface this to the user? Options: ignore, forward as italicized status, or only forward on error.

2. **Binary/image output.** If Claude Code generates files (images, binaries), how do we handle them? Phase 1: ignore non-text output. Phase 2: upload files to Telegram.

3. **Multi-channel generalization.** The architecture is channel-agnostic, but slash command parsing is Telegram/Discord-specific today. When extending to Discord, do we add `/connect` to the existing `parse_runtime_command` for Discord too, or keep it hook-internal?
