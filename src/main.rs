use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tagent::config::AppConfig;
use tagent::graph::engine::{GraphConfig, GraphEngine};
use tagent::graph::state::AgentState;
use tagent::llm::{AnyLLMClient, LLMClient};
use tokio::sync::Semaphore;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const FINANCIAL_DISCLAIMER: &str = "For research and education only. Not financial advice, investment advice, or a recommendation to buy, sell, or hold any security.";

#[derive(Debug, Parser)]
#[command(
    name = "tagent",
    version,
    about = "Rust multi-agent trading-analysis reports",
    long_about = "TAgent runs a TradingAgents-style analyst, debate, trader, risk, and portfolio-manager pipeline from native Rust."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    run: RunArgs,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a ticker analysis.
    Analyze(RunArgs),
    /// Inspect local configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// List supported LLM providers and their environment variables.
    Providers,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Validate provider configuration without running market analysis.
    Check(ConfigCheckArgs),
}

#[derive(Debug, Args, Clone, Default)]
struct ConfigOverrides {
    /// Override TAGENT_PROVIDER for this run.
    #[arg(long, value_name = "PROVIDER")]
    provider: Option<String>,

    /// Override TAGENT_MODEL for both quick and deep calls.
    #[arg(long, value_name = "MODEL")]
    model: Option<String>,

    /// Override TAGENT_QUICK_MODEL for analyst/tool-heavy calls.
    #[arg(long, value_name = "MODEL")]
    quick_model: Option<String>,

    /// Override TAGENT_DEEP_MODEL for synthesis calls.
    #[arg(long, value_name = "MODEL")]
    deep_model: Option<String>,

    /// Override TAGENT_BATCH_CONCURRENCY for batch runs.
    #[arg(long, value_name = "N")]
    concurrency: Option<usize>,

    /// Override TAGENT_REPORTS_DIR for generated Markdown reports.
    #[arg(long, value_name = "DIR")]
    reports_dir: Option<PathBuf>,
}

#[derive(Debug, Args, Clone, Default)]
struct RunArgs {
    #[command(flatten)]
    config: ConfigOverrides,

    /// Trade date in YYYY-MM-DD format. If omitted, today's local date is used.
    #[arg(long, value_name = "YYYY-MM-DD")]
    date: Option<String>,

    /// Tickers to analyze. For compatibility, a final positional YYYY-MM-DD is also accepted as the date.
    #[arg(value_name = "TICKER")]
    tickers: Vec<String>,
}

#[derive(Debug, Args, Clone, Default)]
struct ConfigCheckArgs {
    #[command(flatten)]
    config: ConfigOverrides,
}

#[derive(Debug, PartialEq, Eq)]
struct RunRequest {
    tickers: Vec<String>,
    trade_date: String,
}

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

    let cli = Cli::parse();

    match cli.command {
        Some(Command::Analyze(args)) => run_analysis(args).await,
        Some(Command::Config {
            command: ConfigCommand::Check(args),
        }) => check_config(args),
        Some(Command::Providers) => {
            print_providers();
            Ok(())
        }
        None => run_analysis(cli.run).await,
    }
}

