# TradingAgents v0.2.3→v0.2.5 Upgrade Findings — Action Items

## Status Key
- ✅ Implemented
- ⏳ Not started
- 🔄 In progress

---

## High Priority

### [x] 5-tier Rating Scale
**What:** Upgrade from BUY/HOLD/SELL (3-tier) to Buy/Overweight/Hold/Underweight/Sell (5-tier).
**Why:** Python v0.2.4 shipped this; provides more nuanced portfolio recommendations.
**Where:** `src/graph/engine.rs` — Research Manager + Portfolio Manager prompts use 5-tier; Trader keeps 3-tier (transaction direction is ternary).
**Files touched:** `src/graph/engine.rs`
**Status:** ✅ Implemented

### [x] Persistent Decision Log
**What:** Persist decisions to disk (`~/.trading-agent-rs/decisions/`) with pending/resolved tracking.
**Why:** Python v0.2.4 persistent decision log; enables cross-session learning.
**Where:** `src/memory/mod.rs` — `DecisionLog` struct, `src/graph/engine.rs` — integration.
**Files touched:** `src/memory/mod.rs`, `src/graph/engine.rs`, `Cargo.toml` (added `dirs` crate)
**Status:** ✅ Implemented

### [x] Path-traversal Security Fix
**What:** Validate ticker input — reject `../` or `/` characters before passing to data tools.
**Why:** Python v0.2.5 security fix.
**Where:** `src/data/yfinance.rs` — `validate_ticker()` + `execute_tool` entry point, `src/main.rs` early rejection.
**Files touched:** `src/data/yfinance.rs`, `src/main.rs`
**Status:** ✅ Implemented

---

## Medium Priority

### [x] Regional Benchmarks
**What:** For non-US tickers (`.NS`, `.T`, etc.), use regional benchmark instead of SPY to avoid FX drift.
**Why:** Python v0.2.5 alpha calculations use regional benchmarks.
**Where:** `src/data/yfinance.rs` — `benchmark_for_ticker()`, `src/graph/engine.rs` — market analyst prompt.
**Files touched:** `src/data/yfinance.rs`, `src/graph/engine.rs`
**Status:** ✅ Implemented

### [ ] Multi-language Output
**What:** Support language-specific prompts (EN/ZH at minimum) driven by `output_language` config.
**Why:** Python v0.2.3 shipped this; internal debate stays EN for reasoning quality.
**Where:** `src/graph/engine.rs` — `EVIDENCE_DISCIPLINE` / `DECISION_CALIBRATION` consts.
**Files touched:** `src/graph/engine.rs`, `src/config.rs`

### [ ] Social Analyst Real Data Sources
**What:** Add StockTwits/Reddit/Yahoo News sentiment sources (not just yfinance news).
**Why:** Python v0.2.5 grounded sentiment analyst pulls real social data.
**Where:** `src/tools/mod.rs`, `src/data/yfinance.rs`, `src/graph/engine.rs` — `run_social_analyst`.
**Files touched:** `src/tools/mod.rs`, `src/data/yfinance.rs`

---

## Low Priority

### [ ] Config State Isolation Verification
**What:** Verify `AppConfig::from_env()` doesn't leak state between runs (add a test with multiple rapid invocations).
**Why:** Python v0.2.5 fix: "config state no longer leaks between runs".
**Where:** `src/config.rs`
**Files touched:** `src/config.rs`

---

## Already Implemented ✅
- Provider abstraction (MiniMax/Zai/OpenAI/Anthropic)
- API key / base_url env var with dual prefix (`TRADING_AGENT_*` + `TAGENT_*`)
- Exponential backoff retry (429 / 5xx)
- Type-safe ToolName enum + ToolRegistry
- BM25 Memory (in-memory)
- Parallel analyst and risk debate execution
- Structured output hardening — provider capability table skips `tool_choice` for DeepSeek/MiniMax/Ollama
- Ollama support via custom base_url
- `max_recur_limit`, `max_debate_rounds`, `max_risk_discuss_rounds` config
- Structured output support (DeepSeek V4 / MiniMax M2.x)
- Date-aware data fetching across OHLCV, fundamentals, news endpoints