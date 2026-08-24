# Contributing to Guru Terminal

Guru Terminal is licensed under AGPL-3.0-only. By contributing, you agree to the [Contributor License Agreement](CLA.md) and certify that you have the right to submit the work.

## Development

- Use `AGENTS.md` as the work map and `docs/README.md` as orientation.
- Keep the React/Rust/sidecar authority boundaries intact. See `docs/architecture.md`.
- Prefer deleting obsolete pre-1.0 paths over compatibility shims.
- Prove behavior with tests. Do not add phrase-lock or source-scan tests.
- Update docs when the product story changes.
- Run `scripts/verify.sh` before requesting review.
- Never include credentials, databases, user memory, staged executables, or generated build output.

Security reports should be sent privately to the maintainer rather than opened as public issues.
