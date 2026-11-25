# Restore Cockpit — Design

Date: 2026-11-21
Status: Approved for implementation
Branch: `spike/rust-tui`
Depends on: `docs/specs/2026-11-21-operations-cockpit-design.md`

## Context

The operations cockpit spec covers backup only. Restore remains a terminal
handoff today: the TUI disappears, Rich renders the progress, and the user has
no device-aware preview before files are overwritten.

This spec adds a restore cockpit with a two-phase flow: prepare a plan, show a
package diff and type-to-confirm gate, then stream download, verification,
extraction, and package reinstall. Cancellation is allowed only before
extraction starts.

## Goals

- Show a restore plan before any destructive action.
- Compare the backup package list with the current device.
- Require exact typed confirmation.
- Stream restore stages and progress into the TUI.
- Allow cancel during metadata, download, and verification only.
- Block extraction when checksums fail.
- Report partial failures honestly as `COMPLETED WITH ERRORS`.
- Return a completion checklist and require a manual Termux restart.
- Preserve existing CLI behavior when `--json-events` is absent.

## Non-goals

- File-level or archive-content diff.
- Package uninstall or pruning of device-only packages.
- Automatic Termux restart or shell replacement.
- Cancelling extraction, package reinstall, or cleanup.
- Restoring a backup that fails checksum verification.
- Changing backup formats or manifest schema.

## Flow

1. User selects a backup and chooses Restore.
2. TUI starts `termux-sync restore --name <name> --json-events`.
3. Python fetches only the manifest when the backend supports it.
4. Python emits `plan` and chunked `package_diff` events.
5. TUI shows the plan, package counts, and diff access.
6. User types exactly `RESTORE` on the plan screen.
7. TUI writes `{"confirm":"RESTORE"}` to the child stdin.
8. Python downloads archives if needed, verifies every checksum, and aborts
   before extraction on any failure.
9. Python extracts archives, reinstalls packages, cleans temporary files, and
   emits `result`.
10. TUI shows the completion checklist and tells the user to close and reopen
    Termux.

Declining, pressing Esc, EOF, or malformed confirmation input cleans temporary
files and emits `cancelled` with reason `declined`.

## Plan event

Plan is emitted after the manifest is read and before the confirmation gate.
Checksums are not verified yet; the plan says so explicitly.

```json
{
  "v": 1,
  "event": "plan",
  "ts": 0,
  "name": "termux_backup_20261121_011733",
  "date": "2026-11-21T01:17:33",
  "label": "before-upgrade",
  "storage": "local",
  "versions": {"backup": "1.3.9", "current": "1.3.9"},
  "packages": 1947,
  "to_install": 120,
  "already_installed": 1827,
  "device_only": 210,
  "diff_available": true,
  "archives": ["home.tar.gz", "usr_etc.tar.gz"],
  "size": 22300000000,
  "checksums": "pending"
}
```

- `checksums` is `pending`, `verified`, or `failed`. The restore cockpit never
  starts with `failed`.
- Verification always runs after confirmation and before extraction, even for
  local backups. The plan does not delay the preview by hashing 22 GB first.
- `diff_available` is false when the current package list cannot be read.
  In that case `to_install`, `already_installed`, and `device_only` are
  omitted, and the diff view shows `unavailable`.
- Counts are authoritative; package name lists are best-effort chunks.

## Package diff

### Categories

- `to_install`: in the backup, not installed now.
- `already_installed`: in both.
- `device_only`: installed now, absent from the backup. Kept; restore never
  uninstalls.

The diff is informational. Restore reinstalls **every** package listed in the
backup, including already-installed ones. Device-only packages remain
installed and are not removed.

### Warning

The Package diff screen has a persistent, non-dismissible warning at the top:

```text
! Restore reinstalls all 1947 packages from this backup.
  1827 already-installed packages are reinstalled too.
  210 device-only packages stay installed and are not removed.
```

- Warning uses an `!` marker and warning color, surviving `NO_COLOR`.
- Warning always names every count available. With a working diff it names
  all three; when the diff is unavailable it names the backup total and says
  the current package list could not be read.
- Warning is not part of a dismissible help overlay.
- The plan screen shows a one-line version of the same warning before the
  typed confirmation field:
  `Reinstalls all 1947 packages; 1827 already installed.`

### Diff view

- `d` opens Package diff from the plan screen.
- Three sections with counts: `TO INSTALL`, `ALREADY INSTALLED`, `DEVICE ONLY`.
- `/` filters by substring within the current section.
- `↑↓` scroll, `1`/`2`/`3` jump sections, `G` end, `Esc` return to plan.
- Mouse may select sections and rows; filtering stays keyboard-only.
- The typed confirmation lives only on the plan screen; the diff view cannot
  confirm a restore.
