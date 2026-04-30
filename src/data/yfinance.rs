#![allow(dead_code)]
//! Data fetching from Yahoo Finance via Python yfinance proxy
//!
//! Yahoo Finance requires cookie/crumb session for some tickers.
//! Using Python yfinance library handles this automatically.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::process::Command;

/// Stock quote data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub symbol: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
    pub timestamp: i64,
}

/// OHLCV data for a date range
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OHLCVData {
    pub symbol: String,
    pub quotes: Vec<Quote>,
}

/// Company fundamentals
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fundamentals {
    pub ticker: String,
    pub company_name: String,
    pub sector: Option<String>,
    pub industry: Option<String>,
    pub market_cap: Option<f64>,
    pub pe_ratio: Option<f64>,
    pub dividend_yield: Option<f64>,
    pub eps: Option<f64>,
}

/// Technical indicators result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TechnicalIndicators {
    pub symbol: String,
    pub rsi: Option<f64>,
    pub macd: Option<f64>,
    pub macd_signal: Option<f64>,
    pub macd_hist: Option<f64>,
    pub bb_upper: Option<f64>,
    pub bb_middle: Option<f64>,
    pub bb_lower: Option<f64>,
}

/// Yahoo Finance data fetcher — delegates to Python yfinance proxy
#[derive(Clone)]
pub struct YahooFinanceClient {
    proxy_path: String,
}

impl YahooFinanceClient {
    pub fn new() -> Self {
        Self {
            proxy_path: std::env::var("TAGENT_YFINANCE_PROXY")
                .unwrap_or_else(|_| "yfinance_proxy.py".to_string()),
        }
    }

    pub fn with_proxy_path(path: &str) -> Self {
        Self {
            proxy_path: path.to_string(),
        }
    }

    fn proxy_path(&self) -> &str {
        &self.proxy_path
    }

