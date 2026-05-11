# Release Guide

This project does not yet publish official binary releases. Use this guide when preparing one.

Do not commit build outputs such as `target/`, `target-rel/`, or `.exe` files to the repository. If you want to distribute binaries, attach them to GitHub Releases or publish them from CI as release artifacts.

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
- platform binaries, if intentionally publishing binary releases
- platform binaries, if intentionally publishing binary releases

Do not include:

- `.env`
- generated `reports/`
- benchmark logs
- provider API keys
- `target/` or `target-rel/` build directories
- local binaries committed to git

## Windows binary distribution

On Windows, `cargo build --release` produces:

```powershell
target\release\trading-agent-rs.exe
```

For users who do not want to install Rust, publish a zip file in GitHub Releases, for example:

```text
trading-agent-rs-vX.Y.Z-windows-x86_64.zip
├── trading-agent-rs.exe
├── .env.example
├── README.md
├── LICENSE
└── NOTICE
```

The Windows binary fetches Yahoo Finance data natively through Rust HTTP calls, so users do not need Python for normal use.

## GitHub Actions note

GitHub Actions CI can run automatically on every push and pull request. A typical workflow should run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` before a release is tagged.
