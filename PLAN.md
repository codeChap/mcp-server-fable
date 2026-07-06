# mcp-server-fable — Implementation Plan

Handoff spec for building the Fable 5 MCP server. Written to be executed by a
less-expensive coding model (Sonnet/Haiku/Opus). Follow it file by file; every
Fable-specific deviation from the `mcp-server-claude-chat` template is called out
explicitly. When in doubt, mirror `../mcp-server-claude-chat`.

---

## 1. Purpose & scope

Fable 5 (`claude-fable-5`) is Anthropic's top-tier model: ~2× the price of Opus
($10/$50 per 1M input/output tokens). We do **not** use it as a day-to-day chat
model. This server exists to use Fable for exactly the two things that justify its
cost, then hand the result to a cheaper model to execute:

- **Planning** — turn a goal + context into a terse, unambiguous, executor-ready
  implementation plan.
- **Critique** — deep code/design review with high recall.

Plus a raw `ask` escape hatch and `list_models`. This is **not** a clone of
claude-chat; web search and vision are deliberately omitted from v1 (see §8).

Structurally it mirrors `mcp-server-claude-chat`: Rust, `rmcp` crate, stdio
JSON-RPC, TOML config at `~/.config/mcp-server-fable/config.toml`.

- **Package + binary name:** `fable`
- **Config dir:** `mcp-server-fable`
- **Default model:** `claude-fable-5`

---

## 2. Fable 5 API deltas (READ FIRST — these are the whole job)

The claude-chat request shape does NOT work against Fable. Changes:

| Concern | claude-chat (Opus-era) | Fable 5 server |
|---|---|---|
| `temperature` / `top_p` / `top_k` | sent | **REMOVED** — Fable returns 400 if any sampling param is present. Delete the field from `MessagesRequest` entirely. |
| Thinking | `thinking: {type:"enabled", budget_tokens:N}` | **REMOVED.** Thinking is always on (adaptive). Omit the `thinking` field entirely to run adaptive. An explicit `{type:"disabled"}` returns **400** on Fable — never send it. |
| Reasoning depth | `thinking_budget` | `output_config: {effort: "low"|"medium"|"high"|"xhigh"|"max"}` |
| Reasoning visibility | returned as `[thinking]` block | Raw CoT is never returned. To get a readable summary send `thinking: {type:"adaptive", display:"summarized"}`; default (`omitted`) returns empty thinking blocks. |
| Refusals | n/a | Safety classifiers may decline → HTTP **200** with `stop_reason:"refusal"` (not an error). Must check `stop_reason` before reading content. |
| Fallback | n/a | Opt in by default: top-level `fallbacks:[{"model":"claude-opus-4-8"}]` + beta header `anthropic-beta: server-side-fallback-2026-06-01`. On a policy decline the API re-serves on Opus in the same call. |
| Data retention | n/a | Fable requires 30-day retention. A zero-data-retention org gets 400 on **every** request. Document in README; nothing to code. |
| Context / output | — | 1M context, 128K max output (same tokenizer as Opus 4.8). |
| Web search | `chat_with_search` tool | **NOT** in Fable's launch feature set — omit. |

### Resulting request JSON (the target shape)

```json
{
  "model": "claude-fable-5",
  "max_tokens": 8192,
  "system": "…optional…",
  "messages": [{"role":"user","content":"…"}],
  "output_config": {"effort": "high"},
  "fallbacks": [{"model": "claude-opus-4-8"}]
}
```

Send `thinking: {"type":"adaptive","display":"summarized"}` **only** when the tool
call asks for reasoning to be shown; otherwise omit `thinking` entirely.

Beta header `server-side-fallback-2026-06-01` is required whenever `fallbacks` is
present, and only then.

---

## 3. Config (`src/config.rs`)

Copy claude-chat's `config.rs`; change the config dir to `mcp-server-fable` and
extend `Config`:

