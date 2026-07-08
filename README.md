# mcp-server-fable

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](#license)
[![Rust edition 2024](https://img.shields.io/badge/Rust-edition%202024-dea584.svg?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![MCP](https://img.shields.io/badge/MCP-server-6f42c1.svg)](https://modelcontextprotocol.io/)
[![Model: Claude Fable 5](https://img.shields.io/badge/model-Claude%20Fable%205-d97757.svg)](https://www.anthropic.com/)

An MCP (Model Context Protocol) server for **Anthropic Claude Fable 5** — Anthropic's most capable, most expensive model ($10 / $50 per 1M input / output tokens, ~2× Opus). Built in Rust, it exposes Fable as specialized MCP tools (`plan`, `critique`, `ask`) so **any** MCP client can use them.

Fable is **not** a day-to-day chat model here. This server is built for the one thing that justifies the price: using Fable to **plan** and **critique**, then handing the result to a cheaper model (Sonnet / Haiku / Opus) to execute. It works in Claude Code, Claude Desktop, Cursor, custom agents, and any other environment that supports MCP servers over stdio.

Communicates via stdio using JSON-RPC 2.0. Structurally it mirrors `mcp-server-claude-chat`, but is deliberately adapted to Fable's API surface and purpose-built for the plan-then-execute pattern.

## Why mcp-server-fable (vs. Claude Code's built-in advisor)?

Claude Code has a powerful native advisor feature (`/advisor fable` or the `advisor` tool). It lets a fast executor model (typically Sonnet or Haiku) dynamically ask Fable for guidance on hard decisions inside a single session.

This MCP server takes a **complementary approach** that is useful in more situations:

- **Explicit tools with handoff-optimized prompts** — `plan` and `critique` use fixed, carefully written system prompts designed so the output can be passed *verbatim* to a cheaper model. Plans are numbered, unambiguous, include exact paths/signatures, edge cases, acceptance criteria, and out-of-scope notes. Critiques are deliberately coverage-first (report everything; filter downstream).
- **Works everywhere MCP works** — Not limited to Claude Code. Use it in Claude Desktop, Cursor, Windsurf, custom agent frameworks, VS Code MCP extensions, scripts, or any future tool that supports the Model Context Protocol.
- **You control the orchestration** — Call Fable for planning or review exactly when *you* (or your multi-agent system) decide, instead of relying on the model to escalate.
- **Clean composability** — Treat Fable as a reusable specialist service alongside your other MCP servers. Perfect for modern "expensive model for judgment, cheap model for execution" workflows.
- **Correct Fable-specific behavior** — Proper `effort` levels, refusal handling (no silent fallback), long timeouts, and accurate per-call cost reporting at Fable rates.

Many teams are converging on the same pattern the community discovered: use Fable narrowly for architecture, planning, and review, then execute with cheaper models. This server gives you first-class, portable tools for the "Fable parts" of that pattern.

See the "Technical details" section below for the specific Fable API differences that also required a dedicated implementation.

## Subscription vs. API credits

This server uses the **Anthropic API with an API key (API credits)** — the only supported, terms-compliant way to drive Claude from a third-party tool. A Claude Pro/Max subscription is not usable here. Point `base_url` at an Anthropic-compatible gateway if you run one.

## Technical details: Fable API differences

Fable's Messages API differs from the Opus-era shape (this is why a dedicated server was needed rather than reusing a general Claude chat wrapper):

- **No sampling parameters** — `temperature` / `top_p` / `top_k` are rejected with a 400. There is no `temperature` tool argument.
- **Adaptive thinking only** — reasoning is always on; depth is controlled by **`effort`** (`low` / `medium` / `high` / `xhigh` / `max`), not a token budget. The raw chain of thought is never returned; `ask` can request a readable *summary* via `show_reasoning`.
- **Refusals stop, cleanly** — this is a *dedicated Fable server*. Fable's safety classifiers (cyber / bio / model-distillation) can decline a request; that comes back as a successful response with `stop_reason: "refusal"`, surfaced with its category and explanation rather than as an answer. It is never silently retried on a different model.
- **30-day data retention required** — Fable is not available to zero-data-retention orgs; such orgs get a 400 on every request.

Every response also prints an **estimated USD cost** (at Fable's sticker rates), since cost-consciousness is the whole point.

## Tools

| Tool | Description |
|------|-------------|
| `plan` | **Flagship.** Turn a goal (+ optional context) into an executor-ready implementation plan: numbered steps, exact paths/signatures, edge cases, acceptance criteria, out-of-scope notes — written to be handed to a cheaper model and executed verbatim. |
| `critique` | Coverage-first review of code, a diff, or a design. Reports every finding with severity + confidence for downstream filtering. Optional `focus`. |
| `ask` | Raw one-shot query to Fable. Multi-turn history, system prompt, effort, optional reasoning summary. |

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
| `effort` | string | no | `low`/`medium`/`high`/`xhigh`/`max` (default `medium`) |
| `max_tokens` | integer | no | Max tokens to generate |
| `show_reasoning` | boolean | no | Return a summary of Fable's reasoning in a `[thinking]` block |

## Prerequisites

- Rust (edition 2024)
- An Anthropic API key from [console.anthropic.com](https://console.anthropic.com/settings/keys), on an org with ≥30-day data retention

The server expects a config file at `~/.config/mcp-server-fable/config.toml` containing at minimum your `api_key`. See `config.toml.example`.

```toml
api_key = "sk-ant-..."

# Optional overrides:
# base_url = "https://api.anthropic.com/v1"
# default_model = "claude-fable-5"
# default_max_tokens = 8192
# default_effort = "high"   # low | medium | high | xhigh | max
```

The server fails fast at startup if the config is missing, `api_key` is empty, or `default_effort` (if set) is invalid.

## Build

```bash
cargo build --release   # produces target/release/fable
cargo build             # debug build
cargo run               # run in dev mode
RUST_LOG=debug cargo run
cargo test              # unit tests (response formatting, cost, refusal, effort, message building)
```

## Installation & MCP Configuration

### 1. Build the server

```bash
cargo build --release
# The binary will be at: target/release/fable
```

Use the **full absolute path** to `target/release/fable` in all configuration below.

### 2. AI-assisted installation (recommended modern method)

Copy the block below and paste it directly to your AI coding assistant (Claude Code, Cursor, Grok, etc.). The AI will handle cloning (if needed), building, path resolution, and registration for you.

```
Add the mcp-server-fable MCP server for me.

Repository: https://github.com/<your-username>/mcp-server-fable   (update this URL if you have a fork)

Steps to perform:
1. If the repo isn't cloned locally yet, clone it and cd into it.
2. Build the release binary:
     cargo build --release
3. Determine the absolute path to the built binary (target/release/fable).
4. Set up the config directory and file:
     mkdir -p ~/.config/mcp-server-fable
     cp config.toml.example ~/.config/mcp-server-fable/config.toml
   Then edit the config and add your Anthropic API key (api_key = "sk-ant-...").

5. Register it as an MCP server named "fable".

   For Claude Code, run:
     claude mcp add fable -- <ABSOLUTE_PATH_TO>/target/release/fable

   For Claude Desktop or other MCP clients, add this under the "mcpServers" key (use the real absolute path):
{
  "fable": {
    "command": "<ABSOLUTE_PATH_TO>/target/release/fable"
  }
}

After setup, test that the `plan` tool is available and working.
```

### 3. Manual configuration

Claude Desktop or any MCP client (`~/.config/Claude/claude_desktop_config.json` or equivalent):

```json
{
  "mcpServers": {
    "fable": {
      "command": "/absolute/path/to/mcp-server-fable/target/release/fable"
    }
  }
}
```

Claude Code (one-liner):

```bash
claude mcp add fable -- /absolute/path/to/mcp-server-fable/target/release/fable
```

Replace the path with your actual absolute path to the release binary.

## Usage

Once it's registered, an MCP client calls the tools by name — the flagship is `plan`.

### From an MCP client (e.g. Claude Code)

Ask the model to use it, handing over the goal plus whatever context the executor will need:

> Use the fable **plan** tool. goal: "Add a `--json` flag to the CLI that prints results as JSON". context: "Rust `clap` app; output currently goes through `println!` in `src/main.rs`". effort: high

Claude Code issues a `tools/call` for `plan`; Fable returns a numbered, executor-ready plan — exact paths, signatures, edge cases, acceptance criteria — which you then hand to a cheaper model (Sonnet / Haiku) to implement verbatim.

### Raw JSON-RPC over stdio

The same call without a client — a `tools/call` request the server reads on stdin:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
  "name":"plan",
  "arguments":{
    "goal":"Add a --json flag to the CLI that prints results as JSON",
    "context":"Rust clap app; output currently via println! in src/main.rs",
    "effort":"high"
  }}}
```

`plan` and `critique` return their content followed by a token + estimated-cost footer. Here is an **actual** response (from a tiny `ask` probe) showing that footer:

```
BINARY-OK
[stop_reason: end_turn]
[tokens: 21 input + 9 output = 30 total]
[cost: ≈ $0.0007 (fable rates)]
```

Because the server only ever calls Fable, that cost line is always at Fable's $10 / $50 per-1M rates — accurate by construction, not by convention.

## Project Structure

```
src/
  main.rs    - entry point, config loading, stdio transport setup
  server.rs  - MCP tool definitions (plan, critique, ask) + fixed prompts
  api.rs     - Anthropic HTTP client, Effort enum, Messages types, refusal/cost formatter
  params.rs  - tool parameter types with serde + JSON Schema derives
  config.rs  - TOML config loading + effort validation
```

## License

MIT
