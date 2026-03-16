# Sentinel: Visibility-Boundary Secret Redaction & Unicode Sanitization

**Status:** Revised (v2)
**Author:** Aira (with Sujeeth)
**Date:** 2026-03-11
**Revision:** Addresses all 4 blockers from `sentinel-design-review.md`

---

## Problem

AI agents have access to secrets (API keys, tokens, credentials) via tool outputs, environment variables, and file reads. Even with good system prompts, an LLM can be tricked (or simply err) into including secrets in outbound messages. Current defenses rely on the model's compliance — there's no architectural enforcement.

Additionally, inbound messages from external sources (emails, webhooks, chat) can contain unicode exploits (zero-width characters, RTL overrides, homoglyphs) that enable prompt injection or data exfiltration through invisible text.

**RustyClaw needs a visibility-boundary redaction layer that:**
1. Catches secrets in outbound messages before they cross the visibility boundary to humans or public surfaces
2. Sanitizes inbound messages of unicode exploits before they hit the context window
3. Adds sub-millisecond latency (microsecond-range target — non-negotiable)

---

## Core Principle: Visibility Boundary, Not Execution Boundary

Sentinel guards the **visibility boundary** — the line between internal agent execution and what humans or public surfaces can see.

**What Sentinel scans:**
- Messages leaving the system to humans (all Channel outputs)
- Content sent to public surfaces via external-facing tools (browser form fills, tweet composition, etc.)
- Inbound messages from untrusted sources (webhooks, chat, email)

**What Sentinel does NOT scan:**
- Internal agent operations (env var reads, API calls, builds, shell commands)
- Memory storage — the agent freely stores whatever it needs internally
- LLM provider calls — the agent can see and use secrets; we only prevent exfiltration
- Inter-agent messages (Elixir orchestration) — internal to the system

This is a deliberate design choice: the agent should freely USE secrets internally. Sentinel only prevents secrets from crossing to where humans or the public can see them.

---

## Tech Stack

- **Rust** — native to the existing gateway and channel system
- **`aho-corasick`** crate — O(n) multi-pattern matching, compiled once at startup
- **`regex`** crate — already in deps, for structured pattern matching (API key formats)
- **`unicode-normalization`** crate — NFKC normalization for unicode sanitization
- **Tower middleware** — for inbound HTTP sanitization only
- **Channel trait decorator** — for outbound redaction via `RedactingChannel<C>`
- **Tool trait decorator** — for external-facing tool output scanning

---

## Architecture

```
              INBOUND                                    OUTBOUND

 HTTP ──► [Tower: Unicode Sanitizer] ──► Agent      Agent ──► [RedactingChannel<C>] ──► Human
 Webhooks    (clean input)               Context     Output     (catch leaks)           Channel

 WS ────► [WS frame sanitizer] ────────►            Agent ──► [Tool scan] ──► External-facing
 Frames     (same pipeline)                          Tool out   (browser type,   tool execution
                                                                form fills)
```

**Two integration points, not one:**
- **Inbound:** Tower middleware for HTTP requests + WebSocket frame hook for WS
- **Outbound:** `RedactingChannel<C>` trait decorator wrapping `Channel::send()` + tool output scanning for external-facing tools

### Why NOT Tower for Outbound

The gateway's outbound path calls `channel.send()` directly inside Axum handlers:

```rust
// In handle_whatsapp_message, handle_linq_webhook, etc.:
if let Err(e) = wa.send(&SendMessage::new(response, &msg.reply_target)).await {
```

Tower middleware intercepts HTTP request/response pairs, not business logic calls within handlers. The HTTP response is just `{"status": "ok"}` — the agent output goes through `channel.send()`, which Tower cannot intercept.

### Why Channel Trait Decorator

The `Channel` trait (`src/channels/traits.rs`) defines `send(&self, message: &SendMessage)` as the single output path for all 19+ channels. A `RedactingChannel<C: Channel>` decorator wraps this method, applying redaction before delegating to the inner channel. This aligns with the trait-based extension pattern documented in CLAUDE.md and provides:

- **Complete coverage:** Every channel output goes through `send()`, regardless of whether the channel is webhook-driven, polling-based, or event-driven
- **Zero HTTP overhead:** No request/response interception needed
- **Clean ownership:** Wired at channel construction time in `run_gateway` and `start_channels`

### Why Tool Decorator for External-Facing Tools

Some tools produce output that reaches public surfaces without going through a Channel:
- Browser `type` commands (filling text into web forms, composing tweets)
- Any tool that posts to external APIs on behalf of the user

