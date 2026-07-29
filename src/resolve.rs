//! Decision feedback loop: score pending decisions against realized market
//! outcomes and feed the lessons back into the agent memories.
//!
//! A pending decision becomes resolvable once it is at least `horizon_days`
//! old. Resolution computes the realized return (first close at/after the
//! decision date to the latest close), alpha vs the regional benchmark, and a
//! reflection (LLM-written when a client is available, mechanical otherwise).

use crate::data::yfinance::{benchmark_for_ticker, Quote, YahooFinanceClient};
use crate::llm::LLMClient;
use crate::memory::{BM25Memory, DecisionEntry, DecisionLog};
use anyhow::{Context, Result};
use chrono::NaiveDate;

/// Outcome of resolving one pending decision.
pub struct Resolution {
    pub ticker: String,
    pub date: String,
    pub rating: String,
    pub realized_return: f64,
    pub alpha: f64,
    pub reflection: String,
    /// Situation text used as the retrieval key when recording lessons.
    pub situation: String,
}

/// Resolve every pending decision that is at least `horizon_days` old.
/// Unresolvable entries (no price data, delisted ticker) are warned about and
/// left pending. The caller is responsible for saving the log.
pub async fn resolve_pending(
    log: &mut DecisionLog,
    yf: &YahooFinanceClient,
    llm: Option<&dyn LLMClient>,
    horizon_days: i64,
) -> Result<Vec<Resolution>> {
    let today = chrono::Utc::now().date_naive();
    let candidates: Vec<DecisionEntry> = log
        .pending()
        .into_iter()
        .filter(|e| is_resolvable(&e.date, today, horizon_days))
        .cloned()
        .collect();

    let mut resolutions = Vec::new();
    for entry in candidates {
        match resolve_one(yf, llm, &entry, today).await {
            Ok(res) => {
                log.resolve(
                    &res.ticker,
                    &res.date,
                    res.realized_return,
                    res.alpha,
                    &res.reflection,
                );
                resolutions.push(res);
            }
            Err(e) => {
                tracing::warn!("Could not resolve {} {}: {}", entry.ticker, entry.date, e);
            }
        }
    }
    Ok(resolutions)
}

/// Record resolved lessons into the bull/bear/trader memories on disk so
/// future debates retrieve real outcomes, not just past ratings.
pub fn record_lessons(resolutions: &[Resolution]) {
    if resolutions.is_empty() {
        return;
    }
    for name in ["bull", "bear", "trader"] {
        let path = BM25Memory::default_path(name);
        let mut mem = BM25Memory::from_file(name, &path);
        for r in resolutions {
            let lesson = format!(
                "{} resolved: return {:+.1}%, alpha {:+.1}%. {}",
                r.rating, r.realized_return, r.alpha, r.reflection
            );
            mem.add(&r.situation, &lesson);
        }
        if let Err(e) = mem.save(&path) {
            tracing::warn!("Failed to persist {} memory: {}", name, e);
        }
    }
}

