# Axon Channel Design Review

**Document reviewed:** `docs/axon-channel-design.md`  
**Reviewer:** Aira (subagent)  
**Date:** 2026-03-28  
**Reference files read:** `traits.rs`, `signal.rs`, `envelope.rs`, `listen_cmd.rs`, `send_cmd.rs`

---

## Summary

Solid conceptual foundation. The problem statement is clear, the alternatives table is honest, and the scope is appropriately bounded. However, there are **4 blockers** — two are protocol correctness issues that would produce broken behavior at runtime, one is a wrong socket path, and one is an unresolved architectural question about concurrent socket access. Fix these before implementation starts.

---

## Findings

### 🔴 Blockers

---

**[B1] `send()` doesn't wait for delivery acknowledgment**

*Axis: Protocol accuracy, Requirements*

The design's outbound data flow shows `send()` writing an envelope to the broker and returning. That's wrong. Per `send_cmd.rs`, every `send` envelope requires reading a response envelope before the call is complete:

```
// send_cmd.rs — after writing the send envelope:
loop {
    match client::read_envelope(&mut reader, ...).await? {
        "delivery_ack"  => { /* success */ break; }
        "delivery_nack" => { /* error */   break; }
        "error"         => { /* error */   break; }
        _               => { /* informational, keep waiting */ }
    }
}
```

If `send()` returns without consuming the `delivery_ack`/`delivery_nack`, the response will sit unread on the socket and corrupt the next read in `listen()`. The `send()` implementation must read until it gets one of these three terminal message types.

This also means `send()` and `listen()` share the same read path — which directly causes **B4**.

---

**[B2] `listen()` must respond to broker `ping` with `pong` — design omits this**

*Axis: Protocol accuracy*

`listen_cmd.rs` explicitly handles this:

```rust
if envelope.msg_type == "ping" {
    let pong = Envelope::new("pong", json!({}));
    client::write_envelope(&mut writer, &pong, config.max_message_size).await?;
    continue;
}
```

If `AxonChannel::listen()` doesn't handle `ping`, the broker will detect the client as unresponsive and disconnect it. This will trigger the reconnect loop, which will reconnect, get another ping, miss it again, and loop forever. The design needs to add ping/pong handling to the listen loop and document it explicitly.

---

**[B3] Wrong broker socket path**

*Axis: Protocol accuracy, Tech stack*

The design specifies:
```toml
broker_socket = "~/.axon/broker.sock"
```

The actual Axon transport path (per TOOLS.md and `Config::from_env()` conventions) is:
```
$TMPDIR/axon-$UID/broker.sock
```

These paths are fundamentally different. `~/.axon/` is the **keys directory** — not the socket location. Using the wrong path means the channel will never connect. The config default needs to be corrected. Consider using `$TMPDIR/axon-<uid>/broker.sock` as the default and document that it matches what `axon broker start` creates.

---

**[B4] Concurrent socket access between `send()` and `listen()` — no solution proposed**

*Axis: Architecture, Risk*

`listen()` is a long-running async task that holds a persistent read handle to the UDS socket. `send()` will be called concurrently by the agent loop while `listen()` is running. They need to share the same authenticated socket connection — but the design doesn't address how.

This is the hardest architectural decision in the entire implementation and it's unresolved. Options:

| Approach | Trade-off |
|----------|-----------|
| `Arc<Mutex<OwnedWriteHalf>>` (write half only) | Clean — split with `tokio::io::split`, give writer to send(), reader to listen() |
| Internal mpsc channel — send() drops messages into a sender, background writer task owns the socket | Adds a task boundary, cleaner separation |
| `Arc<Mutex<FullStream>>` — both read and write lock | Deadlock-prone, blocks listen() during sends |

The recommended approach is `tokio::io::split` — give `listen()` the read half and keep the write half in `Arc<Mutex<OwnedWriteHalf>>` accessible to `send()`. But note that `send()` (per B1) also needs to *read* the delivery ack — which means the read half can't be fully owned by `listen()`. This is the real complexity: `send()` needs to temporarily "claim" reads until it gets its ack, while `listen()` is waiting on the same socket.

A request-response multiplexer (keyed on `envelope.id`) is the robust solution, but that's significant scope. A simpler workaround: route delivery acks through listen(), which forwards them to a pending-send map keyed by `envelope.id`. This should be designed explicitly before implementation begins.

---

### 🟡 Recommendations

---

**[R1] `health_check()` should be implemented, not defaulted**

*Axis: Requirements*

The default `health_check()` returns `true` unconditionally. For a channel built on a persistent socket, that's misleading — the daemon could be healthy while the Axon channel is in a reconnect loop. Implement `health_check()` to return the actual connection state (e.g., a `AtomicBool connected` flag set by the reconnect loop). This is used by RustyClaw's health reporting and will give false signals if left as default.

---

**[R2] `client::register()` has undocumented required parameters**

*Axis: Protocol accuracy, Requirements*

`listen_cmd.rs` shows `client::register()` takes more parameters than the design accounts for:

