use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use tagent::config::AppConfig;
use tagent::graph::engine::{GraphConfig, GraphEngine};
use tagent::graph::state::AgentState;
use tagent::llm::{AnyLLMClient, LLMClient};
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

    let app_config = AppConfig::from_env()?;

    tracing::info!(
        "Initializing TAgent with provider={}, quick={}, deep={}",
        app_config.llm.provider.as_str(),
        app_config.llm.quick_model,
        app_config.llm.deep_model
    );

    let llm_quick = Arc::new(AnyLLMClient::new(
        app_config.llm.provider.as_str(),
        &app_config.llm.quick_model,
        &app_config.llm.api_key,
        &app_config.llm.base_url,
    )?);

    let llm_deep = Arc::new(AnyLLMClient::new(
        app_config.llm.provider.as_str(),
        &app_config.llm.deep_model,
        &app_config.llm.api_key,
        &app_config.llm.base_url,
    )?);

    // Validate models
    if !llm_quick.validate_model() {
        anyhow::bail!("Invalid quick LLM configuration");
    }
    if !llm_deep.validate_model() {
        anyhow::bail!("Invalid deep LLM configuration");
    }
    tracing::debug!(
        "LLM clients initialized: quick_provider={}, deep_provider={}",
        llm_quick.provider_name(),
        llm_deep.provider_name()
    );

    // Create graph engine
    let engine = Arc::new(GraphEngine::new(
        GraphConfig::default(),
        llm_quick,
        llm_deep,
    ));

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
        use tagent::data::yfinance::YahooFinanceClient;
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
    std::fs::create_dir_all(&app_config.reports_dir)?;

    if tickers.len() == 1 {
        let ticker = &tickers[0];
        let state = AgentState::new(ticker, &trade_date);
        let start = std::time::Instant::now();
        let result = engine.run(state).await?;
        let elapsed = start.elapsed();
        print_and_save(
            &app_config.reports_dir,
            ticker,
            &trade_date,
            &result.final_trade_decision,
            elapsed,
        )?;
    } else {
        let total_start = std::time::Instant::now();
        let batch_concurrency = app_config.batch_concurrency;
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
                    print_and_save(&app_config.reports_dir, &ticker, &date, &decision, elapsed)?;
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

fn print_and_save(
    reports_dir: &Path,
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
