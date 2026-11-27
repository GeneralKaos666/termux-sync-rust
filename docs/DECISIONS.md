# Port decisions

One ADR per decision, numbered in order, never renumbered. A changed decision gets a new
ADR that supersedes the old.

## ADR-001: Deliverable is the CLI plus the TUI, two binaries over one core

Status: accepted (2026-11-24)

Context: The app is a Python CLI. A separate Rust TUI already exists and shells out to
the CLI with `--json-events`.

Decision: Produce `termux-sync` and `termux-sync-tui` as two binaries sharing one Rust
core crate. Preserve the current two-command installation model.

Consequences: The core must be a library crate. The TUI can later call the core directly.
Both binaries share types, storage backends and error handling.

## ADR-002: Port all 18 commands

Status: accepted (2026-11-24)

Context: `COMMANDS` lists backup, restore, list, delete, setup, schedule, daemon,
export-config, import-config, status, logs, clear-logs, check, clear-cache and help.
`--version` and `--help` are separate paths.

Decision: Port every command, including interactive setup, schedule and daemon. Keep the
unused `encrypt` and `encrypt_password` config keys so configs stay compatible; do not
implement encryption.

Consequences: The original can be retired after the lift. Interactive flows need Rust
prompt handling. Dead encryption fields stay as accepted configuration data.

## ADR-003: Normalized human text, exact machine output

Status: accepted (2026-11-24)

Context: The original uses Rich for tables, colors, panels and prompts. Rich output is
not a stable interface; JSON events, exit codes and on-disk data are.

Decision: Match exit codes, JSON event payloads, config, cron, manifest, log and archive
data exactly. Match human stdout/stderr after normalizing ANSI styling, spacing, box
drawing and prompt layout. Wording and hints that carry behavior must stay.

Consequences: The Rust app may use simpler terminal styling. Scripts that parse JSON
events or exit codes keep working. Nobody may depend on Rich column widths.

## ADR-004: Full backward compatibility

Status: accepted (2026-11-24)

Context: Users already have configs, logs, schedules and backups on disk.

Decision: Keep `~/.config/termux-sync/` paths and key names, CLI command and flag names,
environment variables (`HOME`, `PREFIX`, `TMPDIR`, `TERMUX_SYNC_DEBUG`), event protocol
version 1, and backup archive/manifest layouts. Old backups must restore.

Consequences: The Rust app reads and writes the same files. Archive part naming and
manifest fields stay as in `_PartWriter` and `write_manifest`. New fields need defaults.

## ADR-005: Termux aarch64, rustls, system bz2/xz

Status: accepted (2026-11-24)

Context: Target device is Termux on Android aarch64. Rust 1.100 is installed.
`libbz2` and `liblzma` are available. Python uses gz, bz2 and xz.

Decision: Target Termux aarch64. Use rustls for HTTPS, never OpenSSL. Use `flate2` with
`miniz_oxide` for gzip, and the system `libbz2`/`liblzma` through Rust crates for bz2 and
xz. Do not add a general-purpose C dependency beyond those compression libraries.

Consequences: Builds work without OpenSSL headers. Compression output stays compatible
with Python's tarfile for all three formats.

## ADR-006: Rust front door with Python fallback, named termux-sync-rs

Status: accepted (2026-11-24)

Context: The port must stay runnable and reduce fallback gradually, but the installed
`termux-sync` is in daily use.

Decision: `app` owns the entry point as `termux-sync-rs`. Unported commands run the
original with the same argv, stdin, environment and pass through stdout, stderr and exit
code. Rename to `termux-sync` only at lift.

Consequences: Daily use can point at the front door early. The kill switch per unit stays
until lift. The Python command remains available throughout the port.

## ADR-007: Version 1.3.9 during the port, 2.0.0 at lift

Status: accepted (2026-11-24)

Context: `--version` is recorded behavior and the TUI probes it.

Decision: Report `termux-sync v1.3.9` while porting. Bump to `2.0.0` in the lift commit,
with changelog and install metadata.

Consequences: Recorded cases match during the port. The major bump marks the rewrite.

## ADR-008: Unit tests plus recorded app cases

Status: accepted (2026-11-24)

Context: The Python suite is 2,947 lines and mocks `urllib` and `subprocess` heavily.

