//! Graph execution engine
//!
//! This is a simple state machine that replaces LangGraph's StateGraph.
//! It routes between nodes based on conditional logic and maintains state.

use crate::data::yfinance::{self, YahooFinanceClient};
use crate::graph::state::{AgentState, InvestDebateState, RiskDebateState};
use crate::llm::{AnthropicContentBlock, AnthropicMessage, LLMClient, Tool};
use crate::memory::{BM25Memory, DecisionLog, MemoryMatch};
use crate::tools::{ToolName, ToolRegistry};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

const EVIDENCE_DISCIPLINE: &str = r#"Evidence discipline:
- Apply these instructions silently; do not quote or refer to this instruction block in the report.
- Treat tool outputs and analyst reports as the evidence base; do not infer that a contract, catalyst, or metric is absent merely because one tool did not return it.
- If evidence is missing or thin, say "not observed in current tool output" rather than "absent", "no confirmed", or "does not exist".
- Preserve named customer contracts, GPU/data-center capacity, financing events, published article dates, and upcoming earnings/catalysts from the news report through the final decision.
- Prefer raw quantities over derived percentages; only compute ratios when necessary, and check the arithmetic before using them.
- Separate business/thesis quality from current-entry quality.
"#;

const DECISION_CALIBRATION: &str = r#"Decision calibration:
- Apply these rules silently; do not quote, cite, or mention this calibration block in the report. Avoid phrases such as "per decision guidelines", "decision calibration", or "instruction".
- Separate signed contracts, backlog, and annualized revenue targets from recognized revenue, margins, free cash flow, and balance-sheet capacity.
- Treat analyst price targets and fair-value estimates as sentiment/reference inputs, not proof of intrinsic value or guaranteed upside.
- Do not calculate risk/reward only from third-party price targets; anchor downside and upside to observed price levels, catalysts, and fundamentals.
- Do not assign numeric scenario probabilities unless they come from an explicit quantitative model; use qualitative scenario labels instead.
- If market cap, revenue, EPS, debt, cash flow, or comparable valuation data are not observed, avoid "high-conviction" language and cap confidence at Medium.
- A BUY at the current price requires more than a strong business thesis: current entry quality must also be attractive after considering support/resistance, execution risk, financing/dilution, and missing fundamentals.
- When the business thesis is strong but entry/valuation evidence is incomplete, prefer OVERWEIGHT or HOLD rather than an unconditional BUY. Use UNDERWEIGHT when risks outweigh the thesis but liquidation is premature.
- Do not say a large contract eliminates execution risk; it validates demand but deployment, financing, concentration, and revenue-recognition risks remain.
- Keep position sizing qualitative and research-oriented unless the user supplied a risk profile; avoid exact portfolio-allocation percentages and personalized sizing by investor profile.
"#;

// =============================================================================
// Graph Configuration
// =============================================================================

#[derive(Debug, Clone)]
pub struct GraphConfig {
    #[allow(dead_code)]
    pub company: String,
    #[allow(dead_code)]
    pub trade_date: String,
    pub max_debate_rounds: usize,
    pub max_risk_discuss_rounds: usize,
    #[allow(dead_code)]
    pub max_recur_limit: usize,
    #[allow(dead_code)]
    pub output_language: String,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            company: String::new(),
            trade_date: String::new(),
            max_debate_rounds: 1,
            max_risk_discuss_rounds: 1,
            max_recur_limit: 100,
            output_language: "English".to_string(),
        }
    }
}

// =============================================================================
// Graph Engine
// =============================================================================

pub struct GraphEngine {
    config: GraphConfig,
    llm_quick: Arc<dyn LLMClient>,
    llm_deep: Arc<dyn LLMClient>,
    tool_registry: ToolRegistry,
    yfinance: YahooFinanceClient,
    bull_memory: RwLock<BM25Memory>,
    bear_memory: RwLock<BM25Memory>,
    trader_memory: RwLock<BM25Memory>,
    decision_log: RwLock<DecisionLog>,
    /// Per-process cache of tool results keyed by tool name + args.
    /// Dedupes repeated fetches within a run (social + news analysts both pull
    /// ticker news) and across batch tickers (global news is date-keyed only).
    tool_cache: RwLock<HashMap<String, String>>,
}

impl GraphEngine {
    pub fn new(
        config: GraphConfig,
        llm_quick: Arc<dyn LLMClient>,
        llm_deep: Arc<dyn LLMClient>,
    ) -> Self {
        let decision_log_path = DecisionLog::default_path();
        let decision_log = DecisionLog::load(&decision_log_path, Some(100));
        Self {
            config,
            llm_quick,
            llm_deep,
            tool_registry: ToolRegistry::new(),
            yfinance: YahooFinanceClient::new(),
            bull_memory: RwLock::new(BM25Memory::from_file(
                "bull",
                &BM25Memory::default_path("bull"),
            )),
            bear_memory: RwLock::new(BM25Memory::from_file(
                "bear",
                &BM25Memory::default_path("bear"),
            )),
            trader_memory: RwLock::new(BM25Memory::from_file(
                "trader",
                &BM25Memory::default_path("trader"),
            )),
            decision_log: RwLock::new(decision_log),
            tool_cache: RwLock::new(HashMap::new()),
        }
    }

