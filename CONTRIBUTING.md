# Contributing to chist

Patches welcome. A few ground rules:

- Keep the binary small. `chist` is happy at ~3 MB stripped; please don't
  pull in heavy dependencies for niceties.
- Keep the surface small. New subcommands need a real reason — "could be
  useful" isn't one.
- Tests are nice to have, especially around the JSONL parsing and slug
  resolution. The codebase doesn't have a comprehensive suite yet, but
  please run `cargo test` and `cargo clippy --all-targets -- -D warnings`
  before opening a PR.

If you're not sure whether a change fits, open an issue first and we can
talk it through.

### Local development

    cargo build
    cargo test
    cargo clippy --all-targets -- -D warnings
    cargo fmt --all -- --check
