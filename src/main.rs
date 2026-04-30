//! TAgent - Fast Rust Trading Agent
//!
//! A high-performance Rust rewrite of TradingAgents with parallel execution.

mod data;
mod graph;
mod llm;
mod memory;
mod tools;

use crate::graph::engine::{GraphConfig, GraphEngine};
use crate::graph::state::AgentState;
use crate::llm::{AnyLLMClient, LLMClient};
use anyhow::{bail, Context, Result};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const FINANCIAL_DISCLAIMER: &str = "For research and education only. Not financial advice, investment advice, or a recommendation to buy, sell, or hold any security.";

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("tagent=info".parse()?),
        )
        .init();

    // Load .env file
    dotenvy::dotenv().ok();

    // Get LLM configuration from environment
    // TAGENT_PROVIDER: "minimax" (default), "zai", "openai", "anthropic"
    // Reads provider-specific keys: {MINIMAX,ZAI,OPENAI,ANTHROPIC}_{API_KEY,BASE_URL,MODEL}
    // Falls back to TAGENT_API_KEY / TAGENT_BASE_URL / TAGENT_MODEL for ad-hoc overrides
    let provider = std::env::var("TAGENT_PROVIDER")
        .unwrap_or_else(|_| "minimax".to_string())
        .to_lowercase();

    let (api_key, base_url, default_model, api_key_env) = match provider.as_str() {
        "minimax" => (
            std::env::var("MINIMAX_API_KEY").unwrap_or_default(),
            std::env::var("MINIMAX_BASE_URL")
                .unwrap_or_else(|_| "https://api.minimaxi.com/anthropic".to_string()),
            "MiniMax-M2.7".to_string(),
            "MINIMAX_API_KEY",
        ),
        "zai" => (
            std::env::var("ZAI_API_KEY").unwrap_or_default(),
            std::env::var("ZAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.z.ai/api/anthropic".to_string()),
            "GLM-5.1".to_string(),
            "ZAI_API_KEY",
        ),
        "openai" => (
            std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string()),
            "gpt-4o".to_string(),
            "OPENAI_API_KEY",
        ),
        "anthropic" => (
            std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
            std::env::var("ANTHROPIC_BASE_URL")
                .unwrap_or_else(|_| "https://api.anthropic.com".to_string()),
            "claude-sonnet-4-6".to_string(),
            "ANTHROPIC_API_KEY",
        ),
        other => bail!(
            "Unsupported TAGENT_PROVIDER '{}'. Expected minimax, zai, openai, or anthropic.",
            other
        ),
    };

    // Ad-hoc overrides (highest priority)
    let api_key = std::env::var("TAGENT_API_KEY").unwrap_or(api_key);
    let base_url = std::env::var("TAGENT_BASE_URL").unwrap_or(base_url);
    let default_model = std::env::var("TAGENT_MODEL").unwrap_or(default_model);

    let quick_model = std::env::var("TAGENT_QUICK_MODEL").unwrap_or_else(|_| default_model.clone());
    let deep_model = std::env::var("TAGENT_DEEP_MODEL").unwrap_or(default_model);

    if api_key.trim().is_empty() {
        bail!(
            "Missing API key for provider '{}'. Set TAGENT_API_KEY or {}.",
            provider,
            api_key_env
        );
    }
    if base_url.trim().is_empty() {
        bail!("Missing base URL for provider '{}'. Set TAGENT_BASE_URL or the provider-specific base URL.", provider);
    }
    if quick_model.trim().is_empty() || deep_model.trim().is_empty() {
        bail!("Missing LLM model name. Set TAGENT_MODEL or both TAGENT_QUICK_MODEL and TAGENT_DEEP_MODEL.");
    }

    tracing::info!(
        "Initializing TAgent with provider={}, quick={}, deep={}",
        provider,
        quick_model,
        deep_model
    );

    let llm_quick = Arc::new(AnyLLMClient::new(
        &provider,
        &quick_model,
        &api_key,
        &base_url,
    ));

    let llm_deep = Arc::new(AnyLLMClient::new(
        &provider,
        &deep_model,
        &api_key,
        &base_url,
    ));

    // Validate models
    if !llm_quick.validate_model() {
        anyhow::bail!("Invalid quick LLM configuration");
    }
    if !llm_deep.validate_model() {
        anyhow::bail!("Invalid deep LLM configuration");
    }

    // Create graph engine
    let config = GraphConfig {
        company: String::new(),
        trade_date: String::new(),
        max_debate_rounds: 1,
        max_risk_discuss_rounds: 1,
        max_recur_limit: 100,
        output_language: "English".to_string(),
    };

    let engine = Arc::new(GraphEngine::new(config, llm_quick, llm_deep));

    // Parse command line args: tagent [TICKER...] [DATE]
    // Last arg matching YYYY-MM-DD is treated as date; rest are tickers.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let date_re = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap();
    let (tickers, trade_date) = if args.is_empty() {
        (vec!["NVDA".to_string()], "2026-04-25".to_string())
    } else if args.len() == 1 {
        if date_re.is_match(&args[0]) {
            (vec!["NVDA".to_string()], args[0].clone())
        } else {
            (
                vec![args[0].clone()],
                chrono::Local::now().format("%Y-%m-%d").to_string(),
            )
        }
    } else {
        let last = args.last().unwrap();
        if date_re.is_match(last) {
            (args[..args.len() - 1].to_vec(), last.clone())
        } else {
            (
                args.clone(),
                chrono::Local::now().format("%Y-%m-%d").to_string(),
            )
        }
    };

    tracing::info!("Tickers: {:?}, Date: {}", tickers, trade_date);

    // Validate all tickers in parallel
    {
        use crate::data::yfinance::YahooFinanceClient;
        let yf = YahooFinanceClient::new();
        let mut validates = Vec::new();
        for t in &tickers {
            let yf = yf.clone();
            let t = t.clone();
            validates.push(tokio::spawn(async move {
                // Use recent 5-day window for validation
                let today = chrono::Utc::now().date_naive();
                let start = (today - chrono::Duration::days(5))
                    .format("%Y-%m-%d")
                    .to_string();
                let end = today.format("%Y-%m-%d").to_string();
                let prices = yf.get_stock_data(&t, &start, &end).await;
                match prices {
                    Ok(p) if p.is_empty() => {
                        Err(anyhow::anyhow!("Ticker '{}' returned no data", t))
                    }
                    Err(e) => Err(anyhow::anyhow!("Cannot fetch data for '{}': {}", t, e)),
                    _ => Ok(t),
                }
            }));
        }
        for v in validates {
            v.await??;
        }
        tracing::info!("All tickers validated");
    }

    // Run analysis — single ticker directly, multiple in parallel
    let reports_dir = std::path::Path::new("reports");
    std::fs::create_dir_all(reports_dir)?;

    if tickers.len() == 1 {
        let ticker = &tickers[0];
        let state = AgentState::new(ticker, &trade_date);
        let start = std::time::Instant::now();
        let result = engine.run(state).await?;
        let elapsed = start.elapsed();
        print_and_save(ticker, &trade_date, &result.final_trade_decision, elapsed)?;
    } else {
        let total_start = std::time::Instant::now();
        let batch_concurrency = batch_concurrency()?;
        tracing::info!("Batch concurrency: {}", batch_concurrency);
        let limiter = Arc::new(Semaphore::new(batch_concurrency));
        let mut handles = Vec::new();
        for ticker in &tickers {
            let engine = engine.clone();
            let limiter = limiter.clone();
            let ticker = ticker.clone();
            let trade_date = trade_date.clone();
            handles.push(tokio::spawn(async move {
                let _permit = limiter
                    .acquire_owned()
                    .await
                    .context("Batch concurrency limiter closed")?;
                let state = AgentState::new(&ticker, &trade_date);
                let start = std::time::Instant::now();
                let result = engine.run(state).await?;
                let elapsed = start.elapsed();
                Ok::<_, anyhow::Error>((ticker, trade_date, result.final_trade_decision, elapsed))
            }));
        }
        let mut any_failed = false;
        for h in handles {
            match h.await? {
                Ok((ticker, date, decision, elapsed)) => {
                    print_and_save(&ticker, &date, &decision, elapsed)?;
                }
                Err(e) => {
                    tracing::error!("Analysis failed: {}", e);
                    any_failed = true;
                }
            }
        }
        println!(
            "\n[Batch total: {:.0}s]",
            total_start.elapsed().as_secs_f64()
        );
        if any_failed {
            anyhow::bail!("One or more analyses failed");
        }
    }

    Ok(())
}