    /// Execute a tool through the per-process result cache. Errors are not cached.
    async fn cached_tool(&self, tool: ToolName, args: &serde_json::Value) -> Result<String> {
        // serde_json::Map is a BTreeMap by default, so `args` serializes with
        // sorted keys — the cache key is deterministic.
        let key = format!("{}:{}", tool.as_str(), args);
        if let Some(hit) = self.tool_cache.read().await.get(&key) {
            tracing::debug!("Tool cache hit: {}", key);
            return Ok(hit.clone());
        }
        let result = yfinance::execute_tool(tool, args, &self.yfinance).await?;
        self.tool_cache.write().await.insert(key, result.clone());
        Ok(result)
    }

    /// Run the full analysis for a ticker
    pub async fn run(&self, mut state: AgentState) -> Result<AgentState> {
        tracing::info!("Starting analysis for {}", state.company_of_interest);

        // ========== PHASE 1: Parallel Analysts ==========
        tracing::info!("Phase 1: Running analysts in parallel...");
        let t = std::time::Instant::now();
        let analyst_results = self.run_analysts(&state).await?;
        tracing::info!("Phase 1 done in {:.1}s", t.elapsed().as_secs_f64());
        state.market_report = analyst_results.market.clone();
        state.sentiment_report = analyst_results.social.clone();
        state.news_report = analyst_results.news.clone();
        state.fundamentals_report = analyst_results.fundamentals.clone();

        // ========== PHASE 2: Bull/Bear Debate ==========
        tracing::info!("Phase 2: Running bull/bear debate...");
        let t = std::time::Instant::now();
        self.run_bull_bear_debate(&mut state).await?;
        tracing::info!("Phase 2 done in {:.1}s", t.elapsed().as_secs_f64());

        // ========== PHASE 3: Research Manager ==========
        tracing::info!("Phase 3: Research manager synthesis...");
        let t = std::time::Instant::now();
        let investment_plan = self.run_research_manager(&state).await?;
        tracing::info!("Phase 3 done in {:.1}s", t.elapsed().as_secs_f64());
        state.investment_plan = investment_plan;

        // ========== PHASE 4: Trader ==========
        tracing::info!("Phase 4: Trader decision...");
        let t = std::time::Instant::now();
        let trader_plan = self.run_trader(&state).await?;
        tracing::info!("Phase 4 done in {:.1}s", t.elapsed().as_secs_f64());
        state.trader_investment_plan = trader_plan;

        // ========== PHASE 5: Risk Debate ==========
        tracing::info!("Phase 5: Risk debate...");
        let t = std::time::Instant::now();
        self.run_risk_debate(&mut state).await?;
        tracing::info!("Phase 5 done in {:.1}s", t.elapsed().as_secs_f64());

        // ========== PHASE 6: Portfolio Manager ==========
        tracing::info!("Phase 6: Portfolio manager synthesis...");
        let t = std::time::Instant::now();
        let decision = self.run_portfolio_manager(&state).await?;
        tracing::info!("Phase 6 done in {:.1}s", t.elapsed().as_secs_f64());
        state.final_trade_decision = decision;

        // ========== Persist Decision ==========
        self.persist_decision(&state).await;

        tracing::info!("Analysis complete for {}", state.company_of_interest);
        Ok(state)
    }

    /// Append an output-language directive when a non-English language is
    /// configured (via TRADING_AGENT_OUTPUT_LANG). No-op for English.
    fn with_lang(&self, prompt: &str) -> String {
        let lang = self.config.output_language.trim();
        if lang.is_empty() || lang.eq_ignore_ascii_case("english") || lang.eq_ignore_ascii_case("en") {
            return prompt.to_string();
        }
        format!(
            "{prompt}\n\n=== OUTPUT LANGUAGE (MANDATORY) ===\nWrite your ENTIRE response in {lang}. Every heading, sentence, the Rating line, and all section titles MUST be in {lang}. Keep ticker symbols, numbers, prices, and currency symbols unchanged. Do not switch to English."
        )
    }

    // -------------------------------------------------------------------------
    // PHASE 1: Parallel Analysts
    // -------------------------------------------------------------------------

