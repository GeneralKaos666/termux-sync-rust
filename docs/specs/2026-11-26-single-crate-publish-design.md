# Single Crate and crates.io Readiness Design

- **Status:** Approved design, implementation not started
- **Date:** 2026-11-26
- **Scope:** Merge the `app`, `core`, and `tui` workspace crates into one
  publishable package, then prove crates.io readiness without uploading.

## Purpose

The repository currently builds three crates: a library (`termux-sync-core`), a
CLI (`termux-sync-rs`), and a dashboard (`termux-sync-tui`). All three set
`publish = false`, and the package metadata is incomplete. The goal is one
Cargo package named `termux-sync` that builds both commands, carries complete
crates.io metadata, and passes `cargo publish --dry-run` locally.

Publishing is explicitly not part of this change. No token, login, tag, GitHub
release, or upload is created.

## Goals

1. Collapse the three workspace members into one root package.
2. Keep both commands and all current behavior: `termux-sync` and
   `termux-sync-tui`, including the `termux-sync tui` subcommand.
3. Preserve the backup format, configuration format, event protocol, exit
   codes, and user-visible text.
4. Move tests, fixtures, scripts, CI, and documentation to the new layout.
5. Add complete crates.io metadata and documentation.
6. Prove packaging with `cargo package --list` and
   `cargo publish --dry-run --locked`.

## Non-Goals

- Uploading to crates.io.
- Creating a registry token, running `cargo login`, tagging, or publishing a
  GitHub release.
- Promising a stable library API.
- Feature-gating the TUI or splitting it into an optional dependency set.
- Changing backup, restore, storage, scheduling, or TUI behavior.
- Rewriting historical ADRs or old plan documents.

## Current State

- `Cargo.toml` is a virtual workspace with members `app`, `core`, and `tui`.
- `app` depends on `core` for orchestration and on `tui` for the `tui`
  subcommand.
- `tui` depends on `core` only for `core::fsutil::required_tools`.
- Every crate sets `publish = false`.
- `app` and `core` have no `license`, `repository`, or `readme` metadata.
- `core` and `tui` descriptions describe an internal port and a spike.
- `core/README.md` is stale and falsely claims a PyO3 bridge.
- `paths::VERSION` hardcodes `2.0.0` separately from the manifests.
- Tests live in `app/tests/` and `tui/tests/`; trycmd cases live in
  `app-cases/` at the repository root.
- CI is split between `.github/workflows/rust.yml` and
  `.github/workflows/tui.yml`.
- No git tags, GitHub releases, Cargo credentials, or `CARGO_REGISTRY_TOKEN`
  exist.
- The names `termux-sync`, `termux-sync-rs`, `termux-sync-core`, and
  `termux-sync-tui` are currently free on crates.io.

## Target Layout

```text
Cargo.toml
Cargo.lock
README.md
LICENSE
CHANGELOG.md
src/
  lib.rs
  frontdoor.rs
  port.rs
  core/
    mod.rs
    archive.rs
    backup.rs
    config.rs
    errors.rs
    events.rs
    fmt.rs
    fsutil.rs
    github.rs
    json.rs
    logging.rs
    maintenance.rs
    manifest.rs
    packages.rs
    paths.rs
    rclone.rs
    render.rs
    storage.rs
    term.rs
    text.rs
  tui/
    mod.rs
    app.rs
    data.rs
    history.rs
    ops.rs
    ui.rs
  bin/
    termux-sync.rs
    termux-sync-tui.rs
tests/
  cli_integration.rs
  app_cases.rs
  pty.sh
  pty_fake.sh
  fixtures/
app-cases/
fixtures/
docs/
  tui/
    README.md
    RUNBOOK.md
```

Source mapping:

