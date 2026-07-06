use reqwest::{Client, Method};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::time::Duration;
use thiserror::Error;
use tracing::instrument;

/// API version header value sent on every request — Anthropic requires this.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Fable per-1M-token sticker rates, used for the estimated cost line.
const FABLE_INPUT_PER_1M: f64 = 10.0;
const FABLE_OUTPUT_PER_1M: f64 = 50.0;
/// Cache reads bill ~0.1× input, writes ~1.25× input.
const FABLE_CACHE_READ_PER_1M: f64 = 1.0;
const FABLE_CACHE_WRITE_PER_1M: f64 = 12.5;

/// Reasoning effort accepted by Fable's `output_config.effort`. Serializes and
/// schematizes to the lowercase wire values (`low` … `max`), so the tool schema
/// advertises a real enum and invalid values are rejected before a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// Errors returned by the Anthropic API client.
#[derive(Error, Debug)]
pub enum ApiError {
    #[error("HTTP request failed: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("Anthropic API error ({status}): {body}")]
    Api {
        status: reqwest::StatusCode,
        body: String,
    },
}

/// Shared HTTP client for all Anthropic API calls.
pub struct AnthropicClient {
    api_key: String,
    base_url: String,
    http: Client,
}

impl AnthropicClient {
    /// Create a new client pointing at the given base URL
    /// (typically `https://api.anthropic.com/v1`).
    pub fn new(api_key: String, base_url: String) -> Self {
        Self {
            api_key,
            base_url,
            // Fable turns can run several minutes at high effort — allow the
            // reqwest max rather than claude-chat's 300s.
            http: Client::builder()
                .timeout(Duration::from_secs(600))
                .build()
                .expect("Failed to build reqwest client"),
        }
    }

    /// Unified HTTP request method — handles GET and POST with an optional body.
    /// Adds the `x-api-key` and `anthropic-version` headers Anthropic requires.
    #[instrument(skip(self, body), fields(path = %path))]
    pub async fn request<Req: Serialize, Resp: for<'de> Deserialize<'de>>(
        &self,
        method: Method,
        path: &str,
        body: Option<&Req>,
    ) -> Result<Resp, ApiError> {
        let url = format!("{}{path}", self.base_url);
        let mut builder = self
            .http
            .request(method, &url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION);

        if let Some(b) = body {
            builder = builder.json(b);
        }

        let response = builder.send().await?;

        let status = response.status();
        if !status.is_success() {
            let body = match response.text().await {
                Ok(text) => text,
                Err(e) => format!("<failed to read response body: {e}>"),
            };
            tracing::warn!(status = %status, "API request failed");
            return Err(ApiError::Api { status, body });
        }

        Ok(response.json::<Resp>().await?)
    }
}

// ---------------------------------------------------------------------------
// Messages API types
// ---------------------------------------------------------------------------

/// A request to the `/v1/messages` endpoint.
///
/// Note the Fable-specific shape: there is **no** `temperature` (Fable rejects
/// sampling params with a 400), thinking is adaptive-only (`thinking` is a raw
/// JSON value, sent only to request a summarized display), and reasoning depth is
/// controlled by `output_config.effort`.
#[derive(Serialize)]
pub struct MessagesRequest {
    pub model: String,
    /// Required by the Anthropic API.
    pub max_tokens: u32,
    pub messages: Vec<Message>,
    /// Top-level system prompt (Anthropic keeps this out of the `messages` array).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Adaptive thinking config, e.g. `{"type":"adaptive","display":"summarized"}`.
    /// Omitted entirely to run adaptive with reasoning hidden (Fable's default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
}

/// `output_config` — controls reasoning depth / token spend via `effort`.
#[derive(Serialize)]
pub struct OutputConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
}

/// A single message in a conversation. `content` is either a plain string or an
/// array of content blocks (text, image, …).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub role: String,
    pub content: Value,
}

impl Message {
    /// Create a user message with plain text content.
    pub fn user(text: &str) -> Self {
        Self {
            role: "user".into(),
            content: Value::String(text.into()),
        }
    }
}

/// The response from the `/v1/messages` endpoint. Content blocks are kept as raw
/// JSON values so new block types don't break parsing.
#[derive(Debug, Deserialize)]
pub struct MessagesResponse {
    #[serde(default)]
    pub content: Vec<Value>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// Populated only when `stop_reason == "refusal"`.
    #[serde(default)]
    pub stop_details: Option<StopDetails>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// Structured refusal detail (present only on `stop_reason == "refusal"`).
#[derive(Debug, Deserialize)]
pub struct StopDetails {
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub explanation: Option<String>,
}

/// Token usage statistics.
#[derive(Debug, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
}

impl Usage {
    /// Estimated USD cost at Fable's sticker rates. Accurate for this server —
    /// there is no fallback, so the serving model is always Fable.
    fn estimated_cost_usd(&self) -> f64 {
        let read = self.cache_read_input_tokens.unwrap_or(0) as f64;
        let write = self.cache_creation_input_tokens.unwrap_or(0) as f64;
        (self.input_tokens as f64 * FABLE_INPUT_PER_1M
            + self.output_tokens as f64 * FABLE_OUTPUT_PER_1M
            + read * FABLE_CACHE_READ_PER_1M
            + write * FABLE_CACHE_WRITE_PER_1M)
            / 1_000_000.0
    }

