//! Agent state structures - mirrors Python TradingAgents state

use serde::{Deserialize, Serialize};
/// Investment debate state (bull/bear researchers)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InvestDebateState {
    pub bull_history: String,
    pub bear_history: String,
    pub history: String,
    pub current_response: String,
    pub judge_decision: String,
    pub count: i32,
}

/// Risk debate state (aggressive/conservative/neutral)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RiskDebateState {
    pub aggressive_history: String,
    pub conservative_history: String,
    pub neutral_history: String,
    pub history: String,
    pub latest_speaker: String,
    pub current_aggressive_response: String,
    pub current_conservative_response: String,
    pub current_neutral_response: String,
    pub judge_decision: String,
    pub count: i32,
}

/// Main agent state - the state machine flows through this
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub company_of_interest: String,
    pub trade_date: String,
    pub sender: String,

    // Analyst reports
    pub market_report: String,
    pub sentiment_report: String,
    pub news_report: String,
    pub fundamentals_report: String,

    // Investment debate
    pub investment_debate_state: InvestDebateState,
    pub investment_plan: String,

    // Trader
    pub trader_investment_plan: String,

    // Risk debate
    pub risk_debate_state: RiskDebateState,
    pub final_trade_decision: String,

    // Message history (simplified - just strings for now)
    #[serde(default)]
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            company_of_interest: String::new(),
            trade_date: String::new(),
            sender: String::new(),
            market_report: String::new(),
            sentiment_report: String::new(),
            news_report: String::new(),
            fundamentals_report: String::new(),
            investment_debate_state: InvestDebateState::default(),
            investment_plan: String::new(),
            trader_investment_plan: String::new(),
            risk_debate_state: RiskDebateState::default(),
            final_trade_decision: String::new(),
            messages: Vec::new(),
        }
    }
}

impl AgentState {
    pub fn new(company: &str, trade_date: &str) -> Self {
        Self {
            company_of_interest: company.to_string(),
            trade_date: trade_date.to_string(),
            sender: String::new(),
            ..Default::default()
        }
    }

    /// Build a situation description for memory retrieval
    pub fn situation_summary(&self) -> String {
        format!(
            "Company: {} | Date: {} | Market: {} | Sentiment: {} | News: {} | Fundamentals: {}",
            self.company_of_interest,
            self.trade_date,
            self.market_report.chars().take(200).collect::<String>(),
            self.sentiment_report.chars().take(200).collect::<String>(),
            self.news_report.chars().take(200).collect::<String>(),
            self.fundamentals_report.chars().take(200).collect::<String>(),
        )
    }
}

/// State update returned by node execution
#[derive(Debug)]
#[allow(dead_code)]
pub struct StateUpdate {
    pub field: String,
    pub value: String,
}

#[allow(dead_code)]
impl StateUpdate {
    pub fn set(field: &str, value: &str) -> Self {
        Self {
            field: field.to_string(),
            value: value.to_string(),
        }
    }

    pub fn append(field: &str, value: &str) -> Self {
        Self {
            field: field.to_string(),
            value: value.to_string(),
        }
    }
}