```rust
client::register(
    &mut stream,
    config,
    name,          // identity name
    "axon-cli",    // agent_type — what should RustyClaw pass here?
    capabilities,  // Vec<serde_json::Value> — what capabilities should Rusty advertise?
    config.max_message_size,  // needed for read buffer sizing
    Some(directory),          // directory — what is this? Not mentioned in design.
)
```

The design needs to:
1. Decide what `agent_type` string to use (e.g., `"rustyclaw"`)
2. Decide what capabilities to advertise (empty vec is fine to start, but document it)
3. Understand and document the `directory` parameter purpose
4. Clarify how `max_message_size` flows into the channel (likely from config)

---

**[R3] Testability plan is thin — unit tests need no broker**

*Axis: Testability*

The design says "integration tests" and stops there. Signal's test suite is instructive — it tests all envelope parsing logic as pure unit tests with no HTTP calls. The Axon channel should follow the same pattern:

- `envelope_to_channel_message()` should be a pure function, unit testable
- `channel_message_to_envelope()` should be a pure function, unit testable
- Auth challenge-response signing can be tested against a mock nonce without a socket
- Reconnect backoff logic can be tested with a mock connector

A test that requires a running broker is a CI blocker. Write unit tests for all the mapping and logic; keep integration tests to "happy path connect + round-trip" only. The design should call this out explicitly.

---

**[R4] Reconnect backoff is inconsistently specified**

*Axis: Architecture*

The edge case table says `reconnect_interval_ms = 1000` (fixed from config), but the edge case for rapid reconnects says "1s → 2s → 4s → 8s → cap at 30s" (exponential). These conflict. Pick one and be explicit. Recommend exponential (as in Signal's listen loop) — fixed interval will hammer the broker on a hard failure. Also: the config field name `reconnect_interval_ms` implies fixed interval; rename to `reconnect_initial_delay_ms` if exponential.

---

**[R5] `SendMessage` fields silently dropped — document the decision**

*Axis: Requirements*

`SendMessage` has `subject` and `quote_reply_id` fields that have no Axon equivalent. That's fine — Axon doesn't have subjects or quote-replies. But the design should explicitly note these are dropped with a comment in the implementation. Without documentation, future maintainers will wonder if `subject` was accidentally forgotten.

---

### 🔵 Notes

---

**[N1] Envelope body convention should be made explicit**

*Axis: Protocol accuracy*

The design shows `body: { text: "hey" }` for message content. This matches `send_cmd.rs`'s convention (`json!({ "text": body_str })`). But "convention" isn't a strong enough word here — if the receiving agent (Aira's listener) parses `body.text`, then `body.text` is the de facto contract. Document this as the expected body schema for `type: "send"` messages in the Axon channel.

---

**[N2] `from` field is broker-populated — clarify in design**

*Axis: Protocol accuracy*

The outbound envelope mapping doesn't set `from`. That's correct — the broker fills it in from the authenticated identity. The design should note this explicitly so the implementation doesn't try to set it.

---

**[N3] `allowed_from` filter should normalize agent names**

*Axis: Architecture*

Agent names on the Axon mesh are case-sensitive (assuming). The `allowed_from` filter should normalize both the filter list and incoming `envelope.from` values (lowercase, trim whitespace) before comparison to avoid silent filter failures when identity names differ in case.

---

**[N4] Axon SDK crate extraction is correctly deferred**

*Axis: Tech stack*

The decision to implement the client protocol inline and defer `axon-client` crate extraction is the right call for v1. It's worth filing a Linear issue now so it doesn't get lost — once RustyClaw and OpenClaw both have Axon channels, the shared code will be substantial.

---

**[N5] Message ID derivation**

*Axis: Requirements*

Signal generates `id: format!("sig_{timestamp}")` for `ChannelMessage.id`. For Axon, use `envelope.id` directly (the broker assigns a stable UUID). The design's data flow correctly shows `id: envelope.id` — make sure the implementation uses that and doesn't generate a synthetic ID.

---

## What Breaks First

In order of likelihood:
1. **Ping/pong missing (B2)** — broker will disconnect within seconds of first connect, before any real message is exchanged
2. **Wrong socket path (B3)** — channel silently fails to connect on any machine where `~/.axon/broker.sock` doesn't exist
3. **No delivery ack read (B1)** — `send()` will appear to succeed but will corrupt the socket state; next `listen()` read will get a `delivery_ack` instead of an expected message
4. **Concurrent socket access (B4)** — race condition between `send()` and `listen()`, will manifest as panics or garbled reads under concurrent load

## What's Hardest to Change Later

The socket-sharing architecture (B4). Once `listen()` and `send()` are implemented independently, retrofitting a proper multiplexer or split-ownership model touches the core struct layout and both method implementations. Decide this architecture before writing any code.

---

## Pre-Implementation Checklist

- [ ] Resolve B4: design the socket ownership model (split halves + delivery ack routing)
- [ ] Fix B3: correct the default broker socket path
- [ ] Add B2: ping/pong to listen loop design  
- [ ] Add B1: delivery ack/nack read to send() data flow
- [ ] Clarify R2: `agent_type`, `capabilities`, `directory`, `max_message_size` for `register()`
- [ ] Add R3: explicit unit test plan for pure mapping functions
- [ ] Resolve R4: pick fixed or exponential backoff, align config field name