Decision: Record whole-app CLI behavior into `app-cases/` and replay it. Add Rust
unit tests for internal rules (archive safety, manifest shapes, redaction, formatting,
package names, storage parsing). Do not translate every Python test literally.

Consequences: Outer behavior is covered by replay. Security-sensitive internals get
direct tests. Python tests remain reference material until lift.

## ADR-009: Reuse the TUI; direct core calls at lift

Status: accepted (2026-11-24)

Context: `tui/` already holds 7,573 lines of working Rust that shells out to the CLI.

Decision: Keep the existing TUI code and behavior. While porting, point it at
`termux-sync-rs`. At lift, replace the subprocess and event boundary with calls into the
shared core crate.

Consequences: No TUI rewrite from scratch. The event protocol is still needed during the
port and as a compatibility surface, then becomes an internal detail.

## ADR-010: Native Rust install at lift

Status: accepted (2026-11-24)

Context: Installation currently uses pip/uv and `install.sh`; `pyproject.toml` and
`uv.lock` describe the Python package.

Decision: At lift, ship built Rust binaries through `install.sh` (and `cargo` where
useful). Remove pip/uv from the install path. Keep Python files as reference until the
user retires them.

Consequences: No Python runtime requirement after lift. Packaging work is explicit in the
lift checklist.

## ADR-011: TUI scope is frozen

Status: accepted (2026-11-24)

Context: The TUI just reached CLI parity. Rewrites tempt scope growth.

Decision: Port exactly the current TUI features and CLI parity. Add no new TUI features
during the rewrite.

Consequences: Review compares against the current TUI behavior. New features wait.

## ADR-012: The monolith is one port unit with internal waves

Status: accepted (2026-11-24)

Context: `analyze --whole-app` sees one Python package (plus its byte-identical mirror)
and marks it caution, not split. The file is 3,326 lines with cross-cutting helpers.

Decision: Bridge it as one unit, `termux-sync-core`, with `port.rs` delegating to internal
modules. Track internal waves W1-W7 in `MAP.md`. Run `gate` once on the unit. Wire
commands only after the unit gate passes.

Consequences: Fallback rate stays 100% until the unit is complete, then drops per command.
Checkpoints happen after each wave even though rust-anything sees one unit. The glue in
`app` calls the core module for wired commands.

## ADR-013: Checkpoints after each wave and before lift

Status: accepted (2026-11-24)

Context: The user wants review points without stopping every unit.

Decision: Stop for go-ahead after the front door, after each internal wave, and before
`app lift`. Each checkpoint updates `MAP.md` and writes `HANDOFF.md`.

Consequences: Long waves still have review. The user controls cutover.

## ADR-014: Exclude path-rendering and time-dependent cases from app replay

Status: accepted (2026-11-24)

Context: The original does not replay its own recorded cases for commands that print the
temporary case path inside Rich panels, or that derive backup names from the clock. The
recorded table width and the path text depend on the random `rust-anything-app-run-*`
suffix, and a backup name changes every second, so archives and manifests differ between
two runs. These are recording noise, not behavior.

Decision: Keep stable cases in `app-cases` (125 cases, replayed at 100%). Move the
unstable ones to `app-cases-excluded/` for reference. Cover backup archives,
restore extraction, `status`, `logs`, `clear-logs`, default `check`, and path-rendering
output with Rust unit and integration tests that normalize timestamps and paths.

Consequences: `app replay` proves the stable CLI surface. Backup bytes and path layout
are proven by Rust tests that compare extracted contents and normalized text, not by
trycmd snapshots. The excluded cases are not part of the gate.

## ADR-015: bzip2 is pure Rust; xz links system liblzma; MSRV 1.100

Status: accepted (2026-11-24)

Context: ADR-005 preferred system bz2/xz. Termux has `libbz2` and `bzlib.h` but no
`bzip2.pc`, so `bzip2-sys` would build bundled C. Rust 1.100 is installed and
`std::fs::set_times` needs it.

Decision: Use `bzip2`'s pure-Rust default encoder, `xz2` with the system `liblzma`,
`flate2`/`miniz_oxide` for gzip, and set the core crate MSRV to 1.100.

Consequences: No C build for bzip2 on Termux. xz still requires `liblzma` headers and
library, which Termux provides. The minimum Rust version is the installed toolchain,
not 1.85.

## ADR-016: New archives use GNU tar, not Python's PAX

