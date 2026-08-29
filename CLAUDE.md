# CLAUDE.md

Guidance for AI coding agents (Claude Code, etc.) working in this repository.

## Project overview

trading-agent-rs is a Rust-native AI stock analysis agent: a multi-agent
analyst/debate/trader/risk pipeline that produces Markdown research reports.
See `docs/ARCHITECTURE.md` for the pipeline design.

## Commands

- Build: `cargo build`
- Format: `cargo fmt --check` (CI enforces this)
- Lint: `cargo clippy --all-targets -- -D warnings` (CI treats warnings as errors)
- Tests: `cargo test --all-targets` and `cargo test --doc`

All four checks must pass before pushing; CI runs them on Windows and Ubuntu.

## Conventions

- MSRV is 1.80; `Cargo.lock` is committed intentionally.
- API keys come from `.env` / environment variables only. Never hardcode them,
  log them, or commit generated reports.
- Prefer tests with mock provider responses over tests that hit paid LLM APIs
  (see `docs/DEVELOPMENT.md`).
- Generated outputs (`reports/`, `tagent-results/`, `batch-*.txt`) are
  gitignored; do not commit them.
