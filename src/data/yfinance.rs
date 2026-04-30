#![allow(dead_code)]
//! Native Yahoo Finance data fetching.
//!
//! This module uses Yahoo Finance's public chart and search endpoints directly
//! through reqwest, so the TAgent binary no longer depends on a Python proxy.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

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

/// Yahoo Finance data fetcher.
#[derive(Clone)]
pub struct YahooFinanceClient {
    client: reqwest::Client,
    base_url: String,
}

impl YahooFinanceClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("tagent/0.1.0")
                .build()
                .expect("Failed to build Yahoo Finance HTTP client"),
            base_url: std::env::var("TAGENT_YAHOO_BASE_URL")
                .unwrap_or_else(|_| "https://query1.finance.yahoo.com".to_string()),
        }
    }

    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("tagent/0.1.0")
                .build()
                .expect("Failed to build Yahoo Finance HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    async fn get_json(&self, path: &str, query: &[(&str, String)]) -> Result<serde_json::Value> {
        let response = self
            .client
            .get(self.endpoint(path))
            .query(query)
            .send()
            .await
            .with_context(|| format!("Failed to fetch Yahoo Finance endpoint {}", path))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("Failed to read Yahoo Finance response")?;

        if !status.is_success() {
            anyhow::bail!("Yahoo Finance HTTP {}: {}", status, body);
        }

        serde_json::from_str(&body).context("Failed to parse Yahoo Finance JSON")
    }

    async fn fetch_chart(
        &self,
        symbol: &str,
        start_date: &str,
        end_date: &str,
    ) -> Result<serde_json::Value> {
        let period1 = Self::date_to_timestamp(start_date)?;
        let period2 = Self::date_to_timestamp(end_date)?;
        if period2 <= period1 {
            anyhow::bail!("end_date must be after start_date");
        }

        let path = format!("/v8/finance/chart/{}", encode_component(symbol));
        let json = self
            .get_json(
                &path,
                &[
                    ("period1", period1.to_string()),
                    ("period2", period2.to_string()),
                    ("interval", "1d".to_string()),
                    ("events", "history".to_string()),
                    ("includeAdjustedClose", "true".to_string()),
                ],
            )
            .await?;

        if let Some(err) = json.pointer("/chart/error") {
            if !err.is_null() {
                anyhow::bail!("Yahoo Finance chart error: {}", err);
            }
        }

        Ok(json)
    }

    fn chart_result(json: &serde_json::Value) -> Result<&serde_json::Value> {
        json.pointer("/chart/result/0")
            .context("No chart result in Yahoo Finance response")
    }

    fn parse_chart_quotes(json: &serde_json::Value, symbol: &str) -> Result<Vec<Quote>> {
        let result = Self::chart_result(json)?;
        let timestamps = result
            .get("timestamp")
            .and_then(|v| v.as_array())
            .context("No timestamps in Yahoo Finance chart response")?;
        let quote = result
            .pointer("/indicators/quote/0")
            .context("No quote data in Yahoo Finance chart response")?;

        let opens = quote
            .get("open")
            .and_then(|v| v.as_array())
            .unwrap_or(timestamps);
        let highs = quote
            .get("high")
            .and_then(|v| v.as_array())
            .unwrap_or(timestamps);
        let lows = quote
            .get("low")
            .and_then(|v| v.as_array())
            .unwrap_or(timestamps);
        let closes = quote
            .get("close")
            .and_then(|v| v.as_array())
            .unwrap_or(timestamps);
        let volumes = quote
            .get("volume")
            .and_then(|v| v.as_array())
            .unwrap_or(timestamps);

        let mut quotes = Vec::new();
        for (i, ts) in timestamps.iter().enumerate() {
            let Some(timestamp) = ts.as_i64() else {
                continue;
            };
            let Some(open) = opens.get(i).and_then(|v| v.as_f64()) else {
                continue;
            };
            let Some(high) = highs.get(i).and_then(|v| v.as_f64()) else {
                continue;
            };
            let Some(low) = lows.get(i).and_then(|v| v.as_f64()) else {
                continue;
            };
            let Some(close) = closes.get(i).and_then(|v| v.as_f64()) else {
                continue;
            };
            let volume = volumes.get(i).and_then(|v| v.as_i64()).unwrap_or(0);

            quotes.push(Quote {
                symbol: symbol.to_uppercase(),
                open,
                high,
                low,
                close,
                volume,
                timestamp,
            });
        }

        if quotes.is_empty() {
            anyhow::bail!("No chart data found for {}", symbol);
        }

        Ok(quotes)
    }

    /// Fetch stock data for a date range
    pub async fn get_stock_data(
        &self,
        symbol: &str,
        start_date: &str,
        end_date: &str,
    ) -> Result<String> {
        let json = self.fetch_chart(symbol, start_date, end_date).await?;
        let quotes = Self::parse_chart_quotes(&json, symbol)?;

        let mut output = format!("## {} Stock Data\n\n", symbol);
        output += "| Date | Open | High | Low | Close | Volume |\n";
        output += "|------|------|------|-----|-------|--------|\n";

        for quote in quotes {
            let date = timestamp_to_date(quote.timestamp);
            output += &format!(
                "| {} | {:.2} | {:.2} | {:.2} | {:.2} | {} |\n",
                date, quote.open, quote.high, quote.low, quote.close, quote.volume
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
        let end = NaiveDate::parse_from_str(curr_date, "%Y-%m-%d")
            .with_context(|| format!("Invalid date format: {}", curr_date))?;
        let start = end - chrono::Duration::days(look_back_days.into());
        let json = self
            .fetch_chart(symbol, &start.format("%Y-%m-%d").to_string(), curr_date)
            .await?;
        let quotes = Self::parse_chart_quotes(&json, symbol)?;
        let closes: Vec<f64> = quotes.iter().map(|q| q.close).collect();

        let mut output = format!("## {} Technical Indicators\n\n", symbol);

        if let Some(rsi) = Self::calculate_rsi(&closes, 14) {
            output += &format!("- **RSI (14)**: {:.2}\n", rsi);
        }
        if let Some(sma10) = simple_moving_average(&closes, 10) {
            output += &format!("- **SMA (10)**: {:.2}\n", sma10);
        }
        if let Some(sma20) = simple_moving_average(&closes, 20) {
            output += &format!("- **SMA (20)**: {:.2}\n", sma20);
        }
        if let Some((upper, mid, lower)) = bollinger_bands(&closes, 20) {
            output += &format!("- **BB Upper**: {:.2}\n", upper);
            output += &format!("- **BB Middle**: {:.2}\n", mid);
            output += &format!("- **BB Lower**: {:.2}\n", lower);
        }
        if let Some(price) = closes.last() {
            output += &format!("- **Current Price**: {:.2}\n", price);
        }

        Ok(output)
    }

    /// Get company info / fundamentals
    pub async fn get_fundamentals(&self, ticker: &str) -> Result<String> {
        let today = chrono::Utc::now().date_naive();
        let start = (today - chrono::Duration::days(7))
            .format("%Y-%m-%d")
            .to_string();
        let end = (today + chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let chart_json = self.fetch_chart(ticker, &start, &end).await?;
        let chart_result = Self::chart_result(&chart_json)?;
        let meta = chart_result.get("meta").unwrap_or(&serde_json::Value::Null);

        let search_json = self
            .get_json(
                "/v1/finance/search",
                &[
                    ("q", ticker.to_string()),
                    ("quotesCount", "1".to_string()),
                    ("newsCount", "0".to_string()),
                ],
            )
            .await
            .unwrap_or(serde_json::Value::Null);
        let quote = search_json
            .get("quotes")
            .and_then(|v| v.as_array())
            .and_then(|quotes| quotes.first())
            .unwrap_or(&serde_json::Value::Null);

        let mut output = format!("## {} Fundamentals\n\n", ticker);

        if let Some(name) = meta
            .get("longName")
            .or_else(|| meta.get("shortName"))
            .or_else(|| quote.get("longname"))
            .or_else(|| quote.get("shortname"))
            .and_then(|v| v.as_str())
        {
            output += &format!("**Company**: {}\n\n", name);
        }
        if let Some(sector) = quote
            .get("sector")
            .or_else(|| quote.get("sectorDisp"))
            .and_then(|v| v.as_str())
        {
            output += &format!("**Sector**: {}\n", sector);
        }
        if let Some(industry) = quote
            .get("industry")
            .or_else(|| quote.get("industryDisp"))
            .and_then(|v| v.as_str())
        {
            output += &format!("**Industry**: {}\n", industry);
        }

        output += "\n### Key Statistics\n\n";

        if let Some(price) = meta.get("regularMarketPrice").and_then(|v| v.as_f64()) {
            output += &format!("- **Current Price**: ${:.2}\n", price);
        }
        if let Some(high) = meta.get("fiftyTwoWeekHigh").and_then(|v| v.as_f64()) {
            output += &format!("- **52W High**: {:.2}\n", high);
        }
        if let Some(low) = meta.get("fiftyTwoWeekLow").and_then(|v| v.as_f64()) {
            output += &format!("- **52W Low**: {:.2}\n", low);
        }

        output += "\nNote: Native Yahoo Finance mode uses public chart/search endpoints. Some valuation metrics such as market cap, P/E, EPS, and dividend yield may be unavailable without authenticated quote-summary access.\n";

        Ok(output)
    }

    /// Get recent news for a ticker
    pub async fn get_news(&self, ticker: &str, start_date: &str, end_date: &str) -> Result<String> {
        let _ = (start_date, end_date);
        let json = self
            .get_json(
                "/v1/finance/search",
                &[
                    ("q", ticker.to_string()),
                    ("quotesCount", "0".to_string()),
                    ("newsCount", "10".to_string()),
                ],
            )
            .await?;

        let articles = json
            .get("articles")
            .or_else(|| json.get("news"))
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
                .or_else(|| article.get("publisher"))
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

fn timestamp_to_date(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "N/A".to_string())
}

fn simple_moving_average(values: &[f64], period: usize) -> Option<f64> {
    if values.len() < period {
        return None;
    }
    Some(values.iter().rev().take(period).sum::<f64>() / period as f64)
}

fn bollinger_bands(values: &[f64], period: usize) -> Option<(f64, f64, f64)> {
    if values.len() < period {
        return None;
    }
    let window: Vec<f64> = values.iter().rev().take(period).copied().collect();
    let mean = window.iter().sum::<f64>() / period as f64;
    let variance = window
        .iter()
        .map(|value| {
            let diff = value - mean;
            diff * diff
        })
        .sum::<f64>()
        / period as f64;
    let std_dev = variance.sqrt();
    Some((mean + 2.0 * std_dev, mean, mean - 2.0 * std_dev))
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// Tool execution router - routes tool calls to appropriate data sources
pub async fn execute_tool(
    tool_name: &str,
    args: &serde_json::Value,
    client: &YahooFinanceClient,
) -> Result<String> {
    use crate::tools::ToolName;

    let tool = tool_name.parse::<ToolName>()?;

    match tool {
        ToolName::GetStockData => {
            let symbol = required_tool_arg(args, "symbol")?;
            let raw_start = optional_tool_arg(args, "start_date");
            let raw_end = optional_tool_arg(args, "end_date");
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
            let symbol = required_tool_arg(args, "symbol")?;
            let curr_date = required_tool_arg(args, "curr_date")?;
            let look_back = args
                .get("look_back_days")
                .and_then(|v| v.as_i64())
                .unwrap_or(30) as i32;
            client.get_indicators(symbol, curr_date, look_back).await
        }
        ToolName::GetFinancials => {
            let ticker = required_tool_arg(args, "ticker")?;
            client.get_fundamentals(ticker).await
        }
        ToolName::GetNews => {
            let ticker = required_tool_arg(args, "ticker")?;
            let start_date = optional_tool_arg(args, "start_date");
            let end_date = optional_tool_arg(args, "end_date");
            client.get_news(ticker, start_date, end_date).await
        }
        ToolName::GetGlobalNews => {
            let curr_date = required_tool_arg(args, "curr_date")?;
            client.get_news("^GSPC", curr_date, curr_date).await
        }
    }
}

fn required_tool_arg<'a>(args: &'a serde_json::Value, name: &str) -> Result<&'a str> {
    args.get(name)
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("Missing required tool argument '{}'", name))
}

fn optional_tool_arg<'a>(args: &'a serde_json::Value, name: &str) -> &'a str {
    args.get(name).and_then(|v| v.as_str()).unwrap_or("")
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
    fn test_base_url_override() {
        let client = YahooFinanceClient::with_base_url("https://example.com/");
        assert_eq!(
            client.endpoint("/v8/finance/chart/AAPL"),
            "https://example.com/v8/finance/chart/AAPL"
        );
    }

    #[test]
    fn test_encode_component() {
        assert_eq!(encode_component("^GSPC"), "%5EGSPC");
        assert_eq!(encode_component("BRK-B"), "BRK-B");
    }

    #[test]
    fn test_bollinger_bands() {
        let values: Vec<f64> = (1..=20).map(|value| value as f64).collect();
        let (upper, middle, lower) = bollinger_bands(&values, 20).unwrap();
        assert!(upper > middle);
        assert!(middle > lower);
    }

    #[test]
    fn test_required_tool_arg_rejects_missing_values() {
        let args = serde_json::json!({"symbol": ""});
        let err = required_tool_arg(&args, "symbol").unwrap_err().to_string();
        assert!(err.contains("Missing required tool argument 'symbol'"));
    }

    #[tokio::test]
    #[ignore = "hits live Yahoo Finance endpoints"]
    async fn test_live_yahoo_native_smoke() {
        let client = YahooFinanceClient::new();
        let stock_data = client
            .get_stock_data("AAPL", "2026-04-01", "2026-04-30")
            .await
            .unwrap();
        assert!(stock_data.contains("## AAPL Stock Data"));

        let indicators = client
            .get_indicators("AAPL", "2026-04-30", 30)
            .await
            .unwrap();
        assert!(indicators.contains("Technical Indicators"));

        let fundamentals = client.get_fundamentals("AAPL").await.unwrap();
        assert!(fundamentals.contains("Fundamentals"));

        let news = client
            .get_news("AAPL", "2026-04-01", "2026-04-30")
            .await
            .unwrap();
        assert!(news.contains("Recent News"));
    }
}