These tools are tagged as `external_facing: true` in their `ToolSpec`. A `RedactingTool<T: Tool>` decorator scans output before execution for external-facing tools only. Internal tools (shell, file I/O, memory) are not scanned.

### SSE and WebSocket Coverage

- **WebSocket (`/ws/chat`):** WS frames are bidirectional and bypass Tower HTTP layers. Inbound WS frames pass through the same sanitization pipeline before entering the agent loop. Outbound WS responses pass through the redaction engine before being written to the WS write half.
- **SSE (`/api/events`):** SSE events that include agent outputs pass through redaction before being emitted. The SSE handler applies the same `redact()` call on event payloads.

### Integration with `src/security/`

Sentinel extends the existing security subsystem, not a parallel one. The redaction engine lives under `src/security/sentinel/` alongside `SecurityPolicy` and `secrets`. Pattern definitions may reference `src/security/secrets.rs` if secret patterns are already managed there, avoiding duplication.

---

## Outbound: Secret Redaction

### Pattern Categories

| Category | Detection Method | Examples |
|----------|-----------------|----------|
| API keys | Prefix match + length validation | `sk-`, `ghp_`, `ghu_`, `xoxb-`, `xoxp-`, `AKIA`, `pk_live_`, `sk_live_`, `sk_test_` |
| JWT tokens | Prefix: `eyJ` + base64url charset + dot-separated structure | `eyJhbGci...` (header.payload.signature) |
| Bearer tokens | Regex: `Bearer [A-Za-z0-9_\-\.]+` | Authorization headers leaked in text |
| AWS credentials | Regex: `AKIA[0-9A-Z]{16}` + secret key pattern | AWS access key + secret |
| Connection strings | Regex: protocol URIs with credentials | `postgres://user:pass@`, `mongodb+srv://` |
| Private keys | Prefix: `-----BEGIN (RSA\|EC\|OPENSSH) PRIVATE KEY-----` | PEM-encoded keys |
| High-entropy strings | Shannon entropy > 4.5 + length > 20 + base64-like charset | Generic tokens (opt-in, full-message scan) |
| Custom patterns | User-defined in config | Org-specific secrets |

### Detection Engine

```
Startup:
  1. Compile Aho-Corasick automaton from all prefix patterns (sk-, ghp_, eyJ, AKIA, etc.)
  2. Compile regex set from structural patterns (Bearer, connection strings, PEM)
  3. Load user-defined custom patterns from config

Per-message (outbound):
  1. NFKC-normalize the text (is_ascii fast path — see Performance section)
  2. Run Aho-Corasick scan (O(n), single pass) on normalized text
  3. For any prefix hit, validate with length/charset rules (eliminate false positives)
  4. Run regex set on normalized text (compiled, fast)
  5. If entropy_detection enabled: tokenize full message into candidate strings,
     compute Shannon entropy on each (this is a full-message scan, not just AC hits)
  6. Replace matches with [REDACTED:<category>]
```

Note: NFKC normalization runs BEFORE pattern matching on outbound text. This closes the bypass vector where secrets could be encoded in NFKD/NFD form (e.g., `sk\u0301-ant-...` with combining accents) to defeat Aho-Corasick prefix matching.

### Replacement Strategy

- Default: `[REDACTED:api_key]`, `[REDACTED:jwt]`, `[REDACTED:aws_credential]`, etc.
- Tokens are NOT reversible — no lookup table that could itself leak
- Log redaction events (category + position, never the secret itself) to observability

### Allowlist

Some legitimate outputs contain key-like strings (e.g., tool explaining how to set an env var with a placeholder). Config-level allowlist:

```yaml
sentinel:
  redaction:
    allowlist:
      - "sk-your-key-here"    # documentation placeholders
      - "EXAMPLE_KEY"
    allow_patterns:
      - "^sk-xxx"             # masked examples
```

**No code block relaxation.** Relaxing detection inside markdown code blocks is a bypass vector — an attacker wraps a real secret in backticks and it passes through. The allowlist handles legitimate placeholder strings. This feature is intentionally excluded.

### Sender and Metadata Field Sanitization

Inbound webhook payloads include user-controlled fields beyond message body: `sender`, `reply_target`, and other metadata. These flow into memory keys, log lines, and LLM context unsanitized.

Sentinel sanitizes ALL user-controlled string fields from webhook payloads, not just the message body. This covers:
- `ChannelMessage.sender` — could contain prompt injection or path traversal
- `ChannelMessage.reply_target` — same risk
- Any additional metadata fields parsed from the payload

---

## Inbound: Unicode Sanitization

### Threat Model

