# Contributing to termux-sync

Thank you for taking the time to contribute. This document covers how to report issues, suggest features, and submit code changes.

---

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Reporting Bugs](#reporting-bugs)
- [Requesting Features](#requesting-features)
- [Development Setup](#development-setup)
- [Submitting a Pull Request](#submitting-a-pull-request)
- [Code Style](#code-style)
- [Commit Messages](#commit-messages)

---

## Code of Conduct

This project follows a simple standard: be respectful and constructive. Harassment, personal attacks, and dismissive comments are not acceptable. If you see a violation, open an issue or contact a maintainer directly.

---

## Reporting Bugs

Before opening a new issue, search the existing issues to make sure the bug has not already been reported.

When filing a bug report, include:

- Your Termux version (`termux-info` output or `pkg list-installed | grep termux`)
- Your Android version
- The termux-sync version (`termux-sync --version`)
- The storage backend you are using (local, gdrive, or github)
- The full command you ran
- The complete error output, including any log lines from `termux-sync logs`
- Steps to reproduce the problem

Use the bug report issue template when creating the issue.

---

## Requesting Features

Open a GitHub issue using the feature request template. Describe the problem you want to solve, not just the solution you have in mind. This helps maintainers understand the use case and consider alternative approaches.

---

## Development Setup

termux-sync is a Rust project. To work on it:

```bash
pkg install rust git
git clone https://github.com/GeneralKaos666/termux-sync-rust ~/termux-sync-rust
cd ~/termux-sync-rust

# Build both binaries
bash install.sh
```

Run from the checkout:

```bash
cargo run --bin termux-sync -- --help
cargo run --bin termux-sync -- tui
```

---

## Running Tests

```bash
cargo test --locked
```

Check formatting and lints before opening a pull request:

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
```

App replay cases live in `app-cases/`; CLI integration tests live in
`tests/cli_integration.rs`.

---

## Submitting a Pull Request

1. Fork the repository and create a branch from `main`.
2. Use a descriptive branch name such as `fix/restore-checksum` or `feat/bz2-compression`.
3. Make your changes. Keep each pull request focused on a single concern.
4. Run the Rust test and clippy commands above, and test your changes on a real Termux environment if possible.
5. Update documentation in `README.md` or other `.md` files if your change affects user-facing behavior.
6. Open a pull request against the `main` branch and fill in the pull request template.

Pull requests that lack context, break existing commands, or introduce large unrelated changes may be closed without merging.

---

## Code Style

- Follow standard `rustfmt` output and keep `clippy -D warnings` clean.
- Shell scripts (`install.sh`, `cutover.sh`, `smoke-github.sh`) use 4-space indentation and `set -euo pipefail`.
- Keep functions and modules focused. If a file is growing large, consider splitting it.
- Add inline comments for non-obvious logic. Avoid comments that just restate what the code does.
- Do not add a dependency without discussion in an issue first.

---

## Commit Messages

Write commit messages in the imperative mood: "Fix restore path mismatch" not "Fixed restore path mismatch".

Structure:

```
Short summary (under 72 characters)

Longer explanation if needed. Describe why the change was made, not
just what was changed. Reference relevant issues with #number.
```

Examples of good commit messages:

```
Fix SHA-256 verification skipping empty archives
Add bz2 compression support to backup command
Handle missing rclone in the status command
```

---

## Questions

If you are unsure whether something is a bug or a feature, open an issue and describe what you are seeing. Maintainers are happy to help clarify.
