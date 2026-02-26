# Twilio Voice Integration Spike — RustyClaw

**Date:** 2026-02-26  
**Author:** aira-bot  
**Status:** Research complete — recommendation included  
**TEZ:** TEZ-26

---

## Executive Summary

Adding voice call support to RustyClaw via Twilio is **technically feasible** with two distinct integration modes. The simpler mode (Twilio handles STT/TTS) gets us to a working prototype quickly; the full-fidelity mode (raw audio via Media Streams) gives us control over STT/TTS quality and cost.

**Recommendation: Go — start with Text Mode (Gather + Say), upgrade to Media Streams later.**

---

## How Twilio Voice Works

### The Basics

1. A caller dials your Twilio phone number
2. Twilio sends an HTTP webhook to your server (`/voice/incoming` on RustyClaw's gateway)
3. Your server responds with **TwiML** (Twilio Markup Language — XML) that controls what happens
4. Twilio executes your TwiML instructions (play audio, gather speech, transfer, hang up)

### TwiML Primitives

```xml
<!-- Gather speech, then webhook with transcript -->
<Response>
  <Gather input="speech" action="/voice/process" timeout="5" speechTimeout="auto">
    <Say voice="alice">What can I help you with?</Say>
  </Gather>
</Response>

<!-- Redirect after processing -->
<Response>
  <Say>Got it, one moment...</Say>
  <Say>Here's what I found: your answer here</Say>
  <Gather input="speech" action="/voice/process" speechTimeout="auto">
    <Say>Is there anything else?</Say>
  </Gather>
</Response>
```

### Media Streams (WebSocket Streaming)

Twilio can stream raw μ-law encoded audio (8kHz, 8-bit) via WebSocket for real-time processing:

```xml
<Response>
  <Start>
    <Stream url="wss://your-server.com/voice/stream" />
  </Start>
  <Pause length="60"/>
</Response>
```

Your WebSocket server receives audio chunks and can send back control messages.

---

## Integration Options

### Option A: Text Mode (Twilio handles STT/TTS)

**How it works:**
- Twilio records speech with `<Gather input="speech">` and transcribes via Twilio's built-in STT (powered by Google/Microsoft)
- RustyClaw receives the transcript via webhook, runs it through the agent, and responds with TwiML `<Say>` containing the agent's text response
- Twilio TTS (Alice/Polly) speaks the response

**Latency:** 2–5 seconds (Twilio STT + LLM + webhook RTT)

**Pros:**
- Simple: webhook handler + TwiML generation, no audio processing
- Works over regular HTTPS, no WebSocket infra needed
- Low implementation cost (~200 lines of Rust)
- Proven reliability

**Cons:**
- STT/TTS quality limited to Twilio's choices (no Piper/Parakeet reuse)
- Twilio charges for STT minutes on top of call minutes
- `<Say>` is robotic; no custom voice

**Cost:** ~$0.014/min (inbound call) + ~$0.010/min (Twilio STT) = ~$0.024/min

---

### Option B: Media Streams (Raw Audio)

**How it works:**
- Twilio streams raw μ-law 8kHz audio via WebSocket to RustyClaw
- RustyClaw transcodes to 16kHz PCM, runs local STT (Parakeet/Whisper)
- Agent processes transcript, generates response text
- RustyClaw runs local TTS (Piper), encodes back to μ-law, sends back via WebSocket
- Twilio plays the audio to the caller in real time

**Latency:** 800ms–2s (local STT 200ms + LLM 400ms + TTS 200ms + network RTT)

**Pros:**
- Can reuse Voice OS's Parakeet STT and Piper TTS
- Custom voice quality
- Lower operational cost (no Twilio STT charges)
- Real streaming (interrupt mid-sentence possible)

**Cons:**
- Complex: need WebSocket server, audio codec conversion (μ-law ↔ PCM), ring buffer management
- Requires native audio libraries in Rust (possible, but more work)
- Mac mini must be publicly accessible (ngrok/tunnel or fixed IP)
- ~800 lines of Rust + careful async audio pipeline

**Cost:** ~$0.014/min (call only, no Twilio STT)

---

## Latency Analysis

### Round-Trip Breakdown (Option A — Text Mode)

| Stage | Time |
|-------|------|
| Caller speech (avg) | 3s |
| Twilio STT | 500ms |
| Webhook to RustyClaw | 50ms |
| Agent turn (LLM) | 800–2000ms |
| Response back to Twilio | 50ms |
| Twilio TTS start | 200ms |
| **Total perceived delay** | **~1.6–3.3s** |

This is acceptable for voice assistant interaction (similar to Alexa/Siri response times).

### Round-Trip Breakdown (Option B — Media Streams)

| Stage | Time |
|-------|------|
| First audio chunk arrives | 200ms |
| Local STT (Parakeet, streaming) | 200ms |
| Agent turn (LLM) | 800–2000ms |
| Local TTS + encode | 200ms |
| Audio playback start | 100ms |
| **Total perceived delay** | **~1.5–2.7s** |

Marginally better, with potential for interruption/barge-in. More importantly: consistent latency, not dependent on Twilio's STT pipeline.

---

## RustyClaw Integration Design

### Gateway Endpoint (Option A)

Add a `voice` module to `src/gateway/`:

```rust
// POST /voice/incoming — Twilio calls this when a call arrives
async fn voice_incoming(State(ctx): State<GatewayCtx>) -> impl IntoResponse {
    // Return TwiML: greet + gather speech
    let twiml = r#"<?xml version="1.0"?>
<Response>
    <Gather input="speech" action="/voice/process" timeout="5" speechTimeout="auto">
        <Say voice="alice">RustyClaw. How can I help?</Say>
    </Gather>
</Response>"#;
    (StatusCode::OK, [(CONTENT_TYPE, "text/xml")], twiml)
}

// POST /voice/process — Twilio sends transcript here
async fn voice_process(
    State(ctx): State<GatewayCtx>,
    Form(body): Form<TwilioVoiceBody>,
) -> impl IntoResponse {
    let transcript = body.speech_result; // from Twilio
    
    // Validate caller (From number must be in allowed list)
    if !ctx.config.voice.allowed_callers.contains(&body.from) {
        return hang_up("Unauthorized caller");
    }
    
    // Run agent turn
    let response = ctx.agent_run(&transcript).await?;
    
    // Return TwiML with spoken response + gather for next turn
    twiml_say_and_gather(&response)
}
```

### Auth Flow

- Caller ID validation: `Caller` field in Twilio webhook = E.164 number
- Config: `[voice] allowed_callers = ["+15551234567"]`
- Twilio also signs webhooks with `X-Twilio-Signature` (HMAC-SHA1) — validate to prevent spoofing

### Multi-Agent Integration

When a call arrives, RustyClaw can route to the appropriate persistent agent:

```rust
// If agent_name in the URL: /voice/incoming?agent=research-agent
// → route the call's transcript to that specific persistent agent via bus
// Default: route to main agent loop
```

---

## Cost Model

| Usage | Monthly Cost |
|-------|-------------|
| Phone number | $1.00/month |
| 100 calls × 3 min avg (inbound) | $4.20 |
| Twilio STT at 300 min (Option A) | $3.00 |
| **Total (Option A)** | **~$8/month** |
| **Total (Option B, no Twilio STT)** | **~$5/month** |

This is very affordable for personal use.

---

## Infrastructure Requirements

- **Public endpoint**: RustyClaw's gateway must be reachable from the internet
  - Option: Cloudflare Tunnel (free), ngrok, or static IP via home router port-forward
  - The gateway already runs on configurable host:port — just needs exposure
- **Twilio account**: Free trial gives $15 credit, then pay-as-you-go
- **Phone number**: $1/month US number
- **TLS**: Required by Twilio for webhooks — Cloudflare Tunnel handles this automatically

---

## Implementation Plan

### Phase 1: Text Mode (Estimated: 2–3 days)

1. Add `[voice]` config section (`allowed_callers`, `twilio_auth_token`)
2. Add `/voice/incoming` and `/voice/process` webhook handlers to gateway
3. Twilio signature validation middleware
4. TwiML response builder
5. Session state per call (conversation history via call SID)
6. Tests (mock Twilio webhook payloads)

New Linear issues: TEZ-78 (voice channel — text mode)

### Phase 2: Media Streams (Estimated: 1–2 weeks, lower priority)

1. WebSocket handler in gateway
2. μ-law ↔ PCM codec (use `symphonia` or `rubato` crates)
3. Parakeet STT integration (reuse from Voice OS research)
4. Piper TTS integration (reuse from Voice OS research)
5. Ring buffer + streaming pipeline
6. Barge-in / interrupt detection

---

## Go / No-Go

**GO ✅**

Rationale:
- Low cost ($5–8/month for personal use)
- Text mode is simple to implement (~200 lines, 2–3 days)
- Natural fit as a RustyClaw channel alongside Telegram/Discord
- Can reuse Voice OS STT/TTS research for Phase 2
- Enables "call your AI" use case — high personal value
- No new infrastructure needed (Cloudflare Tunnel for public endpoint)

**Risks:**
- Public endpoint exposure (mitigated by Twilio signature validation + caller allowlist)
- Latency on complex queries (set user expectations: "this may take a moment")
- Twilio dependency (can be swapped for Vonage/SignalWire if needed)

---

## References

- Twilio Voice webhooks: https://www.twilio.com/docs/voice/twiml
- Twilio Media Streams: https://www.twilio.com/docs/voice/twiml/stream
- Twilio pricing: https://www.twilio.com/en-us/voice/pricing
- Cloudflare Tunnel: https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/
