# Review Fixes Design

**Date:** 2026-11-25
**Scope:** Whole-repository review of `termux-sync-rust` at `86660ad`
**Status:** Approved for implementation planning

## Context

The 2026-11-24 review found five data-loss defects and a set of correctness,
validation, and reporting gaps across the core, CLI, TUI, docs, and CI. The
tests passed and clippy was clean, so every finding is a latent behavior bug,
not a build break.

This document records the decisions the plan depends on. The finding inventory
is the authoritative list; the plan references finding IDs.

## Invariants

1. A command reports success only when every stage it depends on succeeded.
2. Backups stay compatible with the existing format: same manifest keys, same
   archive names, same SHA-256 checksums. Older backups restore unchanged.
3. Every path, repository name, branch, and folder ID that comes from config or
   argv is validated before it reaches a filesystem or URL operation.
4. A destructive step either completes or leaves the previous state usable.
   Partial uploads and partial writes never look valid.
5. No secret reaches the log file, an event, or a world-readable file.
6. Malformed config, schedule, archive, or argv produces a clean error, never a
   panic.

## Decisions

### D1 — Setup persists what it asks

`setup()` saves the edited map, not the config it loaded. It starts from a
default config with a warning when the live file is unreadable, so setup can
repair a broken install. An empty token answer keeps the stored token, matching
the existing prompt text.

### D2 — Extraction refuses to replace a directory with a file

`check_member_root` rejects a member whose normalized path equals the expected
top component (`home`, `usr_etc`, `usr_lib`, `packages`). `create_regular`
returns an error when the target exists as a directory. It never calls
`remove_dir_all` for a file entry.

### D3 — Archive failure is fatal for every backend

The GitHub-only `return Err` branch is removed. Any archive error emits a
failed stage, logs, cleans the temporary directory, and returns `Err`, for
local, rclone, and GitHub alike.

### D4 — Restore reports extraction failure

When `failed_archives` is non-empty, restore emits `ok: false`, includes the
failed archive names in `warnings`, and returns `Err`. Package install failures
stay warnings because the files were restored.

### D5 — Retention only touches real backups

- `max_backups` is clamped to at least 1 at every retention call site, so `0`
  or a negative value cannot delete everything.
- Pruning runs only after verification has not failed.
- rclone deletes only entries whose name matches `termux_backup_*` and passes
  `manifest::valid_backup_name`. The Drive root is never purged wholesale.

### D6 — Local upload is atomic

Upload copies into `.<name>.upload-<pid>` inside the storage root, archives
first and `manifest.json` last, then renames the staging directory to the final
name. A failed upload leaves no directory that `list_backups` accepts.

### D7 — Unknown storage is an error

`Storage::from_config` returns `termux_sync.UserError: Unknown storage backend`
for any value outside `local`, `gdrive`, `github`. There is no silent fallback.

### D8 — Package collection is fallible

`installed_packages` returns `Result`. A `dpkg` spawn or exit failure fails the
backup with a clear error instead of writing an empty package list.

### D9 — GitHub API failures are visible

`list_releases` returns `Result`; callers propagate. `delete_old` propagates
release and tag deletion errors. `github_repo` must match `owner/name`.
Dynamic path segments are percent-encoded. An existing asset is reused only
when its size matches the local file; otherwise it is deleted and re-uploaded.
`422 already_exists` triggers the same size check.

### D10 — Upload parts stream

`split_file` writes each part with a 1 MiB reused buffer. It checks free space
for the whole archive before splitting, not only one part.

### D11 — Extraction is bounded

`extract_archive` takes `max_bytes: Option<u64>` and refuses to write more than
that. Restore passes `manifest size + 64 MiB`. Extraction also caps entries at
1,000,000 and checks `fs4::available_space` against the manifest total before
starting. Directory modes are restored with `filtered_mode`. Hard links whose
target appears later in the stream are queued and retried after the entry loop.

### D12 — Import is validated, export cannot destroy config

