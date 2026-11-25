# TUI CLI Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close every remaining `termux-sync` CLI feature gap in the Rust TUI except release packaging, without replacing the setup and schedule handoff cards or adding daemon process control.

**Architecture:** Keep backup, restore, and destructive command logic in `termux-sync`. Add read-only status and daemon views to the TUI from `config.json`, `cron.json`, `sync.log`, and `$PATH`. Route config export/import, disk check, and clear-logs through the CLI. Generalize the existing backup-label prompt into one input prompt used by labels and paths.

**Tech Stack:** Python 3.10+, Rust 1.85+, Ratatui 0.30, Crossterm, serde_json, Termux/Android.

**Spec:**
- `docs/specs/2026-11-21-operations-cockpit-design.md`
- `docs/specs/2026-11-21-restore-cockpit-design.md`
- `tui/README.md` (current behavior and safety notes)
- Scope decision from this session: CLI parity only. Setup and schedule stay handoff cards. Daemon gets a status view plus manual instructions, not start/stop control.

## Global Constraints

- `termux-sync.py` and `src/termux_sync/__init__.py` must remain byte-identical.
- CLI changes stay additive. Existing output and behavior must not change without a new flag.
- Never emit or display GitHub tokens, encryption passwords, or config secrets. Only display repo, branch, Drive folder ID, paths, compression, and backup limits.
- Backup, restore, delete, clear-cache, export-config, import-config, check, and clear-logs logic stays in Python. Rust reads files and drives the CLI.
- Event protocol stays version `1`.
- Restore checksums must pass before extraction. Restore cancel stays refused after the first `extract:*` stage starts.
- TUI remains usable at `48×12`; header baseline is `80×24` and `56×24`.
- Every commit is GPG-signed (`git commit -S`).
- Python checks: `pytest -q`, `ruff check src tests`, `cmp termux-sync.py src/termux_sync/__init__.py`.
- Rust checks: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, `cargo build --locked --release`.

## Review Focus

- **Empty or whitespace import path:** the prompt must not launch `termux-sync import-config` with no path, because captured execution has no stdin and the CLI would prompt into EOF. It must keep the prompt open and say to enter a path.
- **Export path with spaces:** the path must stay one argv element. No shell, no splitting `~` or spaces.
- **Clear logs with an existing old CLI:** missing `--yes` must fall back to the terminal handoff and let the CLI confirm. A missing log file must show "nothing to clear", not an error.
- **GitHub status view:** it may show repo and branch, never the token. The config parser test must keep proving tokens never enter `Config` debug output.
- **Daemon view:** schedule enabled is not proof the daemon is running. The view must say it cannot detect a running daemon and must never start or stop one.

---

## File Structure

- `termux-sync.py` — add `assume_yes` to `cmd_clear_logs`, pass `--yes` from the dispatcher, update the option text.
- `src/termux_sync/__init__.py` — byte-identical mirror.
- `tests/test_commands.py` — clear-logs `--yes` behavior.
- `tui/src/data.rs` — display-only config fields, status report, tool probe, path shortening, daemon activity filter.
- `tui/src/app.rs` — new actions, unified input prompt, status and daemon modal state, log line count.
- `tui/src/ui.rs` — status modal, daemon modal, generic input prompt, clear-logs confirmation, log line count in the Output title.
- `tui/src/main.rs` — static `--help` text for the new output keys.
- `tui/README.md`, `tui/RUNBOOK.md` — user-facing behavior and fallbacks.
- `tui/tests/pty.sh` — one smoke case for the status modal.

---

### Task 1: Status report data

**Files:**
- Modify: `tui/src/data.rs` (structs at lines 10-17, `parse_config` at 125-149, tests at 410+)
- Test: `tui/src/data.rs` test module

**Interfaces:**
- Consumes: existing `Config`, `home()`, `cli_command()`.
- Produces:
  - `Config` gains `gdrive_folder_id: String`, `github_repo: String`, `github_branch: String`.
  - `pub struct ToolStatus { pub name: &'static str, pub path: Option<PathBuf>, pub required: bool }`
  - `pub struct StatusReport { pub storage: String, pub target: String, pub compression: String, pub max_backups: u64, pub schedule: String, pub cli_version: Option<String>, pub config_path: String, pub log_path: String, pub history_path: String, pub tools: Vec<ToolStatus> }`
  - `pub fn status_target(config: &Config) -> String`
  - `pub fn probe_tools(storage: &str, paths: &[PathBuf]) -> Vec<ToolStatus>`
  - `pub fn which_in(name: &str, paths: &[PathBuf]) -> Option<PathBuf>`
  - `pub fn status_report(config: &Config, cli_version: Option<String>, tools: Vec<ToolStatus>) -> StatusReport`
  - `pub fn collect_status(config: &Config) -> StatusReport`
  - `pub fn cli_version() -> Option<String>`
  - `pub fn shorten_path(path: &Path, home: &Path, prefix: &Path) -> String`
  - `pub fn short_path(path: &Path) -> String`

