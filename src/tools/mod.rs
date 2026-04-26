//! Tool registry and invocation system
//!
//! Tools are defined here and routed to vendor implementations (yfinance, alpha_vantage)

use crate::llm::Tool;
use serde_json::{json, Value};
use std::collections::HashMap;
use anyhow::{Result, Context};

/// All available tools in the system
pub fn get_all_tools() -> Vec<Tool> {
    vec![
        // Market tools
        Tool {
            name: "get_stock_data".to_string(),
            description: "Get historical OHLCV stock data".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "symbol": {"type": "string", "description": "Stock ticker symbol"},
                    "start_date": {"type": "string", "description": "Start date yyyy-mm-dd"},
                    "end_date": {"type": "string", "description": "End date yyyy-mm-dd"}
                },
                "required": ["symbol", "start_date", "end_date"]
            }),
        },
        Tool {
            name: "get_indicators".to_string(),
            description: "Get technical indicators (RSI, MACD, Bollinger, etc.)".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "symbol": {"type": "string", "description": "Stock ticker symbol"},
                    "indicator": {"type": "string", "description": "Comma-separated indicators"},
                    "curr_date": {"type": "string", "description": "Current date"},
                    "look_back_days": {"type": "integer", "description": "Lookback period", "default": 30}
                },
                "required": ["symbol", "indicator", "curr_date"]
            }),
        },
        // Fundamentals
        Tool {
            name: "get_fundamentals".to_string(),
            description: "Get company fundamentals overview".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ticker": {"type": "string", "description": "Stock ticker"},
                    "curr_date": {"type": "string", "description": "Current date"}
                },
                "required": ["ticker", "curr_date"]
            }),
        },
        Tool {
            name: "get_balance_sheet".to_string(),
            description: "Get company balance sheet".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ticker": {"type": "string", "description": "Stock ticker"},
                    "freq": {"type": "string", "description": "quarterly or annual", "default": "quarterly"},
                    "curr_date": {"type": "string", "description": "Current date"}
                },
                "required": ["ticker", "curr_date"]
            }),
        },
        Tool {
            name: "get_cashflow".to_string(),
            description: "Get company cash flow statement".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ticker": {"type": "string", "description": "Stock ticker"},
                    "freq": {"type": "string", "description": "quarterly or annual", "default": "quarterly"},
                    "curr_date": {"type": "string", "description": "Current date"}
                },
                "required": ["ticker", "curr_date"]
            }),
        },
        Tool {
            name: "get_income_statement".to_string(),
            description: "Get company income statement".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ticker": {"type": "string", "description": "Stock ticker"},
                    "freq": {"type": "string", "description": "quarterly or annual", "default": "quarterly"},
                    "curr_date": {"type": "string", "description": "Current date"}
                },
                "required": ["ticker", "curr_date"]
            }),
        },
        // News
        Tool {
            name: "get_news".to_string(),
            description: "Get news for a ticker in date range".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ticker": {"type": "string", "description": "Stock ticker"},
                    "start_date": {"type": "string", "description": "Start date yyyy-mm-dd"},
                    "end_date": {"type": "string", "description": "End date yyyy-mm-dd"}
                },
                "required": ["ticker", "start_date", "end_date"]
            }),
        },
        Tool {
            name: "get_global_news".to_string(),
            description: "Get global market news".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "curr_date": {"type": "string", "description": "Current date"},
                    "look_back_days": {"type": "integer", "description": "Days to look back", "default": 7},
                    "limit": {"type": "integer", "description": "Number of articles", "default": 5}
                },
                "required": ["curr_date"]
            }),
        },
        Tool {
            name: "get_insider_transactions".to_string(),
            description: "Get insider trading transactions".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ticker": {"type": "string", "description": "Stock ticker"}
                },
                "required": ["ticker"]
            }),
        },
    ]
}

/// Tool registry for looking up tools
pub struct ToolRegistry {
    tools: HashMap<String, Tool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        let tools = get_all_tools().into_iter()
            .map(|t| (t.name.clone(), t))
            .collect();
        Self { tools }
    }

    pub fn get(&self, name: &str) -> Option<&Tool> {
        self.tools.get(name)
    }

    pub fn all(&self) -> Vec<Tool> {
        self.tools.values().cloned().collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Tool call arguments
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Tool result returned to LLM
#[derive(Debug)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(content: &str) -> Self {
        Self {
            content: content.to_string(),
            is_error: false,
        }
    }

    pub fn err(content: &str) -> Self {
        Self {
            content: content.to_string(),
            is_error: true,
        }
    }
}
