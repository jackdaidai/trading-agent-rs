# TAgent architecture

TAgent is a Rust CLI that turns a ticker and trade date into a Markdown trading-analysis report. The runtime is split into four layers:

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

The final decision is printed and saved by `print_and_save` under `TAGENT_REPORTS_DIR` or `reports` by default.

## Tool calls

`GraphEngine::execute_llm_with_tools` sends a prompt plus available tool schemas to the quick LLM. If the model requests tools, TAgent executes each request through `yfinance::execute_tool`, appends tool results to the message history, and asks again. The loop is capped to avoid unbounded provider calls.

LLM response parsing should fail loudly when required tool-call fields are missing or malformed. Silent empty defaults make provider failures look like valid empty research.

## Provider configuration

`src\config.rs` owns provider selection, environment-variable overrides, report directory, and batch concurrency. Add new providers by extending `Provider`, not by adding provider-specific branching in `main`.
