# Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix every finding from the 2026-11-24 whole-repository review: five data-loss defects, thirty required correctness gaps, and the optional cleanups.

**Architecture:** Make the core fail closed. Archive, backup, restore, upload, and delete either succeed completely or return an error that the CLI exits non-zero on. Validate every config and argv boundary before it reaches a path or URL. Make destructive writes atomic. Delete the duplicate clap parser so `frontdoor` is the only argv parser. Harden the TUI against malformed events and terminal escapes.

**Tech Stack:** Rust 1.100, edition 2024, serde_json, tar, flate2/bzip2/xz2, ureq, ratatui, crossterm, fs4, tempfile, trycmd.

**Spec:** `docs/specs/2026-11-25-review-fixes-design.md`

## Global Constraints

- Rust edition 2024; `rust-version = "1.100"` in every crate.
- Checks in each crate: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`.
- Do not change the backup format: same manifest keys, archive names, and SHA-256 checksums. Backups from `2.0.0` must restore.
- Do not change event protocol version `1`. Adding fields to events is allowed; removing or renaming is not.
- No shell interpolation. `rclone`, `dpkg`, and `pkg` take argv arrays only.
- No secret reaches the log file, an event, or a file with mode other than 0600.
- No new runtime dependency except `rustix` with the `fs` feature, added to `termux-sync-core` for `O_NOFOLLOW`.
- Sign commits with `git commit -S` when a signing key is configured.
- Never edit `.deja/` files.
- All user-facing text stays English, with the existing tone and no new emoji.

## Review Focus

- **Archive member named like a restore root** (`home`, `usr_etc`, `usr_lib`, `packages`): extraction must never delete the existing directory. T2 pins this with a tar containing a regular `home` entry over an existing `$HOME`.
- **`max_backups` set to 0 or negative** by a hand-edited or imported config: the backup just made must survive. T3 and T5 pin the clamp and the post-verify prune.
- **Unknown `storage` value**: must return an error, never write to the local disk while the UI says otherwise. T3 pins it.
- **Interrupted local upload**: must not leave a directory that `list` accepts as a backup with missing archives. T3 pins the staging-directory rename.
- **Malformed `config.json`, `cron.json`, or non-UTF8 argv**: must exit with a clean error, never panic. T8, T10, and T12 pin each.

---

## File Structure

- `rust/termux-sync-core/src/core/archive.rs` — extraction safety, limits, modes, hard links, permission guard.
- `rust/termux-sync-core/src/core/backup.rs` — fail-closed backup and restore, retention ordering, temp directories.
- `rust/termux-sync-core/src/core/storage.rs` — atomic local upload, strict backend selection, fallible pruning.
- `rust/termux-sync-core/src/core/rclone.rs` — backup-name filter, folder validation, stderr in errors.
- `rust/termux-sync-core/src/core/github.rs` — error propagation, URL validation, asset verification, streaming split.
- `rust/termux-sync-core/src/core/config.rs` — `validate_patch`, `retention_limit`.
- `rust/termux-sync-core/src/core/json.rs` — private atomic JSON writer.
- `rust/termux-sync-core/src/core/fsutil.rs` — `try_lock_exclusive`, `private_dir`, `required_tools`.
- `rust/termux-sync-core/src/core/maintenance.rs` — setup persistence, import/export, schedule/daemon, delete/cache/status.
- `rust/termux-sync-core/src/core/packages.rs` — fallible package collection.
- `rust/termux-sync-core/src/core/logging.rs` — redaction before disk.
- `rust/termux-sync-core/src/port.rs` — `set_debug` wrapper.
- `rust/app/src/main.rs` — single parser entry.
- `rust/app/src/frontdoor.rs` — argv, lazy config, debug, tui handoff.
- `rust/app/src/cli.rs`, `rust/app/src/units/` — deleted.
- `tui/src/ops.rs` — probes, cancel timer, bounded line reader, CLI-backed backup list.
- `tui/src/data.rs` — control stripping, installed-version query, bounded tail.
- `tui/src/ui.rs`, `tui/src/app.rs` — safe rendering, cached capabilities, output cap; split in T21.
- `tui/src/history.rs` — 0600 history file.
- `rust/app/tests/cli_integration.rs`, `rust/termux-sync-core/src/core/*` test modules, `tui/src/*` test modules — regression tests.
- `docs/`, `.github/`, `install.sh`, `rust/install.sh`, `.github/workflows/rust.yml` — truth and CI.

---

### Task 1: Setup persists its answers

**Files:**
- Modify: `rust/termux-sync-core/src/core/maintenance.rs:127-290` (`setup`, token prompt, final save)
- Test: `rust/app/tests/cli_integration.rs:229-247`

**Interfaces:**
- Consumes: `config::load_config() -> Result<Value, String>`, `config::default_config() -> Map<String, Value>`, `config::save_config(&Value) -> Result<(), String>`.
- Produces: no new public API; `setup()` keeps `Result<Value, String>`.

- [ ] **Step 1: Replace the vacuous setup test with one that changes values**

In `cli_integration.rs`, change `setup_and_schedule_write_state` to run:

```rust
let setup = fx.run(&["setup"], Some("1\n\nbz2\n7\nn\n"));
assert!(setup.status.success(), "{}", String::from_utf8_lossy(&setup.stdout));
assert_eq!(fx.config()["compression"], "bz2");
assert_eq!(fx.config()["max_backups"], 7);
```

Add a second test that keeps a stored token:

```rust
#[test]
fn setup_keeps_stored_token_on_empty_answer() {
    let fx = Fixture::new();
    let out = fx.run(&["setup"], Some("3\n\nowner/repo\n\n\n\nn\n"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    assert_eq!(fx.config()["github_token"], "secret-token");
    assert_eq!(fx.config()["github_repo"], "owner/repo");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rust/app && cargo test --locked setup_ -- --nocapture`
Expected: FAIL. The first test sees `compression = "gz"` and `max_backups = 5`; the second loops on the token prompt or overwrites the token.

- [ ] **Step 3: Save the edited map and keep an existing token**

In `setup()`:

```rust
let cfg = match config::load_config() {
    Ok(cfg) => cfg,
    Err(error) => {
        render::print_line(&format!("  Warning: {error}"));
        render::print_line("  Starting from the default configuration.");
        Value::Object(config::default_config())
    }
};
```

In the token loop, before asking:

```rust
if !existing_token.is_empty() {
    let value = render::ask("GitHub token", "")?;
    if value.trim().is_empty() {
        break;
    }
    map.insert("github_token".to_string(), Value::String(value.trim().to_string()));
    break;
}
```

Replace `config::save_config(&cfg)?;` with:

```rust
let saved = Value::Object(map.clone());
config::save_config(&saved)?;
```

Keep `map` available for the logging call after the save.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rust/app && cargo test --locked setup_ -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/termux-sync-core/src/core/maintenance.rs rust/app/tests/cli_integration.rs
git commit -S -m "fix(setup): persist wizard answers and keep stored token"
```

---

### Task 2: Extraction cannot clobber roots, drop modes, or exhaust disk

**Files:**
- Modify: `rust/termux-sync-core/src/core/archive.rs:96-140` (`unlock_tree`, `relock`), `240-315` (`fill_archive`), `375-467` (`inside`, `check_member_root`, `filtered_mode`, `create_regular`, `create_dir`), `508-626` (`extract_archive`)
- Modify: `rust/termux-sync-core/Cargo.toml` (add `rustix`)
- Test: `rust/termux-sync-core/src/core/archive.rs` test module

**Interfaces:**
- Produces: `pub fn extract_archive(archive_path: &Path, dest: &Path, expected_top: Option<&str>, max_bytes: Option<u64>, on_member: &mut dyn FnMut(&Path)) -> Result<ExtractResult, String>`
- Produces: `struct UnlockGuard` with `Drop`; `fn unlock_tree(root: &Path, excludes: &[String]) -> UnlockGuard`
- Produces: `fn create_regular(entry: &mut tar::Entry<'_, Box<dyn Read>>, target: &Path, mode: u32, mtime: i64, budget: &mut u64) -> Result<(), String>`
- Produces: `fn create_dir(target: &Path, mode: u32, mtime: i64) -> Result<(), String>`
- Consumes: `manifest::valid_backup_name` not used here.

- [ ] **Step 1: Write failing tests for each defect**

Add to the archive test module:

```rust
#[test]
fn rejects_regular_entry_over_expected_root() {
    let temp = tempfile::tempdir().unwrap();
    let dest = temp.path().join("dest");
    fs::create_dir_all(dest.join("home")).unwrap();
    fs::write(dest.join("home/keep.txt"), b"keep").unwrap();
    let archive = temp.path().join("root.tar");
    let file = File::create(&archive).unwrap();
    let mut builder = Builder::new(file);
    builder.append(&raw_header("home", 1, EntryType::Regular, b""), Cursor::new(b"x")).unwrap();
    builder.finish().unwrap();
    let mut warnings = Vec::new();
    let result = extract_archive(&archive, &dest, Some("home"), None, &mut |_| {}).unwrap();
    assert!(dest.join("home/keep.txt").exists());
    assert!(!result.warnings.is_empty());
}

#[test]
fn restores_directory_mode() {
    let temp = tempfile::tempdir().unwrap();
    let dest = temp.path().join("dest");
    let archive = temp.path().join("mode.tar");
    let file = File::create(&archive).unwrap();
    let mut builder = Builder::new(file);
    let mut header = raw_header("home/private", 0, EntryType::dir(), b"");
    header.set_mode(0o700);
    header.set_cksum();
    builder.append(&header, Cursor::new(b"")).unwrap();
    builder.finish().unwrap();
    extract_archive(&archive, &dest, Some("home"), None, &mut |_| {}).unwrap();
    let mode = fs::metadata(dest.join("home/private")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn resolves_forward_hard_link() {
    let temp = tempfile::tempdir().unwrap();
    let dest = temp.path().join("dest");
    let archive = temp.path().join("links.tar");
    let file = File::create(&archive).unwrap();
    let mut builder = Builder::new(file);
    builder
        .append(
            &raw_header("home/link", 0, EntryType::Link, b"home/target"),
            Cursor::new(b""),
        )
        .unwrap();
    builder
        .append(&raw_header("home/target", 0o644, EntryType::Regular, b""), Cursor::new(b"data"))
        .unwrap();
    builder.finish().unwrap();
    extract_archive(&archive, &dest, Some("home"), None, &mut |_| {}).unwrap();
    assert_eq!(fs::read(dest.join("home/link")).unwrap(), b"data");
}

#[test]
fn refuses_archive_over_byte_budget() {
    let temp = tempfile::tempdir().unwrap();
    let dest = temp.path().join("dest");
    let archive = temp.path().join("big.tar");
    let file = File::create(&archive).unwrap();
    let mut builder = Builder::new(file);
    builder
        .append(&raw_header("home/big", 100, EntryType::Regular, b""), Cursor::new(vec![b'x'; 100]))
        .unwrap();
    builder.finish().unwrap();
    let error = extract_archive(&archive, &dest, Some("home"), Some(10), &mut |_| {}).unwrap_err();
    assert!(error.contains("size limit"));
}
```

Use the existing `raw_header` helper. For the directory test, set `header.set_mode(0o700)` and `EntryType::dir()`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rust/termux-sync-core && cargo test --locked archive::tests`
Expected: FAIL. The root test loses `keep.txt`, the mode test sees `0755`, the hard-link test warns and misses the file, and the budget test compiles only after the signature change (add the parameter first, pass `None` in existing tests).

- [ ] **Step 3: Implement the guards**

1. Add to `Cargo.toml`: `rustix = { version = "1", features = ["fs"] }`.
2. In `check_member_root`, after the `first != top` check:

```rust
if normalized_relative(rel) == Path::new(top) {
    return Err(format!("Archive member replaces expected root {top:?}: {rel:?}"));
}
```

3. In `create_regular`, before `remove_existing`:

```rust
if fs::symlink_metadata(target).is_ok_and(|meta| meta.is_dir()) {
    return Err(format!("Refusing to replace directory with file: {}", target.display()));
}
```

Replace the `OpenOptions` call with `rustix::fs::open(target, OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::NOFOLLOW, Mode::from_raw_mode(mode))`, then `File::from(fd)`.

4. `filtered_mode` for directories returns `mode & 0o777`; `create_dir` applies it with `fs::set_permissions`.

5. In `extract_archive`, keep `let mut budget = max_bytes.unwrap_or(u64::MAX);` and `let mut entries_seen = 0usize;`. Increment `entries_seen` per entry and return `Err("Archive has more than 1000000 members")` past the cap. For file entries, call `create_regular(..., &mut budget)`; inside, `let copied = std::io::copy(&mut entry.take(budget.saturating_add(1)), &mut file)?;` and return `Err("Archive exceeds size limit")` when `copied > budget`; otherwise `*budget -= copied`.

6. Queue hard links instead of creating immediately: collect `(target, resolved, mtime)` in `pending_links`, and after the entry loop retry `fs::hard_link`; only then push a warning.

7. Change `unlock_tree` to skip paths matched by `wrapped_excludes`/`excluded`, return `UnlockGuard`, and implement `Drop` calling `relock`. Call it as `let _guard = unlock_tree(source, &options.excludes);` before `fill_archive`.

8. Move `on_member(&arc, size)` below the exclude check in `fill_archive`.

9. Resolve `dest` once at the top of `extract_archive` and pass the resolved root to `inside`, so per-member checks do not re-stat every component.

- [ ] **Step 4: Update every caller and run the tests**

Update `backup.rs` restore to pass `None` for now (Task 6 supplies the manifest-derived limit) and existing archive tests to pass `None`.

Run: `cd rust/termux-sync-core && cargo test --locked archive::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/termux-sync-core/Cargo.toml rust/termux-sync-core/src/core/archive.rs
git commit -S -m "fix(archive): refuse root replacement, restore modes, bound extraction"
```

---

### Task 3: Local storage uploads atomically and rejects unknown backends

**Files:**
- Modify: `rust/termux-sync-core/src/core/storage.rs:40-87` (`upload`), `161-184` (`delete_old`), `318-330` (`from_config`)
- Test: `rust/termux-sync-core/src/core/storage.rs` test module

**Interfaces:**
- Produces: `LocalStorage::delete_old(&self, max_keep: i64) -> Result<(), String>`
- Produces: `Storage::delete_old(&self, max_keep: i64) -> Result<(), String>`
- Produces: `fn upload_order(files: &mut [PathBuf])` (private, testable)
- Consumes: `manifest::valid_backup_name`.

- [ ] **Step 1: Write failing tests**

```rust
use std::os::unix::fs::PermissionsExt;

#[test]
fn upload_is_atomic_on_copy_failure() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("backups");
    let work = temp.path().join("work");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("home.tar.gz"), b"data").unwrap();
    fs::write(work.join("manifest.json"), b"{}").unwrap();
    fs::set_permissions(work.join("home.tar.gz"), fs::Permissions::from_mode(0o000)).unwrap();
    let cfg = json!({"storage": "local", "local_path": root.to_string_lossy()});
    let storage = LocalStorage::new(&cfg);
    assert!(storage.upload(&work, "termux_backup_x").is_err());
    assert!(!root.join("termux_backup_x").exists());
    let leftovers: Vec<_> = fs::read_dir(&root).unwrap().flatten().collect();
    assert!(leftovers.is_empty());
}

#[test]
fn upload_places_manifest_last() {
    let mut files: Vec<PathBuf> = ["manifest.json", "usr_etc.tar.gz", "home.tar.gz"]
        .into_iter()
        .map(PathBuf::from)
        .collect();
    upload_order(&mut files);
    assert_eq!(files.last().unwrap(), Path::new("manifest.json"));
}

#[test]
fn delete_old_keeps_at_least_one() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("backups");
    fs::create_dir_all(&root).unwrap();
    for name in ["termux_backup_a", "termux_backup_b", "termux_backup_c"] {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("manifest.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let cfg = json!({"storage": "local", "local_path": root.to_string_lossy()});
    LocalStorage::new(&cfg).delete_old(0).unwrap();
    let remaining: Vec<String> = fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(remaining, vec!["termux_backup_c".to_string()]);
}

#[test]
fn unknown_storage_is_an_error() {
    let err = Storage::from_config(&json!({"storage": "dropbox"})).unwrap_err();
    assert!(err.contains("Unknown storage backend"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rust/termux-sync-core && cargo test --locked storage::tests`
Expected: FAIL. The upload leaves a partial directory, manifest sorts before `usr_etc.tar.gz`, `delete_old(0)` removes all three, and `from_config` returns `Local`.

- [ ] **Step 3: Implement**

1. `upload`: create `let staging = self.root.join(format!(".{backup_name}.upload-{}", std::process::id()));` with `fs::create_dir_all`, copy files in `upload_order` order, then `fs::rename(&staging, &dest)`. On any error, `let _ = fs::remove_dir_all(&staging);` and return `Err`.
2. `upload_order`: sort so `manifest.json` is last and other files keep lexical order.
3. `delete_old`: `let keep = max_keep.max(1) as usize;` return `Ok(())`; collect `remove_dir_all` failures and return the first as `Err` after attempting all. Only consider dirs where `is_backup_dir` is true.
4. `from_config`: replace the `_ => Ok(Self::Local(...))` arm with `other => Err(format!("termux_sync.UserError: Unknown storage backend: {other:?}"))`.
5. `Storage::delete_old` forwards the result. Change `RcloneStorage::delete_old` to return `Result<(), String>` in this task too (Task 4 adds the name filter and error propagation). The GitHub arm keeps `let _ = github.delete_old(max_keep); Ok(())` until Task 7 returns `Result`; Task 7 replaces it with `github.delete_old(max_keep)`.

- [ ] **Step 4: Run the tests and update callers**

Update `backup.rs:579` to `storage.delete_old(...)?` (Task 5 moves the call after verification).

Run: `cd rust/termux-sync-core && cargo test --locked storage::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/termux-sync-core/src/core/storage.rs rust/termux-sync-core/src/core/backup.rs
git commit -S -m "fix(storage): atomic upload, strict backend, safe pruning"
```

---

### Task 4: rclone retention only deletes backups

**Files:**
- Modify: `rust/termux-sync-core/src/core/rclone.rs:23-58` (`new`, `remote_path`, `run`), `170-207` (`delete_old`, `delete_backup`)
- Test: `rust/termux-sync-core/src/core/rclone.rs` test module

**Interfaces:**
- Produces: `RcloneStorage::delete_old(&self, max_keep: i64) -> Result<(), String>` (the signature was already changed in Task 3; this task adds the filter and error propagation)
- Produces: `fn valid_folder_id(value: &str) -> bool`
- Produces: `fn is_backup_name(name: &str) -> bool`
- Consumes: `manifest::valid_backup_name`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn only_backup_names_are_prunable() {
    assert!(is_backup_name("termux_backup_20260101_020000"));
    assert!(is_backup_name("termux_backup_20260101_020000_auto"));
    assert!(!is_backup_name("Photos"));
    assert!(!is_backup_name("termux_backup_x/.."));
}

#[test]
fn folder_id_rejects_path_segments() {
    assert!(valid_folder_id(""));
    assert!(valid_folder_id("1AbCdEf"));
    assert!(!valid_folder_id(".."));
    assert!(!valid_folder_id("/root"));
    assert!(!valid_folder_id("a:b"));
}

#[test]
fn run_reports_stderr() {
    // PATH with a fake rclone that prints "boom" to stderr and exits 1
    // assert the Err string contains "boom".
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rust/termux-sync-core && cargo test --locked rclone::tests`
Expected: FAIL to compile (`is_backup_name` and `valid_folder_id` missing); the stderr test fails on the message.

- [ ] **Step 3: Implement**

1. `valid_folder_id`: empty is valid; reject a leading `/`, any `..` segment, `:`, `\`, and control characters.
2. `RcloneStorage::new` returns `termux_sync.UserError: Invalid Google Drive folder ID` when validation fails.
3. `is_backup_name`: `name.starts_with("termux_backup_") && manifest::valid_backup_name(name)`.
4. `delete_old`: filter items with `is_backup_name`, `let keep = max_keep.max(1) as usize`, purge only the rest, and return `Err` on a failed `purge`.
5. `run`: on non-zero exit, include `String::from_utf8_lossy(&output.stderr)` truncated to 500 characters in the error.

- [ ] **Step 4: Run the tests**

Run: `cd rust/termux-sync-core && cargo test --locked rclone::tests`
Expected: PASS. The roundtrip test skips when rclone is absent; it must not fail.

- [ ] **Step 5: Commit**

```bash
git add rust/termux-sync-core/src/core/rclone.rs
git commit -S -m "fix(rclone): prune only backups and surface stderr"
```

---

### Task 5: Backup fails closed

**Files:**
- Modify: `rust/termux-sync-core/src/core/backup.rs:209-285` (temp dir), `440-700` (archive loop, prune, verify, result), `rust/termux-sync-core/src/core/packages.rs:9-27`
- Modify: `rust/termux-sync-core/src/core/config.rs` (add `retention_limit`)
- Test: `rust/termux-sync-core/src/core/config.rs`, `packages.rs` test modules; `rust/app/tests/cli_integration.rs`

**Interfaces:**
- Produces: `pub fn config::retention_limit(cfg: &Value) -> i64`
- Produces: `pub fn packages::installed_packages() -> Result<Vec<String>, String>`
- Produces: `fn installed_packages_with(program: &Path) -> Result<Vec<String>, String>`
- Produces: `fn prepare_work_dir(path: &Path) -> Result<(), String>`

- [ ] **Step 1: Write failing tests**

In `config.rs`:

```rust
#[test]
fn retention_is_at_least_one() {
    assert_eq!(retention_limit(&json!({"max_backups": 0})), 1);
    assert_eq!(retention_limit(&json!({"max_backups": -3})), 1);
    assert_eq!(retention_limit(&json!({})), 5);
}
```

In `packages.rs`:

```rust
#[test]
fn failed_dpkg_is_an_error() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("dpkg");
    fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(installed_packages_with(&script).is_err());
}
```

In `cli_integration.rs`:

```rust
#[test]
fn backup_fails_when_an_archive_fails() {
    let fx = Fixture::new();
    let out = fx.run_env(&["backup"], None, &[("TERMUX_SYNC_TEST_ARCHIVE_FAIL", "home")]);
    assert!(!out.status.success(), "backup reported success");
    assert!(String::from_utf8_lossy(&out.stdout).contains("Failed to archive"));
}
```

Add `run_env` beside `run`, appending the extra environment pairs.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rust/app && cargo test --locked backup_fails_when_an_archive_fails`
Expected: FAIL. The binary exits 0 and prints "Backup completed successfully!".
Run: `cd rust/termux-sync-core && cargo test --locked retention_is_at_least_one`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

1. `retention_limit` returns `cfg.get("max_backups").and_then(fmt::py_int).unwrap_or(5).max(1)`.
2. `installed_packages` returns `Result`; wrap the current body in `installed_packages_with(Path::new("dpkg"))`. Return `Err("OSError: could not run dpkg: {error}")` on spawn failure and `Err("RuntimeError: dpkg exited with {status}")` on non-zero exit. In `backup()`, `let installed = match packages::installed_packages() { Ok(list) => list, Err(error) => { cleanup(&tmp_dir); return Err(error); } };`. In `package_diff`, use `packages::installed_packages().unwrap_or_default()` because a failed diff must not abort a restore.
3. `prepare_work_dir`: `fs::create_dir_all(path.parent())` when a parent exists, then `fs::create_dir(path)` (fails on `AlreadyExists`), then `fs::set_permissions(path, 0o700)`. Use it for the backup directory instead of `create_dir_all`. A repeated backup in the same second now fails with a clear "already exists" error.
4. Before the `archive_result` expression, add the debug-only seam, where
   `key` is the current item key (`"home"`, `"usr_etc"`, …):

```rust
#[cfg(debug_assertions)]
let injected_failure = std::env::var("TERMUX_SYNC_TEST_ARCHIVE_FAIL")
    .is_ok_and(|want| want == key);
#[cfg(not(debug_assertions))]
let injected_failure = false;

let archive_result: Result<(String, u64, String), String> = if injected_failure {
    Err("injected archive failure".to_string())
} else if let Storage::GitHub(github) = &storage {
    // existing GitHub branch
} else {
    // existing local/rclone branch
};
```

5. Replace the whole `Err(error) =>` branch with: emit failed stage, print, log, `cleanup(&tmp_dir)`, `return Err(error)`. Delete the GitHub-only special case.
6. Move `storage.delete_old(...)` from line 579 to after the verify block, guarded by:

```rust
if verified != Some(false) {
    storage.delete_old(config::retention_limit(cfg))?;
}
```

7. In the manifest, keep `packages` as before; a package error now aborts before the manifest is written.

- [ ] **Step 4: Run the tests and the full core suite**

Run: `cd rust/app && cargo test --locked backup_fails_when_an_archive_fails`
Run: `cd rust/termux-sync-core && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/termux-sync-core/src/core/backup.rs rust/termux-sync-core/src/core/packages.rs rust/termux-sync-core/src/core/config.rs rust/app/tests/cli_integration.rs
git commit -S -m "fix(backup): fail closed and prune only verified backups"
```

---

### Task 6: Restore reports extraction failure

**Files:**
- Modify: `rust/termux-sync-core/src/core/backup.rs:772-800` (temp dir), `900-1040` (space precheck), `1040-1215` (extraction result)
- Test: `rust/app/tests/cli_integration.rs`

**Interfaces:**
- Produces: `fn restore_work_dir(name: &str) -> PathBuf` (unique, 0700)
- Produces: `fn ensure_extract_space(root: &Path, needed: u64) -> Result<(), String>`
- Consumes: `archive::extract_archive(..., max_bytes, ...)` from Task 2.

- [ ] **Step 1: Write a failing integration test**

Create a backup directory under the fixture's local path with a valid manifest and a corrupt `home.tar.gz` whose checksum matches the corrupt bytes:

```rust
#[test]
fn restore_fails_when_extraction_fails() {
    let fx = Fixture::new();
    fx.write_corrupt_backup("termux_backup_bad");
    let out = fx.run(&["restore", "--name", "termux_backup_bad"], Some("y\n"));
    assert!(!out.status.success(), "restore reported success");
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"ok\":false"));
}
```

Add `write_corrupt_backup` to `Fixture`: create `~/backups/termux_backup_bad/`, write `home.tar.gz` as `b"not a tar"`, compute its SHA-256 with `sha2` (add `sha2` to app dev-dependencies), and write a manifest with `files.home = {archive, size, checksum}`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd rust/app && cargo test --locked restore_fails_when_extraction_fails`
Expected: FAIL. Restore prints success and exits 0.

- [ ] **Step 3: Implement**

1. `restore_work_dir`: `paths::tmp_root().join(format!("restore_{name}_{}_{}", std::process::id(), nanos))` where `nanos` comes from `SystemTime::now().duration_since(UNIX_EPOCH)`. Create it with `prepare_work_dir` (0700, fail on exists).
2. Before the extraction loop, compute the manifest total (`files.*.size`) and call `ensure_extract_space` for each distinct restore root. `ensure_extract_space` uses `fs4::available_space(root)` and returns `termux_sync.UserError: Not enough free space` when `available < needed + 64 MiB`.
3. Pass `Some(info.size + 64 * 1024 * 1024)` as `max_bytes` to `extract_archive`.
4. In the archive-invalid branch (`clean_backup_name` mismatch), emit a `failed` stage and an error log before `continue`.
5. After the loop, if `!failed_archives.is_empty()`:

```rust
events::emit("result", vec![
    ("ok", Value::Bool(false)),
    ("name", Value::String(name.clone())),
    ("warnings", Value::Bool(true)),
    ("archives_failed", Value::from(failed_archives.clone())),
]);
cleanup(&tmp_dir);
return Err(format!(
    "RuntimeError: Restore incomplete; failed archives: {}",
    failed_archives.join(", ")
));
```

Do not emit the success result afterwards. `failed_archives` must also be included in the normal success result's `warnings` when empty is not possible.

- [ ] **Step 4: Run the test and the app suite**

Run: `cd rust/app && cargo test --locked`
Expected: PASS, including the existing restore integration test.

- [ ] **Step 5: Commit**

```bash
git add rust/termux-sync-core/src/core/backup.rs rust/app/tests/cli_integration.rs rust/app/Cargo.toml
git commit -S -m "fix(restore): fail when archives fail or space is short"
```

---

### Task 7: GitHub backend verifies, validates, and streams

**Files:**
- Modify: `rust/termux-sync-core/src/core/github.rs:55-130` (`new`, auth, request), `197-355` (`create_release`, `get_release_by_tag`, `list_releases`, `split_file`, `upload_asset`), `551-660` (`download`, `delete_old`)
- Test: `rust/termux-sync-core/src/core/github.rs` test module

**Interfaces:**
- Produces: `fn valid_repo(value: &str) -> bool`
- Produces: `fn encode_segment(value: &str) -> String`
- Produces: `fn releases_from_response(status: u16, body: &str) -> Result<Vec<Value>, String>`
- Produces: `fn asset_matches(remote: &Value, local_size: u64) -> bool`
- Produces: `fn split_file_at(&self, src: &Path, tmp_dir: &Path, chunk: u64) -> Result<Vec<PathBuf>, String>`
- Produces: `fn part_groups(dest: &Path) -> Result<HashMap<String, Vec<PathBuf>>, String>`
- Produces: `GitHubStorage::list_releases(&self) -> Result<Vec<Value>, String>`
- Produces: `GitHubStorage::delete_old(&self, max_keep: i64) -> Result<(), String>`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn validates_repo_and_encodes_segments() {
    assert!(valid_repo("owner/repo"));
    assert!(!valid_repo("owner"));
    assert!(!valid_repo("owner/repo/extra"));
    assert!(!valid_repo("../repo"));
    assert_eq!(encode_segment("a b/c"), "a%20b%2Fc");
}

#[test]
fn non_200_release_list_is_an_error() {
    assert!(releases_from_response(401, r#"{"message":"Bad credentials"}"#).is_err());
    assert_eq!(releases_from_response(200, "[]").unwrap().len(), 0);
}

#[test]
fn existing_asset_must_match_size() {
    assert!(asset_matches(&json!({"size": 10}), 10));
    assert!(!asset_matches(&json!({"size": 9}), 10));
    assert!(!asset_matches(&json!({}), 10));
}

#[test]
fn split_uses_small_chunks_without_extra_memory() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("a.bin");
    fs::write(&src, b"0123456789abcdefghij").unwrap();
    let storage = test_storage();
    let parts = storage.split_file_at(&src, temp.path(), 8).unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(fs::read(&parts[1]).unwrap(), b"89abcdef");
}

#[test]
fn missing_download_dir_is_an_error() {
    assert!(part_groups(&tempfile::tempdir().unwrap().path().join("nope")).is_err());
}
```

`test_storage()` builds `GitHubStorage` with dummy token/repo/agent; keep it private to the test module.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rust/termux-sync-core && cargo test --locked github::tests`
Expected: FAIL to compile (new helpers missing).

- [ ] **Step 3: Implement**

1. `valid_repo`: exactly two non-empty segments of `[A-Za-z0-9_.-]`, no `..`.
2. `GitHubStorage::new` validates `github_repo`; error `termux_sync.UserError: Invalid GitHub repository; use owner/repo`.
3. `encode_segment`: percent-encode everything outside `A-Z a-z 0-9 - . _ ~` with uppercase hex. Use it for tags in `get_release_by_tag` and the tag DELETE URL. Repo is validated, so it needs no encoding.
4. `list_releases`: use `request_raw`, then `releases_from_response(status, &body)`.
5. `delete_old`: return `Result`, filter releases to tags that start with `termux_backup_` and pass `manifest::valid_backup_name`, then propagate both DELETE calls. Callers in `backup.rs` already use `?`. In `Storage::delete_old`, replace the Task 3 GitHub placeholder (`let _ = ...; Ok(())`) with `github.delete_old(max_keep)`.
6. `upload_asset`: on `422 already_exists`, call `release_assets(release_id)`; if an asset with the same name matches size, return `Ok(())`; otherwise DELETE it and `continue` the retry loop. Add `fn release_assets(&self, release_id: i64) -> Result<Vec<Value>, String>`.
7. `split_file` delegates to `split_file_at(src, tmp_dir, CHUNK_SIZE)`. `split_file_at` uses `BufWriter` and `io::copy(&mut file.by_ref().take(chunk), &mut writer)` with an 8 KiB buffer; never allocates `chunk` bytes. Before splitting, check `fs4::available_space(tmp_dir) >= size` and return `Not enough free space`.
8. `part_groups` returns `Err("OSError: ...")` when `read_dir` fails; `download` propagates it.

- [ ] **Step 4: Run the tests and the core suite**

Run: `cd rust/termux-sync-core && cargo test --locked github::tests`
Run: `cd rust/termux-sync-core && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/termux-sync-core/src/core/github.rs
git commit -S -m "fix(github): validate URLs, verify assets, stream upload parts"
```

---

### Task 8: Config import is validated and export cannot destroy config

**Files:**
- Modify: `rust/termux-sync-core/src/core/config.rs` (add `validate_patch`)
- Modify: `rust/termux-sync-core/src/core/json.rs` (add `write_json_private`)
- Modify: `rust/termux-sync-core/src/core/maintenance.rs:811-910` (`export_config`, `import_config`)
- Modify: `rust/app/src/frontdoor.rs:221-247` (lazy config)
- Test: `config.rs`, `json.rs` test modules; `cli_integration.rs`

**Interfaces:**
- Produces: `pub fn config::validate_patch(value: &Value) -> Result<(), String>`
- Produces: `pub fn json::write_json_private(path: &Path, value: &Value) -> Result<(), String>`
- Produces: `fn ensure_not_live_config(out: &Path) -> Result<(), String>`
- Consumes: `paths::config_file()`, `paths::resolve_lenient`.

- [ ] **Step 1: Write failing tests**

In `config.rs`:

```rust
#[test]
fn validate_patch_rejects_bad_values() {
    assert!(validate_patch(&json!({"max_backups": 0})).is_err());
    assert!(validate_patch(&json!({"storage": "dropbox"})).is_err());
    assert!(validate_patch(&json!({"compression": "zip"})).is_err());
    assert!(validate_patch(&json!({"github_repo": "owner"})).is_err());
    assert!(validate_patch(&json!({"gdrive_folder_id": "../x"})).is_err());
    assert!(validate_patch(&json!({"unknown_key": 1})).is_err());
    assert!(validate_patch(&json!({"max_backups": 3, "storage": "local"})).is_ok());
}
```

In `json.rs`:

```rust
#[test]
fn private_writer_uses_0600() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    write_json_private(&path, &json!({"a": 1})).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
}
```

In `cli_integration.rs`:

```rust
#[test]
fn setup_recovers_from_broken_config() {
    let fx = Fixture::new();
    fs::write(fx.home.join(".config/termux-sync/config.json"), b"{").unwrap();
    let out = fx.run(&["setup"], Some("1\n\n\n\nn\n"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    let _: Value = serde_json::from_slice(&fs::read(fx.home.join(".config/termux-sync/config.json")).unwrap()).unwrap();
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rust/termux-sync-core && cargo test --locked validate_patch`
Expected: FAIL to compile.
Run: `cd rust/app && cargo test --locked setup_recovers_from_broken_config`
Expected: FAIL with exit 1 from the eager config load.

- [ ] **Step 3: Implement validation**

`validate_patch` accepts only these keys and rules; return `termux_sync.UserError: Invalid config: <detail>`:

- `storage`: `"local" | "gdrive" | "github"`.
- `local_path`: string.
- `gdrive_folder_id`: string; empty or no leading `/`, no `..` segment, no `:`.
- `github_token`: string.
- `github_repo`: string; empty or `owner/name` with `[A-Za-z0-9_.-]`.
- `github_branch`: string; non-empty, no whitespace or control characters.
- `compression`: `"gz" | "bz2" | "xz"`.
- `max_backups`: integer `>= 1`.
- `exclude_patterns`: array of strings.
- `encrypt`: bool.
- `encrypt_password`: string.
- Any other key: `Unknown config key: <key>`.

`import_config` calls `config::validate_patch(&new_cfg)?` before merging. When `config::load_config()` fails, start from `Value::Object(config::default_config())` and print a warning instead of returning.

`export_config` calls `ensure_not_live_config(&out)?` before writing, and writes with `pyjson::write_json_private`.

`frontdoor`: replace the unconditional load with:

```rust
let needs_config = matches!(
    command.as_str(),
    "backup" | "restore" | "list" | "delete" | "status" | "daemon"
);
let redaction_safe = !matches!(
    command.as_str(),
    "setup" | "import-config" | "export-config" | "logs"
);
let cfg = if needs_config || (opts.json_events && redaction_safe) {
    match port::load_config() {
        Ok(cfg) => Some(cfg),
        Err(error) => return report_error(&error),
    }
} else {
    None
};
```

Before this block, make a best-effort load so logging can redact even when the
command itself does not need config:

```rust
if let Ok(cfg) = port::load_config() {
    let _ = port::set_secrets(cfg);
}
```

This needs `events::set_secrets(Vec<String>)` and `port::set_secrets(Value)`.
Add both here; Task 20 wires them into the log writer.

Then pass `cfg.clone().unwrap_or_default()` only in the arms that need it;
`setup`, `schedule`, `export-config`, `import-config`, `logs`, `clear-logs`,
`check`, and `clear-cache` must not fail because of a corrupt config.

- [ ] **Step 4: Run the tests**

Run: `cd rust/termux-sync-core && cargo test --locked validate_patch`
Run: `cd rust/app && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/termux-sync-core/src/core/config.rs rust/termux-sync-core/src/core/json.rs rust/termux-sync-core/src/core/maintenance.rs rust/app/src/frontdoor.rs rust/app/tests/cli_integration.rs
git commit -S -m "fix(config): validate imports, protect export, load config lazily"
```

---

### Task 9: Delete and cache failures exit non-zero

**Files:**
- Modify: `rust/termux-sync-core/src/core/maintenance.rs:43-110` (`delete`), `717-810` (`clear_cache`)
- Test: `rust/app/tests/cli_integration.rs:279-287`, core test module

**Interfaces:**
- Produces: `fn remove_cache_entry(path: &Path) -> Result<u64, String>`
- Consumes: `Storage::delete_backup` (now fallible).

- [ ] **Step 1: Write failing tests**

Update the existing delete assertion:

```rust
let missing_delete = fx.run(&["delete", "--name", "missing", "--yes"], None);
assert_eq!(missing_delete.status.code(), Some(1));
assert!(String::from_utf8_lossy(&missing_delete.stdout).contains("Failed to delete backup"));
```

Add a cache test in `maintenance.rs`:

```rust
#[test]
fn failed_removal_is_an_error() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("cache");
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    assert!(remove_cache_entry(&dir).is_err());
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rust/app && cargo test --locked parse_errors_and_version`
Expected: FAIL because exit code is 0.
Run: `cd rust/termux-sync-core && cargo test --locked failed_removal_is_an_error`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

1. `delete`: after the `Err` arm prints and logs, `return Err(error);`. Do not print twice; the frontdoor reports it once.
2. `clear_cache`: replace the inline removal with `remove_cache_entry`, which returns the removed byte count on success and `Err` on failure. Accumulate only successes; collect failures and, after the loop, return `Err(format!("RuntimeError: Could not remove: {}", failures.join(", ")))` when non-empty. Keep the summary line for the successful part.
3. `remove_cache_entry` handles files, symlinks, and directories, and returns the size measured before removal only after `remove_dir_all`/`remove_file` succeeds.

- [ ] **Step 4: Run the tests**

Run: `cd rust/app && cargo test --locked parse_errors_and_version`
Run: `cd rust/termux-sync-core && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/termux-sync-core/src/core/maintenance.rs rust/app/tests/cli_integration.rs
git commit -S -m "fix(maintenance): return errors for failed delete and cache removal"
```

---

### Task 10: Schedule and daemon cannot panic or double-run

**Files:**
- Modify: `rust/termux-sync-core/src/core/maintenance.rs:112-135` (`load_schedule`, `save_schedule`), `293-305` (`write_daemon_script`), `308-435` (`schedule`, `daemon`)
- Modify: `rust/termux-sync-core/src/core/fsutil.rs` (add `try_lock_exclusive`)
- Test: core test modules; `cli_integration.rs`

**Interfaces:**
- Produces: `fn normalize_schedule(value: Value) -> Result<Value, String>`
- Produces: `fn daemon_script_body(exe: &Path) -> String`
- Produces: `fn shell_quote(value: &str) -> String`
- Produces: `fn write_daemon_script(enabled: bool) -> Result<(), String>`
- Produces: `pub fn fsutil::try_lock_exclusive(path: &Path) -> Result<File, String>`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn non_object_schedule_is_an_error() {
    assert!(normalize_schedule(json!([])).is_err());
    assert!(normalize_schedule(json!({"enabled": "yes"})).is_err());
    assert_eq!(normalize_schedule(json!({"enabled": true, "hour": 3, "minute": 4, "label": "x"})).unwrap()["hour"], 3);
}

#[test]
fn daemon_script_quotes_the_path() {
    let body = daemon_script_body(Path::new("/data/a b/it's"));
    assert!(body.contains("'/data/a b/it'\\''s'"));
    assert!(!body.contains("\"/data"));
}

#[test]
fn second_lock_fails() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cron.lock");
    let first = fsutil::try_lock_exclusive(&path).unwrap();
    assert!(fsutil::try_lock_exclusive(&path).is_err());
    drop(first);
    assert!(fsutil::try_lock_exclusive(&path).is_ok());
}
```

In `cli_integration.rs`:

```rust
#[test]
fn schedule_survives_broken_cron_file() {
    let fx = Fixture::new();
    fs::write(fx.home.join(".config/termux-sync/cron.json"), b"[]").unwrap();
    let out = fx.run(&["schedule"], Some("n\n"));
    assert_eq!(out.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rust/termux-sync-core && cargo test --locked non_object_schedule_is_an_error`
Expected: FAIL to compile.
Run: `cd rust/app && cargo test --locked schedule_survives_broken_cron_file`
Expected: FAIL with exit 101 (panic).

- [ ] **Step 3: Implement**

1. `normalize_schedule`: require an object; `enabled` bool, `hour` int 0–23, `minute` int 0–59, `label` string. Missing keys get defaults `false`, `2`, `0`, `"auto"`. Wrong types return `termux_sync.UserError: Invalid cron.json: <detail>`.
2. `load_schedule` calls `normalize_schedule` on the parsed value.
3. `save_schedule` calls `json::write_json_private(&paths::cron_file(), sched)`; that writer already creates a 0600 temp file and renames it.
4. `schedule`: replace `sched["enabled"] = ...` with `let map = sched.as_object_mut().ok_or("termux_sync.UserError: Invalid cron.json")?;` and `map.insert(...)`.
5. `daemon`: call `load_schedule()`; on error print the message and return `Err` instead of exiting through `?` with a panic-free message. Take `let _lock = fsutil::try_lock_exclusive(&paths::config_dir().join("cron.lock"))?;` at the top of the function and hold it for the loop.
6. `daemon_script_body` uses `shell_quote(current_exe)`. `write_daemon_script(enabled)` removes the script when disabled and propagates `set_permissions` errors.
7. `schedule` calls `write_daemon_script(enable)?`.
8. `fsutil::try_lock_exclusive`: create the file with mode 0600, then:

```rust
use fs4::fs_std::FileExt;
if !file.try_lock_exclusive().map_err(|error| format!("OSError: {error}"))? {
    return Err("termux_sync.UserError: Another daemon is already running".to_string());
}
Ok(file)
```

- [ ] **Step 4: Run the tests**

Run: `cd rust/termux-sync-core && cargo test --locked`
Run: `cd rust/app && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/termux-sync-core/src/core/maintenance.rs rust/termux-sync-core/src/core/fsutil.rs rust/app/tests/cli_integration.rs
git commit -S -m "fix(daemon): validate schedule, lock single instance, quote boot script"
```

---

### Task 11: Status reports only required Rust tools

**Files:**
- Modify: `rust/termux-sync-core/src/core/fsutil.rs` (add `required_tools`)
- Modify: `rust/termux-sync-core/src/core/maintenance.rs:495-525` (status loop)
- Modify: `tui/src/data.rs:442-459` (`probe_tools`)
- Test: `tui/src/data.rs`, `fsutil.rs`

**Interfaces:**
- Produces: `pub fn fsutil::required_tools(storage: &str) -> Vec<(&'static str, bool)>`
- Consumes: `which`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn only_rust_tools_are_required() {
    let local: Vec<_> = required_tools("local").into_iter().map(|(name, _)| name).collect();
    assert_eq!(local, vec!["dpkg"]);
    let gdrive: Vec<_> = required_tools("gdrive").into_iter().map(|(name, _)| name).collect();
    assert_eq!(gdrive, vec!["dpkg", "rclone"]);
}
```

In `tui/src/data.rs`, assert `probe_tools("local", &[]).iter().all(|tool| tool.name != "python3")`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rust/termux-sync-core && cargo test --locked only_rust_tools_are_required`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

`required_tools` returns `vec![("dpkg", true)]` plus `("rclone", true)` for `"gdrive"`. The maintenance status loop and `tui::data::probe_tools` iterate it. Delete the `python3`, `tar`, `curl`, and `git` entries from both places.

- [ ] **Step 4: Run the tests**

Run: `cd rust/termux-sync-core && cargo test --locked`
Run: `cd tui && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/termux-sync-core/src/core/fsutil.rs rust/termux-sync-core/src/core/maintenance.rs tui/src/data.rs
git commit -S -m "fix(status): require only dpkg and optional rclone"
```

---

### Task 12: One argv parser

**Files:**
- Delete: `rust/app/src/cli.rs`, `rust/app/src/units/` (`mod.rs`, `termux_sync_core.rs`)
- Modify: `rust/app/src/main.rs`, `rust/app/src/frontdoor.rs:112-252`, `rust/app/Cargo.toml` (remove `clap`)
- Test: `rust/app/tests/cli_integration.rs`

**Interfaces:**
- Produces: `pub fn frontdoor::run_from_argv() -> ExitCode` remains the only entry.
- Consumes: `termux_sync_tui::main_entry`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn non_utf8_target_is_rejected_without_panic() {
    use std::os::unix::ffi::OsStringExt;
    let fx = Fixture::new();
    let bad = std::ffi::OsString::from_vec(vec![0xff, 0xfe]);
    let out = fx.run_os(&["import-config", bad], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).contains("valid UTF-8"));
}

#[test]
fn flag_before_tui_is_not_silent_success() {
    let fx = Fixture::new();
    let out = fx.run(&["--debug", "tui"], None);
    assert_eq!(out.status.code(), Some(2));
}
```

Add `run_os` to `Fixture` that accepts `&[OsString]` and reuses the same environment setup.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rust/app && cargo test --locked non_utf8_target_is_rejected_without_panic`
Expected: FAIL with exit 101 or 0.
Run: `cd rust/app && cargo test --locked flag_before_tui_is_not_silent_success`
Expected: FAIL with exit 0.

- [ ] **Step 3: Delete the duplicate parser**

1. `main.rs` becomes:

```rust
mod frontdoor;

use std::process::ExitCode;

fn main() -> ExitCode {
    frontdoor::run_from_argv()
}
```

2. Delete `cli.rs` and the `units` directory. Remove `clap` from `rust/app/Cargo.toml`.
3. In `frontdoor`, collect once:

```rust
let args: Vec<OsString> = std::env::args_os().skip(1).collect();
if args.first().is_some_and(|arg| arg == std::ffi::OsStr::new("tui")) {
    return run_tui(&args[1..]);
}
```

`run_tui` calls `termux_sync_tui::main_entry("termux-sync tui", &args.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>())` and maps errors to exit 1.
4. Loop over `args` by reference. Use `arg.to_str()` for flag matching; if it is `None`, push the `OsString` to `positionals`. After the loop, convert the command with `positionals[0].to_str().ok_or_else(|| usage("command must be valid UTF-8"))` and the target with `positionals[1].to_str().ok_or_else(...)`; return exit 2 on failure.
5. Remove `"tui"` from `COMMANDS` and delete the `"tui" => Ok(Value::Null)` arm.

- [ ] **Step 4: Run the app suite**

Run: `cd rust/app && cargo test --locked`
Expected: PASS, including trycmd cases and the TUI `--version` test.

- [ ] **Step 5: Commit**

```bash
git add rust/app/src rust/app/Cargo.toml rust/app/tests/cli_integration.rs
git commit -S -m "refactor(cli): remove duplicate clap parser and fix argv handling"
```

---

### Task 13: Debug flag works and hints stop naming tsctl

**Files:**
- Modify: `rust/termux-sync-core/src/core/maintenance.rs:23-30` (`debug_enabled`), `rust/termux-sync-core/src/port.rs` (add `set_debug`)
- Modify: `rust/app/src/frontdoor.rs:76-95` (hints), `140-145` (`--debug`)
- Test: `rust/app/tests/cli_integration.rs`

**Interfaces:**
- Produces: `pub fn maintenance::set_debug(enabled: bool)`
- Produces: `pub fn port::set_debug(enabled: bool) -> Result<Value, String>`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn debug_flag_shows_trace_lines() {
    let fx = Fixture::new();
    fs::write(fx.home.join(".config/termux-sync/sync.log"),
        "[2026-01-01 00:00:00] [TRACE] detail\n").unwrap();
    let plain = fx.run(&["logs"], None);
    assert!(!String::from_utf8_lossy(&plain.stdout).contains("[TRACE] detail"));
    let debug = fx.run(&["--debug", "logs"], None);
    assert!(String::from_utf8_lossy(&debug.stdout).contains("[TRACE] detail"));
}

#[test]
fn update_hint_uses_install_script() {
    let fx = Fixture::new();
    let out = fx.run(&["update"], None);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("bash install.sh"));
    assert!(!text.contains("tsctl"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rust/app && cargo test --locked debug_flag_shows_trace_lines`
Expected: FAIL. `--debug` does nothing.

- [ ] **Step 3: Implement**

1. In `maintenance.rs`, add `static DEBUG: AtomicBool = AtomicBool::new(false);` and `pub fn set_debug(enabled: bool) { DEBUG.store(enabled, Ordering::Relaxed); }`. `debug_enabled` returns `DEBUG.load(..) || env::var("TERMUX_SYNC_DEBUG").is_ok_and(|v| v == "1")`.
2. `port::set_debug` calls it.
3. `frontdoor`: after parsing, `if opts.debug { let _ = port::set_debug(true); }` before dispatch.
4. Hints: `update`/`upgrade` → `"To update: git pull --ff-only && bash install.sh"`; `install` → `"To install: bash install.sh"`; `remove` → `"To delete a backup use termux-sync delete. To uninstall, delete the two binaries from $PREFIX/bin."`.

- [ ] **Step 4: Run the tests**

Run: `cd rust/app && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/termux-sync-core/src/core/maintenance.rs rust/termux-sync-core/src/port.rs rust/app/src/frontdoor.rs rust/app/tests/cli_integration.rs
git commit -S -m "fix(cli): honor --debug and replace tsctl hints"
```

---

### Task 14: TUI probes, cancel timer, and line reader are correct

**Files:**
- Modify: `tui/src/ops.rs:387-403` (`supports_flag`), `477-490` (line reader), `815-835` (`request_cancel`), `784-790` (`finalize_exit`)
- Test: `tui/src/ops.rs` test module

**Interfaces:**
- Produces: `fn supports_flag_with(bin: &Path, flag: &str) -> bool`
- Produces: `enum LineRead { Eof, Line, Oversize }`
- Produces: `fn read_event_line<R: BufRead>(reader: &mut R, limit: usize, out: &mut String) -> io::Result<LineRead>`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn flag_probe_rejects_version_only_cli() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("fake");
    fs::write(&bin, "#!/bin/sh\n[ \"$1\" = logs ] && [ \"$2\" = --json-events ] && exit 0\nexit 2\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!supports_flag_with(&bin, "--bogus"));
    assert!(supports_flag_with(&bin, "--json-events"));
}

#[test]
fn cancelling_from_confirm_arms_the_timer() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("fake");
    fs::write(&bin, "#!/bin/sh\nread line\nsleep 5\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    // Edition 2024 makes set_var unsafe; tests run single-threaded per binary.
    unsafe { std::env::set_var("TERMUX_SYNC_BIN", &bin) };
    let mut op = Operation::start(Kind::Backup, "x", vec!["backup".into()], false).unwrap();
    op.status = Status::AwaitingConfirm;
    assert!(op.request_cancel());
    assert!(op.cancel_sent_at.is_some());
    op.kill_forcefully();
    let _ = op.poll();
    unsafe { std::env::remove_var("TERMUX_SYNC_BIN") };
}

#[test]
fn oversized_line_is_drained() {
    let mut input = vec![b'x'; 32];
    input.push(b'\n');
    input.extend_from_slice(b"next\n");
    let mut reader = BufReader::new(&input[..]);
    let mut line = String::new();
    assert!(matches!(read_event_line(&mut reader, 8, &mut line).unwrap(), LineRead::Oversize));
    assert!(matches!(read_event_line(&mut reader, 8, &mut line).unwrap(), LineRead::Line));
    assert_eq!(line, "next\n");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd tui && cargo test --locked oversized_line_is_drained`
Expected: FAIL to compile.
Run: `cd tui && cargo test --locked flag_probe_rejects_version_only_cli`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

1. `supports_flag_with` runs `["logs", flag]` (no `--version`) and returns `status.success()`. `supports_flag` calls `data::cli_command(&["logs".to_string(), flag.to_string()])` and uses the same check.
2. `read_event_line` reads with `reader.take(limit + 1)`, returns `Oversize` and drains the rest of the physical line with `read_line` into a scratch string, then `Line`, or `Eof`.
3. Replace the worker loop's `read_line` with `read_event_line(&mut reader, MAX_LINE, &mut line)`; send `Malformed` on `Oversize`.
4. `request_cancel` AwaitingConfirm branch: `let sent = self.send_stdin(...); if sent { self.cancel_sent_at = Some(Instant::now()); } self.status = Status::Cancelling; return true;`.
5. `finalize_exit`: first `if self.status == Status::Cancelling { self.status = Status::Cancelled; return; }`, then `if (self.status == Status::Cancelled || code == Some(130)) && !self.protected { ... }`.

- [ ] **Step 4: Run the TUI suite**

Run: `cd tui && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tui/src/ops.rs
git commit -S -m "fix(tui): correct capability probes, cancel deadline, line bounds"
```

---

### Task 15: TUI strips terminal controls and reports the installed version

**Files:**
- Modify: `tui/src/data.rs:229-260` (`parse_manifest`), `239-330` (`parse_event`), `461-480` (`cli_version`)
- Modify: `tui/src/ui.rs:540-555` (log rendering)
- Test: `tui/src/data.rs` test module

**Interfaces:**
- Produces: `pub fn strip_controls(text: &str) -> String`
- Produces: `pub fn cli_version_with(bin: &Path) -> Option<String>`
- Consumes: `parse_cli_version`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn strips_escape_sequences_from_labels_and_logs() {
    assert_eq!(strip_controls("ok\u{1b}[31mred\u{1b}[0m"), "ok[31mred[0m");
    assert_eq!(strip_controls("a\u{0007}b"), "ab");
    let backup = parse_manifest(&json!({"name": "termux_backup_x", "label": "x\u{1b}]0;pwned\u{0007}"})).unwrap();
    assert!(!backup.label.contains('\u{1b}'));
}

#[test]
fn cli_version_queries_the_binary() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("fake");
    fs::write(&bin, "#!/bin/sh\nprintf 'termux-sync v9.9.9\\n'\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(cli_version_with(&bin), Some("termux-sync v9.9.9".to_string()));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd tui && cargo test --locked strips_escape_sequences_from_labels_and_logs`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

1. `strip_controls` removes `char::is_control()` characters except `\n` and `\t`.
2. Apply it to `label` and `name` in `parse_manifest`; to `Log.message`, `Stage.label`, `Plan` strings, `OpError.message`/`hint`, and `Cancelled.reason` in `parse_event`; and to `entry.message` in `ui.rs` before `Span::raw`.
3. `cli_version_with` spawns `<bin> --version`, captures stdout, and calls `parse_cli_version(status.success(), &stdout)`. `cli_version()` calls it with `cli_bin()`.
4. Add `pub fn cli_bin() -> PathBuf` reading `TERMUX_SYNC_BIN` with `"termux-sync"` fallback; use it in `cli_command`.

- [ ] **Step 4: Run the TUI suite**

Run: `cd tui && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tui/src/data.rs tui/src/ui.rs
git commit -S -m "fix(tui): strip terminal controls and query installed version"
```

---

### Task 16: TUI reads bounded data and uses the configured CLI

**Files:**
- Modify: `tui/src/data.rs:355-365` (`tail_log`), `tui/src/ops.rs:376-385` (`collect_backups`), `tui/src/app.rs:1609-1618` (output)
- Modify: `tui/src/history.rs:32-45` (0600)
- Test: `tui/src/data.rs`, `tui/src/ops.rs`, `tui/src/history.rs`

**Interfaces:**
- Produces: `pub fn collect_backups_with(bin: &Path) -> Result<Vec<Backup>, String>`
- Produces: `fn tail_log_bounded(path: &Path, lines: usize, max_bytes: usize) -> Vec<String>`
- Produces: `pub(crate) const LOG_LIMIT: usize` in `ops.rs`
- Modifies: `HistoryStore::save` writes mode 0600 (no new name)

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn tail_reads_only_the_end_of_a_large_log() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("sync.log");
    let mut text = String::new();
    for i in 0..50_000 { text.push_str(&format!("line {i}\n")); }
    fs::write(&path, text).unwrap();
    let tail = tail_log_bounded(&path, 2, 4096);
    assert_eq!(tail.len(), 2);
    assert!(tail[1].starts_with("line 49999"));
}

#[test]
fn backup_list_uses_the_configured_cli() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("fake");
    fs::write(&bin, "#!/bin/sh\nprintf '%s\\n' '{\"event\":\"backup\",\"name\":\"termux_backup_a\",\"date\":\"2026-01-01\",\"size\":1}'\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    let list = collect_backups_with(&bin).unwrap();
    assert_eq!(list[0].name, "termux_backup_a");
}

#[test]
fn history_file_is_private() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("history.json");
    HistoryStore::save(&path, &[]).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd tui && cargo test --locked tail_reads_only_the_end_of_a_large_log`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

1. `tail_log_bounded`: open the file, seek to `len.saturating_sub(max_bytes as u64)`, read to string, drop the first partial line when the start was non-zero, keep the last `lines`. `tail_log` calls it with `max_bytes = 256 * 1024`.
2. `collect_backups_with`: run `<bin> list --json-events`, parse each stdout line with `parse_event`, collect `Event::Backup`. `collect_backups` uses `data::cli_bin()`.
3. `app.rs` `WorkerMsg::Output`: keep at most `crate::ops::LOG_LIMIT` lines from the tail:

```rust
let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
if lines.len() > crate::ops::LOG_LIMIT {
    lines = lines.split_off(lines.len() - crate::ops::LOG_LIMIT);
}
```

Change the existing `const LOG_LIMIT` in `ops.rs` to `pub(crate) const LOG_LIMIT`.

4. `HistoryStore::save`: open the temp file with `OpenOptions::new().write(true).create(true).truncate(true).mode(0o600)` and `write_all`, then rename. The history test calls `HistoryStore::save` and asserts mode 0600.

- [ ] **Step 4: Run the TUI suite**

Run: `cd tui && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tui/src/data.rs tui/src/ops.rs tui/src/app.rs tui/src/history.rs
git commit -S -m "fix(tui): bound log reads, use configured CLI, private history"
```

---

### Task 17: Documentation tells the truth

**Files:**
- Modify: `tui/README.md`, `tui/RUNBOOK.md`, `rust/GLOSSARY.md`, `CHANGELOG.md`
- Modify: `.github/ISSUE_TEMPLATE/bug_report.md`, `.github/ISSUE_TEMPLATE/bug_report.yml`, `.github/PULL_REQUEST_TEMPLATE.md`
- Test: grep-based verification

**Interfaces:**
- Consumes: none.
- Produces: none.

- [ ] **Step 1: Rewrite the TUI docs**

`tui/README.md`: replace the "spike on branch `spike/rust-tui`" paragraph with a description of the released Rust TUI, its CLI dependency, and its `TERMUX_SYNC_BIN` override. `tui/RUNBOOK.md`: replace "Python event protocol", "redaction and import validation stay in Python", and "Python cleanup path" with the Rust module names (`tui/src/ops.rs`, `termux-sync-core`).

`rust/GLOSSARY.md`: change the `fallback` row to "A command the Rust front door does not implement; it exits with an unknown-command error." Delete the row if no such concept remains.

`CHANGELOG.md`: add under `[Unreleased]` a `### Fixed` entry for the data-loss fixes and a `### Changed` entry for the CLI parser consolidation.

- [ ] **Step 2: Remove Python from templates**

`.github/ISSUE_TEMPLATE/bug_report.yml`: delete the Python-version question and the "older Python release" line. `.github/ISSUE_TEMPLATE/bug_report.md`: delete the Python-version field. `.github/PULL_REQUEST_TEMPLATE.md`: replace "No new external Python dependencies" with "No new external Rust dependencies without a note in the PR".

- [ ] **Step 3: Verify no stale references remain**

Run:

```bash
grep -rn "Python version\|python3\|tsctl\|spike/rust-tui\|stay in Python" tui/README.md tui/RUNBOOK.md rust/GLOSSARY.md .github/ISSUE_TEMPLATE .github/PULL_REQUEST_TEMPLATE.md
```

Expected: no output.

- [ ] **Step 4: Commit**

```bash
git add tui/README.md tui/RUNBOOK.md rust/GLOSSARY.md CHANGELOG.md .github
git commit -S -m "docs: describe the Rust release and remove Python-era guidance"
```

---

### Task 18: CI and installers use locked builds

**Files:**
- Modify: `rust/install.sh:10-13`
- Modify: `.github/workflows/rust.yml:3-14`
- Test: shell syntax and a locked build

**Interfaces:**
- Consumes: existing `Cargo.lock` files.
- Produces: none.

- [ ] **Step 1: Add `--locked`**

`rust/install.sh`:

```bash
cargo build --locked --release --manifest-path "$ROOT/rust/app/Cargo.toml"
cargo build --locked --release --manifest-path "$ROOT/tui/Cargo.toml"
```

- [ ] **Step 2: Cover the root installer in CI**

Add `- "install.sh"` to both `paths` lists in `.github/workflows/rust.yml`.

- [ ] **Step 3: Verify**

Run:

```bash
bash -n install.sh rust/install.sh tui/install.sh
cd rust/app && cargo build --locked --release
cd ../../tui && cargo build --locked --release
```

Expected: all exit 0.

- [ ] **Step 4: Commit**

```bash
git add rust/install.sh .github/workflows/rust.yml
git commit -S -m "ci: build locked and watch the root installer"
```

---

### Task 19: Test harness catches hangs and runs anywhere

**Files:**
- Modify: `tui/tests/pty.sh`
- Modify: `rust/app-cases/*.toml` (PATH lines)
- Modify: `rust/app/tests/cli_integration.rs`
- Test: the modified suites

**Interfaces:**
- Consumes: the binaries under test.
- Produces: none.

- [ ] **Step 1: Make the PTY cases fail on timeout**

Replace `timeout 15 script ... || true` with:

```bash
set +e
timeout 15 script -qec "stty rows 24 cols 90; $BIN" /dev/null >"$out" 2>&1
status=$?
set -e
if [[ $status -ne 0 ]]; then
    printf '%s: exit %s\n' "$name" "$status"
    return 1
fi
```

Replace fixed `sleep` feeds with a `wait_for` helper that polls the output file for the next expected pattern, up to 10 s, then sends the next key. Keep the existing assertions as the patterns.

- [ ] **Step 2: Remove device-absolute PATH from fixtures**

In every `rust/app-cases/*.toml`, set `env.add.PATH` to the fixture `bin` directory only (for example `rust/fixtures/app/bin`). The fixtures provide `dpkg`, `pkg`, and `rclone`; no host tool is needed.

- [ ] **Step 3: Add error-path integration cases**

Add to `cli_integration.rs`:

```rust
#[test]
fn error_paths_exit_non_zero() {
    let fx = Fixture::new();
    assert_eq!(fx.run(&["import-config", "/nope/missing.json"], None).status.code(), Some(1));
    assert_eq!(fx.run(&["restore", "--name", "nope"], None).status.code(), Some(1));
    assert!(fx.run(&["logs", "--lines=3"], None).status.success());
    assert_eq!(fx.run(&["delete", "-n", "missing", "--yes"], None).status.code(), Some(1));
    assert!(fx.run(&["backup", "-l", "from-flag"], None).status.success());
}
```

The `-l` case runs a real local backup in the fixture; the fixture already
supports it. The `-n` case proves the short name flag parsed because the
command reaches the delete path and fails on the missing backup.

- [ ] **Step 4: Verify**

Run: `cd rust/app && cargo test --locked`
Run: `cd tui && cargo test --locked`
Run: `bash tui/tests/pty.sh` on a device with a TTY.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tui/tests/pty.sh rust/app-cases rust/app/tests/cli_integration.rs
git commit -S -m "test: fail on PTY hangs and add error-path coverage"
```

---

### Task 20: Logs are redacted and migration leftovers are gone

**Files:**
- Modify: `rust/termux-sync-core/src/core/events.rs:41-60` (expose redaction)
- Modify: `rust/termux-sync-core/src/core/logging.rs:12-32`
- Delete: `rust/app/src/main.rs` `report_fallbacks` (already removed by Task 12), `rust/app-cases/rust-anything.json`, `rust/app-cases/replay-report.md`
- Test: `events.rs` test module

**Interfaces:**
- Produces: `pub fn events::redact_message(message: &str) -> String`
- Consumes: `events::set_secrets` and `port::set_secrets` added in Task 8.
- Consumes: `events::enable` keeps its event-mode behavior.

- [ ] **Step 1: Write a failing test**

```rust
#[test]
fn log_messages_are_redacted() {
    set_secrets(vec!["secret-token".to_string()]);
    assert_eq!(redact_message("token=secret-token"), "token=***");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd rust/termux-sync-core && cargo test --locked log_messages_are_redacted`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

1. Use the `events::set_secrets` and `port::set_secrets` added in Task 8. Add `pub fn redact_message(message: &str) -> String` that applies the stored list. Keep `enable` as the event-mode switch.
2. `logging::log` computes `let message = events::redact_message(message);` before writing the file, and emits the redacted message. `events::emit` may redact again; the replacement marker is idempotent.
3. Delete `rust/app-cases/rust-anything.json` and `rust/app-cases/replay-report.md`. Confirm `rust/fixtures/run-original.sh` does not exist; no `.toml` references it.

- [ ] **Step 4: Verify**

Run: `cd rust/termux-sync-core && cargo test --locked`
Run: `cd rust/app && cargo test --locked`
Expected: PASS. The trycmd suite must still pass without the deleted metadata.

- [ ] **Step 5: Commit**

```bash
git add rust/termux-sync-core/src/core/events.rs rust/termux-sync-core/src/core/logging.rs rust/app-cases
git commit -S -m "fix(logging): redact before writing and drop migration leftovers"
```

---

### Task 21: Split the large TUI modules (optional, do last)

**Files:**
- Create: `tui/src/ui/mod.rs`, `tui/src/ui/dashboard.rs`, `tui/src/ui/operation.rs`, `tui/src/ui/overlay.rs`
- Create: `tui/src/app/mod.rs`, `tui/src/app/state.rs`, `tui/src/app/input.rs`, `tui/src/app/tasks.rs`, `tui/src/app/commands.rs`
- Delete: `tui/src/ui.rs`, `tui/src/app.rs`
- Test: existing TUI tests

**Interfaces:**
- Produces: unchanged public items; `mod ui;` and `mod app;` resolve to the new directories.
- Consumes: everything from Tasks 14-16.

- [ ] **Step 1: Move without editing**

Move dashboard rendering, operation rendering, and overlays into the three `ui` modules; move `App` state, key handling, worker/task handling, and argv builders into the four `app` modules. Re-export the public items from `ui/mod.rs` and `app/mod.rs` so `lib.rs` and `main.rs` need no changes. Change cross-module items from private to `pub(super)` where a sibling module calls them; leave everything else private.

- [ ] **Step 2: Verify the move changed no behavior**

Run: `cd tui && cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked`
Expected: PASS with the same test count as before the split.

- [ ] **Step 3: Commit**

```bash
git add tui/src
git commit -S -m "refactor(tui): split ui and app modules by responsibility"
```

---

## Final Verification

- [ ] `cd rust/termux-sync-core && cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked`
- [ ] `cd rust/app && cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked`
- [ ] `cd tui && cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked`
- [ ] `bash tui/tests/pty.sh` on a device with a TTY
- [ ] Manual smoke on a fixture: `setup` changes persist; a backup with `max_backups: 0` keeps one; a corrupt `home.tar.gz` restore exits non-zero; `cron.json` containing `[]` exits 1 without a panic; `--debug logs` shows TRACE lines.
- [ ] `git status --porcelain` shows only intended changes; `.deja/` is untouched.
