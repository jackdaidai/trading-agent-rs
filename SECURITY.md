# Security Policy

## Supported versions

Security fixes are handled on the default branch until the project publishes formal releases.

## Reporting a vulnerability

Please report security issues privately to the project maintainer instead of opening a public issue. Include enough detail to reproduce or understand the issue, but do not include live API keys, private account data, or non-public trading information.

Relevant security issues include:

- leaked or mishandled API keys
- command injection or unsafe subprocess behavior
- dependency vulnerabilities that affect runtime use
- unsafe handling of generated reports or local files
- provider/API behavior that could expose secrets or private prompts

## Secrets handling

- Store API keys in `.env` or your shell environment.
- Use `.env.example` as the public template.
- Never commit `.env`, provider keys, generated reports containing private data, or shell history.
- If a key is accidentally committed, revoke it with the provider immediately and rotate to a new key.

## Financial safety

trading-agent-rs is for research and education only. It must not be presented as a source of personalized financial, investment, tax, or legal advice.