- At 48 columns the warning wraps; list rows reduce, not disappear.
- If chunks are missing or partial, the view shows `partial diff` and still
  renders received packages.

### Diff event chunks

Lists are split so no JSON line exceeds 64 KiB:

```json
{"v":1,"event":"package_diff","ts":0,"category":"to_install","part":1,"parts":2,"packages":["pkg-a","pkg-b"]}
```

- Categories: `to_install`, `already_installed`, `device_only`.
- At most 500 package names per chunk.
- `part` is 1-based.
- Unknown categories are ignored.
- If `dpkg --get-selections` fails, Python emits no chunks and
  `diff_available: false`.

## Confirmation gate

After `plan` and diff chunks, Python blocks on stdin for one JSON line:

```json
{"confirm":"RESTORE"}
{"confirm":"CANCEL"}
```

- `RESTORE` is case-sensitive after trimming surrounding whitespace.
- `RESTORE` is rejected with `error` if `checksums` is `failed`.
- `CANCEL`, EOF, or malformed input cleans temp files and emits `cancelled`.
- TUI sends nothing until the typed input exactly matches `RESTORE`.
- Mouse and Enter alone cannot confirm.

## Restore stages

| id | label | weight |
| --- | --- | ---: |
| `metadata` | Metadata | 2 |
| `download` | Download | 25 |
| `verify` | Verify | 15 |
| `extract:home` | Home | 20 |
| `extract:usr_etc` | Config | 5 |
| `packages` | Packages | 28 |
| `cleanup` | Cleanup | 5 |

- Skipped stages are removed and remaining weights normalize to 100.
- `download` is skipped for local storage when archives are already present.
- `progress` events use `unit` = `bytes`, `items`, or `packages`.
- Extract progress uses existing per-file callbacks.
- Package progress emits `current/total`.
- Google Drive download emits byte progress through the existing rclone path
  where available; otherwise the stage is indeterminate.
- The plan gate has no weight and no gauge movement.

## Cancel policy

Allowed during:

- `metadata`
- `download`
- `verify`
- waiting at the plan confirmation gate

Refused after the first `extract:*` stage starts, including `packages` and
`cleanup`.

- `Esc`, `c`, `q`, and Ctrl+C during the protected phase show
  `Restore in progress — cannot cancel safely`.
- No signal is sent to the child in the protected phase.
- Footer changes to `wait for completion · ↑↓ scroll logs`.
- Quitting is refused during the protected phase.
- Closing the terminal externally is outside the TUI contract.
- Before extraction, cancel uses the existing process-group SIGINT path,
  cleans temp files, and emits `cancelled` reason `user`.

Ctrl+C remains an immediate quit during backup and on the dashboard. During a
protected restore it is refused; this is the one exception to the global
Ctrl+C rule.

## Python instrumentation

- `metadata`: list backups and fetch only `manifest.json` when the backend
  supports a manifest fetch. Fall back to the existing download-then-read path
  when it does not; emit `log` warning about the fallback. When the fallback
  downloads archives, emit the `download` stage before `plan`; do not download
  again after confirmation.
- `package_diff`: compare manifest packages with `get_installed_packages()`.
- `download`: existing `storage.download`.
- `verify`: existing checksum loop. Emit per-archive status. Any failure emits
  `error` and stops before extraction.
- `extract:home`, `extract:usr_etc`: existing `extract_archive` with progress.
- `packages`: existing `install_packages`, with per-package progress and
  collected failures.
- `cleanup`: remove temp directory.
- `result`: success or completed-with-warnings.

Restore must not prompt through Rich in `--json-events` mode. All confirmations
come from the stdin JSON line.

## Rust architecture

`tui/src/ops.rs` grows a restore operation:

- `OperationKind::Backup | Restore`.
- Restore event parsing: `plan`, `package_diff`, `result` warnings.
- `RestorePhase`: `Preparing`, `Plan`, `WaitingForConfirmation`, `Running`,
  `Protected`, `Succeeded`, `Warnings`, `Failed`, `Cancelled`.
- `PackageDiff` with three ordered sets and chunk reassembly.
- `confirm_restore()` writes the JSON line and flushes.
- `cancel_allowed()` returns false in the protected phase.
- Reuses the child process group, SIGINT cancel, and SIGKILL timeout.

App changes:

