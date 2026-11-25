# Operations Cockpit — Design

Date: 2026-11-21
Status: Approved for planning
Branch: `spike/rust-tui`

## Context

The Rust TUI currently wraps the `termux-sync` CLI by handing the terminal to
the child process for backup, restore, and other interactive commands. The
dashboard is useful, but the primary backup operation still looks like the old
CLI: the TUI disappears, progress is rendered by Rich, and the dashboard cannot
show live stages, precise progress, or a trustworthy result.

This spec adds a backup-first operations cockpit: the TUI runs the backup as a
child process, consumes a machine-readable event stream, renders live stages
and progress, and can cancel a backup with cleanup.

## Goals

- Stream backup progress from the Python backup engine into the TUI.
- Show named stages, an overall gauge, a stage gauge when byte totals exist,
  and a live log.
- Cancel a running backup, allow the existing Python cleanup path to run, and
  return to a clean dashboard state.
- Return to the dashboard on success with the new backup selected.
- Keep failures actionable: message, hint, retry, return.
- Add honest verification status for the storage backends.
- Preserve all existing CLI behavior when `--json-events` is absent.
- Never emit tokens, passwords, or secrets in events.

## Non-goals

- Streaming restore in this milestone. Restore keeps the terminal handoff.
- Detached or background backups after quitting the TUI.
- Remote verification the backend cannot actually perform.
- Reimplementing backup logic in Rust.
- Changing backup formats or manifest schema.

## User flows

### Successful backup

1. User selects `Backup now` and presses Enter or clicks it.
2. TUI switches to the Operation screen and starts
   `termux-sync backup --json-events`.
3. Stages advance: preflight, packages, home archive, config archive,
   manifest, upload, verify.
4. Progress events update the stage gauge and overall gauge.
5. Log events append to the live log.
6. A `result` event returns success, name, size, location, verification state.
7. TUI refreshes the snapshot, selects the new backup, and shows a result
   card. `Esc` returns to the dashboard.

### Cancelled backup

1. User presses `Esc` or `c` while running.
2. Confirmation appears: "Cancel backup and clean partial files?"
3. Confirm sends SIGINT to the child process group.
4. Operation state becomes `Cancelling`.
5. Python's existing `KeyboardInterrupt` path removes temporary files and any
   incomplete stream. It emits `cancelled`, then exits 130.
6. TUI shows `Cancelled` and returns to the dashboard on `Esc`.
7. If the child does not exit within 10 seconds, TUI sends SIGKILL and shows
   that cleanup may be incomplete.

Quitting with `q` during a backup uses the same confirmation and cancel path.
There must be no orphaned backup process.

### Failed backup

1. `error` event or a non-zero exit without a result sets `Failed`.
2. Operation screen keeps the error message, hint, and exit code.
3. `r` retries the same backup command from scratch.
4. `Esc` returns to the dashboard and keeps the failure in status.

### Verification

- Local storage: hash stored archives, compare with manifest checksums.
  Result is `VERIFIED` or `CORRUPT`.
- GitHub streaming: use the digest calculated during streaming upload.
- Google Drive: `SKIPPED` with a reason unless a future backend check exists.
- Verification failure does not delete the backup.
- The result event includes `verified: true | false | null` and a reason.

## Python event mode

### CLI

```
termux-sync backup --json-events
```

- `--json-events` is additive and applies to backup in this milestone.
- When enabled, JSON Lines are written to **stdout**.
- Rich/human output goes to **stderr**, so existing `console.print` and
  `Progress` calls need no wholesale rewrite.
- The child stdout is UTF-8 JSON Lines. Each line is flushed immediately.
- Without `--json-events`, behavior and output are unchanged.
- `termux-sync.py` and `src/termux_sync/__init__.py` are identical today and
  must be updated in lockstep.

### Event envelope

Every event is one JSON object per line:

```json
{"v":1,"event":"stage","ts":1763770000123,"id":"packages","label":"Collecting packages","state":"start","index":1,"total":7}
```