`config::validate_patch` checks every key present in an imported object:
types, enums, ranges, `owner/name`, branch characters, and folder-ID segments.
Unknown keys are rejected. `export-config` refuses to write to the live config
path and writes mode 0600 via `json::write_json_private`.

### D13 — Commands fail loudly

`delete` returns `Err` after printing the failure. `clear_cache` counts only
verified removals and returns `Err` when any removal failed.

### D14 — Schedule and daemon are safe

`load_schedule` normalizes to an object with bool `enabled`, int `hour`
0–23, int `minute` 0–59, string `label`. A non-object file is an error, not a
panic. `save_schedule` writes a 0600 temp file and renames. `daemon` takes an
exclusive `flock` on `cron.lock` and exits with a clear error when another
daemon holds it. The boot script single-quotes the executable path, propagates
chmod errors, and is removed when scheduling is disabled.

### D15 — Status reports the Rust requirements

Required tools: `dpkg` always; `rclone` for gdrive. `python3`, `tar`, `curl`,
and `git` are removed from the required list in both the CLI and the TUI.

### D16 — One CLI parser

Delete `rust/app/src/cli.rs` and `rust/app/src/units/`. `main` calls
`frontdoor::run_from_argv` directly. `frontdoor` collects `args_os` once,
rejects non-UTF8 command/target with exit 2, keeps the early `tui` handoff,
removes `tui` from `COMMANDS`, loads config only for commands that need it, and
sets debug through a process-local flag (edition 2024 makes `set_var` unsafe).

### D17 — TUI probes and state are correct

`supports_flag` probes `logs <flag>` and requires exit 0. Cancel from
`AwaitingConfirm` records `cancel_sent_at`. Event lines are read through a
bounded reader. Manifest labels, event text, and log lines are stripped of C0
and C1 controls before display. Status queries the installed CLI version. TUI
reads a bounded log tail and caches capability probes.

## Finding Inventory

### Critical

| ID | Location | Finding | Task |
|----|----------|---------|------|
| C1 | `maintenance.rs:278` | Setup saves the unedited `cfg`; every answer is discarded | T1 |
| C2 | `archive.rs:446` | Regular entry over an existing directory runs `remove_dir_all` | T2 |
| C3 | `backup.rs:488` | Local/rclone archive failure continues and reports success | T5 |
| C4 | `backup.rs:1204` | Restore emits `ok: true` with failed archives | T6 |
| C5 | `rclone.rs:190` | `delete_old` purges every top-level Drive directory | T4 |

### Required

