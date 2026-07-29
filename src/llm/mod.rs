//! LLM Client abstraction for multiple providers (OpenAI, Anthropic, MiniMax, etc.)

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

// =============================================================================
// Types for Anthropic multi-turn message format
// =============================================================================

/// A single message in Anthropic message history format
#[derive(Debug, Clone)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: Vec<AnthropicContentBlock>,
}

/// Content block types for Anthropic messages
#[derive(Debug, Clone)]
pub enum AnthropicContentBlock {
    /// Plain text content
    Text(String),
    /// Tool use request from assistant
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// Tool result provided to assistant (user role)
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

impl From<AnthropicMessage> for Value {
    fn from(msg: AnthropicMessage) -> Value {
        let content: Vec<Value> = msg.content.into_iter().map(|b| b.into()).collect();
        json!({
            "role": msg.role,
            "content": content,
        })
    }
}

impl From<AnthropicContentBlock> for Value {
    fn from(block: AnthropicContentBlock) -> Value {
        match block {
            AnthropicContentBlock::Text(s) => json!({"type": "text", "text": s}),
            AnthropicContentBlock::ToolUse { id, name, input } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            AnthropicContentBlock::ToolResult {
                tool_use_id,
                content,
            } => {
                // Anthropic API: tool_result content is a string or nested content array
                // For simplicity, use a structured object format
                json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": content})
            }
        }
    }
}

// =============================================================================
// Retry helper — exponential backoff on 429 / 5xx
// =============================================================================

const MAX_RETRIES: u32 = 3;
const BASE_DELAY_MS: u64 = 1000;
const MAX_RETRY_AFTER_MS: u64 = 60_000;
const DEFAULT_LLM_CONCURRENCY: usize = 4;
const DEFAULT_LLM_TIMEOUT_SECS: u64 = 240;
const DEFAULT_MAX_TOKENS: u64 = 4096;

fn env_u64(primary: &str, legacy: &str, default: u64) -> u64 {
    std::env::var(primary)
        .or_else(|_| std::env::var(legacy))
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

/// HTTP timeout for LLM requests. Deep-synthesis calls on reasoning models
/// can legitimately run for minutes; a timeout that's too short re-runs the
/// whole expensive call on each retry and then fails the ticker.
fn llm_timeout() -> Duration {
    Duration::from_secs(env_u64(
        "TRADING_AGENT_LLM_TIMEOUT",
        "TAGENT_LLM_TIMEOUT",
        DEFAULT_LLM_TIMEOUT_SECS,
    ))
}

fn llm_max_tokens() -> u64 {
    env_u64(
        "TRADING_AGENT_MAX_TOKENS",
        "TAGENT_MAX_TOKENS",
        DEFAULT_MAX_TOKENS,
    )
}

/// Process-wide cap on concurrent in-flight LLM requests. Keeps batch runs
/// under provider rate limits regardless of how many tickers run in parallel.
/// Configurable via TRADING_AGENT_LLM_CONCURRENCY (legacy TAGENT_LLM_CONCURRENCY).
static LLM_CONCURRENCY_LIMIT: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| {
        let permits = std::env::var("TRADING_AGENT_LLM_CONCURRENCY")
            .or_else(|_| std::env::var("TAGENT_LLM_CONCURRENCY"))
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_LLM_CONCURRENCY);
        tokio::sync::Semaphore::new(permits)
    });

/// Parse a Retry-After header value (seconds form only) into milliseconds,
/// capped at MAX_RETRY_AFTER_MS.
fn retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|secs| (secs * 1000).min(MAX_RETRY_AFTER_MS))
}

fn endpoint_url(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let suffix = path.trim_start_matches('/');
    if base.ends_with(suffix) {
        base.to_string()
    } else if let Some(versionless_suffix) = suffix.strip_prefix("v1/") {
        if base.ends_with("/v1") {
            format!("{}/{}", base, versionless_suffix)
        } else {
            format!("{}/{}", base, suffix)
        }
    } else {
        format!("{}/{}", base, suffix)
    }
}