| Current | New |
|---|---|
| `core/src/core/*.rs` | `src/core/*.rs` |
| `core/src/port.rs` | `src/port.rs` |
| `core/src/lib.rs` | `src/lib.rs` |
| `tui/src/lib.rs` | `src/tui/mod.rs` |
| `tui/src/{app,data,history,ops,ui}.rs` | `src/tui/*.rs` |
| `app/src/frontdoor.rs` | `src/frontdoor.rs` |
| `app/src/main.rs` | `src/bin/termux-sync.rs` |
| `tui/src/main.rs` | `src/bin/termux-sync-tui.rs` |
| `app/tests/*.rs` | `tests/*.rs` |
| `tui/tests/pty.sh`, `pty_fake.sh` | `tests/pty.sh`, `tests/pty_fake.sh` |
| `tui/tests/fixtures/` | `tests/fixtures/` (distinct from the root `fixtures/`) |
| `tui/README.md`, `tui/RUNBOOK.md` | `docs/tui/README.md`, `docs/tui/RUNBOOK.md` |
| `core/README.md` | deleted |
| `app/`, `core/`, `tui/` directories | deleted after moves |

All moves use `git mv` where possible so file history survives.

## Library Surface

`src/lib.rs` declares:

```rust
pub mod core;
pub mod frontdoor;
pub mod port;
pub mod tui;
```

The library is public only because Cargo binaries in the same package cannot
access private library items. README and crate docs state that the library is
an internal implementation detail with no stability promise. No
`#[doc(hidden)]` mass annotation and no curated facade are added.

Both binaries stay thin:

- `src/bin/termux-sync.rs` calls `termux_sync::frontdoor::run_from_argv()`.
- `src/bin/termux-sync-tui.rs` calls
  `termux_sync::tui::main_entry("termux-sync-tui", &args)`.

The `termux-sync tui` subcommand keeps calling
`termux_sync::tui::main_entry("termux-sync tui", &rest)`.

## Code Changes

- Replace the 44 `crate::app`, `crate::data`, `crate::history`, `crate::ops`,
  and `crate::ui` references in the TUI with `crate::tui::…`.
- In `src/frontdoor.rs`, replace `termux_sync_core::…` with `crate::core::…`
  and `termux_sync_tui::main_entry` with `crate::tui::main_entry`.
- In `src/tui/data.rs`, replace `termux_sync_core::core::fsutil` with
  `crate::core::fsutil`.
- In `src/core/paths.rs`, set `VERSION` to `env!("CARGO_PKG_VERSION")`.
  Keep the `VERSION` name so manifest and render code do not change.
- Update the trycmd test to register `env!("CARGO_BIN_EXE_termux-sync")` and
  resolve cases from `CARGO_MANIFEST_DIR`.
- Update `tests/pty.sh` so it builds from the repository root and runs
  `target/debug/termux-sync-tui`.
- Do not change any storage path, config key, manifest key, event name, exit
  code, or user-facing string.

## Cargo Package

Root `Cargo.toml` becomes a package manifest, not a virtual workspace.

```toml
[package]
name = "termux-sync"
version = "2.0.0"
edition = "2024"
rust-version = "1.100"
license = "MIT"
description = "Back up and restore a Termux environment across devices"
repository = "https://github.com/GeneralKaos666/termux-sync-rust"
homepage = "https://github.com/GeneralKaos666/termux-sync-rust"
readme = "README.md"
keywords = ["termux", "android", "backup", "restore", "cli"]
categories = ["command-line-utilities", "filesystem"]
publish = true

[[bin]]
name = "termux-sync"
path = "src/bin/termux-sync.rs"

[[bin]]
name = "termux-sync-tui"
path = "src/bin/termux-sync-tui.rs"

[lib]
name = "termux_sync"
path = "src/lib.rs"
```

`publish = true` is the explicit gate. It does not upload anything.

Dependencies become the union of the three current manifests:

- Runtime: `serde`, `serde_json`, `chrono`, `sha2`, `thiserror`,
  `terminal_size`, `regex`, `ureq`, `fs4`, `tar`, `flate2`, `bzip2`, `xz2`,
  `rustix`, `ratatui`, `libc`, `unicode-width`.
- Development: `trycmd`, `tempfile`, `sha2`, `tar`.

The TUI is always built. No feature flags are added.

Workspace lint settings move into `[lints.clippy]` and `[lints.rust]` in the
root manifest. `[profile.release]` stays unchanged.

Package contents are limited with `include`:

```toml
include = [
  "src/**",
  "tests/**",
  "app-cases/**",
  "fixtures/**",
  "README.md",
  "LICENSE",
  "CHANGELOG.md",
]
```