```rust
pub struct Config {
    pub api_key: String,
    #[serde(default = "default_base_url")] pub base_url: String,          // https://api.anthropic.com/v1
    #[serde(default)] pub default_model: Option<String>,                  // claude-fable-5
    #[serde(default)] pub default_max_tokens: Option<u32>,                // 8192
    #[serde(default)] pub default_effort: Option<String>,                 // "high"
    #[serde(default = "default_enable_fallback")] pub enable_fallback: bool, // true
    #[serde(default = "default_fallback_model")] pub fallback_model: String, // claude-opus-4-8
}
```

Keep the same fail-fast validation (missing file / empty `api_key`). Add a
validation that `default_effort`, if set, is one of the valid efforts (reuse the
validator from §5).

---

## 4. API client (`src/api.rs`)

Start from claude-chat's `api.rs`. Changes:

### 4a. `request` must support beta headers
Add an `anthropic-beta` header. Simplest: give `request` an extra `betas: &[&str]`
param and, when non-empty, set `.header("anthropic-beta", betas.join(","))`. Update
call sites. (Alternative: a dedicated `messages()` method — either is fine.)

### 4b. `MessagesRequest`
- **Delete** `temperature`.
- **Delete** the `Thinking {kind, budget_tokens}` struct and its field. Replace with:
  ```rust
  #[serde(skip_serializing_if = "Option::is_none")] pub thinking: Option<Value>,        // {"type":"adaptive","display":"summarized"} or None
  #[serde(skip_serializing_if = "Option::is_none")] pub output_config: Option<OutputConfig>,
  #[serde(skip_serializing_if = "Option::is_none")] pub fallbacks: Option<Vec<Fallback>>,
  ```
- New structs:
  ```rust
  #[derive(Serialize)] pub struct OutputConfig { #[serde(skip_serializing_if="Option::is_none")] pub effort: Option<String> }
  #[derive(Serialize)] pub struct Fallback { pub model: String }
  ```
- Keep `tools`, `stop_sequences` for forward-compat (skip-if-none).

### 4c. `MessagesResponse` — refusal + fallback awareness
Extend:
```rust
pub struct MessagesResponse {
    #[serde(default)] pub content: Vec<Value>,
    #[serde(default)] pub stop_reason: Option<String>,
    #[serde(default)] pub stop_details: Option<StopDetails>,   // populated only on refusal
    #[serde(default)] pub model: Option<String>,               // which model actually answered
    #[serde(default)] pub usage: Option<Usage>,
}
pub struct StopDetails { #[serde(default)] pub category: Option<String>, #[serde(default)] pub explanation: Option<String> }
```

### 4d. `Display` for `MessagesResponse`
Keep claude-chat's block-walking logic, and add:

1. **Refusal first.** If `stop_reason == "refusal"`, emit a clear line:
   `⛔ Fable declined this request (category: <category>). <explanation>` and, if
   `model` differs from the requested model, note the fallback also declined. Do
   not try to render content blocks as an answer.
2. **`fallback` content block** (`type == "fallback"`): render
   `[fallback: <from.model> declined → <to.model> continued]`.