Status: accepted (2026-11-24)

Context: The `tar` crate writes GNU long-name records and complete headers; Python
`tarfile` writes PAX when a field needs it. Byte-identical archives are impossible anyway
because gzip stores a timestamp.

Decision: Write GNU tar with `HeaderMode::Complete`, `follow_symlinks(false)` and
`sparse(false)`. Python must read Rust archives and Rust must read Python archives. The
manifest checksum always covers the archive written by the current run.

Consequences: Sub-second mtimes and `uname`/`gname` are lost; archive bytes differ from
Python's. Restore behavior and contents stay compatible. ADR-004's on-disk contract is
about layout and readability, not archive bytes.

## ADR-017: One restore path, with events and prompts

Status: accepted (2026-11-24)

Context: Python has a Rich restore path and a separate JSON-event path that the TUI uses.
Keeping two ports would duplicate extraction, verification and package logic.

Decision: Implement one restore orchestration in Rust. It emits protocol-v1 events when
event mode is on and asks a plain terminal confirmation when it is off. The same checks
and extraction order apply in both modes.

Consequences: Event and human restore cannot drift. Prompt styling is simpler than Rich.
`RESTORE` remains the JSON confirmation token; a terminal answer accepts `y`/`yes`.

## ADR-018: GitHub storage uses ureq with rustls and webpki roots

Status: accepted (2026-11-24)

Context: `reqwest` 0.13 on Termux pulls `aws-lc-sys` and a platform verifier that needs a
JVM. `ureq` builds with rustls/ring and static webpki roots and reaches the GitHub API.

Decision: Use `ureq` 3 with `rustls`, `webpki-roots`, `http_status_as_error(false)` and
GitHub's JSON error bodies. Keep Python's `token` auth header and `v3+json` accept header.
Do not send `X-GitHub-Api-Version`. Upload with an explicit `Content-Length`; the token is
not forwarded on cross-host redirects.

Consequences: No OpenSSL and no platform verifier. GitHub errors can be mapped like
Python's. A corporate TLS-inspecting proxy needs extra roots. Live GitHub behavior still
needs a token-backed smoke test before cutover.

## ADR-019: GitHub streaming keeps the 200 MiB part contract

Status: accepted (2026-11-24)

Context: The archive must stream to GitHub without holding a full copy, and existing
backups use `NAME.partNNNN`.

Decision: Implement `PartWriter` as a `Write` sink over `tar`/compressor streams. Keep the
200 MiB limit, four-digit part names, delete-after-upload, single-part rename, whole-stream
SHA-256, and release deletion on abort.

Consequences: Restore can reassemble old and new backups with the same regex. A failed
upload aborts the release. No resumable upload exists; retries restart a part.

## ADR-020: One page of releases, and rclone stays a subprocess

Status: accepted (2026-11-24)

Context: Python reads one page of 100 releases and shells out to `rclone`. Pagination and a
native Drive client would be new behavior.

Decision: Keep one `per_page=100` release page for parity. Keep rclone as a subprocess with
the `termux-sync-gdrive` remote and the same `copy`/`cat`/`lsjson`/`purge` calls.

Consequences: More than 100 backups on GitHub are not listed until the user deletes some.
rclone must be installed for Drive. Rclone progress is emitted per completed copy, not
parsed from stderr.

## ADR-021: The boot script starts the Rust binary

Status: accepted (2026-11-24)

Context: Python writes `~/.termux/boot/termux-sync-daemon.sh` with
`python3 "<script>" daemon &`. The Rust app must run after Python is retired.

Decision: `schedule` writes the boot script with the current executable path followed by
`daemon &`, mode 0755. Shebang stays `/data/data/com.termux/files/usr/bin/sh`.

Consequences: Boot starts the same binary the user ran. An upgrade at a different path
needs `termux-sync schedule` re-run or a stable install path.

## ADR-022: Setup and schedule use plain line prompts

Status: accepted (2026-11-24)

Context: Rich `Prompt`/`Confirm` are interactive and non-TTY behavior matters.

Decision: Setup, schedule, delete, restore and cache clearing read plain lines. EOF is a
`UserError: Input was closed`, matching the original's EOF path. Choices re-ask on invalid
input; defaults are shown in brackets.

Consequences: Piped answers work. Styling differs from Rich (ADR-003). Password input is
not hidden when stdin is a terminal yet; fix at lift or accept the difference.

