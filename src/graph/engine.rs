//! Graph execution engine
//!
//! This is a simple state machine that replaces LangGraph's StateGraph.
//! It routes between nodes based on conditional logic and maintains state.

use crate::graph::state::{AgentState, InvestDebateState, RiskDebateState};
use crate::llm::{LLMClient, AnyLLMClient, Tool};
use crate::memory::BM25Memory;
use crate::tools::{ToolRegistry, ToolCall};
use crate::data::yfinance;
use anyhow::{Result, Context};
use std::sync::Arc;
use tokio::sync::RwLock;


// =============================================================================
// Graph Configuration
// =============================================================================

#[derive(Debug, Clone)]
pub struct GraphConfig {
    pub max_debate_rounds: i32,
    pub max_risk_discuss_rounds: i32,
    pub max_recur_limit: usize,
    pub output_language: String,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
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
    bull_memory: RwLock<BM25Memory>,
    bear_memory: RwLock<BM25Memory>,
    trader_memory: RwLock<BM25Memory>,
}

impl GraphEngine {
    pub fn new(
        config: GraphConfig,
        llm_quick: Arc<dyn LLMClient>,
        llm_deep: Arc<dyn LLMClient>,
    ) -> Self {
        Self {
            config,
            llm_quick,
            llm_deep,
            tool_registry: ToolRegistry::new(),
            bull_memory: RwLock::new(BM25Memory::new("bull")),
            bear_memory: RwLock::new(BM25Memory::new("bear")),
            trader_memory: RwLock::new(BM25Memory::new("trader")),
        }
    }

    /// Run the full analysis for a ticker
    pub async fn run(&self, mut state: AgentState) -> Result<AgentState> {
        tracing::info!("Starting analysis for {}", state.company_of_interest);

        // ========== PHASE 1: Parallel Analysts ==========
        tracing::info!("Phase 1: Running analysts in parallel...");
        let analyst_results = self.run_analysts(&state).await?;
        state.market_report = analyst_results.market.clone();
        state.sentiment_report = analyst_results.social.clone();
        state.news_report = analyst_results.news.clone();
        state.fundamentals_report = analyst_results.fundamentals.clone();

        // ========== PHASE 2: Bull/Bear Debate ==========
        tracing::info!("Phase 2: Running bull/bear debate...");
        self.run_bull_bear_debate(&mut state).await?;

        // ========== PHASE 3: Research Manager ==========
        tracing::info!("Phase 3: Research manager synthesis...");
        let investment_plan = self.run_research_manager(&state).await?;
        state.investment_plan = investment_plan;

        // ========== PHASE 4: Trader ==========
        tracing::info!("Phase 4: Trader decision...");
        let trader_plan = self.run_trader(&state).await?;
        state.trader_investment_plan = trader_plan;

        // ========== PHASE 5: Risk Debate ==========
        tracing::info!("Phase 5: Risk debate...");
        self.run_risk_debate(&mut state).await?;

        // ========== PHASE 6: Portfolio Manager ==========
        tracing::info!("Phase 6: Portfolio manager synthesis...");
        let decision = self.run_portfolio_manager(&state).await?;
        state.final_trade_decision = decision;

        tracing::info!("Analysis complete for {}", state.company_of_interest);
        Ok(state)
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

        Ok(AnalystResults {
            market: market?,
            social: social?,
            news: news?,
            fundamentals: fundamentals?,
        })
    }

    async fn run_market_analyst(&self, ticker: &str, date: &str) -> Result<String> {
        let prompt = format!(
            r#"You are a market analyst. Analyze the stock data for {ticker} on {date}.

            Use the get_stock_data tool to get recent OHLCV data, and get_indicators tool to get RSI, MACD, Bollinger Bands.

            Provide a concise market analysis covering:
            - Current price trend
            - Volume analysis
            - Key technical indicators (RSI, MACD, Bollinger position)
            - Support/resistance levels if apparent

            End with: FINAL MARKET ANALYSIS: **SUMMARY**"#
        );

        let tools = vec![
            self.tool_registry.get("get_stock_data").cloned().unwrap(),
            self.tool_registry.get("get_indicators").cloned().unwrap(),
        ];

        self.execute_llm_with_tools(&prompt, &tools).await
    }

    async fn run_social_analyst(&self, ticker: &str, date: &str) -> Result<String> {
        let prompt = format!(
            r#"You are a social media sentiment analyst. Analyze sentiment for {ticker} on {date}.

            Use the get_news tool to get recent news and social sentiment around the stock.

            Provide a concise sentiment analysis covering:
            - Overall investor sentiment (bullish/bearish/neutral)
            - Key themes in recent coverage
            - Notable positive or negative catalysts
            - Social media trends if apparent

            End with: FINAL SENTIMENT ANALYSIS: **POSITIVE/NEGATIVE/NEUTRAL**"#
        );

        let tools = vec![self.tool_registry.get("get_news").cloned().unwrap()];

        self.execute_llm_with_tools(&prompt, &tools).await
    }

