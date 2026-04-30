# Contributing to TAgent

Thanks for helping improve TAgent. This project combines Rust orchestration, native Yahoo Finance data fetching, and LLM provider integrations, so contributions should keep reliability, safety, and reproducibility in mind.

## Development setup

```powershell
Copy-Item .env.example .env
cargo test
```

Fill in `.env` only when you need to run live provider-backed analysis. Do not commit real API keys, generated reports, benchmark output, or local binaries.

`Cargo.lock` is intentionally committed because TAgent is a binary application and release builds should be reproducible.

## Before opening a pull request

Run the checks that apply to your change:

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

If your change affects Yahoo Finance data fetching, also run at least one live smoke test through the Rust CLI with a mock or low-cost LLM endpoint when possible.

```powershell
cargo test
```

For LLM/provider behavior, prefer mock endpoints or small targeted tests where possible. Avoid adding tests that require paid API calls or real secrets.

## Pull request guidelines

- Keep changes focused and explain the user-visible impact.
- Add or update tests for behavior changes.
- Update `README.md`, `.env.example`, or examples when configuration or output changes.
- Preserve the financial-advice disclaimer in CLI output, generated reports, examples, and docs.
- Do not include generated `reports/`, `target/`, `target-rel/`, benchmark logs, local helper scripts, or secrets.

## Coding conventions

- Rust code should be formatted with `cargo fmt`.
- Prefer explicit error messages over silent fallbacks.
- Keep provider-specific behavior isolated in the LLM/data integration layers.
- Keep batch defaults conservative to avoid provider rate limits.

## Reporting bugs

When filing an issue, include:

- OS and shell
- Rust version
- Provider name and model, without API keys
- Command run
- Relevant error output
- Whether the issue reproduces with `TAGENT_BATCH_CONCURRENCY=1`