async fn retry_request(
    build_request: impl Fn() -> Pin<
        Box<dyn Future<Output = Result<reqwest::Response, reqwest::Error>> + Send>,
    >,
) -> Result<Value> {
    for attempt in 0..=MAX_RETRIES {
        // Hold a concurrency permit only while the request is in flight,
        // not during backoff sleeps.
        let permit = LLM_CONCURRENCY_LIMIT
            .acquire()
            .await
            .expect("LLM concurrency semaphore closed");
        let resp = build_request().await;

        let resp = match resp {
            Ok(r) => r,
            // A client-side timeout means the model needed longer than the
            // configured limit — re-sending the same request will time out
            // again and multiply latency and token billing. Fail fast.
            Err(e) if e.is_timeout() => {
                return Err(e).context(
                    "LLM request timed out; raise TRADING_AGENT_LLM_TIMEOUT if the model legitimately needs longer",
                );
            }
            Err(e) if attempt < MAX_RETRIES => {
                drop(permit);
                let delay = BASE_DELAY_MS * 2u64.pow(attempt);
                tracing::warn!(
                    "Request error (attempt {}): {}. Retrying in {}ms",
                    attempt + 1,
                    e,
                    delay
                );
                tokio::time::sleep(Duration::from_millis(delay)).await;
                continue;
            }
            Err(e) => return Err(e).context("Request failed after retries"),
        };

        let status = resp.status();
        if status == 429 || status.is_server_error() {
            if attempt < MAX_RETRIES {
                // Prefer the provider's Retry-After hint over exponential backoff
                let delay =
                    retry_after_ms(resp.headers()).unwrap_or(BASE_DELAY_MS * 2u64.pow(attempt));
                drop(permit);
                tracing::warn!(
                    "HTTP {} (attempt {}). Retrying in {}ms",
                    status,
                    attempt + 1,
                    delay
                );
                tokio::time::sleep(Duration::from_millis(delay)).await;
                continue;
            }
            let body = resp.text().await.unwrap_or_default();
            bail!(
                "HTTP {} after {} retries: {}",
                status,
                MAX_RETRIES,
                body.chars().take(300).collect::<String>()
            );
        }

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("HTTP {}: {}", status, body);
        }

        let json: Value = resp.json().await.context("Failed to parse response JSON")?;
        return Ok(json);
    }
    unreachable!()
}

fn parse_openai_response(resp_json: &Value) -> Result<LLMResponse> {
    let message = resp_json
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .context("OpenAI response missing choices[0].message")?;
    let content = optional_str(message, "content")
        .unwrap_or_default()
        .to_string();
    let tool_calls = match message.get("tool_calls") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let calls = value
                .as_array()
                .context("OpenAI response field choices[0].message.tool_calls is not an array")?
                .iter()
                .map(parse_openai_tool_call)
                .collect::<Result<Vec<_>>>()?;
            Some(calls)
        }
    };

    if resp_json
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        == Some("length")
    {
        tracing::warn!(
            "OpenAI response truncated at the model's token limit — output may be cut off mid-sentence"
        );
    }

    Ok(LLMResponse {
        content,
        tool_calls,
        reasoning: None,
    })
}

fn parse_openai_tool_call(value: &Value) -> Result<ToolCall> {
    let id = required_str(value, "id", "OpenAI tool call")?.to_string();
    let function = value
        .get("function")
        .context("OpenAI tool call missing function object")?;
    let name = required_str(function, "name", "OpenAI tool call function")?.to_string();
    let args_text = required_str(function, "arguments", "OpenAI tool call function")?;
    let arguments = serde_json::from_str(args_text)
        .with_context(|| format!("Failed to parse OpenAI tool call arguments for '{}'", name))?;

    Ok(ToolCall {
        id,
        name,
        arguments,
    })
}

