# Hindsight-Guided On-Policy Distillation (OPD) — Design Document

*TEZ-224 | Created: 2026-03-21*
*Inspired by: OpenClaw-RL (arXiv:2603.10165)*

---

## 1. Motivation

RustyClaw's existing trajectory collection pipeline (TEZ-155, `HERMES_INSPIRED_FEATURES_DESIGN.md`) captures ShareGPT JSONL trajectories and feeds them to Atropos for GRPO training. GRPO uses scalar rewards only (+1/−1 per conversation turn).

OpenClaw-RL demonstrates this is insufficient: GRPO alone improves pass rate from 0.17→0.23 after 16 training steps. Their Hindsight-Guided On-Policy Distillation (OPD) technique extracts **token-level directional advantages** from next-state signals, producing dramatically better results when combined with GRPO (**0.17→0.81 in 16 steps**).

The key insight: after the agent takes an action, we can observe the resulting state and use a judge model to generate a "hint" describing what the ideal action *should have been*. By computing the token-level probability difference between the hinted (good) continuation and the original (possibly bad) continuation, we get a dense per-token training signal that GRPO's sparse scalar reward cannot provide.

### Why This Matters for RustyClaw

- **Tool-use agents are hard to train with sparse rewards.** A 10-step tool chain might succeed or fail for reasons invisible in a scalar reward. OPD decomposes the signal to each token.
- **Additive to existing pipeline.** OPD annotations sit alongside existing ShareGPT JSONL files as `.opd.jsonl` — zero changes to the collector or GRPO path.
- **Batch-only initially.** All OPD computation happens offline (post-conversation), so there is zero serving latency impact.

---

## 2. Architecture Overview

```
┌──────────────────────────────────────────────────────────────────┐
│                    Agent Turn Loop (existing)                     │
│                    src/agent/agent.rs                             │
│                                                                  │
│  user_msg → provider.chat() → parse → tool_exec → next turn     │
│      │              │            │          │                    │
│      ▼              ▼            ▼          ▼                    │
│  TrajectoryCollector (existing) ──────────────────────────────>  │
│  Writes ShareGPT JSONL to trajectories/completed/*.jsonl         │
└──────────────────────────────────────────────────────────────────┘
                          │
                          │  (batch, offline)
                          ▼
┌──────────────────────────────────────────────────────────────────┐
│                    OPD Pipeline (NEW)                             │
│                                                                  │
│  ┌─────────────┐    ┌──────────────────┐    ┌─────────────────┐ │
│  │ HintExtractor│──>│EnhancedPromptBuilder│──>│TokenAdvantage   │ │
│  │             │    │                  │    │  Computer        │ │
│  └─────────────┘    └──────────────────┘    └─────────────────┘ │
│        │                    │                       │            │
│  Reads completed    Builds hinted +         Computes per-token  │
│  trajectories       unhinted prompts        logprob deltas      │
│                                                                  │
│  Output: trajectories/opd/*.opd.jsonl                            │
└──────────────────────────────────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────────────┐
│                    Training (existing + enhanced)                 │
│                                                                  │
│  Atropos GRPO (existing scalar rewards)                          │
│      +                                                           │
│  OPD token-level advantages (NEW)                                │
│      =                                                           │
│  Combined signal: A_t = w_grpo * R + w_opd * δ_t                │
└──────────────────────────────────────────────────────────────────┘
```

---

## 3. Module Architecture

### 3.1 Module Structure

```
src/trajectory/
├── mod.rs              # Existing — add pub mod opd
├── collector.rs        # Existing — unchanged
├── sharegpt.rs         # Existing — unchanged
├── rotation.rs         # Existing — unchanged
└── opd/
    ├── mod.rs          # OPD public API: OpdPipeline, OpdConfig
    ├── hint.rs         # HintExtractor — judge model hint generation
    ├── prompt.rs       # EnhancedPromptBuilder — hinted/unhinted prompt pairs
    ├── advantage.rs    # TokenAdvantageComputer — logprob delta computation
    └── types.rs        # OPD data types and JSONL serialization
```

### 3.2 Data Types