    async fn run_analysts(&self, state: &AgentState) -> Result<AnalystResults> {
        use tokio::join;

        let ticker = &state.company_of_interest;
        let date = &state.trade_date;

        // Run all 4 analysts in parallel
        let (market, social, news, fundamentals) = join!(
            self.run_market_analyst(ticker, date),
            self.run_social_analyst(ticker, date),
            self.run_news_analyst(ticker, date),
            self.run_fundamentals_analyst(ticker, date),
        );

        // One failed analyst degrades to an "unavailable" note so the other
        // three still produce a decision; only abort when all four failed.
        let all_failed =
            market.is_err() && social.is_err() && news.is_err() && fundamentals.is_err();
        if all_failed {
            anyhow::bail!(
                "All four analysts failed for {}; first error: {}",
                ticker,
                market.unwrap_err()
            );
        }

        Ok(AnalystResults {
            market: report_or_unavailable("Market", market),
            social: report_or_unavailable("Social sentiment", social),
            news: report_or_unavailable("News", news),
            fundamentals: report_or_unavailable("Fundamentals", fundamentals),
        })
    }

    async fn run_market_analyst(&self, ticker: &str, date: &str) -> Result<String> {
        // Compute a 30-day lookback for stock data
        let end = date;
        let start = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map(|d| {
                (d - chrono::Duration::days(30))
                    .format("%Y-%m-%d")
                    .to_string()
            })
            .unwrap_or_else(|_| date.to_string());

        let benchmark = yfinance::benchmark_for_ticker(ticker);

        // Prefetch the data the prompt prescribes — saves LLM tool round trips.
        let stock_args =
            serde_json::json!({"symbol": ticker, "start_date": start, "end_date": end});
        let indicator_args = serde_json::json!({"symbol": ticker, "curr_date": end});
        let (stock_data, indicators) = tokio::join!(
            self.cached_tool(ToolName::GetStockData, &stock_args),
            self.cached_tool(ToolName::GetIndicators, &indicator_args),
        );
        let benchmark_block = if benchmark != ticker {
            let bm_args =
                serde_json::json!({"symbol": benchmark, "start_date": start, "end_date": end});
            let bm_data = self.cached_tool(ToolName::GetStockData, &bm_args).await;
            prefetched_block(&format!("Benchmark {benchmark} stock data"), &bm_data)
        } else {
            String::new()
        };

        let prompt = format!(
            r#"You are a market analyst. Analyze the stock data for {ticker} on {date}.
            Regional benchmark for alpha comparison: {benchmark}

            Pre-fetched data:
            {stock_block}
            {indicators_block}
            {benchmark_block}

            Tools are available if you need additional data (e.g., a different date window).

            Provide a concise market analysis covering:
            - Current price trend
            - Volume analysis
            - Key technical indicators (RSI, MACD, Bollinger position)
            - Support/resistance levels if apparent
            - Relative performance vs {benchmark} (alpha)

            End with: FINAL MARKET ANALYSIS: **SUMMARY**"#,
            stock_block = prefetched_block(&format!("{ticker} stock data"), &stock_data),
            indicators_block = prefetched_block("Technical indicators", &indicators),
        );

        let tools = vec![
            self.tool_registry.get_by_name(ToolName::GetStockData),
            self.tool_registry.get_by_name(ToolName::GetIndicators),
        ];

        self.execute_llm_with_tools(&prompt, &tools).await
    }

    async fn run_social_analyst(&self, ticker: &str, date: &str) -> Result<String> {
        let (news_start, news_end) = news_window(date);
        let news_args =
            serde_json::json!({"ticker": ticker, "start_date": news_start, "end_date": news_end});
        let news = self.cached_tool(ToolName::GetNews, &news_args).await;

        let prompt = format!(
            r#"You are a social media sentiment analyst. Analyze sentiment for {ticker} on {date}.

            {evidence_discipline}

            Pre-fetched data:
            {news_block}

            Tools are available if you need additional data.

            Provide a concise sentiment analysis covering:
            - Overall investor sentiment (bullish/bearish/neutral)
            - Key themes in recent coverage
            - Notable positive or negative catalysts
            - Social media trends if apparent

            End with: FINAL SENTIMENT ANALYSIS: **POSITIVE/NEGATIVE/NEUTRAL**"#,
            evidence_discipline = EVIDENCE_DISCIPLINE,
            news_block = prefetched_block("Recent news and sentiment", &news),
        );

        let tools = vec![self.tool_registry.get_by_name(ToolName::GetNews)];

        self.execute_llm_with_tools(&prompt, &tools).await
    }

