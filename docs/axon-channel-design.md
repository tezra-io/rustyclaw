# Axon Channel — Design Doc

## Problem

RustyClaw has 20 messaging channels (Telegram, Signal, Discord, etc.) but no way to communicate with other AI agents on the same machine. Axon is a local agent mesh protocol (UDS transport, Ed25519 identity, JSON wire format) running on Sujeeth's Mac mini with a 24/7 broker. OpenClaw (Aira) is already on the mesh. RustyClaw (Rusty) needs a native Axon channel so agents can talk to each other without relays, hacks, or shell exec workarounds.

The goal: Axon becomes the standard inter-agent protocol. Any agent that implements an Axon channel joins the mesh and talks natively.

## Tech Stack

- **Language:** Rust (same as all RustyClaw channels)
- **Transport:** Unix Domain Socket at `~/.axon/broker.sock`
- **Wire format:** JSON newline-delimited (Axon `Envelope` v1)
- **Auth:** Ed25519 challenge-response (keypair at `~/.axon/keys/{name}.key` / `{name}.pub`)
- **Dependencies:** `tokio` (async UDS), `serde_json` (envelope), `ed25519-dalek` or `ring` (signing) — check what Axon already uses and match it
- **No new external dependencies** beyond what RustyClaw + Axon already have

## Approach

Implement `axon.rs` as a new channel in `src/channels/`, following the same `Channel` trait pattern as Signal, Telegram, etc. The channel connects to the Axon broker over UDS, authenticates with Rusty's keypair, and translates between Axon `Envelope` messages and RustyClaw `ChannelMessage`/`SendMessage` types.

### Why this approach over alternatives

| Approach | Pros | Cons |
|----------|------|------|
| **Native channel (chosen)** | First-class citizen, same as Telegram/Signal. Full trait compliance. Agent loop handles messages natively. | Needs Rust implementation of Axon client protocol |
| CLI exec relay | Zero code in RustyClaw | Fragile, latency, no streaming, process overhead per message |
| HTTP bridge | Could reuse Axon dashboard REST API | Extra hop, not real-time, dashboard API is read-only |
| Elixir-side integration | Could bypass Rust entirely | Wrong layer — channels are Rust-owned per architecture |

## Scope

**In scope:**
- `src/channels/axon.rs` implementing `Channel` trait (`name()`, `send()`, `listen()`)
- Config section in `config.toml` for Axon channel settings
- UDS connection to broker with auto-reconnect
- Ed25519 challenge-response authentication
- Mapping Axon `Envelope` → `ChannelMessage` (inbound)
- Mapping `SendMessage` → Axon `Envelope` (outbound)
- Thread support via Axon `thread_id`
- Registration in channel dispatcher (`mod.rs`)
- Integration tests

**Out of scope:**
- Axon group/pub-sub (future enhancement)
- Axon broker changes (broker is stable, no modifications needed)
- OpenClaw Axon channel (separate project, but same protocol)
- Axon SDK crate extraction (future — for now, implement protocol inline)
- Encryption at rest or in transit beyond auth (UDS is local-only)

## Data Flow

```
Inbound (another agent → Rusty):

  Agent (e.g. Aira)
    │
    │ axon send --to rusty "hey"
    │
    ▼
  Axon Broker (UDS)
    │
    │ Envelope { type: "send", to: "rusty", body: { text: "hey" } }
    │
    ▼
  AxonChannel::listen()
    │
    │ Parse Envelope → ChannelMessage {
    │   id: envelope.id,
    │   sender: envelope.from (resolved name),
    │   reply_target: envelope.from,
    │   content: body.text,
    │   channel: "axon",
    │   timestamp: envelope.ts,
    │   thread_ts: envelope.thread_id,
    │ }
    │
    ▼
  RustyClaw agent loop (same as any channel message)
    │
    ▼
  Response generated


Outbound (Rusty → another agent):

  RustyClaw agent loop
    │
    │ SendMessage { content: "sup", recipient: "aira" }
    │
    ▼
  AxonChannel::send()
    │
    │ Envelope {
    │   v: 1,
    │   type: "send",
    │   to: "aira",
    │   body: { text: "sup" },
    │   thread_id: send_message.thread_ts,
    │ }
    │
    ▼
  Axon Broker (UDS) → delivers to Aira's listener
```

## User Scenarios

### 1. Aira sends Rusty a message
Aira (OpenClaw) runs `axon send --to rusty "what's the status of the Elixir migration?"`. The broker delivers the envelope to Rusty's listener. `AxonChannel::listen()` receives it, maps to `ChannelMessage`, and feeds it into the agent loop. Rusty processes it, generates a response, and `AxonChannel::send()` sends the reply envelope back to Aira.

### 2. Rusty proactively messages Aira
During a build session, Rusty hits a blocker. The agent decides to ask Aira for help. It sends via the Axon channel with `recipient: "aira"`. Aira's listener picks it up. (Note: Aira's side doesn't have a native channel yet — she'd see it in her Axon listener log. Full bidirectional chat requires both sides to have the channel.)