| Attack | Unicode Technique | Impact |
|--------|-------------------|--------|
| Invisible instruction injection | Zero-width spaces (U+200B), zero-width joiners (U+200D), zero-width non-joiners (U+200C) | Hidden text in emails/messages that the LLM sees but humans don't |
| Direction override | RTL override (U+202E), LTR override (U+202D) | Text renders differently than actual byte content |
| Homoglyph substitution | Cyrillic а (U+0430) vs Latin a (U+0061) | Bypasses keyword filters, confuses pattern matching |
| Tag characters | U+E0001–U+E007F | Invisible metadata that can carry payload |
| Variation selectors | U+FE00–U+FE0F (beyond emoji use) | Invisible modifiers |
| BOM / ZWNBSP | U+FEFF | Invisible separator in injection payloads |
| Soft hyphen | U+00AD | Splits keywords: `ign\u00ADore` renders as "ignore" |
| Combining grapheme joiner | U+034F | Invisible concatenation |
| Line/paragraph separators | U+2028, U+2029 | Break token parsing |
| Invisible operators | U+2060–U+2064 | Word joiner, invisible times/plus/separator |
| Mongolian vowel separator | U+180E | Zero-width, historically whitespace |
| Object replacement | U+FFFC | Payload carrier |

### Sanitization Pipeline

```
Per-message (inbound):
  1. Fast path: if text.is_ascii() → skip NFKC, only strip control chars → Cow::Borrowed if clean
  2. Strip: zero-width chars, tag characters, variation selectors (non-emoji), BOM, soft hyphens,
     combining grapheme joiner, invisible operators, Mongolian vowel separator, object replacement
  3. Replace: RTL/LTR overrides and line/paragraph separators with space
  4. Normalize: NFKC (collapses homoglyphs to canonical forms) — allocates for non-ASCII text
  5. Flag messages that had significant sanitization (log + metadata)
```

### What We Preserve
- Standard emoji (including ZWJ sequences like 👨‍💻)
- Legitimate RTL text (Arabic, Hebrew) — only strip *override* characters, not natural bidi
- Accented characters (NFKC preserves these correctly)

### Memory Ordering

Inbound messages must be sanitized BEFORE storage to memory. The current code path stores raw input to memory before processing — sanitized content must be stored instead. Otherwise unicode exploits survive in memory and re-enter the context window on future recalls.

---

## Performance Target: Sub-Millisecond (Microsecond Range)

This is non-negotiable. Sentinel must not perceptibly affect message latency.

### Realistic Performance Model

| Operation | Expected Latency | Allocation | Notes |
|-----------|-----------------|------------|-------|
| ASCII fast-path check | ~0.1μs | None | `str::is_ascii()` — single SIMD pass |
| Aho-Corasick scan | ~1-5μs per KB | None | Linear scan, no backtracking, SIMD-optimized |
| Regex set match | ~5-20μs per KB | None | Compiled lazy DFA |
| NFKC normalization | ~2-10μs per KB | **Yes — heap alloc** | Only for non-ASCII text; ASCII fast path avoids this entirely |
| Entropy calculation | ~1-3μs per candidate | Minimal | Only when entropy_detection enabled; scans full message |
| **Total per message** | **~10-50μs** | **Cow::Borrowed for clean ASCII** | For typical 1-4KB messages |

### Design Decisions for Speed

1. **ASCII fast path:** `text.is_ascii()` is a single SIMD-width check. For pure-ASCII messages (the overwhelming majority), skip NFKC entirely — zero allocation, return `Cow::Borrowed`.
2. **Compile once, scan many:** All patterns compiled at startup into automata.
3. **Single-pass where possible:** Aho-Corasick does one pass for all prefix patterns simultaneously.
4. **Honest allocation model:** NFKC normalization allocates for non-ASCII text. The `Cow<str>` fast path covers the common ASCII case. Non-ASCII text takes the `Cow::Owned` branch. No false "zero-alloc" claims.
5. **Short-circuit:** If message is below a length threshold and contains no trigger characters, skip scanning entirely.
6. **Entropy is opt-in and full-message:** High-entropy detection scans the full message (tokenized into candidate strings), not just AC hits. It's the most expensive check. Off by default.

### Benchmark Targets (Must Be Validated with Criterion)

- P50: < 10μs for clean ASCII messages (no redaction needed)
- P99: < 100μs for messages requiring redaction
- Must not exceed 1ms even for pathological inputs (64KB body full of key-like strings)

Benchmark suite must include:
- Pathological: 64KB of `sk-`-prefixed garbage
- Clean ASCII at varying sizes (64B, 1KB, 4KB, 16KB)
- Real message samples with emoji, markdown, code blocks
- Non-ASCII legitimate content (Arabic, Chinese, mixed scripts)

