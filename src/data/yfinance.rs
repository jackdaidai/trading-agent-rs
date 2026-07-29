#![allow(dead_code)]
//! Native Yahoo Finance data fetching.
//!
//! This module uses Yahoo Finance's public chart and search endpoints directly
//! through reqwest, so the trading-agent-rs binary no longer depends on a Python proxy.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

const NEWS_ARTICLE_LIMIT: usize = 20;
const NEWS_EXCERPT_LIMIT: usize = 8;
const NEWS_EXCERPT_CHARS: usize = 900;
const ARTICLE_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const YAHOO_USER_AGENT: &str = "Mozilla/5.0";

// =============================================================================
// Regional Benchmark Mapping
// =============================================================================

/// Returns the appropriate benchmark index ticker for a given stock ticker,
/// based on the exchange suffix. Falls back to SPY (S&P 500) for US tickers.
///
/// Configurable via `TRADING_AGENT_BENCHMARK` / `TAGENT_BENCHMARK` env var
/// which overrides the automatic detection.
pub fn benchmark_for_ticker(ticker: &str) -> String {
    // Check for explicit override first
    if let Ok(override_bm) =
        std::env::var("TRADING_AGENT_BENCHMARK").or_else(|_| std::env::var("TAGENT_BENCHMARK"))
    {
        if !override_bm.trim().is_empty() {
            return override_bm.trim().to_string();
        }
    }

    // Extract exchange suffix (e.g., ".NS" from "RELIANCE.NS")
    let suffix = ticker.rfind('.').map(|i| &ticker[i..]).unwrap_or("");

    match suffix.to_uppercase().as_str() {
        ".NS" => "^NSEI".to_string(),             // India NSE → Nifty 50
        ".BO" => "^BSESN".to_string(),            // India BSE → Sensex
        ".T" => "^N225".to_string(),              // Tokyo → Nikkei 225
        ".HK" => "^HSI".to_string(),              // Hong Kong → Hang Seng
        ".L" => "^FTSE".to_string(),              // London → FTSE 100
        ".TO" | ".V" => "^GSPTSE".to_string(),    // Toronto/TSX Venture → S&P/TSX
        ".AX" => "^AXJO".to_string(),             // Australia → ASX 200
        ".SS" | ".SZ" => "000001.SS".to_string(), // Shanghai/Shenzhen → SSE Composite
        ".SH" => "000001.SS".to_string(),         // Shanghai alt suffix
        ".KS" | ".KQ" => "^KS11".to_string(),     // Korea KOSPI/KOSDAQ
        ".TW" => "^TWII".to_string(),             // Taiwan → TAIEX
        ".DE" | ".F" | ".PA" | ".AS" | ".MI" | ".MC" | ".BR" => "^STOXX50E".to_string(), // Eurozone → Euro Stoxx 50
        _ => "SPY".to_string(), // US default
    }
}

/// Stock quote data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub symbol: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    /// Dividend-adjusted close (falls back to `close` when Yahoo omits it).
    #[serde(default)]
    pub adjclose: Option<f64>,
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
    /// Cached Yahoo auth crumb for the quoteSummary endpoint (fetched once).
    crumb: Arc<tokio::sync::OnceCell<String>>,
}