async fn resolve_one(
    yf: &YahooFinanceClient,
    llm: Option<&dyn LLMClient>,
    entry: &DecisionEntry,
    today: NaiveDate,
) -> Result<Resolution> {
    let end = (today + chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let quotes = yf.get_quotes(&entry.ticker, &entry.date, &end).await?;
    let realized_return =
        window_return(&quotes).context("insufficient price data in resolution window")?;

    let benchmark = benchmark_for_ticker(&entry.ticker);
    let benchmark_return = match yf.get_quotes(&benchmark, &entry.date, &end).await {
        Ok(q) => window_return(&q).unwrap_or(0.0),
        Err(e) => {
            tracing::warn!("Benchmark {} unavailable, using 0%: {}", benchmark, e);
            0.0
        }
    };
    let alpha = realized_return - benchmark_return;

    let days_elapsed = NaiveDate::parse_from_str(&entry.date, "%Y-%m-%d")
        .map(|d| (today - d).num_days())
        .unwrap_or(0);

    let reflection = match llm {
        Some(client) => {
            match llm_reflection(
                client,
                entry,
                realized_return,
                &benchmark,
                benchmark_return,
                alpha,
                days_elapsed,
            )
            .await
            {
                Ok(text) if !text.trim().is_empty() => text.trim().to_string(),
                Ok(_) | Err(_) => mechanical_reflection(&entry.rating, realized_return, alpha),
            }
        }
        None => mechanical_reflection(&entry.rating, realized_return, alpha),
    };

    Ok(Resolution {
        ticker: entry.ticker.clone(),
        date: entry.date.clone(),
        rating: entry.rating.clone(),
        realized_return,
        alpha,
        reflection,
        situation: format!(
            "{} {} {}: {}",
            entry.ticker, entry.date, entry.rating, entry.summary
        ),
    })
}

async fn llm_reflection(
    llm: &dyn LLMClient,
    entry: &DecisionEntry,
    realized_return: f64,
    benchmark: &str,
    benchmark_return: f64,
    alpha: f64,
    days_elapsed: i64,
) -> Result<String> {
    let prompt = format!(
        r#"You are reviewing a past trading decision to extract a lesson.

Decision: {rating} on {ticker} dated {date} (confidence: {confidence}).
Original reasoning summary: {summary}

Outcome after {days} days: return {ret:+.1}%, benchmark ({bench}) {bret:+.1}%, alpha {alpha:+.1}%.

Write a 2-3 sentence reflection: was the call right, what evidence was likely over- or under-weighted, and one lesson for similar future situations. Plain text only, no headings."#,
        rating = entry.rating,
        ticker = entry.ticker,
        date = entry.date,
        confidence = entry.confidence,
        summary = entry.summary,
        days = days_elapsed,
        ret = realized_return,
        bench = benchmark,
        bret = benchmark_return,
        alpha = alpha,
    );
    llm.complete(&prompt).await
}

/// True when the decision date is parseable and at least `horizon_days` old.
fn is_resolvable(entry_date: &str, today: NaiveDate, horizon_days: i64) -> bool {
    NaiveDate::parse_from_str(entry_date, "%Y-%m-%d")
        .map(|d| (today - d).num_days() >= horizon_days)
        .unwrap_or(false)
}

/// Percent change from the first to the last close in the window.
/// Uses the dividend-adjusted close when available so total return
/// (and thus alpha) includes dividends paid during the window.
fn window_return(quotes: &[Quote]) -> Option<f64> {
    let first_q = quotes.first()?;
    let last_q = quotes.last()?;
    let first = first_q.adjclose.unwrap_or(first_q.close);
    let last = last_q.adjclose.unwrap_or(last_q.close);
    if first == 0.0 {
        return None;
    }
    Some((last - first) / first * 100.0)
}

fn mechanical_reflection(rating: &str, realized_return: f64, alpha: f64) -> String {
    let verdict = match rating {
        "BUY" | "OVERWEIGHT" => {
            if alpha > 0.0 {
                "the bullish call outperformed the benchmark"
            } else {
                "the bullish call underperformed the benchmark"
            }
        }
        "SELL" | "UNDERWEIGHT" => {
            if realized_return < 0.0 {
                "caution was warranted"
            } else {
                "the stock rose despite the negative call"
            }
        }
        _ => "neutral call; compare against alpha for opportunity cost",
    };
    format!("{rating}: return {realized_return:+.1}%, alpha {alpha:+.1}% — {verdict}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(close: f64) -> Quote {
        Quote {
            symbol: "TEST".to_string(),
            open: close,
            high: close,
            low: close,
            close,
            adjclose: None,
            volume: 0,
            timestamp: 0,
        }
    }

    #[test]
    fn window_return_computes_percent_change() {
        let quotes = vec![quote(100.0), quote(105.0), quote(110.0)];
        assert!((window_return(&quotes).unwrap() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn window_return_handles_degenerate_input() {
        assert!(window_return(&[]).is_none());
        assert!(window_return(&[quote(0.0), quote(5.0)]).is_none());
        // Single quote: zero return, not an error
        assert_eq!(window_return(&[quote(50.0)]), Some(0.0));
    }

    #[test]
    fn is_resolvable_respects_horizon() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
        assert!(is_resolvable("2026-06-01", today, 14));
        assert!(!is_resolvable("2026-06-02", today, 14));
        assert!(!is_resolvable("not-a-date", today, 14));
    }

    #[test]
    fn mechanical_reflection_judges_rating_against_outcome() {
        let r = mechanical_reflection("BUY", 12.5, 8.3);
        assert!(r.contains("+12.5%"));
        assert!(r.contains("outperformed"));

        let r = mechanical_reflection("SELL", -5.0, -7.0);
        assert!(r.contains("caution was warranted"));

        let r = mechanical_reflection("HOLD", 1.0, -1.0);
        assert!(r.contains("neutral"));
    }
}