    async fn run_news_analyst(&self, ticker: &str, date: &str) -> Result<String> {
        let prompt = format!(
            r#"You are a news analyst. Analyze news impact for {ticker} on {date}.

            Use the get_news tool to get recent news coverage, and get_global_news for broader market context.

            Provide a concise news analysis covering:
            - Key news items affecting the stock
            - Impact on near-term outlook
            - Risks or opportunities from recent developments

            End with: FINAL NEWS ANALYSIS: **IMPACT SUMMARY**"#
        );

        let tools = vec![
            self.tool_registry.get("get_news").cloned().unwrap(),
            self.tool_registry.get("get_global_news").cloned().unwrap(),
        ];

        self.execute_llm_with_tools(&prompt, &tools).await
    }

    async fn run_fundamentals_analyst(&self, ticker: &str, date: &str) -> Result<String> {
        let prompt = format!(
            r#"You are a fundamentals analyst. Analyze fundamental data for {ticker} on {date}.

            Use the get_fundamentals tool to get company overview, and relevant financial statement tools if available.

            Provide a concise fundamentals analysis covering:
            - Business model and competitive position
            - Key financial metrics (P/E, EPS, revenue growth)
            - Valuation assessment
            - Key risks and strengths

            End with: FINAL FUNDAMENTALS ANALYSIS: **STRONG/WEAK/FAIR**"#
        );

        let tools = vec![self.tool_registry.get("get_fundamentals").cloned().unwrap()];

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
            "Past lessons: Bull memories: {:?}, Bear memories: {:?}",
            bull_memories, bear_memories
        );

        let mut debate_state = InvestDebateState::default();
        let max_rounds = self.config.max_debate_rounds;

        // Run debate rounds
        for round in 0..max_rounds {
            tracing::info!("Bull/Bear debate round {}", round + 1);

            // Bull makes argument
            let bull_prompt = format!(
                r#"You are a bullish researcher arguing FOR investing in {company}.

                Context from analysts:
                Market: {market}
                Sentiment: {sentiment}
                News: {news}
                Fundamentals: {fundamentals}

                {memory_context}

                Provide your bullish argument focusing on:
                - Growth catalysts
                - Competitive advantages
                - Positive indicators

                Format: Start with "Bull:" and make your case.
                "#,
                company = state.company_of_interest,
                market = state.market_report,
                sentiment = state.sentiment_report,
                news = state.news_report,
                fundamentals = state.fundamentals_report,
            );

            let bull_response = self.llm_quick.complete(&bull_prompt).await?;
            debate_state.bull_history.push_str(&format!("\nBull: {}", bull_response));
            debate_state.current_response = bull_response.clone();

            // Bear makes argument
            let bear_prompt = format!(
                r#"You are a bearish researcher arguing AGAINST investing in {company}.

                Context from analysts:
                Market: {market}
                Sentiment: {sentiment}
                News: {news}
                Fundamentals: {fundamentals}

                {memory_context}

                Bull's argument: {bull_response}

                Provide your bearish counter-argument focusing on:
                - Risks and challenges
                - Negative indicators
                - Why the bull case is flawed

                Format: Start with "Bear:" and make your case.
                "#,
                company = state.company_of_interest,
                market = state.market_report,
                sentiment = state.sentiment_report,
                news = state.news_report,
                fundamentals = state.fundamentals_report,
                bull_response = bull_response,
            );

            let bear_response = self.llm_quick.complete(&bear_prompt).await?;
            debate_state.bear_history.push_str(&format!("\nBear: {}", bear_response));
            debate_state.history.push_str(&format!("Round {} Bull: {} Bear: {}", round + 1, bull_response, bear_response));
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

            Provide a comprehensive investment plan that:
            - Weighs the bull and bear arguments
            - Provides a clear BUY/HOLD/SELL recommendation
            - Outlines specific strategic actions
            - Justifies the reasoning

            End with: INVESTMENT DECISION: **BUY/HOLD/SELL** with confidence level
            "#,
            company = state.company_of_interest,
            history = state.investment_debate_state.history,
            market = state.market_report,
            sentiment = state.sentiment_report,
            news = state.news_report,
            fundamentals = state.fundamentals_report,
        );

