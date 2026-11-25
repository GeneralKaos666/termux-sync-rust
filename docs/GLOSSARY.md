# Glossary

Domain terms from the original and their Rust names. One term per concept; units use
these names.

| Term | Meaning in the original | Rust name |
|---|---|---|
| backup | One manifest plus compressed archives stored by a backend | `Backup` |
| backup name | Directory/release name, e.g. `termux-backup-2026-11-24_10-00-00` | `BackupName` |
| label | User note stored in the manifest | `label: String` |
| manifest | `manifest.json` metadata inside a backup | `Manifest` |
| archive | `.tar.gz`, `.tar.bz2` or `.tar.xz` payload | `Archive` |
| checksum | SHA-256 of an archive or of the whole streamed backup | `Sha256Digest` |
| storage backend | Local, Google Drive via rclone, or GitHub Releases | `Storage` trait |
| local storage | Backup directory under `DEFAULT_DEST` | `LocalStorage` |
| gdrive storage | rclone remote path | `RcloneStorage` |
| github storage | GitHub Releases assets, split into parts | `GitHubStorage` |
| stream part | `NAME.partNNNN` chunk uploaded then deleted | `Part` / `PartWriter` |
| event | One JSON line on stdout in `--json-events` mode | `Event` |
| stage | Named progress phase such as `archive:home` or `extract:home` | `Stage` |
| stage state | pending, active, complete, failed, skipped | `StageState` |
| plan (restore) | Parsed restore intent shown before extraction | `RestorePlan` |
| package diff | Packages missing on this device vs the backup | `PackageDiff` |
| schedule | `cron.json`, interval and retention for auto-backup | `Schedule` |
| daemon | Scheduled runner invoked by Termux:Boot | `daemon` module |
| setup | Interactive config wizard | `setup` module |
| maintenance | status, logs, clear-logs, check, clear-cache, export/import | `maintenance` module |
| keep | Retention count for backups (`max_backups`) | `max_backups` |
| exclude pattern | Path component excluded from archives | `ExcludePattern` |
| backup item | packages, home, usr_etc, usr_lib | `BackupItem` |
| user error | Expected failure with title/hint, no traceback | `UserError` |
| crash | Unexpected failure with optional debug traceback | `Crash` |
| redaction | Mask token and password in exported config/events | `Redaction` |
| narrow terminal | Width below the threshold, compact output | `is_narrow()` |
| TUI | Ratatui operations cockpit | `termux-sync-tui` |
| front door | Rust binary owning the entry point during the port | `app` |
| fallback | A command the Rust front door does not implement; it exits with an unknown-command error | `frontdoor` |
| lift | Removal of fallback, kill switches and bridges | `app lift` |