3. **thinking block with empty text:** skip silently (Fable's default). Only print
   `[thinking]…[/thinking]` when the text is non-empty (summarized display).
4. **Served-by line:** if `model` is present and differs from the requested model,
   append `[served by <model>]`.
5. **Cost line** (new — the point of a cost-conscious server). After the existing
   `[tokens: …]` line, append an estimated USD cost. Formula (sticker rates,
   cache-aware):
   ```
   uncached_in = input_tokens
   cost = (uncached_in * 10.0 + output_tokens * 50.0
           + cache_read.unwrap_or(0) * 1.0            // ~0.1× input
           + cache_write.unwrap_or(0) * 12.5) / 1e6   // ~1.25× input
   ```
   Print `≈ $0.0123` (4 dp). Rates are Fable's; if `model` shows a fallback served
   the turn, this is an over-estimate — note "(fable rates)".

Keep `Usage` as-is; it already has cache fields. Ignore `usage.iterations` in v1
(mention as future for exact per-hop billing).

Keep `ModelsResponse` / `ModelInfo` unchanged.

---

## 5. Tool params (`src/params.rs`)

Four param structs. All share an `effort: Option<String>` and
`max_tokens: Option<u32>`. Validate effort against `["low","medium","high","xhigh","max"]`.

```rust
pub struct AskParams {       // raw one-shot Fable query
    prompt: String,
    system_prompt: Option<String>,
    messages: Option<String>,     // JSON history, user/assistant only (reuse build_messages)
    model: Option<String>,
    effort: Option<String>,
    max_tokens: Option<u32>,
    show_reasoning: Option<bool>, // true → thinking display "summarized"
}
pub struct PlanParams {      // FLAGSHIP
    goal: String,                 // what to build/fix
    context: Option<String>,      // code, constraints, file tree, error output, prior attempts
    effort: Option<String>,       // default from config or "high"
    max_tokens: Option<u32>,
}
pub struct CritiqueParams {
    content: String,              // code / diff / design doc to review
    focus: Option<String>,        // e.g. "correctness", "security", "concurrency"
    effort: Option<String>,
    max_tokens: Option<u32>,
}
// list_models: no params
```

Descriptions should tell the caller these tools are *expensive* and meant for
plan/critique-then-handoff, not chit-chat.

---

## 6. Server + tools (`src/server.rs`)

Mirror claude-chat's structure (`ToolRouter`, `#[tool_router]`, `#[tool_handler]`,
`build_messages`, `resolve_model`, `do_messages`, models cache). Constants:

```rust
const DEFAULT_MODEL: &str = "claude-fable-5";
const DEFAULT_MAX_TOKENS: u32 = 8192;
const DEFAULT_EFFORT: &str = "high";
const FALLBACK_BETA: &str = "server-side-fallback-2026-06-01";
const VALID_EFFORTS: &[&str] = &["low","medium","high","xhigh","max"];
```

Add `fn validate_effort(e: Option<&str>) -> Result<(), McpError>`.

`do_messages` must: attach `fallbacks` (from server config) when `enable_fallback`,
and pass `&[FALLBACK_BETA]` as the beta header in that case (else `&[]`).

### Tools

1. **`ask`** — general Fable access. Build messages from history+prompt, set
   `output_config.effort`, set `thinking` to summarized iff `show_reasoning`.
   Default effort = config/`medium` here (raw calls shouldn't burn max).

2. **`plan`** — flagship. `system_prompt` is a **fixed server-side prompt** (below);
   the user `goal` + `context` become the user message. Default effort = `high`
   (bump to `xhigh` for coding tasks — leave configurable). This is the money tool.

   System prompt (embed as a `const`):
   > You are a senior engineer producing an implementation plan that will be
   > handed to a **less capable model** to execute verbatim. Optimise for that
   > handoff. Output ONLY the plan — no preamble, no restating the request, no
   > "here is". Be unambiguous, not terse-to-the-point-of-vague. Use: (1) a
   > one-line goal restatement, (2) numbered steps in dependency order, each
   > naming exact files/paths, function signatures, and the concrete change,
   > (3) explicit edge cases and error handling to include, (4) acceptance
   > criteria the executor can self-check against, (5) anything to NOT do
   > (out of scope, no refactoring beyond the ask). Assume the executor cannot
   > infer intent — spell out decisions rather than offering options.

3. **`critique`** — coverage-first review. Fixed system prompt (embed as `const`),
   aligned with Anthropic's own code-review guidance (report everything, filter
   downstream):
   > Review the provided material and report every issue you find, including
   > low-confidence and low-severity ones — a downstream step will filter. For
   > each: a one-line description, file/location if applicable, severity
   > (high/med/low), and confidence. Group by severity. Do not fix the code;
   > identify problems. If a `focus` is given, weight it but don't ignore others.
   `focus`, if present, is appended to the user message.

4. **`list_models`** — identical to claude-chat (5-min moka cache).

### ServerHandler / get_info
`Implementation::new("fable", env!("CARGO_PKG_VERSION"))`, instructions describing
the four tools and the expensive-planner intent.

---

## 7. main.rs / lib.rs
- `main.rs`: same as claude-chat but wire the new config fields into
  `FableServer::new(client, default_model, default_max_tokens, default_effort, enable_fallback, fallback_model)`.