- [ ] **Step 1: Write the failing data tests**

Add to the existing `mod tests` in `tui/src/data.rs`:

```rust
#[test]
fn config_parser_keeps_display_targets_and_no_secrets() {
    let raw = serde_json::json!({
        "storage": "github",
        "github_repo": "me/backups",
        "github_branch": "backup",
        "gdrive_folder_id": "folder-id",
        "github_token": "secret-token",
        "encrypt_password": "secret-password"
    });
    let config = parse_config(&raw, &serde_json::json!({"enabled": false}));
    assert_eq!(config.github_repo, "me/backups");
    assert_eq!(config.github_branch, "backup");
    assert_eq!(config.gdrive_folder_id, "folder-id");
    let debug = format!("{config:?}");
    assert!(!debug.contains("secret-token"));
    assert!(!debug.contains("secret-password"));
}

#[test]
fn status_target_names_each_backend() {
    let mut config = Config::default();
    config.storage = "local".into();
    config.local_path = "~/termux-backups".into();
    assert_eq!(status_target(&config), "~/termux-backups");

    config.storage = "gdrive".into();
    config.gdrive_folder_id.clear();
    assert_eq!(status_target(&config), "(root)");
    config.gdrive_folder_id = "folder-id".into();
    assert_eq!(status_target(&config), "folder-id");

    config.storage = "github".into();
    config.github_repo = "me/backups".into();
    config.github_branch = "backup".into();
    assert_eq!(status_target(&config), "me/backups @ backup");
}

#[test]
fn probe_tools_requires_rclone_only_for_gdrive() {
    let local = probe_tools("local", &[]);
    assert!(local.iter().all(|tool| tool.path.is_none()));
    assert!(!local.iter().find(|tool| tool.name == "rclone").unwrap().required);

    let drive = probe_tools("gdrive", &[]);
    assert!(drive.iter().find(|tool| tool.name == "rclone").unwrap().required);
}

#[test]
fn which_in_finds_a_file_in_path_order() {
    let scratch = |name: &str| {
        let dir = std::env::temp_dir().join(format!("ts-tui-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    };
    let first = scratch("first");
    let second = scratch("second");
    std::fs::write(second.join("git"), "").unwrap();
    let paths = vec![first, second.clone()];
    assert_eq!(which_in("git", &paths), Some(second.join("git")));
}

#[test]
fn shorten_path_replaces_home_and_prefix() {
    let home = Path::new("/data/data/com.termux/files/home");
    let prefix = Path::new("/data/data/com.termux/files/usr");
    assert_eq!(
        shorten_path(&home.join(".config/termux-sync"), home, prefix),
        "~/.config/termux-sync"
    );
    assert_eq!(
        shorten_path(&prefix.join("bin/tar"), home, prefix),
        "$PREFIX/bin/tar"
    );
}
```