### 3. Agent discovery
On startup, the Axon channel can optionally run a discover to see who's online. This is informational — it doesn't gate message sending (messages to offline agents get `recipient_disconnected` errors which the channel should handle gracefully).

## Agent Scenarios

### How Rusty discovers Axon
The channel is configured in `config.toml`. On daemon startup, if `[channels.axon]` is present and `enabled = true`, the channel connects to the broker and authenticates. No manual intervention needed.

### How Rusty uses Axon autonomously
The agent loop treats Axon messages like any other channel. Rusty can be instructed (via SOUL.md or workspace context) to check Axon, respond to peer agents, or proactively reach out. The channel itself is passive — it just delivers messages in and out.

## Config

```toml
[channels.axon]
enabled = true
identity = "rusty"                          # name to register as
broker_socket = "~/.axon/broker.sock"       # UDS path
keys_dir = "~/.axon/keys"                   # Ed25519 keypair directory
reconnect_initial_delay_ms = 1000             # exponential backoff: 1s → 2s → 4s → ... → cap 30s
# Optional filters:
# allowed_from = ["aira", "codex"]          # only accept messages from these agents
# groups = ["dev-team"]                     # subscribe to these Axon groups (future)
```

## Socket Architecture

The core challenge: `listen()` is a long-running reader on the UDS socket, but `send()` also needs to write AND read delivery acks from the same socket concurrently.

**Solution: Split ownership with delivery ack routing**

```
                    ┌─────────────────────────┐
                    │   UDS Connection         │
                    │  (tokio::io::split)      │
                    ├────────────┬─────────────┤
                    │ ReadHalf   │ WriteHalf   │
                    └─────┬──────┴──────┬──────┘
                          │             │
                          ▼             │
                 ┌────────────────┐     │
                 │ Reader Task    │     │
                 │ (owns ReadHalf)│     │
                 │                │     │
                 │ Routes:        │     │
                 │  "send" msgs   │     │
                 │   → tx channel │     │
                 │  "ping" msgs   ├─────┤ (writes pong via
                 │   → auto-pong  │     │  Arc<Mutex<WriteHalf>>)
                 │  "delivery_*"  │     │
                 │   → pending map│     │
                 └────────────────┘     │
                                        │
                 ┌────────────────┐     │
                 │ send()         ├─────┘
                 │ (Arc<Mutex<    │
                 │  WriteHalf>>)  │
                 │                │
                 │ 1. Write envelope
                 │ 2. Wait on     │
                 │    oneshot rx   │
                 │    from pending │
                 │    map (keyed   │
                 │    by msg id)   │
                 └────────────────┘
```

**How it works:**
1. On connect, `tokio::io::split()` the stream into `ReadHalf` + `WriteHalf`
2. `WriteHalf` wrapped in `Arc<Mutex<OwnedWriteHalf>>` — shared between `send()` and the reader task (for pong responses)
3. Reader task owns `ReadHalf` exclusively. It routes messages by type:
   - `"send"` → feeds into the `tx` mpsc channel (becomes `ChannelMessage`)
   - `"ping"` → immediately writes `"pong"` envelope via the shared writer
   - `"delivery_ack"` / `"delivery_nack"` / `"error"` → looks up `envelope.body.in_response_to` in a `DashMap<String, oneshot::Sender>` and resolves the pending send
4. `send()` inserts a `oneshot::channel()` into the pending map keyed by message ID, writes the envelope, then `await`s the oneshot receiver for the ack/nack
5. If oneshot times out (5s), treat as delivery failure

**Pending sends map:** `Arc<DashMap<String, oneshot::Sender<Envelope>>>` — shared between the reader task and `send()`. Lock-free concurrent map.

## Connection Lifecycle

1. **Connect:** Open UDS to `broker_socket`
2. **Handshake:** Receive `challenge` envelope with nonce → sign nonce with `{identity}.key` → send `auth` envelope with signature + public key
3. **Register:** Send `register` envelope with name, agent_type (`"rustyclaw"`), capabilities (`[]`), directory (workspace path), and max_message_size (65536)
4. **Split:** `tokio::io::split()` → reader task + shared writer
5. **Listen:** Reader task reads JSON lines from socket. Routes `send` → tx channel, `ping` → auto-pong, `delivery_*` → pending map
6. **Reconnect:** On socket error/EOF, exponential backoff: 1s → 2s → 4s → 8s → cap at 30s. Reset on successful connection. Log but don't crash.

## Health Check

`health_check()` returns the actual connection state via an `AtomicBool`. Set to `true` on successful registration, `false` on socket error/disconnect. The reconnect loop flips it back when re-registered. This feeds into RustyClaw's daemon health reporting.

## SendMessage Field Mapping