## ADR-023: App replay keeps only deterministic, non-interactive cases

Status: accepted (2026-11-24)

Context: Rich tables, prompts, tracebacks and absolute temp paths make most recorded CLI
cases fail exact replay even after spacing/box normalization. The core's 28 unit cases
and the Rust smoke tests already cover the logic. Keeping unreadable snapshots would block
the lift without proving more.

Decision: `app-cases` keeps 10 deterministic cases (version, help, no args, unknown
command, invalid usage, list, and other non-interactive paths). Interactive, error-panel,
backup, restore, setup, schedule, export/import and path-dependent cases stay in
`app-cases-excluded/` for reference. The unit gate plus smoke tests prove those
behaviors.

Consequences: `app replay` proves wiring and exit codes, not full human text. A behavior
change in an excluded command is caught only by unit/smoke tests. Revisit the excluded set
before release if more replay coverage is wanted.

## ADR-024: The TUI reads through the core but keeps the CLI for mutations

Status: accepted (2026-11-24)

Context: ADR-009 planned direct core calls at lift. Direct backup/restore would need a
global event sink, suppressed human output and a safe cancellation path; the CLI already
provides all three through its event stream and process signals.

Decision: The TUI reads config, backups and the app version through `termux-sync-core`
directly. Backup, restore and destructive commands still run the Rust CLI as a child and
consume `--json-events`, preserving cancel and retry behavior. `--json-events` remains the
TUI contract.

Consequences: One less subprocess for read-only data. Mutations keep the proven event and
cancel path. ADR-009's "direct for everything" is superseded for mutations.

## ADR-025: Version 2.0.0 and native install at cutover

Status: accepted (2026-11-24)

Context: ADR-007 and ADR-010 set the version bump and native packaging for the lift.

Decision: Report `2.0.0` from all Rust crates. `install.sh` builds release binaries
and installs `termux-sync` and `termux-sync-tui`. `cutover.sh` backs up the Python
command, installs the Rust binaries and documents rollback.

Consequences: `--version` and help show 2.0.0. The root `install.sh` builds and
installs both binaries; `cutover.sh` uses the root scripts.

## ADR-026: Post-lift hardening checks

Status: accepted (2026-11-25)

Context: The first lift left three practical gaps: CLI commands were covered only by unit
and smoke tests, GitHub storage had no live call, and the root installer still described
Python.

Decision: Add `app/tests/cli_integration.rs` for backup/list/restore/delete, status,
logs, check, cache clearing, export/import, setup/schedule, version and parse errors.
Add `smoke-github.sh` for a token-backed live backup/list/restore/delete smoke. Make
root `install.sh` build both binaries directly and rewrite `tsctl` for the Rust binaries.

Consequences: The excluded app cases are backed by binary-level integration tests. GitHub
has a repeatable live smoke; it was run on a temporary private repository and the
repository was deleted. `tsctl` no longer downloads or installs Python.

## ADR-027: The TUI is a library with a `tui` CLI command

Status: accepted (2026-11-25)

Context: The TUI shipped only as `termux-sync-tui`. Users want one command,
`termux-sync tui`, and the dashboard still needs its standalone entry for scripts.

Decision: Move TUI startup into `termux-sync-tui`'s library (`main_entry`), add `tui` to
the CLI, and keep `termux-sync-tui` as a thin wrapper over the same library. Arguments
after `tui` pass through (`--ascii`, `--no-anim`, `--help`, `--version`).

Consequences: The CLI binary grows by the TUI dependency. Both entry points share one
implementation. Help text and integration tests cover `tui --version`, `tui --help` and
the non-TTY refusal.

## ADR-028: One crate for the CLI, core, and TUI

Status: accepted (2026-11-26)

Context: The repository shipped three crates: a core library, a CLI, and a TUI.
Publishing them separately would require three crates.io releases, versioned path
dependencies, and a coordinated publish order. Users install the two commands
together and never depend on the library directly.

Decision: Merge `app`, `core`, and `tui` into one `termux-sync` package. Both
binaries live in the same package. The library remains public because Cargo
binaries cannot access private library items, but it is documented as an internal
implementation detail with no stability promise.

Consequences: One `cargo install termux-sync` installs both commands. One
`Cargo.toml` holds the version and publish metadata. The package builds and
verifies as a single unit.
