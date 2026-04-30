# TAgent

TAgent is a Rust-based trading-analysis agent that combines market data, news, fundamentals, multi-agent debate, and LLM synthesis into a generated ticker report.

> This project is for research and education only. It is not financial advice, investment advice, or a recommendation to buy, sell, or hold any security.
>
> TAgent is a Rust rebuild inspired by [TauricResearch/TradingAgents](https://github.com/TauricResearch/TradingAgents), which is licensed under Apache-2.0.

## What it does

- Fetches market data, technical indicators, fundamentals, and news through a Python `yfinance` proxy.
- Runs a multi-phase analysis pipeline:
  1. market, sentiment, news, and fundamentals analysts
  2. bull/bear debate
  3. research-manager synthesis
  4. trader plan
  5. risk debate
  6. portfolio-manager final synthesis
- Supports Anthropic-compatible providers, OpenAI-compatible providers, MiniMax, and Z.ai.
- Saves generated reports under `reports/`.

## Prerequisites

- Rust toolchain with Cargo
- Python 3.10+
- Python dependencies from `requirements.txt`
- An API key for one supported LLM provider

## Setup

```powershell
git clone <repo-url>
cd tagent

python -m pip install -r requirements.txt
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

## Usage

Run one ticker for today's local date:

```powershell
cargo run -- AAPL
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
| `TAGENT_YFINANCE_PROXY` | Path to the Python yfinance proxy script | `yfinance_proxy.py` |

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

The Python proxy can also be exercised directly:

```powershell
python yfinance_proxy.py get_stock_data AAPL 2026-04-01 2026-04-30
python yfinance_proxy.py get_indicators AAPL 2026-04-30 30
python yfinance_proxy.py get_financials AAPL
python yfinance_proxy.py get_news AAPL 2026-04-01 2026-04-30
```

## Project docs

- [Contributing guide](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- [Release guide](docs/RELEASE.md)
- [Mock report example](examples/mock-report.md)

## License and attribution

TAgent is licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).

This project is a Rust rebuild inspired by [TauricResearch/TradingAgents](https://github.com/TauricResearch/TradingAgents), which is also licensed under Apache-2.0. See [NOTICE](NOTICE).
