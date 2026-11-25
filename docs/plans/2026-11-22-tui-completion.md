# TUI Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the remaining remote-storage, verification, navigation, distribution and test gaps in the Rust TUI without dropping the existing backup/restore behavior.

**Architecture:** Extend the existing JSON event protocol for list and remote metadata, stream backend progress through the same event writer, keep backup logic in Python, and add TUI-only state for filtering, history, labels and a context-preserving handoff screen. Tests stay split between Python protocol tests, Rust state/render tests, and a small PTY suite.

**Tech Stack:** Python 3.10+, Rust 1.85+, Ratatui 0.30, Crossterm, serde_json, libc, unicode-width, GitHub Actions, Termux/Android.

**Spec:**
- `docs/specs/2026-11-21-operations-cockpit-design.md`
- `docs/specs/2026-11-21-restore-cockpit-design.md`

## Global Constraints

- `termux-sync.py` and `src/termux_sync/__init__.py` must remain byte-identical.
- `--json-events` remains additive; existing CLI output/behavior must not change without the flag.
- Event protocol stays version `1`. Unknown event types are ignored; newer major versions fail with a clear message.
- Never emit GitHub tokens, encryption passwords, or config secrets. Every event string goes through `EventWriter._redact`.
- Backup and restore logic stays in Python. Rust only reads manifests/config/log and drives the CLI.
- Restore checksums must pass before extraction. Restore cancel stays refused after the first `extract:*` stage starts.
- TUI remains usable at `48×12`; header test baseline is `80×24` and `56×24`.
- Every commit is GPG-signed (`git commit -S`).
- Python checks: `pytest -q`, `ruff check src tests`, `cmp termux-sync.py src/termux_sync/__init__.py`.
- Rust checks: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, `cargo build --locked --release`.

## Review Focus

- An installed `termux-sync` without `--yes` or `--json-events`: the TUI must explain and fall back safely instead of running a command that immediately fails.
- A remote backend returning an empty list, a malformed manifest, or a backup name with a path separator: no unsafe selection, no restore target derived from it.
- A manifest-only fetch that fails: restore must fall back to download before showing the plan, never extract before checksum verification.
- Local checksums computed during copy: a truncated or mismatched copy must never report `VERIFIED`.
- The 56-column header with `GITHUB`, a backup count and `WARN`: all must remain visible and readable; the TUI must not hide the warning or count.

---

## File Structure

- `termux-sync.py` — event `list`, `fetch_manifest`, rclone/GitHub progress, checksum-on-copy, `--yes` already present.
- `src/termux_sync/__init__.py` — mirror.
- `tests/test_events.py` — protocol tests; add list/manifest/progress/checksum tests.
- `tests/test_storage.py` — backend/mock tests where existing patterns fit.
- `tui/src/ops.rs` — event parsing, list collection, capability probes, remote progress state.
- `tui/src/data.rs` — backup metadata helpers, history path, verification helper wrappers.
- `tui/src/app.rs` — action flow, label prompt, filter, history, handoff, touch hit areas.
- `tui/src/ui.rs` — label/filter/history/handoff views, scrollbars, header guard.
- `tui/src/history.rs` — persisted operation history.
- `tui/install.sh` — source install into `$PREFIX/bin`.
- `tui/tests/pty.sh` — PTY smoke harness with a fake `termux-sync`.
- `tui/tests/pty_fake.sh` — fake event emitter used by the harness.
- `.github/workflows/tui.yml` — add Android cross-check and PTY/install checks.
- `tui/RUNBOOK.md` — protected restore, cancel timeout, panic, recovery.

---

## Task 1: Capability probes and safe fallbacks

**Files:**
- Modify: `tui/src/ops.rs`
- Modify: `tui/src/app.rs`
- Test: `tui/src/ops.rs` tests, `tui/src/app.rs` tests

