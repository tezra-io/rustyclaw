# Session Bridge: Telegram ↔ Claude Code

## Problem

Design and brainstorming sessions need real-time back-and-forth with Claude Code. Current cron/subagent workflow is great for well-defined tasks but wrong for exploratory work where you need to guide, redirect, and iterate quickly.

Today: open laptop → terminal → `claude`. Goal: do it from Telegram on your phone.

## Solution

Bake a session bridge into RustyClaw's hook system. Slash commands to bind/unbind a Telegram conversation to a Claude Code process. When bound, messages route directly — no RustyClaw LLM in the middle.

## UX

```
You: /connect claude-code ~/projects/rustyclaw
Rusty: Connected to Claude Code in ~/projects/rustyclaw. Messages route directly. /disconnect to end.

You: Look at the hooks system and tell me what's missing for a plugin architecture
Claude Code: [reads files, responds with analysis]

You: What about using JSON-RPC over stdio?
Claude Code: [discusses approach]

You: /disconnect
Rusty: Disconnected from Claude Code. Back to normal.
```

## Architecture

```
Telegram → RustyClaw Router
                ↓
         Binding Table lookup
                ↓
    ┌── Unbound → Agent Loop (normal chat with RustyClaw)
    └── Bound   → Session Bridge Hook
                       ↓
                  Child Process (claude CLI)
                       ↓
                  stdin/stdout pipe
                       ↓
                  Response → Telegram
```

### Components

1. **Binding Table** (`HashMap<ConversationId, SessionBinding>`)
   - Maps Telegram chat → active Claude Code session
   - In-memory, persisted to disk for restart survival
   - One binding per conversation (can't bind twice)

2. **Session Bridge Hook** (`src/hooks/builtin/session_bridge.rs`)
   - Implements `HookHandler`
   - Intercepts `on_message_received` — if conversation is bound, route to plugin process
   - Priority: high (runs before agent loop consumes the message)
   - Returns `HookResult::Cancel` for bound messages (prevents agent loop from seeing them)

3. **Process Manager**
   - Spawns `claude --print --permission-mode bypassPermissions` as child process
   - Working directory set to the requested project path
   - Reads stdout line by line, sends to Telegram
   - Writes Telegram messages to stdin
   - Handles process exit gracefully (auto-unbind, notify user)

4. **Slash Commands** (registered on Telegram channel)
   - `/connect <agent> [path]` — spawn process, bind conversation
     - `agent`: `claude-code` (initially, extensible later to `codex`, etc.)
     - `path`: working directory (defaults to configured default)
   - `/disconnect` — kill process, unbind conversation
   - `/status` — show current binding info (agent, path, uptime)

### Message Flow (Bound)

1. Telegram message arrives at RustyClaw
2. Router checks binding table → found
3. `on_message_received` hook fires → `SessionBridgeHook` intercepts
4. Hook writes message text to child process stdin
5. Child process (Claude Code) processes, writes response to stdout
6. Hook reads stdout, sends to Telegram via channel
7. Returns `HookResult::Cancel` (message never reaches agent loop)

### Message Flow (Unbound)

1. Telegram message arrives at RustyClaw
2. Router checks binding table → not found
3. `on_message_received` hook fires → `SessionBridgeHook` passes through
4. Returns `HookResult::Continue` → normal agent loop handles it

## Edge Cases

- **Process crashes**: Detect exit, auto-unbind, notify user
- **RustyClaw restart**: Reload binding table from disk, but child processes are gone — auto-unbind stale entries, notify user
- **Multiple chats**: Each Telegram chat can bind to a different session independently
- **Long output**: Claude Code can output large responses — chunk into multiple Telegram messages (4096 char limit)
- **Streaming**: Buffer stdout, send on newlines or after a short delay (avoid flooding Telegram)
- **Tool approvals**: In `--permission-mode bypassPermissions` mode, no approvals needed. If we add a mode with approvals later, pipe the prompt to Telegram and wait for response.

## Config

```toml
[session_bridge]
default_workspace = "~/projects"
max_sessions = 5
output_buffer_ms = 500  # Buffer stdout before sending to Telegram
max_message_length = 4096  # Telegram limit

[session_bridge.agents.claude-code]
command = "claude"
args = ["--print", "--permission-mode", "bypassPermissions"]
```

## Future Extensions (not in scope now)

- Codex support (`codex` CLI as another agent type)
- Interactive buttons (approve/deny, model switch)
- Natural language binding ("start a Claude Code session")
- Session resume (reconnect to a persistent Claude Code session)
- Axon integration (bridge becomes an Axon agent)
- Real plugin system (extract into JSON-RPC plugin)

## Implementation Plan

### Phase 1: Core bridge (MVP)
- Binding table (in-memory + disk persistence)
- `/connect claude-code [path]` and `/disconnect` commands
- Child process spawn/kill
- Message routing (Telegram ↔ stdin/stdout)
- Output buffering and chunking
- Auto-unbind on process exit

### Phase 2: Polish
- `/status` command
- Streaming output (partial responses)
- Error handling and user-friendly messages
- Multiple concurrent sessions

### Phase 3: Multi-agent
- Codex support
- Agent-agnostic config
- Session handoff (switch agents mid-conversation)