```rust
// src/trajectory/opd/types.rs

/// A single OPD annotation for one conversation turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpdAnnotation {
    /// Reference to the source trajectory conversation ID.
    pub conversation_id: String,
    /// Zero-based turn index within the conversation.
    pub turn_index: usize,
    /// The original assistant response text at this turn.
    pub original_response: String,
    /// The hint generated by the judge model.
    pub hint: OpdHint,
    /// Per-token advantage scores (δ_t values).
    pub token_advantages: Vec<TokenAdvantage>,
    /// Metadata about how this annotation was computed.
    pub compute_metadata: OpdComputeMetadata,
}

/// A hint extracted from the next-state observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpdHint {
    /// The hint text describing what the ideal action should have been.
    pub text: String,
    /// Quality score assigned by the judge model (0.0-1.0).
    /// Hints below quality_threshold are discarded.
    pub quality_score: f64,
    /// The judge model used to generate this hint.
    pub judge_model: String,
}

/// Per-token advantage: the directional signal for training.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenAdvantage {
    /// The token string.
    pub token: String,
    /// Log-probability under the hinted (improved) prompt.
    pub logprob_hinted: f64,
    /// Log-probability under the original (unhinted) prompt.
    pub logprob_original: f64,
    /// Directional advantage: logprob_hinted - logprob_original.
    /// Positive = token aligns with the hint direction.
    /// Negative = token diverges from the hint direction.
    pub delta: f64,
}

/// Metadata about the OPD computation for auditability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpdComputeMetadata {
    /// Timestamp when this annotation was computed.
    pub computed_at: String,
    /// Model used for logprob computation.
    pub logprob_model: String,
    /// Total tokens processed for this annotation.
    pub tokens_processed: usize,
    /// Wall-clock time for hint extraction (ms).
    pub hint_extraction_ms: u64,
    /// Wall-clock time for logprob computation (ms).
    pub logprob_computation_ms: u64,
}

/// Metadata for a batch of OPD annotations for one trajectory file.
/// The JSONL file contains one `OpdAnnotation` per line, followed by a
/// final metadata line containing `OpdBatch` stats for completion tracking.
/// Example: `{"complete": true, "source_file": "...", "stats": {...}}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpdBatch {
    /// Source trajectory file path.
    pub source_file: String,
    /// Annotations for individual turns.
    pub annotations: Vec<OpdAnnotation>,
    /// Aggregate statistics.
    pub stats: OpdBatchStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpdBatchStats {
    /// Total turns processed.
    pub turns_processed: usize,
    /// Turns skipped (hint quality below threshold).
    pub turns_skipped: usize,
    /// Average |δ| across all tokens.
    pub mean_abs_delta: f64,
    /// Total cost estimate (USD) for judge + logprob calls.
    pub estimated_cost_usd: f64,
}
```

---

## 4. HintExtractor Module

### 4.1 Purpose

Given a completed conversation trajectory, the HintExtractor generates "hindsight hints" for each assistant turn by examining the subsequent state (next turn's context). The hint describes what the assistant *should* have done differently (or confirms the action was correct).

### 4.2 Data Flow

```
ShareGptConversation (from completed/*.jsonl)
    │
    │  For each assistant turn t:
    │
    ▼
┌──────────────────────────────────────────────────────────┐
│  HintExtractor                                            │
│                                                           │
│  Input:                                                   │
│    - context[0..t]: all turns up to and including turn t  │
│    - context[t+1..]: subsequent turns (the "future")      │
│    - final_outcome: conversation status + metadata        │
│                                                           │
│  Process:                                                 │
│    1. Build hindsight prompt with full context             │
│    2. Call judge model to generate hint                    │
│    3. Parse hint + quality score                           │
│    4. Filter: discard if quality_score < threshold         │
│                                                           │
│  Output: OpdHint { text, quality_score, judge_model }     │
└──────────────────────────────────────────────────────────┘
```

### 4.3 Prompt Template

```rust
// src/trajectory/opd/hint.rs

const HINT_SYSTEM_PROMPT: &str = r#"You are an expert AI agent evaluator. Given a conversation
trajectory and a specific assistant turn, analyze whether the assistant's action was optimal.

You have HINDSIGHT: you can see what happened AFTER this turn. Use that knowledge to generate
a concise hint describing what the ideal action should have been at this point.

Rules:
1. Be specific and actionable — "use shell tool with grep instead of reading the entire file"
2. If the action was already optimal, say so explicitly
3. Rate your hint quality 0.0-1.0 (1.0 = very confident the hint would improve the action)
4. Focus on the DECISION, not the formatting or style
5. Do NOT include specific user data, file paths, API keys, or personal information in your hint. Focus only on the abstract action strategy

Respond in JSON:
{
  "hint": "<your hint text>",
  "quality_score": <0.0-1.0>,
  "was_optimal": <true|false>,
  "reasoning": "<brief explanation>"
}"#;

fn build_hint_prompt(
    conversation: &ShareGptConversation,
    turn_index: usize,
) -> Option<String> {
    let context_before = &conversation.conversations[..=turn_index];
    let context_after = &conversation.conversations[turn_index + 1..];
    let outcome = &conversation.metadata.status;

    // Skip the last assistant turn — no hindsight available.
    // Without future context, the judge has nothing to evaluate against.
    if context_after.is_empty() {
        return None;
    }

    Some(format!(
        "## Conversation Context (up to turn {turn_index})\n\
         {before}\n\n\
         ## What Happened After (hindsight)\n\
         {after}\n\n\
         ## Final Outcome: {outcome}\n\n\
         ## Your Task\n\
         Evaluate the assistant's action at turn {turn_index} and generate a hint \
         for what the ideal action should have been, given what you now know happened after.",
        before = format_turns(context_before),
        after = format_turns(context_after),
    ))
}
```

### 4.4 Quality Filtering

Not all hints are useful. The HintExtractor applies quality filtering:

| Filter | Threshold | Rationale |
|--------|-----------|-----------|
| `quality_score` | ≥ 0.3 | Below this, the judge is not confident the hint improves anything |
| `was_optimal` | Skip if true | No training signal when the action was already correct |
| Hint length | ≥ 10 chars | Too-short hints lack actionable content |
| Parse failure | Skip | If JSON parsing fails, the judge response was malformed |

Skipped turns are counted in `OpdBatchStats.turns_skipped` for monitoring.

### 4.5 Judge Model Strategy

The judge model generates hints. Key design decision:

| Option | Model | Cost | Quality | Latency |
|--------|-------|------|---------|---------|
| A. Same as agent | e.g., claude-sonnet-4-5 | High (~$3/1M in) | Best | Slow |
| B. Cheaper judge | e.g., claude-haiku-4-5 | Low (~$0.25/1M in) | Good enough | Fast |
| C. Local model | e.g., Ollama/Qwen-2.5 | Free | Variable | Depends |

**Recommendation: Option B (cheaper judge) as default, configurable.**

Rationale: Hints are evaluated for quality via `quality_score` — a cheaper model that produces poor hints will be filtered out automatically. Cost savings of 10-12x per hint make Option B sustainable for high-volume trajectory processing. The judge only needs to *describe* what should have been done, not *execute* it.

```rust
// Configuration
pub struct OpdConfig {
    /// Judge model for hint extraction (default: provider's cheapest capable model).
    pub judge_model: String,
    /// Provider to use for judge calls (default: same as agent provider).
    pub judge_provider: Option<String>,
    /// Minimum quality score to keep a hint (default: 0.3).
    pub quality_threshold: f64,
    // ...
}
```

---

## 5. EnhancedPromptBuilder Module

### 5.1 Purpose

For each turn with a valid hint, the EnhancedPromptBuilder constructs two prompt variants:

1. **Hinted prompt**: Original context + hint injected before the assistant turn
2. **Unhinted prompt**: Original context exactly as it was (no hint)

Both prompts share the same prefix tokens and diverge only at the point where the hint is injected. This ensures the logprob delta (§6) isolates the effect of the hint.

### 5.2 Data Flow

```
OpdHint + ShareGptConversation[0..t]
    │
    ▼
┌──────────────────────────────────────────────────────────┐
│  EnhancedPromptBuilder                                    │
│                                                           │
│  Hinted prompt:                                           │
│    [system] [user_1] [gpt_1] ... [user_t]                │
│    [HINT: "use grep instead of cat for large files"]      │
│    → feed to logprob model, record P(token | hinted)     │
│                                                           │
│  Unhinted prompt:                                         │
│    [system] [user_1] [gpt_1] ... [user_t]                │
│    → feed to logprob model, record P(token | unhinted)   │
│                                                           │
│  The assistant response at turn t is the TARGET text      │
│  for which we compute logprobs under both conditions.     │
└──────────────────────────────────────────────────────────┘
```

### 5.3 Hint Injection Format

The hint is injected as a system-level annotation just before the assistant's response position:

```
[Previous context...]
User: List all Python files in this project
[HINT: The project has 500+ files. Use `find . -name "*.py"` instead of `ls -R`. Avoid reading directory listings manually.]
Assistant: I'll use the shell tool to find Python files...
```

The hint is wrapped in a `[HINT: ...]` delimiter so it's distinguishable from actual conversation content. The logprob model sees the hint as additional context that biases its token predictions.

### 5.4 Implementation

```rust
// src/trajectory/opd/prompt.rs

pub struct PromptPair {
    /// Messages with hint injected before the target turn.
    pub hinted_messages: Vec<ChatMessage>,
    /// Messages without hint (original context).
    pub unhinted_messages: Vec<ChatMessage>,
    /// The target response text to compute logprobs for.
    pub target_response: String,
    /// Turn index this pair corresponds to.
    pub turn_index: usize,
}

impl EnhancedPromptBuilder {
    /// Build a hinted/unhinted prompt pair for a specific turn.
    pub fn build_pair(
        conversation: &ShareGptConversation,
        turn_index: usize,
        hint: &OpdHint,
    ) -> PromptPair {
        let turns = &conversation.conversations;

        // Build message history up to (but not including) the target turn
        // Include all turn types: system, human, gpt, tool_call, tool_response.
        // Tool interactions are critical context for a tool-use agent — dropping
        // them produces incorrect logprob comparisons. For logprob providers that
        // don't support tool messages natively, serialize tool turns as text
        // within assistant/user roles as a fallback.
        let context_messages: Vec<ChatMessage> = turns[..turn_index]
            .iter()
            .filter(|t| matches!(t.from.as_str(),
                "system" | "human" | "gpt" | "tool_call" | "tool_response"))
            .map(|t| sharegpt_turn_to_chat_message(t))
            .collect();

        // Unhinted: just the context
        let unhinted_messages = context_messages.clone();

        // Hinted: context + hint as a system message
        let mut hinted_messages = context_messages;
        hinted_messages.push(ChatMessage::system(
            &format!("[HINT: {}]", hint.text)
        ));

        // Target: the actual assistant response at this turn
        let target_response = turns[turn_index].value.clone();

        PromptPair {
            hinted_messages,
            unhinted_messages,
            target_response,
            turn_index,
        }
    }
}
```

---

## 6. TokenAdvantageComputer Module

### 6.1 Purpose

Given a `PromptPair` (hinted and unhinted prompts + target response), the TokenAdvantageComputer feeds both through a logprob-capable model and computes the per-token directional advantage:

```
δ_t(token) = log P(token | hinted_context) − log P(token | unhinted_context)
```

- **δ > 0**: The hint *increases* the probability of this token → the token aligns with the improved action.
- **δ < 0**: The hint *decreases* the probability → the token diverges from the improved action.
- **δ ≈ 0**: The hint has no effect on this token → it's context-independent.

### 6.2 Logprob Provider Strategy

**Critical constraint:** Not all LLM providers expose token-level logprobs. This is the most significant technical risk in the OPD design.

#### Provider Logprob Support Matrix

| Provider | Logprob Support | API Parameter | Notes |
|----------|----------------|---------------|-------|
| OpenAI | Yes | `logprobs: true, top_logprobs: N` | Returns per-token logprobs in response |
| OpenRouter | Yes (passthrough) | `logprobs: true` | Depends on underlying model |
| Anthropic | **No** | N/A | Claude does not expose logprobs |
| Gemini | Partial | `response_logprobs: true` (preview) | Limited availability |
| Ollama (local) | Yes | `logprobs: true` in /api/generate | Full control, best for dev |
| Bedrock | Depends on model | Varies | Llama models support it |
| vLLM (local) | Yes | OpenAI-compatible with `logprobs` | Best for production scale |

**Recommendation:** Use a **dedicated logprob provider** for OPD computation, separate from the agent's primary provider. Default to Ollama for development, vLLM/OpenAI for production.

#### Why a Separate Provider

1. **Anthropic (Claude) doesn't expose logprobs.** If the agent uses Claude, we need a different model for logprob computation anyway.
2. **Cost isolation.** Logprob computation is high-volume (every token of every annotated turn). Using a cheap/local model avoids inflating the agent's provider bill.
3. **Model alignment.** Ideally the logprob model matches or approximates the agent's base model (same tokenizer, similar distribution). In practice, any instruction-tuned model of similar capability provides useful signal.

```rust
// src/trajectory/opd/advantage.rs

pub struct TokenAdvantageComputer {
    /// Provider used for logprob computation (must support logprobs).
    logprob_provider: Arc<dyn Provider>,
    /// Model name to use with the logprob provider.
    logprob_model: String,
}

impl TokenAdvantageComputer {
    /// Compute per-token advantages for a prompt pair.
    pub async fn compute(
        &self,
        pair: &PromptPair,
    ) -> anyhow::Result<Vec<TokenAdvantage>> {
        // 1. Get logprobs for target response under hinted context
        let hinted_logprobs = self.get_logprobs(
            &pair.hinted_messages,
            &pair.target_response,
        ).await?;

        // 2. Get logprobs for target response under unhinted context
        let unhinted_logprobs = self.get_logprobs(
            &pair.unhinted_messages,
            &pair.target_response,
        ).await?;

        // 3. Compute deltas (aligned by token position)
        let advantages: Vec<TokenAdvantage> = hinted_logprobs
            .iter()
            .zip(unhinted_logprobs.iter())
            .map(|(h, u)| TokenAdvantage {
                token: h.token.clone(),
                logprob_hinted: h.logprob,
                logprob_original: u.logprob,
                delta: h.logprob - u.logprob,
            })
            .collect();

        Ok(advantages)
    }
}
```

### 6.3 Provider Trait Extension

The current `Provider` trait does not expose logprobs. OPD requires a new trait method or a separate interface:

```rust
// Proposed extension to src/providers/traits.rs

/// Token-level logprob information returned by providers that support it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenLogprob {
    /// The token string.
    pub token: String,
    /// Log-probability of this token.
    pub logprob: f64,
    /// Byte offset in the response text.
    pub offset: usize,
}

/// Extended response that includes per-token logprobs.
#[derive(Debug, Clone)]
pub struct ChatResponseWithLogprobs {
    pub response: ChatResponse,
    /// Per-token logprobs for the response text.
    /// Only populated when the provider supports logprobs and they were requested.
    pub logprobs: Option<Vec<TokenLogprob>>,
}

// Option A: Extend Provider trait (preferred — keeps it unified)
// Add to Provider trait:
//   async fn chat_with_logprobs(
//       &self,
//       messages: &[ChatMessage],
//       model: &str,
//       temperature: f64,
//       target_text: &str,  // text to compute logprobs for
//   ) -> anyhow::Result<ChatResponseWithLogprobs>;

// Option B: Separate LogprobProvider trait (simpler, less invasive)
pub trait LogprobProvider: Send + Sync {
    /// Compute log-probabilities for each token in `target_text`
    /// given the message context.
    async fn compute_logprobs(
        &self,
        messages: &[ChatMessage],
        target_text: &str,
        model: &str,
    ) -> anyhow::Result<Vec<TokenLogprob>>;
}
```

**Recommendation: Option B (separate `LogprobProvider` trait).** Rationale:
- Most providers don't support logprobs. Adding a method to the main `Provider` trait that most implementations would stub out violates YAGNI.
- The `LogprobProvider` is only used by OPD — keeping it separate means zero impact on the main provider infrastructure.
- Implementations: `OllamaLogprobProvider`, `OpenAiLogprobProvider`, `VllmLogprobProvider`.

### 6.4 Token Alignment

When computing deltas, the hinted and unhinted prompts may cause the model to tokenize the target response differently (different BPE splits). The TokenAdvantageComputer must handle this:

1. **Default: Character-level alignment.** Most logprob APIs (OpenAI, Ollama) don't accept pre-tokenized input — they tokenize the prompt themselves. Since hinted and unhinted prompts differ in length, BPE context shifts will cause different token boundaries for the same response text. Use character byte offsets as the alignment key:
   - For each token in both hinted and unhinted responses, record `(byte_start, byte_end, logprob)`.
   - Compute alignment via longest common subsequence on byte ranges.
   - For overlapping but misaligned tokens, interpolate logprobs proportionally by character overlap.
   - Per-character delta is then aggregated back to token-level granularity for training.
2. **Optimization: Force same tokenization.** If the provider exposes a tokenizer API (e.g., local vLLM/SGLang with `--return-tokens-as-token-ids`), pre-tokenize the target text and request logprobs for exact token IDs. This enables direct `.zip()` comparison — faster and more precise, but only available with local models.
3. **Safety: Skip if alignment fails.** If fewer than 80% of response characters can be aligned between hinted and unhinted outputs, skip this turn (log warning, count in `turns_skipped`).

---

## 7. OPD Pipeline Orchestration

### 7.1 Batch Processing Flow

```
OpdPipeline::run(input_dir, output_dir)
    │
    ├── 1. Scan input_dir/completed/*.jsonl for unprocessed trajectories
    │       (check: no corresponding .opd.jsonl in output_dir)
    │
    ├── 2. For each trajectory file:
    │       ├── Parse ShareGptConversation entries
    │       ├── For each conversation:
    │       │   ├── For each assistant turn:
    │       │   │   ├── HintExtractor.extract_hint(conv, turn_idx)
    │       │   │   ├── Filter: skip if quality < threshold
    │       │   │   ├── EnhancedPromptBuilder.build_pair(conv, turn_idx, hint)
    │       │   │   └── TokenAdvantageComputer.compute(pair)
    │       │   └── Collect OpdAnnotations
    │       └── Write OpdBatch to output_dir/*.opd.jsonl
    │
    └── 3. Log OpdBatchStats summary
```

### 7.2 Configuration

```rust
// src/trajectory/opd/mod.rs

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpdConfig {
    /// Enable OPD processing (default: false).
    #[serde(default)]
    pub enabled: bool,

    /// Judge model for hint extraction.
    /// Default: "claude-haiku-4-5" (cheap, fast, filtered by quality score).
    #[serde(default = "default_judge_model")]
    pub judge_model: String,

    /// Provider name for judge calls (default: same as agent provider).
    #[serde(default)]
    pub judge_provider: Option<String>,

    /// Model for logprob computation (must support logprobs).
    /// Default: "qwen2.5:7b" (via Ollama).
    #[serde(default = "default_logprob_model")]
    pub logprob_model: String,

    /// Provider name for logprob calls.
    /// Default: "ollama".
    #[serde(default = "default_logprob_provider")]
    pub logprob_provider: String,

    /// Minimum hint quality score to keep (0.0-1.0, default: 0.3).
    #[serde(default = "default_quality_threshold")]
    pub quality_threshold: f64,

    /// Output directory for .opd.jsonl files.
    /// Default: same as trajectory output_dir + "/opd/"
    #[serde(default)]
    pub output_dir: Option<PathBuf>,

    /// Maximum concurrent hint extraction requests (default: 4).
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,

    /// GRPO weight in combined training signal (default: 0.3).
    #[serde(default = "default_w_grpo")]
    pub w_grpo: f64,

    /// OPD weight in combined training signal (default: 0.7).
    #[serde(default = "default_w_opd")]
    pub w_opd: f64,
}
```

**Config JSON:**
```json
{
  "trajectory": {
    "enabled": true,
    "output_dir": "~/.rustyclaw/trajectories",
    "opd": {
      "enabled": true,
      "judge_model": "claude-haiku-4-5",
      "logprob_model": "qwen2.5:7b",
      "logprob_provider": "ollama",
      "quality_threshold": 0.3,
      "max_concurrent": 4,
      "w_grpo": 0.3,
      "w_opd": 0.7
    }
  }
}
```

### 7.3 File Layout

```
~/.rustyclaw/trajectories/
├── completed/
│   ├── traj-2026-03-21-001.jsonl       # ShareGPT trajectories (existing)
│   └── traj-2026-03-21-002.jsonl
├── failed/
│   └── traj-2026-03-21-001.jsonl       # Failed trajectories (existing)
└── opd/
    ├── traj-2026-03-21-001.opd.jsonl   # OPD annotations for traj-001
    └── traj-2026-03-21-002.opd.jsonl   # OPD annotations for traj-002
```

The `.opd.jsonl` filename mirrors the source trajectory filename with `.opd` inserted before `.jsonl`. This makes it trivial to match annotations to source trajectories.

---

## 8. Integration with Existing Pipeline

### 8.1 TrajectoryCollector — No Changes

The existing `TrajectoryCollector` is unchanged. It continues to write ShareGPT JSONL files via its mpsc channel + background writer. OPD is a strictly downstream consumer.

### 8.2 Combined Training Signal

For training, the OPD annotations merge with GRPO scalar rewards:

```
A_t = w_grpo * reward_scalar + w_opd * mean(token_advantages_t)
```

Where:
- `reward_scalar`: +1 (completed) or −1 (failed/truncated) from conversation status
- `token_advantages_t`: vector of δ values from `OpdAnnotation.token_advantages`
- `mean(token_advantages_t)`: average δ across all tokens at turn t (scalar summary for GRPO-compatible trainers)
- `w_grpo`, `w_opd`: configurable weights (default 0.3, 0.7 per OpenClaw-RL findings)

### 8.3 Training Framework Compatibility

**Question: Can Atropos GRPO accept per-token advantages?**

Atropos expects scalar rewards per response. Two integration paths:

| Path | Approach | Complexity | Signal Quality |
|------|----------|------------|----------------|
| A. Scalar summary | `mean(δ_t)` as additional reward term | Low | Lossy — loses token-level granularity |
| B. Custom training loop | Direct per-token advantage in loss function | High | Full — preserves all signal |
| C. Hybrid | Scalar for GRPO, per-token for separate SFT loss | Medium | Good — best of both |

**Recommendation: Start with Path A (scalar summary), upgrade to Path C when validated.**

Path A works immediately with existing Atropos GRPO:
```python
# In Atropos reward function:
def compute_reward(conversation, opd_annotation):
    base_reward = 1.0 if conversation.status == "completed" else -1.0
    opd_signal = mean([t.delta for t in opd_annotation.token_advantages])
    return w_grpo * base_reward + w_opd * opd_signal
```

Path C adds a separate SFT-style loss weighted by token-level δ values:
```python
# Separate loss term using per-token advantages:
# L_opd = -Σ_t δ_t * log P(token_t | context)
# Positive δ → increase probability; negative δ → decrease probability
```

---

## 9. Open Questions — Answers

### 9.a Logprob Access

**Q: Which providers support token-level logprobs? Need local model?**

**A:** See §6.2 matrix. OpenAI, OpenRouter (passthrough), Ollama, and vLLM support logprobs. **Anthropic (Claude) does not.** Since the primary agent likely runs on Claude, a separate logprob provider is required.

**Recommendation:** Default to Ollama with a 7B model (e.g., Qwen-2.5:7B) for development. For production, use vLLM serving the same base model family the agent was fine-tuned from. The logprob model doesn't need to be identical to the agent model — it just needs a compatible tokenizer and similar enough distribution to produce meaningful directional signals.

### 9.b Judge Model

**Q: Same as agent or cheaper? Cost vs quality.**

**A:** Cheaper. See §4.5. The judge only describes what should have been done — it doesn't need to be as capable as the agent itself. Quality filtering (`quality_score ≥ 0.3`) automatically discards unhelpful hints from weaker judges. Claude Haiku 4.5 at ~$0.25/1M input tokens vs Claude Sonnet 4.5 at ~$3/1M = 12x cost savings.

For a typical conversation with 8 assistant turns:
- Judge cost: ~$0.002 per conversation (Haiku)
- Logprob cost: ~$0.00 per conversation (local Ollama)
- **Total OPD cost: ~$0.002 per conversation** (vs ~$0.024 with Sonnet as judge)

### 9.c Integration with Elixir Layer

**Q: OPD batch as Elixir GenServer or Rust?**

**A:** Rust, with Elixir as optional orchestration wrapper.

Rationale:
1. OPD operates on files written by the Rust `TrajectoryCollector`. Keeping the pipeline in Rust avoids cross-layer serialization.
2. The `Provider` trait (and future `LogprobProvider` trait) are Rust-side. OPD needs to call providers directly.
3. OPD is CPU-bound (prompt construction) + I/O-bound (provider API calls) — both well-served by Tokio async.

**Elixir's role (optional):** The Elixir layer can schedule OPD batch runs via `CronBridge` (existing plugin). A thin `OpdScheduler` GenServer calls into the Rust OPD pipeline via the bridge at configured intervals.

```elixir
# elixir/rustyclaw_orchestrator/lib/plugins/opd_scheduler.ex
defmodule RustyClawOrchestrator.Plugins.OpdScheduler do
  use GenServer

  # Called by CronBridge on schedule (e.g., hourly)
  def handle_info(:run_opd_batch, state) do
    RustyClawOrchestrator.RustBridge.call(state.bridge, :opd_run_batch, %{})
    schedule_next(state)
    {:noreply, state}
  end
end
```

### 9.d Training Framework

**Q: Can Atropos GRPO accept per-token advantages, or custom loop?**

**A:** See §8.3. Atropos GRPO accepts scalar rewards only. Start with scalar summary (mean δ), upgrade to hybrid loss when validated.

### 9.e Privacy

**Q: OPD hints and ContentScanner retention policies.**

**A:** OPD hints are derived from trajectory data, which is already subject to `scrub_secrets` processing in the TrajectoryCollector. Additional privacy measures for OPD:

1. **Hint content scanning.** Before writing `.opd.jsonl`, run each hint through `ContentScanner` (existing, §5 of HERMES_INSPIRED_FEATURES_DESIGN.md). If the judge model leaks sensitive information into a hint, it's caught and scrubbed.

2. **Retention policy.** OPD files follow the same retention as trajectory files:
   - Default: no automatic deletion (user manages)
   - Configurable: `opd.retention_days` (delete `.opd.jsonl` files older than N days)
   - File permissions: `0o600` (owner read/write only), inherited from `RotatingWriter`

3. **No PII in hints.** The hint prompt template (§4.3) instructs the judge to focus on *decisions and actions*, not on user content. However, hints may reference user queries contextually. The `scrub_secrets` pass is the safety net.

4. **Opt-in only.** OPD is disabled by default (`opd.enabled: false`). Enabling trajectory collection does NOT automatically enable OPD — they are independently toggled.

---

## 10. Cost and Latency Analysis

### 10.1 Per-Conversation Cost Estimate

Assumptions: 8 assistant turns per conversation, average 200 tokens per turn.

| Component | Model | Input Tokens | Output Tokens | Cost/Conv |
|-----------|-------|-------------|---------------|-----------|
| Hint extraction (8 turns) | Haiku 4.5 | ~8 × 2000 = 16K | ~8 × 150 = 1.2K | ~$0.004 |
| Logprob hinted (8 turns) | Ollama (local) | ~8 × 2000 = 16K | 8 × 200 = 1.6K | $0.00 |
| Logprob unhinted (8 turns) | Ollama (local) | ~8 × 1800 = 14.4K | 8 × 200 = 1.6K | $0.00 |
| **Total** | | **~46.4K** | **~4.4K** | **~$0.004** |

For 1000 conversations/day: **~$4/day** for OPD annotation.

With OpenAI as logprob provider instead of Ollama:

| Component | Model | Cost/Conv |
|-----------|-------|-----------|
| Logprob (16 calls) | GPT-4o-mini | ~$0.005 |
| **Total with OpenAI logprobs** | | **~$0.009** |

For 1000 conversations/day: **~$9/day**.

### 10.2 Latency (Batch Processing)

OPD runs offline. There is **zero impact on agent serving latency**.

Batch processing time per conversation:

| Component | Time (local Ollama, M2) | Time (API, Haiku) |
|-----------|------------------------|-------------------|
| Hint extraction (8 turns, serial) | N/A | ~4s |
| Hint extraction (8 turns, 4 concurrent) | N/A | ~2s |
| Logprob computation (16 calls, serial) | ~8s | N/A |
| Logprob computation (16 calls, 4 concurrent) | ~4s | N/A |
| **Total per conversation** | **~6s** | **~6s** |

For 1000 conversations: ~100 minutes with `max_concurrent: 4`.

### 10.3 Storage

Each `.opd.jsonl` annotation is ~2-5KB per turn (dominated by token-level data). For 8 turns: ~20-40KB per conversation. For 1000 conversations/day: ~20-40MB/day of OPD data.

---

## 11. Architecture Decision Record

### Decision: Batch Processing (Option B) Initially

**Options considered:**

| Option | Description | Pros | Cons |
|--------|-------------|------|------|
| A. Inline | OPD runs during agent conversation | Freshest signal | 6s+ latency per turn, blocks agent |
| B. Batch | OPD runs on completed trajectories offline | Zero latency impact, simpler error handling | Signal is delayed by batch interval |
| C. Streaming | OPD runs in parallel with agent, slight delay | Near-real-time signal | Complex synchronization, partial contexts |

**Decision: Option B (Batch).**

**Rationale:**
1. **Zero serving impact.** Agent conversations are latency-sensitive. Adding 6s per turn for OPD is unacceptable.
2. **Complete context.** Batch processing sees the full conversation including outcome, which produces better hints than partial contexts.
3. **Error isolation.** Judge or logprob provider failures don't affect agent operation.
4. **Simplicity.** A CLI command (`rustyclaw opd run`) or cron schedule is simpler than inline async pipelines.
5. **Matches OpenClaw-RL.** The paper's OPD is also applied post-hoc to completed trajectories.

**Migration path to Option C:** If real-time signal proves necessary for online learning, the `OpdPipeline` can be adapted to consume trajectory events from the mpsc channel (adding a second subscriber) instead of reading JSONL files. The core modules (HintExtractor, EnhancedPromptBuilder, TokenAdvantageComputer) are agnostic to batch vs streaming.

---

## 12. Implementation Plan

### Phase 2ba: OPD Core Types and Config (S)

| File | Change |
|------|--------|
| `src/trajectory/opd/types.rs` | New — OPD data types |
| `src/trajectory/opd/mod.rs` | New — OpdConfig, OpdPipeline public API |
| `src/trajectory/mod.rs` | Add `pub mod opd;` |
| `src/config/schema.rs` | Add `OpdConfig` to `TrajectoryConfig` |

### Phase 2bb: HintExtractor (M)

| File | Change |
|------|--------|
| `src/trajectory/opd/hint.rs` | New — judge model hint generation + quality filtering |

Dependencies: Phase 2ba, existing Provider trait.

### Phase 2bc: LogprobProvider Trait + Ollama Implementation (M)

| File | Change |
|------|--------|
| `src/providers/traits.rs` | Add `LogprobProvider` trait, `TokenLogprob` struct |
| `src/providers/ollama.rs` | Implement `LogprobProvider` for Ollama |
| `src/trajectory/opd/advantage.rs` | New — TokenAdvantageComputer |

Dependencies: Phase 2ba.

### Phase 2bd: EnhancedPromptBuilder + Pipeline Assembly (M)

| File | Change |
|------|--------|
| `src/trajectory/opd/prompt.rs` | New — hinted/unhinted prompt construction |
| `src/trajectory/opd/mod.rs` | Wire OpdPipeline.run() orchestration |

Dependencies: Phase 2bb, 2bc.

### Phase 2be: CLI Integration + OpenAI LogprobProvider (S)

| File | Change |
|------|--------|
| `src/main.rs` | Add `opd run` subcommand |
| `src/providers/openai.rs` | Implement `LogprobProvider` for OpenAI |

Dependencies: Phase 2bd.

---

## 13. Testing Strategy

### 13.1 Unit Tests

| Module | Test Focus |
|--------|------------|
| `opd/types.rs` | Serialization round-trip for all OPD types |
| `opd/hint.rs` | Prompt template construction, quality filtering logic, JSON parsing |
| `opd/prompt.rs` | Hinted vs unhinted prompt construction, hint injection format |
| `opd/advantage.rs` | Delta computation, token alignment, edge cases (empty response, single token) |

### 13.2 Integration Tests

| Scenario | Validates |
|----------|-----------|
| Full pipeline on a canned trajectory | End-to-end: read JSONL → hints → logprobs → .opd.jsonl |
| Quality filtering drops low-score hints | Filtering logic + turns_skipped counter |
| ContentScanner on generated hints | No secrets leak through judge model |
| Missing logprob provider graceful error | Pipeline reports error, doesn't crash |

### 13.3 Mock Providers

For unit testing, mock implementations of `LogprobProvider` return deterministic logprob vectors. This avoids requiring a running Ollama instance for `cargo test`.

---

## 14. Risk Assessment

| Risk | Severity | Mitigation |
|------|----------|------------|
| Logprob provider unavailable | Medium | Graceful fallback: skip OPD, log warning. Trajectory collection unaffected. |
| Judge model produces poor hints | Low | Quality filtering auto-discards. Monitor `turns_skipped` ratio. |
| Token alignment fails between hinted/unhinted | Medium | Fallback to character-level alignment; skip turn if <80% aligned. |
| OPD cost exceeds budget | Low | All cost is from judge calls (configurable model). Local logprob = free. |
| Training framework can't use per-token signal | Low | Start with scalar summary. Full signal preserved in .opd.jsonl for future use. |
| Privacy: hints contain sensitive user data | Medium | ContentScanner on hints + scrub_secrets on source trajectories. |
| Stale OPD annotations after model update | Low | Re-run OPD batch after fine-tuning. Old annotations are still directionally valid. |

---

## 15. Future Extensions

1. **Online OPD (Option C):** Stream OPD computation during conversations for near-real-time training signal.
2. **Multi-turn hints:** Generate hints that consider multi-step strategies, not just single-turn actions.
3. **Self-play OPD:** Agent generates its own hints by replaying conversations with different strategies.
4. **Reward model distillation:** Use OPD token advantages to train a lightweight reward model that replaces the judge + logprob pipeline.
5. **vLLM LogprobProvider:** Add logprob support for self-hosted vLLM instances (highest throughput for production scale).

---

## 16. Summary

| Component | New Files | Modified Files | Complexity |
|-----------|-----------|---------------|------------|
| OPD Types + Config | 2 | 2 | S |
| HintExtractor | 1 | 0 | M |
| LogprobProvider + Ollama impl | 1 | 2 | M |
| EnhancedPromptBuilder + Pipeline | 2 | 0 | M |
| CLI + OpenAI LogprobProvider | 0 | 2 | S |
| **Total** | **6** | **6** | **~1,500 LOC** |

OPD is additive — no existing behavior is modified. The TrajectoryCollector, ShareGPT serialization, and GRPO pipeline all remain unchanged. OPD annotations are written to separate `.opd.jsonl` files and consumed by an enhanced training reward function.
