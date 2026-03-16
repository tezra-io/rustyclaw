# Sentinel Design Review

**Reviewer:** Aira  
**Date:** 2026-03-11  
**Doc reviewed:** `sentinel-gateway-redaction-design.md`  
**Status:** Draft — blockers must be resolved before implementation starts

---

## Executive Summary

The design is well-intentioned and gets the inbound sanitization direction right. The outbound redaction has a **fundamental architectural mismatch** with the existing gateway code that will make "Tower middleware" the wrong implementation vehicle for roughly half its stated purpose. Several security bypass vectors are unaddressed. Performance claims are mostly realistic but the zero-alloc claim for NFKC is wrong. The scope in 6 issues is achievable if the Tower architecture question gets resolved first.

**Verdict: Do not start implementation until the outbound interception architecture is resolved (Blocker #1).**

---

## 1. Architecture

### 🔴 BLOCKER — Outbound interception is not achievable via Tower HTTP middleware

The design claims: *"Tower middleware intercepts all `SendMessage` calls before they hit channel `.send()`."*

This is incorrect for the existing codebase. Looking at `src/gateway/mod.rs`:

```rust
// Inside handle_whatsapp_message, handle_linq_webhook, handle_nextcloud_talk_webhook, etc.
match run_gateway_chat_with_tools(&state, &msg.content).await {
    Ok(response) => {
        if let Err(e) = wa.send(&SendMessage::new(response, &msg.reply_target)).await {
```

The LLM response is sent directly to the channel inside the Axum handler via `channel.send()`. The HTTP response returned from the handler is just `{"status": "ok"}` — not the agent output. Tower middleware intercepts the HTTP request/response pair, NOT business logic calls made within handlers.

**Tower middleware CAN handle inbound** (request body → sanitize → pass to handler). It **cannot transparently intercept** `wa.send()` / `linq.send()` / etc. unless every channel call is also made through an HTTP response path.

**Three viable alternatives — pick one before implementation:**

**Option A (Recommended):** Wrap the `Channel` trait with a `RedactingChannel<C: Channel>` decorator that intercepts `send()` and applies redaction before delegating. Wire this at channel construction time in `run_gateway`. Clean ownership, zero HTTP overhead, stays aligned with the trait-based extension points documented in CLAUDE.md.

**Option B:** Apply redaction at the `run_gateway_chat_with_tools` / `run_gateway_chat_simple` return boundary — before the caller gets the `String`. Simpler, but harder to extend and mixes concerns.

**Option C:** Route all agent output through a dedicated HTTP subrequest that a Tower layer CAN intercept. This is overengineered and adds IPC.

Inbound sanitization via Tower layer is sound. Keep that. The design should split Sentinel into two integration points: Tower layer for inbound, Channel decorator for outbound.

---

### 🟡 RECOMMENDATION — Integration with existing `src/security/` module not addressed

CLAUDE.md explicitly calls `src/security/` a **High-risk** tier. There is already a `SecurityPolicy` type and a `secrets` submodule in that path. Sentinel creates a parallel security layer without acknowledging whether it extends, replaces, or duplicates existing security work.

Before implementation: read what's in `src/security/secrets.rs` (or equivalent). If secret patterns are already managed there, Sentinel should extend that module rather than introduce a competing one. Duplication here will create maintenance and trust confusion — which layer is authoritative?

---

### 🟡 RECOMMENDATION — The 19-channel claim needs verification

The design says *"all 19 channels route through the gateway."* The gateway code shows direct channel instances (`WhatsApp`, `Linq`, `NextcloudTalk`, `WATI`) with per-channel handler functions. Channels like Telegram, Discord, and Signal likely have their own listener loops that don't go through these webhook handlers.

If non-webhook channels (polling-based, bot API listeners, etc.) bypass the gateway HTTP layer, outbound messages from those channels are not covered regardless of what Tower middleware is added. The "single enforcement point" claim may be false for 14 of the 19 channels.

**Action:** Audit which channels are webhook-driven vs. poll/event-driven before claiming complete coverage.

---

### 📝 NOTE — Tower layer placement in existing middleware stack

The existing `run_gateway` function applies layers in this order (innermost to outermost):
1. `RequestBodyLimitLayer::new(MAX_BODY_SIZE)` (64KB)
2. `TimeoutLayer` (30s)

Sentinel's inbound Tower layer must sit **between** body limit (outer, already enforced) and the handler (inner), so it receives bodies that have already been size-limited. Don't wrap the entire `app` outside the body limit layer — that would scan the raw bytes before the 64KB limit is enforced, creating a window where a >64KB body could trigger expensive Sentinel scanning before being rejected. Place the Sentinel layer inside the existing stack, not outside it.

---

## 2. Performance

### 🔴 BLOCKER — NFKC normalization always allocates; zero-alloc claim is wrong

The design claims: *"Use `Cow<str>` — return borrowed original if no redaction needed (common case)"* and *"Zero allocation per scan."*

This is true for the Aho-Corasick scan and the regex scan. It is **not true for NFKC normalization.**

The `unicode-normalization` crate's `UnicodeNormalization::nfkc()` method returns a lazy iterator of `char`s. Collecting this back to a `String` requires heap allocation unconditionally. The "quick check" (`is_nfkc_quick()`) returns `MaybeYes`, `Yes`, or `No` — `MaybeYes` requires running full normalization to confirm, which is the common case for mixed ASCII/Unicode input. 

For a message that is pure ASCII (overwhelmingly common for chat messages), NFKC will be a no-op and you can shortcut. Add an explicit ASCII fast path:

```rust
if text.is_ascii() {
    // Only strip zero-width / tag chars; skip NFKC entirely
    // Return Cow::Borrowed if nothing stripped
}
```

For the non-ASCII path, allocate and document the behavior honestly. Don't claim zero-alloc for the NFKC pipeline.

---

### 🟡 RECOMMENDATION — Entropy detection logic is internally contradictory

The performance table says: *"Only runs on Aho-Corasick hits, not full text"*

The detection algorithm says: *"Optional: entropy scan for unrecognized high-entropy strings"*

These are mutually exclusive. If entropy only runs on Aho-Corasick hits, it catches secrets with known prefixes that also have high entropy — but you'd catch those with the prefix match anyway. The *purpose* of entropy detection is to catch **unknown** secret formats with no known prefix. That requires scanning the full message, tokenized into candidate strings (words, base64-looking chunks, etc.).

Pick one: either entropy is a full-message scan for novel secrets (slower, ~10-20μs per 1KB, runs on all messages when enabled), or it's a validation step for prefix hits (faster but nearly redundant). The design implies the former; the performance claim implies the latter. This needs to be resolved in the spec before the builder implements it.

---

### 📝 NOTE — `regex` crate DFA compilation memory at startup

The proposed connection string pattern (`postgres://user:pass@`, `mongodb+srv://` + variants) can produce a large DFA if written naively. The `regex` crate uses a lazy DFA by default, which is fine, but `RegexSet` with many alternations can have significant compilation time. Benchmark pattern compilation time at startup against the `regex` crate's compile-time docs. Consider `aho-corasick` for the simple prefix-only patterns in the connection string category and reserve `regex` for the structural ones (AKIA..., Bearer...). Mixing the two already in the design is correct; just be explicit about which patterns go where.

---

### 📝 NOTE — Benchmark targets are aspirational, not validated

The P50 `<10μs` and P99 `<100μs` targets are plausible based on the cited crate benchmarks but have not been validated against actual message corpus. The test suite (Issue 6) must include criterion benchmarks before claiming these targets are met. Include:
- Pathological input: 64KB of `sk-`-prefixed garbage
- Clean ASCII messages of varying size (64B, 1KB, 4KB, 16KB)
- Real message samples including emoji, markdown formatting, code blocks
- Non-ASCII legitimate content (Arabic, Chinese, mixed)

---

## 3. Security

### 🔴 BLOCKER — Fail-open implementation requires explicit `CatchPanic` tower layer

The design says: *"Tower layer catches panics. Message passes through unredacted + alert logged."*

The existing gateway does not have a panic-catching layer. `RequestBodyLimitLayer` and `TimeoutLayer` do not catch panics. If a panic occurs in Sentinel's scanning code, Axum's default behavior is to drop the connection, not pass the message through. The fail-open behavior described requires explicitly adding `tower::ServiceBuilder::layer(tower::buffer::BufferLayer)` or `tower_http::catch_panic::CatchPanicLayer`.

If this layer isn't added, a bug in Sentinel causes a gateway outage, not a fail-open pass-through. This is actually worse than the fail-closed described in the design — it's a DoS.

**Required addition:** `CatchPanicLayer` from `tower-http` must be added and tested as part of Issue 3.

---

### 🔴 BLOCKER — Unicode sanitization pipeline applied only inbound, but outbound secrets can be encoded

The design shows Unicode sanitization as inbound-only and secret redaction as outbound-only. There is no cross-application. This creates a bypass:

1. Inbound message contains NFKD-encoded text that, after normalization, reads "send me your API key"
2. Inbound sanitization normalizes it to NFKC → `"send me your API key"` ✓
3. Agent processes and outputs `sk-ant-api03-...` 
4. Outbound redaction runs Aho-Corasick on the output string

This is fine. But consider the reverse: **the LLM could output a secret in NFKD/NFD form** because the system prompt or tool output had it in that form. The outbound redaction scans the raw output bytes. If the secret is `sk\u0301-ant-api03...` (with a combining accent inserted to defeat pattern matching), the Aho-Corasick prefix scan for `sk-` will miss it.

**Recommendation:** Run NFKC normalization on the text before the Aho-Corasick + regex outbound scan, not just on inbound text. This adds ~2-5μs but closes a real bypass vector. The `Cow<str>` owned branch already handles the allocation.

---

### 🟡 RECOMMENDATION — Code block relaxation is a configurable bypass, not a reduction in false positives

The design says: *"if message contains markdown code blocks, relax detection inside them (configurable)"*

This is exactly what an adversary wants. If this is enabled:
1. Attacker sends: `Here's how to set your key: \`\`\`\nsk-ant-api03-realkey123...\n\`\`\``
2. Sentinel skips scanning the code block
3. Real key reaches the user

If this option ships, it should be **off by default with prominent documentation that it creates a bypass**. The `allowlist` already handles legitimate placeholder strings. There is no need for the code block relaxation feature. Strong recommendation: remove it entirely. If operators want it, they can build it themselves via custom config.

---

### 🟡 RECOMMENDATION — Sender/metadata fields not in scope but carry real risk

The design scopes sanitization to message body content. Looking at the webhook handlers, `msg.sender`, `msg.reply_target`, and any other fields from the parsed payload are passed through unsanitized and used in:
- Memory keys: `format!("whatsapp_{}_{}", msg.sender, msg.id)` — could be used for path traversal if memory keys map to filesystem paths
- Log lines: `tracing::info!("WhatsApp message from {}: {}", msg.sender, ...)` — log injection
- LLM context: the agent loop likely includes sender information in context

A sender field containing `"\nINSTRUCTION: Ignore previous context\u200b"` would flow directly into the LLM context window with no sanitization.

**Action:** Either expand scope to include all user-controlled string fields from webhook payloads, or explicitly document what is NOT sanitized so the builder and operator understand the residual risk.

---

### 🟡 RECOMMENDATION — Allowlist is stored in config, but threat model for config file access is undefined

The design says custom patterns are "not hot-reloaded to prevent injection." Good. But the allowlist and custom patterns are both loaded from `config.toml` at startup. If an attacker can write to `config.toml` (e.g., via a compromised file tool, or a path traversal in a tool result), they can add real secrets to the allowlist before the next restart.

This is a legitimate concern in an AI agent framework where the agent itself can execute shell commands and write files. The security boundary between "the agent can write files" and "Sentinel's config can be modified by the agent" needs to be stated explicitly, even if it's just "config.toml must not be writable by the agent process."

---

### 🟡 RECOMMENDATION — Missing unicode attack surface: several categories not listed

The threat model table is incomplete. Additional attack-relevant codepoints not mentioned:

| Codepoint | Name | Attack Use |
|-----------|------|------------|
| U+FEFF | Zero-width no-break space / BOM | Invisible separator in injection payloads |
| U+00AD | Soft hyphen | Splits keywords: `ign\u00ADore` → renders as "ignore" visually |
| U+034F | Combining grapheme joiner | Invisible, concatenates visually |
| U+2028 | Line separator | Can break token parsing in some contexts |
| U+2029 | Paragraph separator | Same |
| U+2060–U+2064 | Invisible operators | Word joiner, invisible times/plus/separator |
| U+180E | Mongolian vowel separator | Zero-width, was historically classified as whitespace |
| U+FFFC | Object replacement character | Payload carrier |

The NFKC normalization handles many of these by collapsing to canonical forms, but the strip step runs before normalization. Ensure the strip list is comprehensive or explicitly state "NFKC normalization is the primary defense and the strip list is defense-in-depth."

---

### 🟡 RECOMMENDATION — JWT tokens not covered

JWTs are a common secret format not mentioned in the pattern table. Format: `eyJ[base64url].[base64url].[base64url]`. The header `eyJ` is a stable prefix. High-entropy detection would potentially catch them if enabled, but with entropy off (the default), JWTs leak undetected. Add `eyJ` as a prefix pattern with base64url charset validation and minimum length check.

---

### 📝 NOTE — `regex` crate is ReDoS-safe for built-in patterns

The design uses `regex` crate. This crate uses a guaranteed linear-time engine for patterns without backreferences. However, **custom patterns loaded from config are not safe by default** if they use features that force the `regex-automata` crate into NFA mode (e.g., backreferences, lookahead — though the `regex` crate rejects these at compile time). Custom patterns will fail to compile if they use these features, which is the correct behavior. Document this for operators.

---

## 4. Completeness

### 🟡 RECOMMENDATION — WebSocket path `/ws/chat` not addressed

The gateway has `src/gateway/ws.rs` handling `/ws/chat`. WebSocket connections are bidirectional and long-lived. Inbound WS frames are not HTTP request bodies and won't be processed by a Tower `RequestBodyLimitLayer`-style inbound layer. Outbound WS messages (agent responses) are sent via the WS write half, not via `channel.send()`.

Sentinel as designed has no coverage for WebSocket traffic. This should be explicitly scoped in or out. If out, document the gap prominently.

---

### 🟡 RECOMMENDATION — Memory stored BEFORE sanitization in current code

In `handle_webhook` and all channel handlers, the current code stores messages to memory before any processing:

```rust
if state.auto_save {
    let key = webhook_memory_key();
    let _ = state.mem.store(&key, message, MemoryCategory::Conversation, None).await;
}
```

If Sentinel sanitizes inbound messages, the sanitized version should be stored in memory, not the raw input. Otherwise unicode exploits survive in the memory store and re-enter the context window on future recalls, bypassing the inbound sanitization entirely.

**The memory store call must happen after Sentinel inbound sanitization, not before.**

---

### 🟡 RECOMMENDATION — SSE output path not addressed

`/api/events` (handled by `sse.rs`) emits real-time events that include agent outputs and potentially tool results. If a secret is emitted via the SSE event stream, it bypasses both the Tower middleware (which operates on HTTP responses, not SSE frames) and the channel decorator approach (SSE isn't a `Channel` trait implementor).

Scope this in or out explicitly.

---

### 🟡 RECOMMENDATION — `/api/config` endpoint leaks config including secrets

`GET /api/config` is listed in the router. If the config contains channel tokens, API keys, or other credentials that are part of the config schema, this endpoint exposes them directly. This is not strictly a Sentinel scope issue, but it's worth noting that Sentinel redacting message traffic doesn't help if secrets are directly served by an API endpoint. Review whether `api::handle_api_config_get` scrubs sensitive fields before returning.

---

### 📝 NOTE — Streaming responses not in scope but worth noting

The design is scoped to non-streaming message processing. If the gateway ever exposes a streaming mode (token-by-token via SSE or chunked responses), single tokens won't contain full secrets and per-token redaction is impractical. The correct approach for streaming would be a buffer-ahead scheme that holds N tokens before flushing, scanning the buffer for patterns. This is a Phase 2 concern but should be noted as an architectural dependency if streaming support is planned.

---

### 📝 NOTE — Multi-agent inter-agent messages (Elixir bridge)

Open question 4 in the doc asks about Elixir orchestration. Per CLAUDE.md, the Elixir layer communicates with Rust via HTTP or Erlang Port. If inter-agent messages are routed through the HTTP gateway, inbound Sentinel covers them. If they use a separate Port/pipe interface, they are not covered. Confirm the bridge interface before the Elixir layer ships.

---

## 5. Practicality

### 🟡 RECOMMENDATION — Issue 3 (Tower middleware) is severely underscoped

The issue is described as: *"Axum layer, inbound/outbound hooks, SendMessage interception."* 

Given the architectural blocker above — that outbound `SendMessage` interception cannot be done purely via Tower — this issue actually needs to:

1. Resolve the architecture question (Tower inbound + Channel decorator outbound)
2. Design and implement the `RedactingChannel<C>` wrapper (if Option A chosen)
3. Wire the wrapper at channel construction in `run_gateway` for all 19 channels (including non-webhook channels that aren't in `mod.rs`)
4. Add `CatchPanicLayer` for fail-open behavior
5. Verify inbound Tower layer placement within the existing middleware stack

This is easily 2 issues. Consider splitting into:
- Issue 3a: Inbound Tower layer (sanitization hook on request body before handlers)
- Issue 3b: Outbound Channel decorator (redaction hook on `Channel::send` across all channel types)

---

### 🟡 RECOMMENDATION — Issue 6 (test suite) is also undersized

*"Unit tests for all pattern categories, unicode edge cases, benchmark suite, false positive validation"* — this is described as a single P2 issue but covers:
- Unit tests for each of 7+ pattern categories (each with positive/negative cases)
- Unicode edge cases (at minimum 15+ categories of attack characters)  
- Criterion benchmark suite with multiple input profiles
- False positive corpus (generated, validated)
- Integration tests verifying Tower layer placement
- Integration tests verifying `CatchPanicLayer` behavior
- Integration tests verifying memory-after-sanitization ordering

Split into test-suite-unit and test-suite-benchmarks, or budget significantly more time than implied.

---

### 📝 NOTE — The 6-issue plan is achievable with the right sequence

If the architecture blocker is resolved first (add a pre-Issue-1 design spike), the remaining 6 issues are reasonable in scope with the size adjustments noted above. Suggested order:

1. **Spike:** Resolve outbound interception architecture (Channel decorator vs. other). No code yet — just a decision logged in the design doc.
2. **TEZ-1XX (Issue 1):** Core redaction engine — now you know its interface (`fn redact(input: &str) -> Cow<str>`)
3. **TEZ-1XX (Issue 2):** Unicode sanitization pipeline — similarly bounded
4. **TEZ-1XX (Issue 3a):** Inbound Tower layer — the tractable half
5. **TEZ-1XX (Issue 3b):** Outbound Channel decorator — the hard half
6. **TEZ-1XX (Issue 4):** Config schema + allowlist
7. **TEZ-1XX (Issue 5):** Observability + alerting
8. **TEZ-1XX (Issue 6):** Test suite + benchmarks

That's 7 real issues + 1 spike. Adjust the Linear plan accordingly.

---

## Summary Table

| # | Finding | Severity | Axis |
|---|---------|----------|------|
| 1 | Outbound interception not achievable via Tower HTTP middleware | 🔴 Blocker | Architecture |
| 2 | Fail-open requires explicit `CatchPanicLayer` not currently in gateway | 🔴 Blocker | Security |
| 3 | Outbound redaction must normalize text first (NFKD bypass) | 🔴 Blocker | Security |
| 4 | NFKC zero-alloc claim is incorrect; ASCII fast-path needed | 🔴 Blocker | Performance |
| 5 | Entropy detection logic is internally contradictory | 🟡 Rec | Performance |
| 6 | Integration with `src/security/` module not addressed | 🟡 Rec | Architecture |
| 7 | 19-channel coverage claim needs verification for non-webhook channels | 🟡 Rec | Architecture |
| 8 | Code block relaxation is a configurable bypass vector | 🟡 Rec | Security |
| 9 | Sender/metadata fields unsanitized, enter LLM context | 🟡 Rec | Security |
| 10 | Config file writeable by agent = allowlist injection | 🟡 Rec | Security |
| 11 | Unicode attack surface table is incomplete | 🟡 Rec | Security |
| 12 | JWT tokens not in redaction pattern table | 🟡 Rec | Completeness |
| 13 | WebSocket path not covered by Sentinel | 🟡 Rec | Completeness |
| 14 | Memory stored before sanitization in current handlers | 🟡 Rec | Completeness |
| 15 | SSE output path not addressed | 🟡 Rec | Completeness |
| 16 | `/api/config` endpoint may expose secrets directly | 🟡 Rec | Completeness |
| 17 | Issue 3 (Tower middleware) is underscoped — split into 3a/3b | 🟡 Rec | Practicality |
| 18 | Issue 6 (test suite) is underscoped — split or budget more | 🟡 Rec | Practicality |
| 19 | `regex` ReDoS safety for custom patterns — document for operators | 📝 Note | Security |
| 20 | Streaming responses not in scope — note as future arch dependency | 📝 Note | Completeness |
| 21 | Tower layer placement relative to existing middleware stack | 📝 Note | Architecture |
| 22 | Benchmark targets need criterion validation, not just estimation | 📝 Note | Performance |

---

*Review complete. Four blockers must be addressed before implementation. The design is directionally correct — the threat model is real, the technology choices are right. The gaps are in integration details and one core architectural assumption that the implementation will hit immediately if not resolved first.*