fn parse_anthropic_response(resp_json: &Value) -> Result<LLMResponse> {
    if resp_json.get("stop_reason").and_then(Value::as_str) == Some("max_tokens") {
        tracing::warn!(
            "Response truncated at max_tokens — consider raising TRADING_AGENT_MAX_TOKENS"
        );
    }

    let blocks = resp_json
        .get("content")
        .and_then(Value::as_array)
        .context("Anthropic response missing content array")?;
    let mut content = String::new();
    let mut tool_calls = Vec::new();

    for block in blocks {
        match required_str(block, "type", "Anthropic content block")? {
            "text" => {
                // Tolerate empty text blocks — Anthropic-compatible providers
                // (MiniMax, z.ai) can emit them alongside tool_use blocks.
                if let Some(text) = optional_str(block, "text") {
                    content.push_str(text);
                }
            }
            "tool_use" => {
                tool_calls.push(ToolCall {
                    id: required_str(block, "id", "Anthropic tool_use content block")?.to_string(),
                    name: required_str(block, "name", "Anthropic tool_use content block")?
                        .to_string(),
                    arguments: block
                        .get("input")
                        .context("Anthropic tool_use content block missing input")?
                        .clone(),
                });
            }
            other => {
                tracing::warn!(
                    "Ignoring unsupported Anthropic content block type '{}'",
                    other
                );
            }
        }
    }

    Ok(LLMResponse {
        content,
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        reasoning: None,
    })
}

fn required_str<'a>(value: &'a Value, key: &str, context: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .with_context(|| format!("{} missing non-empty string field '{}'", context, key))
}

fn optional_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// Tool definition for LLM tool calling
#[derive(Debug, Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value, // JSON Schema
}

/// LLM response that may include tool calls
#[derive(Debug)]
pub struct LLMResponse {
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
    // Reserved for providers that return a separate reasoning channel.
    #[allow(dead_code)]
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// LLM Client trait - implemented by each provider
#[async_trait]
pub trait LLMClient: Send + Sync {
    /// Send a simple text completion request
    async fn complete(&self, prompt: &str) -> Result<String>;

    /// Send a completion request with tool definitions
    async fn complete_with_tools(&self, prompt: &str, tools: &[Tool]) -> Result<LLMResponse>;

    /// Send a completion request with proper multi-turn message history.
    /// Messages must include tool_result blocks for prior tool calls.
    async fn complete_messages(
        &self,
        messages: Vec<AnthropicMessage>,
        tools: &[Tool],
    ) -> Result<LLMResponse>;

    /// Validate that the model is available
    fn validate_model(&self) -> bool;

    /// Provider name for logging
    fn provider_name(&self) -> &str;
}

// =============================================================================
// Provider Capability Table
// =============================================================================

/// Per-provider/model capabilities that control structured output behavior.
#[derive(Debug, Clone)]
struct ProviderCapabilities {
    /// Whether this provider/model supports the `tool_choice` parameter.
    supports_tool_choice: bool,
}

impl ProviderCapabilities {
    /// Determine capabilities from model name and base URL.
    ///
    /// DeepSeek V4+ and MiniMax M2.x (via OpenAI-compat path) reject `tool_choice`,
    /// so we skip it for those. Ollama models also generally don't support it.
    fn for_model(model: &str, base_url: &str) -> Self {
        let model_lower = model.to_lowercase();
        let url_lower = base_url.to_lowercase();

        let supports_tool_choice = !model_lower.contains("deepseek")
            && !model_lower.starts_with("minimax")
            && !url_lower.contains("minimax")
            && !url_lower.contains("ollama")
            && !url_lower.contains("localhost:11434");

        Self {
            supports_tool_choice,
        }
    }
}

// =============================================================================
// OpenAI Client (also used for MiniMax, Ollama, OpenRouter)
// =============================================================================

pub struct OpenAIClient {
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    client: reqwest::Client,
    capabilities: ProviderCapabilities,
}

impl OpenAIClient {
    pub fn new(model: &str, api_key: &str, base_url: &str) -> Result<Self> {
        let capabilities = ProviderCapabilities::for_model(model, base_url);
        Ok(Self {
            model: model.to_string(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            client: reqwest::Client::builder()
                .timeout(llm_timeout())
                .build()
                .context("Failed to build OpenAI HTTP client")?,
            capabilities,
        })
    }
}

#[async_trait]
impl LLMClient for OpenAIClient {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let response = self.complete_with_tools(prompt, &[]).await?;
        Ok(response.content)
    }

