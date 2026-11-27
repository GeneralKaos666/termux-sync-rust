# TUI Runbook

Operational recovery notes for `termux-sync-tui`.

## Event protocol

- The TUI runs `termux-sync backup|restore|list --json-events`.
- Events are JSON Lines on stdout; human Rich output goes to stderr.
- Protocol version is `1`. A newer major version is refused with an update message.
- No GitHub token or encryption password is emitted. Event strings pass through
  the redactor in `EventWriter`.

## Old CLI installs

At start of an operation the TUI probes the installed `termux-sync`:

- missing `--json-events`: backup and restore refuse before starting; `L`
  falls back to captured text output.
- missing `--yes`: delete, clear-logs and clear-cache fall back to the
  terminal handoff and the CLI asks its own confirmation.

Update `termux-sync` before using the cockpit if backup or restore refuses.

## Ctrl+C

On the dashboard, Ctrl+C quits. During a backup it cancels through the same
SIGINT cleanup path. During a protected restore it is refused with a message.
Use `q` after an operation completes to leave the TUI.

## Backup cancel

`Esc` / `c` on a running backup opens a confirmation card. Enter sends SIGINT
to the child process group; the cleanup path in the core library removes
temporary files and incomplete stream parts. The TUI records `cancelled`.

If the process does not exit within 10 seconds, the TUI sends SIGKILL and says
cleanup may be incomplete. Temporary files can remain under `$PREFIX/tmp` or
`$TMPDIR`; search for `termux_backup_*` and remove only paths you recognize.

## Restore cancel

Cancellation is allowed during metadata, download, verification, and while the
plan is open.

Once the first `extract:*` stage starts, cancel and quit are refused.
`termux-sync` also ignores SIGINT during extraction. The footer shows
`CANNOT CANCEL SAFELY`.
Wait for completion or failure.

If the TUI process dies during extraction:

- The terminal restore hook runs for normal exits and panics.
- The child may continue restoring; do not start a second restore.
- After restarting Termux, inspect the log at `~/.config/termux-sync/sync.log`
  and re-run the restore if necessary.

Closing the terminal externally is outside the guarantee.

## Verification states

- `VERIFIED`: stored local archives were re-read and hashed, or the GitHub
  stream digest matched.
- `CORRUPT`: local checksum mismatch. Backups are not deleted automatically.
- `SKIPPED`: the backend cannot prove integrity (for example Google Drive).
  Treat as unverified, not as failed.

Restore refuses to extract when any archive checksum fails.

## Clear logs

- `Clear logs` asks once in the TUI, then runs
  `termux-sync clear-logs --yes` captured.
- An older CLI without `--yes` hands the terminal to `termux-sync clear-logs`
  and the CLI asks its own question.
- With no log file, the CLI reports that there is nothing to clear. The TUI
  does not create an empty log file.

## Status

- `s` opens the read-only status view.
- It shows the storage target, compression and backup limit, schedule, TUI and
  CLI versions, config/log/history paths, and `dpkg` plus `rclone` for Google
  Drive.
- `CLI not found` means `TERMUX_SYNC_BIN` or `termux-sync` could not be run.
- `rclone` is required only for Google Drive.

## Daemon

- The daemon view shows schedule state, boot script presence, and recent
  daemon lines from `sync.log`.
- An enabled schedule is not proof that a daemon is running. Check manually:
  `pgrep -f "termux-sync daemon"`.
- Start it with `termux-sync daemon &`, or install Termux:Boot and keep
  `~/.termux/boot/termux-sync-daemon.sh`.
- The TUI never starts or stops the daemon.

## Layout recovery

Minimum layout is `48×12`. Below that the TUI shows the required dimensions.
At `< 80` columns it shows one pane; `Tab` switches focus.

If icons render as boxes or wide gaps, use `--ascii` or
`TERMUX_SYNC_TUI_ASCII=1`.

## Files

- Config: `~/.config/termux-sync/config.json`
- Log: `~/.config/termux-sync/sync.log`
- Schedule: `~/.config/termux-sync/cron.json`
- TUI history: `~/.config/termux-sync/tui-history.json`

Config stores the GitHub token in plaintext. The TUI never reads it into
state or events, but the file is readable by anything with your Termux user.

## Restore completion

The TUI never restarts Termux. Close and reopen Termux after a successful
restore so packages, shell files, and environment changes take effect.