impl YahooFinanceClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(YAHOO_USER_AGENT)
                .timeout(Duration::from_secs(12))
                .cookie_store(true)
                .build()
                .expect("Failed to build Yahoo Finance HTTP client"),
            base_url: std::env::var("TRADING_AGENT_YAHOO_BASE_URL")
                .or_else(|_| std::env::var("TAGENT_YAHOO_BASE_URL"))
                .unwrap_or_else(|_| "https://query1.finance.yahoo.com".to_string()),
            crumb: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(YAHOO_USER_AGENT)
                .timeout(Duration::from_secs(12))
                .cookie_store(true)
                .build()
                .expect("Failed to build Yahoo Finance HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
            crumb: Arc::new(tokio::sync::OnceCell::new()),
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
        // period2 is an exclusive upper bound on bar timestamps, and daily bars
        // are stamped at market open (after midnight UTC for most exchanges).
        // Add a day so the end_date's own bar is included.
        let period2 = Self::date_to_timestamp(end_date)? + 86_400;
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
            .context("No open prices in Yahoo Finance chart response")?;
        let highs = quote
            .get("high")
            .and_then(|v| v.as_array())
            .context("No high prices in Yahoo Finance chart response")?;
        let lows = quote
            .get("low")
            .and_then(|v| v.as_array())
            .context("No low prices in Yahoo Finance chart response")?;
        let closes = quote
            .get("close")
            .and_then(|v| v.as_array())
            .context("No close prices in Yahoo Finance chart response")?;
        let volumes = quote.get("volume").and_then(|v| v.as_array());
        let adjcloses = result
            .pointer("/indicators/adjclose/0/adjclose")
            .and_then(|v| v.as_array());

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
            let volume = volumes
                .and_then(|a| a.get(i))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

            let adjclose = adjcloses.and_then(|a| a.get(i)).and_then(|v| v.as_f64());

            quotes.push(Quote {
                symbol: symbol.to_uppercase(),
                open,
                high,
                low,
                close,
                adjclose,
                volume,
                timestamp,
            });
        }

        if quotes.is_empty() {
            anyhow::bail!("No chart data found for {}", symbol);
        }

        Ok(quotes)
    }

    /// Fetch parsed OHLCV quotes for a date range (used by decision resolution).
    pub async fn get_quotes(
        &self,
        symbol: &str,
        start_date: &str,
        end_date: &str,
    ) -> Result<Vec<Quote>> {
        let json = self.fetch_chart(symbol, start_date, end_date).await?;
        Self::parse_chart_quotes(&json, symbol)
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
        if let Some((macd_line, signal, hist)) = macd(&closes) {
            output += &format!("- **MACD (12,26)**: {:.2}\n", macd_line);
            output += &format!("- **MACD Signal (9)**: {:.2}\n", signal);
            output += &format!("- **MACD Histogram**: {:.2}\n", hist);
        } else {
            output += "- **MACD**: insufficient history (needs at least 34 trading days)\n";
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

    /// Fetch (once, cached) a Yahoo auth crumb. Warms the cookie jar first.
    async fn get_crumb(&self) -> Result<String> {
        self.crumb
            .get_or_try_init(|| async {
                // fc.yahoo.com sets the session cookie the crumb endpoint needs.
                let _ = self.client.get("https://fc.yahoo.com").send().await;
                let resp = self
                    .client
                    .get(self.endpoint("/v1/test/getcrumb"))
                    .header("accept", "text/plain")
                    .send()
                    .await
                    .context("crumb request failed")?;
                let status = resp.status();
                let crumb = resp.text().await.context("reading crumb")?;
                let crumb = crumb.trim().to_string();
                if !status.is_success() {
                    anyhow::bail!(
                        "crumb request HTTP {}: {}",
                        status,
                        crumb.chars().take(80).collect::<String>()
                    );
                }
                // Error bodies ("Too Many Requests") contain spaces/HTML; a
                // real crumb is a short single token. Never cache garbage —
                // OnceCell would pin it for the process lifetime.
                if crumb.is_empty()
                    || crumb.contains('<')
                    || crumb.contains(char::is_whitespace)
                    || crumb.len() > 64
                {
                    anyhow::bail!("invalid crumb response");
                }
                Ok::<String, anyhow::Error>(crumb)
            })
            .await
            .map(|s| s.clone())
    }

    /// Valuation + financial fundamentals via the authenticated quoteSummary
    /// endpoint (market cap, P/E, EPS, margins, revenue, cash flow).
    async fn fundamentals_quote_summary(&self, ticker: &str) -> Result<String> {
        fn raw(m: Option<&serde_json::Value>, key: &str) -> Option<f64> {
            m?.get(key)?.get("raw")?.as_f64()
        }
        fn money(n: f64) -> String {
            if n.abs() >= 1e9 {
                format!("${:.2}B", n / 1e9)
            } else if n.abs() >= 1e6 {
                format!("${:.1}M", n / 1e6)
            } else {
                format!("${:.0}", n)
            }
        }

        let crumb = self.get_crumb().await?;
        let path = format!("/v10/finance/quoteSummary/{}", encode_component(ticker));
        let modules = "assetProfile,price,summaryDetail,defaultKeyStatistics,financialData";
        let response = self
            .client
            .get(self.endpoint(&path))
            .query(&[("modules", modules), ("crumb", crumb.as_str())])
            .send()
            .await
            .context("quoteSummary request failed")?;
        let status = response.status();
        let body = response.text().await.context("reading quoteSummary")?;
        if !status.is_success() {
            anyhow::bail!(
                "quoteSummary HTTP {}: {}",
                status,
                body.chars().take(160).collect::<String>()
            );
        }
        let json: serde_json::Value =
            serde_json::from_str(&body).context("parsing quoteSummary JSON")?;
        let r = json
            .pointer("/quoteSummary/result/0")
            .context("quoteSummary missing result")?;

        let profile = r.get("assetProfile");
        let price = r.get("price");
        let sd = r.get("summaryDetail");
        let ks = r.get("defaultKeyStatistics");
        let fd = r.get("financialData");

        let mut out = format!("## {} Fundamentals\n\n", ticker);
        if let Some(n) = price
            .and_then(|p| p.get("longName").or_else(|| p.get("shortName")))
            .and_then(|v| v.as_str())
        {
            out += &format!("**Company**: {}\n\n", n);
        }
        if let Some(s) = profile.and_then(|p| p.get("sector")).and_then(|v| v.as_str()) {
            out += &format!("**Sector**: {}\n", s);
        }
        if let Some(i) = profile.and_then(|p| p.get("industry")).and_then(|v| v.as_str()) {
            out += &format!("**Industry**: {}\n", i);
        }

        out += "\n### Valuation\n\n";
        if let Some(v) = raw(fd, "currentPrice").or_else(|| raw(price, "regularMarketPrice")) {
            out += &format!("- **Current Price**: ${:.2}\n", v);
        }
        if let Some(v) = raw(price, "marketCap").or_else(|| raw(sd, "marketCap")) {
            out += &format!("- **Market Cap**: {}\n", money(v));
        }
        if let Some(v) = raw(sd, "trailingPE") {
            out += &format!("- **Trailing P/E**: {:.2}\n", v);
        }
        if let Some(v) = raw(sd, "forwardPE").or_else(|| raw(ks, "forwardPE")) {
            out += &format!("- **Forward P/E**: {:.2}\n", v);
        }
        if let Some(v) = raw(ks, "trailingEps") {
            out += &format!("- **Trailing EPS**: ${:.2}\n", v);
        }
        if let Some(v) = raw(ks, "forwardEps") {
            out += &format!("- **Forward EPS**: ${:.2}\n", v);
        }
        if let Some(v) = raw(ks, "priceToBook") {
            out += &format!("- **P/B**: {:.2}\n", v);
        }
        if let Some(v) = raw(ks, "enterpriseValue") {
            out += &format!("- **Enterprise Value**: {}\n", money(v));
        }
        if let Some(v) = raw(sd, "dividendYield") {
            out += &format!("- **Dividend Yield**: {:.2}%\n", v * 100.0);
        }

        out += "\n### Financials (TTM)\n\n";
        if let Some(v) = raw(fd, "totalRevenue") {
            out += &format!("- **Revenue**: {}\n", money(v));
        }
        if let Some(v) = raw(fd, "revenueGrowth") {
            out += &format!("- **Revenue Growth (YoY)**: {:.1}%\n", v * 100.0);
        }
        if let Some(v) = raw(fd, "grossMargins") {
            out += &format!("- **Gross Margin**: {:.1}%\n", v * 100.0);
        }
        if let Some(v) = raw(fd, "operatingMargins") {
            out += &format!("- **Operating Margin**: {:.1}%\n", v * 100.0);
        }
        if let Some(v) = raw(fd, "profitMargins").or_else(|| raw(ks, "profitMargins")) {
            out += &format!("- **Profit Margin**: {:.1}%\n", v * 100.0);
        }
        if let Some(v) = raw(fd, "returnOnEquity") {
            out += &format!("- **ROE**: {:.1}%\n", v * 100.0);
        }
        if let Some(v) = raw(fd, "freeCashflow") {
            out += &format!("- **Free Cash Flow**: {}\n", money(v));
        }
        if let Some(v) = raw(fd, "totalCash") {
            out += &format!("- **Total Cash**: {}\n", money(v));
        }
        if let Some(v) = raw(fd, "totalDebt") {
            out += &format!("- **Total Debt**: {}\n", money(v));
        }
        if let Some(v) = raw(sd, "fiftyTwoWeekHigh") {
            out += &format!("- **52W High**: {:.2}\n", v);
        }
        if let Some(v) = raw(sd, "fiftyTwoWeekLow") {
            out += &format!("- **52W Low**: {:.2}\n", v);
        }

        if let Some(k) = fd
            .and_then(|f| f.get("recommendationKey"))
            .and_then(|v| v.as_str())
        {
            out += &format!("\n- **Analyst Reco**: {}\n", k);
        }
        if let Some(v) = raw(fd, "targetMeanPrice") {
            out += &format!("- **Mean Target Price**: ${:.2}\n", v);
        }

        out += "\nSource: Yahoo Finance quoteSummary (authenticated).\n";
        Ok(out)
    }

    /// Get company info / fundamentals. Prefers the authenticated quoteSummary
    /// endpoint; falls back to the public chart/search endpoints if it fails.
    pub async fn get_fundamentals(&self, ticker: &str) -> Result<String> {
        match self.fundamentals_quote_summary(ticker).await {
            Ok(s) => Ok(s),
            Err(e) => {
                tracing::warn!(
                    "quoteSummary fundamentals unavailable for {} ({}); using chart/search fallback",
                    ticker,
                    e
                );
                self.fundamentals_basic(ticker).await
            }
        }
    }

    /// Fallback: company info from the public chart `meta` + search endpoints
    /// (no valuation metrics; used when quoteSummary/crumb is unavailable).
    async fn fundamentals_basic(&self, ticker: &str) -> Result<String> {
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
        let json = self
            .get_json(
                "/v1/finance/search",
                &[
                    ("q", ticker.to_string()),
                    ("quotesCount", "0".to_string()),
                    ("newsCount", NEWS_ARTICLE_LIMIT.to_string()),
                ],
            )
            .await?;

        let articles = json
            .get("articles")
            .or_else(|| json.get("news"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut output = format!(
            "## Recent News\n\nRequested date window: {} to {}. Yahoo search may return relevant articles outside this window; use the published dates below for date fidelity.\n\n",
            if start_date.is_empty() { "unspecified" } else { start_date },
            if end_date.is_empty() { "unspecified" } else { end_date }
        );

        if articles.is_empty() {
            output += "No Yahoo Finance news articles returned for this query.\n";
            return Ok(output);
        }

        // Fetch article excerpts in parallel — serial fetches at up to
        // ARTICLE_FETCH_TIMEOUT each dominated get_news latency.
        let mut excerpt_handles = Vec::new();
        for (i, article) in articles.iter().enumerate().take(NEWS_EXCERPT_LIMIT) {
            let Some(link) = article.get("link").and_then(|v| v.as_str()) else {
                continue;
            };
            let client = self.clone();
            let link = link.to_string();
            let ticker = ticker.to_string();
            excerpt_handles.push((
                i,
                tokio::spawn(async move { client.fetch_article_excerpt(&link, &ticker).await }),
            ));
        }
        let mut excerpts = std::collections::HashMap::new();
        for (i, handle) in excerpt_handles {
            if let Ok(Some(excerpt)) = handle.await {
                excerpts.insert(i, excerpt);
            }
        }

        for (i, article) in articles.iter().enumerate().take(NEWS_ARTICLE_LIMIT) {
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
            let published = article
                .get("providerPublishTime")
                .and_then(|v| v.as_i64())
                .map(timestamp_to_date)
                .unwrap_or_else(|| "N/A".to_string());
            let summary = article
                .get("summary")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty());

            output += &format!("### {}. {}\n", i + 1, title);
            output += &format!(
                "Source: {} | Published: {} | [Link]({})\n",
                source, published, link
            );
            if let Some(summary) = summary {
                output += &format!("Summary: {}\n", summary);
            }
            if let Some(excerpt) = excerpts.get(&i) {
                output += &format!("Article excerpt: {}\n", excerpt);
            }
            output += "\n";
        }

        Ok(output)
    }

    async fn fetch_article_excerpt(&self, link: &str, ticker: &str) -> Option<String> {
        if !link.starts_with("http://") && !link.starts_with("https://") {
            return None;
        }

        let response = self
            .client
            .get(link)
            .timeout(ARTICLE_FETCH_TIMEOUT)
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;
        let html = response.text().await.ok()?;
        let text = html_to_text(&html);
        let terms = [
            ticker,
            "contract",
            "customer",
            "guidance",
            "revenue",
            "margin",
            "financing",
            "capex",
            "earnings",
            "outlook",
            "acquisition",
        ];
        excerpt_around_terms(&text, &terms, NEWS_EXCERPT_CHARS)
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
        // Wilder's RSI: seed with the SMA of the first `period` deltas, then
        // recursively smooth. Matches TA-Lib / charting-platform values.
        let mut avg_gain: f64 = gains[..period].iter().sum::<f64>() / period as f64;
        let mut avg_loss: f64 = losses[..period].iter().sum::<f64>() / period as f64;
        for i in period..gains.len() {
            avg_gain = (avg_gain * (period as f64 - 1.0) + gains[i]) / period as f64;
            avg_loss = (avg_loss * (period as f64 - 1.0) + losses[i]) / period as f64;
        }
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

/// EMA over `values`, seeded with the SMA of the first `period` points.
/// Returns one EMA value per close starting at index `period - 1`.
fn ema_series(values: &[f64], period: usize) -> Option<Vec<f64>> {
    if period == 0 || values.len() < period {
        return None;
    }
    let alpha = 2.0 / (period as f64 + 1.0);
    let mut ema = values[..period].iter().sum::<f64>() / period as f64;
    let mut series = Vec::with_capacity(values.len() - period + 1);
    series.push(ema);
    for value in &values[period..] {
        ema = alpha * value + (1.0 - alpha) * ema;
        series.push(ema);
    }
    Some(series)
}

/// MACD(12,26,9): returns (macd line, signal line, histogram) for the latest
/// close. Requires at least 34 data points (26 for the slow EMA + 9 for the
/// signal EMA, overlapping by one).
fn macd(closes: &[f64]) -> Option<(f64, f64, f64)> {
    let ema12 = ema_series(closes, 12)?;
    let ema26 = ema_series(closes, 26)?;
    // ema12[j + 14] and ema26[j] correspond to the same close index (j + 25).
    let macd_line: Vec<f64> = ema26
        .iter()
        .enumerate()
        .map(|(j, e26)| ema12[j + 14] - e26)
        .collect();
    let signal_series = ema_series(&macd_line, 9)?;
    let macd_last = *macd_line.last()?;
    let signal_last = *signal_series.last()?;
    Some((macd_last, signal_last, macd_last - signal_last))
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

fn html_to_text(html: &str) -> String {
    let article_text = extract_paragraph_text(html);
    if !article_text.is_empty() {
        return article_text;
    }

    let without_scripts = remove_html_block(remove_html_block(html, "script"), "style");
    strip_html_tags(&without_scripts)
}

fn extract_paragraph_text(html: &str) -> String {
    let mut paragraphs = Vec::new();
    let mut remaining = html;

    while let Some(open_index) = find_paragraph_open(remaining) {
        let after_open = &remaining[open_index..];
        let Some(tag_end) = after_open.find('>') else {
            break;
        };
        let after_tag = &after_open[tag_end + 1..];
        // ASCII lowercase is byte-length-preserving, so indices found in the
        // lowered copy are valid in the original (to_lowercase() is not: 'İ'
        // expands to 2 chars and would shift every later index).
        let lower_after_tag = after_tag.to_ascii_lowercase();
        let Some(close_index) = lower_after_tag.find("</p>") else {
            break;
        };

        let paragraph = strip_html_tags(&after_tag[..close_index]);
        if is_useful_article_paragraph(&paragraph) {
            paragraphs.push(paragraph);
        }

        remaining = &after_tag[close_index + "</p>".len()..];
    }

    collapse_whitespace(&paragraphs.join(" "))
}

fn find_paragraph_open(input: &str) -> Option<usize> {
    let lower = input.to_ascii_lowercase();
    let mut offset = 0;

    while let Some(relative_index) = lower[offset..].find("<p") {
        let index = offset + relative_index;
        let next = lower[index + 2..].chars().next();
        if matches!(next, Some('>' | ' ' | '\t' | '\n' | '\r')) {
            return Some(index);
        }
        offset = index + 2;
    }

    None
}

fn is_useful_article_paragraph(paragraph: &str) -> bool {
    if paragraph.chars().count() < 40 {
        return false;
    }

    let lower = paragraph.to_lowercase();
    let boilerplate_prefixes = [
        "advertisement",
        "read the full narrative",
        "explore ",
        "find ",
        "opportunities like",
        "this article by",
        "our free ",
        "disagree with existing narratives",
    ];

    !boilerplate_prefixes
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn strip_html_tags(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                text.push(' ');
            }
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }

    collapse_whitespace(&decode_html_entities(&text))
}

fn remove_html_block(input: impl AsRef<str>, tag: &str) -> String {
    let input = input.as_ref();
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);

    loop {
        let lower = remaining.to_ascii_lowercase();
        let Some(start) = lower.find(&open) else {
            output.push_str(remaining);
            break;
        };
        output.push_str(&remaining[..start]);
        let after_start = &remaining[start..];
        let lower_after = after_start.to_ascii_lowercase();
        if let Some(end) = lower_after.find(&close) {
            remaining = &after_start[end + close.len()..];
        } else {
            break;
        }
    }

    output
}

fn decode_html_entities(value: &str) -> String {
    value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn collapse_whitespace(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut last_was_space = false;
    for ch in value.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                output.push(' ');
                last_was_space = true;
            }
        } else {
            output.push(ch);
            last_was_space = false;
        }
    }
    output.trim().to_string()
}

fn excerpt_around_terms(text: &str, terms: &[&str], max_chars: usize) -> Option<String> {
    if text.is_empty() {
        return None;
    }

    let lower = text.to_lowercase();
    let mut best_index = None;
    for term in terms {
        let term = term.trim().to_lowercase();
        if term.is_empty() {
            continue;
        }
        if let Some(index) = lower.find(&term) {
            best_index = Some(best_index.map_or(index, |current: usize| current.min(index)));
        }
    }

    let start = best_index
        .map(|index| index.saturating_sub(max_chars / 3))
        .unwrap_or(0);
    let mut end = (start + max_chars).min(text.len());
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    let mut start = start;
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }

    let mut excerpt = text[start..end].trim().to_string();
    if start > 0 {
        excerpt = format!("...{}", excerpt);
    }
    if end < text.len() {
        excerpt.push_str("...");
    }
    Some(excerpt)
}

