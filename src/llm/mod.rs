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
#[allow(dead_code)]
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
    #[allow(dead_code)]
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
    pub fn new(model: &str, api_key: &str, base_url: &str) -> Self {
        Self {
            model: model.to_string(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("Failed to build HTTP client"),
        }
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

        // Extract content
        let content = resp_json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        // Extract tool calls if present
        let tool_calls = resp_json["choices"][0]["message"]["tool_calls"]
            .as_array()
            .map(|tc| {
                tc.iter()
                    .map(|t| {
                        let args: Value = serde_json::from_str(
                            t["function"]["arguments"].as_str().unwrap_or("{}"),
                        )
                        .unwrap_or(json!({}));
                        ToolCall {
                            id: t["id"].as_str().unwrap_or("").to_string(),
                            name: t["function"]["name"].as_str().unwrap_or("").to_string(),
                            arguments: args,
                        }
                    })
                    .collect()
            });

        Ok(LLMResponse {
            content,
            tool_calls,
            reasoning: None,
        })
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
    pub fn new(model: &str, api_key: &str, base_url: &str) -> Self {
        Self {
            model: model.to_string(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("Failed to build HTTP client"),
        }
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

        // Extract content blocks
        let mut content = String::new();
        let mut tool_calls = Vec::new();

        if let Some(blocks) = resp_json["content"].as_array() {
            for block in blocks {
                match block["type"].as_str().unwrap_or("") {
                    "text" => content.push_str(block["text"].as_str().unwrap_or("")),
                    "tool_use" => {
                        let id = block["id"].as_str().unwrap_or("").to_string();
                        let args: Value = block.get("input").cloned().unwrap_or(json!({}));
                        tool_calls.push(ToolCall {
                            id,
                            name: block["name"].as_str().unwrap_or("").to_string(),
                            arguments: args,
                        });
                    }
                    _ => {}
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

        // Extract content blocks
        let mut content = String::new();
        let mut tool_calls = Vec::new();

        if let Some(blocks) = resp_json["content"].as_array() {
            for block in blocks {
                match block["type"].as_str().unwrap_or("") {
                    "text" => content.push_str(block["text"].as_str().unwrap_or("")),
                    "tool_use" => {
                        let id = block["id"].as_str().unwrap_or("").to_string();
                        let args: Value = block.get("input").cloned().unwrap_or(json!({}));
                        tool_calls.push(ToolCall {
                            id,
                            name: block["name"].as_str().unwrap_or("").to_string(),
                            arguments: args,
                        });
                    }
                    _ => {}
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
    pub fn new(provider: &str, model: &str, api_key: &str, base_url: &str) -> Self {
        match provider.to_lowercase().as_str() {
            "anthropic" | "minimax" | "zai" => {
                // MiniMax and z.ai use Anthropic-compatible API
                Self::Anthropic(AnthropicClient::new(model, api_key, base_url))
            }
            _ => Self::OpenAI(OpenAIClient::new(model, api_key, base_url)),
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
    use super::endpoint_url;

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
}