Use the existing test style. If no `tempdir()` helper exists, use `std::env::temp_dir().join(format!("ts-tui-{}", std::process::id()))` plus `std::fs::create_dir_all`, then remove at the end.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd tui && cargo test --locked data::tests::config_parser_keeps_display_targets_and_no_secrets`
Expected: FAIL — missing fields and functions.

- [ ] **Step 3: Implement the data layer**

In `parse_config`, read `gdrive_folder_id`, `github_repo`, `github_branch` with `string_field`. Defaults: `""`, `""`, `"main"`. Update `Config::default`.

Implement `status_target`:
- `local`: `config.local_path`
- `gdrive`: `gdrive_folder_id`, or `"(root)"` when empty
- `github`: `"{repo} @ {branch}"`, or `"—"` when repo is empty
- anything else: `"—"`

`probe_tools(storage, paths)` returns tools in this order with these exact names:

```text
python3  required
dpkg     required
tar      required
rclone   required only when storage == "gdrive"
git      required
curl     required
```

Each `path` comes from `which_in(name, paths)`. `which_in` joins each path with the name and returns the first `is_file()` match.

`collect_status` passes `env::split_paths` of `PATH` to `probe_tools`, and `cli_version()` to `status_report`. `cli_version` runs `cli_command(&["--version"])`, takes the first non-empty stdout line, and returns `None` on spawn failure.

`status_report` fills `config_path`, `log_path`, and `history_path` with `short_path(...)`. `short_path` passes `home()` and `$PREFIX` to `shorten_path`. `shorten_path` replaces a path equal to or below `prefix` with `$PREFIX...`, then `home` with `~...`.

- [ ] **Step 4: Run the data tests**

Run: `cd tui && cargo test --locked data::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tui/src/data.rs
git commit -S -m "feat(tui): collect status and environment details"
```

---

### Task 2: Unified input prompt and path actions

**Files:**
- Modify: `tui/src/app.rs` (`App` fields 232-275, key handling 445-611, mouse 990-1020, `run` 1192-1250, tests 1460+)
- Modify: `tui/src/ui.rs` (`render_backup_label` 172-218, draw overlay 110-112)
- Test: `tui/src/app.rs`, `tui/src/ui.rs` test modules

**Interfaces:**
- Consumes: `Action::Backup`, `Action::DiskCheck`, `backup_command`.
- Produces:
  - `pub enum InputPrompt { BackupLabel, ExportConfig, ImportConfig, CheckTarget }`
  - `impl InputPrompt { pub fn title(self) -> &'static str; pub fn hint(self) -> &'static str; pub fn seed(self) -> &'static str; pub fn max_len(self) -> usize; pub fn cancel_status(self) -> &'static str; }`
  - `pub fn export_config_command(path: &str) -> Vec<String>`
  - `pub fn import_config_command(path: &str) -> Vec<String>`
  - `pub fn check_command(target: &str) -> Vec<String>`
  - `pub fn input_command(prompt: InputPrompt, text: &str) -> Option<Vec<String>>`
  - App fields `pending_input: Option<InputPrompt>`, `input_text: String`, `input_yes: Rect`, `input_no: Rect` replace `pending_label`, `label_text`, `label_yes`, `label_no`.

- [ ] **Step 1: Write the failing command-builder and prompt tests**

Add to `tui/src/app.rs` tests:

```rust
#[test]
fn path_command_builders_keep_paths_as_single_arguments() {
    assert_eq!(
        export_config_command("/tmp/my config.json"),
        vec!["export-config", "/tmp/my config.json"]
    );
    assert_eq!(
        import_config_command("~/export.json"),
        vec!["import-config", "~/export.json"]
    );
    assert_eq!(check_command(""), vec!["check"]);
    assert_eq!(check_command("$PREFIX"), vec!["check", "$PREFIX"]);
}

#[test]
fn blank_paths_are_rejected_but_blank_check_target_is_not() {
    assert_eq!(input_command(InputPrompt::ExportConfig, "  "), None);
    assert_eq!(input_command(InputPrompt::ImportConfig, ""), None);
    assert_eq!(
        input_command(InputPrompt::CheckTarget, " "),
        Some(vec!["check".to_string()])
    );
}

#[test]
fn export_action_opens_prefilled_prompt() {
    let mut app = App::with_snapshot(snapshot());
    app.run(Action::ExportConfig);
    assert_eq!(app.pending_input, Some(InputPrompt::ExportConfig));
    assert_eq!(app.input_text, "~/termux-sync-config-export.json");
    app.handle_key(KeyCode::Esc);
    assert!(app.pending_input.is_none());
}

#[test]
fn import_and_check_actions_open_empty_prompts() {
    let mut app = App::with_snapshot(snapshot());
    app.run(Action::ImportConfig);
    assert_eq!(app.pending_input, Some(InputPrompt::ImportConfig));
    assert!(app.input_text.is_empty());
    app.handle_key(KeyCode::Esc);

    app.run(Action::DiskCheck);
    assert_eq!(app.pending_input, Some(InputPrompt::CheckTarget));
    assert!(app.input_text.is_empty());
}
```

Update the existing `backup_action_opens_label_prompt` test to assert `pending_input == Some(InputPrompt::BackupLabel)`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd tui && cargo test --locked input_command`
Expected: FAIL — missing enum and builders.

- [ ] **Step 3: Generalize the prompt**

Add `ExportConfig` and `ImportConfig` to `Action` and `ACTIONS` now:
- `"Export config"`, glyph `"⇧"`, ASCII `">"`
- `"Import config"`, glyph `"⇩"`, ASCII `"<"`

Move the existing label prompt flow to `pending_input`:

- `Action::Backup` sets `InputPrompt::BackupLabel` and `input_text` to `seed()`.
- `Action::ExportConfig` sets `InputPrompt::ExportConfig`, seeded with `~/termux-sync-config-export.json`.
- `Action::ImportConfig` sets `InputPrompt::ImportConfig`, seeded empty.
- `Action::DiskCheck` sets `InputPrompt::CheckTarget`, seeded empty. Remove the direct `check ~` task.
- `KeyCode::Enter` calls `submit_input`; `Esc` clears and sets `cancel_status`; `Backspace` pops; printable non-control chars append up to `max_len`.
- `submit_input`:
  - `BackupLabel`: `start_operation(Kind::Backup, "backup", backup_command(text.trim()))`
  - other prompts: `input_command(prompt, text)`; when `None`, set status `"enter a path"` and keep the prompt open; otherwise `start_task(Task::Command(args))` and clear the prompt.