fn batch_concurrency() -> Result<usize> {
    match std::env::var("TAGENT_BATCH_CONCURRENCY") {
        Ok(value) => {
            let parsed = value
                .parse::<usize>()
                .with_context(|| format!("Invalid TAGENT_BATCH_CONCURRENCY '{}'", value))?;
            if parsed == 0 {
                bail!("TAGENT_BATCH_CONCURRENCY must be at least 1");
            }
            Ok(parsed)
        }
        Err(std::env::VarError::NotPresent) => Ok(1),
        Err(e) => Err(e).context("Invalid TAGENT_BATCH_CONCURRENCY"),
    }
}

fn print_and_save(
    ticker: &str,
    trade_date: &str,
    decision: &str,
    elapsed: std::time::Duration,
) -> Result<()> {
    println!("\n{}", "=".repeat(60));
    println!("FINAL DECISION FOR {} ON {}", ticker, trade_date);
    println!("{}", "=".repeat(60));
    println!("\nDisclaimer: {}", FINANCIAL_DISCLAIMER);
    println!("\n{}", decision);
    println!("\n[Completed in {:.0}s]", elapsed.as_secs_f64());

    let reports_dir = std::path::Path::new("reports");
    std::fs::create_dir_all(reports_dir)?;
    let filename = format!("{}_{}.md", ticker, trade_date);
    let report_path = reports_dir.join(&filename);
    let report = format!(
        "# {} Analysis — {}\n\n> {}\n\n**Completed in {:.0}s**\n\n{}\n",
        ticker,
        trade_date,
        FINANCIAL_DISCLAIMER,
        elapsed.as_secs_f64(),
        decision
    );
    std::fs::write(&report_path, &report)?;
    tracing::info!("Report saved to {}", report_path.display());
    Ok(())
}
