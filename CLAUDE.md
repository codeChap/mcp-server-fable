# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

An MCP (Model Context Protocol) server that wraps **Anthropic Claude Fable 5** — Anthropic's most capable, most expensive model — exposing `plan`, `critique`, and `ask` as MCP tools over stdio. It mirrors the structure of `mcp-server-claude-chat`, but is deliberately purpose-built: Fable is used to plan and critique, then the output is handed to a cheaper model to execute. It is not a general chat wrapper.

## Build & Run

```bash
cargo build --release    # binary: target/release/fable
cargo build              # debug build
cargo run                # dev mode
RUST_LOG=debug cargo run # debug logging (to stderr; stdout is reserved for JSON-RPC)
cargo test               # unit tests
```

## Configuration

Config file: `~/.config/mcp-server-fable/config.toml`

```toml
api_key = "sk-ant-..."        # required
# base_url = "https://api.anthropic.com/v1"   # optional override (gateway/proxy)
# default_model = "claude-fable-5"            # optional
# default_max_tokens = 8192                    # optional
# default_effort = "high"                      # optional: low|medium|high|xhigh|max
```

Loaded at startup in `src/config.rs`; the server fails immediately if the file is missing, `api_key` is empty, or `default_effort` is not a valid effort (the `Effort` enum's deserializer rejects it).

## Auth model (important)

Uses the Anthropic **API key** (`x-api-key` header) — API credits. A Claude Pro/Max subscription cannot be used. `base_url` exists so the same code can target an Anthropic-compatible gateway.

## How Fable differs from the Opus-era shape (vs. claude-chat)

These are the deliberate deviations from `mcp-server-claude-chat`:

- **No sampling params.** `temperature`/`top_p`/`top_k` are rejected by Fable with a 400 — `MessagesRequest` has no such field, and there is no temperature tool argument.
- **Adaptive thinking only.** No `Thinking {enabled, budget_tokens}`. Reasoning is always on; depth is `output_config.effort`, a typed `Effort` enum (`low`/`medium`/`high`/`xhigh`/`max`) advertised as a real JSON Schema enum on each tool. Never send `thinking: {type: "disabled"}` (400 on Fable). The `thinking` request field is a raw JSON value, sent as `{"type":"adaptive","display":"summarized"}` only when a tool asks to show reasoning.
- **Raw chain of thought is never returned.** `thinking` blocks come back empty unless display is summarized; `Display` skips empty-text thinking blocks.
- **Refusals** are a successful HTTP 200 with `stop_reason: "refusal"` and a `stop_details` category/explanation. `Display` surfaces these (with any billed usage from a mid-stream refusal) and does not render an answer. **This is a dedicated Fable server — there is no fallback.** A refusal is never retried on another model, so no `fallbacks` param and no `anthropic-beta` header are ever sent.
- **Cost line.** `Display` appends an estimated USD cost at Fable's sticker rates ($10/$50 per 1M, cache-aware). Accurate here because the serving model is always Fable (no fallback repricing).
- **`max_tokens` is clamped** to Fable's 128K ceiling in `build_request` before the request goes out.
- **HTTP timeout is 600s** (not 300s) — Fable turns can run several minutes at high effort.
- **Purpose-built tools**, not chat clones: `plan` and `critique` use fixed server-side system prompts (`PLAN_SYSTEM`, `CRITIQUE_SYSTEM`).

## Architecture

Five source files, no sub-crates:

- **`main.rs`** — entry point. Loads config, builds `AnthropicClient`, builds `FableServer`, starts rmcp stdio transport.
- **`config.rs`** — TOML config from `~/.config/mcp-server-fable/config.toml`. `api_key` (required), `base_url` (defaulted), `default_model`/`default_max_tokens`/`default_effort` (optional). `default_effort` is an `Effort` enum, so an invalid value is rejected at TOML parse time (no manual validation).
- **`api.rs`** — the `Effort` enum (shared by config/params/requests), `AnthropicClient` (reqwest, generic `request<Req,Resp>()`), API types (`MessagesRequest`, `OutputConfig`, `Message`, `MessagesResponse`, `StopDetails`, `Usage`), and the `Display` formatter (refusal, token + cost lines). `Usage` owns both the cost estimate (`estimated_cost_usd`) and its line formatter (`write_summary`).
- **`server.rs`** — MCP tools via rmcp `#[tool]`/`#[tool_router]`/`#[tool_handler]`. Tools: `plan`, `critique`, `ask`. Shared helpers: `resolve_effort`, `build_messages`, `build_request` (pure request assembly + `max_tokens` clamp, unit-tested), `run` (sends it), and `structured_task` (the shared `plan`/`critique` path: default model + fixed system prompt + work effort). Fixed prompts `PLAN_SYSTEM` / `CRITIQUE_SYSTEM`. Every request goes to the configured Fable model — there is no per-call model override, so the cost line is always at Fable rates by construction.
- **`params.rs`** — serde + `JsonSchema` parameter structs; `#[schemars(description)]` becomes the MCP tool parameter docs.

## Key Constants (`server.rs` / `api.rs`)

- Default model: `claude-fable-5` (`server.rs`)
- Default max tokens: `8192` (`server.rs`)
- Default effort: `high` for `plan`/`critique`, `medium` for `ask` (`server.rs`)
- Efforts: the `Effort` enum — `low`, `medium`, `high`, `xhigh`, `max` (`api.rs`)
- Max output tokens clamp: `128000` (`server.rs`)
- Fable rates for the cost estimate: $10 / $50 per 1M input / output (`api.rs`)
- API base URL: `https://api.anthropic.com/v1` (default in `config.rs`)
- API version header: `2023-06-01` (`api.rs`)
- HTTP timeout: 600 seconds (`api.rs`)

## Adding a New Tool

1. Add a parameter struct to `params.rs` (`Deserialize` + `JsonSchema`).
2. Add a `#[tool(description = "...")]` method inside the `#[tool_router] impl FableServer` block in `server.rs`; assemble the request via the shared `run` helper.
3. Add any new API types to `api.rs` if calling a new endpoint.

## Dependencies

`rmcp` v1.2 for MCP protocol handling, `reqwest` for HTTP. Rust edition 2024.