    async fn run_news_analyst(&self, ticker: &str, date: &str) -> Result<String> {
        let (news_start, news_end) = news_window(date);
        let news_args =
            serde_json::json!({"ticker": ticker, "start_date": news_start, "end_date": news_end});
        let global_args = serde_json::json!({"curr_date": date});
        let (news, global_news) = tokio::join!(
            self.cached_tool(ToolName::GetNews, &news_args),
            self.cached_tool(ToolName::GetGlobalNews, &global_args),
        );

        let prompt = format!(
            r#"You are a news analyst. Analyze news impact for {ticker} on {date}.

            {evidence_discipline}

            {decision_calibration}

            Pre-fetched data:
            {news_block}
            {global_news_block}

            Tools are available if you need additional data.

            Provide a concise news analysis covering:
            - Key news items affecting the stock
            - Named contracts, customer wins, GPU/data-center capacity, financing, and upcoming earnings/catalysts if present in the news
            - A separation between reported company facts and analyst opinions/price targets
            - Impact on near-term outlook
            - Risks or opportunities from recent developments

            End with: FINAL NEWS ANALYSIS: **IMPACT SUMMARY**"#,
            evidence_discipline = EVIDENCE_DISCIPLINE,
            decision_calibration = DECISION_CALIBRATION,
            news_block = prefetched_block(&format!("{ticker} news"), &news),
            global_news_block = prefetched_block("Global market news", &global_news),
        );

        let tools = vec![
            self.tool_registry.get_by_name(ToolName::GetNews),
            self.tool_registry.get_by_name(ToolName::GetGlobalNews),
        ];

        self.execute_llm_with_tools(&prompt, &tools).await
    }

    async fn run_fundamentals_analyst(&self, ticker: &str, date: &str) -> Result<String> {
        let financial_args = serde_json::json!({"ticker": ticker});
        let financials = self
            .cached_tool(ToolName::GetFinancials, &financial_args)
            .await;

        let prompt = format!(
            r#"You are a fundamentals analyst. Analyze fundamental data for {ticker} on {date}.

            {evidence_discipline}

            {decision_calibration}

            Pre-fetched data:
            {financials_block}

            Detailed financial statements are not available natively; treat metrics that are not returned as not observed. Tools are available if you need additional data.

            Provide a concise fundamentals analysis covering:
            - Business model and competitive position
            - Key financial metrics (P/E, EPS, revenue growth)
            - Valuation assessment
            - Whether any valuation view is supported by reported financials or only by external targets/narrative
            - Key risks and strengths

            End with: FINAL FUNDAMENTALS ANALYSIS: **STRONG/WEAK/FAIR**"#,
            evidence_discipline = EVIDENCE_DISCIPLINE,
            decision_calibration = DECISION_CALIBRATION,
            financials_block = prefetched_block("Company overview", &financials),
        );

        let tools = vec![self.tool_registry.get_by_name(ToolName::GetFinancials)];

        self.execute_llm_with_tools(&prompt, &tools).await
    }

    // -------------------------------------------------------------------------
    // PHASE 2: Bull/Bear Debate
    // -------------------------------------------------------------------------

    async fn run_bull_bear_debate(&self, state: &mut AgentState) -> Result<()> {
        let situation = state.situation_summary();

        // Get memories
        let bull_memories = self.bull_memory.read().await.get_memories(&situation, 2);
        let bear_memories = self.bear_memory.read().await.get_memories(&situation, 2);

        let memory_context = format!(
            "Past lessons:\nBull memories:\n{}\nBear memories:\n{}",
            format_memory_matches(&bull_memories),
            format_memory_matches(&bear_memories)
        );

        let mut debate_state = InvestDebateState::default();
        let max_rounds = self.config.max_debate_rounds;

        // Run debate rounds (bull & bear argue in parallel)
        for round in 0..max_rounds {
            tracing::info!("Bull/Bear debate round {}", round + 1);

            let prior_history = debate_state.history.clone();

            // Bull and Bear argue independently in parallel
            let bull_prompt = format!(
                r#"You are a bullish researcher arguing FOR investing in {company}.

                Context from analysts:
                Market: {market}
                Sentiment: {sentiment}
                News: {news}
                Fundamentals: {fundamentals}

                {evidence_discipline}

                {decision_calibration}

                {memory_context}
                {prior}

                Provide your bullish argument focusing on:
                - Growth catalysts
                - Competitive advantages
                - Positive indicators

                Format: Start with "Bull:" and make your case concisely (max 300 words).
                "#,
                company = state.company_of_interest,
                market = state.market_report,
                sentiment = state.sentiment_report,
                news = state.news_report,
                fundamentals = state.fundamentals_report,
                evidence_discipline = EVIDENCE_DISCIPLINE,
                decision_calibration = DECISION_CALIBRATION,
                prior = if prior_history.is_empty() {
                    String::new()
                } else {
                    format!("Prior debate:\n{}", prior_history)
                },
            );

            let bear_prompt = format!(
                r#"You are a bearish researcher arguing AGAINST investing in {company}.

                Context from analysts:
                Market: {market}
                Sentiment: {sentiment}
                News: {news}
                Fundamentals: {fundamentals}

                {evidence_discipline}

                {decision_calibration}

                {memory_context}
                {prior}

                Provide your bearish counter-argument focusing on:
                - Risks and challenges
                - Negative indicators
                - Overvaluation concerns

                Format: Start with "Bear:" and make your case concisely (max 300 words).
                "#,
                company = state.company_of_interest,
                market = state.market_report,
                sentiment = state.sentiment_report,
                news = state.news_report,
                fundamentals = state.fundamentals_report,
                evidence_discipline = EVIDENCE_DISCIPLINE,
                decision_calibration = DECISION_CALIBRATION,
                prior = if prior_history.is_empty() {
                    String::new()
                } else {
                    format!("Prior debate:\n{}", prior_history)
                },
            );

            let (bull_response, bear_response) = tokio::try_join!(
                self.llm_quick.complete(&bull_prompt),
                self.llm_quick.complete(&bear_prompt),
            )?;

            debate_state
                .bull_history
                .push_str(&format!("\nBull: {}", bull_response));
            debate_state
                .bear_history
                .push_str(&format!("\nBear: {}", bear_response));
            debate_state.history.push_str(&format!(
                "Round {} Bull: {} Bear: {}",
                round + 1,
                bull_response,
                bear_response
            ));
            debate_state.current_response = bear_response;
            debate_state.count += 1;
        }

        state.investment_debate_state = debate_state;
        Ok(())
    }

