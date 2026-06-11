# trading-agent-rs architecture

trading-agent-rs is a Rust CLI that turns a ticker and trade date into a Markdown trading-analysis report. The runtime is split into four layers:

1. `src\main.rs` parses CLI arguments, loads `AppConfig`, validates tickers, controls batch concurrency, and writes reports.
2. `src\graph\engine.rs` runs the analysis graph. It calls analysts, debate phases, trader synthesis, risk review, and portfolio-manager synthesis in order.
3. `src\llm\mod.rs` hides provider differences behind `LLMClient`. OpenAI-compatible providers use chat completions; MiniMax, Z.ai, and Anthropic use the Anthropic-compatible message format.
4. `src\data\yfinance.rs` fetches market data, indicators, fundamentals, and news from Yahoo Finance endpoints and exposes them as tool-call functions.

## State flow

`AgentState` is created from a ticker and trade date in `src\graph\state.rs`. Each graph phase appends or replaces a focused part of the state:

- analyst phases populate market, sentiment, news, and fundamentals reports
- bull and bear researchers write an investment debate history
- the research manager turns debate history into an investment plan
- the trader turns the plan into a trading plan
- risky, safe, and neutral analysts write risk debate history
- the portfolio manager writes the final trade decision

The final decision is printed and saved by `print_and_save` under `TRADING_AGENT_REPORTS_DIR` or `reports` by default. Legacy `TAGENT_REPORTS_DIR` is still accepted.

## Tool calls

Each analyst prompt prescribes exactly which data it needs, so the engine prefetches that data (stock data, indicators, benchmark, news, global news, fundamentals) and inlines it into the first prompt. Tools remain available for follow-up requests, but a typical analyst completes in one LLM call instead of several tool round trips.

`GraphEngine::execute_llm_with_tools` sends a prompt plus available tool schemas to the quick LLM. If the model requests tools, trading-agent-rs executes each request through the engine's tool cache (`cached_tool`), appends tool results to the message history, and asks again. The loop is capped; when the budget is exhausted the model is explicitly told to synthesize. The cache is keyed by tool name plus arguments and is shared across the process, so the social and news analysts share one ticker-news fetch and global market news is fetched once per batch.

LLM response parsing should fail loudly when required tool-call fields are missing or malformed. Silent empty defaults make provider failures look like valid empty research. The same rule applies to market data: missing OHLC arrays are an error, never a silent fallback.

## Concurrency and rate limits

Batch mode runs multiple tickers concurrently (`TRADING_AGENT_BATCH_CONCURRENCY`), while a process-wide semaphore in `src\llm\mod.rs` caps in-flight LLM requests (`TRADING_AGENT_LLM_CONCURRENCY`, default 4). The semaphore is the rate-limit guard: ticker concurrency can rise without tripping provider 429 limits, and 429/5xx retries honor the provider's `Retry-After` header.

## Memory and the decision feedback loop

Each completed analysis appends a pending entry to the decision log (`~\.trading-agent-rs\decisions\decisions.json`) and stores the run as a retrievable lesson in the per-agent BM25 memories (`~\.trading-agent-rs\memory\{bull,bear,trader}.json`). The `resolve` subcommand (`src\resolve.rs`) later scores pending decisions that are older than the horizon against realized returns and alpha vs the regional benchmark, writes a reflection, and records the outcome into the same memories — so future debates retrieve what actually happened, not just what was previously rated.

## Provider configuration

`src\config.rs` owns provider selection, environment-variable overrides, report directory, and batch concurrency. Add new providers by extending `Provider`, not by adding provider-specific branching in `main`.