**Interfaces:**
- Produces:
  - `pub fn supports_flag(flag: &str) -> bool`
  - `pub fn supports_events() -> bool` (calls `supports_flag("--json-events")`)
  - `pub fn supports_yes() -> bool` (calls `supports_flag("--yes")`)
  - `fn execute_confirmed_action_with(&mut self, action: Action, yes_supported: bool)`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn confirmed_delete_falls_back_to_handoff_without_yes() {
    let mut app = App::with_snapshot(snapshot());
    app.execute_confirmed_action_with(Action::Delete, false);
    assert!(app.pending.as_ref().is_some_and(|args| args[0] == "delete"));
    assert!(!app.pending.unwrap().contains(&"--yes".to_string()));
}
```

- [ ] **Step 2: Run it**

Run: `cargo test --locked confirmed_delete_falls_back_to_handoff_without_yes`
Expected: FAIL — method not found.

- [ ] **Step 3: Implement**

`supports_flag` runs `termux-sync <flag> --version` with stdin/stdout/stderr null and returns success. `execute_confirmed_action` calls `execute_confirmed_action_with(action, supports_yes())`. In the false branch, queue the handoff command without `--yes`; the CLI asks its own confirmation. Set status: `"termux-sync is too old for in-TUI delete; using CLI confirmation"`.

- [ ] **Step 4: Run the test**

Run: `cargo test --locked confirmed_delete_falls_back_to_handoff_without_yes`
Expected: PASS.

- [ ] **Step 5: Guard clear-cache too**

Add the same parameter to the `Action::ClearCache` branch. Extend the test to assert `pending == ["clear-cache"]` without `--yes`.

- [ ] **Step 6: Run all Rust checks**

Run: `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add tui/src/ops.rs tui/src/app.rs
git commit -S -m "fix(tui): probe --yes support before captured delete"
```

---

## Task 2: Structured remote backup listing

**Files:**
- Modify: `termux-sync.py`, `src/termux_sync/__init__.py`
- Modify: `tui/src/ops.rs`, `tui/src/app.rs`, `tui/src/ui.rs`
- Test: `tests/test_events.py`, `tui/src/ops.rs`, `tui/src/app.rs`

**Interfaces:**
- Python events:
  - `{"event":"backup","name":str,"date":str,"label":str,"packages":int,"size":int,"version":str,"compression":str,"storage":str}`
  - final `{"event":"result","ok":true,"count":int}`
- Rust:
  - `Event::Backup(data::Backup)`
  - `Task::ListBackups`
  - `WorkerMsg::Backups(Vec<data::Backup>)`
  - `ops::collect_backups() -> Result<Vec<data::Backup>, String>`
  - `fn apply_backup_list(&mut self, backups: Vec<Backup>)`

- [ ] **Step 1: Write the failing Python test**

```python
def test_list_emits_backup_events(events, local_cfg, termux_tree):
    make_backup(local_cfg)
    events.seek(0); events.truncate(0)
    ts.cmd_list(local_cfg)
    items = [e for e in parsed(events) if e["event"] == "backup"]
    assert len(items) == 1
    assert items[0]["name"].startswith("termux_backup_")
    assert items[0]["packages"] == 2
    assert items[0]["storage"] == "local"