- Restore action starts an operation instead of a handoff.
- Plan state owns the typed input buffer and package diff view state.
- `d` opens the diff; `/` filter; section jumps.
- `q` and Ctrl+C consult `cancel_allowed`.
- On `result` with warnings, refresh the snapshot but keep the result screen.
- On clean success, refresh the snapshot and keep the completion checklist.

## UI

Operation screen only; the dashboard is not visible during restore.

Header:

```text
RESTORE  termux_backup_20261121_011733  LOCAL  02:14
```

Plan screen at 80+ columns:

```text
RESTORE PLAN
Date        2026-11-21 01:17
Label       before-upgrade
Size        22.3 GB
Packages    1947 total · 120 to install · 1827 already installed
Device-only 210 packages stay installed
Checksums   will verify before extraction

! Reinstalls all 1947 packages; 1827 already installed.
Type RESTORE to continue: _
```

At 48–79 columns, fields stack; warning stays visible; typed field remains.

Running screen:

- Stage stepper at 80+ columns; current stage plus `3/7` below 80.
- Overall gauge.
- Current stage gauge when progress exists; otherwise indeterminate.
- Live log, last 100 lines, autoscroll paused by manual scroll; `G` resumes.
- Cancel hint only in cancellable phases.

Protected screen:

- Same as running, with amber `CANNOT CANCEL SAFELY` label.
- Footer: `wait for completion · ↑↓ scroll logs`.
- No cancel key hint.

Result screen:

- `SUCCESS` or `COMPLETED WITH ERRORS`.
- Checklist: archives extracted, packages reinstalled, package failures,
  archive failures.
- `Close and reopen Termux to use the restored environment.`
- `Esc` returns to the dashboard.
- Failure: message, hint, failed stage; `r` re-plans, `Esc` returns.
- Cancelled: `Cancelled. Temporary files removed.`; `Esc` returns.

Reduced motion: no sweep movement; text and gauge updates remain.

## Failure handling

- Manifest missing or invalid: `error` before plan; no extraction.
- Checksum failure after confirmation: `error`, cleanup, no extraction.
- Unsafe archive name or unknown manifest key: refused per existing checks;
  warning event, archive marked failed, remaining safe archives continue.
- Archive extraction failure: continue remaining archives where safe; result
  has warnings and the failed archive list.
- Package install failure: continue remaining packages, collect failures,
  result has warnings.
- Child exits without result: synthesize failure with exit code and stderr
  tail.
- Exit 130 before extraction: cancelled.
- Exit 130 during protected phase: failed with partial-restore warning.
- Event stream malformed, oversized, or non-UTF-8: same handling as backup.
- Panic during extraction: terminal restores; the TUI does not intentionally
  kill the child. Documented risk.

## Testing

Python:

- Plan event fields for local and mocked remote manifest fetch.
- Package diff set algebra and 500-name chunking.
- Missing `dpkg` sets `diff_available: false`.
- Confirmation gate: `RESTORE`, `CANCEL`, EOF, malformed input.
- Checksum failure rejects `RESTORE` and never extracts.
- Cancel before extraction cleans temp; cancel after extraction is not sent.
- Package partial failure produces warnings in `result`.
- Redaction of tokens and passwords.

Rust:

- Parser for `plan`, `package_diff`, `result` warnings, unknown category.
- Chunk reassembly, partial chunks, out-of-order chunks.
- Set algebra view state, section jumps, filter.
- Typed input exact match; whitespace trim; case sensitivity.
- Cancel allowed only before protected phase; Ctrl+C and `q` refused after.
- Render tests at 100×30, 80×24, 60×20, 48×12 for:
  plan, diff with warning, running, protected, warnings, failed, cancelled.
- PTY with a fake restore script:
  plan renders, `d` opens diff, `/` filters, typed RESTORE writes the JSON
  line, protected Ctrl+C is refused, result returns to dashboard.

## Acceptance criteria

- CLI restore without `--json-events` is unchanged.
- Plan appears before any extraction.
- Warning naming all available package counts is always visible on the diff
  screen.
- Exact `RESTORE` is required; Enter, mouse, and partial input cannot confirm.
- Checksum failure prevents extraction.
- Cancel works before extraction and is refused after it starts.
- Partial failures are `COMPLETED WITH ERRORS`, never clean success.
- Completion tells the user to restart Termux manually.
- No secret appears in events, logs, or TUI state.
- Reduced motion disables moving indicators.
- All Python and Rust tests pass.

## Rollout

- Additive `--json-events` behavior; no CLI migration.
- Both Python copies updated together.
- TUI remains on `spike/rust-tui` until acceptance.
- No PyPI or `tsctl` release change in this milestone.
