<p align="center">
  <img src="https://raw.githubusercontent.com/GeneralKaos666/termux-sync-rust/main/assets/logo.svg" alt="termux-sync logo" width="480">

[![Rust 1.100+](https://img.shields.io/badge/rust-1.100%2B-orange?badge.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?badge.svg)](LICENSE)
[![Termux](https://img.shields.io/badge/Termux-compatible-brightgreen?badge.svg)](https://termux.dev)
[![GitHub release](https://img.shields.io/github/v/release/GeneralKaos666/termux-sync-rust?logo=github)](https://github.com/GeneralKaos666/termux-sync-rust/releases/latest)
[![Last commit](https://img.shields.io/github/last-commit/GeneralKaos666/termux-sync-rust?logo=git&logoColor=white)](https://github.com/GeneralKaos666/termux-sync-rust/commits/main)
[![Stars](https://img.shields.io/github/stars/GeneralKaos666/termux-sync-rust?style=flat&logo=github)](https://github.com/GeneralKaos666/termux-sync-rust/stargazers)
[![Forks](https://img.shields.io/github/forks/GeneralKaos666/termux-sync-rust?style=flat&logo=github)](https://github.com/GeneralKaos666/termux-sync-rust/network/members)
[![Issues](https://img.shields.io/github/issues/GeneralKaos666/termux-sync-rust?logo=github)](https://github.com/GeneralKaos666/termux-sync-rust/issues)
[![Pull requests](https://img.shields.io/github/issues-pr/GeneralKaos666/termux-sync-rust?logo=github)](https://github.com/GeneralKaos666/termux-sync-rust/pulls)
[![Repo size](https://img.shields.io/github/repo-size/GeneralKaos666/termux-sync-rust)](https://github.com/GeneralKaos666/termux-sync-rust)
[![Platform](https://img.shields.io/badge/platform-Termux%20%7C%20Android-3DDC84?logo=android&logoColor=white)](https://termux.dev)
[![Storage](https://img.shields.io/badge/storage-local%20%7C%20Google%20Drive%20%7C%20GitHub-blue)](#setup)
[![Terminal UI](https://img.shields.io/badge/UI-Ratatui-8A2BE2)](https://ratatui.rs)
[![Checksums](https://img.shields.io/badge/integrity-SHA--256-success)](#backup-structure)
[![PRs welcome](https://img.shields.io/badge/PRs-welcome-brightgreen)](CONTRIBUTING.md)
[![Maintained](https://img.shields.io/badge/maintained-yes-brightgreen)](https://github.com/GeneralKaos666/termux-sync-rust/commits/main)
</p>

# termux-sync

Back up and restore a Termux environment across devices. termux-sync captures the
installed package list, the home directory, and Termux configuration into a portable
backup. Store backups on the device, in Google Drive through rclone, or in a private
GitHub repository as GitHub Releases.

This repository is the pure Rust implementation of
[termux-sync](https://github.com/djunekz/termux-sync), the original Python project by
[djunekz](https://github.com/djunekz). The command line, configuration file, manifest,
and backup layout are unchanged, so backups move between this build and the original
in both directions.

---

<p align="center">
  <img src="https://raw.githubusercontent.com/GeneralKaos666/termux-sync-rust/main/assets/menu.svg" alt="termux-sync command menu" width="720">
</p>

## Features

| Feature | Description |
|---|---|
| Full backup | Installed packages, home directory, and `$PREFIX/etc` configuration |
| Full restore | Checksum verification, archive extraction, and package reinstallation |
| Delete backup | Remove a specific backup from any storage backend |
| Local storage | Save backups on-device or on any mounted path |
| Google Drive | Upload and retrieve through rclone |
| GitHub storage | Private repository through GitHub Releases; assets split at 200 MB |
| Auto-backup | Daily scheduler with a Termux:Boot startup script |
| Disk check | Storage usage for Termux, home, `$PREFIX`, or a custom path |
| Cache cleaner | Selective or full cache removal with two-step confirmation |
| Log viewer | Recent log lines with a configurable count |
| Clear logs | Truncate the log file in one command |
| Export config | Save configuration to a portable file with secrets redacted |
| Import config | Load an exported configuration without overwriting stored secrets |
| Checksum verification | SHA-256 check performed on every restore |
| Terminal UI | Ratatui operations cockpit in `termux-sync-tui` |
| JSON events | `--json-events` progress stream used by the TUI and scripts |
| Persistent logging | Full log written to `~/.config/termux-sync/sync.log` |

---

## Requirements

```
Rust 1.100+ (pkg install rust)
dpkg (included in Termux)
```

Optional dependencies:

- `rclone` for Google Drive (`pkg install rclone`)
- `git` only to clone this repository (`pkg install git`)
- `Termux:Boot` for auto-backup at device start (install from F-Droid)

The installed binaries have no Python runtime dependency. Backup archives are built
by the Rust code, so no external `tar` command is required.

---

## Installation

Build and install both binaries from a clone:

```bash
pkg install rust git
git clone https://github.com/GeneralKaos666/termux-sync-rust ~/termux-sync
cd ~/termux-sync
bash install.sh
```

`install.sh` builds `termux-sync` and `termux-sync-tui` in release mode and installs
them into `$PREFIX/bin`.

To keep an existing Python command available as a rollback, run:

```bash
bash cutover.sh
```

The old command is saved as `termux-sync-python` before the Rust binary replaces it.

---

## Compatibility with other termux-sync builds

Backups are portable across implementations:

- `manifest.json` keeps the same keys (`name`, `date`, `label`, `termux_home`,
  `termux_usr`, `storage`, `compression`, `version`, `files`, `packages`) and the same
  SHA-256 checksums.
- Archive names follow the same pattern (`home.tar.gz`, `usr_etc.tar.gz`), and the
  extension follows the configured compression.
- The configuration file stays at `~/.config/termux-sync/config.json` with the same keys.

Any backup produced by the original Python termux-sync or another termux-sync build
can be listed, verified, and restored here. Backups created here can also be restored
by those builds. No migration step is required.

---

## Setup

Run the interactive setup wizard before the first backup:

```bash
termux-sync setup
```

The wizard asks for a storage backend, compression, and retention limit. It can also
customise the exclude patterns.

### Local storage

Choose option **1**. The default destination is `~/termux-backups`. Enter another
path to store backups on a mounted drive.

### Google Drive

Install rclone and create a remote named exactly `termux-sync-gdrive`:

```bash
pkg install rclone
rclone config
termux-sync setup
```

Choose option **2**. Leave the folder ID empty to write to the Drive root, or enter a
folder ID to keep backups in one folder.

### GitHub storage (recommended)

1. Create a private GitHub repository, for example `my-termux-backup`.
2. Open **Settings → Developer settings → Personal access tokens → Fine-grained tokens**.
3. Grant the token access to that repository only, with **Contents: Read and write**.
4. Run `termux-sync setup` and select option **3 (GitHub)**.
5. Enter the token, the repository as `owner/repo`, and the branch (default `main`).

Each backup becomes a GitHub Release tagged with the backup name. Archive files are
release assets, split into 200 MB parts when needed. A classic token with the `repo`
scope also works, but it grants access to every repository you own.

---

## Command Reference

```text
termux-sync <command> [target] [options]
```

| Command | Purpose |
|---|---|
| `backup` | Create a new backup |
| `restore` | Restore from a backup |
| `list` | List available backups |
| `delete` | Delete a specific backup |
| `setup` | Interactive configuration wizard |
| `schedule` | Configure the daily auto-backup |
| `daemon` | Run the background scheduler used by Termux:Boot |
| `export-config` | Export configuration with secrets redacted |
| `import-config` | Import configuration from a file |
| `status` | Show configuration and environment |
| `logs` | Show recent log lines |
| `clear-logs` | Truncate the log file |
| `check` | Show disk usage |
| `clear-cache` | Remove temporary and cached files |
| `tui` | Launch the operations TUI |
| `help` | Show help |

Global options:

| Option | Description |
|---|---|
| `-l, --label LABEL` | Label for the backup |
| `-n, --name NAME` | Backup name to restore or delete |
| `--lines N` | Number of log lines to show (default 40) |
| `--yes` | Skip confirmation for `delete`, `clear-logs`, and `clear-cache` |
| `--json-events` | Emit JSON progress events on stdout and human text on stderr |
| `--debug` | Show tracebacks and include TRACE lines in `logs` |
| `-v, --version` | Print the version and exit |
| `-h, --help` | Show help and exit |

### Backup

```bash
termux-sync backup
termux-sync backup --label "before-new-phone"
```

### Restore

```bash
termux-sync restore                                          # interactive picker
termux-sync restore --name termux_backup_20240415_143000     # restore one backup
```

Restore verifies the manifest checksums before extraction, extracts the archives, and
reinstalls the recorded packages.

### List all backups

```bash
termux-sync list
```

Shows name, date, label, package count, file count, compression, version, and total
size for each backup.

### Delete a backup

```bash
termux-sync delete                                          # interactive picker
termux-sync delete --name termux_backup_20240415_143000     # delete one backup
```

Works with local, Google Drive, and GitHub storage. Use `--yes` to skip the prompt.

### Setup wizard

```bash
termux-sync setup
```

### Schedule daily auto-backup

```bash
termux-sync schedule
```

### Check configuration and environment

```bash
termux-sync status
```

Shows the storage backend, compression, retention limit, schedule, app version,
config and log paths, required tools, and the number of stored local backups.

### View log entries

```bash
termux-sync logs
termux-sync logs --lines 100
termux-sync logs --debug
```

### Clear log file

```bash
termux-sync clear-logs
termux-sync clear-logs --yes
```

Truncates `~/.config/termux-sync/sync.log` after confirmation.

### Check disk usage

```bash
termux-sync check                              # overview: Termux root, ~, $PREFIX
termux-sync check ~                            # home directory with subfolder breakdown
termux-sync check $PREFIX                      # $PREFIX with subfolder breakdown
termux-sync check /data/data/com.termux/files  # custom path
```

### Clear cache

```bash
termux-sync clear-cache
```

Lists the cache directories that exist, with sizes. Clear all of them or select
entries by number. Two confirmation prompts guard the deletion.

Targets: `$PREFIX/tmp`, `~/.cache`, `~/.npm`, `~/.cargo/registry`, `~/.cargo/git`,
`~/__pycache__`, `~/.gradle/caches`, `~/.m2/repository`, `~/go/pkg/mod/cache`.

### Export configuration

```bash
termux-sync export-config
termux-sync export-config ~/my-backup-config.json
```

Without a path, the wizard offers `~/termux-sync-config-export.json`. The export
replaces `github_token` and `encrypt_password` with `*** REDACTED ***` and writes the
file with mode `0600`.

### Import configuration

```bash
termux-sync import-config ~/my-backup-config.json
```

Loads an exported file. Values containing `REDACTED` are skipped, so stored
credentials are not overwritten.

### Background daemon

```bash
termux-sync daemon &
```

Runs the scheduler in the background. The Termux:Boot script written by
`termux-sync schedule` starts it at boot. Only one daemon can hold the lock.

---

## Auto-Backup

Configure a daily automatic backup:

```bash
termux-sync schedule
```

The wizard stores the schedule in `~/.config/termux-sync/cron.json` and writes
`~/.termux/boot/termux-sync-daemon.sh` when auto-backup is enabled. Install
[Termux:Boot](https://f-droid.org/en/packages/com.termux.boot/) from F-Droid and open
it once, then review the generated script:

```bash
cat ~/.termux/boot/termux-sync-daemon.sh
termux-sync daemon &
```

---

## What Gets Backed Up

| Component | Source | Description |
|---|---|---|
| Package list | `dpkg --get-selections` | Installed packages, recorded in `packages.txt` |
| Home directory | `$HOME` | Dotfiles, scripts, and projects |
| Termux config | `$PREFIX/etc` | `sources.list`, `motd`, and other configuration |

Shared libraries, installed binaries, and caches are not archived. Packages are
reinstalled from the package list during restore.

These paths are excluded from the home directory archive by default:

```text
.cache  .npm  __pycache__  node_modules  .gradle  .android  .thumbnails
DCIM  Movies  Music  .local/share/Trash  .local/lib  .java  .m2  .ivy2
go/pkg  .rustup/toolchains  .cargo/registry  .cargo/git
```

`~/.config/termux-sync` is always excluded, so the configuration and log do not leak
into the archive. Change the other exclusions in `~/.config/termux-sync/config.json`
or during `termux-sync setup`.

Exclude patterns match whole path components at any depth: `.cache` excludes
`~/.cache` but not `~/notes.cache`. Wildcards are not supported, so `*.log` matches
nothing. A leading `./` is ignored.

---

## Configuration File

Location: `~/.config/termux-sync/config.json`

```json
{
  "storage": "github",
  "local_path": "~/termux-backups",
  "gdrive_folder_id": "",
  "github_token": "github_pat_...",
  "github_repo": "username/my-termux-backup",
  "github_branch": "main",
  "compression": "gz",
  "exclude_patterns": [
    ".cache", ".npm", "__pycache__", "node_modules", ".gradle",
    ".android", ".thumbnails", "DCIM", "Movies", "Music",
    ".local/share/Trash", ".local/lib", ".java",
    ".m2", ".ivy2", "go/pkg", ".rustup/toolchains",
    ".cargo/registry", ".cargo/git"
  ],
  "max_backups": 5,
  "encrypt": false,
  "encrypt_password": ""
}
```

| Key | Default | Description |
|---|---|---|
| `storage` | `local` | Storage backend: `local`, `gdrive`, or `github` |
| `local_path` | `~/termux-backups` | Path for local backups |
| `gdrive_folder_id` | empty | Google Drive folder ID; empty uses the Drive root |
| `github_token` | empty | GitHub token with `Contents: Read and write` |
| `github_repo` | empty | Repository in `owner/repo` format |
| `github_branch` | `main` | Branch used as the release target |
| `compression` | `gz` | Compression: `gz` (fast), `bz2` (balanced), `xz` (best) |
| `exclude_patterns` | list above | Paths excluded from the home archive |
| `max_backups` | `5` | Backups kept before pruning |
| `encrypt` | `false` | Reserved for compatibility; encryption is not implemented |
| `encrypt_password` | empty | Reserved for compatibility |

---

## Environment Variables

A reference file is provided at [`.env.example`](.env.example). All variables are
optional; Termux sets sensible defaults.

| Variable | Default | Description |
|---|---|---|
| `TERMUX_SYNC_DEBUG` | empty | Any non-empty value enables debug output |
| `PREFIX` | `/data/data/com.termux/files/usr` | Termux installation prefix |
| `HOME` | `/data/data/com.termux/files/home` | Home directory used as the backup source |
| `TMPDIR` | `/data/data/com.termux/files/usr/tmp` | Temporary directory for archive work |

---

## Backup Structure

A local backup is a directory with this layout:

```text
termux-backups/
└── termux_backup_20240415_143000/
    ├── manifest.json       metadata, checksums, package list, source paths, version
    ├── packages.txt        plain-text package list
    ├── home.tar.gz         home directory archive
    └── usr_etc.tar.gz      Termux configuration archive ($PREFIX/etc)
```

Archive names use the configured compression: `home.tar.gz`, `home.tar.bz2`, or
`home.tar.xz`. On GitHub, each archive becomes a release asset; archives larger than
200 MB are uploaded as `.part0001`, `.part0002`, and so on, then joined on download.

`manifest.json` records the termux-sync version, compression, label, package list,
per-archive size, SHA-256 checksum, and source path. Restore reads this file and
verifies every checksum before extracting anything.

---

## Migrating to a New Device

**On the old device**, create a labeled backup:

```bash
termux-sync backup --label "migration"
```

**On the new device**, install termux-sync and restore:

```bash
pkg install rust git
git clone https://github.com/GeneralKaos666/termux-sync-rust ~/termux-sync
cd ~/termux-sync
bash install.sh

# Configure with the same credentials as the old device
termux-sync setup

# Restore the latest backup
termux-sync restore
```

Restore verifies checksums, extracts the archives, and reinstalls all packages.
Backups from the original Python termux-sync work without conversion.

---

## Troubleshooting

| Problem | Solution |
|---|---|
| `rclone not found` | Run `pkg install rclone`, then `rclone config`, and name the remote `termux-sync-gdrive` |
| GitHub 401 error | Check that the token is valid and has `Contents: Read and write` on the backup repository |
| GitHub 403 error | Confirm the token can read the repository and that fine-grained access includes it |
| Archive too large | Add paths to `exclude_patterns`, or run `termux-sync clear-cache` first |
| Restore overwrites the wrong path | Check the source path in the backup's `manifest.json` |
| `termux-sync: command not found` | Re-run `bash install.sh` from the clone |
| `cargo: command not found` | Run `pkg install rust` |
| Daemon does not start on boot | Install Termux:Boot from F-Droid and open it once |
| GitHub upload fails on a large backup | Add paths to `exclude_patterns` to reduce the backup size |
| Cache not cleared | Run `termux-sync clear-cache` and confirm both prompts |
| Config lost after reinstall | Run `export-config` before uninstalling and `import-config` afterwards |
| Old backups not pruned on Drive | Fixed in v1.2.0; update to the current build |

---

## Project Structure

```text
termux-sync-rust/
├── app/                    termux-sync CLI
├── core/                   shared core library
├── tui/                    Ratatui operations TUI
├── app-cases/              recorded CLI behavior cases (trycmd)
├── fixtures/               fixture homes, prefixes, and tool stubs
├── assets/                 README logo and command menu
├── docs/                   design records and plans
├── install.sh              release build and install for both binaries
├── cutover.sh              switch from an installed Python command
├── smoke-github.sh         live GitHub storage smoke test
├── Cargo.toml              workspace manifest
├── CHANGELOG.md            version history
├── FAQ.md                  frequently asked questions
└── README.md               this file
```

---

## Optional Rust TUI

The `tui/` directory contains a Ratatui dashboard and operations cockpit. It shares
the Rust core and is installed by `install.sh`.

The TUI streams backup and restore through `termux-sync --json-events`, shows live
stages and logs, verifies backups, lists remote backups, and gates restore behind a
package diff and typed confirmation. It also keeps a local operation history, filters
backups, prompts for labels, and supports ASCII icons.

Requirements:

- Rust 1.100 or newer (`pkg install rust` on Termux)
- A `termux-sync` 2.0 build

Run it from the CLI:

```bash
termux-sync tui
```

Or run the standalone alias:

```bash
termux-sync-tui
```

Use `--ascii` or `TERMUX_SYNC_TUI_ASCII=1` when the terminal font renders the default
icons badly.

If the installed `termux-sync` is too old for events, backup and restore refuse with
a clear message, and `L` falls back to captured text output. See
[tui/README.md](tui/README.md) for keys, cancellation rules, and layout behavior.
Recovery steps live in [tui/RUNBOOK.md](tui/RUNBOOK.md).

The TUI has its own workflow in `.github/workflows/tui.yml`. That workflow also
cross-checks Android targets with the NDK. Prebuilt release binaries remain a later
distribution task.

---

## Contributing

Contributions are welcome. Read [CONTRIBUTING](CONTRIBUTING.md) before opening a pull
request. For bug reports and feature requests, use the GitHub issue tracker and follow
the provided templates.

---

## Credits

- Original Python implementation: [djunekz/termux-sync](https://github.com/djunekz/termux-sync)
- Rust rewrite: [GeneralKaos666/termux-sync-rust](https://github.com/GeneralKaos666/termux-sync-rust)

---

## License

MIT License. See [LICENSE](LICENSE) for the full text.