    /// Call Python yfinance proxy and parse JSON output
    fn call_proxy(&self, action: &str, args: &[&str]) -> Result<serde_json::Value> {
        let mut cmd = Command::new("python");
        cmd.arg(self.proxy_path()).arg(action);
        cmd.args(args);

        let output = cmd
            .output()
            .with_context(|| format!("Failed to run yfinance proxy at {}", self.proxy_path()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("yfinance proxy failed: {}", stderr);
        }

        let json_str = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value =
            serde_json::from_str(&json_str).context("Failed to parse yfinance proxy JSON")?;

        if let Some(err) = json.get("error").and_then(|v| v.as_str()) {
            anyhow::bail!("yfinance proxy error: {}", err);
        }

        Ok(json)
    }

    /// Fetch stock data for a date range
    pub async fn get_stock_data(
        &self,
        symbol: &str,
        start_date: &str,
        end_date: &str,
    ) -> Result<String> {
        let json = self.call_proxy("get_stock_data", &[symbol, start_date, end_date])?;

        let records = json
            .get("records")
            .and_then(|v| v.as_array())
            .context("No records in yfinance response")?;

        let mut output = format!("## {} Stock Data\n\n", symbol);
        output += "| Date | Open | High | Low | Close | Volume |\n";
        output += "|------|------|------|-----|-------|--------|\n";

        for record in records {
            let date = record.get("date").and_then(|v| v.as_str()).unwrap_or("N/A");
            let open = record.get("open").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let high = record.get("high").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let low = record.get("low").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let close = record.get("close").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let volume = record.get("volume").and_then(|v| v.as_i64()).unwrap_or(0);

            output += &format!(
                "| {} | {:.2} | {:.2} | {:.2} | {:.2} | {} |\n",
                date, open, high, low, close, volume
            );
        }

        Ok(output)
    }

    /// Fetch technical indicators
    pub async fn get_indicators(
        &self,
        symbol: &str,
        curr_date: &str,
        look_back_days: i32,
    ) -> Result<String> {
        let json = self.call_proxy(
            "get_indicators",
            &[symbol, curr_date, &look_back_days.to_string()],
        )?;

        let mut output = format!("## {} Technical Indicators\n\n", symbol);

        if let Some(rsi) = json.get("rsi_14").and_then(|v| v.as_f64()) {
            output += &format!("- **RSI (14)**: {:.2}\n", rsi);
        }
        if let Some(sma10) = json.get("sma_10").and_then(|v| v.as_f64()) {
            output += &format!("- **SMA (10)**: {:.2}\n", sma10);
        }
        if let Some(sma20) = json.get("sma_20").and_then(|v| v.as_f64()) {
            output += &format!("- **SMA (20)**: {:.2}\n", sma20);
        }
        if let (Some(upper), Some(mid), Some(lower)) = (
            json.get("bb_upper").and_then(|v| v.as_f64()),
            json.get("bb_middle").and_then(|v| v.as_f64()),
            json.get("bb_lower").and_then(|v| v.as_f64()),
        ) {
            output += &format!("- **BB Upper**: {:.2}\n", upper);
            output += &format!("- **BB Middle**: {:.2}\n", mid);
            output += &format!("- **BB Lower**: {:.2}\n", lower);
        }
        if let Some(price) = json.get("current_price").and_then(|v| v.as_f64()) {
            output += &format!("- **Current Price**: {:.2}\n", price);
        }

        Ok(output)
    }

    /// Get company info / fundamentals
    pub async fn get_fundamentals(&self, ticker: &str) -> Result<String> {
        let json = self.call_proxy("get_financials", &[ticker])?;

        let mut output = format!("## {} Fundamentals\n\n", ticker);

        if let Some(name) = json.get("company_name").and_then(|v| v.as_str()) {
            output += &format!("**Company**: {}\n\n", name);
        }
        if let Some(sector) = json.get("sector").and_then(|v| v.as_str()) {
            output += &format!("**Sector**: {}\n", sector);
        }
        if let Some(industry) = json.get("industry").and_then(|v| v.as_str()) {
            output += &format!("**Industry**: {}\n", industry);
        }

        output += "\n### Key Statistics\n\n";

        if let Some(mkt_cap) = json.get("market_cap").and_then(|v| v.as_i64()) {
            output += &format!("- **Market Cap**: ${:.2}B\n", mkt_cap as f64 / 1e9);
        }
        if let Some(pe) = json.get("pe_ratio").and_then(|v| v.as_f64()) {
            output += &format!("- **P/E Ratio**: {:.2}\n", pe);
        }
        if let Some(eps) = json.get("eps").and_then(|v| v.as_f64()) {
            output += &format!("- **EPS**: ${:.2}\n", eps);
        }
        if let Some(div) = json.get("dividend_yield").and_then(|v| v.as_f64()) {
            output += &format!("- **Dividend Yield**: {:.2}%\n", div * 100.0);
        }
        if let Some(high) = json.get("52w_high").and_then(|v| v.as_f64()) {
            output += &format!("- **52W High**: {:.2}\n", high);
        }
        if let Some(low) = json.get("52w_low").and_then(|v| v.as_f64()) {
            output += &format!("- **52W Low**: {:.2}\n", low);
        }

        Ok(output)
    }

    /// Get recent news for a ticker
    pub async fn get_news(&self, ticker: &str, start_date: &str, end_date: &str) -> Result<String> {
        let json = self.call_proxy("get_news", &[ticker, start_date, end_date])?;

        let articles = json
            .get("articles")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut output = "## Recent News\n\n".to_string();

        for (i, article) in articles.iter().enumerate().take(10) {
            let title = article
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("N/A");
            let source = article
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("N/A");
            let link = article.get("link").and_then(|v| v.as_str()).unwrap_or("#");

            output += &format!("### {}. {}\n", i + 1, title);
            output += &format!("Source: {} | [Link]({})\n\n", source, link);
        }

        Ok(output)
    }

    // Keep utility methods for tests
    #[allow(dead_code)]
    fn date_to_timestamp(date_str: &str) -> Result<i64> {
        let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
            .with_context(|| format!("Invalid date format: {}", date_str))?;
        Ok(date
            .and_hms_opt(0, 0, 0)
            .expect("valid hms")
            .and_utc()
            .timestamp())
    }

    #[allow(dead_code)]
    fn calculate_rsi(prices: &[f64], period: usize) -> Option<f64> {
        if prices.len() < period + 1 {
            return None;
        }
        let mut gains = Vec::new();
        let mut losses = Vec::new();
        for i in 1..prices.len() {
            let diff = prices[i] - prices[i - 1];
            if diff > 0.0 {
                gains.push(diff);
                losses.push(0.0);
            } else {
                gains.push(0.0);
                losses.push(diff.abs());
            }
        }
        let avg_gain: f64 = gains.iter().skip(gains.len() - period).sum::<f64>() / period as f64;
        let avg_loss: f64 = losses.iter().skip(losses.len() - period).sum::<f64>() / period as f64;
        if avg_loss == 0.0 {
            return Some(100.0);
        }
        let rs = avg_gain / avg_loss;
        Some(100.0 - (100.0 / (1.0 + rs)))
    }
}

impl Default for YahooFinanceClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Tool execution router - routes tool calls to appropriate data sources
pub async fn execute_tool(
    tool_name: &str,
    args: &serde_json::Value,
    client: &YahooFinanceClient,
) -> Result<String> {
    use crate::tools::ToolName;

    let tool = ToolName::from_str(tool_name)
        .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", tool_name))?;

    match tool {
        ToolName::GetStockData => {
            let symbol = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let raw_start = args
                .get("start_date")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let raw_end = args.get("end_date").and_then(|v| v.as_str()).unwrap_or("");
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let thirty_ago = (chrono::Utc::now() - chrono::Duration::days(30))
                .format("%Y-%m-%d")
                .to_string();
            let end_date = if raw_end.is_empty() { &today } else { raw_end };
            let start_date = if raw_start.is_empty() {
                &thirty_ago
            } else {
                raw_start
            };
            client.get_stock_data(symbol, start_date, end_date).await
        }
        ToolName::GetIndicators => {
            let symbol = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let curr_date = args.get("curr_date").and_then(|v| v.as_str()).unwrap_or("");
            let look_back = args
                .get("look_back_days")
                .and_then(|v| v.as_i64())
                .unwrap_or(30) as i32;
            client.get_indicators(symbol, curr_date, look_back).await
        }
        ToolName::GetFinancials => {
            let ticker = args.get("ticker").and_then(|v| v.as_str()).unwrap_or("");
            client.get_fundamentals(ticker).await
        }
        ToolName::GetNews => {
            let ticker = args.get("ticker").and_then(|v| v.as_str()).unwrap_or("");
            let start_date = args
                .get("start_date")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let end_date = args.get("end_date").and_then(|v| v.as_str()).unwrap_or("");
            client.get_news(ticker, start_date, end_date).await
        }
        ToolName::GetGlobalNews => {
            let curr_date = args.get("curr_date").and_then(|v| v.as_str()).unwrap_or("");
            client.get_news("^GSPC", curr_date, curr_date).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_rsi() {
        let prices = vec![
            44.0, 44.25, 44.5, 43.75, 44.0, 44.5, 45.0, 45.25, 45.5, 46.0, 45.75, 46.0, 46.5, 47.0,
            47.5, 48.0, 47.75, 48.0, 48.5, 49.0,
        ];
        assert!(YahooFinanceClient::calculate_rsi(&prices, 14).is_some());
    }

    #[test]
    fn test_date_to_timestamp() {
        let ts = YahooFinanceClient::date_to_timestamp("2025-01-01").unwrap();
        assert!(ts > 0);
        assert!(YahooFinanceClient::date_to_timestamp("not-a-date").is_err());
    }

    #[test]
    fn test_proxy_path_override() {
        let client = YahooFinanceClient::with_proxy_path("custom_proxy.py");
        assert_eq!(client.proxy_path(), "custom_proxy.py");
    }
}
