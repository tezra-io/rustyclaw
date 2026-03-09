# RustyClaw Commands Reference

This reference is derived from the current CLI surface (`rustyclaw --help`).

Last verified: **February 21, 2026**.

## Top-Level Commands

| Command | Purpose |
|---|---|
| `onboard` | Initialize workspace/config quickly or interactively |
| `agent` | Run interactive chat or single-message mode |
| `gateway` | Start webhook and WhatsApp HTTP gateway |
| `daemon` | Start supervised runtime (gateway + channels + optional heartbeat/scheduler) |
| `service` | Manage user-level OS service lifecycle |
| `doctor` | Run diagnostics and freshness checks |
| `status` | Print current configuration and system summary |
| `estop` | Engage/resume emergency stop levels and inspect estop state |
| `cron` | Manage scheduled tasks |
| `models` | Refresh provider model catalogs |
| `providers` | List provider IDs, aliases, and active provider |
| `channel` | Manage channels and channel health checks |
| `integrations` | Inspect integration details |
| `skills` | List/install/remove skills |
| `migrate` | Import from external runtimes (currently OpenClaw) |
| `config` | Export machine-readable config schema |
| `completions` | Generate shell completion scripts to stdout |
| `hardware` | Discover and introspect USB hardware |
| `peripheral` | Configure and flash peripherals |

## Command Groups

### `onboard`

- `rustyclaw onboard`
- `rustyclaw onboard --interactive`
- `rustyclaw onboard --channels-only`
- `rustyclaw onboard --force`
- `rustyclaw onboard --api-key <KEY> --provider <ID> --memory <sqlite|lucid|markdown|none>`
- `rustyclaw onboard --api-key <KEY> --provider <ID> --model <MODEL_ID> --memory <sqlite|lucid|markdown|none>`
- `rustyclaw onboard --api-key <KEY> --provider <ID> --model <MODEL_ID> --memory <sqlite|lucid|markdown|none> --force`

`onboard` safety behavior:

- If `config.toml` already exists and you run `--interactive`, onboarding now offers two modes:
  - Full onboarding (overwrite `config.toml`)
  - Provider-only update (update provider/model/API key while preserving existing channels, tunnel, memory, hooks, and other settings)
- In non-interactive environments, existing `config.toml` causes a safe refusal unless `--force` is passed.
- Use `rustyclaw onboard --channels-only` when you only need to rotate channel tokens/allowlists.

### `agent`

- `rustyclaw agent`
- `rustyclaw agent -m "Hello"`
- `rustyclaw agent --provider <ID> --model <MODEL> --temperature <0.0-2.0>`
- `rustyclaw agent --peripheral <board:path>`

Tip:

- In interactive chat, you can ask for route changes in natural language (for example “conversation uses kimi, coding uses gpt-5.3-codex”); the assistant can persist this via tool `model_routing_config`.

### `gateway` / `daemon`

- `rustyclaw gateway [--host <HOST>] [--port <PORT>]`
- `rustyclaw daemon [--host <HOST>] [--port <PORT>]`

### `estop`

- `rustyclaw estop` (engage `kill-all`)
- `rustyclaw estop --level network-kill`
- `rustyclaw estop --level domain-block --domain "*.chase.com" [--domain "*.paypal.com"]`
- `rustyclaw estop --level tool-freeze --tool shell [--tool browser]`
- `rustyclaw estop status`
- `rustyclaw estop resume`
- `rustyclaw estop resume --network`
- `rustyclaw estop resume --domain "*.chase.com"`
- `rustyclaw estop resume --tool shell`
- `rustyclaw estop resume --otp <123456>`

Notes:

- `estop` commands require `[security.estop].enabled = true`.
- When `[security.estop].require_otp_to_resume = true`, `resume` requires OTP validation.
- OTP prompt appears automatically if `--otp` is omitted.

### `service`

- `rustyclaw service install`
- `rustyclaw service start`
- `rustyclaw service stop`
- `rustyclaw service restart`
- `rustyclaw service status`
- `rustyclaw service uninstall`

### `cron`

