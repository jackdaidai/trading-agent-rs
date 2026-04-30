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
        let resp = build_request().await;

        let resp = match resp {
            Ok(r) => r,
            Err(e) if attempt < MAX_RETRIES => {
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
                let delay = BASE_DELAY_MS * 2u64.pow(attempt);
                tracing::warn!(
                    "HTTP {} (attempt {}). Retrying in {}ms",
                    status,
                    attempt + 1,
                    delay
                );
                tokio::time::sleep(Duration::from_millis(delay)).await;
                continue;
            }
            bail!("HTTP {} after {} retries", status, MAX_RETRIES);
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
    let blocks = resp_json
        .get("content")
        .and_then(Value::as_array)
        .context("Anthropic response missing content array")?;
    let mut content = String::new();
    let mut tool_calls = Vec::new();

    for block in blocks {
        match required_str(block, "type", "Anthropic content block")? {
            "text" => {
                content.push_str(required_str(block, "text", "Anthropic text content block")?);
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
// OpenAI Client (also used for MiniMax, Ollama, OpenRouter)
// =============================================================================

pub struct OpenAIClient {
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    client: reqwest::Client,
}

impl OpenAIClient {
    pub fn new(model: &str, api_key: &str, base_url: &str) -> Result<Self> {
        Ok(Self {
            model: model.to_string(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .context("Failed to build OpenAI HTTP client")?,
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
            "messages": [{"role": "user", "content": prompt}]
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
}

impl AnthropicClient {
    pub fn new(model: &str, api_key: &str, base_url: &str) -> Result<Self> {
        Ok(Self {
            model: model.to_string(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .context("Failed to build Anthropic HTTP client")?,
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
            "max_tokens": 4096,
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
            "max_tokens": 4096,
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
    use super::{endpoint_url, parse_anthropic_response, parse_openai_response};
    use serde_json::json;

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