| ID | Location | Finding | Task |
|----|----------|---------|------|
| R1 | `backup.rs:579`, `storage.rs:180`, `rclone.rs:189` | `max_backups <= 0` deletes all; prune runs before verify | T3, T5 |
| R2 | `storage.rs:49-69` | Upload copies `manifest.json` before some archives | T3 |
| R3 | `storage.rs:328` | Unknown storage silently falls back to local | T3 |
| R4 | `packages.rs:9` | `dpkg` failures become an empty package list | T5 |
| R5 | `github.rs:240,643,649` | Release list/delete errors are swallowed | T7 |
| R6 | `github.rs:337,389` | Existing assets and `422` are accepted without size check | T7 |
| R7 | `github.rs:263` | 200 MiB buffer per chunk; no pre-split free-space check | T7 |
| R8 | `archive.rs:508` | No byte/entry cap and no restore free-space precheck | T2, T6 |
| R9 | `archive.rs:459` | Directory modes are dropped | T2 |
| R10 | `archive.rs:596` | Forward-referenced hard links are lost | T2 |
| R11 | `maintenance.rs:888` | `import-config` accepts arbitrary keys and ranges | T8 |
| R12 | `maintenance.rs:843` | Export can overwrite live config; writes 0644 | T8 |
| R13 | `maintenance.rs:86-106` | Failed delete returns `Ok` | T9 |
| R14 | `frontdoor.rs:221` | Eager config load blocks setup/import/export recovery | T8 |
| R15 | `maintenance.rs:189-193` | Empty token cannot keep the stored token | T1 |
| R16 | `maintenance.rs:376` | No single-instance daemon lock | T10 |
| R17 | `maintenance.rs:507`, `tui/data.rs:444` | Status requires Python-era tools | T11 |
| R18 | `main.rs:20`, `cli.rs`, `units/mod.rs` | clap tree is parsed then discarded; second parser disagrees | T12 |
| R19 | `frontdoor.rs:143` | `--debug` sets an unread field | T13 |
| R20 | `frontdoor.rs:244` | `tui` in `COMMANDS` returns success without starting the TUI | T12 |
| R21 | `frontdoor.rs:113` | `std::env::args()` panics on non-UTF8 argv | T12 |
| R22 | `frontdoor.rs:78-82` | Unknown-command hints reference removed `tsctl` | T13 |
| R23 | `tui/ops.rs:388` | `supports_flag` passes for CLIs that reject the flag | T14 |
| R24 | `tui/ops.rs:820` | Awaiting-confirm cancel never arms the force-kill timer | T14 |
| R25 | `tui/ops.rs:477` | Event line is allocated before the 64 KiB check | T14 |
| R26 | `tui/data.rs:243`, `ui.rs:548` | Remote labels/log text can inject terminal controls | T15 |
| R27 | `tui/data.rs:461` | Status shows a compiled-in version, never the installed CLI | T15 |
| R28 | `tui/README.md`, `tui/RUNBOOK.md` | Still describe a Python-era spike | T17 |
| R29 | `.github/ISSUE_TEMPLATE/*`, `PULL_REQUEST_TEMPLATE.md` | Still ask for Python versions and ban Python deps | T17 |
| R30 | `tests/cli_integration.rs:229` | Setup test passes while setup writes nothing | T1 |
| R31 | `maintenance.rs:325` | `cron.json` non-object panics on index | T10 |
| R32 | `maintenance.rs:124` | `save_schedule` truncates in place | T10 |
| R33 | `maintenance.rs:293-305` | Boot script is unquoted; chmod error ignored; written when disabled | T10 |
| R34 | `maintenance.rs:793` | `clear_cache` reports freed bytes for failed removals | T9 |
| R35 | `github.rs:215` | Repo/tag interpolated into URLs unvalidated | T7 |

### Optional / Nit

| ID | Location | Finding | Task |
|----|----------|---------|------|
| O1 | `backup.rs:241,779` | Temp dirs are 0755; restore dir name collides; backup dir reuses existing | T5, T6 |
| O2 | `archive.rs:98,375,443` | Symlink-swap window, per-member resolve cost, live-tree chmod without guard, progress before excludes | T2 |
| O3 | `rclone.rs:44,54` | `folder_id` unvalidated; stderr discarded | T4 |
| O4 | `github.rs:586` | Failed `read_dir` returns success | T7 |
| O5 | `logging.rs:22` | Log file receives unredacted messages | T20 |
| O6 | `tui/app.rs:1389,1046,1613` | Whole log read on UI thread; synchronous probes; unbounded output | T16 |
| O7 | `tui/ops.rs:377` | Backup list bypasses the configured CLI | T16 |
| O8 | `tui/ui.rs`, `tui/app.rs` | 2736/2516-line modules | T21 |
| O9 | `tui/tests/pty.sh` | `timeout ... || true` hides hangs; fixed sleeps race | T19 |
| O10 | `rust/app-cases/*.toml` | Device-absolute PATH in fixtures | T19 |
| O11 | `tests/cli_integration.rs` | Error paths untested | T19 |
| O12 | `main.rs:8`, `rust/app-cases/rust-anything.json`, `GLOSSARY.md` | Migration leftovers | T20 |
| O13 | `install.sh`, `rust/install.sh`, `.github/workflows/rust.yml` | No `--locked`; root installer not in CI paths | T18 |
| O14 | `tui/history.rs:42` | History file is 0644 | T16 |