    async fn complete_with_tools(&self, prompt: &str, tools: &[Tool]) -> Result<LLMResponse> {
        let mut body = json!({
            "model": self.model,
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": llm_max_tokens()
        });

        // Add tool definitions if provided
        if !tools.is_empty() {
            body["tools"] = json!(tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect::<Vec<_>>());

            // Only set tool_choice for providers that support it.
            // DeepSeek V4+, MiniMax M2.x, and Ollama reject this parameter.
            if self.capabilities.supports_tool_choice {
                body["tool_choice"] = json!("auto");
            }
        }

        let resp_json = retry_request(|| {
            let client = self.client.clone();
            let url = endpoint_url(&self.base_url, "/v1/chat/completions");
            let key = self.api_key.clone();
            let body = body.clone();
            Box::pin(async move {
                client
                    .post(&url)
                    .bearer_auth(key)
                    .header("content-type", "application/json")
                    .json(&body)
                    .send()
                    .await
            })
        })
        .await?;

        parse_openai_response(&resp_json)
    }

    async fn complete_messages(
        &self,
        messages: Vec<AnthropicMessage>,
        tools: &[Tool],
    ) -> Result<LLMResponse> {
        // OpenAI doesn't use Anthropic multi-turn message format.
        // Concatenate all text content into a flat prompt as a fallback.
        let mut flat = String::new();
        for msg in &messages {
            flat.push_str(&format!("[{}] ", msg.role));
            for block in &msg.content {
                match block {
                    AnthropicContentBlock::Text(s) => flat.push_str(s),
                    AnthropicContentBlock::ToolUse { name, input, .. } => {
                        flat.push_str(&format!("(tool call: {} with {:?})", name, input));
                    }
                    AnthropicContentBlock::ToolResult { content, .. } => {
                        flat.push_str(&format!("(tool result: {})", content));
                    }
                }
                flat.push_str("; ");
            }
            flat.push('\n');
        }
        self.complete_with_tools(&flat, tools).await
    }

    fn validate_model(&self) -> bool {
        !self.model.is_empty() && !self.api_key.is_empty()
    }

    fn provider_name(&self) -> &str {
        "openai"
    }
}

// =============================================================================
// Anthropic Client (also supports MiniMax compatible endpoint)
// =============================================================================

pub struct AnthropicClient {
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    client: reqwest::Client,
    capabilities: ProviderCapabilities,
    /// When true, send `thinking: disabled` on every call, not just tool calls.
    /// Speeds up reasoning models (MiniMax M2, GLM) on short debate/risk prompts.
    disable_thinking: bool,
    max_tokens: u64,
}

impl AnthropicClient {
    pub fn new(model: &str, api_key: &str, base_url: &str) -> Result<Self> {
        let capabilities = ProviderCapabilities::for_model(model, base_url);
        Ok(Self {
            model: model.to_string(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            client: reqwest::Client::builder()
                .timeout(llm_timeout())
                .build()
                .context("Failed to build Anthropic HTTP client")?,
            capabilities,
            disable_thinking: false,
            max_tokens: llm_max_tokens(),
        })
    }
}

#[async_trait]
impl LLMClient for AnthropicClient {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let response = self.complete_with_tools(prompt, &[]).await?;
        Ok(response.content)
    }

    async fn complete_with_tools(&self, prompt: &str, tools: &[Tool]) -> Result<LLMResponse> {
        let mut body = json!({
            "model": self.model,
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": self.max_tokens,
        });

        // Add tool definitions if provided
        if !tools.is_empty() {
            body["tools"] = json!(tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect::<Vec<_>>());

            // Anthropic supports tool_choice; MiniMax (via Anthropic-compat) may not.
            if self.capabilities.supports_tool_choice {
                body["tool_choice"] = json!({"type": "auto"});
            }
        }
        if !tools.is_empty() || self.disable_thinking {
            body["thinking"] = json!({"type": "disabled"});
        }

        let resp_json = retry_request(|| {
            let client = self.client.clone();
            let url = endpoint_url(&self.base_url, "/v1/messages");
            let key = self.api_key.clone();
            let body = body.clone();
            Box::pin(async move {
                client
                    .post(&url)
                    .header("x-api-key", key)
                    .header("anthropic-version", "2023-06-01")
                    .header("content-type", "application/json")
                    .json(&body)
                    .send()
                    .await
            })
        })
        .await?;

        parse_anthropic_response(&resp_json)
    }