    /// Render the token-usage and estimated-cost lines (no leading newline).
    fn write_summary(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[tokens: {} input + {} output = {} total",
            self.input_tokens,
            self.output_tokens,
            self.input_tokens + self.output_tokens
        )?;
        if let Some(read) = self.cache_read_input_tokens.filter(|&n| n > 0) {
            write!(f, "; {read} cache read")?;
        }
        if let Some(created) = self.cache_creation_input_tokens.filter(|&n| n > 0) {
            write!(f, "; {created} cache write")?;
        }
        write!(f, "]")?;
        write!(f, "\n[cost: ≈ ${:.4} (fable rates)]", self.estimated_cost_usd())
    }
}

impl fmt::Display for MessagesResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A refusal carries no usable answer — surface it (with any billed usage,
        // e.g. a mid-stream refusal) and stop.
        if self.stop_reason.as_deref() == Some("refusal") {
            let (category, explanation) = self
                .stop_details
                .as_ref()
                .map(|d| (d.category.as_deref(), d.explanation.as_deref()))
                .unwrap_or((None, None));
            write!(
                f,
                "⛔ Fable declined this request (category: {}).",
                category.unwrap_or("unknown")
            )?;
            if let Some(exp) = explanation {
                write!(f, " {exp}")?;
            }
            if let Some(usage) = &self.usage {
                write!(f, "\n")?;
                usage.write_summary(f)?;
            }
            return Ok(());
        }

        let mut first = true;
        let sep = |f: &mut fmt::Formatter<'_>, first: &mut bool| -> fmt::Result {
            if !*first {
                writeln!(f)?;
            }
            *first = false;
            Ok(())
        };

        for block in &self.content {
            let btype = block.get("type").and_then(Value::as_str).unwrap_or("");
            match btype {
                "text" => {
                    if let Some(t) = block.get("text").and_then(Value::as_str) {
                        sep(f, &mut first)?;
                        write!(f, "{t}")?;
                    }
                }
                "thinking" => {
                    // Fable returns empty thinking text unless display=summarized —
                    // only surface non-empty reasoning.
                    if let Some(t) = block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        sep(f, &mut first)?;
                        write!(f, "[thinking]\n{t}\n[/thinking]")?;
                    }
                }
                "tool_use" => {
                    sep(f, &mut first)?;
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("?");
                    let input = block.get("input").map(|v| v.to_string()).unwrap_or_default();
                    write!(f, "[tool_use: {name}] {input}")?;
                }
                "" => {}
                other => {
                    sep(f, &mut first)?;
                    write!(f, "[{other}]")?;
                }
            }
        }

        if let Some(reason) = &self.stop_reason {
            write!(f, "\n[stop_reason: {reason}]")?;
        }

        if let Some(usage) = &self.usage {
            write!(f, "\n")?;
            usage.write_summary(f)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resp(content: Vec<Value>, usage: Option<Usage>) -> MessagesResponse {
        MessagesResponse {
            content,
            stop_reason: Some("end_turn".into()),
            stop_details: None,
            usage,
        }
    }

    fn usage(input: u32, output: u32) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }
    }

    #[test]
    fn effort_serializes_lowercase() {
        assert_eq!(serde_json::to_value(Effort::Xhigh).unwrap(), json!("xhigh"));
        assert_eq!(serde_json::to_value(Effort::Max).unwrap(), json!("max"));
    }

    #[test]
    fn display_text_block() {
        let r = resp(
            vec![json!({"type": "text", "text": "The plan."})],
            Some(usage(100, 50)),
        );
        let s = r.to_string();
        assert!(s.contains("The plan."));
        assert!(s.contains("[tokens: 100 input + 50 output = 150 total]"));
        assert!(s.contains("[stop_reason: end_turn]"));
    }

    #[test]
    fn display_cost_line() {
        // 100 input * $10/M + 50 output * $50/M = 0.001 + 0.0025 = 0.0035
        let r = resp(
            vec![json!({"type": "text", "text": "x"})],
            Some(usage(100, 50)),
        );
        assert!(r.to_string().contains("[cost: ≈ $0.0035 (fable rates)]"));
    }

    #[test]
    fn display_refusal_shows_category_not_answer() {
        let r = MessagesResponse {
            content: vec![],
            stop_reason: Some("refusal".into()),
            stop_details: Some(StopDetails {
                category: Some("cyber".into()),
                explanation: Some("declined for safety".into()),
            }),
            usage: None,
        };
        let s = r.to_string();
        assert!(s.contains("⛔ Fable declined"));
        assert!(s.contains("category: cyber"));
        assert!(s.contains("declined for safety"));
        assert!(!s.contains("[tokens"));
    }

    #[test]
    fn display_refusal_with_billed_usage_shows_cost() {
        // A mid-stream refusal still bills the streamed partial.
        let r = MessagesResponse {
            content: vec![],
            stop_reason: Some("refusal".into()),
            stop_details: None,
            usage: Some(usage(30, 10)),
        };
        let s = r.to_string();
        assert!(s.contains("⛔ Fable declined"));
        assert!(s.contains("[tokens: 30 input + 10 output = 40 total]"));
        assert!(s.contains("[cost:"));
    }

    #[test]
    fn display_empty_thinking_is_skipped() {
        let r = resp(
            vec![
                json!({"type": "thinking", "thinking": ""}),
                json!({"type": "text", "text": "answer"}),
            ],
            None,
        );
        let s = r.to_string();
        assert!(!s.contains("[thinking]"));
        assert!(s.contains("answer"));
    }

    #[test]
    fn display_empty_content() {
        let r = MessagesResponse {
            content: vec![],
            stop_reason: None,
            stop_details: None,
            usage: None,
        };
        assert_eq!(r.to_string(), "");
    }
}