- `input_text` is trimmed only when building the command. Leading/trailing spaces never reach argv.
- Ctrl+C clears `pending_input` and quits, matching the current label prompt.
- Import completion refreshes the dashboard: add `title.starts_with("import-config")` to the `poll_worker` refresh condition. Export does not refresh.

`input_command` rules:
- `BackupLabel` returns `None`
- `ExportConfig` and `ImportConfig` return `None` for an empty trimmed value
- `CheckTarget` always returns `check_command(trimmed)`
- Builders never invoke a shell.

Rename `render_backup_label` to `render_input_prompt`; use `prompt.title()` for the block title and `prompt.hint()` below the text. Render the same two clickable areas into `input_yes` / `input_no`. Mouse submit uses the same `submit_input` path.

- [ ] **Step 4: Run the app and UI tests**

Run: `cd tui && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tui/src/app.rs tui/src/ui.rs
git commit -S -m "feat(tui): add path prompts for config and disk check"
```

---

### Task 3: Clear logs

**Files:**
- Modify: `termux-sync.py` (`cmd_clear_logs` 2831-2853, dispatcher around 3308, `OPTIONS` 3171)
- Modify: `src/termux_sync/__init__.py` (byte-identical mirror)
- Modify: `tests/test_commands.py` (clear-logs tests 268-282)
- Modify: `tui/src/app.rs` (`Action`, `ACTIONS`, `run`, `execute_confirmed_action_with`, `poll_worker`)
- Modify: `tui/src/ui.rs` (`render_action_confirm` 220-233)
- Test: `tests/test_commands.py`, `tui/src/app.rs`, `tui/src/ui.rs`

**Interfaces:**
- Consumes: the delete and clear-cache `--yes` fallback pattern.
- Produces: `cmd_clear_logs(assume_yes: bool = False)`; `Action::ClearLogs`.

- [ ] **Step 1: Write the failing Python test**

Add to `tests/test_commands.py`:

```python
def test_clear_logs_assume_yes_skips_confirmation(config_home, answers, capsys):
    ts.log("INFO", "remove me")
    answers(confirm=False)
    ts.cmd_clear_logs(assume_yes=True)
    remaining = (config_home / "sync.log").read_text()
    assert "remove me" not in remaining
    assert "Log file cleared by user" in remaining
    assert "Log file cleared" in capsys.readouterr().out


def test_clear_logs_without_a_log_file_reports_nothing(config_home, capsys):
    (config_home / "sync.log").unlink(missing_ok=True)
    ts.cmd_clear_logs(assume_yes=True)
    assert "nothing to clear" in capsys.readouterr().out.lower()
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python -m pytest tests/test_commands.py::test_clear_logs_assume_yes_skips_confirmation -q`
Expected: FAIL — `unexpected keyword argument 'assume_yes'`.

- [ ] **Step 3: Implement the Python flag**

- Change `cmd_clear_logs()` to `cmd_clear_logs(assume_yes: bool = False)`.
- Change `if not Confirm.ask("  [bold]Clear all log entries?[/bold]", default=False):` to `if not assume_yes and not Confirm.ask(...)`.
- In the dispatcher, change the handler to `lambda: cmd_clear_logs(assume_yes=getattr(args, "yes", False))`.
- Update the `--yes` option text to `"Skip delete / clear-logs / clear-cache confirmations"`.
- Copy the file: `cp termux-sync.py src/termux_sync/__init__.py`.

- [ ] **Step 4: Run the Python checks**

Run: `python -m pytest tests/test_commands.py -q && ruff check src tests && cmp termux-sync.py src/termux_sync/__init__.py`
Expected: PASS and `cmp` prints nothing.

- [ ] **Step 5: Write the failing TUI test**

Add to `tui/src/app.rs` tests:

```rust
#[test]
fn clear_logs_confirms_inside_the_tui() {
    let mut app = App::with_snapshot(snapshot());
    app.run(Action::ClearLogs);
    assert_eq!(app.pending_confirm, Some(Action::ClearLogs));
}

#[test]
fn confirmed_clear_logs_falls_back_without_yes() {
    let mut app = App::with_snapshot(snapshot());
    app.execute_confirmed_action_with(Action::ClearLogs, false);
    assert_eq!(app.pending, Some(vec!["clear-logs".to_string()]));
}
```

- [ ] **Step 6: Implement the TUI action**

- Add `ClearLogs` to `Action` and to `ACTIONS` with label `"Clear logs"`, glyph `"⌫"`, ASCII `"-"`.
- `run(Action::ClearLogs)` sets `pending_confirm = Some(Action::ClearLogs)`.
- `execute_confirmed_action_with`:
  - with `--yes`: `start_task(Task::Command(vec!["clear-logs".to_string(), "--yes".to_string()]))`
  - without: `queue_command(vec!["clear-logs"])` and status `"termux-sync is too old for in-TUI clear-logs; using CLI confirmation"`.
