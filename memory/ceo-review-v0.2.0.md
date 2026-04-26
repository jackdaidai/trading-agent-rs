# CEO Review Summary - TAgent v0.2.0

**Date:** 2026-04-26
**Mode:** Selective Expansion (Approach #2: Structural Improvement)

## Strongest Challenges
1. MACD signal line is mathematically wrong (macd * 0.9 instead of 9-period EMA)
2. Single-round tool calling caps analyst research depth
3. 4 duplicate tools (balance_sheet, cashflow, income_statement, insider) all return identical data

## Accepted Scope (16 items)

| # | Item | Effort | Priority |
|---|------|--------|----------|
| 4.1 | Fix MACD signal line (proper 9-period EMA) | S | P0 - quality |
| 1.2 | Merge 4 duplicate tools into 1 get_financials | S | P0 - quality |
| 1.1 | Multi-round tool loop (max 3 iterations) | M | P0 - quality |
| 1.3 | Retry with exponential backoff on 429/5xx | S | P1 - resilience |
| 2.1 | Expose HTTP status codes for retry decisions | S | P1 - resilience |
| 2.2 | Warn-level logging on tool failures | XS | P1 - resilience |
| 4.2 | Validate ticker before running pipeline | XS | P1 - resilience |
| 5.1 | Extract 6 phases into private methods | S | P2 - quality |
| 7.1 | Per-phase timing instrumentation | XS | P2 - observability |
| 8.1 | Persist reports to ./reports/ | S | P2 - feature |
| 9.1 | Switch to clap for CLI | S | P2 - UX |
| 11.1 | Structured console output with verdict summary | S | P2 - UX |
| 10.2 | Parallel batch mode with concurrency limit | M | P3 - feature |
| 6.1 | Unit tests for tool dispatch, debate state, prompts | M | P3 - quality |
| 1.4 | Remove empty graph/nodes/mod.rs | XS | P3 - cleanup |
| 1.5 | Remove unused StateUpdate struct | XS | P3 - cleanup |

## Deferred
- BM25 IDF recomputation optimization (premature at current scale)

## NOT in Scope
- Configurable pipeline (YAML/TOML)
- Streaming output
- CI/CD setup
- Full integration tests
- Separate Yahoo API endpoints for each financial tool

## Implementation Order
1. P0: Fix MACD, merge tools, multi-round tool loop
2. P1: Retries, status codes, ticker validation, tool failure logging
3. P2: Refactor engine.rs, timing, reports, clap CLI, structured output
4. P3: Batch mode, unit tests, dead code cleanup
