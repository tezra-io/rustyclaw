# Session Bridge Design Review

Reviewed by Claude (claude-sonnet-4-6), 2026-03-27.

---

## Integration Claim Verification

Four specific claims in the design doc, checked against source:

### 1. HookRunner short-circuits on Cancel ✅ CONFIRMED

`run_on_message_received` (runner.rs:248–275) iterates handlers sequentially and
`return HookResult::Cancel(reason)` at line 264 exits immediately — subsequent
handlers never run. Behavior is consistent across all modifying dispatchers.
Tests in runner.rs cover this (line 447: `modifying_hook_can_cancel`).

### 2. `on_message_received` fires before `handle_runtime_command_if_needed` ✅ CONFIRMED

channels/mod.rs:1610–1627 ordering is exactly as claimed:
- Hook pipeline runs at 1611–1621; Cancel → `return` at 1615
- `handle_runtime_command_if_needed` at 1627 is only reached if hooks Continue

### 3. Daemon calls `fire_gateway_stop` on shutdown ❌ FALSE — BLOCKER

`fire_gateway_stop` exists in runner.rs:51–54 but is **never called anywhere in
the codebase**. The grep confirms it — only the definition, no call sites.

The actual daemon shutdown path (daemon/mod.rs:177–193):
1. Stops Elixir orchestrator explicitly
2. Cleans up bridge socket file
3. Calls `handle.abort()` on all component JoinHandles — no hook dispatch

The `HookRunner` lives inside the channels component which gets aborted, not
gracefully stopped. **`on_gateway_stop` is a dead void hook.** The design doc
describes Option A ("Add `on_gateway_stop` implementation — fires on shutdown")
as a viable choice and says it's "already dispatched." It isn't.

Option B (implement `Drop` on the process manager) is the correct path. The
design already recommends it — but the framing of Option A as equivalent is
wrong and could mislead an implementer into Option A, which will silently fail
(children never killed on daemon shutdown, bindings never persisted).

### 4. `BuiltinHooksConfig` is the right place for config ✅ CONFIRMED

schema.rs:1947–1951 is exactly where `command_logger: bool` lives. Adding
`session_bridge: bool` there is the right pattern. The registration site
(channels/mod.rs:3400–3408) mirrors the pattern with a conditional block.

---

## Blockers (must fix before building)

### B1: `--print` mode is one-shot — no conversation memory across messages

This is Open Question #1 in the design doc, marked as "the biggest design risk."
It is a blocker, not a risk to manage later.

`claude --print` runs one prompt and exits. The design's user scenario shows:
> "What if we added a Timeout variant?" — Claude Code has its own conversation context via `--print`

This is incorrect. Each message spawns a **fresh process with zero context**.
Claude Code has no idea what "it" or "Timeout variant" refers to.

The primary stated use case — "exploratory development sessions... reading code
together, steering direction" — requires multi-turn context. Without it, the
bridge is only useful for one-shot questions, which is a much weaker product.

**Mitigation:** Claude Code supports `--resume <session-id>` which, combined
with `--print`, preserves conversation context across invocations. The
implementation would need to:
- Capture the session ID after the first invocation (from `~/.claude/projects/`
  conversation metadata or from `--output-format json` if it includes session ID)
- Pass `--resume <session-id>` on subsequent messages in the same binding

This must be investigated before committing to the `--print` process model.
If `--resume` doesn't work with `--print` in a way that preserves context,
the session-per-connection model (long-running stdio process) must be used
instead. The architecture supports either, but the binding table structure and
process lifecycle code differ significantly between them.

**Action:** Before implementation, run:
```sh
# First message
claude --print "What is 2+2?"
# Find the session ID in ~/.claude/projects/...
# Second message using resume
claude --print --resume <session-id> "What did I just ask you?"
```
If the second response demonstrates memory of the first, `--resume` works.
If not, the design must pivot to a long-running stdio process.

### B2: `on_gateway_stop` is never dispatched — Option A cleanup won't work

Covered in claim #3 above. The design must commit to Option B (Drop-based
cleanup) and remove any suggestion that Option A is viable. If Option A is
accidentally chosen during implementation, child processes will be orphaned on
every daemon shutdown/restart, accumulating over time.

### B3: `/btw` interaction is inverted for bound users

The design says: "Recommendation: `/btw` always goes to Elixir."

The actual code flow for a bound user sending `/btw foo`:
1. `run_on_message_received` fires (line 1611) — hook sees bound conversation
2. Hook routes to Claude Code, returns `HookResult::Cancel`
3. Control returns at line 1615 — everything after (including `/btw` check at
   line 1632) never executes

Result: `/btw` goes to Claude Code, not Elixir, for bound users. This is the
opposite of the stated recommendation.

**Fix:** The hook must explicitly check for `/btw` prefix before routing bound
messages, and return `HookResult::Continue` for `/btw` so it falls through to
the Elixir router at line 1632. Add this to the hook's decision tree:

```
if message starts with "/btw " → return Continue (regardless of binding)
if binding exists → route to child process, return Cancel
...
```