    // -------------------------------------------------------------------------
    // PHASE 3: Research Manager
    // -------------------------------------------------------------------------

    async fn run_research_manager(&self, state: &AgentState) -> Result<String> {
        let prompt = format!(
            r#"You are a research manager synthesizing the bull/bear debate for {company}.

            Debate history:
            {history}

            Investment plans from analysts:
            Market: {market}
            Sentiment: {sentiment}
            News: {news}
            Fundamentals: {fundamentals}

            {evidence_discipline}

            {decision_calibration}

            Provide a comprehensive investment plan that:
            - Weighs the bull and bear arguments
            - Provides a clear rating: BUY / OVERWEIGHT / HOLD / UNDERWEIGHT / SELL
            - Outlines specific strategic actions
            - Justifies the reasoning

            End with: INVESTMENT DECISION: **{{BUY|OVERWEIGHT|HOLD|UNDERWEIGHT|SELL}}** with confidence level
            "#,
            company = state.company_of_interest,
            history = state.investment_debate_state.history,
            market = state.market_report,
            sentiment = state.sentiment_report,
            news = state.news_report,
            fundamentals = state.fundamentals_report,
            evidence_discipline = EVIDENCE_DISCIPLINE,
            decision_calibration = DECISION_CALIBRATION,
        );

        self.llm_deep.complete(&self.with_lang(&prompt)).await
    }

    // -------------------------------------------------------------------------
    // PHASE 4: Trader
    // -------------------------------------------------------------------------

    async fn run_trader(&self, state: &AgentState) -> Result<String> {
        let situation = state.situation_summary();
        let memories = self.trader_memory.read().await.get_memories(&situation, 2);

        let memory_context = format!(
            "Past trading lessons:\n{}",
            format_memory_matches(&memories)
        );

        let prompt = format!(
            r#"You are a trader converting the investment plan into a specific trading decision for {company}.

            Investment plan: {investment_plan}

            Analyst reports:
            Market: {market}
            Sentiment: {sentiment}
            News: {news}
            Fundamentals: {fundamentals}

            {evidence_discipline}

            {decision_calibration}

            {memory_context}

            Provide a specific trading recommendation:
            - BUY/HOLD/SELL with exact entry points if applicable
            - Position sizing guidance
            - Risk management rules
            - Expected timeframe

            End with: FINAL TRADE: **{{BUY|HOLD|SELL}}** at $XXX or current price
            "#,
            company = state.company_of_interest,
            investment_plan = state.investment_plan,
            market = state.market_report,
            sentiment = state.sentiment_report,
            news = state.news_report,
            fundamentals = state.fundamentals_report,
            evidence_discipline = EVIDENCE_DISCIPLINE,
            decision_calibration = DECISION_CALIBRATION,
        );

        self.llm_quick.complete(&self.with_lang(&prompt)).await
    }

    // -------------------------------------------------------------------------
    // PHASE 5: Risk Debate (Parallel)
    // -------------------------------------------------------------------------

