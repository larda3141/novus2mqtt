---
description: Run the full local CI gate — fmt, clippy, build, test
argument-hint: "[optional cargo target filter]"
allowed-tools: Bash(cargo fmt:*), Bash(cargo clippy:*), Bash(cargo build:*), Bash(cargo test:*)
---

Run the same checks CI runs, in order, and stop at the first failure. Report which step failed and the relevant output. If everything passes, give a one-line summary.

1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo build`
4. `cargo test $ARGUMENTS`

Do not auto-fix formatting or lints unless the user asks.