    async fn complete_messages(
        &self,
        messages: Vec<AnthropicMessage>,
        tools: &[Tool],
    ) -> Result<LLMResponse> {
        // Build Anthropic API message format with proper content blocks
        let api_messages: Vec<Value> = messages
            .into_iter()
            .map(|m| {
                let content: Vec<Value> = m.content.into_iter().map(|b| b.into()).collect();
                json!({
                    "role": m.role,
                    "content": content,
                })
            })
            .collect();

        let mut body = json!({
            "model": self.model,
            "messages": api_messages,
            "max_tokens": self.max_tokens,
        });

        if !tools.is_empty() {
            body["tools"] = json!(tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect::<Vec<_>>());

            if self.capabilities.supports_tool_choice {
                body["tool_choice"] = json!({"type": "auto"});
            }
        }
        if !tools.is_empty() || self.disable_thinking {
            body["thinking"] = json!({"type": "disabled"});
        }

        let resp_json = retry_request(|| {
            let client = self.client.clone();
            let url = endpoint_url(&self.base_url, "/v1/messages");
            let key = self.api_key.clone();
            let body = body.clone();
            Box::pin(async move {
                client
                    .post(&url)
                    .header("x-api-key", key)
                    .header("anthropic-version", "2023-06-01")
                    .header("content-type", "application/json")
                    .json(&body)
                    .send()
                    .await
            })
        })
        .await?;

        parse_anthropic_response(&resp_json)
    }

    fn validate_model(&self) -> bool {
        !self.model.is_empty() && !self.api_key.is_empty()
    }

    fn provider_name(&self) -> &str {
        "anthropic"
    }
}

// =============================================================================
// Factory
// =============================================================================

pub enum AnyLLMClient {
    OpenAI(OpenAIClient),
    Anthropic(AnthropicClient),
}

impl AnyLLMClient {
    pub fn new(provider: &str, model: &str, api_key: &str, base_url: &str) -> Result<Self> {
        let client = match provider.to_lowercase().as_str() {
            "anthropic" | "minimax" | "zai" => {
                // MiniMax and z.ai use Anthropic-compatible API
                Self::Anthropic(AnthropicClient::new(model, api_key, base_url)?)
            }
            _ => Self::OpenAI(OpenAIClient::new(model, api_key, base_url)?),
        };
        Ok(client)
    }

    /// Disable extended thinking on all calls (Anthropic-format providers only;
    /// no-op for OpenAI-format providers, which have no thinking parameter).
    pub fn with_thinking_disabled(self, disabled: bool) -> Self {
        match self {
            Self::Anthropic(mut c) => {
                c.disable_thinking = disabled;
                Self::Anthropic(c)
            }
            other => other,
        }
    }
}

#[async_trait]
impl LLMClient for AnyLLMClient {
    async fn complete(&self, prompt: &str) -> Result<String> {
        match self {
            Self::OpenAI(c) => c.complete(prompt).await,
            Self::Anthropic(c) => c.complete(prompt).await,
        }
    }

    async fn complete_with_tools(&self, prompt: &str, tools: &[Tool]) -> Result<LLMResponse> {
        match self {
            Self::OpenAI(c) => c.complete_with_tools(prompt, tools).await,
            Self::Anthropic(c) => c.complete_with_tools(prompt, tools).await,
        }
    }

    async fn complete_messages(
        &self,
        messages: Vec<AnthropicMessage>,
        tools: &[Tool],
    ) -> Result<LLMResponse> {
        match self {
            Self::OpenAI(c) => c.complete_messages(messages, tools).await,
            Self::Anthropic(c) => c.complete_messages(messages, tools).await,
        }
    }

    fn validate_model(&self) -> bool {
        match self {
            Self::OpenAI(c) => c.validate_model(),
            Self::Anthropic(c) => c.validate_model(),
        }
    }

    fn provider_name(&self) -> &str {
        match self {
            Self::OpenAI(c) => c.provider_name(),
            Self::Anthropic(c) => c.provider_name(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        endpoint_url, parse_anthropic_response, parse_openai_response, retry_after_ms,
        ProviderCapabilities,
    };
    use serde_json::json;

    #[test]
    fn retry_after_header_parsed_in_seconds_and_capped() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after_ms(&headers), None);

        headers.insert(reqwest::header::RETRY_AFTER, "7".parse().unwrap());
        assert_eq!(retry_after_ms(&headers), Some(7_000));

        headers.insert(reqwest::header::RETRY_AFTER, "999".parse().unwrap());
        assert_eq!(retry_after_ms(&headers), Some(60_000));

        // HTTP-date form is not supported — fall back to exponential backoff
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after_ms(&headers), None);
    }