    async fn run_risk_debate(&self, state: &mut AgentState) -> Result<()> {
        let mut risk_state = RiskDebateState::default();
        let max_rounds = self.config.max_risk_discuss_rounds;

        for round in 0..max_rounds {
            tracing::info!("Risk debate round {}", round + 1);

            // Run all 3 risk analysts in parallel
            let (agg_result, cons_result, neut_result) = tokio::join!(
                self.run_aggressive_risk(state),
                self.run_conservative_risk(state),
                self.run_neutral_risk(state),
            );

            // Extract string values before ? moves them
            let agg_response = agg_result?;
            let cons_response = cons_result?;
            let neut_response = neut_result?;

            risk_state
                .aggressive_history
                .push_str(&format!("\nAggressive: {}", agg_response));
            risk_state
                .conservative_history
                .push_str(&format!("\nConservative: {}", cons_response));
            risk_state
                .neutral_history
                .push_str(&format!("\nNeutral: {}", neut_response));
            risk_state.history.push_str(&format!(
                "\nRound {}:\nAggressive: {}\nConservative: {}\nNeutral: {}",
                round + 1,
                agg_response,
                cons_response,
                neut_response
            ));
            risk_state.current_aggressive_response = agg_response;
            risk_state.current_conservative_response = cons_response;
            risk_state.current_neutral_response = neut_response;
            risk_state.count += 1;
            risk_state.latest_speaker = "Neutral".to_string();
        }

        state.risk_debate_state = risk_state;
        Ok(())
    }

    async fn run_aggressive_risk(&self, state: &AgentState) -> Result<String> {
        let prompt = format!(
            r#"You are an aggressive risk analyst championing high-risk, high-reward strategies.

            Trader's plan: {trader_plan}

            Analyst reports:
            Market: {market}
            News: {news}
            Fundamentals: {fundamentals}

            {evidence_discipline}

            {decision_calibration}

            Provide your aggressive risk assessment:
            - Maximizing upside scenarios
            - Why risk is worth taking
            - Position sizing for maximum gains

            Format: Start with "Aggressive:" and state your position concisely (max 300 words).
            "#,
            trader_plan = state.trader_investment_plan,
            market = state.market_report,
            news = state.news_report,
            fundamentals = state.fundamentals_report,
            evidence_discipline = EVIDENCE_DISCIPLINE,
            decision_calibration = DECISION_CALIBRATION,
        );

        self.llm_quick.complete(&self.with_lang(&prompt)).await
    }

    async fn run_conservative_risk(&self, state: &AgentState) -> Result<String> {
        let prompt = format!(
            r#"You are a conservative risk analyst focused on protecting capital.

            Trader's plan: {trader_plan}

            Analyst reports:
            Market: {market}
            News: {news}
            Fundamentals: {fundamentals}

            {evidence_discipline}

            {decision_calibration}

            Provide your conservative risk assessment:
            - Downside protection
            - Volatility concerns
            - Risk mitigation strategies

            Format: Start with "Conservative:" and state your position concisely (max 300 words).
            "#,
            trader_plan = state.trader_investment_plan,
            market = state.market_report,
            news = state.news_report,
            fundamentals = state.fundamentals_report,
            evidence_discipline = EVIDENCE_DISCIPLINE,
            decision_calibration = DECISION_CALIBRATION,
        );

        self.llm_quick.complete(&self.with_lang(&prompt)).await
    }

    async fn run_neutral_risk(&self, state: &AgentState) -> Result<String> {
        let prompt = format!(
            r#"You are a neutral risk analyst balancing risk and reward.

            Trader's plan: {trader_plan}

            Analyst reports:
            Market: {market}
            News: {news}
            Fundamentals: {fundamentals}

            {evidence_discipline}

            {decision_calibration}

            Provide your balanced risk assessment:
            - Weighing upside potential vs downside risk
            - Moderate position sizing
            - Key risk metrics to monitor

            Format: Start with "Neutral:" and state your position concisely (max 300 words).
            "#,
            trader_plan = state.trader_investment_plan,
            market = state.market_report,
            news = state.news_report,
            fundamentals = state.fundamentals_report,
            evidence_discipline = EVIDENCE_DISCIPLINE,
            decision_calibration = DECISION_CALIBRATION,
        );

        self.llm_quick.complete(&self.with_lang(&prompt)).await
    }

    // -------------------------------------------------------------------------
    // PHASE 6: Portfolio Manager
    // -------------------------------------------------------------------------

    async fn run_portfolio_manager(&self, state: &AgentState) -> Result<String> {
        let decision_history = self
            .decision_log
            .read()
            .await
            .format_context(&state.company_of_interest);

        let prompt = format!(
            r#"You are a portfolio manager making the final trading decision for {company}.

            Risk debate summary:
            {risk_debate}

            Trader's recommendation: {trader_plan}

            Investment thesis: {investment_plan}

            Original analyst evidence:
            Market: {market}
            Sentiment: {sentiment}
            News: {news}
            Fundamentals: {fundamentals}

            {decision_history}

            {evidence_discipline}

            {decision_calibration}

            Provide your final decision with:
            - Rating: one of BUY / OVERWEIGHT / HOLD / UNDERWEIGHT / SELL
            - Executive summary
            - Investment thesis
            - Current price judgment for new money, existing holders, adding, and reducing/selling
            - Evidence quality and data gaps, especially whether valuation is supported by observed financials or only by external targets
            - Key risks and monitoring points

            Avoid personalized allocation guidance. End with: FINAL RATING: **{{BUY|OVERWEIGHT|HOLD|UNDERWEIGHT|SELL}}** with confidence level and 1-sentence justification
            "#,
            company = state.company_of_interest,
            risk_debate = state.risk_debate_state.history,
            trader_plan = state.trader_investment_plan,
            investment_plan = state.investment_plan,
            market = state.market_report,
            sentiment = state.sentiment_report,
            news = state.news_report,
            fundamentals = state.fundamentals_report,
            decision_history = decision_history,
            evidence_discipline = EVIDENCE_DISCIPLINE,
            decision_calibration = DECISION_CALIBRATION,
        );

        self.llm_deep.complete(&self.with_lang(&prompt)).await
    }

