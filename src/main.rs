//! TAgent - Fast Rust Trading Agent
//!
//! A high-performance Rust rewrite of TradingAgents with parallel execution.

mod llm;
mod graph;
mod memory;
mod tools;
mod data;

use crate::graph::engine::{GraphEngine, GraphConfig};
use crate::graph::state::AgentState;
use crate::llm::{AnyLLMClient, LLMClient};
use anyhow::Result;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("tagent=info".parse()?))
        .init();

    // Load .env file
    dotenvy::dotenv().ok();

    // Get API credentials from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .unwrap_or_else(|_| "sk-...".to_string());
    let base_url = std::env::var("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|_| "https://api.minimaxi.com/anthropic".to_string());
    let quick_model = std::env::var("TAGENT_QUICK_MODEL")
        .unwrap_or_else(|_| "claude-sonnet-4-6".to_string());
    let deep_model = std::env::var("TAGENT_DEEP_MODEL")
        .unwrap_or_else(|_| "claude-sonnet-4-6".to_string());

    tracing::info!("Initializing TAgent...");

    // Create LLM clients
    // Using MiniMax's Anthropic-compatible endpoint (same as Python version)
    let llm_quick = Arc::new(AnyLLMClient::new(
        "anthropic",
        &quick_model,
        &api_key,
        &base_url,
    ));

    let llm_deep = Arc::new(AnyLLMClient::new(
        "anthropic",
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

    // Parse command line args
    let args: Vec<String> = std::env::args().collect();
    let ticker = args.get(1).cloned().unwrap_or_else(|| "NVDA".to_string());
    let trade_date = args.get(2).cloned().unwrap_or_else(|| "2026-04-25".to_string());

    tracing::info!("Running analysis for {} on {}", ticker, trade_date);

    // Create initial state
    let state = AgentState::new(&ticker, &trade_date);

    // Run the graph
    let start = std::time::Instant::now();
    let result = engine.run(state).await?;
    let elapsed = start.elapsed();

    // Print results
    println!("\n{}", "=".repeat(60));
    println!("FINAL DECISION FOR {} ON {}", ticker, trade_date);
    println!("{}", "=".repeat(60));
    println!("\n{}", result.final_trade_decision);
    println!("\n[Completed in {:.0}s]", elapsed.as_secs_f64());

    // Save decision to memory
    let _memory_entry = (
        result.situation_summary(),
        result.final_trade_decision.clone(),
    );

    // Store in trader memory for future reference
    // (In a real system, you'd persist this)

    Ok(())
}