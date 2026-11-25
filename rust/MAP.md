# Port map

Goal: replace the Python `termux-sync` app with a pure Rust app, keeping the original
only as reference. Deliverable: `termux-sync` (core + CLI) and `termux-sync-tui`,
sharing one Rust core crate. Original version 1.3.9 during the port; 2.0.0 at lift.

Sources of truth:

- Decisions: `rust/DECISIONS.md`
- Terms: `rust/GLOSSARY.md`
- Session handoff: `rust/HANDOFF.md`
- Measured state: `rust/PORT_MAP.md` (`rust-anything map --write`)
- Research: `rust/research/*.md`
- Recorded behavior: `rust/app-cases/`
- Original: `termux-sync.py`, `src/termux_sync/__init__.py` (byte-identical mirror)

## ADR index

| ADR | Topic | Status |
|---|---|---|
| ADR-001 | Deliverable: CLI + TUI, two binaries, one core | accepted |
| ADR-002 | Scope: all 18 commands | accepted |
| ADR-003 | Equivalence: normalized text, exact machine output | accepted |
| ADR-004 | Compatibility: full backward compatibility | accepted |
| ADR-005 | Platform: Termux aarch64, rustls, system bz2/xz | accepted |
| ADR-006 | Cutover: front door with Python fallback | accepted |
| ADR-007 | Version: 1.3.9 during port, 2.0.0 at lift | accepted |
| ADR-008 | Tests: unit tests + recorded app cases | accepted |
| ADR-009 | TUI: reuse, direct core calls at lift | accepted |
| ADR-010 | Packaging: native Rust install at lift | accepted |
| ADR-011 | TUI scope: freeze current features | accepted |
| ADR-012 | Monolith treated as one port unit, internal modules | accepted |
| ADR-013 | Checkpoints: after each internal wave and before lift | accepted |
| ADR-014 | Exclude path/time-dependent cases from app replay | accepted |
| ADR-015 | bzip2 pure Rust; xz system liblzma; MSRV 1.100 | accepted |
| ADR-016 | New archives use GNU tar, not PAX | accepted |
| ADR-017 | One restore path, with events and prompts | accepted |
| ADR-018 | GitHub uses ureq + rustls + webpki roots | accepted |
| ADR-019 | GitHub streaming keeps 200 MiB parts | accepted |
| ADR-020 | One release page; rclone stays a subprocess | accepted |
| ADR-021 | Boot script starts the Rust binary | accepted |
| ADR-022 | Setup and schedule use plain line prompts | accepted |
| ADR-023 | App replay keeps deterministic non-interactive cases | accepted |
| ADR-024 | TUI reads through core, mutations stay CLI | accepted |
| ADR-025 | Version 2.0.0 and native install | accepted |
| ADR-026 | Post-lift integration tests and installer cleanup | accepted |
| ADR-027 | TUI library with a tui CLI command | accepted |

## Units

| Unit | Original | Crate | State | Gate | Replay |
|---|---|---|---|---|---|
| termux-sync-core | `src/termux_sync/__init__.py` | `rust/termux-sync-core` | claimed by this session 2026-11-24 | pending | pending |
| tui | `tui/src/*.rs` (already Rust) | `rust/termux-sync-tui` | open | pending | pending |

`termux-sync.py` is byte-identical to `src/termux_sync/__init__.py`; one unit covers both.

## Internal waves (ADR-012)

The Python app is one tangled file; rust-anything sees one unit. To keep checkpoints
useful, port it in internal waves under one unit claim. A wave is done when its modules
compile, their unit tests pass, and recorded function cases for that wave are green.
The unit gate runs once at the end.

| Wave | Modules | State |
|---|---|---|
| W1 | config, paths, logging, events, errors, formatting helpers | done (26/26 cases, 14 unit tests) |
| W2 | archive create/extract, checksums, manifests | done (21 unit tests, Python interop) |
| W3 | local storage, backup/restore orchestration | done (backup+restore smoke passed) |
| W4 | rclone storage, GitHub Releases storage, streaming parts | done (mock rclone; GitHub untested live) |
| W5 | maintenance commands, setup, schedule, daemon | done (unit gate passed) |
| W6 | CLI parser, command dispatch, all 18 commands wired | done (lifted) |
| W7 | TUI direct core calls, version 2.0.0, native install | done (hybrid, see ADR-024) |

## Current position

- Grilling complete; ADRs written.
- Research: HTTP/GitHub, CLI/output and Python semantics done. Archive research pending.
- Whole-app recorded: 125 stable cases in `rust/app-cases`, 132/132 replay on the
  original before pruning, 100% now.
- Front door: `rust/app` builds as `termux-sync-rs`; 125/125 cases pass at 100% fallback.
- W1 done: config, paths, logging, events, errors, formatting. 26/26 recorded cases,
  14 unit tests, fmt and clippy clean. Later functions return `pending` errors.
- W2 done: archive create/extract for gz/bz2/xz, path-traversal and link checks,
  hardlinks, excludes, mtime/mode filtering, manifests. 21 unit tests including
  Python↔Rust archive interop. 26/26 recorded unit cases.
- W3 done: local storage, `cmd_backup`, `cmd_restore`, `cmd_list`, package diff,
  checksum verification, extraction roots. 26 unit tests; backup+restore smoke passed
  against a fixture home; 26/26 recorded unit cases.
- W4 done: rclone backend, GitHub Releases API, streaming 200 MiB parts, part reassembly.
  Local backup+restore smoke passed; rclone backend tested against a fake script. GitHub
  has no live token-backed test yet.
- W5 done: setup, schedule, daemon, delete, status, logs, clear-logs, check,
  clear-cache, export/import. All 42 unit exports implemented.
- W6 done: front door wired all commands; `app lift` succeeded. `rust/app` is pure Rust,
  `rust/termux-sync-core` is a plain library, fallback paths removed.
- Unit gate: passed. 28/28 unit cases, 30 Rust tests.
- App replay: 10/10 stable cases, fallback 0%.
- W7 done: TUI reads config/backups/version through the core; version 2.0.0;
  `rust/install.sh` and `rust/cutover.sh` added.
- Post-lift: 5 CLI integration tests added; GitHub live smoke passed on a temporary
  private repo; root `install.sh` and `tsctl` now install the Rust binaries.
- `termux-sync tui` launches the dashboard from the CLI; `termux-sync-tui` stays as
  an alias (ADR-027).
- Fallback rate: 0%. Cutover is the user's call.
- Last replay report: `rust/app-cases/replay-report.md`.

## Open questions

- None blocking. Recording will show which commands need fixture executables (`dpkg`,
  `pkg`, `rclone`) to stay deterministic.

## Out of scope

- New features.
- Python packaging after lift.
- Release publishing and store listings (user-owned).
- Encryption: config keys stay, but no encryption implementation exists in the original.