    // -------------------------------------------------------------------------
    // Decision persistence
    // -------------------------------------------------------------------------

    /// Extract rating and confidence from the final decision text and persist.
    async fn persist_decision(&self, state: &AgentState) {
        let decision_text = &state.final_trade_decision;
        let (rating, confidence) = extract_rating_and_confidence(decision_text);
        // Use first ~300 chars as summary
        let summary: String = decision_text.chars().take(300).collect();

        {
            let mut log = self.decision_log.write().await;
            log.log_decision(
                &state.company_of_interest,
                &state.trade_date,
                &rating,
                &confidence,
                &summary,
            );
            if let Err(e) = log.save() {
                tracing::warn!("Failed to persist decision log: {}", e);
            }
        }

        // Store this run as a retrievable "past lesson" for future debates.
        let situation = state.situation_summary();
        let lesson = format!("{} (confidence: {}): {}", rating, confidence, summary);
        for (memory, name) in [
            (&self.bull_memory, "bull"),
            (&self.bear_memory, "bear"),
            (&self.trader_memory, "trader"),
        ] {
            let mut mem = memory.write().await;
            mem.add(&situation, &lesson);
            if let Err(e) = mem.save(&BM25Memory::default_path(name)) {
                tracing::warn!("Failed to persist {} memory: {}", name, e);
            }
        }
    }

    // -------------------------------------------------------------------------
    // Helper: Execute LLM with tool calling (proper multi-turn message history)
    // -------------------------------------------------------------------------

    async fn execute_llm_with_tools(&self, prompt: &str, tools: &[Tool]) -> Result<String> {
        const MAX_ROUNDS: usize = 3;

        // Build proper message history: each user/assistant turn is a separate message.
        // This allows the LLM to correctly attribute tool results to tool calls.
        let mut messages = vec![AnthropicMessage {
            role: "user".to_string(),
            content: vec![AnthropicContentBlock::Text(prompt.to_string())],
        }];

        for round in 0..MAX_ROUNDS {
            // Use complete_messages for proper Anthropic multi-turn format
            let response = self
                .llm_quick
                .complete_messages(messages.clone(), tools)
                .await?;

            let tool_calls = match response.tool_calls {
                Some(tc) if !tc.is_empty() => tc,
                _ => return Ok(response.content),
            };

            // Add assistant message with tool_use blocks (one per tool call)
            let tool_use_ids: Vec<String> = tool_calls
                .iter()
                .enumerate()
                .map(|(i, tc)| {
                    if tc.id.is_empty() {
                        format!("tc_{}_{}", round, i)
                    } else {
                        tc.id.clone()
                    }
                })
                .collect();

            let assistant_blocks: Vec<AnthropicContentBlock> = tool_calls
                .iter()
                .zip(tool_use_ids.iter())
                .map(|(tc, tool_use_id)| AnthropicContentBlock::ToolUse {
                    id: tool_use_id.clone(),
                    name: tc.name.clone(),
                    input: tc.arguments.clone(),
                })
                .collect();
            messages.push(AnthropicMessage {
                role: "assistant".to_string(),
                content: assistant_blocks,
            });

            // Execute each tool and add tool_result messages (one per tool call)
            for (tc, tool_use_id) in tool_calls.iter().zip(tool_use_ids.iter()) {
                tracing::info!("Round {}: executing tool {}", round + 1, tc.name);
                let executed = match tc.name.parse::<ToolName>() {
                    Ok(tool) => self.cached_tool(tool, &tc.arguments).await,
                    Err(e) => Err(e),
                };
                let result_content = match executed {
                    Ok(result) => result,
                    Err(e) => {
                        tracing::warn!("Tool {} failed: {}", tc.name, e);
                        format!("error: {}", e)
                    }
                };
                messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: vec![AnthropicContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: result_content,
                    }],
                });
            }
        }

        // Max rounds reached — explicitly instruct the model to synthesize,
        // otherwise it may answer with another tool_use and empty content.
        messages.push(AnthropicMessage {
            role: "user".to_string(),
            content: vec![AnthropicContentBlock::Text(
                "Tool budget exhausted. Do not request any more tools; write your final analysis now using the data gathered so far.".to_string(),
            )],
        });
        let final_response = self.llm_quick.complete_messages(messages, tools).await?;
        Ok(final_response.content)
    }
}

