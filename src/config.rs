use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::PathBuf;

use crate::api::Effort;

/// Default Anthropic API base URL. Overridable via the `base_url` config field
/// (e.g. to route through a compatible gateway/proxy).
fn default_base_url() -> String {
    "https://api.anthropic.com/v1".to_string()
}

/// Configuration loaded from the TOML config file.
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub api_key: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub default_max_tokens: Option<u32>,
    /// Default reasoning effort when a tool call omits it. Invalid values are
    /// rejected at TOML parse time by the `Effort` enum's deserializer.
    #[serde(default)]
    pub default_effort: Option<Effort>,
}

/// Returns the path to the config file, using `dirs::config_dir()` for cross-platform support.
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
            PathBuf::from(home).join(".config")
        })
        .join("mcp-server-fable")
        .join("config.toml")
}

/// Load and validate the config file.
pub fn load() -> Result<Config> {
    let path = config_path();
    let content = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "Failed to read config file: {}\n\
             Create it with your Anthropic API key.\n\
             Example:\n\n\
             api_key = \"sk-ant-...\"",
            path.display()
        )
    })?;
    let config: Config =
        toml::from_str(&content).with_context(|| format!("Failed to parse {}", path.display()))?;

    if config.api_key.trim().is_empty() {
        bail!(
            "api_key in {} is empty — set it to your Anthropic API key",
            path.display()
        );
    }

    Ok(config)
}
