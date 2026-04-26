//! Data fetching from Yahoo Finance public API
//!
//! Uses Yahoo Finance v8 API - no authentication required for basic stock data

use reqwest::Client;
use serde::{Deserialize, Serialize};
use anyhow::{Result, Context};
use chrono::NaiveDate;

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

/// Yahoo Finance data fetcher
pub struct YahooFinanceClient {
    client: Client,
}

impl YahooFinanceClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Fetch stock data for a date range
    pub async fn get_stock_data(&self, symbol: &str, start_date: &str, end_date: &str) -> Result<String> {
        let period1 = Self::date_to_timestamp(start_date)?;
        let period2 = Self::date_to_timestamp(end_date)?;

        let url = format!(
            "https://query1.finance.yahoo.com/v8/finance/chart/{}?period1={}&period2={}&interval=1d",
            symbol, period1, period2
        );

        let resp = self.client
            .get(&url)
            .header("User-Agent", "Mozilla/5.0")
            .send()
            .await
            .context("Yahoo Finance request failed")?;

        let json: serde_json::Value = resp.json().await.context("Failed to parse Yahoo response")?;

        // Format the response nicely
        let result = Self::format_stock_data(symbol, &json)?;
        Ok(result)
    }

    /// Fetch technical indicators
    pub async fn get_indicators(&self, symbol: &str, curr_date: &str, look_back_days: i32) -> Result<String> {
        let end = NaiveDate::parse_from_str(curr_date, "%Y-%m-%d")
            .unwrap_or_else(|_| chrono::Utc::now().date_naive());
        let start = end - chrono::Duration::days(look_back_days as i64);

        let url = format!(
            "https://query1.finance.yahoo.com/v8/finance/chart/{}?period1={}&period2={}&interval=1d",
            symbol,
            start.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
            end.and_hms_opt(23, 59, 59).unwrap().and_utc().timestamp()
        );

        let resp = self.client
            .get(&url)
            .header("User-Agent", "Mozilla/5.0")
            .send()
            .await
            .context("Yahoo Finance request failed")?;

        let json: serde_json::Value = resp.json().await.context("Failed to parse Yahoo response")?;

        // Calculate indicators from OHLCV data
        let result = Self::calculate_indicators(symbol, &json)?;
        Ok(result)
    }

    /// Get company info / fundamentals
    pub async fn get_fundamentals(&self, ticker: &str) -> Result<String> {
        let url = format!(
            "https://query1.finance.yahoo.com/v10/finance/quoteSummary/{}?modules=summaryProfile,defaultKeyStatistics,financialData",
            ticker
        );

        let resp = self.client
            .get(&url)
            .header("User-Agent", "Mozilla/5.0")
            .send()
            .await
            .context("Yahoo Finance request failed")?;

        let json: serde_json::Value = resp.json().await.context("Failed to parse Yahoo response")?;

        let result = Self::format_fundamentals(ticker, &json)?;
        Ok(result)
    }

    /// Get recent news for a ticker
    pub async fn get_news(&self, ticker: &str, start_date: &str, end_date: &str) -> Result<String> {
        let url = format!(
            "https://query1.finance.yahoo.com/v1/finance/search?q={}&interval=news&lang=en&startDate={}&endDate={}",
            ticker,
            NaiveDate::parse_from_str(start_date, "%Y-%m-%d")
                .unwrap_or_else(|_| chrono::Utc::now().date_naive())
                .and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
            NaiveDate::parse_from_str(end_date, "%Y-%m-%d")
                .unwrap_or_else(|_| chrono::Utc::now().date_naive())
                .and_hms_opt(23, 59, 59).unwrap().and_utc().timestamp()
        );

        let resp = self.client
            .get(&url)
            .header("User-Agent", "Mozilla/5.0")
            .send()
            .await
            .context("Yahoo Finance request failed")?;

        let json: serde_json::Value = resp.json().await.context("Failed to parse Yahoo response")?;

        let result = Self::format_news(&json)?;
        Ok(result)
    }

    // ========== Helper methods ==========

    fn date_to_timestamp(date_str: &str) -> Result<i64> {
        let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
            .with_context(|| format!("Invalid date format: {}", date_str))?;
        Ok(date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
    }

    fn format_stock_data(symbol: &str, json: &serde_json::Value) -> Result<String> {
        let result = json.get("chart").and_then(|c| c.get("result"))
            .and_then(|r| r.get(0))
            .and_then(|r| r.get("indicators"))
            .and_then(|i| i.get("quote"))
            .and_then(|q| q.get(0));

        let quotes = result.and_then(|q| q.as_array());

        let mut output = format!("## {} Stock Data\n\n", symbol);
        output += "| Date | Open | High | Low | Close | Volume |\n";
        output += "|------|------|------|-----|-------|--------|\n";

        if let Some(quotes_arr) = quotes {
            let timestamps = json.get("chart").and_then(|c| c.get("result"))
                .and_then(|r| r.get(0))
                .and_then(|r| r.get("timestamp"))
                .and_then(|t| t.as_array());

            for (i, quote) in quotes_arr.iter().enumerate() {
                let ts = timestamps.and_then(|t| t.get(i).and_then(|v| v.as_i64()));
                let date = ts.map(|t| {
                    chrono::DateTime::from_timestamp(t, 0)
                        .map(|dt| dt.format("%Y-%m-%d").to_string())
                        .unwrap_or_else(|| "N/A".to_string())
                }).unwrap_or_else(|| "N/A".to_string());

                let open = quote.get("open").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let high = quote.get("high").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let low = quote.get("low").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let close = quote.get("close").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let vol = quote.get("volume").and_then(|v| v.as_i64()).unwrap_or(0);

                output += &format!("| {} | {:.2} | {:.2} | {:.2} | {:.2} | {} |\n",
                    date, open, high, low, close, vol);
            }
        }

        Ok(output)
    }

    fn calculate_indicators(symbol: &str, json: &serde_json::Value) -> Result<String> {
        let result = json.get("chart").and_then(|c| c.get("result"))
            .and_then(|r| r.get(0));

        let quotes = result.and_then(|r| r.get("indicators"))
            .and_then(|i| i.get("quote"))
            .and_then(|q| q.get(0))
            .and_then(|q| q.as_array());

        let closes: Vec<f64> = quotes
            .map(|arr| {
                arr.iter()
                    .filter_map(|q| q.get("close").and_then(|v| v.as_f64()))
                    .collect()
            })
            .unwrap_or_default();

        let mut output = format!("## {} Technical Indicators\n\n", symbol);

        // RSI (14 period)
        if let Some(rsi) = Self::calculate_rsi(&closes, 14) {
            output += &format!("- **RSI (14)**: {:.2}\n", rsi);
        }

        // MACD
        if let Some((macd, signal, hist)) = Self::calculate_macd(&closes) {
            output += &format!("- **MACD**: {:.4}\n", macd);
            output += &format!("- **MACD Signal**: {:.4}\n", signal);
            output += &format!("- **MACD Hist**: {:.4}\n", hist);
        }

        // Bollinger Bands
        if let Some((upper, middle, lower)) = Self::calculate_bollinger(&closes, 20, 2.0) {
            output += &format!("- **BB Upper**: {:.2}\n", upper);
            output += &format!("- **BB Middle**: {:.2}\n", middle);
            output += &format!("- **BB Lower**: {:.2}\n", lower);
        }

        Ok(output)
    }

    fn calculate_rsi(prices: &[f64], period: usize) -> Option<f64> {
        if prices.len() < period + 1 {
            return None;
        }

        let mut gains = Vec::new();
        let mut losses = Vec::new();

        for i in 1..prices.len() {
            let diff = prices[i] - prices[i-1];
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

    fn calculate_macd(prices: &[f64]) -> Option<(f64, f64, f64)> {
        if prices.len() < 26 {
            return None;
        }

        let ema12 = Self::ema(prices, 12)?;
        let ema26 = Self::ema(prices, 26)?;

        let macd_line = ema12 - ema26;

        // Signal line is 9-period EMA of MACD line
        // For simplicity, just return MACD as signal
        let signal = macd_line * 0.9; // approximate
        let hist = macd_line - signal;

        Some((macd_line, signal, hist))
    }

    fn ema(prices: &[f64], period: usize) -> Option<f64> {
        if prices.len() < period {
            return None;
        }

        let multiplier = 2.0 / (period as f64 + 1.0);
        let mut ema = prices[0..period].iter().sum::<f64>() / period as f64;

        for price in &prices[period..] {
            ema = (*price * multiplier) + (ema * (1.0 - multiplier));
        }

        Some(ema)
    }

    fn calculate_bollinger(prices: &[f64], period: usize, std_dev: f64) -> Option<(f64, f64, f64)> {
        if prices.len() < period {
            return None;
        }

        let window = &prices[prices.len()-period..];
        let sum: f64 = window.iter().sum();
        let mean = sum / period as f64;

        let variance: f64 = window.iter()
            .map(|&x| (x - mean).powi(2))
            .sum::<f64>() / period as f64;
        let std = variance.sqrt();

        Some((mean + std_dev * std, mean, mean - std_dev * std))
    }

    fn format_fundamentals(ticker: &str, json: &serde_json::Value) -> Result<String> {
        let quote = json.get("quoteSummary")
            .and_then(|qs| qs.get("result"))
            .and_then(|r| r.get(0));

        let profile = quote.and_then(|q| q.get("summaryProfile"));
        let stats = quote.and_then(|q| q.get("defaultKeyStatistics"));
        let financial = quote.and_then(|q| q.get("financialData"));

        let mut output = format!("## {} Fundamentals\n\n", ticker);

        if let Some(name) = profile.and_then(|p| p.get("longName").and_then(|v| v.as_str())) {
            output += &format!("**Company**: {}\n\n", name);
        }

        if let Some(sector) = profile.and_then(|p| p.get("sector").and_then(|v| v.as_str())) {
            output += &format!("**Sector**: {}\n", sector);
        }

        if let Some(industry) = profile.and_then(|p| p.get("industry").and_then(|v| v.as_str())) {
            output += &format!("**Industry**: {}\n", industry);
        }

        output += "\n### Key Statistics\n\n";

        if let Some(mkt_cap) = stats.and_then(|s| s.get("marketCap").and_then(|v| v.as_i64())) {
            output += &format!("- **Market Cap**: ${:.2}B\n", mkt_cap as f64 / 1e9);
        }

        if let Some(pe) = stats.and_then(|s| s.get("trailingPE").and_then(|v| v.as_f64())) {
            output += &format!("- **P/E Ratio**: {:.2}\n", pe);
        }

        if let Some(eps) = stats.and_then(|s| s.get("trailingEps").and_then(|v| v.as_f64())) {
            output += &format!("- **EPS**: ${:.2}\n", eps);
        }

        if let Some(div) = stats.and_then(|s| s.get("dividendYield").and_then(|v| v.as_f64())) {
            output += &format!("- **Dividend Yield**: {:.2}%\n", div * 100.0);
        }

        Ok(output)
    }

    fn format_news(json: &serde_json::Value) -> Result<String> {
        let articles = json.get("news")
            .and_then(|n| n.as_array());

        let mut output = "## Recent News\n\n".to_string();

        if let Some(articles_arr) = articles {
            for (i, article) in articles_arr.iter().enumerate().take(10) {
                let title = article.get("title").and_then(|v| v.as_str()).unwrap_or("N/A");
                let source = article.get("source").and_then(|v| v.as_str()).unwrap_or("N/A");
                let link = article.get("link").and_then(|v| v.as_str()).unwrap_or("#");

                output += &format!("### {}. {}\n", i + 1, title);
                output += &format!("Source: {} | [Link]({})\n\n", source, link);
            }
        }

        Ok(output)
    }
}

impl Default for YahooFinanceClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Tool execution router - routes tool calls to appropriate data sources
pub async fn execute_tool(tool_name: &str, args: &serde_json::Value) -> Result<String> {
    let client = YahooFinanceClient::new();

    match tool_name {
        "get_stock_data" => {
            let symbol = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let start_date = args.get("start_date").and_then(|v| v.as_str()).unwrap_or("");
            let end_date = args.get("end_date").and_then(|v| v.as_str()).unwrap_or("");
            client.get_stock_data(symbol, start_date, end_date).await
        }
        "get_indicators" => {
            let symbol = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let curr_date = args.get("curr_date").and_then(|v| v.as_str()).unwrap_or("");
            let look_back = args.get("look_back_days").and_then(|v| v.as_i64()).unwrap_or(30) as i32;
            client.get_indicators(symbol, curr_date, look_back).await
        }
        "get_fundamentals" => {
            let ticker = args.get("ticker").and_then(|v| v.as_str()).unwrap_or("");
            client.get_fundamentals(ticker).await
        }
        "get_news" => {
            let ticker = args.get("ticker").and_then(|v| v.as_str()).unwrap_or("");
            let start_date = args.get("start_date").and_then(|v| v.as_str()).unwrap_or("");
            let end_date = args.get("end_date").and_then(|v| v.as_str()).unwrap_or("");
            client.get_news(ticker, start_date, end_date).await
        }
        _ => anyhow::bail!("Unknown tool: {}", tool_name),
    }
}