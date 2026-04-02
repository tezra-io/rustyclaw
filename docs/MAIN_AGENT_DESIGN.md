# Main Agent Architecture — Design Document

**Status**: Draft
**Author**: Sujeeth / Claude
**Date**: 2026-03-29

---

## 1. Problem Statement

RustyClaw currently treats all agents as **peers** — no agent has a distinguished role, persistent context, or authority over others. This creates several issues:

1. **No central "brain"** — When a user sends a message, BtwRouter routes to whichever AgentServer is configured, but there's no concept of a primary agent that knows the user, tracks ongoing work, and maintains continuity across conversations.

2. **Memory is per-fact, not per-agent** — Hermes extracts individual facts with confidence scores, but there's no cohesive "gist" that gives an agent long-term common sense about the user and their world.

3. **Context window overflows silently** — When conversation history exceeds the LLM's context window, there's no compaction strategy. The agent either truncates or fails.

4. **Delegation lacks hierarchy** — AgentCoordinator matches capabilities, but there's no parent-child authority model. Any agent can delegate to any other agent if ACLs allow.

5. **Cron results have no home** — Scheduled tasks complete in Rust, but there's no designated agent to receive and act on the results.

---

## 2. Desired Architecture: Hub-and-Spoke Model

### 2.1 The Main Agent

A designated **Main Agent** acts as the user's "chief of staff":

```
User → Channel → Gateway → BtwRouter ─┬─ /btw → BtwServer (side-channel)
                                       │
                                       └─ normal → Main Agent (always)
                                                      │
                                              ┌───────┼────────┐
                                              ▼       ▼        ▼
                                          :coding  :research  :devops
                                          (persistent sub-agents)
```

**Main Agent properties:**
- Always running, never killed (`:permanent` restart strategy)
- Receives ALL non-btw inbound messages
- Has authority to delegate to any sub-agent
- Maintains long-term extracted memory (loaded into every context)
- Receives cron job results and sub-agent completion notifications
- One Main Agent per user/workspace (not per channel)

