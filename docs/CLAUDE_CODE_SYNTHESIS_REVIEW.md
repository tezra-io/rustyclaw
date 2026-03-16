# Claude Code Review: Tool Synthesis Design
*Reviewer: Claude Code | Date: 2026-03-14*

## VERDICT: REVISE

Architecture is sound. BEAM is the right platform. Security model has a critical `apply/3` bypass and relies on blocklist that will always lag behind. Switch to allowlist-primary. Block import/use/require. Address atom exhaustion.

## Key Findings

### Critical
1. `apply/3` / `Kernel.apply/3` / `:erlang.apply/3` bypass entire blocklist
2. `import File` makes `write/2` callable without prefix, evading string-based detection
3. `@on_load` callbacks execute during compilation, before post-compilation validation
4. `:erlang` module equivalents (`:file`, `:os`) bypass Elixir-level blocks

### High
5. Module compiled into global module table — no process-level isolation
6. Atom exhaustion via `String.to_atom/1` (atoms not GC'd)
7. Missing from blocklist: `Module.create/3`, `:code.load_binary/3`, `EEx.eval_string/3`, `:ets`

### Suggestions (agreed with)
1. Allowlist > blocklist as primary gate
2. Block all `import`/`use`/`require` except explicit allowlist
3. Add output validation + size cap
4. Add "dry run" / preview mode
5. Rate limit synthesis attempts per agent
6. Consider sacrificial Erlang node for true isolation (Phase 5)
7. Specify Rust-side tool discovery mechanism
