# Contributing

## Setup

Contributions target macOS with Herdr 0.7.4 or newer and Rust 1.88 or newer.

```bash
git clone https://github.com/sendhil/herdr-command-palette.git
cd herdr-command-palette
cargo build --release --locked
herdr plugin link "$PWD"
```

`herdr plugin link` uses the local checkout; build first because linking does
not execute the manifest build command.

## Validate changes

Run these commands before opening a pull request:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo build --release --locked
cargo test --release --locked --test performance -- --ignored --nocapture
```

## Pull requests

Use lowercase Conventional Commit subjects. Keep changes focused, add a
regression test for changed behavior, and do not add generated files or internal
planning artifacts. Do not add AI co-author lines.

For popup or runtime changes, describe the manual popup test you performed,
including open, idle, close, and process-cleanup observations. Use public Herdr
CLI/plugin/JSON contracts only; do not depend on Herdr internals.