- `v`: schema version, currently `1`.
- `event`: event type.
- `ts`: Unix milliseconds.
- Unknown fields must be ignored by the TUI.

### Events

`start`

```json
{"v":1,"event":"start","ts":0,"command":"backup","name":"termux_backup_20261121_120000","storage":"local","version":"1.3.9"}
```

`stage`

```json
{"v":1,"event":"stage","ts":0,"id":"archive:home","label":"Home directory","state":"start","index":3,"total":7}
```

`state` is one of `start`, `complete`, `failed`, `skipped`.

`progress`

```json
{"v":1,"event":"progress","ts":0,"stage":"archive:home","current":12582912,"total":23907532800,"unit":"bytes","detail":"home/.cache"}
```

`current` and `total` are optional when a stage cannot measure progress.
`unit` is `bytes`, `items`, or `parts`. `detail` is optional and must be
redacted.

`log`

```json
{"v":1,"event":"log","ts":0,"level":"INFO","message":"Collected 1947 packages"}
```

`level` is one of `DEBUG`, `INFO`, `WARNING`, `ERROR`.

`verify`

```json
{"v":1,"event":"verify","ts":0,"state":"verified","details":"local checksums match"}
```

`state` is `verified`, `failed`, or `skipped`.

`result`

```json
{"v":1,"event":"result","ts":0,"ok":true,"name":"termux_backup_20261121_120000","size":22300000000,"location":"/sdcard/termux-backups/termux_backup_20261121_120000","verified":true,"duration_ms":812345}
```

`error`

```json
{"v":1,"event":"error","ts":0,"message":"Upload failed","hint":"Check the GitHub token","exit_code":1}
```

`cancelled`

```json
{"v":1,"event":"cancelled","ts":0,"reason":"user"}
```

### Stage ids and weights

| id | label | weight |
| --- | --- | ---: |
| `preflight` | Preflight | 2 |
| `packages` | Packages | 5 |
| `archive:home` | Home | 50 |
| `archive:usr_etc` | Config | 13 |
| `manifest` | Manifest | 5 |
| `upload` | Upload | 20 |
| `verify` | Verify | 5 |

- If a stage is skipped or absent, remaining weights are normalized to 100.
- Progress within a stage scales that stage's weight.
- A stage without a total contributes 0 until it completes, then its full
  weight. The TUI shows an indeterminate sweep for that stage.

### Instrumentation points

- `preflight`: before storage validation.
- `packages`: around package collection.
- `archive:home`, `archive:usr_etc`: around each archive in `cmd_backup`.
  Existing `create_archive` progress callbacks emit `progress` events.
- `manifest`: around `write_manifest`.
- `upload`: around `storage.upload`. Local emits per-file progress. GitHub
  streaming emits part progress. Google Drive emits start/complete only;
  byte progress is omitted rather than guessed.
- `verify`: local hashing, GitHub stream digest, or Google Drive skip.
- `result`: after the success summary is known.
- `error`: where `print_error`/upload errors are handled.
- `cancelled`: from the existing `_abort` path, plus exit 130.

### Redaction

- `EventWriter` receives secret values at backup start:
  `github_token`, `encrypt_password`, and any future secret keys.
- Every event string is passed through a redactor replacing each non-empty
  secret with `***`.
- No config object is serialized wholesale.
- Error messages and hints are redacted before emission.
- The TUI never stores the raw config.

## Rust architecture

New module `tui/src/ops.rs`:

- `Event` enum parsed from JSONL.
- `OperationState`: name, storage, started time, stages, current stage,
  overall progress, log ring buffer, verification, result, failure, cancel
  state.
- `OperationStatus`: `Running`, `Cancelling`, `Succeeded`, `Failed`,
  `Cancelled`.
- `run_backup()` spawns the child with stdout piped and a reader thread that
  sends parsed events over `std::sync::mpsc`.
- `cancel()` sends SIGINT to the child process group.
- The reader thread reports EOF and parse errors. Malformed lines become
  warnings, not fatal errors.

Child process:

