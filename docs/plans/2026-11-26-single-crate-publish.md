# Single Crate and crates.io Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Merge the `app`, `core`, and `tui` crates into one `termux-sync` package and prove crates.io readiness with a successful dry run.

**Architecture:** The repository root becomes one Cargo package. Core modules move to `src/core`, TUI modules to `src/tui`, the CLI parser to `src/frontdoor.rs`, and both entry points to explicit `src/bin` targets. Tests and scripts move to the root `tests` directory. Package metadata, documentation, scripts, and CI are updated, then `cargo publish --dry-run` verifies the result without uploading.

**Tech Stack:** Rust 1.100, edition 2024, Cargo, ratatui, serde, tar/flate2/bzip2/xz2, ureq, trycmd, GitHub Actions.

**Spec:** `docs/specs/2026-11-26-single-crate-publish-design.md`

## Global Constraints

- Package name is `termux-sync`; version is `2.0.0`; edition is `2024`; MSRV is `1.100`.
- Binaries are named `termux-sync` and `termux-sync-tui`; the TUI is always built and is not feature-gated.
- Do not change backup format, configuration keys, manifest keys, event protocol, exit codes, storage paths, or user-facing behavior.
- Do not run `cargo login`, create a token, set `CARGO_REGISTRY_TOKEN`, tag, create a GitHub release, or upload to crates.io. Only `cargo publish --dry-run` is allowed.
- Every commit uses `git commit -S`. Run `git verify-commit HEAD` after each commit and require `G`.
- Run `sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"` before committing. Never print or commit the passphrase.
- No new dependency beyond the union of the three current manifests.
- Use `git mv` for source, test, and documentation moves to preserve history.
- `Cargo.toml` is the only source of the release version.
- The final dry run uses a clean worktree and no `--allow-dirty`.
- All user-facing text stays English with the current tone.

## Review Focus

- A packaged crate must still contain the fixtures that `app_cases.rs` and `pty.sh` read. Task 6 asserts the package file list includes `app-cases/` and `fixtures/`.
- `cargo install` must produce both exact command names. Task 7 installs into a temporary prefix and runs both `--version` commands.
- `termux-sync tui` must dispatch to the same dashboard with the same arguments. Task 3 checks both entry points and Task 4 runs the existing dispatch tests.
- The PTY harness must launch the new `target/debug/termux-sync-tui` path. Task 5 runs all PTY cases after the path change.
- The reported version must come from the manifest, not a second hardcoded string. Task 1 changes `paths::VERSION`; Task 4 runs the version assertions.
- Package verification must build the exact packaged source, catching path dependencies or missing targets. Task 9 runs `cargo publish --dry-run --locked` on a clean tree.

---

### Task 1: Create the root package and move core

**Files:**
- Create: `Cargo.toml` (replace the virtual workspace manifest)
- Move: `core/src/core/` to `src/core/`
- Move: `core/src/port.rs` to `src/port.rs`
- Move: `core/src/lib.rs` to `src/lib.rs`
- Move: `core/examples/` to `examples/`
- Delete: `core/`
- Modify: `examples/*.rs`
- Modify: `src/core/paths.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: library crate `termux_sync` with `termux_sync::core`, `termux_sync::port`, and `termux_sync::core::paths::VERSION: &'static str`.

- [ ] **Step 1: Replace `Cargo.toml` with the package manifest**

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
include = [
    "src/**",
    "examples/**",
    "tests/**",
    "app-cases/**",
    "fixtures/**",
    "README.md",
    "LICENSE",
    "CHANGELOG.md",
]

[lib]
name = "termux_sync"
path = "src/lib.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["preserve_order", "float_roundtrip", "unbounded_depth"] }
chrono = { version = "0.4", default-features = false, features = ["clock"] }
sha2 = "0.10"
thiserror = "2"
terminal_size = "0.4"
regex = "1"
ureq = { version = "3.5", default-features = false, features = ["rustls", "json"] }
fs4 = "0.13"
tar = { version = "0.4", default-features = false }
flate2 = { version = "1", default-features = false, features = ["miniz_oxide"] }
bzip2 = "0.6"
xz2 = "0.1"
rustix = { version = "1", features = ["fs"] }
ratatui = { version = "0.30", features = ["unstable-rendered-line-info"] }
libc = "0.2"
unicode-width = "0.2"