    #[test]
    fn capability_table_supports_tool_choice_for_standard_providers() {
        let openai = ProviderCapabilities::for_model("gpt-4o", "https://api.openai.com");
        assert!(openai.supports_tool_choice);

        let anthropic =
            ProviderCapabilities::for_model("claude-sonnet-4-6", "https://api.anthropic.com");
        assert!(anthropic.supports_tool_choice);
    }

    #[test]
    fn capability_table_skips_tool_choice_for_unsupported() {
        let deepseek =
            ProviderCapabilities::for_model("deepseek-chat-v4", "https://api.deepseek.com");
        assert!(!deepseek.supports_tool_choice);

        let minimax =
            ProviderCapabilities::for_model("MiniMax-M2.7", "https://api.minimaxi.com/anthropic");
        assert!(!minimax.supports_tool_choice);

        let ollama = ProviderCapabilities::for_model("llama3", "http://localhost:11434");
        assert!(!ollama.supports_tool_choice);

        let remote_ollama =
            ProviderCapabilities::for_model("qwen2", "http://myserver:8080/ollama/v1");
        assert!(!remote_ollama.supports_tool_choice);
    }

    #[test]
    fn endpoint_url_avoids_duplicate_version_path() {
        assert_eq!(
            endpoint_url("https://api.openai.com", "/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint_url("https://api.openai.com/v1", "/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint_url(
                "https://api.openai.com/v1/chat/completions",
                "/v1/chat/completions"
            ),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn parse_openai_response_rejects_malformed_tool_arguments() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": "",
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "get_stock_data",
                            "arguments": "{not-json}"
                        }
                    }]
                }
            }]
        });

        let err = parse_openai_response(&response).unwrap_err().to_string();
        assert!(err.contains("Failed to parse OpenAI tool call arguments"));
    }

    #[test]
    fn parse_openai_response_extracts_tool_calls() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": "checking",
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "get_stock_data",
                            "arguments": "{\"symbol\":\"AAPL\"}"
                        }
                    }]
                }
            }]
        });

        let parsed = parse_openai_response(&response).unwrap();
        assert_eq!(parsed.content, "checking");
        let tool_calls = parsed.tool_calls.unwrap();
        assert_eq!(tool_calls[0].id, "call_1");
        assert_eq!(tool_calls[0].name, "get_stock_data");
        assert_eq!(tool_calls[0].arguments["symbol"], "AAPL");
    }

    #[test]
    fn parse_anthropic_response_requires_content_blocks() {
        let err = parse_anthropic_response(&json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing content array"));
    }

    #[test]
    fn parse_anthropic_response_survives_max_tokens_truncation() {
        let response = json!({
            "stop_reason": "max_tokens",
            "content": [{"type": "text", "text": "truncated repor"}]
        });

        let parsed = parse_anthropic_response(&response).unwrap();
        assert_eq!(parsed.content, "truncated repor");
    }

    #[test]
    fn parse_anthropic_response_tolerates_empty_text_blocks() {
        let response = json!({
            "content": [
                {"type": "text", "text": ""},
                {
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "get_news",
                    "input": {"ticker": "MSFT"}
                }
            ]
        });

        let parsed = parse_anthropic_response(&response).unwrap();
        assert_eq!(parsed.content, "");
        assert_eq!(parsed.tool_calls.unwrap().len(), 1);
    }

    #[test]
    fn parse_anthropic_response_extracts_content_and_tool_calls() {
        let response = json!({
            "content": [
                {"type": "text", "text": "Need data"},
                {
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "get_news",
                    "input": {"ticker": "MSFT"}
                }
            ]
        });

        let parsed = parse_anthropic_response(&response).unwrap();
        assert_eq!(parsed.content, "Need data");
        let tool_calls = parsed.tool_calls.unwrap();
        assert_eq!(tool_calls[0].id, "toolu_1");
        assert_eq!(tool_calls[0].name, "get_news");
        assert_eq!(tool_calls[0].arguments["ticker"], "MSFT");
    }
}
