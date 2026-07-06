use moka::future::Cache;
use reqwest::Method;
use rmcp::{
    ErrorData as McpError, ServerHandler, handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters, model::*, tool, tool_handler, tool_router,
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;

use crate::api::{
    AnthropicClient, Effort, Message, MessagesRequest, MessagesResponse, ModelsResponse,
    OutputConfig,
};
use crate::params::{AskParams, CritiqueParams, PlanParams};

const DEFAULT_MODEL: &str = "claude-fable-5";
const DEFAULT_MAX_TOKENS: u32 = 8192;
/// Fable's hard output ceiling; requests above it are rejected with a 400, so we clamp.
const MAX_OUTPUT_TOKENS: u32 = 128_000;
/// Effort used by `plan`/`critique` when neither the call nor config sets one.
const WORK_EFFORT: Effort = Effort::High;
/// Effort used by raw `ask` when neither the call nor config sets one.
const ASK_EFFORT: Effort = Effort::Medium;

/// Fixed system prompt for the `plan` tool — the output is written to be executed
/// verbatim by a cheaper model, so it is spelled out rather than terse.
const PLAN_SYSTEM: &str = "You are a senior engineer producing an implementation plan that will be \
    handed to a less capable model to execute verbatim. Optimise for that handoff. Output ONLY the \
    plan — no preamble, no restating the request, no \"here is\". Be unambiguous, not \
    terse-to-the-point-of-vague. Use: (1) a one-line goal restatement, (2) numbered steps in \
    dependency order, each naming exact files/paths, function signatures, and the concrete change, \
    (3) explicit edge cases and error handling to include, (4) acceptance criteria the executor can \
    self-check against, (5) anything to NOT do (out of scope, no refactoring beyond the ask). \
    Assume the executor cannot infer intent — spell out decisions rather than offering options.";

/// Fixed system prompt for the `critique` tool — coverage-first review.
const CRITIQUE_SYSTEM: &str = "Review the provided material and report every issue you find, \
    including low-confidence and low-severity ones — a downstream step will filter. For each issue: \
    a one-line description, the file/location if applicable, a severity (high/medium/low), and your \
    confidence. Group findings by severity, highest first. Do not rewrite or fix the code; identify \
    problems. If a focus area is given, weight it but do not ignore other issue classes.";

/// Roles Anthropic accepts inside the `messages` array. System instructions go
/// in the top-level `system` field, not as a message.
const VALID_ROLES: &[&str] = &["user", "assistant"];

/// The MCP server wrapping Anthropic Claude Fable 5.
#[derive(Clone)]
pub struct FableServer {
    client: Arc<AnthropicClient>,
    default_model: String,
    default_max_tokens: u32,
    default_effort: Option<Effort>,
    models_cache: Cache<(), String>,
    tool_router: ToolRouter<Self>,
}

// ---------------------------------------------------------------------------
// Shared helpers — keep tool methods DRY
// ---------------------------------------------------------------------------

impl FableServer {
    /// Resolve effort: explicit call value → config default → per-tool default.
    fn resolve_effort(&self, requested: Option<Effort>, default: Effort) -> Effort {
        requested.or(self.default_effort).unwrap_or(default)
    }

    /// Build the messages vec from optional history JSON plus the current prompt.
    fn build_messages(history_json: Option<&str>, prompt: &str) -> Result<Vec<Message>, String> {
        let mut messages = Vec::new();

        if let Some(json) = history_json {
            let parsed: Vec<Message> =
                serde_json::from_str(json).map_err(|e| format!("Invalid messages JSON: {e}"))?;
            for m in &parsed {
                if !VALID_ROLES.contains(&m.role.as_str()) {
                    return Err(format!(
                        "Invalid role '{}' in messages — Anthropic accepts only: {}. \
                         Use the system_prompt parameter for system instructions.",
                        m.role,
                        VALID_ROLES.join(", ")
                    ));
                }
            }
            messages.extend(parsed);
        }

        messages.push(Message::user(prompt));
        Ok(messages)
    }

    /// Resolve the model to use for a call, falling back to the server default.
    fn resolve_model(&self, model: Option<&str>) -> String {
        model
            .map(str::to_string)
            .unwrap_or_else(|| self.default_model.clone())
    }

    /// Assemble a Fable Messages request. Pure — no I/O — so the wire shape is
    /// unit-testable. `max_tokens` is clamped to Fable's ceiling.
    fn build_request(
        model: String,
        messages: Vec<Message>,
        system: Option<String>,
        effort: Effort,
        max_tokens: u32,
        thinking: Option<Value>,
    ) -> MessagesRequest {
        MessagesRequest {
            model,
            max_tokens: max_tokens.min(MAX_OUTPUT_TOKENS),
            messages,
            system,
            thinking,
            output_config: Some(OutputConfig {
                effort: Some(effort),
            }),
            tools: None,
            stop_sequences: None,
        }
    }

    /// Send a request and return the formatted result.
    async fn run(
        &self,
        model: String,
        messages: Vec<Message>,
        system: Option<String>,
        effort: Effort,
        max_tokens: u32,
        thinking: Option<Value>,
    ) -> Result<CallToolResult, McpError> {
        let req = Self::build_request(model, messages, system, effort, max_tokens, thinking);
        match self
            .client
            .request::<_, MessagesResponse>(Method::POST, "/messages", Some(&req))
            .await
        {
            Ok(resp) => Ok(CallToolResult::success(vec![Content::text(
                resp.to_string(),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Run a fixed-system-prompt task on the default (Fable) model: resolve the
    /// work effort, send the single-message prompt, format the result. Shared by
    /// `plan` and `critique`.
    async fn structured_task(
        &self,
        system: &'static str,
        prompt: String,
        effort: Option<Effort>,
        max_tokens: Option<u32>,
    ) -> Result<CallToolResult, McpError> {
        self.run(
            self.default_model.clone(),
            vec![Message::user(&prompt)],
            Some(system.to_string()),
            self.resolve_effort(effort, WORK_EFFORT),
            max_tokens.unwrap_or(self.default_max_tokens),
            None,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

#[tool_router]
impl FableServer {
    pub fn new(
        client: AnthropicClient,
        default_model: Option<String>,
        default_max_tokens: Option<u32>,
        default_effort: Option<Effort>,
    ) -> Self {
        let models_cache = Cache::builder()
            .max_capacity(1)
            .time_to_live(Duration::from_secs(300))
            .build();

        Self {
            client: Arc::new(client),
            default_model: default_model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            default_max_tokens: default_max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            default_effort,
            models_cache,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Raw one-shot query to Claude Fable 5 (Anthropic's most capable, most \
                       expensive model). Supports multi-turn history, a system prompt, effort \
                       control, and an optional reasoning summary. Prefer 'plan' or 'critique' for \
                       their intended jobs; use this for anything else. Expensive — not for chit-chat."
    )]
    async fn ask(&self, Parameters(p): Parameters<AskParams>) -> Result<CallToolResult, McpError> {
        debug!(model = ?p.model, "ask tool called");

        let messages = Self::build_messages(p.messages.as_deref(), &p.prompt)
            .map_err(|e| McpError::invalid_params(e, None))?;
        let effort = self.resolve_effort(p.effort, ASK_EFFORT);
        let thinking = if p.show_reasoning.unwrap_or(false) {
            Some(json!({"type": "adaptive", "display": "summarized"}))
        } else {
            None
        };

        self.run(
            self.resolve_model(p.model.as_deref()),
            messages,
            p.system_prompt,
            effort,
            p.max_tokens.unwrap_or(self.default_max_tokens),
            thinking,
        )
        .await
    }

    #[tool(
        description = "Use Fable 5 to produce an implementation plan for a goal, written to be \
                       handed to a cheaper model and executed verbatim: numbered steps, exact \
                       file paths and signatures, edge cases, acceptance criteria, and out-of-scope \
                       notes. This is the intended high-value use of Fable. Expensive."
    )]
    async fn plan(&self, Parameters(p): Parameters<PlanParams>) -> Result<CallToolResult, McpError> {
        debug!("plan tool called");

        let prompt = match p.context {
            Some(ctx) if !ctx.trim().is_empty() => format!("GOAL:\n{}\n\nCONTEXT:\n{}", p.goal, ctx),
            _ => format!("GOAL:\n{}", p.goal),
        };
        self.structured_task(PLAN_SYSTEM, prompt, p.effort, p.max_tokens)
            .await
    }

    #[tool(
        description = "Use Fable 5 to review code, a diff, or a design for issues — coverage-first: \
                       every finding reported with severity and confidence for downstream \
                       filtering. Optionally weight a focus area. Expensive."
    )]
    async fn critique(
        &self,
        Parameters(p): Parameters<CritiqueParams>,
    ) -> Result<CallToolResult, McpError> {
        debug!("critique tool called");

        let prompt = match p.focus {
            Some(focus) if !focus.trim().is_empty() => {
                format!("FOCUS: {}\n\nMATERIAL TO REVIEW:\n{}", focus, p.content)
            }
            _ => format!("MATERIAL TO REVIEW:\n{}", p.content),
        };
        self.structured_task(CRITIQUE_SYSTEM, prompt, p.effort, p.max_tokens)
            .await
    }

    #[tool(description = "List available Claude models and their IDs (cached for 5 minutes).")]
    async fn list_models(&self) -> Result<CallToolResult, McpError> {
        if let Some(cached) = self.models_cache.get(&()).await {
            debug!("list_models: returning cached result");
            return Ok(CallToolResult::success(vec![Content::text(cached.clone())]));
        }

        debug!("list_models: fetching from API");
        match self
            .client
            .request::<(), ModelsResponse>(Method::GET, "/models?limit=1000", None)
            .await
        {
            Ok(resp) => {
                let lines: Vec<String> = resp
                    .data
                    .iter()
                    .map(|m| match &m.display_name {
                        Some(name) => format!("- {} ({})", m.id, name),
                        None => format!("- {}", m.id),
                    })
                    .collect();
                let result = if lines.is_empty() {
                    "No models returned.".to_string()
                } else {
                    lines.join("\n")
                };
                self.models_cache.insert((), result.clone()).await;
                Ok(CallToolResult::success(vec![Content::text(result)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }
}

// ---------------------------------------------------------------------------
// MCP ServerHandler
// ---------------------------------------------------------------------------

#[tool_handler]
impl ServerHandler for FableServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fable", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Claude Fable 5 MCP server — Anthropic's most capable, most expensive model. \
                 Intended for high-value planning and code critique that is then handed to a \
                 cheaper model to execute. Tools: plan (executor-ready implementation plans), \
                 critique (coverage-first review), ask (raw one-shot query), list_models. A \
                 request Fable's safety classifiers decline is surfaced as a refusal (this is a \
                 dedicated Fable server — it does not silently retry on another model).",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- build_messages -------------------------------------------------------

    #[test]
    fn build_messages_basic() {
        let msgs = FableServer::build_messages(None, "hello").unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn build_messages_with_history() {
        let history =
            r#"[{"role": "user", "content": "hi"}, {"role": "assistant", "content": "hey"}]"#;
        let msgs = FableServer::build_messages(Some(history), "next").unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[2].role, "user");
    }

    #[test]
    fn build_messages_rejects_system_role() {
        let history = r#"[{"role": "system", "content": "be nice"}]"#;
        let err = FableServer::build_messages(Some(history), "hi").unwrap_err();
        assert!(err.contains("Invalid role 'system'"));
    }

    #[test]
    fn build_messages_invalid_json() {
        let result = FableServer::build_messages(Some("not json"), "hello");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid messages JSON"));
    }

    // -- build_request (the Fable wire shape) --------------------------------

    #[test]
    fn build_request_wire_shape() {
        let req = FableServer::build_request(
            "claude-fable-5".into(),
            vec![Message::user("hi")],
            Some("sys".into()),
            Effort::Xhigh,
            8192,
            None,
        );
        let v = serde_json::to_value(&req).unwrap();

        // effort lands inside output_config, as the lowercase wire value.
        assert_eq!(v["output_config"]["effort"], json!("xhigh"));
        // Fable rejects sampling params and (on this dedicated server) fallbacks —
        // neither must ever appear on the wire.
        assert!(v.get("temperature").is_none());
        assert!(v.get("top_p").is_none());
        assert!(v.get("fallbacks").is_none());
        // thinking omitted entirely when not requested (runs adaptive by default).
        assert!(v.get("thinking").is_none());
    }

    #[test]
    fn build_request_clamps_max_tokens() {
        let req = FableServer::build_request(
            "claude-fable-5".into(),
            vec![Message::user("hi")],
            None,
            Effort::High,
            999_999,
            None,
        );
        assert_eq!(req.max_tokens, MAX_OUTPUT_TOKENS);
    }

    #[test]
    fn build_request_includes_thinking_when_requested() {
        let thinking = Some(json!({"type": "adaptive", "display": "summarized"}));
        let req = FableServer::build_request(
            "claude-fable-5".into(),
            vec![Message::user("hi")],
            None,
            Effort::Medium,
            100,
            thinking,
        );
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["thinking"]["display"], json!("summarized"));
    }

    // -- end-to-end through the HTTP client (mocked) -------------------------

    #[tokio::test]
    async fn run_posts_to_messages_without_beta_header_and_formats_response() {
        let mut mock_server = mockito::Server::new_async().await;
        let mock = mock_server
            .mock("POST", "/messages")
            // A dedicated Fable server sends no fallback, so no anthropic-beta header.
            .match_header("anthropic-beta", mockito::Matcher::Missing)
            .match_body(mockito::Matcher::PartialJson(
                json!({"output_config": {"effort": "high"}}),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"content":[{"type":"text","text":"PLAN OK"}],
                    "stop_reason":"end_turn",
                    "usage":{"input_tokens":10,"output_tokens":20}}"#,
            )
            .create_async()
            .await;

        let client = AnthropicClient::new("k".into(), mock_server.url());
        let srv = FableServer::new(client, None, Some(1000), None);

        let res = srv
            .plan(Parameters(PlanParams {
                goal: "do x".into(),
                context: None,
                effort: Some(Effort::High),
                max_tokens: None,
            }))
            .await
            .unwrap();

        mock.assert_async().await;

        // Serialize the tool result to inspect its rendered text robustly.
        let json = serde_json::to_string(&res).unwrap();
        assert!(json.contains("PLAN OK"), "response text missing: {json}");
        assert!(json.contains("cost:"), "cost line missing: {json}");
    }
}
