# Development guide

## Local checks

Run these before opening a pull request:

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo test --doc
```

`Cargo.lock` is committed intentionally. Update it when dependency changes are part of the patch.

The minimum supported Rust version is 1.80.

## Configuration for local runs

Copy `.env.example` to `.env` only when running live analysis:

```powershell
Copy-Item .env.example .env
```

The default provider is MiniMax. Generic `TRADING_AGENT_*` variables override provider-specific variables. Use `TRADING_AGENT_REPORTS_DIR` to keep generated reports outside the repo root during testing. Legacy `TAGENT_*` variables are still accepted for compatibility.

## Testing provider behavior

Prefer tests that use mock JSON responses and local parsing helpers over tests that hit paid LLM APIs. Useful test cases:

- missing API key or unsupported provider
- malformed OpenAI `tool_calls[*].function.arguments`
- missing Anthropic `content` blocks
- provider HTTP 400, 429, and 500 responses
- batch mode where one ticker fails and another succeeds

Live Yahoo Finance coverage belongs behind ignored smoke tests because endpoints and market data can change.

## Helper scripts

`scripts\` contains Windows development helpers (a Defender exclusion for the build directory, MSVC environment wrappers). Treat them as local build-environment support, not release artifacts. Machine-specific setup scripts are intentionally not tracked in the repository.