async fn run_analysis(args: RunArgs) -> Result<()> {
    apply_config_overrides(&args.config)?;
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

    let RunRequest {
        tickers,
        trade_date,
    } = resolve_run_request(
        args.tickers,
        args.date,
        chrono::Local::now().format("%Y-%m-%d").to_string(),
    )?;

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

fn check_config(args: ConfigCheckArgs) -> Result<()> {
    apply_config_overrides(&args.config)?;
    let app_config = AppConfig::from_env()?;
    println!("Configuration OK");
    println!("Provider: {}", app_config.llm.provider.as_str());
    println!("Quick model: {}", app_config.llm.quick_model);
    println!("Deep model: {}", app_config.llm.deep_model);
    println!("Reports dir: {}", app_config.reports_dir.display());
    println!("Batch concurrency: {}", app_config.batch_concurrency);
    Ok(())
}

fn print_providers() {
    println!("Supported providers:");
    println!("  minimax    API key: MINIMAX_API_KEY     Base URL: MINIMAX_BASE_URL");
    println!("  zai        API key: ZAI_API_KEY         Base URL: ZAI_BASE_URL");
    println!("  openai     API key: OPENAI_API_KEY      Base URL: OPENAI_BASE_URL");
    println!("  anthropic  API key: ANTHROPIC_API_KEY   Base URL: ANTHROPIC_BASE_URL");
    println!();
    println!("Generic overrides: TAGENT_API_KEY, TAGENT_BASE_URL, TAGENT_MODEL, TAGENT_QUICK_MODEL, TAGENT_DEEP_MODEL");
}

fn apply_config_overrides(overrides: &ConfigOverrides) -> Result<()> {
    set_env_if_present("TAGENT_PROVIDER", overrides.provider.as_deref());
    set_env_if_present("TAGENT_MODEL", overrides.model.as_deref());
    set_env_if_present("TAGENT_QUICK_MODEL", overrides.quick_model.as_deref());
    set_env_if_present("TAGENT_DEEP_MODEL", overrides.deep_model.as_deref());

    if let Some(concurrency) = overrides.concurrency {
        if concurrency == 0 {
            anyhow::bail!("--concurrency must be at least 1");
        }
        std::env::set_var("TAGENT_BATCH_CONCURRENCY", concurrency.to_string());
    }

    if let Some(reports_dir) = &overrides.reports_dir {
        if reports_dir.as_os_str().is_empty() {
            anyhow::bail!("--reports-dir must not be empty");
        }
        std::env::set_var("TAGENT_REPORTS_DIR", reports_dir);
    }

    Ok(())
}

fn set_env_if_present(key: &str, value: Option<&str>) {
    if let Some(value) = value {
        std::env::set_var(key, value);
    }
}

fn resolve_run_request(
    mut positional: Vec<String>,
    cli_date: Option<String>,
    default_date: String,
) -> Result<RunRequest> {
    let date_re = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap();
    let trade_date = match cli_date {
        Some(date) => {
            ensure_date_format(&date)?;
            date
        }
        None => match positional.last() {
            Some(last) if date_re.is_match(last) => positional.pop().unwrap(),
            _ => default_date,
        },
    };

    ensure_date_format(&trade_date)?;

    if positional.is_empty() {
        positional.push("NVDA".to_string());
    }

    Ok(RunRequest {
        tickers: positional,
        trade_date,
    })
}

fn ensure_date_format(date: &str) -> Result<()> {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .with_context(|| format!("Invalid date '{}'. Expected YYYY-MM-DD.", date))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_nvda_and_today_when_no_args() {
        let request = resolve_run_request(Vec::new(), None, "2026-05-01".to_string()).unwrap();
        assert_eq!(
            request,
            RunRequest {
                tickers: vec!["NVDA".to_string()],
                trade_date: "2026-05-01".to_string()
            }
        );
    }

    #[test]
    fn accepts_legacy_final_positional_date() {
        let request = resolve_run_request(
            vec![
                "AAPL".to_string(),
                "MSFT".to_string(),
                "2026-04-30".to_string(),
            ],
            None,
            "2026-05-01".to_string(),
        )
        .unwrap();

        assert_eq!(
            request,
            RunRequest {
                tickers: vec!["AAPL".to_string(), "MSFT".to_string()],
                trade_date: "2026-04-30".to_string()
            }
        );
    }

    #[test]
    fn cli_date_overrides_default_date() {
        let request = resolve_run_request(
            vec!["AAPL".to_string()],
            Some("2026-04-30".to_string()),
            "2026-05-01".to_string(),
        )
        .unwrap();

        assert_eq!(
            request,
            RunRequest {
                tickers: vec!["AAPL".to_string()],
                trade_date: "2026-04-30".to_string()
            }
        );
    }

    #[test]
    fn rejects_invalid_cli_date() {
        let err = resolve_run_request(
            vec!["AAPL".to_string()],
            Some("2026-13-30".to_string()),
            "2026-05-01".to_string(),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("Invalid date"));
    }

    #[test]
    fn clap_accepts_legacy_top_level_ticker_args() {
        let cli = Cli::try_parse_from(["tagent", "AAPL", "MSFT", "2026-04-30"]).unwrap();

        assert!(cli.command.is_none());
        assert_eq!(
            cli.run.tickers,
            vec![
                "AAPL".to_string(),
                "MSFT".to_string(),
                "2026-04-30".to_string()
            ]
        );
    }

    #[test]
    fn clap_accepts_analyze_subcommand_args() {
        let cli = Cli::try_parse_from([
            "tagent",
            "analyze",
            "AAPL",
            "--date",
            "2026-04-30",
            "--concurrency",
            "2",
        ])
        .unwrap();

        match cli.command {
            Some(Command::Analyze(args)) => {
                assert_eq!(args.tickers, vec!["AAPL".to_string()]);
                assert_eq!(args.date, Some("2026-04-30".to_string()));
                assert_eq!(args.config.concurrency, Some(2));
            }
            _ => panic!("expected analyze command"),
        }
    }
}
