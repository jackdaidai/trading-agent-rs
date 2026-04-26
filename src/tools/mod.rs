#![allow(dead_code)]
//! Tool registry and invocation system
//!
//! Tools are defined here and routed to vendor implementations (yfinance, alpha_vantage)

use crate::llm::Tool;
use serde_json::{json, Value};
use std::collections::HashMap;


/// Type-safe tool names — adding a variant here without handling it in
/// `execute_tool` will produce a compile error (non-exhaustive match).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(clippy::enum_variant_names)]
pub enum ToolName {
    GetStockData,
    GetIndicators,
    GetFundamentals,
    GetBalanceSheet,
    GetCashflow,
    GetIncomeStatement,
    GetNews,
    GetGlobalNews,
    GetInsiderTransactions,
}

impl ToolName {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GetStockData => "get_stock_data",
            Self::GetIndicators => "get_indicators",
            Self::GetFundamentals => "get_fundamentals",
            Self::GetBalanceSheet => "get_balance_sheet",
            Self::GetCashflow => "get_cashflow",
            Self::GetIncomeStatement => "get_income_statement",
            Self::GetNews => "get_news",
            Self::GetGlobalNews => "get_global_news",
            Self::GetInsiderTransactions => "get_insider_transactions",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "get_stock_data" => Some(Self::GetStockData),
            "get_indicators" => Some(Self::GetIndicators),
            "get_fundamentals" => Some(Self::GetFundamentals),
            "get_balance_sheet" => Some(Self::GetBalanceSheet),
            "get_cashflow" => Some(Self::GetCashflow),
            "get_income_statement" => Some(Self::GetIncomeStatement),
            "get_news" => Some(Self::GetNews),
            "get_global_news" => Some(Self::GetGlobalNews),
            "get_insider_transactions" => Some(Self::GetInsiderTransactions),
            _ => None,
        }
    }

    /// All variants, used to generate tool definitions.
    pub fn all() -> &'static [Self] {
        &[
            Self::GetStockData,
            Self::GetIndicators,
            Self::GetFundamentals,
            Self::GetBalanceSheet,
            Self::GetCashflow,
            Self::GetIncomeStatement,
            Self::GetNews,
            Self::GetGlobalNews,
            Self::GetInsiderTransactions,
        ]
    }
}

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

    /// Get a tool by its typed name. Panics only if the registry wasn't populated
    /// with all tools (a programmer error, not user input).
    pub fn get_by_name(&self, name: ToolName) -> Tool {
        self.tools.get(name.as_str())
            .cloned()
            .expect("ToolRegistry missing a registered tool — this is a bug")
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
