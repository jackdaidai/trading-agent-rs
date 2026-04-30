# Release Guide

This project does not yet publish official binary releases. Use this guide when preparing one.

## Supported targets

Start with source releases and clearly document tested platforms. Before publishing binaries, decide which targets are supported, for example:

- Windows x86_64
- Linux x86_64
- macOS ARM64/x86_64

## Pre-release checks

```powershell
cargo fmt --check
cargo test
cargo build --release
python yfinance_proxy.py get_stock_data AAPL 2026-04-01 2026-04-30
```

Use mock LLM endpoints for release smoke tests when possible, so releases do not depend on paid API calls or real secrets.

## Versioning

Use semantic versioning:

- patch: bug fixes and docs
- minor: backward-compatible features or provider additions
- major: breaking CLI, configuration, report format, or API changes

## Release contents

Each release should include:

- source archive
- changelog or release notes
- supported platform list
- setup instructions
- `.env.example`
- `LICENSE` and `NOTICE`

Do not include:

- `.env`
- generated `reports/`
- benchmark logs
- provider API keys
- local binaries or build directories unless they are intentional release artifacts

## GitHub Actions note

GitHub Actions CI can run automatically on every push and pull request. A typical workflow should run `cargo fmt --check`, `cargo test`, and Python proxy smoke tests before a release is tagged.
