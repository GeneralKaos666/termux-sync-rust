# Handoff

## Where the port stands

- Front door proven: `rust/app` builds `termux-sync-rs`, 125/125 app cases pass with
  100% fallback. Build: `cargo build --manifest-path rust/app/Cargo.toml`.
- Unit `termux-sync-core` is claimed and W1-W3 are done. W3 added local storage,
  backup/restore orchestration, list, package diff and checksum verification.
  26 unit tests; a fixture backup+restore smoke passed; 26/26 recorded unit cases.
- Functions from W4 onward still return `RuntimeError: ... is not ported yet`.
  Do not run the unit gate until they are implemented.
- W4 adds rclone and GitHub Releases storage, streaming `PartWriter` parts and
  reassembly. 30 unit tests; local backup+restore smoke passed; rclone tested with a fake
  executable. **GitHub has no live token-backed test yet**; run one before cutover.
- W6 done: all commands wired, `app lift` succeeded. `rust/app` is pure Rust with
  0% fallback; `rust/termux-sync-core` is a plain library. Replay is 10/10 stable cases.
  Most Rich/interactive cases are in `rust/app-cases-excluded/` (ADR-023).
- W7 done: TUI reads config/backups/version through the core; backup/restore stay on the
  CLI for cancellation. Version is 2.0.0. `rust/install.sh` builds/installs both binaries;
  `rust/cutover.sh` backs up and replaces the Python command.
- Post-lift hardening: `rust/app/tests/cli_integration.rs` covers the excluded CLI paths;
  `rust/smoke-github.sh` passed a live backup/list/restore/delete smoke; root `install.sh`
  and `tsctl` now install the Rust binaries.
- `termux-sync tui` launches the dashboard; `termux-sync-tui` remains an alias.
- Fallback rate: 0%. Cutover is the user's call; the Python source is still reference.
- `rust/fixtures/unit-home/.config/termux-sync/config.json` is intentionally invalid:
  one recorded `load_config` failure case depends on it. Do not "fix" it.

## In progress

- Four research tasks are done: `rust/research/http-github.md`,
  `rust/research/cli-output.md`, `rust/research/python-semantics.md`,
  `rust/research/archives.md`. Read a topic before porting its unit.
  Archive note: Termux has no `bzip2.pc`, so `bzip2-sys` builds its bundled C bzip2
  instead of linking the system library; confirm this at W2 and record it in ADR-005
  if it stays.
- The TUI still runs the Python CLI. Point it at the front door with
  `TERMUX_SYNC_BIN=rust/app/target/debug/termux-sync-rs` when testing the front door.

## Recording side effect to know about

The first recording attempt ran the README examples with the real `HOME`. It exported
the config and immediately imported that export back (tokens were empty), appended to the
real `~/.config/termux-sync/sync.log`, and left two generated JSON files. Those two files
were removed. The config's values are unchanged apart from being rewritten; its mtime is
2026-11-24. All later recording used `rust/fixtures/run-original.sh` with a fixture home.

## Cases

- `rust/app-cases/`: 125 stable cases, 100% on the original and on the front door.
- `rust/app-cases-excluded/`: backup, default `check`, `status`, `logs`, `clear-logs`
  and path-panel cases that the original cannot replay deterministically (ADR-014).
  Their behavior must be covered by Rust tests during W2/W5.
- Recording command is saved by rust-anything; `rust-anything app record .` alone should
  reuse it. The wrapper `rust/fixtures/run-original.sh` switches to fixtures only when
  `RUST_ANYTHING_FALLBACK_LOG` is set, so production fallback uses the caller's env.

## Read next

- `rust/MAP.md` for unit state and waves.
- `rust/DECISIONS.md` ADR-003, ADR-006, ADR-012, ADR-014.
- `rust/GLOSSARY.md` for names.
- `rust/app-cases/replay-report.md` for the last replay.
- The rust-anything whole-app guide.