| SendMessage field | Axon mapping | Notes |
|-------------------|-------------|-------|
| `content` | `body.text` | Primary message content |
| `recipient` | `to` | Agent name on the mesh |
| `thread_ts` | `thread_id` | Thread continuity |
| `subject` | *(dropped)* | Axon has no subject concept |
| `quote_reply_id` | *(dropped)* | Axon has no quote-reply concept |

## Envelope Body Contract

For `type: "send"` messages, the body schema is: `{ "text": "<content>" }`. This matches the Axon CLI convention (`send_cmd.rs`) and is the de facto contract between agents. The `from` field is broker-populated — the channel must NOT set it on outbound envelopes.

## Edge Cases & Failure Modes

| Scenario | Behavior |
|----------|----------|
| Broker not running | Connect fails → log warning, retry on interval. Channel is degraded but doesn't crash the daemon. |
| Broker restarts | Socket EOF → reconnect loop picks it up. Messages during downtime are lost (Axon is fire-and-forget for real-time). |
| Recipient offline | Broker returns `delivery_failed` with `recipient_disconnected`. Channel logs it, returns error from `send()`. Agent can handle gracefully. |
| Malformed envelope | Skip and log. Don't crash the listener. |
| Identity key missing | Channel refuses to start. Log error with clear message: "Axon identity '{name}' not found at {keys_dir}/{name}.key — run `axon identity create {name}` to create it." |
| Duplicate identity | Broker rejects registration. Log error — another process is already connected as this identity. |
| Message too large | Axon has no built-in size limit, but cap at 64KB in the channel to prevent memory issues. Log and drop oversized messages. |
| Rapid reconnect loop | Exponential backoff (see Connection Lifecycle). Reset on successful connection. |

## Error UX

| Error | What the agent sees |
|-------|-------------------|
| Broker unreachable | `[axon] broker not available at ~/.axon/broker.sock — retrying in {N}s` (log only, no user-facing error unless agent tries to send) |
| Send to offline agent | `send()` returns `Err("recipient 'aira' is offline")` — agent can decide to retry, queue, or skip |
| Auth failure | `[axon] authentication failed for identity 'rusty' — check keypair at ~/.axon/keys/rusty.key` |
| Unknown message type | `[axon] ignoring unknown message type: '{type}'` (log, don't error) |

## Security & Privacy

- **Transport:** UDS only — no network exposure. Communication stays on the local machine.
- **Auth:** Ed25519 challenge-response prevents identity spoofing. Each agent has a unique keypair.
- **Trust model:** All agents on the mesh are trusted peers (they're all on the same machine, owned by the same user). No message encryption beyond OS-level UDS permissions.
- **File permissions:** Keypairs at `~/.axon/keys/` should be `0600`. Broker socket at `0700`.
- **No PII concerns:** Messages are ephemeral (broker stores history in SQLite for retrieval but no long-term retention policy). Agents can exchange whatever they need.
- **allowed_from filter:** Optional config to restrict which agents can message this channel. Defense in depth if new agents join the mesh.

## Performance

- **Latency:** UDS round-trip is sub-millisecond. Total message delivery (send → broker → receive → parse) should be < 5ms P99.
- **Throughput:** Not a concern — agent-to-agent chat is low volume (dozens of messages per session, not thousands).
- **Memory:** One persistent UDS connection + small read buffer. Negligible compared to LLM inference.
- **No polling:** The listener reads from the socket stream directly. Zero CPU when idle.

## Testing Strategy

**Unit tests (no broker needed):**
- `envelope_to_channel_message()` — pure function: given an Envelope, returns ChannelMessage. Test all field mappings, missing fields, malformed body.
- `send_message_to_envelope()` — pure function: given a SendMessage, returns Envelope. Test content, recipient, thread_id, dropped fields.
- Auth challenge-response: given a mock nonce + keypair, verify signature output matches.
- Reconnect backoff calculation: verify exponential sequence and cap.
- `allowed_from` filter: test case normalization, empty list (allow all), exact match.

**Integration tests (require broker):**
- Happy path: connect → auth → register → send → receive → ack
- Round-trip: two test clients, A sends to B, B receives
- Reconnect: kill broker, restart, verify client reconnects and re-authenticates
- Ping/pong: verify client stays connected over a duration that would trigger broker disconnect

**Keep unit:integration ratio at ~4:1.** All mapping logic must be testable without a socket.

## Open Questions

1. **Should the Axon channel support Axon groups (pub/sub)?** For now, scoped to direct agent-to-agent messaging only. Groups can be added later as a config option.
2. **Message queuing for offline recipients?** Currently fire-and-forget. Could add a retry queue in the channel, but adds complexity. Defer unless there's a real need.
3. **OpenClaw side:** OpenClaw doesn't have a pluggable channel system like RustyClaw. The integration there would likely be a heartbeat-based poll or a dedicated exec-based listener. Separate design needed.
4. **Axon SDK crate:** As more Rust agents adopt Axon, the client protocol (connect, auth, send, listen) should be extracted into a reusable `axon-client` crate. Not blocking this work but worth tracking.