**Main Agent is NOT:**
- An omniscient god agent (it doesn't see sub-agent internals)
- A router (BtwRouter still handles /btw interception)
- A bottleneck (delegates heavy work to sub-agents)

### 2.2 Skills, Not Persistent Sub-Agents

There are **no persistent sub-agents**. Everything except the Main Agent is ephemeral — spawn, execute, return result, die. Sub-agents are created from **skill templates**.

| Concept | What it is | Lifecycle |
|---------|-----------|-----------|
| **Main Agent** | Always running, the brain | `:permanent` — always restart |
| **Skill Template** | YAML+MD file defining an agent type | Static file, not a process |
| **Skill Instance** | Ephemeral agent spawned from a template | Spawn → execute → return result → die |
| **Cron Skill** | Skill triggered on a schedule | Spawn on schedule → execute → report to Main Agent → die |

**The Main Agent never does specialized work itself.** It recognizes what needs to be done and invokes the right skill:

```
User: "Fix the auth bug"
Main Agent thinks: "This needs code. I'll invoke the coding skill."
  → Spawns ephemeral :coding-agent from coding-skill template
  → Passes: task description + relevant context from conversation
  → :coding-agent does the work (reads files, writes code, runs tests)
  → :coding-agent returns summary to Main Agent
  → :coding-agent dies
Main Agent: "The bug is fixed. Here's what changed: ..."
```

**Skill templates are just agent definition files:**
```yaml
---
name: coding-skill
persistent: false
model: anthropic/claude-sonnet-4-6
capabilities: ["code", "shell", "file_operations", "git"]
allowed_tools: [shell, file_read, file_write, file_edit, git_operations]
memory: :isolated
memory_backend: "sqlite"
---
You are a coding agent. You receive a task, execute it thoroughly,
and return a clear summary of what you did and what changed.
```

**Why skills instead of persistent sub-agents:**
- No idle processes consuming memory
- No stale state getting out of sync
- Main Agent is the single source of truth
- Skills are just files — easy to create, version, share, audit
- Scales to unlimited skill types without resource cost

### 2.3 Message Flow

**Simple query (Main Agent handles directly):**
```
User: "What's the weather in Tokyo?"
  → BtwRouter → Main Agent → RustBridge → Agent Loop → LLM → response
  (Elixir is passthrough, minimal overhead)
```

**Complex query (skill invocation):**
```
User: "Deploy staging and check the Linear sprint"
  → BtwRouter → Main Agent
      → Main Agent LLM decides to split:
          → invoke coding-skill("deploy staging") → ephemeral agent → summary
          → invoke pm-skill("check Linear sprint") → ephemeral agent → summary
      → Main Agent combines summaries → response to user
      → Both ephemeral agents die after returning
```

**BTW side-channel (non-blocking):**
```
User: "/btw check disk space"
  → BtwRouter intercepts → BtwServer → RustBridge → response
  (Main Agent not interrupted)
```

**Cron skill execution:**
```
Cron trigger (e.g., daily 9am)
  → Spawn ephemeral agent from daily-standup-skill template
  → Agent runs: checks PRs, Linear tickets, deploy status
  → Agent writes full log to its journal
  → Agent returns summary to Main Agent
  → Main Agent stores summary in its memory + optionally notifies user
  → Ephemeral agent dies
```

---

## 3. Three-Tier Memory Architecture

### 3.1 Overview

```
┌─────────────────────────────────────────────────────┐
│ Tier 1: Extracted Memory (always in context)        │
│                                                     │
│ A compact gist of everything the agent knows.       │
│ Overwritten/refined daily. Gives the model          │
│ "common sense" about the user, projects, patterns.  │
│ Target: fits within ~2K-4K tokens.                  │
└─────────────────────────────────────────────────────┘
                    ▲ compacted from
┌─────────────────────────────────────────────────────┐
│ Tier 2: Daily Memory (structured storage)           │
│                                                     │
│ Full conversation summaries organized by date.      │
│ SQLite or Markdown files. NOT loaded into context   │
│ by default. Agent retrieves on demand:              │
│ "what did we discuss last Tuesday?"                 │
└─────────────────────────────────────────────────────┘
                    ▲ persisted from
┌─────────────────────────────────────────────────────┐
│ Tier 3: Active Context (conversation window)        │
│                                                     │
│ Current conversation history. When this exceeds     │
│ the context limit, trigger compaction:              │
│ - Extract key facts → update Tier 1                 │
│ - Summarize conversation → store in Tier 2          │
│ - Clear active context, reload Tier 1               │
│ - Agent continues with gist as foundation           │
└─────────────────────────────────────────────────────┘
```

### 3.2 Tier 1: Extracted Memory

**What it is:** A single, cohesive document (not individual facts) that captures everything the agent needs to know about the user and their world. Think of it as the agent's "briefing document."

**Contents:**
- User profile (name, role, preferences, communication style)
- Active projects and their status
- Key decisions and their rationale
- Recurring tasks and schedules
- Important contacts and relationships
- Technical context (stack, infrastructure, conventions)
- User's goals and priorities

**How it's maintained:**
- At the end of each day (or each significant conversation), Hermes runs an extraction pass
- The extraction produces an **updated gist**, not individual facts
- The new gist **replaces** the previous one (not appends)
- Confidence decay still applies — stale information fades over time
- Target size: 2K-4K tokens (always fits in context)

**How it's loaded:**
- Injected into the system prompt at the start of every agent interaction
- Part of the `ContextBuilder` / `SystemPromptBuilder` chain
- Main Agent always gets it; sub-agents get a subset relevant to their task

### 3.3 Tier 2: Daily Memory

**What it is:** Structured logs of conversations and outcomes, organized by date.

**Storage options:**
- **SQLite** (current default backend) — queryable, structured
- **Markdown files** — human-readable, git-friendly
- Both can coexist: SQLite for querying, Markdown for export

**Schema:**
```
daily_memory:
  date: 2026-03-29
  conversations:
    - id: conv_123
      channel: telegram
      summary: "Discussed architecture diagrams for RustyClaw..."
      key_decisions:
        - "Keep Mermaid for message flow diagram"
        - "Use Figma for system overview and orchestration"
      delegations:
        - agent: :coding-agent
          task: "Fix arrow directions in Figma"
          result: completed
      duration: 45min
  cron_results:
    - job: "daily-standup-summary"
      result: "3 PRs merged, 1 blocking issue on TEZ-280"
```

**Retrieval:** Agent uses `memory_recall` tool with date-based queries. Not loaded automatically.

### 3.4 Tier 3: Context Window Management

**Current behavior:** No management. Conversation history grows until... it doesn't fit.

**Desired behavior:**

```
Context usage approaching limit (e.g., 80% of model's window)
  │
  ▼
Compaction triggers:
  1. Hermes extracts key facts from current conversation
  2. Updates Tier 1 (extracted memory gist)
  3. Summarizes conversation → stores in Tier 2 (daily memory)
  4. Clears conversation history from active context
  5. Reloads Tier 1 into context
  6. Agent continues — has the gist, can retrieve details if needed
```

**Trigger conditions:**
- Token count exceeds configurable threshold (default: 80% of model's context)
- Explicit user command ("compact your memory")
- End of conversation / session timeout
- Daily cron job (overnight compaction)

---

## 4. Skill Journal Architecture

Every skill instance (cron or ad-hoc) maintains a **journal** — a full log of everything it did. The Main Agent only receives a summary.

### 4.1 Two levels of record-keeping

```
┌─────────────────────────────────────────────────────┐
│ Main Agent Memory                                   │
│                                                     │
│ "2026-03-29 14:30: coding-skill fixed auth bug      │
│  in src/auth/mod.rs. 3 files changed, tests pass."  │
│                                                     │
│ Executive summary only. Enough to know what          │
│ happened. Feeds into gist over time.                │
└─────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────┐
│ Skill Journal (per skill instance)                  │
│                                                     │
│ Full log: files read, approaches tried, failures,   │
│ LLM reasoning, tool calls + outputs, errors,        │
│ final diff, test results — everything.              │
│                                                     │
│ Stored in: ~/.rustyclaw/workspace/journals/         │
│   coding-skill/2026-03-29_143022_auth-bug.md        │
└─────────────────────────────────────────────────────┘
```

### 4.2 Journal structure

Each skill instance writes a journal file:

```
~/.rustyclaw/workspace/journals/
  coding-skill/
    2026-03-29_143022_auth-bug.md       ← ad-hoc invocation
    2026-03-28_091500_refactor-config.md
  daily-standup-skill/
    2026-03-29_090000.md                ← cron invocation
    2026-03-28_090000.md
```

**Journal file contents:**
```markdown
# coding-skill — Fix auth bug
**Date:** 2026-03-29 14:30:22
**Invoked by:** Main Agent
**Task:** Fix the authentication bug in src/auth/mod.rs
**Duration:** 3m 42s
**Status:** completed

## Context received
[task description and context passed by Main Agent]

## Execution log
1. Read src/auth/mod.rs — found expired token check missing
2. Attempted fix: added token expiry validation
3. Ran tests — 2 failures
4. Fixed edge case for refresh tokens
5. Ran tests — all pass

## Files changed
- src/auth/mod.rs (12 lines added, 3 removed)
- src/auth/tests.rs (25 lines added)

## Summary returned to Main Agent
Fixed auth bug: added token expiry validation in src/auth/mod.rs.
Added refresh token edge case handling. All tests pass.
```

### 4.3 Cron skill journals

Cron skills accumulate journals over time. Each run can reference previous journals for continuity:

```
daily-standup-skill spawns at 9am
  → Loads its most recent journal (yesterday's standup)
  → Compares: what was pending yesterday vs what's done today
  → Writes today's journal
  → Returns summary to Main Agent
  → Dies
```

The journal gives cron skills **memory across runs** without keeping a process alive. The skill template defines `memory: :isolated` with its own memory namespace — but the journal is the primary record.

### 4.4 Retrieval

When the user asks Main Agent "what did the coding agent do yesterday?":

```
Main Agent checks its own memory → finds summary
  "2026-03-28: coding-skill refactored config module. 5 files changed."

User: "Show me the details"

Main Agent → reads journal file:
  ~/.rustyclaw/workspace/journals/coding-skill/2026-03-28_091500_refactor-config.md
  → Returns full execution log to user
```

No need to keep the coding agent alive. The journal is the full record.

---

## 5. Current State vs Desired State

### 5.1 What exists today (verified from code)

| Component | Status | Actual Implementation |
|-----------|--------|----------------------|
| AgentServer | Exists | Has `parent_pid`, `child_pids` (MapSet), `accumulated_state`, `delegate_to_child/3`, `report_to_parent/2`. Status: `:idle\|:running\|:initializing\|:stopping`. Health: `:healthy\|:degraded\|:unhealthy`. Max 100 history entries, 20 snapshots. |
| AgentSupervisor | Exists | DynamicSupervisor, no per-agent restart strategy distinction |
| AgentDefinition | Exists | **Already has `persistent: boolean()` and `parent: String.t()`**. Also: `capabilities`, `delegates_to`, `memory: :isolated\|:shared_read\|:shared`, `channels`, `schedule`, `model`, `allowed_tools`. No `role: main` field yet. |
| BtwRouter | Exists | Routes by `agent_name` parameter. No "resolve main agent" logic — caller must provide the name. Fetches `accumulated_state` and `definition` for BTW context. |
| AgentCoordinator | Exists | Capability matching + ACL check |
| Hermes | Exists | Per-fact extraction: `ExtractedFact { key, content, category, confidence }`. Categories: preference, knowledge, context, decision, goal. Consolidation actions: Keep, Forget, Merge, Update. Config: `extraction_turn_threshold: 3`, `max_extractions_per_tick: 10`, `consolidation_interval_hours: 24`. |
| Memory backends | Exists | `MemoryCategory::Core, Daily, Conversation, Custom(String)`. `MemoryEntry` has `confidence: f64`, `source: MemorySource` (Explicit, AutoSave, Extracted, Hydrated), `last_recalled_at`. |
| Context compaction | **Partially exists** | Agent Loop has `auto_compact_history()`: triggers at `DEFAULT_MAX_HISTORY_MESSAGES = 50`, keeps 20 recent, summarizes oldest via LLM (temp 0.2), max 12K chars source → 2K summary. Falls back to deterministic truncation on LLM failure. But: **no token counting**, **no Tier 1 gist injection**, **no Hermes integration with compaction**. |
| Memory recall in context | Exists | `build_context()` fetches top 5 memories by relevance score before each LLM call. But it's query-based (retrieves per-message), not gist-based (persistent context). |
| CronBridge plugin | Exists | No "route results to Main Agent" convention |
| Confidence decay | Exists | TEZ-277, factor=0.95, floor=0.1 |
| Memory consolidation | Exists | TEZ-276, LLM-based deduplication |

### 5.2 What needs to change

**Elixir layer (smaller changes than expected — good foundations exist):**

1. **AgentDefinition** — Add `role: :main | :sub` field. `persistent: true` already exists. `parent` field already exists. Just need role to distinguish the Main Agent.
2. **AgentSupervisor** — Pass `:permanent` restart for `role: :main` and `persistent: true` agents, `:temporary` for ephemeral. Currently all agents use the same strategy.
3. **BtwRouter** — Add `resolve_main_agent/0` that looks up the agent with `role: :main` in the Registry. Currently requires caller to pass `agent_name` explicitly.
4. **AgentServer** — `accumulated_state` already exists. Extend it to store/load Tier 1 gist. Add `inject_gist_into_context/1` before each `run_task` call to RustBridge.
5. **CronBridge** — Route cron results to `resolve_main_agent/0`.
6. **AgentCoordinator** — Main Agent gets implicit `delegates_to: ["*"]` (already a field in AgentDefinition).

**Rust layer:**

7. **Agent Loop** — Token counting per turn. `auto_compact_history` already exists (triggers at 50 messages). Extend to count tokens, not just messages. Integrate with Hermes: on compaction, trigger gist update.
8. **SystemPromptBuilder** (`src/agent/prompt.rs`) — Load Tier 1 gist from `MemoryCategory::Core` and inject into system prompt. Currently `build_context()` does per-query recall of top 5 memories — this stays, but gist is always loaded.
9. **Hermes** — Add `generate_gist` mode alongside existing per-fact extraction. Gist mode: takes all `Core` category memories + recent conversation → produces a single cohesive document → stores as a special `Core` entry with key `_agent_gist`. Runs daily or on compaction.
10. **Memory backends** — Add `store_daily_summary(date, summary)` convenience method. `MemoryCategory::Daily` already exists but isn't structured by date.
11. **Hooks** — Add `on_context_compaction` hook (fires after `auto_compact_history`). Hermes listens → triggers gist update.

**Configuration:**

12. **Agent definition files** — Main Agent: `role: main`, `persistent: true`, `delegates_to: ["*"]`
13. **HermesConfig** — Add `gist_enabled: bool` (default: true), `gist_max_tokens: usize` (default: 4096)
14. **Config** — `compaction_token_threshold: f64` (default: 0.8 = 80% of model context)

---

## 5.3 Parallel Skill Execution

A key capability: Main Agent can spawn **multiple skill instances in parallel** to work on independent tasks concurrently.

### The scenario

```
User: "Here are 50 tasks for the refactor. Execute them."

Main Agent analyzes the 50 tasks:
  → Groups into independent batches (no file conflicts)
  → Spawns 10 coding-skill instances in parallel via Elixir

Elixir (AgentSupervisor):
  :coding-001 → RustBridge → Agent Loop #1 → working on tasks 1-5
  :coding-002 → RustBridge → Agent Loop #2 → working on tasks 6-10
  :coding-003 → RustBridge → Agent Loop #3 → working on tasks 11-15
  ...
  :coding-010 → RustBridge → Agent Loop #10 → working on tasks 46-50

Each instance:
  → Has its own conversation with LLM (independent context)
  → Writes its own journal
  → Returns summary to Main Agent on completion
  → Dies after finishing

Main Agent:
  → Collects all 10 summaries
  → Reports to user: "48/50 tasks complete. 2 failed: [details]"
```

### Why this works through Elixir

1. **BEAM processes are lightweight (~2KB each)** — spawning 10 or 50 is trivial
2. **AgentSupervisor monitors all instances** — if :coding-005 crashes, it's detected and can be retried
3. **No shared state between instances** — each has isolated memory, separate RustBridge calls
4. **Rust Agent Loop is stateless per-request** — 10 concurrent HTTP calls to `/api/agent/run` is fine, Tokio handles them all

### Conflict management

The hard problem: **10 agents editing the same repo simultaneously**.

**Git worktrees solve this.** Each coding-skill instance gets its own worktree:

```
~/.rustyclaw/workspace/worktrees/
  coding-001/ → git worktree of main repo
  coding-002/ → git worktree of main repo
  ...
```

Each agent works in isolation. When done:
- Agent commits to its worktree branch
- Main Agent (or a merge-skill) reconciles branches
- Conflicts are resolved before merging to main

The `GitWorktree` plugin in Elixir already exists for this pattern.

### Invocation API

```
Main Agent calls: invoke_skill_batch([
  { skill: "coding-skill", task: "Refactor auth module", context: {...} },
  { skill: "coding-skill", task: "Add pagination to API", context: {...} },
  { skill: "coding-skill", task: "Fix memory leak in cache", context: {...} },
  ...
], concurrency: 10)

→ Elixir spawns up to 10 concurrent AgentServer processes
→ Each runs independently
→ Results collected as they complete (not waiting for slowest)
→ Main Agent receives: [{ task, status, summary, journal_path }, ...]
```

### Failure handling

**Main Agent handles all failures — not the user.**

```
10 coding agents dispatched
  → 7 succeed → summaries collected
  → 3 fail → failure reports (journal + error) collected

Main Agent reviews failures:
  → "coding-005 failed: compile error in test_auth.rs"
  → Decides: retry with more context? Fix dependency first? Reassign?
  → Spawns new skill instances for retry
  → Repeats until resolved or truly stuck

User sees:
  ✓ "All 50 tasks complete"
  OR "47 done, 3 need your input because [genuine blocker]"
```

The user is shielded from intermediate failures. Main Agent is the manager — it debugs, retries, and escalates to the user only when it genuinely can't resolve the issue.

### Reporting

Results flow to **Main Agent always**. Main Agent decides what the user needs to know:
- Progress updates for long batches ("25/50 complete...")
- Final summary when all done
- Escalation only for genuine blockers
- Full journals available on demand

---

## 6. Implementation Order

### Phase 1: Main Agent Identity
- Add `role: :main | :sub` to AgentDefinition
- BtwRouter resolves Main Agent by role lookup
- Main Agent uses `:permanent` restart strategy
- Simple, no memory changes. Establishes the hub.

### Phase 2: Skill Templates + Invocation
- Define skill template format (reuse AgentDefinition with `role: :sub`)
- Skill directory: `~/.rustyclaw/workspace/skills/` (YAML+MD files)
- Main Agent gets a tool: `invoke_skill(skill_name, task, context)`
- Invoke = spawn ephemeral AgentServer from template → run_task → return summary → die
- Journal writing on skill completion

### Phase 3: Tier 1 — Extracted Memory as Gist
- Modify Hermes to produce cohesive gist document
- Store gist as `~/.rustyclaw/workspace/memory/gist.md`
- SystemPromptBuilder loads gist into every Main Agent interaction
- Daily extraction/overwrite via cron or end-of-conversation hook
- Keep per-fact extraction alongside gist generation

### Phase 4: Tier 2 — Daily Memory + Journals
- Journal infrastructure: `~/.rustyclaw/workspace/journals/<skill>/<date>.md`
- Skill instances write journals automatically
- Main Agent stores summaries in its own Daily memory
- Agent can retrieve journals via file read tool

### Phase 5: Tier 3 — Context Compaction
- Token counting in Agent Loop (extend existing `auto_compact_history`)
- On compaction: trigger Hermes gist update
- Reload gist + keep 20 recent messages (existing behavior)
- `on_context_compaction` hook

### Phase 6: Cron Skills
- Cron skill templates with `schedule` field
- CronBridge spawns ephemeral agent on schedule
- Agent loads previous journal for continuity
- Returns summary to Main Agent
- Journal written automatically

---

## 7. Design Decisions (Resolved)

1. **One Main Agent per workspace.** All channels route to the same Main Agent. The user is one person — the agent should know them holistically. Future: support multiple workspaces (e.g., personal vs work), each with its own Main Agent. Config structure should anticipate this (`workspaces: [{ name, main_agent, ... }]`).

2. **Sub-agents see the gist only, not full memory.** Unnecessary context bloats their window and leaks information across task boundaries. When Main Agent delegates, it passes: (a) the task description, (b) the gist, (c) any task-specific context it chooses to include.

3. **Mid-conversation compaction is acceptable.** Sometimes unavoidable. The gist injection during compaction gives the agent enough "common sense" to continue coherently. The agent won't feel like it forgot — it'll have the gist plus the 20 most recent messages (already implemented in `auto_compact_history`).

4. **Gist lives in a Markdown file.** `~/.rustyclaw/workspace/memory/gist.md` — human-readable, editable by the user, simple to load. Hermes overwrites it on each extraction pass.

5. **Keep both fact extraction and gist generation.** Per-fact extraction (current Hermes) stays for specific recall ("what's the API key?"). Gist generation is a new mode that produces the cohesive briefing document. If one doesn't add value over time, remove it.

6. **Main Agent discovers sub-agents through Elixir, not memory.** At startup and on demand, Main Agent queries the Elixir layer (AgentSupervisor/AgentCoordinator) for available persistent sub-agents and their capabilities. This is live data, not stored memory — ensures accuracy. When the user asks "what agents do I have?", Main Agent calls ListAgentsTool → Elixir responds with current state.

---

## 7. Relationship to Existing Design Docs

- **`docs/ELIXIR_ORCHESTRATION_DESIGN.md`** — Covers the Elixir layer architecture. Main Agent concept should be added as a new section.
- **`docs/session-bridge-design.md`** — Session bridge binds channels to agents. Needs update for Main Agent routing.
- **Hermes (TEZ-275, TEZ-276, TEZ-277)** — Extraction, consolidation, and decay hooks. Phase 2 extends Hermes with gist generation.
- **BTW (TEZ-182)** — BtwRouter/BtwServer. Phase 1 updates routing logic.