- Spawn with `CommandExt::process_group(0)` on Unix.
- Follow-ups controlled by `libc::kill(-pgid, SIGINT)`.
- `SIGKILL` fallback after a 10-second cancel timeout.
- Add `libc` as a Unix-only dependency.

App changes:

- `Screen::Dashboard | Screen::Operation`.
- Dashboard `Backup` action starts an operation instead of queueing a handoff.
- Operation keys: `Esc`/`c` cancel or leave, `r` retry, `g`/`G` log
  scroll, `?` help, `q` quit with cancel confirmation.
- `poll_operation()` replaces the normal worker poll while an operation is
  active.
- On success, call `refresh()` and select the new backup by name.

UI changes:

- Operation screen replaces the dashboard body while active.
- Header: name, storage, elapsed time.
- Stage stepper on one line at 80+ columns.
- At 48–79 columns: current stage plus `3/7`, no full stepper.
- Overall gauge plus stage gauge when totals exist.
- Live log, autoscroll by default. Manual scroll pauses follow; `G` resumes.
- Cancel confirmation modal: Enter confirms, `Esc` dismisses.
- Result card: name, size, location, verification, duration; `Esc` returns.
- Failure card: message, hint, exit code; `r` retry, `Esc` back.
- Reduced motion: no sweep movement; keep stage text and gauge updates.

## Error handling

- Malformed JSON line: append a warning log and continue.
- Line longer than 64 KiB: discard and warn; do not buffer indefinitely.
- Non-UTF-8 bytes: decode lossily.
- Stream ends without `result`, `error`, or `cancelled`: synthesize
  "termux-sync exited without a result" with the exit code.
- Exit 130 with no result: treat as cancelled.
- Child exit before `start`: show failure with the captured stderr tail.
- Cancel timeout: SIGKILL, show "process did not stop; killed" and the
  temporary path if Python reported one.
- Write errors on the child pipe: mark failed and surface the OS error.
- TUI panic while an operation runs: existing panic hook restores the
  terminal. The child process group is signalled best-effort before exit.

## Testing

Python:

- `tests/test_events.py` captures JSONL from `EventWriter`.
- Stage order for a backup with fake storage and mocked archiving.
- Redaction: a message containing the token emits `***`.
- Cancel path emits `cancelled` and exits 130.
- `--json-events` does not change Rich output when absent.
- No test performs a real 22 GB backup or network upload.

Rust:

- Parser tests: every event, unknown fields, malformed JSON, oversized lines,
  missing fields, EOF without result.
- State tests: weighted gauge, skipped-stage normalization, cancel transition,
  exit-code handling.
- Render tests at 100×30, 80×24, 60×20, 48×12 for running, cancelling,
  succeeded, failed, verified, and skipped states.
- Cancel argument test: process group signal construction.
- PTY smoke with a fake event emitter:
  - renders stages and logs,
  - cancel sends SIGINT and cleans state,
  - success returns to dashboard and selects the new backup.

## Acceptance criteria

- `termux-sync backup` without `--json-events` is byte-for-byte unchanged in
  behavior.
- `termux-sync backup --json-events` emits only JSON Lines on stdout and human
  output on stderr.
- TUI backup shows at least the seven stages, an overall gauge, and a live log.
- Normal cancel runs Python cleanup, emits `cancelled`, exits, and leaves no
  temporary backup. A hard-kill timeout reports that cleanup may be incomplete.
- Quit during backup cannot leave an orphaned child.
- Success refreshes the dashboard and selects the new backup.
- Failure shows message and hint; retry starts a fresh backup.
- No secret appears in any event, log line, or TUI state.
- Verification reports `VERIFIED`, `CORRUPT`, or `SKIPPED` honestly.
- Reduced motion disables moving indicators.
- All Python tests and Rust tests pass.

## Rollout

- Additive Python flag; no migration.
- Event schema version 1; unknown event types ignored for forward compatibility.
- Both Python copies updated together.
- TUI remains a spike branch until the cockpit passes acceptance; no PyPI or
  `tsctl` release change in this milestone.
