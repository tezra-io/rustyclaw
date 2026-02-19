# MAPLE Spike — Memory Architecture Update for RustyClaw

## Paper Reference
**MAPLE: A Sub-Agent Architecture for Memory, Learning, and Personalization in Agentic AI Systems**
- URL: https://arxiv.org/html/2602.13258v1
- Authors: Submitted to ALA '26 at AAMAS 2026

## Paper Summary

MAPLE argues that "memory" in LLM agents is actually **three distinct capabilities** that need different infrastructure:

### 1. Memory (ℳ) — Storage Infrastructure
- Passive storage and retrieval
- Answers: "What do we know about this user?"
- Three classification lenses:
  - **By Form**: Token-level (text/records), Parametric (weights/LoRA), Latent (KV cache/embeddings)
  - **By Structure**: Flat (1D vector DB), Planar (2D knowledge graph), Hierarchical (3D multi-layer)
  - **By Cognitive Analogy**: Working (session context), Episodic (specific events), Semantic (abstracted facts), Procedural (skills)
- Key insight: episodic captures *what happened*; semantic captures *what we've concluded*. Need both.

### 2. Learning (ℒ) — Intelligence Extraction (ASYNC)
- Runs in background, NOT in request path
- Extracts: Facts, Preferences (explicit + implicit), Experiences
- Three levels:
  - Level 1 — Replay: Store + retrieve successful trajectories
  - Level 2 — Strategy Extraction: Identify patterns across cases
  - Level 3 — Skill Synthesis: Create new capabilities from accumulated experience
- Three timescales:
  - End-of-session processing
  - Periodic batch jobs (cross-user patterns)
  - Event-triggered (critical feedback → immediate update)
- **Symbolic learning** (structured facts in DB), NOT gradient-based (no weight updates)
- No catastrophic forgetting — insights about User A don't overwrite User B

### 3. Personalization (𝒫) — Real-Time Adaptation
- Operates in request path (15-70ms latency)
- Three levels: Surface (names/prefs), Behavioral (interaction patterns), Goal (understanding *why* they're asking)
- User Model Structure: U = (Static attributes, Dynamic state, Behavioral patterns, Predictive elements)
- Mechanism: Context analysis → Selective retrieval → Context assembly → Instruction composition → Generation

### The Closed Loop
```
Memory → retrieves → Personalization → adapts → response → feedback → Learning → writes → Memory
```

### Key Architecture Decisions
- Sub-agents, not monolithic — each has own LLM instance, tools, responsibilities
- Separation of concerns: Memory doesn't decide what to learn, Learning doesn't apply in real-time, Personalization doesn't determine storage
- Asynchronous: Memory + Personalization in request path; Learning in background
- Explicit user models (structured, editable, transparent)
- 14.6% improvement in personalization score vs stateless baseline

---

## Spike Task

Analyze RustyClaw's current memory system and produce a design proposal that incorporates MAPLE's decomposition:

1. **Current State Analysis**: Map RustyClaw's existing memory (AsyncMemoryLedger, hash-chained append-only, compaction) to MAPLE's taxonomy
2. **Gap Analysis**: What RustyClaw has vs what MAPLE recommends
3. **Design Proposal**: How to add Learning + Personalization sub-agents to RustyClaw
   - Where does async learning fit in RustyClaw's tokio event loop?
   - How does the user model integrate with agent definitions (markdown)?
   - How does personalization inject into the ContextBuilder?
4. **RustyClaw-specific constraints**: 
   - Single binary, no external services
   - Agents = markdown data, not code
   - Memory = append-only hash-chained JSONL (tamper-proof)
   - Local-first, privacy-preserving
   - Each agent has own memory directory
5. **Implementation phases**: What to build first, what can wait

### Files to Analyze
- `src/memory/` — current memory implementation
- `src/multi_agent/` — agent definitions, registry, bus
- `src/context/` — ContextBuilder
- `docs/MULTI_AGENT_V2_DESIGN.md` — existing design doc
- `CLAUDE.md` — project context