/// Unwrap an analyst result, degrading a failure to an "unavailable" note so
/// the pipeline can continue on the remaining analysts' evidence.
fn report_or_unavailable(analyst: &str, result: Result<String>) -> String {
    match result {
        Ok(report) => report,
        Err(e) => {
            tracing::warn!("{} analyst failed: {}", analyst, e);
            format!("{analyst} analyst report unavailable (error: {e})")
        }
    }
}

/// Format a prefetched tool result for prompt inclusion, degrading to a
/// tool-usage hint if the fetch failed.
fn prefetched_block(label: &str, result: &Result<String>) -> String {
    match result {
        Ok(text) => format!("### {label}\n{text}"),
        Err(e) => {
            format!("### {label}\nNot available ({e}). Use the tools to fetch it if needed.")
        }
    }
}

/// 14-day news lookback window ending on `date`. Shared by the social and
/// news analysts so their prefetches hit the same tool-cache entry.
fn news_window(date: &str) -> (String, String) {
    let start = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| {
            (d - chrono::Duration::days(14))
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|_| date.to_string());
    (start, date.to_string())
}

fn format_memory_matches(memories: &[MemoryMatch]) -> String {
    if memories.is_empty() {
        return "None".to_string();
    }

    memories
        .iter()
        .map(|memory| {
            format!(
                "- Situation: {}; Recommendation: {}; Similarity: {:.2}",
                memory.matched_situation, memory.recommendation, memory.similarity_score
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// =============================================================================
// Helper Types
// =============================================================================

struct AnalystResults {
    market: String,
    social: String,
    news: String,
    fundamentals: String,
}

/// Extract rating and confidence from the final decision markdown.
/// Looks for patterns like `FINAL RATING: **BUY**` and `confidence: High`.
fn extract_rating_and_confidence(text: &str) -> (String, String) {
    use regex::Regex;
    use std::sync::LazyLock;

    static RATING_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)FINAL\s+RATING:\s*\*{0,2}(BUY|OVERWEIGHT|HOLD|UNDERWEIGHT|SELL)\*{0,2}")
            .unwrap()
    });
    static CONFIDENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)confidence(?:\s+level)?[:\s]+\*{0,2}(High|Medium|Low)\*{0,2}").unwrap()
    });

    let rating = RATING_RE
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_uppercase())
        .unwrap_or_else(|| "UNKNOWN".to_string());

    let confidence = CONFIDENCE_RE
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| {
            let s = m.as_str();
            let mut c = s.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => s.to_string(),
            }
        })
        .unwrap_or_else(|| "Unknown".to_string());

    (rating, confidence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LLMResponse;

    struct MockLLM;

    #[async_trait::async_trait]
    impl LLMClient for MockLLM {
        async fn complete(&self, _prompt: &str) -> Result<String> {
            Ok(String::new())
        }

        async fn complete_with_tools(&self, _prompt: &str, _tools: &[Tool]) -> Result<LLMResponse> {
            Ok(LLMResponse {
                content: String::new(),
                tool_calls: None,
                reasoning: None,
            })
        }

        async fn complete_messages(
            &self,
            _messages: Vec<AnthropicMessage>,
            _tools: &[Tool],
        ) -> Result<LLMResponse> {
            Ok(LLMResponse {
                content: String::new(),
                tool_calls: None,
                reasoning: None,
            })
        }

        fn validate_model(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &str {
            "mock"
        }
    }

    #[test]
    fn report_or_unavailable_degrades_failures_to_note() {
        let ok = report_or_unavailable("Market", Ok("strong uptrend".to_string()));
        assert_eq!(ok, "strong uptrend");

        let err = report_or_unavailable("Market", Err(anyhow::anyhow!("HTTP 500")));
        assert!(err.contains("Market analyst report unavailable"));
        assert!(err.contains("HTTP 500"));
    }

    #[tokio::test]
    async fn cached_tool_returns_cached_value_without_fetching() {
        let engine = GraphEngine::new(GraphConfig::default(), Arc::new(MockLLM), Arc::new(MockLLM));
        let args = serde_json::json!({"ticker": "AAPL"});
        let key = format!("{}:{}", ToolName::GetFinancials.as_str(), args);
        engine
            .tool_cache
            .write()
            .await
            .insert(key, "cached result".to_string());

        let out = engine
            .cached_tool(ToolName::GetFinancials, &args)
            .await
            .unwrap();
        assert_eq!(out, "cached result");
    }
}