- `render_action_confirm` adds:
  ```rust
  Action::ClearLogs => ("Clear logs", "Clear all log entries?".to_string()),
  ```
- `poll_worker` refresh condition adds `title.starts_with("clear-logs")`.

- [ ] **Step 7: Run the TUI tests and commit**

Run: `cd tui && cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked`
Expected: PASS.

```bash
git add termux-sync.py src/termux_sync/__init__.py tests/test_commands.py tui/src/app.rs tui/src/ui.rs
git commit -S -m "feat(tui): clear logs from the dashboard"
```

---

### Task 4: Status modal

**Files:**
- Modify: `tui/src/app.rs` (`Action`, `ACTIONS`, `App`, key handling, `run`)
- Modify: `tui/src/ui.rs` (draw overlays, new `render_status`)
- Test: `tui/src/app.rs`, `tui/src/ui.rs`

**Interfaces:**
- Consumes: `data::StatusReport`, `data::collect_status`.
- Produces:
  - `Action::Status`
  - App fields `show_status: bool`, `status_scroll: usize`, `status_report: Option<StatusReport>`
  - `pub fn open_status(&mut self)` and `pub fn open_status_report(&mut self, report: StatusReport)`

- [ ] **Step 1: Write the failing app test**

```rust
#[test]
fn status_report_opens_and_closes() {
    let mut app = App::with_snapshot(snapshot());
    let report = crate::data::StatusReport {
        storage: "github".into(),
        target: "me/backups @ main".into(),
        compression: "xz".into(),
        max_backups: 5,
        schedule: "daily 04:05".into(),
        cli_version: Some("termux-sync v1.3.9".into()),
        config_path: "~/.config/termux-sync/config.json".into(),
        log_path: "~/.config/termux-sync/sync.log".into(),
        history_path: "~/.config/termux-sync/tui-history.json".into(),
        tools: vec![crate::data::ToolStatus {
            name: "rclone",
            path: None,
            required: true,
        }],
    };
    app.open_status_report(report);
    assert!(app.show_status);
    app.handle_key(KeyCode::Esc);
    assert!(!app.show_status);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd tui && cargo test --locked status_report_opens_and_closes`
Expected: FAIL — missing fields and method.

- [ ] **Step 3: Implement the state**

- Add `Status` to `Action` and `ACTIONS`: label `"Status"`, glyph `"◉"`, ASCII `"i"`.
- Add the App fields and initialize them in `with_snapshot`.
- `open_status()` calls `data::collect_status(&self.snapshot.config)` and delegates to `open_status_report`.
- `open_status_report` stores the report, sets `show_status = true`, resets `status_scroll`.
- `run(Action::Status)` calls `open_status()`.
- Key handling, before operation handling:
  - `Esc`, `q`, `?` close
  - `Up`/`k`, `Down`/`j`, `PageUp`, `PageDown` change `status_scroll`
- Ctrl+C resets `show_status`.
- Mouse input while the modal is open does nothing.

- [ ] **Step 4: Write the failing render test**

Add a `render_with` helper in `tui/src/ui.rs` tests:

```rust
fn render_with(app: &mut App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| draw(frame, app))
        .expect("draw succeeds");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}
```

Then make the existing `render_at` helper call `render_with` so both use one draw path, and add:

```rust
#[test]
fn status_modal_shows_backend_target_and_missing_tool() {
    let mut app = app();
    app.open_status_report(crate::data::StatusReport {
        storage: "github".into(),
        target: "me/backups @ main".into(),
        compression: "xz".into(),
        max_backups: 5,
        schedule: "daily 04:05".into(),
        cli_version: Some("termux-sync v1.3.9".into()),
        config_path: "~/.config/termux-sync/config.json".into(),
        log_path: "~/.config/termux-sync/sync.log".into(),
        history_path: "~/.config/termux-sync/tui-history.json".into(),
        tools: vec![crate::data::ToolStatus {
            name: "rclone",
            path: None,
            required: true,
        }],
    });
    let frame = render_with(&mut app, 100, 30);
    assert!(frame.contains("STATUS"));
    assert!(frame.contains("GITHUB"));
    assert!(frame.contains("me/backups @ main"));
    assert!(frame.contains("rclone"));
    assert!(frame.contains("missing"));
}
```

- [ ] **Step 5: Implement the render**

Add `render_status` in `tui/src/ui.rs`, called from `draw` after history and before the task overlay. Use a centered popup clamped to `24..72` wide and `10..24` high. Render a `Paragraph` with `Wrap { trim: false }` and a `scrollbar`.