- `rustyclaw cron list`
- `rustyclaw cron add <expr> [--tz <IANA_TZ>] <command>`
- `rustyclaw cron add-at <rfc3339_timestamp> <command>`
- `rustyclaw cron add-every <every_ms> <command>`
- `rustyclaw cron once <delay> <command>`
- `rustyclaw cron remove <id>`
- `rustyclaw cron pause <id>`
- `rustyclaw cron resume <id>`

Notes:

- Mutating schedule/cron actions require `cron.enabled = true`.
- Shell command payloads for schedule creation (`create` / `add` / `once`) are validated by security command policy before job persistence.

### `models`

- `rustyclaw models refresh`
- `rustyclaw models refresh --provider <ID>`
- `rustyclaw models refresh --force`

`models refresh` currently supports live catalog refresh for provider IDs: `openrouter`, `openai`, `anthropic`, `groq`, `mistral`, `deepseek`, `xai`, `together-ai`, `gemini`, `ollama`, `llamacpp`, `sglang`, `vllm`, `astrai`, `venice`, `fireworks`, `cohere`, `moonshot`, `glm`, `zai`, `qwen`, and `nvidia`.

### `doctor`

- `rustyclaw doctor`
- `rustyclaw doctor models [--provider <ID>] [--use-cache]`
- `rustyclaw doctor traces [--limit <N>] [--event <TYPE>] [--contains <TEXT>]`
- `rustyclaw doctor traces --id <TRACE_ID>`

`doctor traces` reads runtime tool/model diagnostics from `observability.runtime_trace_path`.

### `channel`

- `rustyclaw channel list`
- `rustyclaw channel start`
- `rustyclaw channel doctor`
- `rustyclaw channel bind-telegram <IDENTITY>`
- `rustyclaw channel add <type> <json>`
- `rustyclaw channel remove <name>`

Runtime in-chat commands (Telegram/Discord while channel server is running):

- `/models`
- `/models <provider>`
- `/model`
- `/model <model-id>`
- `/new`

Channel runtime also watches `config.toml` and hot-applies updates to:
- `default_provider`
- `default_model`
- `default_temperature`
- `api_key` / `api_url` (for the default provider)
- `reliability.*` provider retry settings

`add/remove` currently route you back to managed setup/manual config paths (not full declarative mutators yet).

### `integrations`

- `rustyclaw integrations info <name>`

### `skills`

- `rustyclaw skills list`
- `rustyclaw skills audit <source_or_name>`
- `rustyclaw skills install <source>`
- `rustyclaw skills remove <name>`

`<source>` accepts git remotes (`https://...`, `http://...`, `ssh://...`, and `git@host:owner/repo.git`) or a local filesystem path.

`skills install` always runs a built-in static security audit before the skill is accepted. The audit blocks:
- symlinks inside the skill package
- script-like files (`.sh`, `.bash`, `.zsh`, `.ps1`, `.bat`, `.cmd`)
- high-risk command snippets (for example pipe-to-shell payloads)
- markdown links that escape the skill root, point to remote markdown, or target script files

Use `skills audit` to manually validate a candidate skill directory (or an installed skill by name) before sharing it.

Skill manifests (`SKILL.toml`) support `prompts` and `[[tools]]`; both are injected into the agent system prompt at runtime, so the model can follow skill instructions without manually reading skill files.

### `migrate`

- `rustyclaw migrate openclaw [--source <path>] [--dry-run]`

### `config`

- `rustyclaw config schema`

`config schema` prints a JSON Schema (draft 2020-12) for the full `config.toml` contract to stdout.

### `completions`

- `rustyclaw completions bash`
- `rustyclaw completions fish`
- `rustyclaw completions zsh`
- `rustyclaw completions powershell`
- `rustyclaw completions elvish`

`completions` is stdout-only by design so scripts can be sourced directly without log/warning contamination.

### `hardware`

- `rustyclaw hardware discover`
- `rustyclaw hardware introspect <path>`
- `rustyclaw hardware info [--chip <chip_name>]`

### `peripheral`

- `rustyclaw peripheral list`
- `rustyclaw peripheral add <board> <path>`
- `rustyclaw peripheral flash [--port <serial_port>]`
- `rustyclaw peripheral setup-uno-q [--host <ip_or_host>]`
- `rustyclaw peripheral flash-nucleo`

## Validation Tip

To verify docs against your current binary quickly:

```bash
rustyclaw --help
rustyclaw <command> --help
```
