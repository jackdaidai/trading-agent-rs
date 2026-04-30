# TAgent

TAgent is a Rust-based trading-analysis agent that combines market data, news, fundamentals, multi-agent debate, and LLM synthesis into a generated ticker report.

It is a Rust rebuild inspired by [TauricResearch/TradingAgents](https://github.com/TauricResearch/TradingAgents). The main motivation is performance: the original Python workflow can become slow when analyzing multiple tickers, because each ticker runs a long multi-agent pipeline. TAgent keeps the TradingAgents-style research/debate/synthesis flow, but rewrites the orchestration in Rust so ticker validation, analyst phases, debates, and batch runs can execute with lower overhead and controlled concurrency.

> This project is for research and education only. It is not financial advice, investment advice, or a recommendation to buy, sell, or hold any security.
>
> TAgent is a Rust rebuild inspired by [TauricResearch/TradingAgents](https://github.com/TauricResearch/TradingAgents), which is licensed under Apache-2.0.

## What it does

- Fetches market data, technical indicators, fundamentals, and news through native Rust HTTP calls to Yahoo Finance public endpoints.
- Runs a multi-phase analysis pipeline:
  1. market, sentiment, news, and fundamentals analysts
  2. bull/bear debate
  3. research-manager synthesis
  4. trader plan
  5. risk debate
  6. portfolio-manager final synthesis
- Supports Anthropic-compatible providers, OpenAI-compatible providers, MiniMax, and Z.ai.
- Saves generated reports under `reports/`.
- Improves multi-ticker workflows with Rust async execution and configurable batch concurrency.

## Prerequisites

- Rust 1.80 or newer with Cargo
- An API key for one supported LLM provider

## Setup

```powershell
git clone <repo-url>
cd tagent

Copy-Item .env.example .env
```

Edit `.env` and set the provider plus API key you want to use. The default provider is MiniMax:

```dotenv
TAGENT_PROVIDER=minimax
MINIMAX_API_KEY=your-key-here
```

Build and run the tests:

```powershell
cargo test
cargo build --release
```

After a release build, the Windows binary is at `target\release\tagent.exe`.

## Usage

Run one ticker for today's local date:

```powershell
cargo run -- AAPL
```

Or, after building a release binary:

```powershell
target\release\tagent.exe AAPL
```

Run one ticker for an explicit date:

```powershell
cargo run -- AAPL 2026-04-30
```

Run multiple tickers:

```powershell
cargo run -- AAPL MSFT NVDA 2026-04-30
```

Generated reports are written to `reports/<TICKER>_<DATE>.md`.

## Configuration

TAgent loads `.env` automatically. Generic `TAGENT_*` variables override provider-specific values.

| Variable | Description | Default |
| --- | --- | --- |
| `TAGENT_PROVIDER` | `minimax`, `zai`, `openai`, or `anthropic` | `minimax` |
| `TAGENT_API_KEY` | Generic API key override | unset |
| `TAGENT_BASE_URL` | Generic base URL override | provider default |
| `TAGENT_MODEL` | Generic model override for both quick and deep calls | provider default |
| `TAGENT_QUICK_MODEL` | Model for analyst/tool-heavy calls | `TAGENT_MODEL` |
| `TAGENT_DEEP_MODEL` | Model for synthesis calls | `TAGENT_MODEL` |
| `TAGENT_BATCH_CONCURRENCY` | Number of ticker analyses to run concurrently in batch mode | `1` |
| `TAGENT_REPORTS_DIR` | Directory for generated Markdown reports | `reports` |
| `TAGENT_YAHOO_BASE_URL` | Optional Yahoo Finance base URL override | `https://query1.finance.yahoo.com` |

Provider-specific variables:

| Provider | API key | Base URL | Default model |
| --- | --- | --- | --- |
| MiniMax | `MINIMAX_API_KEY` | `MINIMAX_BASE_URL` | `MiniMax-M2.7` |
| Z.ai | `ZAI_API_KEY` | `ZAI_BASE_URL` | `GLM-5.1` |
| OpenAI | `OPENAI_API_KEY` | `OPENAI_BASE_URL` | `gpt-4o` |
| Anthropic | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL` | `claude-sonnet-4-6` |

For rate-limit-sensitive providers, keep `TAGENT_BATCH_CONCURRENCY=1`.

## Development

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

GitHub Actions CI runs these checks on every push and pull request.

Yahoo Finance data is fetched natively from Rust; no Python runtime is required for normal use.

## Project docs

- [Contributing guide](CONTRIBUTING.md)
- [Architecture notes](docs/ARCHITECTURE.md)
- [Development guide](docs/DEVELOPMENT.md)
- [Security policy](SECURITY.md)
- [Release guide](docs/RELEASE.md)
- [Mock report example](examples/mock-report.md)

## License and attribution

TAgent is licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).

This project is a Rust rebuild inspired by [TauricResearch/TradingAgents](https://github.com/TauricResearch/TradingAgents), which is also licensed under Apache-2.0. See [NOTICE](NOTICE).