Lines, in order:

```text
STATUS
Storage      GITHUB
Target       me/backups @ main
Compression  XZ · keep 5
Auto-backup  daily 04:05
Versions     TUI {env!("CARGO_PKG_VERSION")} · CLI {cli_version or "not found"}
Config       ~/.config/termux-sync/config.json
Log          ~/.config/termux-sync/sync.log
History      ~/.config/termux-sync/tui-history.json

TOOLS
python3      /path or missing
dpkg         /path or missing
tar          /path or missing
rclone       /path or missing (required only for Google Drive)
git          /path or missing
curl         /path or missing
```

When `cli_version` is `None`, show `CLI not found`. Color required missing tools with `DANGER`, optional missing with `MUTED`, present tools with `SUCCESS`.

- [ ] **Step 6: Run tests and commit**

Run: `cd tui && cargo test --locked`
Expected: PASS.

```bash
git add tui/src/app.rs tui/src/ui.rs
git commit -S -m "feat(tui): add status view for config and tools"
```

---

### Task 5: Daemon status view

**Files:**
- Modify: `tui/src/data.rs` (daemon helpers and tests)
- Modify: `tui/src/app.rs` (`Action`, `ACTIONS`, `App`, key handling, `run`)
- Modify: `tui/src/ui.rs` (draw overlays, new `render_daemon`)
- Test: `tui/src/data.rs`, `tui/src/app.rs`, `tui/src/ui.rs`

**Interfaces:**
- Consumes: `Config.schedule`, `tail_log`, `home`.
- Produces:
  - `pub fn boot_script_path() -> PathBuf`
  - `pub fn daemon_activity(lines: &[String]) -> Vec<String>`
  - `Action::Daemon`
  - App fields `show_daemon: bool`, `daemon_scroll: usize`, `daemon_log: Vec<String>`, `daemon_boot_present: bool`
  - `pub fn open_daemon(&mut self)`

- [ ] **Step 1: Write the failing data test**

```rust
#[test]
fn daemon_activity_keeps_only_daemon_lines_and_caps_them() {
    let lines: Vec<String> = (0..10)
        .map(|index| format!("line {index} [INFO] Daemon tick {index}"))
        .chain(std::iter::once("plain line".to_string()))
        .collect();
    let activity = daemon_activity(&lines);
    assert_eq!(activity.len(), 5);
    assert!(activity.iter().all(|line| line.contains("Daemon")));
    assert_eq!(activity.last().unwrap(), "line 9 [INFO] Daemon tick 9");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd tui && cargo test --locked daemon_activity_keeps_only_daemon_lines_and_caps_them`
Expected: FAIL — missing function.

- [ ] **Step 3: Implement the data helpers**

- `boot_script_path()` returns `home().join(".termux/boot/termux-sync-daemon.sh")`.
- `daemon_activity` keeps lines containing any of: `"Daemon"`, `"Next backup"`, `"Auto-backup triggered"`, `"Daemon backup failed"`, `"auto-backup was disabled"`. Return the last 5 in original order.

- [ ] **Step 4: Write the failing app test**

```rust
#[test]
fn daemon_action_opens_a_status_view() {
    let mut app = App::with_snapshot(snapshot());
    app.run(Action::Daemon);
    assert!(app.show_daemon);
    app.handle_key(KeyCode::Esc);
    assert!(!app.show_daemon);
}
```

- [ ] **Step 5: Implement the state**

- Add `Daemon` to `Action` and `ACTIONS`: label `"Daemon"`, glyph `"◌"`, ASCII `"d"`.
- `open_daemon` sets `daemon_log = data::daemon_activity(&data::tail_log(500))`, `daemon_boot_present = data::boot_script_path().is_file()`, `show_daemon = true`, resets scroll.
- Key handling mirrors the status modal.
- Ctrl+C resets `show_daemon`.
- `run(Action::Daemon)` calls `open_daemon()`.

- [ ] **Step 6: Write the failing render test**

```rust
#[test]
fn daemon_modal_states_it_does_not_control_the_process() {
    let mut app = app();
    app.open_daemon();
    let frame = render_with(&mut app, 100, 30);
    assert!(frame.contains("DAEMON"));
    assert!(frame.contains("termux-sync daemon &"));
    assert!(frame.contains("does not start or stop"));
}
```

- [ ] **Step 7: Implement the render**

`render_daemon` uses the same popup pattern as status. Lines:

```text
DAEMON
Auto-backup     ENABLED daily 04:05
Boot script     present ~/.termux/boot/termux-sync-daemon.sh
                or missing, with the install hint
Running state   not detectable from the TUI

Start manually:
termux-sync daemon &
Or install Termux:Boot so the boot script starts it.

RECENT ACTIVITY
<last 5 activity lines, or "no daemon activity in sync.log">

The TUI does not start or stop the daemon.
```