---

## Fail-Open with CatchPanicLayer

### Fail Mode: Fail-Open with Alert (Configurable)

If Sentinel panics or errors during scanning, the message passes through unredacted and an alert is logged. This prevents a Sentinel bug from causing a gateway outage.

**Critical requirement:** The existing gateway has no panic-catching layer. `RequestBodyLimitLayer` and `TimeoutLayer` do not catch panics. Without explicit panic handling, a panic in Sentinel drops the connection — which is effectively a DoS, worse than either fail-open or fail-closed.

`CatchPanicLayer` from `tower-http` must be added to the middleware stack, wrapping the Sentinel inbound layer. For outbound (Channel decorator), the `RedactingChannel::send()` implementation uses `std::panic::catch_unwind` to achieve the same fail-open behavior.

```yaml
sentinel:
  fail_mode: open  # or "closed" for high-security deployments
```

### Tower Layer Placement

The inbound Sentinel layer sits INSIDE the existing middleware stack — between the body limit layer (outer, already enforced) and the handlers (inner). This ensures:
1. Bodies are already size-limited to 64KB before Sentinel scans them
2. Sentinel doesn't scan oversized bodies that will be rejected anyway

```
Outer → RequestBodyLimitLayer(64KB)
       → TimeoutLayer(30s)
         → CatchPanicLayer  ← NEW
           → SentinelInboundLayer  ← NEW
             → Router (handlers)
```

---

## Configuration

```yaml
sentinel:
  enabled: true
  fail_mode: open  # "open" (default) or "closed"

  redaction:
    enabled: true
    custom_prefixes: ["myorg_key_"]
    custom_patterns: ["MYORG-[A-Z0-9]{32}"]
    allowlist: ["sk-your-key-here", "EXAMPLE"]
    entropy_detection: false  # opt-in, scans full message when enabled
    entropy_threshold: 4.5
    min_entropy_length: 20
    replacement_format: "[REDACTED:{category}]"
    log_redactions: true

  sanitization:
    enabled: true
    strip_zero_width: true
    strip_tag_characters: true
    normalize_unicode: true  # NFKC — allocates for non-ASCII only
    strip_bidi_overrides: true
    preserve_emoji_zwj: true
    sanitize_metadata_fields: true  # sender, reply_target, etc.
    log_sanitizations: true

  external_tools:
    scan_enabled: true  # scan external-facing tool outputs
```

### Config Security

Custom patterns and allowlists are loaded from config at startup — not hot-reloaded to prevent injection. The config file must not be writable by the agent process. If an attacker gains write access to config (e.g., via a compromised file tool), they could add secrets to the allowlist. This is documented as a security boundary: `config.toml` must be protected by filesystem permissions.

### Custom Pattern Safety

The `regex` crate uses a guaranteed linear-time engine and rejects patterns with backreferences or lookahead at compile time. Custom patterns from config that use unsupported features will fail to compile with a clear error. This is correct behavior — document it for operators.

---

## User Scenarios

### Scenario 1: LLM Accidentally Outputs API Key
Agent runs a shell tool that returns env vars including `ANTHROPIC_API_KEY=sk-ant-abc123...`. LLM includes this in a response to the user on Telegram.

**Flow:** Agent output → `RedactingChannel<TelegramChannel>::send()` → NFKC normalize (ASCII fast path, no alloc) → Aho-Corasick matches `sk-ant-` prefix → validates length/charset → replaces with `[REDACTED:api_key]` → inner `TelegramChannel::send()` sends masked message.

### Scenario 2: Prompt Injection via Email
Webhook delivers an email containing hidden zero-width instructions between visible text.

**Flow:** Email payload → Tower inbound layer → strips zero-width characters → NFKC normalizes → hidden text becomes visible/neutralized → sanitized message stored to memory → clean message enters agent context.

### Scenario 3: JWT Token in Agent Response
Agent reads a `.env` file and includes a JWT (`eyJhbGci...`) in its response.

**Flow:** Agent output → `RedactingChannel::send()` → Aho-Corasick matches `eyJ` prefix → validates base64url charset and dot-separated structure → `[REDACTED:jwt]`.

### Scenario 4: Browser Tool Composing a Tweet
Agent uses browser `type` tool to compose a tweet that accidentally includes an API key from context.

**Flow:** Tool `execute()` → `RedactingTool<BrowserTool>` (tagged `external_facing`) → scans typed text → catches `sk-` prefix → redacts before the keystroke reaches the browser.

### Scenario 5: Unicode-Encoded Secret Bypass Attempt
Attacker crafts a prompt injection that causes the LLM to output a secret with combining accents: `sk\u0301-ant-api03-real_secret`.

