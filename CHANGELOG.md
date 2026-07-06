# Changelog

## 0.1.0 — 2026-07-06

Initial release.

### Added
- MCP server for **Anthropic Claude Fable 5** over stdio (mirrors the structure
  of `mcp-server-claude-chat`, adapted to Fable's API surface).
- Tools:
  - `plan` — turn a goal (+ optional context) into an executor-ready
    implementation plan, written to be handed to a cheaper model and run
    verbatim. Fixed server-side system prompt; default effort `high`.
  - `critique` — coverage-first review of code / diff / design; every finding
    with severity + confidence for downstream filtering. Optional `focus`.
  - `ask` — raw one-shot Fable query with history, system prompt, effort, and an
    optional reasoning summary (`show_reasoning`).
- TOML config at `~/.config/mcp-server-fable/config.toml`: `api_key` (required),
  plus optional `base_url`, `default_model`, `default_max_tokens`,
  `default_effort` (validated as an enum at startup).
- Response formatter that renders text, non-empty thinking summaries, token
  usage, and an **estimated USD cost** line (accurate — the serving model is
  always Fable).

### Fable-specific behaviour
- **No sampling params** — `temperature`/`top_p`/`top_k` are never sent (Fable
  400s on them).
- **Adaptive thinking only** — no `thinking_budget`; reasoning depth is the
  `effort` parameter (`low`/`medium`/`high`/`xhigh`/`max`), sent as
  `output_config.effort`. Raw chain of thought is never returned; `ask` can ask
  for a summary via `show_reasoning` (`thinking.display = "summarized"`).
- **Refusals** — `stop_reason: "refusal"` (a successful HTTP 200) is surfaced
  with its category/explanation, not rendered as an answer. This is a dedicated
  Fable server — a refusal is never silently retried on another model.
- **`effort` is a typed enum** (`low`/`medium`/`high`/`xhigh`/`max`) — advertised
  as a real JSON Schema enum on each tool and validated by deserialization.
- **`max_tokens` clamped** to Fable's 128K ceiling before the request.
- HTTP timeout raised to 600s (Fable turns can run several minutes at high effort).

### Notes
- Uses the Anthropic **API key** (`x-api-key`), i.e. API credits — a Claude
  Pro/Max subscription is intentionally not supported.
- Fable requires ≥30-day data retention; zero-data-retention orgs get a 400 on
  every request.
- Default model `claude-fable-5` (override with `default_model` in config).
- Web search and vision are intentionally omitted from v1 (web search is not in
  Fable's launch feature set; vision is supported and can be added later by
  porting claude-chat's `chat_with_vision`).