- [ ] **Step 8: Run tests and commit**

Run: `cd tui && cargo test --locked`
Expected: PASS.

```bash
git add tui/src/data.rs tui/src/app.rs tui/src/ui.rs
git commit -S -m "feat(tui): show daemon schedule and recent activity"
```

---

### Task 6: Log line count

**Files:**
- Modify: `tui/src/app.rs` (`App`, `run(Action::Logs)`, `apply_snapshot`, `poll_worker`, output key handling)
- Modify: `tui/src/ui.rs` (`render_output` 1371-1390, help text, `main.rs` static help)
- Test: `tui/src/app.rs`, `tui/src/ui.rs`

**Interfaces:**
- Consumes: `data::tail_log`.
- Produces:
  - `pub enum OutputSource { Log, Command }`
  - `pub fn stepped_log_lines(current: usize, delta: isize) -> usize`
  - App fields `log_lines: usize` (default `200`), `output_source: OutputSource`

- [ ] **Step 1: Write the failing app test**

```rust
#[test]
fn log_line_count_clamps_and_steps() {
    assert_eq!(stepped_log_lines(40, -200), 40);
    assert_eq!(stepped_log_lines(200, 200), 400);
    assert_eq!(stepped_log_lines(2000, 200), 2000);
}

#[test]
fn bracket_keys_change_the_log_line_count() {
    let mut app = App::with_snapshot(snapshot());
    app.focus = Focus::Output;
    app.handle_key(KeyCode::Char(']'));
    assert_eq!(app.log_lines, 400);
    app.handle_key(KeyCode::Char('['));
    assert_eq!(app.log_lines, 200);
}
```

- [ ] **Step 2: Implement the state**

- Default `log_lines` to `200`, `output_source` to `OutputSource::Log`.
- `stepped_log_lines` clamps to `40..=2000` with a step of `200`.
- `Action::Logs` sets `output_source = Log`, reloads `output = data::tail_log(self.log_lines)`.
- `apply_snapshot` sets `output_source = Log`.
- Worker output and errors set `output_source = Command`.
- In `handle_output_key`, `[` and `]` call a new `adjust_log_lines(delta)`:
  - update `log_lines` through `stepped_log_lines`
  - when `output_source == Log`, reload `output` and reset `output_offset`
  - status becomes `format!("log {} lines", self.log_lines)`

- [ ] **Step 3: Write the failing render test**

```rust
#[test]
fn output_title_shows_the_log_line_count() {
    let frame = render_at(100, 30);
    assert!(frame.contains("log 200"));
}
```

- [ ] **Step 4: Implement the render and help text**

- `render_output` passes `Some(format!("log {}", app.log_lines))` as the pane detail when `app.output_source == OutputSource::Log`; use `Some("command".to_string())` otherwise. Keep the title `"Output"`.
- Help adds: `("[ / ]", "fewer / more log lines")`.
- `tui/src/main.rs` static `--help` adds the same line.

- [ ] **Step 5: Run tests and commit**

Run: `cd tui && cargo test --locked`
Expected: PASS.

```bash
git add tui/src/app.rs tui/src/ui.rs tui/src/main.rs
git commit -S -m "feat(tui): adjust the log tail length from the output pane"
```

---

### Task 7: Help, docs, and PTY smoke

**Files:**
- Modify: `tui/src/ui.rs` (`render_help` 1610-1686)
- Modify: `tui/src/main.rs` (`print_help`)
- Modify: `tui/README.md`
- Modify: `tui/RUNBOOK.md`
- Modify: `tui/tests/pty.sh`
- Test: `tui/src/ui.rs`, `tui/tests/pty.sh`

**Interfaces:**
- Consumes: all actions from Tasks 1-6.
- Produces: documented keys and fallbacks.

- [ ] **Step 1: Update help text**

Add to `render_help` and `print_help`:
- `s` not needed; the actions pane lists every action.
- `[ / ]` changes log line count.
- Action summary includes: clear logs, status, export config, import config, daemon.
- Note that setup and schedule still use the handoff card.

- [ ] **Step 2: Add the failing UI assertion**

```rust
#[test]
fn help_lists_parity_actions() {
    let mut app = app();
    app.open_help();
    let frame = render_with(&mut app, 100, 30);
    assert!(frame.contains("Clear logs"));
    assert!(frame.contains("Export config"));
    assert!(frame.contains("Daemon"));
}
```

- [ ] **Step 3: Update `tui/README.md`**