**Flow:** Agent output → `RedactingChannel::send()` → NFKC normalize collapses `\u0301` → text becomes `sk-ant-api03-real_secret` → Aho-Corasick matches `sk-ant-` → `[REDACTED:api_key]`. Bypass defeated.

### Scenario 6: Clean Message (Common Case)
99%+ of messages contain no secrets and no unicode exploits.

**Flow:** Message → `is_ascii()` check (true) → Aho-Corasick scan (no hits) → regex set (no hits) → `Cow::Borrowed` returned → zero allocation, ~1-5μs.

---

## Edge Cases & Known Limitations

| Edge Case | Handling |
|-----------|----------|
| Base64-encoded secrets | Not caught in v1. Would require decode-and-scan with high false-positive risk on legitimate base64 (images, file data). Documented limitation. |
| Secrets split across messages | Not caught. Would require stateful tracking across messages (too expensive, privacy concerns). |
| False positives on UUIDs/hashes | Length + charset validation after prefix match reduces these. Allowlist for persistent false positives. |
| Legitimate RTL text (Arabic/Hebrew) | Only strip override *control characters*, not natural bidi text |
| Emoji ZWJ sequences | Explicitly preserved — ZWJ between emoji codepoints is not stripped |
| Pathological input (64KB of fake keys) | Bounded by request body limit (64KB). Scan is O(n), won't degrade. |
| Sentinel panic | `CatchPanicLayer` (inbound) and `catch_unwind` (outbound) ensure fail-open + alert. |
| Non-webhook channels | `RedactingChannel<C>` wraps `Channel::send()` regardless of how the channel receives messages — polling, WebSocket, event-driven all covered. |
| Streaming responses | Not in v1 scope. Token-by-token streaming would need a buffer-ahead scheme. Noted as future architectural dependency. |
| Elixir inter-agent messages | Not scanned — internal to the system. All external I/O routes through Rust channels. |
| `/api/config` endpoint | Out of Sentinel scope but noted: review whether `handle_api_config_get` scrubs sensitive fields. |

---

## Resolved Questions (from v1 Open Questions)

1. **Entropy detection:** Off by default. When enabled, scans the full message (tokenized into candidate strings), not just AC hits. This is intentional — the purpose is catching unknown secret formats with no known prefix.

2. **Code block relaxation:** Removed. It's a bypass vector. Allowlist handles legitimate placeholders.

3. **Outbound scope:** Channel output + external-facing tools. NOT LLM provider calls (agent should freely see secrets) and NOT memory storage (internal).

4. **Elixir integration:** Not needed. All external I/O goes through Rust channels. Confirm bridge interface doesn't introduce a separate external output path.

---

## Implementation Plan

| Issue | Title | Priority | Scope |
|-------|-------|----------|-------|
| TEZ-??? | Sentinel: Core redaction engine | P1 Urgent | Aho-Corasick + regex + is_ascii fast path, NFKC on outbound, `Cow<str>` output, JWT pattern |
| TEZ-??? | Sentinel: Unicode sanitization pipeline | P1 Urgent | Zero-width strip, NFKC normalize, bidi override handling, emoji preservation, expanded codepoint table |
| TEZ-??? | Sentinel: RedactingChannel\<C\> trait decorator | P2 High | Channel decorator, wiring in run_gateway + start_channels, catch_unwind fail-open |
| TEZ-??? | Sentinel: External-facing tool scanning | P2 High | Tool tagging (external_facing flag), RedactingTool\<T\> decorator, browser type coverage |
| TEZ-??? | Sentinel: Inbound Tower middleware + WS + CatchPanicLayer | P2 High | Tower layer for HTTP, WS frame hook, CatchPanicLayer, metadata field sanitization, layer placement |
| TEZ-??? | Sentinel: Config schema, allowlist, fail mode | P2 High | YAML config, custom patterns, fail mode, allowlist, config security docs |
| TEZ-??? | Sentinel: Observability, alerting, test suite + benchmarks | P2 High | Redaction event logging, metrics, fail-open alerts, criterion benchmarks, unit + integration tests |

---

## References

- Aho-Corasick crate: https://docs.rs/aho-corasick
- Unicode security considerations: https://unicode.org/reports/tr36/
- NFKC normalization: https://unicode.org/reports/tr15/
- Tower middleware: https://docs.rs/tower
- tower-http CatchPanicLayer: https://docs.rs/tower-http/latest/tower_http/catch_panic
- Prompt injection via unicode: https://embracethered.com/blog/posts/2024/unicode-prompt-injection/
