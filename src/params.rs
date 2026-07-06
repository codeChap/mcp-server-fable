use schemars::JsonSchema;
use serde::Deserialize;

use crate::api::Effort;

/// Parameters for the `ask` tool — raw one-shot Fable access.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AskParams {
    #[schemars(description = "The user message / prompt to send to Fable 5")]
    pub prompt: String,

    #[schemars(description = "Optional system prompt to set context/behaviour")]
    pub system_prompt: Option<String>,

    #[schemars(
        description = "Full conversation history as a JSON array of {role, content} objects, \
                       where role is \"user\" or \"assistant\". When provided, 'prompt' is \
                       appended as the final user message. Use 'system_prompt' for system \
                       instructions — Anthropic does not accept a \"system\" role here."
    )]
    pub messages: Option<String>,

    #[schemars(
        description = "Model ID. Defaults to the server default (claude-fable-5). \
                       Call the list_models tool for the current set of available models."
    )]
    pub model: Option<String>,

    #[schemars(
        description = "Reasoning effort. Higher costs more tokens. Defaults to 'medium' for raw asks."
    )]
    pub effort: Option<Effort>,

    #[schemars(description = "Maximum tokens to generate (defaults to the server default).")]
    pub max_tokens: Option<u32>,

    #[schemars(
        description = "When true, return a readable summary of Fable's reasoning in a [thinking] \
                       block. The raw chain of thought is never returned; default hides reasoning."
    )]
    pub show_reasoning: Option<bool>,
}

/// Parameters for the `plan` tool — the flagship. Produces an executor-ready plan.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PlanParams {
    #[schemars(
        description = "What to build or fix — the goal the plan should achieve. Be concrete."
    )]
    pub goal: String,

    #[schemars(
        description = "Optional supporting context: relevant code, file tree, constraints, error \
                       output, prior attempts, or API shapes the executor will need."
    )]
    pub context: Option<String>,

    #[schemars(
        description = "Reasoning effort. Defaults to 'high' (use 'xhigh' for hard coding tasks). \
                       Expensive — this is Fable 5."
    )]
    pub effort: Option<Effort>,

    #[schemars(description = "Maximum tokens to generate (defaults to the server default).")]
    pub max_tokens: Option<u32>,
}

/// Parameters for the `critique` tool — coverage-first review.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CritiqueParams {
    #[schemars(
        description = "The material to review: source code, a diff, or a design document."
    )]
    pub content: String,

    #[schemars(
        description = "Optional focus to weight the review, e.g. \"correctness\", \"security\", \
                       \"concurrency\". Other issue classes are still reported."
    )]
    pub focus: Option<String>,

    #[schemars(description = "Reasoning effort. Defaults to 'high'. Expensive — this is Fable 5.")]
    pub effort: Option<Effort>,

    #[schemars(description = "Maximum tokens to generate (defaults to the server default).")]
    pub max_tokens: Option<u32>,
}