/// Tool execution router - routes tool calls to appropriate data sources
/// Validate a ticker symbol to prevent path-traversal and injection attacks.
///
/// Allows alphanumeric characters, dots (for exchange suffixes like `.NS`, `.T`),
/// hyphens (e.g. `BRK-B`), carets (e.g. `^GSPC`), and underscores.
/// Rejects `..`, `/`, `\`, null bytes, and any other suspicious characters.
pub fn validate_ticker(ticker: &str) -> Result<()> {
    if ticker.is_empty() {
        anyhow::bail!("Ticker symbol cannot be empty");
    }
    if ticker.len() > 20 {
        anyhow::bail!("Ticker symbol too long (max 20 chars): '{}'", ticker);
    }
    if ticker.contains("..")
        || ticker.contains('/')
        || ticker.contains('\\')
        || ticker.contains('\0')
    {
        anyhow::bail!(
            "Invalid ticker '{}': contains path-traversal characters",
            ticker
        );
    }
    // Allow only safe characters: alphanumeric, dot, hyphen, caret, underscore
    if !ticker
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '^' | '_'))
    {
        anyhow::bail!(
            "Invalid ticker '{}': contains disallowed characters",
            ticker
        );
    }
    Ok(())
}

pub async fn execute_tool(
    tool: crate::tools::ToolName,
    args: &serde_json::Value,
    client: &YahooFinanceClient,
) -> Result<String> {
    use crate::tools::ToolName;

    match tool {
        ToolName::GetStockData => {
            let symbol = required_tool_arg(args, "symbol")?;
            validate_ticker(symbol)?;
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
            validate_ticker(symbol)?;
            let curr_date = required_tool_arg(args, "curr_date")?;
            // 90 calendar days ≈ 62 trading days — enough history for MACD(12,26,9).
            let look_back = args
                .get("look_back_days")
                .and_then(|v| v.as_i64())
                .map(|v| v.clamp(1, 3650))
                .unwrap_or(90) as i32;
            client.get_indicators(symbol, curr_date, look_back).await
        }
        ToolName::GetFinancials => {
            let ticker = required_tool_arg(args, "ticker")?;
            validate_ticker(ticker)?;
            client.get_fundamentals(ticker).await
        }
        ToolName::GetNews => {
            let ticker = required_tool_arg(args, "ticker")?;
            validate_ticker(ticker)?;
            let start_date = optional_tool_arg(args, "start_date");
            let end_date = optional_tool_arg(args, "end_date");
            client.get_news(ticker, start_date, end_date).await
        }
        ToolName::GetGlobalNews => {
            let curr_date = required_tool_arg(args, "curr_date")?;
            // ^GSPC is a hardcoded safe value, no validation needed
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
    fn test_benchmark_for_ticker_regional() {
        assert_eq!(benchmark_for_ticker("RELIANCE.NS"), "^NSEI");
        assert_eq!(benchmark_for_ticker("7203.T"), "^N225");
        assert_eq!(benchmark_for_ticker("0700.HK"), "^HSI");
        assert_eq!(benchmark_for_ticker("VOD.L"), "^FTSE");
        assert_eq!(benchmark_for_ticker("SHOP.TO"), "^GSPTSE");
        assert_eq!(benchmark_for_ticker("CBA.AX"), "^AXJO");
        assert_eq!(benchmark_for_ticker("600519.SS"), "000001.SS");
        assert_eq!(benchmark_for_ticker("SAP.DE"), "^STOXX50E");
    }

    #[test]
    fn test_benchmark_for_ticker_us_default() {
        assert_eq!(benchmark_for_ticker("AAPL"), "SPY");
        assert_eq!(benchmark_for_ticker("NVDA"), "SPY");
        assert_eq!(benchmark_for_ticker("BRK-B"), "SPY");
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
    fn test_parse_chart_quotes_rejects_missing_price_arrays() {
        // "open" array absent — must error, not substitute timestamps as prices
        let json = serde_json::json!({
            "chart": {"result": [{
                "timestamp": [1700000000i64],
                "indicators": {"quote": [{
                    "high": [2.0], "low": [0.5], "close": [1.5], "volume": [100]
                }]}
            }]}
        });
        let err = YahooFinanceClient::parse_chart_quotes(&json, "TEST")
            .unwrap_err()
            .to_string();
        assert!(err.contains("open"));
    }

    #[test]
    fn test_parse_chart_quotes_missing_volume_defaults_to_zero() {
        let json = serde_json::json!({
            "chart": {"result": [{
                "timestamp": [1700000000i64],
                "indicators": {"quote": [{
                    "open": [1.0], "high": [2.0], "low": [0.5], "close": [1.5]
                }]}
            }]}
        });
        let quotes = YahooFinanceClient::parse_chart_quotes(&json, "test").unwrap();
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].open, 1.0);
        assert_eq!(quotes[0].volume, 0);
        assert_eq!(quotes[0].symbol, "TEST");
    }

    #[test]
    fn test_macd_insufficient_history() {
        let values: Vec<f64> = (1..=20).map(|v| v as f64).collect();
        assert!(macd(&values).is_none());
    }

    #[test]
    fn test_macd_constant_prices_is_zero() {
        let values = vec![50.0; 40];
        let (m, s, h) = macd(&values).unwrap();
        assert!(m.abs() < 1e-9);
        assert!(s.abs() < 1e-9);
        assert!(h.abs() < 1e-9);
    }

    #[test]
    fn test_macd_uptrend_is_positive() {
        let values: Vec<f64> = (1..=60).map(|v| v as f64).collect();
        let (m, s, _h) = macd(&values).unwrap();
        assert!(m > 0.0);
        assert!(s > 0.0);
    }

    #[test]
    fn test_bollinger_bands() {
        let values: Vec<f64> = (1..=20).map(|value| value as f64).collect();
        let (upper, middle, lower) = bollinger_bands(&values, 20).unwrap();
        assert!(upper > middle);
        assert!(middle > lower);
    }

    #[test]
    fn test_html_to_text_prefers_article_paragraphs() {
        let html = r#"
            <html>
                <title>IREN Microsoft AI Deal</title>
                <nav>Skip to navigation News Weather Shopping</nav>
                <script>var noise = "Microsoft";</script>
                <svg><path d="M11 15h2v2h-2"></path></svg>
                <progress max="100" value="0"></progress>
                <p>In recent weeks, IREN secured a five-year, US$9.70 billion agreement with Microsoft for AI infrastructure capacity.</p>
                <p>The March 2026 plan includes over 50,000 NVIDIA B300 GPUs, taking the fleet to 150,000 units.</p>
            </html>
        "#;

        let text = html_to_text(html);
        assert!(text.starts_with("In recent weeks"));
        assert!(text.contains("US$9.70 billion agreement with Microsoft"));
        assert!(text.contains("50,000 NVIDIA B300 GPUs"));
        assert!(!text.contains("Skip to navigation"));
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

    #[tokio::test]
    #[ignore = "hits live Yahoo Finance endpoints"]
    async fn test_live_iren_news_extracts_article_evidence() {
        let client = YahooFinanceClient::new();
        let news = client
            .get_news("IREN", "2026-04-01", "2026-05-01")
            .await
            .unwrap();

        // Assert the excerpt mechanism works, not what any specific article
        // said on a given day (Yahoo's result set drifts over time).
        assert!(news.contains("Recent News"));
        assert!(
            news.contains("Article excerpt:"),
            "no article excerpts extracted:\n{news}"
        );
        let has_financial_substance = ["contract", "revenue", "earnings", "$"]
            .iter()
            .any(|t| news.to_lowercase().contains(&t.to_lowercase()));
        assert!(
            has_financial_substance,
            "excerpts lack financial substance:\n{news}"
        );
    }

    #[test]
    fn test_validate_ticker_accepts_valid() {
        assert!(validate_ticker("AAPL").is_ok());
        assert!(validate_ticker("BRK-B").is_ok());
        assert!(validate_ticker("7203.T").is_ok());
        assert!(validate_ticker("^GSPC").is_ok());
        assert!(validate_ticker("RELIANCE.NS").is_ok());
        assert!(validate_ticker("0700.HK").is_ok());
    }

    #[test]
    fn test_validate_ticker_rejects_traversal() {
        assert!(validate_ticker("../etc/passwd").is_err());
        assert!(validate_ticker("AAPL/../../secret").is_err());
        assert!(validate_ticker("AAPL\\..\\secret").is_err());
        assert!(validate_ticker("..").is_err());
    }

    #[test]
    fn test_validate_ticker_rejects_special_chars() {
        assert!(validate_ticker("").is_err());
        assert!(validate_ticker("AAPL;rm -rf /").is_err());
        assert!(validate_ticker("A\0APL").is_err());
        assert!(validate_ticker("AAPL MSFT").is_err());
        assert!(validate_ticker("A".repeat(21).as_str()).is_err());
    }
}
