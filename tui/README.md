# termux-sync-tui (spike)

Ratatui dashboard around the existing `termux-sync` CLI. This is a throwaway
spike on branch `spike/rust-tui`, not part of the released Python package.

It never reimplements backup logic. Read-only views come from
`~/.config/termux-sync/config.json`, local `manifest.json` files, and
`sync.log`. Backup, restore, list, delete and clear-cache run through captured output or
the Python event protocol; setup and schedule hand the terminal to
`termux-sync`.

## What it shows

- Hero header: gradient `TERMUX SYNC` mark, tagline, storage and backup-count
  chips, plus compression and max-backup settings on wide terminals.
- Stat cards: backups saved, total stored size, latest package count and
  schedule. Cards appear at 80+ columns and 20+ rows.
- Actions pane: icon for every action, plus backup, restore, delete, list,
  logs, clear logs, disk check, status, export config, import config, daemon,
  setup, schedule, refresh, help, quit.
- Status: `s` or the Status action shows storage target, compression and
  backup limit, schedule, TUI and CLI versions, config/log/history paths, and
  required tool checks. Tokens and passwords are never read.
- Daemon: shows schedule state, boot script presence, recent daemon log lines,
  and the manual start command. It does not start or stop the daemon.
- Config: export and import prompts take a path and run the CLI. Export
  redaction and import validation stay in Python.
- Disk check: blank uses the CLI default overview; a path checks that target.
- Clear logs: confirms in the TUI, then runs `termux-sync clear-logs --yes`.
  An older CLI falls back to the terminal confirmation.
- Backups pane: short backup name, age, size, relative usage bar and package
  count, with the selected backup's label, version and compression in the
  pane title. `/` filters by name, label or date.
- Remote list: `L` runs `termux-sync list --json-events` and replaces the
  table for local, GitHub and Drive backups.
- Backup label prompt: `Enter` on Backup now opens an optional label input.
- History: `h` shows the last 20 operations; `r` retries the selected entry.
- Output pane: scrollable log tail and captured command output, with dimmed
  dates and INFO / WARNING / ERROR chips.
- Operations cockpit: backup streams stages, progress and logs inside the TUI.
  Restore first shows a plan, package diff and type-to-confirm gate, then
  streams download, verification, extraction and package reinstall.
- Restore safety: checksums must pass before extraction. Cancel is allowed
  only during metadata, download and verification. After extraction starts,
  cancel is refused and the screen says so. Cancel asks once: Enter confirms,
  Esc dismisses.
- Package diff: `d` from the restore plan opens `TO INSTALL`,
  `ALREADY INSTALLED` and `DEVICE ONLY` sections with a persistent warning
  that every backup package is reinstalled.
- Motion: help text reveals at a fixed size in about 200ms. Captured commands
  show a WORKING card only when they run longer than 150ms, hold DONE for
  150ms, then reveal output. Pane changes and output updates are instant.
- `--no-anim` or `TERMUX_SYNC_TUI_REDUCED_MOTION=1` removes reveal motion while
  keeping status and color feedback.
- Startup draws immediately and loads backups in a worker; the header shows
  `LOADING` until the first snapshot arrives.
- Idle app does not redraw on a timer. Frames are produced on input, worker
  results, resize, and active animations only. Ctrl+C always quits.

## Build and run

From the repository root:

```bash
bash rust/install.sh
termux-sync tui
```

The standalone `termux-sync-tui` binary is kept as an alias for the same
library. In a development checkout:

```bash
cargo run --manifest-path rust/app/Cargo.toml -- tui
```

`install.sh` accepts `--prefix PATH` and `--skip-build`. It copies the release
binary into `$PREFIX/bin`.

Requires a terminal on stdout. Non-TTY runs exit with status 2 instead of
drawing garbage.

It also requires a `termux-sync` build with `--json-events`. If the installed
CLI is older, the TUI refuses to start an operation and tells you to update it.

Use another binary with:

```bash
TERMUX_SYNC_BIN=/path/to/termux-sync ./target/release/termux-sync-tui
```

`--help` and `--version` work without a terminal.

Disable motion with `--no-anim` or `TERMUX_SYNC_TUI_REDUCED_MOTION=1`.
Use ASCII icons with `--ascii` or `TERMUX_SYNC_TUI_ASCII=1`.

## Keys

| Key | Action |
|---|---|
| `↑` / `↓` or `k` / `j` | move selection |
| `Enter` | run action; restore selected backup from Backups pane |
| `d` | delete selected backup from Backups pane |
| `Tab` / `Shift-Tab` | switch pane |
| `R` | refresh backups and status |
| `L` | list backups, including remote storage |
| `s` | status |
| `/` | filter backups in the Backups pane |
| `h` | operation history |
| `g` / `G` | top / bottom of output |
| `[` / `]` | Output pane: fewer / more log lines (40–2000) |
| any printable | type a backup label or path when prompted |
| `?` | help |
| `↑` / `↓` in help | scroll help at small sizes |
| `Ctrl+C` | quit dashboard; cancel a running backup; refused during protected restore |
| `q` | quit |

Operation keys:

| Key | Action |
|---|---|
| `Esc` / `c` | cancel while allowed; asks once, or explains why not |
| `d` | open package diff on the restore plan |
| `1` / `2` / `3` | jump diff sections |
| `/` | filter the current diff section |
| `r` | retry a failed operation or history entry |
| `↑` / `↓`, `PgUp` / `PgDn`, `G` | scroll logs or diff |
| type `RESTORE`, then `Enter` | confirm a restore |

Mouse: click an action to run it, click a backup row to select it, click a
pane to focus it, and scroll the wheel over lists and logs. Backup-label,
delete, clear-cache and handoff dialogs have clickable confirm/cancel areas.
Operation cancel is keyboard-only. Mouse capture disables terminal text
selection; hold Shift to select text in most terminals.

## Layout

- `>= 80` columns and 20+ rows: hero, stat cards, actions, backups and output.
- `>= 80` columns below 20 rows: hero drops the stat cards.
- `48–79` columns: hero plus one pane at a time; `Tab` switches.
- `< 48×12`: "terminal too small" message naming the minimum.
- `NO_COLOR=1`: same layout, gradient, chips and focus markers without color.

## Safety notes

- The config parser keeps only display fields. Tokens and passwords in
  `config.json` are never read into the TUI state. Status shows repo and
  branch, never the token.
- Status, daemon, backup list and logs are read-only. The daemon view never
  starts or stops the process.
- Backup, restore, delete, clear-logs and clear-cache run the real
  `termux-sync` command. Delete, clear-logs and clear-cache confirm inside the
  TUI, then run captured with `--yes`; restore uses the event protocol.
- Export config, import config and disk check run the CLI captured. Export
  paths are passed as one argument, never through a shell.
- Setup and schedule use the explicit handoff card.

## Runbook

Recovery steps for protected restore, cancel timeout, panics, verification
states, and old CLI installs: [RUNBOOK.md](RUNBOOK.md).

## Spike limits

- Restore cannot be cancelled after extraction starts.
- Setup and schedule still hand over the terminal after a confirmation card.
- Delete, clear-logs and clear-cache run captured after an in-TUI confirmation
  using `termux-sync --yes`; older CLI falls back to the terminal prompt.
- The operations protocol is version 1 and backup/restore only.