[dev-dependencies]
trycmd = "1"
tempfile = "3"
sha2 = "0.10"
tar = { version = "0.4", default-features = false }

[lints.clippy]
all = { level = "warn", priority = -1 }
redundant_clone = "warn"
perf = "warn"
assigning_clones = "warn"
unused_self = "warn"
match_same_arms = "warn"

[lints.rust]
missing_docs = "warn"
unused_lifetimes = "warn"
trivial_numeric_casts = "warn"

[profile.release]
lto = true
codegen-units = 1
strip = true
```

- [ ] **Step 2: Move core sources and examples**

```bash
mkdir -p src
git mv core/src/core src/core
git mv core/src/port.rs src/port.rs
git mv core/src/lib.rs src/lib.rs
git mv core/examples examples
git rm -r core
rmdir core/src core 2>/dev/null || true
```

- [ ] **Step 3: Point example imports at the package library**

Run:

```bash
sed -i 's/termux_sync_core::/termux_sync::/g' examples/*.rs
```

- [ ] **Step 4: Derive the version from the manifest**

In `src/core/paths.rs`, replace the hardcoded version line with:

```rust
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
```

- [ ] **Step 5: Refresh the lockfile without `--locked` once**

Run: `cargo check --lib`
Expected: `Finished` and an updated `Cargo.lock` that no longer lists the three
workspace crates.

Then run: `cargo check --locked --lib`
Expected: `Finished`, confirming the refreshed lockfile is consistent.

- [ ] **Step 6: Verify the library and examples**

Run: `cargo test --locked --lib`
Expected: all core unit tests pass.

Run: `cargo check --locked --examples`
Expected: all four examples compile.

- [ ] **Step 7: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add Cargo.toml Cargo.lock src examples
git commit -S -m "refactor: make the repository one package with core"
git verify-commit HEAD
```

---

### Task 2: Move the TUI modules into the package

**Files:**
- Move: `tui/src/lib.rs` to `src/tui/mod.rs`
- Move: `tui/src/app.rs`, `data.rs`, `history.rs`, `ops.rs`, `ui.rs` to `src/tui/`
- Modify: `src/lib.rs`
- Modify: `src/tui/*.rs`

**Interfaces:**
- Consumes: `termux_sync::core` from Task 1.
- Produces: `termux_sync::tui::main_entry(program: &str, args: &[String]) -> std::io::Result<std::process::ExitCode>`.

- [ ] **Step 1: Move the TUI source files**

```bash
mkdir -p src/tui
git mv tui/src/lib.rs src/tui/mod.rs
git mv tui/src/app.rs src/tui/app.rs
git mv tui/src/data.rs src/tui/data.rs
git mv tui/src/history.rs src/tui/history.rs
git mv tui/src/ops.rs src/tui/ops.rs
git mv tui/src/ui.rs src/tui/ui.rs
```

- [ ] **Step 2: Declare the TUI module**

`src/lib.rs` must contain exactly:

```rust
//! Core and terminal UI for `termux-sync`.
//!
//! The library is an implementation detail of the two binaries. It is public
//! because Cargo binaries cannot access private library items, but no API
//! stability is promised.

pub mod core;
pub mod port;
pub mod tui;
```

- [ ] **Step 3: Rewrite internal TUI paths**

Run:

```bash
sed -i -E 's/crate::(app|data|history|ops|ui)/crate::tui::\1/g' src/tui/*.rs
sed -i 's/termux_sync_core::/crate::/g' src/tui/data.rs
```

Expected: `src/tui/data.rs` calls `crate::core::fsutil::required_tools(storage)`. No `termux_sync_core` or `termux_sync_tui` remains in `src/tui`.

- [ ] **Step 4: Verify the library tests**

Run: `cargo test --locked --lib`
Expected: all core and TUI unit tests pass.

- [ ] **Step 5: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add src/lib.rs src/tui tui
git commit -S -m "refactor: move the TUI into the package library"
git verify-commit HEAD
```

---

### Task 3: Add the CLI parser and both binaries

**Files:**
- Move: `app/src/frontdoor.rs` to `src/frontdoor.rs`
- Move: `app/src/main.rs` to `src/bin/termux-sync.rs`
- Move: `tui/src/main.rs` to `src/bin/termux-sync-tui.rs`
- Modify: `src/lib.rs`
- Modify: `Cargo.toml`
- Delete: `app/Cargo.toml`, `tui/Cargo.toml`

**Interfaces:**
- Consumes: `termux_sync::core`, `termux_sync::tui::main_entry`.
- Produces: binaries `target/debug/termux-sync` and `target/debug/termux-sync-tui`.

- [ ] **Step 1: Move the entry points**

```bash
mkdir -p src/bin
git mv app/src/frontdoor.rs src/frontdoor.rs
git mv app/src/main.rs src/bin/termux-sync.rs
git mv tui/src/main.rs src/bin/termux-sync-tui.rs
git rm app/Cargo.toml tui/Cargo.toml
```

- [ ] **Step 2: Declare the front door and the binaries**

Add `pub mod frontdoor;` to `src/lib.rs` before `pub mod tui;`.

Add to `Cargo.toml` after `[lib]`:

```toml
[[bin]]
name = "termux-sync"
path = "src/bin/termux-sync.rs"

[[bin]]
name = "termux-sync-tui"
path = "src/bin/termux-sync-tui.rs"
```

- [ ] **Step 3: Rewrite imports in the front door**

Run:

```bash
sed -i 's/termux_sync_core::/crate::/g' src/frontdoor.rs
sed -i 's/termux_sync_tui::main_entry/crate::tui::main_entry/g' src/frontdoor.rs
```

Expected: no `termux_sync_core` or `termux_sync_tui` remains in `src/frontdoor.rs`.

- [ ] **Step 4: Rewrite both binary entry points**

`src/bin/termux-sync.rs`:

```rust
//! Command-line entry point for `termux-sync`.
//!
//! Argument parsing and command dispatch live in `frontdoor`; the binary only
//! maps the returned `ExitCode` to the process status.

use std::process::ExitCode;

fn main() -> ExitCode {
    termux_sync::frontdoor::run_from_argv()
}
```

`src/bin/termux-sync-tui.rs`:

```rust
//! Standalone entry point for the `termux-sync-tui` dashboard.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match termux_sync::tui::main_entry("termux-sync-tui", &args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("termux-sync-tui: {error}");
            ExitCode::from(1)
        }
    }
}
```

- [ ] **Step 5: Build and check both commands**

Run: `cargo build --locked --bins`
Expected: both binaries build.

Run: `target/debug/termux-sync --version`
Expected: `termux-sync v2.0.0`

Run: `target/debug/termux-sync tui --version`
Expected: `termux-sync tui 2.0.0`

Run: `target/debug/termux-sync-tui --version`
Expected: `termux-sync-tui 2.0.0`

- [ ] **Step 6: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add Cargo.toml src/lib.rs src/frontdoor.rs src/bin app tui
git commit -S -m "refactor: add CLI and TUI binaries to the package"
git verify-commit HEAD
```

---

### Task 4: Move and update the integration tests

**Files:**
- Move: `app/tests/cli_integration.rs` to `tests/cli_integration.rs`
- Move: `app/tests/app_cases.rs` to `tests/app_cases.rs`
- Delete: `app/`
- Modify: `tests/cli_integration.rs`
- Modify: `tests/app_cases.rs`

**Interfaces:**
- Consumes: binaries from Task 3.
- Produces: `cargo test` coverage for CLI, TUI dispatch, and recorded cases.

- [ ] **Step 1: Move the tests**

```bash
mkdir -p tests
git mv app/tests/cli_integration.rs tests/cli_integration.rs
git mv app/tests/app_cases.rs tests/app_cases.rs
rmdir app/tests app 2>/dev/null || true
```

- [ ] **Step 2: Update the binary environment variable**

Run:

```bash
sed -i 's/CARGO_BIN_EXE_termux-sync-rs/CARGO_BIN_EXE_termux-sync/g' tests/cli_integration.rs tests/app_cases.rs
```

Expected: both files use `env!("CARGO_BIN_EXE_termux-sync")`.

- [ ] **Step 3: Resolve trycmd cases from the package root**

In `tests/app_cases.rs`, replace:

```rust
let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
```

with:

```rust
let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
```

- [ ] **Step 4: Run the moved tests**

Run: `cargo test --locked --test cli_integration --test app_cases`
Expected: all integration tests and all recorded trycmd cases pass.

- [ ] **Step 5: Run the full suite**

Run: `cargo test --locked`
Expected: all unit, integration, and doc tests pass.

- [ ] **Step 6: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add tests app
git commit -S -m "test: move integration tests to the package root"
git verify-commit HEAD
```

---

### Task 5: Move the PTY harness

**Files:**
- Move: `tui/tests/pty.sh` to `tests/pty.sh`
- Move: `tui/tests/pty_fake.sh` to `tests/pty_fake.sh`
- Move: `tui/tests/fixtures/` to `tests/fixtures/`
- Modify: `tests/pty.sh`

**Interfaces:**
- Consumes: `target/debug/termux-sync-tui` from Task 3.
- Produces: `bash tests/pty.sh` with six passing cases.

- [ ] **Step 1: Move the harness**

```bash
git mv tui/tests/pty.sh tests/pty.sh
git mv tui/tests/pty_fake.sh tests/pty_fake.sh
git mv tui/tests/fixtures tests/fixtures
rmdir tui/tests 2>/dev/null || true
```

- [ ] **Step 2: Update the binary path**

In `tests/pty.sh`, replace:

```bash
BIN="$PWD/../target/debug/termux-sync-tui"
```

with:

```bash
BIN="$PWD/target/debug/termux-sync-tui"
```

The script already changes to the repository root with
`cd "$(dirname "${BASH_SOURCE[0]}")/.."`, so no other path changes are needed.

- [ ] **Step 3: Run the harness**

Run: `bash tests/pty.sh`
Expected: `backup-ok`, `backup-cancel`, `restore-plan`, `old-list`, `status`, and `restore-protected` each report assertions passed.

- [ ] **Step 4: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add tests tui
git commit -S -m "test: move the PTY harness to the package root"
git verify-commit HEAD
```

---

### Task 6: Update package documentation and content checks

**Files:**
- Modify: `README.md`
- Move: `tui/README.md` to `docs/tui/README.md`
- Move: `tui/RUNBOOK.md` to `docs/tui/RUNBOOK.md`
- Modify: `CONTRIBUTING.md`
- Modify: `GOVERNANCE.md`
- Modify: `FAQ.md`
- Modify: `CHANGELOG.md`
- Modify: `docs/DECISIONS.md`

**Interfaces:**
- Consumes: the final package layout from Tasks 1–5.
- Produces: current documentation with no stale paths and a reviewed package file list.

- [ ] **Step 1: Move the TUI documentation**

```bash
mkdir -p docs/tui
git mv tui/README.md docs/tui/README.md
git mv tui/RUNBOOK.md docs/tui/RUNBOOK.md
rmdir tui 2>/dev/null || true
```

- [ ] **Step 2: Update `README.md`**

- Change screenshot image sources from `assets/…` to
  `https://raw.githubusercontent.com/GeneralKaos666/termux-sync-rust/main/assets/…`.
- Add `cargo install termux-sync` as an installation option. State that it
  installs both commands.
- Add a short paragraph after the feature table: the library is an internal
  implementation detail and no API stability is promised.
- Replace the TUI links with
  `https://github.com/GeneralKaos666/termux-sync-rust/blob/main/docs/tui/README.md`
  and `.../docs/tui/RUNBOOK.md`.

- [ ] **Step 3: Update current-state documents**

- `CONTRIBUTING.md`: change `cargo run -p termux-sync-rs -- …` to
  `cargo run --bin termux-sync -- …`, change `app/tests/cli_integration.rs`
  to `tests/cli_integration.rs`, remove `tui/install.sh` from the shell-script
  list, and change `cargo test --locked --workspace` to `cargo test --locked`.
- `GOVERNANCE.md`: change the release step to update the version in the single
  root `Cargo.toml`.
- `FAQ.md`: add a `cargo install termux-sync` answer under Installation and note
  that it installs both commands.
- `docs/tui/README.md`: remove the `tui/install.sh` paragraph; the root
  `install.sh` installs both binaries.
- `CHANGELOG.md`: add under `[Unreleased]` a `Changed` entry describing the
  single `termux-sync` crate and crates.io metadata.
- `docs/DECISIONS.md`: append `ADR-028: One crate for the CLI, core, and TUI`
  with status accepted, the context of avoiding three path-dependent crates,
  the decision to merge, and the consequence that the library is internal.

- [ ] **Step 4: Check the package file list**

Run:

```bash
cargo package --list --locked > ~/.tmp/opencode/package-list.txt
grep -qx 'src/bin/termux-sync.rs' ~/.tmp/opencode/package-list.txt
grep -qx 'app-cases/version.toml' ~/.tmp/opencode/package-list.txt
grep -qx 'fixtures/app-ro/bin/dpkg' ~/.tmp/opencode/package-list.txt
grep -qx 'tests/fixtures/list.jsonl' ~/.tmp/opencode/package-list.txt
```

Expected: every `grep` exits 0. Then run:

```bash
grep -qE '^(app|core|tui|target)/' ~/.tmp/opencode/package-list.txt
```

Expected: `grep` exits 1 with no output, because no old crate directory or
build output is packaged.

- [ ] **Step 5: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add README.md CONTRIBUTING.md GOVERNANCE.md FAQ.md CHANGELOG.md docs
git commit -S -m "docs: describe the single-crate package"
git verify-commit HEAD
```

---

### Task 7: Update install and smoke scripts

**Files:**
- Modify: `install.sh`
- Modify: `smoke-github.sh`

**Interfaces:**
- Consumes: binaries from Task 3.
- Produces: an installer that places `termux-sync` and `termux-sync-tui` in `$PREFIX/bin`.

- [ ] **Step 1: Rewrite `install.sh` build and install steps**

The build section becomes one command:

```bash
echo "Building termux-sync (release)..."
cargo build --locked --release --manifest-path "$ROOT/Cargo.toml" --bins

install -Dm755 "$ROOT/target/release/termux-sync" "$BIN/termux-sync"
install -Dm755 "$ROOT/target/release/termux-sync-tui" "$BIN/termux-sync-tui"
```

Keep the existing header, `set -euo pipefail`, `ROOT`, `PREFIX`, `BIN`, and the
final version print lines.

- [ ] **Step 2: Update the smoke default binary**

In `smoke-github.sh`, change the default from `target/release/termux-sync-rs` to
`target/release/termux-sync`.

- [ ] **Step 3: Check script syntax**

Run: `bash -n install.sh && bash -n smoke-github.sh && bash -n cutover.sh`
Expected: exit 0 with no output.

- [ ] **Step 4: Install into a temporary prefix**

```bash
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
PREFIX="$TMP/prefix" bash install.sh
"$TMP/prefix/bin/termux-sync" --version
"$TMP/prefix/bin/termux-sync-tui" --version
```

Expected:

```text
termux-sync v2.0.0
termux-sync-tui 2.0.0
```

- [ ] **Step 5: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add install.sh smoke-github.sh
git commit -S -m "build: install both binaries from the single package"
git verify-commit HEAD
```

---

### Task 8: Consolidate CI

**Files:**
- Modify: `.github/workflows/rust.yml`
- Delete: `.github/workflows/tui.yml`

**Interfaces:**
- Consumes: final source and test layout.
- Produces: one CI workflow with check, PTY, Android, release, and package jobs.

- [ ] **Step 1: Replace `rust.yml`**

```yaml
name: CI

on:
  push:
    paths:
      - "src/**"
      - "tests/**"
      - "examples/**"
      - "app-cases/**"
      - "fixtures/**"
      - "Cargo.toml"
      - "Cargo.lock"
      - "install.sh"
      - "README.md"
      - "LICENSE"
      - ".github/workflows/rust.yml"
  pull_request:
    paths:
      - "src/**"
      - "tests/**"
      - "examples/**"
      - "app-cases/**"
      - "fixtures/**"
      - "Cargo.toml"
      - "Cargo.lock"
      - "install.sh"
      - "README.md"
      - "LICENSE"
      - ".github/workflows/rust.yml"

permissions:
  contents: read

jobs:
  check:
    name: Check
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - name: Format
        run: cargo fmt --all --check
      - name: Test
        run: cargo test --locked
      - name: Clippy
        run: cargo clippy --locked --all-targets -- -D warnings

  pty:
    name: PTY
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
      - name: PTY tests
        run: bash tests/pty.sh

  android:
    name: Android targets
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - name: Set up Android NDK
        uses: nttld/setup-ndk@v1
        with:
          ndk-version: r27c
      - name: Set up Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-linux-android,armv7-linux-androideabi
      - name: Install cargo-ndk
        run: cargo install cargo-ndk --locked
      - name: Build for Android
        run: cargo ndk -t arm64-v8a -t armeabi-v7a build --locked --release --bins

  release:
    name: Release build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
      - name: Build release binaries
        run: cargo build --locked --release --bins

  package:
    name: Package
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
      - name: List package
        run: cargo package --list --locked
      - name: Check publish
        run: cargo publish --dry-run --locked
```

- [ ] **Step 2: Delete the split TUI workflow**

Run: `git rm .github/workflows/tui.yml`

- [ ] **Step 3: Recheck local formatting and whitespace**

Run: `cargo fmt --all --check && git diff --check`
Expected: exit 0 with no output.

- [ ] **Step 4: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add .github/workflows
git commit -S -m "ci: consolidate checks for the single package"
git verify-commit HEAD
```

---

### Task 9: Run the final publish-readiness verification

**Files:**
- No planned file changes. Fix and commit only if a check fails.

**Interfaces:**
- Consumes: every earlier task.
- Produces: proof that the package passes all gates and would upload.

- [ ] **Step 1: Confirm a clean worktree**

Run: `git status --porcelain`
Expected: no output.

- [ ] **Step 2: Run the quality gates**

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
bash tests/pty.sh
cargo build --locked --release --bins
```

Expected: every command exits 0; all unit, integration, trycmd, and PTY cases
pass.

- [ ] **Step 3: Inspect the package contents**

Run:

```bash
cargo package --list --locked > ~/.tmp/opencode/package-list-final.txt
grep -E '^(src|tests|examples|app-cases|fixtures)/' ~/.tmp/opencode/package-list-final.txt | head -20
grep -qE '^(app|core|tui|target)/' ~/.tmp/opencode/package-list-final.txt
```

Expected: the first command lists package files. The final `grep` exits 1 with
no output.

- [ ] **Step 4: Prove the package passes a real dry run**

Run: `cargo publish --dry-run --locked`
Expected: Cargo packages `termux-sync v2.0.0`, verifies the packaged source,
prints `warning: aborting upload due to dry run`, and exits 0.

- [ ] **Step 5: Check for stale live references**

Run:

```bash
git grep -n -E 'termux-sync-core|termux-sync-rs|app/src|core/src|tui/src' \
  -- ':!docs/plans' ':!docs/specs' ':!CHANGELOG.md' ':!docs/DECISIONS.md'
```

Expected: no output. Historical plans, specs, changelog entries, and accepted
ADRs may mention the old layout.

- [ ] **Step 6: Report results**

Report the exact dry-run result, the two binary versions from the temporary
prefix install, and that no tag, release, login, or upload occurred. If any file
changed during verification, commit the fix with `git commit -S` and verify the
signature before reporting.