Assets, docs, `.github`, scripts, `target`, and local environment files stay
out of the crate. `Cargo.lock` remains in the repository but is not forced
into the package.

Version `2.0.0` is retained because no version has ever been published to
crates.io. `Cargo.toml` becomes the single source of truth for the version.

## Documentation and Scripts

- Rewrite `README.md` installation to show both `bash install.sh` and
  `cargo install termux-sync`.
- State that both commands are installed and that the library is internal.
- Convert README image and document links to absolute GitHub URLs so crates.io
  renders them.
- Point TUI links at `docs/tui/README.md` and `docs/tui/RUNBOOK.md` through
  absolute GitHub URLs.
- Delete `core/README.md`.
- Update `CONTRIBUTING.md`: `cargo run --bin termux-sync -- --help`,
  `cargo run --bin termux-sync -- tui`, `tests/cli_integration.rs`, and remove
  the nonexistent `tui/install.sh` reference.
- Update `GOVERNANCE.md` so a release bump edits one `Cargo.toml`.
- Add a crates.io install note to `FAQ.md`.
- Add an Unreleased CHANGELOG entry describing the single-crate layout and the
  publish metadata.
- Add one ADR to `docs/DECISIONS.md` recording the single-crate decision. Leave
  historical ADRs unchanged.
- `install.sh` builds `--bins` and installs `target/release/termux-sync` and
  `target/release/termux-sync-tui`.
- `smoke-github.sh` defaults to `target/release/termux-sync`.
- `cutover.sh` keeps its behavior; it only calls `install.sh`.

## CI

Replace `rust.yml` and `tui.yml` with one workflow. Jobs:

1. **Check:** format, clippy with `-D warnings`, full tests.
2. **PTY:** `bash tests/pty.sh`.
3. **Android:** build both Android targets with `cargo ndk`.
4. **Release:** build `--release --bins`.
5. **Package:** `cargo package --list` and
   `cargo publish --dry-run --locked`.

The package job runs without registry credentials. It fails if metadata,
path dependencies, or packaged files are invalid. No CI job uploads.

## Verification

Run in this order on a clean worktree:

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
bash tests/pty.sh
cargo build --locked --release --bins
cargo package --list
cargo publish --dry-run --locked
```

Then install into a temporary prefix:

```bash
PREFIX="$TMP/prefix" bash install.sh
"$TMP/prefix/bin/termux-sync" --version
"$TMP/prefix/bin/termux-sync-tui" --version
```

Finally verify no stale references remain:

```bash
git grep -n 'termux-sync-core\|termux-sync-rs\|app/\|core/\|tui/' \
  -- ':!docs/plans' ':!docs/specs' ':!CHANGELOG.md' ':!docs/DECISIONS.md'
```

Historical plan and ADR references are allowed. Live source, scripts, CI, and
current documentation must be clean.

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| trycmd binary name changes | Register `CARGO_BIN_EXE_termux-sync`; run the test before packaging |
| PTY harness uses `tui/..` relative path | Rewrite to repository-root paths; run it in CI and locally |
| TUI internal imports break | Mechanical `crate::tui::` rewrite covered by compiler and full test suite |
| Version drift between manifest and `paths::VERSION` | Use `env!("CARGO_PKG_VERSION")` |
| Package includes stale or private files | Explicit `include`; review `cargo package --list` |
| `publish = true` accidentally uploaded | No credentials exist; no publish job; only `--dry-run` is run |
| Dry-run needs a clean tree | Commit changes before the final dry-run; use no `--allow-dirty` |
| README links break on crates.io | Absolute GitHub URLs for images and documents |

## Acceptance Criteria

- One root package named `termux-sync`; `app`, `core`, and `tui` directories are
  gone.
- `cargo build --bins` produces `termux-sync` and `termux-sync-tui`.
- `termux-sync tui` still launches the dashboard.
- Format, clippy, tests, PTY, and release build pass.
- `cargo package --list` contains only intentional files.
- `cargo publish --dry-run --locked` succeeds on a clean worktree.
- No registry upload, token, tag, or GitHub release is created.
- No backup format, config format, event protocol, or user-visible behavior
  changes.

## Rollback

The entire change is local and reversible with `git revert` or by resetting the
merge commit. No external state is created, so no registry or release cleanup
is required.
