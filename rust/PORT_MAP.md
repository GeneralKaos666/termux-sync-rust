# Port map: termux-sync-rust-tui

5 of 6 units have a Rust port; no original changed since it was recorded.

## Ports

| Unit | Crate | In Rust | Kept in the original | Cases | Last diff | Original |
| --- | --- | --- | --- | ---: | ---: | --- |
| `src/termux_sync/` | `rust/fixtures/unit-home/tmp/rust-anything-lift-12446-1795578503534931538/rust/termux-sync-core` | `log`, `load_config`, `save_config`, `event_stage`, `event_confirmation`, `enable_event_mode`, `print_error`, `handle_crash`, `print_banner`, `note`, `print_section`, `status_icon`, `is_narrow`, `make_progress`, `fmt_elapsed`, `print_backup_list`, `human_size`, `sha256`, `get_installed_packages`, `install_packages`, `write_manifest`, `read_manifest`, `get_storage`, `cmd_backup`, `cmd_restore`, `cmd_list`, `cmd_delete`, `cmd_setup`, `load_schedule`, `save_schedule`, `cmd_schedule`, `cmd_daemon`, `short_path`, `cmd_status`, `cmd_logs`, `cmd_clear_logs`, `dir_size`, `cmd_check`, `cmd_clear_cache`, `cmd_export_config`, `cmd_import_config`, `print_help` | `emit_event`, `create_archive`, `extract_archive`, `build_parser`, `main`, `EventWriter`, `EventProgress`, `UserError`, `LocalStorage`, `GDriveStorage`, `GitHubStorage`, `StreamAbort`, `FriendlyParser` | 28 | 28/28 | unchanged |
| `src/termux_sync/` | `rust/fixtures/unit-home/tmp/rust-anything-lift-13183-1795579848190209150/rust/termux-sync-core` | `log`, `load_config`, `save_config`, `event_stage`, `event_confirmation`, `enable_event_mode`, `print_error`, `handle_crash`, `print_banner`, `note`, `print_section`, `status_icon`, `is_narrow`, `make_progress`, `fmt_elapsed`, `print_backup_list`, `human_size`, `sha256`, `get_installed_packages`, `install_packages`, `write_manifest`, `read_manifest`, `get_storage`, `cmd_backup`, `cmd_restore`, `cmd_list`, `cmd_delete`, `cmd_setup`, `load_schedule`, `save_schedule`, `cmd_schedule`, `cmd_daemon`, `short_path`, `cmd_status`, `cmd_logs`, `cmd_clear_logs`, `dir_size`, `cmd_check`, `cmd_clear_cache`, `cmd_export_config`, `cmd_import_config`, `print_help` | `emit_event`, `create_archive`, `extract_archive`, `build_parser`, `main`, `EventWriter`, `EventProgress`, `UserError`, `LocalStorage`, `GDriveStorage`, `GitHubStorage`, `StreamAbort`, `FriendlyParser` | 28 | 28/28 | unchanged |
| `src/termux_sync/` | `rust/fixtures/unit-home/tmp/rust-anything-lift-25560-1795580410350121383/rust/termux-sync-core` | `log`, `load_config`, `save_config`, `event_stage`, `event_confirmation`, `enable_event_mode`, `print_error`, `handle_crash`, `print_banner`, `note`, `print_section`, `status_icon`, `is_narrow`, `make_progress`, `fmt_elapsed`, `print_backup_list`, `human_size`, `sha256`, `get_installed_packages`, `install_packages`, `write_manifest`, `read_manifest`, `get_storage`, `cmd_backup`, `cmd_restore`, `cmd_list`, `cmd_delete`, `cmd_setup`, `load_schedule`, `save_schedule`, `cmd_schedule`, `cmd_daemon`, `short_path`, `cmd_status`, `cmd_logs`, `cmd_clear_logs`, `dir_size`, `cmd_check`, `cmd_clear_cache`, `cmd_export_config`, `cmd_import_config`, `print_help` | `emit_event`, `create_archive`, `extract_archive`, `build_parser`, `main`, `EventWriter`, `EventProgress`, `UserError`, `LocalStorage`, `GDriveStorage`, `GitHubStorage`, `StreamAbort`, `FriendlyParser` | 28 | 28/28 | unchanged |
| `src/termux_sync/` | `rust/fixtures/unit-home/tmp/rust-anything-lift-30326-1795579197205779867/rust/termux-sync-core` | `log`, `load_config`, `save_config`, `event_stage`, `event_confirmation`, `enable_event_mode`, `print_error`, `handle_crash`, `print_banner`, `note`, `print_section`, `status_icon`, `is_narrow`, `make_progress`, `fmt_elapsed`, `print_backup_list`, `human_size`, `sha256`, `get_installed_packages`, `install_packages`, `write_manifest`, `read_manifest`, `get_storage`, `cmd_backup`, `cmd_restore`, `cmd_list`, `cmd_delete`, `cmd_setup`, `load_schedule`, `save_schedule`, `cmd_schedule`, `cmd_daemon`, `short_path`, `cmd_status`, `cmd_logs`, `cmd_clear_logs`, `dir_size`, `cmd_check`, `cmd_clear_cache`, `cmd_export_config`, `cmd_import_config`, `print_help` | `emit_event`, `create_archive`, `extract_archive`, `build_parser`, `main`, `EventWriter`, `EventProgress`, `UserError`, `LocalStorage`, `GDriveStorage`, `GitHubStorage`, `StreamAbort`, `FriendlyParser` | 28 | 28/28 | unchanged |
| `src/termux_sync/` | `rust/termux-sync-core` | `log`, `load_config`, `save_config`, `event_stage`, `event_confirmation`, `enable_event_mode`, `print_error`, `handle_crash`, `print_banner`, `note`, `print_section`, `status_icon`, `is_narrow`, `make_progress`, `fmt_elapsed`, `print_backup_list`, `human_size`, `sha256`, `get_installed_packages`, `install_packages`, `write_manifest`, `read_manifest`, `get_storage`, `cmd_backup`, `cmd_restore`, `cmd_list`, `cmd_delete`, `cmd_setup`, `load_schedule`, `save_schedule`, `cmd_schedule`, `cmd_daemon`, `short_path`, `cmd_status`, `cmd_logs`, `cmd_clear_logs`, `dir_size`, `cmd_check`, `cmd_clear_cache`, `cmd_export_config`, `cmd_import_config`, `print_help` | `emit_event`, `create_archive`, `extract_archive`, `build_parser`, `main`, `EventWriter`, `EventProgress`, `UserError`, `LocalStorage`, `GDriveStorage`, `GitHubStorage`, `StreamAbort`, `FriendlyParser` | 28 | 0/28 | unchanged |

## Whole app

| Glue | Runs |
| --- | --- |
| `rust/app/src/units/termux_sync_core.rs` | Rust only |

## Not ported yet

In the order `analyze` suggests.

| Unit | Lines | Exports | Readiness |
| --- | ---: | ---: | ---: |
| `termux-sync.py` | 2866 | 55 | 0 |

Regenerate with `rust-anything map --write`. Commit this file: when the original changes upstream, the next map marks every port whose recording it outdated.