---

## Recommendations (should fix)

### R1: `/connect` must always return Cancel, never Continue

The design correctly recommends Option A (handle slash commands inside the hook).
But the consequence needs to be explicit: `/connect`, `/disconnect`, `/status`
must be consumed by the hook (Cancel) under ALL circumstances — valid args,
invalid args, already-bound, everything. If the hook returns Continue for an
invalid `/connect` (e.g., bad path), the message falls through to
`handle_runtime_command_if_needed` and then the agent loop, which will try to
process it as a chat message. UX-wise this is a confusing failure mode.

### R2: Construction ordering — `channels_by_name` availability

The design says the hook gets `Arc<HashMap<String, Arc<dyn Channel>>>` "at
construction time during registration in start_channels()". This is correct,
but the hook runner is built at lines 3400–3408, before the `ChannelRuntimeContext`
is returned from `start_channels`. The `channels_by_name` map must be captured
from `start_channels`'s local scope and passed into the hook constructor
*before* it's placed into the `Arc<HookRunner>`. This is straightforward but
not explicitly called out — easy to miss when implementing.

### R3: `auto_save_memory` is benign but worth noting

Lines 1666–1677 auto-save messages to memory that pass through the hook
pipeline. Since bound messages return Cancel at 1615, they are never saved to
conversation memory. This is the right behavior (bridge sessions are external),
but it means bridge conversations leave no trace in RustyClaw's memory store.
If you ever want replay or search of bridge sessions, you'd need explicit
logging inside the hook. Note this as intentional in the implementation.

### R4: Clarify the binding key type early

The design uses `(channel, sender)` as the binding key. `ChannelMessage` has
`channel: String` and `sender: String`. For Telegram, `sender` is the user ID
(numeric string). This is stable and collision-safe. Document the key type
explicitly in the data structures — it affects the JSON persistence format and
any future multi-channel extension.

### R5: The process monitor must be async, not poll-based

The design mentions "detected via stdout EOF or `Child::try_wait()` polling."
`try_wait` polling requires a timer or a background task. The cleaner approach:
the stdout reader task will naturally terminate on EOF, which can be used as the
process exit signal (the reader task drops and notifies the binding table via a
channel). This avoids any periodic polling. Design the reader task as the
process lifecycle monitor — its completion = process exited.

---

## Notes (context for builder)

### N1: The hook trait requires `on_message_received` to be `async fn(&self, ...)`

`HookHandler::on_message_received` takes `&self` (immutable). All mutable state
(binding table, process handles, message queues) must be behind `Arc<Mutex<...>>`
or `Arc<RwLock<...>>`. The binding table is read-heavy (every message checks it)
and write-rare (only on /connect, /disconnect, crash), so `RwLock` is
appropriate for the binding table itself. The per-session stdin writer should be
a separate `Mutex<ChildStdin>` stored inside the session entry.

### N2: `HookRunner` is behind `Arc` — the hook itself must be `Send + Sync`

`HookHandler: Send + Sync` (traits.rs:25). `SessionBridgeHook` fields must all
be `Send + Sync`. `Arc<RwLock<...>>` satisfies this; bare `Mutex` does too.
`ChildStdin` is `Send` but not `Sync` — wrap per-session stdin in `Mutex`.
`Child` itself is `Send` — process handles in the binding table are fine.

### N3: `on_message_sending` fires when Channel::send() is called from the hook

Void hook `fire_message_sent` is called from the channel's send path. If
`CommandLoggerHook` or any future hook is registered, it will see responses
sent by `SessionBridgeHook`. This is benign for logging but worth knowing —
outbound bridge responses will appear in observability pipelines alongside
normal agent responses. No action needed; just awareness.

### N4: The `panic recovery` in HookRunner is a hidden risk for bridge state

If `SessionBridgeHook::on_message_received` panics mid-message (e.g., after
writing to stdin but before reading stdout), the runner's catch_unwind at
runner.rs:254–256 swallows the panic and continues with the previous message
value. The child process is now in an unknown state — it may have received input
and be generating a response that nobody will read. The stdin/stdout deadlock
prevention (separate tasks) described in the design mitigates this, but the
binding table could be inconsistent (message sent to child, no response
delivered). Consider making the hook crash-safe: if the panic recovery fires,
auto-unbind that session and notify the user.

### N5: Config schema needs a top-level `[session_bridge]` section too

The design says to add `session_bridge: bool` to `BuiltinHooksConfig` AND add
`SessionBridgeConfig` to top-level `Config`. These are two separate changes:
one gates the feature, the other provides configuration. The registration site
(channels/mod.rs:3400–3408) will need both — the bool to decide whether to
register, and the config struct to pass to the hook constructor. Don't
conflate them; both are needed.

### N6: Persistence format is append-only on every bind/unbind

The design writes the full binding table on every state change. For Phase 1
(max 5 sessions), this is a full rewrite of a tiny file. Acceptable. If max
sessions ever grows significantly, switch to a write-ahead log or SQLite.
For now, keep it simple: serialize/deserialize the whole table on each change.
