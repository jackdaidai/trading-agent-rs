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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
            self.fundamentals_report
                .chars()
                .take(200)
                .collect::<String>(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_state_new() {
        let s = AgentState::new("AAPL", "2025-06-01");
        assert_eq!(s.company_of_interest, "AAPL");
        assert_eq!(s.trade_date, "2025-06-01");
        assert!(s.market_report.is_empty());
        assert!(s.messages.is_empty());
    }

    #[test]
    fn test_agent_state_default() {
        let s = AgentState::default();
        assert!(s.company_of_interest.is_empty());
        assert_eq!(s.investment_debate_state.count, 0);
        assert_eq!(s.risk_debate_state.count, 0);
    }

    #[test]
    fn test_situation_summary_truncates() {
        let mut s = AgentState::new("TSLA", "2025-01-01");
        s.market_report = "x".repeat(500);
        let summary = s.situation_summary();
        // 200 chars max per field
        assert!(summary.len() < 500 * 4);
        assert!(summary.contains("TSLA"));
    }

    #[test]
    fn test_debate_states_clone() {
        let mut s = AgentState::new("NVDA", "2025-01-01");
        s.investment_debate_state.count = 3;
        s.risk_debate_state.count = 5;
        let cloned = s.clone();
        assert_eq!(cloned.investment_debate_state.count, 3);
        assert_eq!(cloned.risk_debate_state.count, 5);
    }
}