```

Run: `pytest tests/test_events.py::test_list_emits_backup_events -v`
Expected: FAIL — no backup events.

- [ ] **Step 2: Implement list events**

In `cmd_list`, when `EVENTS` is active, emit one `backup` event per manifest with `size=sum(v["size"] for v in files.values())`, then `result(count=len(backups))`. Keep the Rich table on stderr.

- [ ] **Step 3: Sync Python copies and rerun**

Run: `cp termux-sync.py src/termux_sync/__init__.py && pytest tests/test_events.py::test_list_emits_backup_events -v`
Expected: PASS.

- [ ] **Step 4: Write the failing Rust parser test**

```rust
#[test]
fn parses_backup_list_event() {
    let event = parse_event(r#"{"v":1,"event":"backup","name":"b1","date":"2026-01-01T00:00:00","label":"x","packages":3,"size":10,"version":"1.3.9","compression":"gz","storage":"github"}"#).unwrap();
    assert!(matches!(event, Event::Backup(ref backup) if backup.name == "b1" && backup.packages == 3));
}
```

Run: `cargo test --locked parses_backup_list_event`
Expected: FAIL — variant missing.

- [ ] **Step 5: Implement parser and collector**

Parse into `data::Backup { name, date, label, packages, size, version, compression, modified: None }`. `collect_backups` spawns `termux-sync list --json-events`, reads stdout lines, parses `Event::Backup`, stops on `Result`, returns an error if the child exits non-zero or no result arrives.

- [ ] **Step 6: Replace text capture**

Change `Action::List` to `start_task(Task::ListBackups)`. On `WorkerMsg::Backups`, replace `snapshot.backups`, clear `snapshot.warning`, select index 0, and focus Backups. Remove the `"terminal"` wording from remote warnings.

- [ ] **Step 7: Write the failing UI test**

```rust
#[test]
fn remote_warning_does_not_claim_list_is_outside() {
    let mut app = app();
    app.snapshot.config.storage = "github".into();
    app.snapshot.warning = Some("GITHUB storage: press L to list backups in the TUI".into());
    let frame = render_app(&mut app, 80, 24);
    assert!(!frame.contains("in the terminal"));
}
```

- [ ] **Step 8: Run tests and commit**

Run: `pytest tests/test_events.py -q && cargo test --locked && cargo clippy --locked --all-targets -- -D warnings`
Expected: pass.

```bash
git add termux-sync.py src/termux_sync/__init__.py tests/test_events.py tui
git commit -S -m "feat(tui): list remote backups through JSON events"
```

---

## Task 3: Manifest-only restore preview

**Files:**
- Modify: `termux-sync.py`, `src/termux_sync/__init__.py`
- Test: `tests/test_events.py`

**Interfaces:**
- `Storage.fetch_manifest(self, backup_name: str) -> Optional[dict]`
- `_cmd_restore_events` metadata order: `fetch_manifest`; if `None`, emit `download` stage, download, read manifest from temp.

- [ ] **Step 1: Write the failing tests**

```python
def test_local_manifest_fetch_does_not_copy_archives(local_cfg, termux_tree, monkeypatch):
    manifest = make_backup(local_cfg)
    storage = ts.LocalStorage(local_cfg)
    monkeypatch.setattr(storage, "download", lambda *a, **k: (_ for _ in ()).throw(AssertionError("download called")))
    assert storage.fetch_manifest(manifest["name"])["name"] == manifest["name"]

def test_gdrive_manifest_fetch_uses_rclone_cat(monkeypatch):
    calls = []
    def fake_run(args, **kwargs):
        calls.append(args)
        return type("R", (), {"stdout": '{"name":"b1"}'})()
    monkeypatch.setattr(ts.subprocess, "run", fake_run)
    monkeypatch.setattr(ts.shutil, "which", lambda name: "/bin/rclone")
    storage = ts.GDriveStorage({"gdrive_folder_id": ""})
    assert storage.fetch_manifest("b1")["name"] == "b1"
    assert any("cat" in call for call in calls)
```

- [ ] **Step 2: Run them**

Run: `pytest tests/test_events.py -k manifest_fetch -v`
Expected: FAIL — no `fetch_manifest`.

- [ ] **Step 3: Implement per backend**

- `LocalStorage`: `read_manifest(self.root / _clean_backup_name(name))`.
- `GDriveStorage`: `rclone cat <remote>/<name>/manifest.json`, parse JSON, return `None` on failure.
- `GitHubStorage`: reuse `self.list_backups()` and return the matching entry; return `None` if absent.
- Base `get_storage` objects do not need a default method; use `getattr(storage, "fetch_manifest", None)`.

- [ ] **Step 4: Wire the restore metadata stage**

In `_cmd_restore_events`, call `fetch_manifest` first. If it returns a manifest, mark metadata complete without download. Only when it returns `None` emit the download stage and download. The existing local path stays as the fast path.

- [ ] **Step 5: Run tests**

Run: `pytest tests/test_events.py -q`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add termux-sync.py src/termux_sync/__init__.py tests/test_events.py
git commit -S -m "feat: fetch restore manifests before downloading archives"
```

---

## Task 4: Remote byte progress

**Files:**
- Modify: `termux-sync.py`, `src/termux_sync/__init__.py`
- Test: `tests/test_events.py`, `tests/test_storage.py`

**Interfaces:**
- `_rclone_progress(args: list[str], stage: str) -> None` runs rclone with `--stats 500ms --stats-one-line`, reads stderr lines, emits `progress`.
- GitHub upload/download emit `progress` from `EventProgress(stage)`.
- TUI needs no new parser; existing `Event::Progress` handles it.

- [ ] **Step 1: Write the failing rclone test**

```python
def test_rclone_progress_emits_bytes(events, monkeypatch):
    class Fake:
        stderr = iter(["Transferred: 5 MiB / 10 MiB, 50%"])
        returncode = 0
        def wait(self): return 0
    monkeypatch.setattr(ts.subprocess, "Popen", lambda *a, **k: Fake())
    ts._rclone_progress(["rclone", "copy", "a", "b"], "download")
    progress = [e for e in parsed(events) if e["event"] == "progress"]
    assert progress[-1]["current"] == 5 * 1024 * 1024
    assert progress[-1]["total"] == 10 * 1024 * 1024
```

- [ ] **Step 2: Run it**

Run: `pytest tests/test_events.py::test_rclone_progress_emits_bytes -v`
Expected: FAIL — helper missing.

- [ ] **Step 3: Implement the parser**

Add `_bytes_from_rclone(value: str, unit: str) -> int`; units `B`, `KiB`, `MiB`, `GiB`, `TiB`. `_rclone_progress` uses `Popen(..., stderr=PIPE, text=True)`, parses `Transferred: X / Y`, emits only when values change or 500ms elapsed, waits and raises on non-zero.

- [ ] **Step 4: Wire Google Drive**

Replace `subprocess.run(["rclone","copy",...,"--progress"], capture_output=True)` in upload/download with `_rclone_progress([... "--stats","500ms","--stats-one-line"], "upload"|"download")`.

- [ ] **Step 5: Wire GitHub**

- Upload: in `_split_file`/asset upload loop, emit `progress(stage="upload", current=bytes_written, total=part_size, unit="bytes", detail=chunk.name)` at most every 500ms.
- Download: in the retry read loop, emit `progress(stage="download", current=done_bytes, total=total_bytes, unit="bytes")` after each chunk.

- [ ] **Step 6: Add tests**

- `_bytes_from_rclone` table test for B/KiB/MiB.
- Mock `_upload_one_asset` to write bytes and assert progress events in increasing order.
- Existing download test with a fake loop asserts one progress event.

- [ ] **Step 7: Run and commit**

Run: `pytest tests/test_events.py tests/test_storage.py -q`
Expected: PASS.

```bash
git add termux-sync.py src/termux_sync/__init__.py tests
git commit -S -m "feat: emit remote byte progress in event mode"
```

---

## Task 5: Backup label prompt

**Files:**
- Modify: `tui/src/app.rs`, `tui/src/ui.rs`
- Test: `tui/src/app.rs`, `tui/src/ui.rs`

**Interfaces:**
- `pub fn backup_command(label: &str) -> Vec<String>` returns `["backup"]` or `["backup","--label",label]`.
- `App.pending_label: Option<String>`
- `App.label_text: String`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn backup_command_includes_clean_label() {
    assert_eq!(backup_command("before-upgrade"), vec!["backup","--label","before-upgrade"]);
    assert_eq!(backup_command(""), vec!["backup"]);
}

#[test]
fn backup_action_opens_label_prompt() {
    let mut app = App::with_snapshot(snapshot());
    app.run(Action::Backup);
    assert!(app.pending_label.is_some());
    assert!(app.operation.is_none());
}
```

- [ ] **Step 2: Run**

Run: `cargo test --locked backup_command_includes_clean_label`
Expected: FAIL.

- [ ] **Step 3: Implement prompt state**

`Action::Backup` sets `pending_label = Some(String::new())` instead of starting directly. `handle_key_with` before operation handling:
- printable chars append, max 64;
- Backspace pops;
- Enter starts `start_operation(Kind::Backup, "backup", backup_command(&text))` and clears;
- Esc cancels.

- [ ] **Step 4: Render modal**

Add a centered `Backup label` card with input and `Enter start · Esc cancel`. Use `Clear`, `op_block`, and the same focus styling. Include the modal in `draw` after task overlays.

- [ ] **Step 5: Mouse**

Click start/cancel areas like the action confirm dialog. Store `label_yes`/`label_no` rects in App.

- [ ] **Step 6: Tests and commit**

Add render test at 100×30 and 48×12 asserting the input line and hint.

Run: `cargo test --locked && cargo clippy --locked --all-targets -- -D warnings`

```bash
git add tui/src/app.rs tui/src/ui.rs
git commit -S -m "feat(tui): ask for a backup label before starting"
```

---

## Task 6: Verification without rehashing local archives

**Files:**
- Modify: `termux-sync.py`, `src/termux_sync/__init__.py`
- Test: `tests/test_events.py`, `tests/test_storage.py`

**Interfaces:**
- `LocalStorage.stored_checksums: dict[str, str]`
- `_copy_with_sha(src: Path, dst: Path) -> str`
- `_verify_local_backup(root: Path, files: dict, precomputed: dict[str,str] | None) -> tuple[bool, str]`

- [ ] **Step 1: Write failing test**

```python
def test_local_upload_records_checksums_and_verifies_without_rehash(tmp_path, monkeypatch):
    src = tmp_path / "src"; src.mkdir(); (src / "a.tar.gz").write_bytes(b"abc")
    dest = tmp_path / "dest"
    storage = ts.LocalStorage({"local_path": str(dest)})
    with ts.make_progress() as progress:
        storage.upload(src, "b1", progress)
    assert storage.stored_checksums["a.tar.gz"] == ts.hashlib.sha256(b"abc").hexdigest()

    monkeypatch.setattr(ts, "sha256", lambda *a, **k: (_ for _ in ()).throw(AssertionError("rehash")))
    ok, details = ts._verify_local_backup(dest / "b1", {"home": {"archive": "a.tar.gz", "checksum": storage.stored_checksums["a.tar.gz"]}}, storage.stored_checksums)
    assert ok and details == "local checksums match"
```

- [ ] **Step 2: Run**

Run: `pytest tests/test_events.py::test_local_upload_records_checksums_and_verifies_without_rehash -v`
Expected: FAIL.

- [ ] **Step 3: Implement copy-with-hash**

Copy in 1 MiB chunks with `hashlib.sha256`, update `stored_checksums[f.name]`. Use it in `LocalStorage.upload`. Keep `shutil.copy2` metadata by `copystat` after writing.

- [ ] **Step 4: Implement verification helper**

`_verify_local_backup` checks each expected archive exists and, when `precomputed` has a value, compares it; otherwise calls `sha256` once. Return `(ok, details)`.

- [ ] **Step 5: Wire backup verify**

In `cmd_backup`, call `_verify_local_backup(Path(location), meta["files"], getattr(storage, "stored_checksums", None))`. Emit one progress event per archive.

- [ ] **Step 6: Tests and commit**

Add test for a corrupt stored file with precomputed checksum (returns False) and missing file (`False`, details names the archive).

Run: `pytest tests/test_events.py tests/test_storage.py -q`

```bash
git add termux-sync.py src/termux_sync/__init__.py tests
git commit -S -m "perf: checksum local backups while copying"
```

---

## Task 7: Backup search and filter

**Files:**
- Modify: `tui/src/app.rs`, `tui/src/ui.rs`
- Test: `tui/src/app.rs`, `tui/src/ui.rs`

**Interfaces:**
- `App.backup_filter: String`
- `App.backup_filtering: bool`
- `pub fn filtered_backups(&self) -> Vec<usize>`
- `selected_backup()` maps through `filtered_backups`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn backup_filter_matches_name_and_label() {
    let mut app = App::with_snapshot(snapshot());
    app.backup_filter = "upgrade".into();
    assert_eq!(app.filtered_backups(), vec![1]);
    app.backup_filter = "backup-one".into();
    assert_eq!(app.filtered_backups(), vec![0]);
}
```

- [ ] **Step 2: Run**

Run: `cargo test --locked backup_filter_matches_name_and_label`
Expected: FAIL.

- [ ] **Step 3: Implement state**

In Backups focus, `/` sets filtering and clears the filter. Chars append, Backspace pops, Esc exits filtering, Enter leaves filtering. Filter is case-insensitive over `name`, `label`, `date`. Reset selected to first filtered index when the filter changes; clamp.

- [ ] **Step 4: Render**

Show `/ filter: ...` line inside the Backups pane title or first row. Show `N/M` count. Table rows use filtered indices. Empty result shows `No backup matches`.

- [ ] **Step 5: Enter restore uses the mapped backup**

Add test: filter to index 1, press Enter, assert restore command uses `backup-two`.

- [ ] **Step 6: Commit**

```bash
git add tui/src/app.rs tui/src/ui.rs
git commit -S -m "feat(tui): filter backups by name, label and date"
```

---

## Task 8: Operation history

**Files:**
- Create: `tui/src/history.rs`
- Modify: `tui/src/main.rs`, `tui/src/app.rs`, `tui/src/ui.rs`, `tui/src/data.rs`
- Test: `tui/src/history.rs`, `tui/src/app.rs`

**Interfaces:**
- `HistoryEntry { kind: String, name: String, status: String, size: u64, duration_ms: u64, verified: Option<bool>, at: u64 }`
- `HistoryStore::load(path: &Path) -> Vec<HistoryEntry>`
- `HistoryStore::save(path: &Path, entries: &[HistoryEntry]) -> io::Result<()>`
- `data::history_path() -> PathBuf` = `config_dir()/tui-history.json`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn history_keeps_latest_twenty() {
    let dir = std::env::temp_dir().join(format!("ts-history-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("history.json");
    let entries: Vec<_> = (0..25).map(|n| sample_entry(n)).collect();
    HistoryStore::save(&path, &entries).unwrap();
    let loaded = HistoryStore::load(&path);
    assert_eq!(loaded.len(), 20);
    assert_eq!(loaded[0].name, "entry-24");
    std::fs::remove_dir_all(dir).ok();
}
```

- [ ] **Step 2: Implement store**

Load ignores missing/corrupt JSON and returns empty. Save writes a temp file in the same directory then renames. Keep newest first, cap 20.

- [ ] **Step 3: Append on operation finish**

In `App::poll_operation`, when `finished` becomes true, append an entry with kind, name, result/status, size, duration, verified. Save best-effort; set status warning if save fails.

- [ ] **Step 4: History view**

`h` on dashboard toggles a centered `History` overlay listing up to 20 entries: time, kind, name, status, size, duration. `↑↓` scroll, `r` retries the selected entry (backup or restore command), `Esc` closes. History view does not start anything on Enter.

- [ ] **Step 5: Tests**

- Load corrupt JSON returns empty.
- Retry maps backup/restore kind to correct command.
- Render test at 100×30 and 48×12 shows `History` and an entry.

- [ ] **Step 6: Commit**

```bash
git add tui/src/history.rs tui/src/main.rs tui/src/app.rs tui/src/ui.rs tui/src/data.rs
git commit -S -m "feat(tui): persist and browse operation history"
```

---

## Task 9: Touch, scrollbars and hit areas

**Files:**
- Modify: `tui/src/app.rs`, `tui/src/ui.rs`, `tui/src/main.rs`
- Test: `tui/src/app.rs`, `tui/src/ui.rs`

**Interfaces:**
- `HeaderAreas { count: Rect, warning: Rect, status: Rect }`
- `App.header_areas: HeaderAreas`
- `App.stat_areas: [Rect; 4]`
- `App.scrollbar_state: ScrollbarState` (one per list view, or local state structs)
- Click mapping:
  - count/Backups card -> focus Backups
  - warning chip/status line -> `Action::Logs`
  - Stored/Packages cards -> focus Backups
  - Auto card -> focus Actions and set status `AUTO: <schedule>` (no implicit command)
- Scrollbar indicator on Actions, Backups, Output, operation log, package diff when content overflows.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn clicking_the_header_count_focuses_backups() {
    let mut app = App::with_snapshot(snapshot());
    app.header_areas.count = Rect::new(40, 1, 10, 1);
    app.handle_mouse(42, 1);
    assert_eq!(app.focus, Focus::Backups);
}

#[test]
fn clicking_status_opens_logs() {
    let mut app = App::with_snapshot(snapshot());
    app.header_areas.status = Rect::new(0, 23, 80, 1);
    app.handle_mouse(5, 23);
    assert_eq!(app.focus, Focus::Output);
    assert_eq!(app.status, "showing sync.log");
}
```

- [ ] **Step 2: Implement hit areas**

UI sets `header_areas` and `stat_areas` every dashboard draw. Reset to `Rect::default()` at draw start. `handle_mouse` checks operation first, then confirm dialogs, then header/status/stats, then existing panes. Warning click calls `self.run(Action::Logs)`; ensure `Action::Logs` does not require a selected backup.

- [ ] **Step 3: Add scrollbars**

Use `ratatui::widgets::Scrollbar` with `ScrollbarState`. Compute content length and viewport. Show only when `content_len > viewport_height`. Add to action list, backup table, output, operation log and diff list.

- [ ] **Step 4: Tests**

- Clicking warning focuses Output and sets status `"showing sync.log"`.
- Scrollbar is rendered for 30 actions? Actions fixed 12; use a test snapshot with 30 backups to assert a scrollbar cell is present (`█`/`▐`).
- Header click zones are empty on operation screen.

- [ ] **Step 5: Commit**

```bash
git add tui/src/app.rs tui/src/ui.rs tui/src/main.rs
git commit -S -m "feat(tui): widen touch targets and add scrollbars"
```

---

## Task 10: Setup/schedule handoff context screen

**Files:**
- Modify: `tui/src/app.rs`, `tui/src/ui.rs`, `tui/src/main.rs`
- Test: `tui/src/app.rs`, `tui/src/ui.rs`

**Interfaces:**
- `HandoffInfo { action: Action, title: String, detail: String }`
- `App.pending_handoff: Option<HandoffInfo>`
- Enter starts the existing handoff; Esc cancels.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn setup_action_opens_handoff_card_instead_of_prompting_inline() {
    let mut app = App::with_snapshot(snapshot());
    app.run(Action::Setup);
    assert!(app.pending_handoff.is_some());
    assert!(app.pending.is_none());
}
```

- [ ] **Step 2: Implement**

`Action::Setup` and `Action::Schedule` set `pending_handoff` with title/detail. Enter sets `pending = Some(["setup"])`/`["schedule"]`; Esc clears. Keep the existing terminal handoff and return prompt; after return, status is `returned from setup`/`returned from schedule`.

- [ ] **Step 3: Render**

Modal `Handoff` card: title, why the terminal will change, command, `Enter continue · Esc cancel`. At 48×12 truncate detail to two wrapped lines.

- [ ] **Step 4: Tests**

- Enter queues setup; Esc leaves dashboard.
- Render test asserts `Handoff` and command.
- Mouse click on confirm/cancel areas.

- [ ] **Step 5: Commit**

```bash
git add tui/src/app.rs tui/src/ui.rs tui/src/main.rs
git commit -S -m "feat(tui): show context before setup and schedule handoff"
```

---

## Task 11: Runbook and security documentation

**Files:**
- Create: `tui/RUNBOOK.md`
- Modify: `tui/README.md`
- Test: none (documentation), verify links with `test -f`.

- [ ] **Step 1: Write the runbook**

Include:
- event protocol v1 and `--json-events`
- old CLI and `--yes` fallback
- backup cancel path and cleanup
- restore cancel locked after extraction; SIGINT ignored
- cancel timeout / SIGKILL and incomplete cleanup
- panic during extraction: terminal restores, child may continue
- temporary paths and manual cleanup
- verification states and what `SKIPPED` means
- log and history file locations
- privacy: no tokens in events; config remains plaintext

- [ ] **Step 2: Link it**

Add `See tui/RUNBOOK.md` to root Optional TUI section and `tui/README.md`.

- [ ] **Step 3: Verify**

Run: `test -f tui/RUNBOOK.md && grep -q "SIGKILL" tui/RUNBOOK.md`
Expected: exit 0.

- [ ] **Step 4: Commit**

```bash
git add tui/RUNBOOK.md tui/README.md README.md
git commit -S -m "docs: add TUI recovery and security runbook"
```

---

## Task 12: Source install script

**Files:**
- Create: `tui/install.sh`
- Modify: `tui/README.md`, `.github/workflows/tui.yml`
- Test: `bash -n tui/install.sh`, CI temp-prefix dry run.

**Interfaces:**
- `tui/install.sh [--prefix PATH]`; default `$PREFIX`.
- Installs `termux-sync-tui` beside the existing Python command.

- [ ] **Step 1: Write the script test**

```bash
bash -n tui/install.sh
TMP=$(mktemp -d)
PREFIX="$TMP" bash tui/install.sh --skip-build
test -x "$TMP/bin/termux-sync-tui"
```

Use a real build in CI; `--skip-build` uses an existing `target/release/termux-sync-tui` in the test fixture.

- [ ] **Step 2: Implement**

Script checks `cargo`, `target/release/termux-sync-tui` or runs `cargo build --locked --release`, creates `$PREFIX/bin`, and copies with `install -m755`. Reject non-Termux `PREFIX`? Allow any prefix for tests. Print next steps and `--ascii` note.

- [ ] **Step 3: Add CI**

Add a job step after release build:
```yaml
- name: Install script
  run: bash install.sh --skip-build
  env:
    PREFIX: ${{ runner.temp }}/prefix
```
Run from `tui`.

- [ ] **Step 4: Commit**

```bash
git add tui/install.sh tui/README.md .github/workflows/tui.yml
git commit -S -m "build: add source installer for the TUI"
```

---

## Task 13: Android cross-compile CI

**Files:**
- Modify: `.github/workflows/tui.yml`
- Optionally create: `tui/.cargo/config.toml` only if required by cargo-ndk.

**Interfaces:**
- CI job `android` builds `aarch64-linux-android` and `armv7-linux-androideabi`.
- No release artifact yet; compilation is the gate.

- [ ] **Step 1: Add the job**

```yaml
android:
  name: Android targets
  runs-on: ubuntu-latest
  defaults:
    run:
      working-directory: tui
  steps:
    - uses: actions/checkout@v7
    - uses: nttld/setup-ndk@v1
      with:
        ndk-version: r27c
    - uses: dtolnay/rust-toolchain@stable
    - run: cargo install cargo-ndk --locked
    - run: cargo ndk -t arm64-v8a -t armeabi-v7a build --locked --release
```

- [ ] **Step 2: Verify workflow syntax locally**

Run: `python -c "import yaml,sys; yaml.safe_load(open('.github/workflows/tui.yml'))"`
Expected: no error. If PyYAML is unavailable, run `actionlint` if installed; otherwise inspect indentation.

- [ ] **Step 3: Document**

Add a README note: CI checks Android targets; binary artifacts are a later release task.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/tui.yml tui/README.md
git commit -S -m "ci: cross-check TUI for Android targets"
```

---

## Task 14: PTY integration harness

**Files:**
- Create: `tui/tests/pty_fake.sh`
- Create: `tui/tests/pty.sh`
- Modify: `.github/workflows/tui.yml`
- Test: the harness itself.

**Interfaces:**
- `pty_fake.sh` dispatches on `TEST_MODE`:
  - `backup-ok`: emits start, stages, log, result
  - `backup-cancel`: emits start/archive then traps INT, emits cancelled
  - `restore-plan`: emits plan/diff, waits stdin, emits verify/extract/result
- Keys are injected through `script -qec` and `sleep`.

- [ ] **Step 1: Write fake emitter**

Use `printf '%s\n'` JSON lines, `sleep` only where needed. Restore reads one line with `read` and exits 130 on `CANCEL`.

- [ ] **Step 2: Write harness**

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
BIN=target/debug/termux-sync-tui
run() { script -qec "stty rows 24 cols 90; TERMUX_SYNC_BIN=$PWD/tests/pty_fake.sh $BIN" /dev/null; }
```

Test backup success: feed Enter, wait, Esc, q; grep `SUCCESS`.
Test backup cancel: feed Enter, wait, Esc, Enter (confirm), q; grep `Operation cancelled`.
Test restore plan: Tab, Enter, type `RESTORE`, Enter, wait, Esc, q; grep `Close and reopen Termux`.

- [ ] **Step 3: Add CI step**

```yaml
- name: PTY tests
  run: bash tests/pty.sh
```

- [ ] **Step 4: Run locally**

Run: `cargo build --locked && bash tests/pty.sh`
Expected: all three cases exit 0.

- [ ] **Step 5: Commit**

```bash
git add tui/tests/pty.sh tui/tests/pty_fake.sh .github/workflows/tui.yml
git commit -S -m "test(tui): add PTY operation smoke tests"
```

---

## Task 15: Remote and protocol fixture tests

**Files:**
- Create: `tests/fixtures/rclone_stats.txt`
- Create: `tests/fixtures/github_manifest.json`
- Create: `tui/tests/fixtures/*.jsonl`
- Modify: `tests/test_events.py`, `tests/test_storage.py`, `tui/src/ops.rs`

**Interfaces:**
- Fixtures are plain files read by tests; no network.
- `parse_event` tests cover `backup`, `plan`, `package_diff`, `progress`, `result`, `error`, `cancelled`, `Unsupported`.

- [ ] **Step 1: Add Python fixtures/tests**

- `rclone_stats.txt` with `Transferred: 5 MiB / 10 MiB, 50%` and `Transferred: 10 MiB / 10 MiB, 100%`; assert two progress events.
- `github_manifest.json` used by a mocked urllib asset response; assert `fetch_manifest` returns it.
- A malformed manifest with `archive: "../escape"`; assert list/restore refuses.

- [ ] **Step 2: Add Rust fixture tests**

`tui/tests/fixtures/list.jsonl` with start/backup/backup/result; parse all into 2 backups. `malformed.jsonl` with a truncated line; parser returns a warning and no panic.

- [ ] **Step 3: Run**

Run: `pytest tests/test_events.py tests/test_storage.py -q && cargo test --locked`
Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add tests/fixtures tests/test_events.py tests/test_storage.py tui/tests/fixtures tui/src/ops.rs
git commit -S -m "test: cover remote events and malformed fixtures"
```

---

## Task 16: 56-column header visual regression

**Files:**
- Modify: `tui/src/ui.rs` tests
- Test: `tui/src/ui.rs`

**Interfaces:**
- Test helper `render_with_warning(width, height) -> String`.
- Assertions use the rendered buffer; no image snapshots required.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn header_fits_at_56_columns_with_github_and_warning() {
    let mut app = app();
    app.snapshot.config.storage = "github".into();
    app.snapshot.backups.clear();
    app.snapshot.warning = Some("GITHUB storage".into());
    let frame = render_app(&mut app, 56, 24);
    assert!(frame.contains("GITHUB"));
    assert!(frame.contains("0 BACKUPS"));
    assert!(frame.contains("WARN"));
    assert!(frame.contains("1 BACKUP") || frame.contains("0 BACKUPS"));
}
```

- [ ] **Step 2: Run**

Run: `cargo test --locked header_fits_at_56_columns_with_github_and_warning`
Expected: FAIL if chips clip.

- [ ] **Step 3: Fix layout**

At `<60` columns, drop the `backup dashboard` subtitle or shrink the right chip group; keep storage, count and `WARN`. Do not shorten the warning text itself. Keep the header one row.

- [ ] **Step 4: Run all checks**

Run: `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add tui/src/ui.rs
git commit -S -m "fix(tui): keep header chips readable at 56 columns"
```

---

## Task 17: Final integration pass

**Files:**
- Modify: `README.md`, `tui/README.md`, `CHANGELOG.md` only if release notes are requested.
- Test: full local gate.

- [ ] **Step 1: Run Python gate**

```bash
python -m pytest -q
ruff check src tests
cmp termux-sync.py src/termux_sync/__init__.py
```

- [ ] **Step 2: Run Rust gate**

```bash
cd tui
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
bash install.sh --skip-build
bash tests/pty.sh
```

- [ ] **Step 3: Update docs**

Add any new keys/flags to both READMEs: `--ascii`, `--yes`, history, filter, label prompt, handoff card. Link the runbook.

- [ ] **Step 4: Push and let CI run**

```bash
git push
gh run list --branch spike/rust-tui --limit 2
```

Expected: TUI workflow success, Rust job, Android job, PTY step.

- [ ] **Step 5: Commit**

```bash
git add README.md tui/README.md CHANGELOG.md
git commit -S -m "docs: document completed TUI features"
```

---

## Later / Not Required

These are deliberately excluded from this plan:

- **Merge to `main` or open a PR.** User explicitly requested no PR; the branch stays separate.
- **Version compatibility matrix.** TUI `0.1.0` vs Python `1.3.9` vs event protocol `v1` needs a release-time decision.
- **Prebuilt release binaries.** No aarch64/armv7 tarballs or `.deb`; Task 13 checks compilation only. Distribution artifacts are a later release task.
