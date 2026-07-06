# mcp-server-fable

An MCP (Model Context Protocol) server for **Anthropic Claude Fable 5** — Anthropic's most capable, most expensive model ($10 / $50 per 1M input / output tokens, ~2× Opus). Built in Rust, it exposes Fable as MCP tools so any MCP client (Claude Desktop, Claude Code, etc.) can call it.

Fable is **not** a day-to-day chat model here. This server is built for the one thing that justifies the price: using Fable to **plan** and **critique**, then handing the result to a cheaper model (Sonnet / Haiku / Opus) to execute. Communicates via stdio using JSON-RPC 2.0, like the other servers in this collection. Structurally it mirrors `mcp-server-claude-chat`, adapted to Fable's API surface.

## Why a separate server (Fable ≠ Opus API-wise)

Fable's Messages API differs from the Opus-era shape, so this is an adaptation, not a copy:

- **No sampling parameters** — `temperature` / `top_p` / `top_k` are rejected with a 400. There is no `temperature` tool argument.
- **Adaptive thinking only** — reasoning is always on; depth is controlled by **`effort`** (`low` / `medium` / `high` / `xhigh` / `max`), not a token budget. The raw chain of thought is never returned; `ask` can request a readable *summary* via `show_reasoning`.
- **Refusals stop, cleanly** — this is a *dedicated Fable server*. Fable's safety classifiers (cyber / bio / model-distillation) can decline a request; that comes back as a successful response with `stop_reason: "refusal"`, surfaced with its category and explanation rather than as an answer. It is never silently retried on a different model.
- **30-day data retention required** — Fable is not available to zero-data-retention orgs; such orgs get a 400 on every request.

Every response also prints an **estimated USD cost** (at Fable's sticker rates), since cost-consciousness is the whole point.

## Subscription vs. API credits

This server uses the **Anthropic API with an API key (API credits)** — the only supported, terms-compliant way to drive Claude from a third-party tool. A Claude Pro/Max subscription is not usable here. Point `base_url` at an Anthropic-compatible gateway if you run one.

## Tools

| Tool | Description |
|------|-------------|
| `plan` | **Flagship.** Turn a goal (+ optional context) into an executor-ready implementation plan: numbered steps, exact paths/signatures, edge cases, acceptance criteria, out-of-scope notes — written to be handed to a cheaper model and executed verbatim. |
| `critique` | Coverage-first review of code, a diff, or a design. Reports every finding with severity + confidence for downstream filtering. Optional `focus`. |
| `ask` | Raw one-shot query to Fable. Multi-turn history, system prompt, effort, optional reasoning summary. |
| `list_models` | List available Claude models and their IDs (cached 5 minutes). |

### plan

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `goal` | string | yes | What to build or fix |
| `context` | string | no | Relevant code, file tree, constraints, error output, prior attempts |
| `effort` | string | no | `low`/`medium`/`high`/`xhigh`/`max` (default `high`) |
| `max_tokens` | integer | no | Max tokens to generate (server default otherwise) |

### critique

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `content` | string | yes | Code, diff, or design to review |
| `focus` | string | no | Area to weight, e.g. `security`, `concurrency` |
| `effort` | string | no | `low`/`medium`/`high`/`xhigh`/`max` (default `high`) |
| `max_tokens` | integer | no | Max tokens to generate |

### ask

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `prompt` | string | yes | The user message |
| `system_prompt` | string | no | System prompt |
| `messages` | string | no | History as a JSON array of `{role, content}` (roles `user`/`assistant` only) |
| `model` | string | no | Model ID (default: server default, `claude-fable-5`) |
| `effort` | string | no | `low`/`medium`/`high`/`xhigh`/`max` (default `medium`) |
| `max_tokens` | integer | no | Max tokens to generate |
| `show_reasoning` | boolean | no | Return a summary of Fable's reasoning in a `[thinking]` block |

### list_models

No parameters. Results cached for 5 minutes.

## Prerequisites

- Rust (edition 2024)
- An Anthropic API key from [console.anthropic.com](https://console.anthropic.com/settings/keys), on an org with ≥30-day data retention

## Setup

```bash
mkdir -p ~/.config/mcp-server-fable
cp config.toml.example ~/.config/mcp-server-fable/config.toml
# then edit it and set api_key
```

Config (`~/.config/mcp-server-fable/config.toml`):

```toml
api_key = "sk-ant-..."

# Optional:
# base_url = "https://api.anthropic.com/v1"   # or a compatible gateway
# default_model = "claude-fable-5"
# default_max_tokens = 8192
# default_effort = "high"                       # low|medium|high|xhigh|max
```

The server fails fast at startup if the config is missing, `api_key` is empty, or `default_effort` is invalid.

## Build

```bash
cargo build --release   # produces target/release/fable
cargo build             # debug build
cargo run               # run in dev mode
RUST_LOG=debug cargo run
cargo test              # unit tests (response formatting, cost, refusal, effort, message building)
```

## MCP Configuration

Claude Desktop (`~/.config/Claude/claude_desktop_config.json`) or any MCP client:

```json
{
  "mcpServers": {
    "fable": {
      "command": "/media/codechap/4TB/develop/mcps/mcp-server-fable/target/release/fable"
    }
  }
}
```

Claude Code:

```bash
claude mcp add fable -- /media/codechap/4TB/develop/mcps/mcp-server-fable/target/release/fable
```

## Project Structure

```
src/
  main.rs    - entry point, config loading, stdio transport setup
  server.rs  - MCP tool definitions (plan, critique, ask, list_models) + fixed prompts
  api.rs     - Anthropic HTTP client, Effort enum, Messages/Models types, refusal/cost formatter
  params.rs  - tool parameter types with serde + JSON Schema derives
  config.rs  - TOML config loading + effort validation
```

## License

MIT