- `lib.rs`: `pub mod api; pub mod config; pub mod params; pub mod server;`

---

## 8. Deliberately omitted from v1 (document, don't build)
- **Web search** — not in Fable's launch feature set.
- **Vision** — supported by Fable, but not needed for plan/critique. Easy to add
  later by porting claude-chat's `chat_with_vision` (drop temperature, add effort).
- **Task budgets** (beta `task-budgets-2026-03-13`) — good future add for capping
  spend on long agentic runs; not needed for bounded plan/critique outputs.
- **Streaming** — see risks.

---

## 9. Known risks / decisions to honor
1. **Timeout.** Fable turns can run minutes at high effort. claude-chat uses a
   300s reqwest timeout — **bump to 600s** (reqwest max) for this server. Keep tool
   defaults at `high`, not `max`, and document that `max` may time out on large
   contexts. Streaming is the real fix (v2).
2. **Cost visibility is a feature.** The cost line in Display (§4d) is required, not
   optional — the whole reason this server exists is cost-consciousness.
3. **Fallback default = on.** Ship with `enable_fallback = true`. A user can disable
   it in config, but the default must opt in (per Anthropic guidance a refused
   request otherwise just stops).
4. **Never send `temperature` or `thinking:{disabled}`** — both 400 on Fable.

---

## 10. Supporting files
- `Cargo.toml` — copy claude-chat's deps verbatim (rmcp 1.2, tokio, serde,
  serde_json, schemars, reqwest, anyhow, thiserror, toml, tracing,
  tracing-subscriber, dirs, moka; dev: mockito). `name = "fable"`, edition 2024,
  `[profile.release]` lto+strip.
- `config.toml.example` — api_key + commented optionals incl. `default_effort`,
  `enable_fallback`, `fallback_model`.
- `README.md` — adapt claude-chat's. MUST include: the 30-day-retention
  requirement, the refusal/fallback behavior, the "expensive — use for plan/critique
  then hand off to a cheaper model" framing, and the tool table.
- `CHANGELOG.md`, `.gitignore` (ignore `/target`, `config.toml`), `CLAUDE.md`
  (per-project, short).
- **Update root `../CLAUDE.md`**: add `fable` to the binary-output table (binary
  `fable`), the config path list (`~/.config/mcp-server-fable/config.toml`), and the
  architecture-patterns section (note it mirrors claude-chat but adapts to Fable's
  effort/refusal/fallback surface and adds `plan`/`critique`).

---

## 11. Tests (mirror claude-chat's `#[cfg(test)]` blocks)
- `validate_effort`: accepts the 5 valid values; rejects others.
- `build_messages`: reuse claude-chat's tests (basic, history, rejects system role,
  invalid json).
- `Display`:
  - refusal → shows `⛔` + category, does not render an answer.
  - fallback content block → `[fallback: … → …]` and `[served by claude-opus-4-8]`.
  - empty-text thinking block → not printed.
  - cost line → present and correctly computed for a known input/output.

---

## 12. Build & verify
```bash
cd mcp-server-fable
cargo build --release        # binary: target/release/fable
cargo test                   # unit tests above
RUST_LOG=debug cargo run     # smoke: expects config at ~/.config/mcp-server-fable/config.toml
```
Manual smoke test (needs a funded key + non-ZDR org): call `plan` with a tiny goal,
confirm you get plan-only output, a `[served by …]`/cost line, and no 400.

---

## 13. Executor task checklist (do in order)
1. `Cargo.toml`, `.gitignore`, `src/lib.rs`
2. `src/config.rs` (new fields + effort validation)
3. `src/api.rs` (drop temperature/Thinking; add OutputConfig/Fallback; beta-header
   `request`; refusal/fallback/cost Display)
4. `src/params.rs` (4 structs)
5. `src/server.rs` (constants, validate_effort, 4 tools, fixed plan/critique prompts)
6. `src/main.rs`
7. `config.toml.example`, `README.md`, `CHANGELOG.md`, `CLAUDE.md`
8. Update root `../CLAUDE.md`
9. `cargo test` + `cargo build --release`, fix warnings