Add to "What it shows" and "Keys":
- Status: storage target, compression, limits, schedule, CLI version, paths, tool checks.
- Clear logs: TUI confirm, CLI `--yes`, fallback to terminal prompt on old CLI.
- Export/import config: path prompt; export is redacted by the CLI.
- Disk check: blank means the default overview; typing a path checks that path.
- Daemon: schedule, boot script presence, recent activity, manual start command. The TUI never starts or stops it.
- `[` / `]` adjust the log tail from 40 to 2000 lines.
- Remove "export/import" from the missing list, if mentioned.

Update "Safety notes": status reads only display fields; repo and branch are shown, tokens are not.

- [ ] **Step 4: Update `tui/RUNBOOK.md`**

Add:
- Clear logs fallback: old CLI without `--yes` hands the terminal to `termux-sync clear-logs` and the CLI asks its own question.
- Clear logs with no log file: the CLI reports nothing to clear; the TUI does not create an empty log.
- Status modal: CLI version shows `not found` when the binary is missing; tool rows show missing required tools.
- Daemon view: enabled schedule is not proof of a running daemon; use `pgrep -f "termux-sync daemon"` to check manually.

- [ ] **Step 5: Add a PTY smoke case**

Append to `tui/tests/pty.sh` a case that opens Status without depending on action-list order. Add a global shortcut in `handle_key_with`:

```rust
KeyCode::Char('s') => self.run(Action::Status),
```

Place it next to the `L` and `h` shortcuts. Then:

```bash
feed_status='sleep 1; printf "s"; sleep 0.5; printf "\033"; sleep 0.3; printf "q"'
run_case status status "$feed_status"
plain "$TMP/status.out" | tr -d '\n' | grep -q "STATUS"
printf 'status: assertions passed\n'
```

Add `status)` to `pty_fake.sh` only if needed; the status view is native and does not call the fake CLI.

- [ ] **Step 6: Run the full TUI gate**

Run:

```bash
cd tui
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
bash tests/pty.sh
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add tui/src/ui.rs tui/src/main.rs tui/README.md tui/RUNBOOK.md tui/tests/pty.sh
git commit -S -m "docs(tui): document CLI parity actions and fallbacks"
```

---

### Task 8: Final integration pass

**Files:**
- Modify: none expected.

- [ ] **Step 1: Run the Python gate**

```bash
python -m pytest -q
ruff check src tests
cmp termux-sync.py src/termux_sync/__init__.py
```

Expected: PASS and `cmp` prints nothing.

- [ ] **Step 2: Run the Rust gate**

```bash
cd tui
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
bash tests/pty.sh
```

Expected: PASS.

- [ ] **Step 3: Manual TUI smoke**

```bash
cd tui
TMP=$(mktemp -d)
mkdir -p "$TMP/.config/termux-sync" "$TMP/backups/termux_backup_test"
printf '{"storage":"local","local_path":"%s/backups","compression":"gz","max_backups":5}\n' "$TMP" \
  > "$TMP/.config/termux-sync/config.json"
printf '{"name":"termux_backup_test","date":"2026-01-01T12:00:00","label":"smoke","packages":["git"],"files":{"home.tar.gz":{"size":1024}},"version":"1.3.9","compression":"gz"}\n' \
  > "$TMP/backups/termux_backup_test/manifest.json"
printf '[2026-01-01 12:00] [INFO] Daemon started\n' > "$TMP/.config/termux-sync/sync.log"
HOME="$TMP" TERMUX_SYNC_BIN="$PWD/tests/pty_fake.sh" ./target/release/termux-sync-tui
rm -rf "$TMP"
```

Check by hand:
- Status shows storage target, tool rows, and `CLI not found` when the fake has no version output.
- Export prompt is prefilled and Esc cancels.
- Import with a blank path keeps the prompt open.
- Clear logs shows the confirmation card.
- Daemon shows `DAEMON`, schedule state, and `termux-sync daemon &`.
- `[` / `]` changes the Output title between 40 and 2000.
- Layout still works at `48×12`, and `--ascii` keeps icons readable.

- [ ] **Step 4: Commit any final fixes**

```bash
git add -A
git commit -S -m "fix(tui): final parity review findings"
```

Skip this commit when the tree is clean.

---

## Deferred: Release Gaps

These stay out of this plan and are covered last:

- Prebuilt aarch64/armv7 binaries and a `.deb`. CI only cross-compiles today.
- Version compatibility matrix for TUI `0.1.0`, Python CLI `1.3.9`, and event protocol `v1`.
- Merge to `main` or opening a PR. Branch `spike/rust-tui` stays separate.

## Explicit Non-Goals

- Native setup and schedule forms. They stay handoff cards.
- Starting, stopping, supervising, or restarting the daemon from the TUI.
- Restore cancellation after extraction starts.
- Event protocol changes beyond version `1`.
- Background polling of remote storage. Remote lists stay on demand with `L`.
- Changing backup, restore, or delete behavior.
