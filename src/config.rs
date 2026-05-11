//! Runtime configuration loaded from environment variables.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    MiniMax,
    Zai,
    OpenAI,
    Anthropic,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MiniMax => "minimax",
            Self::Zai => "zai",
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
        }
    }

    fn api_key_env(self) -> &'static str {
        match self {
            Self::MiniMax => "MINIMAX_API_KEY",
            Self::Zai => "ZAI_API_KEY",
            Self::OpenAI => "OPENAI_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
        }
    }

    fn base_url_env(self) -> &'static str {
        match self {
            Self::MiniMax => "MINIMAX_BASE_URL",
            Self::Zai => "ZAI_BASE_URL",
            Self::OpenAI => "OPENAI_BASE_URL",
            Self::Anthropic => "ANTHROPIC_BASE_URL",
        }
    }

    fn default_base_url(self) -> &'static str {
        match self {
            Self::MiniMax => "https://api.minimaxi.com/anthropic",
            Self::Zai => "https://api.z.ai/api/anthropic",
            Self::OpenAI => "https://api.openai.com",
            Self::Anthropic => "https://api.anthropic.com",
        }
    }

    fn default_model(self) -> &'static str {
        match self {
            Self::MiniMax => "MiniMax-M2.7",
            Self::Zai => "GLM-5.1",
            Self::OpenAI => "gpt-4o",
            Self::Anthropic => "claude-sonnet-4-6",
        }
    }
}

fn env_var(primary: &str, legacy: &str) -> std::result::Result<String, std::env::VarError> {
    match std::env::var(primary) {
        Ok(value) => Ok(value),
        Err(std::env::VarError::NotPresent) => std::env::var(legacy),
        Err(err) => Err(err),
    }
}

impl FromStr for Provider {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_lowercase().as_str() {
            "minimax" => Ok(Self::MiniMax),
            "zai" => Ok(Self::Zai),
            "openai" => Ok(Self::OpenAI),
            "anthropic" => Ok(Self::Anthropic),
            other => bail!(
                "Unsupported TRADING_AGENT_PROVIDER '{}'. Expected minimax, zai, openai, or anthropic.",
                other
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub provider: Provider,
    pub api_key: String,
    pub base_url: String,
    pub quick_model: String,
    pub deep_model: String,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub llm: LlmConfig,
    pub reports_dir: PathBuf,
    pub batch_concurrency: usize,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let provider = env_var("TRADING_AGENT_PROVIDER", "TAGENT_PROVIDER")
            .unwrap_or_else(|_| Provider::MiniMax.as_str().to_string())
            .parse::<Provider>()?;

        let provider_api_key = std::env::var(provider.api_key_env()).unwrap_or_default();
        let provider_base_url = std::env::var(provider.base_url_env())
            .unwrap_or_else(|_| provider.default_base_url().to_string());
        let provider_model = provider.default_model().to_string();

        let api_key =
            env_var("TRADING_AGENT_API_KEY", "TAGENT_API_KEY").unwrap_or(provider_api_key);
        let base_url =
            env_var("TRADING_AGENT_BASE_URL", "TAGENT_BASE_URL").unwrap_or(provider_base_url);
        let default_model =
            env_var("TRADING_AGENT_MODEL", "TAGENT_MODEL").unwrap_or(provider_model);
        let quick_model = env_var("TRADING_AGENT_QUICK_MODEL", "TAGENT_QUICK_MODEL")
            .unwrap_or_else(|_| default_model.clone());
        let deep_model =
            env_var("TRADING_AGENT_DEEP_MODEL", "TAGENT_DEEP_MODEL").unwrap_or(default_model);
        let reports_dir = env_var("TRADING_AGENT_REPORTS_DIR", "TAGENT_REPORTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("reports"));
        let batch_concurrency = batch_concurrency_from_env()?;

        if api_key.trim().is_empty() {
            bail!(
                "Missing API key for provider '{}'. Set TRADING_AGENT_API_KEY, TAGENT_API_KEY, or {}.",
                provider.as_str(),
                provider.api_key_env()
            );
        }
        if base_url.trim().is_empty() {
            bail!(
                "Missing base URL for provider '{}'. Set TRADING_AGENT_BASE_URL, TAGENT_BASE_URL, or {}.",
                provider.as_str(),
                provider.base_url_env()
            );
        }
        if quick_model.trim().is_empty() || deep_model.trim().is_empty() {
            bail!("Missing LLM model name. Set TRADING_AGENT_MODEL or both TRADING_AGENT_QUICK_MODEL and TRADING_AGENT_DEEP_MODEL.");
        }
        if reports_dir.as_os_str().is_empty() {
            bail!("TRADING_AGENT_REPORTS_DIR must not be empty.");
        }

        Ok(Self {
            llm: LlmConfig {
                provider,
                api_key,
                base_url,
                quick_model,
                deep_model,
            },
            reports_dir,
            batch_concurrency,
        })
    }
}

fn batch_concurrency_from_env() -> Result<usize> {
    match env_var(
        "TRADING_AGENT_BATCH_CONCURRENCY",
        "TAGENT_BATCH_CONCURRENCY",
    ) {
        Ok(value) => {
            let parsed = value
                .parse::<usize>()
                .with_context(|| format!("Invalid TRADING_AGENT_BATCH_CONCURRENCY '{}'", value))?;
            if parsed == 0 {
                bail!("TRADING_AGENT_BATCH_CONCURRENCY must be at least 1");
            }
            Ok(parsed)
        }
        Err(std::env::VarError::NotPresent) => Ok(1),
        Err(e) => Err(e).context("Invalid TRADING_AGENT_BATCH_CONCURRENCY"),
    }
}

#[cfg(test)]
mod tests {
    use super::Provider;

    #[test]
    fn provider_from_str_accepts_supported_values_case_insensitively() {
        assert_eq!("minimax".parse::<Provider>().unwrap(), Provider::MiniMax);
        assert_eq!("ZAI".parse::<Provider>().unwrap(), Provider::Zai);
        assert_eq!("openai".parse::<Provider>().unwrap(), Provider::OpenAI);
        assert_eq!(
            "Anthropic".parse::<Provider>().unwrap(),
            Provider::Anthropic
        );
    }

    #[test]
    fn provider_from_str_rejects_unknown_values() {
        let err = "ollama".parse::<Provider>().unwrap_err().to_string();
        assert!(err.contains("Unsupported TRADING_AGENT_PROVIDER"));
    }
}