        self.llm_deep.complete(&prompt).await
    }

    // -------------------------------------------------------------------------
    // PHASE 4: Trader
    // -------------------------------------------------------------------------

    async fn run_trader(&self, state: &AgentState) -> Result<String> {
        let situation = state.situation_summary();
        let memories = self.trader_memory.read().await.get_memories(&situation, 2);

        let memory_context = format!("Past trading lessons: {:?}", memories);

        let prompt = format!(
            r#"You are a trader converting the investment plan into a specific trading decision for {company}.

            Investment plan: {investment_plan}

            Analyst reports:
            Market: {market}
            Sentiment: {sentiment}
            News: {news}
            Fundamentals: {fundamentals}

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
        );

        self.llm_quick.complete(&prompt).await
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
                self.run_aggressive_risk(&state),
                self.run_conservative_risk(&state),
                self.run_neutral_risk(&state),
            );

            // Extract string values before ? moves them
            let agg_response = agg_result?;
            let cons_response = cons_result?;
            let neut_response = neut_result?;

            risk_state.aggressive_history.push_str(&format!("\nAggressive: {}", agg_response));
            risk_state.conservative_history.push_str(&format!("\nConservative: {}", cons_response));
            risk_state.neutral_history.push_str(&format!("\nNeutral: {}", neut_response));
            risk_state.history.push_str(&format!(
                "\nRound {}:\nAggressive: {}\nConservative: {}\nNeutral: {}",
                round + 1, agg_response, cons_response, neut_response
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
            Fundamentals: {fundamentals}

            Provide your aggressive risk assessment:
            - Maximizing upside scenarios
            - Why risk is worth taking
            - Position sizing for maximum gains

            Format: Start with "Aggressive:" and state your position.
            "#,
            trader_plan = state.trader_investment_plan,
            market = state.market_report,
            fundamentals = state.fundamentals_report,
        );

        self.llm_quick.complete(&prompt).await
    }

    async fn run_conservative_risk(&self, state: &AgentState) -> Result<String> {
        let prompt = format!(
            r#"You are a conservative risk analyst focused on protecting capital.

            Trader's plan: {trader_plan}

            Analyst reports:
            Market: {market}
            Fundamentals: {fundamentals}

            Provide your conservative risk assessment:
            - Downside protection
            - Volatility concerns
            - Risk mitigation strategies

            Format: Start with "Conservative:" and state your position.
            "#,
            trader_plan = state.trader_investment_plan,
            market = state.market_report,
            fundamentals = state.fundamentals_report,
        );

        self.llm_quick.complete(&prompt).await
    }

    async fn run_neutral_risk(&self, state: &AgentState) -> Result<String> {
        let prompt = format!(
            r#"You are a neutral risk analyst balancing risk and reward.

            Trader's plan: {trader_plan}

            Aggressive view: {agg}
            Conservative view: {cons}

            Provide your balanced risk assessment:
            - Weighing both sides
            - Moderate position sizing
            - Key risk metrics to monitor

            Format: Start with "Neutral:" and state your position.
            "#,
            trader_plan = state.trader_investment_plan,
            agg = state.risk_debate_state.current_aggressive_response,
            cons = state.risk_debate_state.current_conservative_response,
        );

        self.llm_quick.complete(&prompt).await
    }

    // -------------------------------------------------------------------------
    // PHASE 6: Portfolio Manager
    // -------------------------------------------------------------------------

    async fn run_portfolio_manager(&self, state: &AgentState) -> Result<String> {
        let prompt = format!(
            r#"You are a portfolio manager making the final trading decision for {company}.

            Risk debate summary:
            {risk_debate}

            Trader's recommendation: {trader_plan}

            Investment thesis: {investment_plan}

            Provide your final decision with:
            - Rating (BUY/HOLD/SELL/OVERWEIGHT/UNDERWEIGHT)
            - Executive summary
            - Investment thesis
            - Key risks and monitoring points

            End with: FINAL RATING: **{{RATING}}** with 1-sentence justification
            "#,
            company = state.company_of_interest,
            risk_debate = state.risk_debate_state.history,
            trader_plan = state.trader_investment_plan,
            investment_plan = state.investment_plan,
        );

        self.llm_deep.complete(&prompt).await
    }

    // -------------------------------------------------------------------------
    // Helper: Execute LLM with tool calling
    // -------------------------------------------------------------------------

    async fn execute_llm_with_tools(&self, prompt: &str, tools: &[Tool]) -> Result<String> {
        let response = self.llm_quick.complete_with_tools(prompt, tools).await?;

        // Handle tool calls
        if let Some(tool_calls) = response.tool_calls {
            let mut tool_results = Vec::new();

            for tc in tool_calls {
                tracing::info!("Executing tool: {}", tc.name);
                match yfinance::execute_tool(&tc.name, &tc.arguments).await {
                    Ok(result) => tool_results.push(format!("Tool {} result: {}", tc.name, result)),
                    Err(e) => tool_results.push(format!("Tool {} error: {}", tc.name, e)),
                }
            }

            // Continue with tool results
            let follow_up = format!(
                "{}\n\nHere are the tool results:\n{}",
                prompt,
                tool_results.join("\n")
            );

            // Final response after tool execution
            let final_response = self.llm_quick.complete(&follow_up).await?;
            Ok(final_response)
        } else {
            Ok(response.content)
        }
    }
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