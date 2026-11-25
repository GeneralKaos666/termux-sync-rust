use std::fs;
use std::io::Cursor;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tar::{Builder, EntryType, Header};

struct Fixture {
    temp: tempfile::TempDir,
    home: PathBuf,
    prefix: PathBuf,
    tmp: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let prefix = temp.path().join("prefix");
        let tmp = temp.path().join("tmp");
        let bin = temp.path().join("bin");
        for dir in [
            home.join(".config/termux-sync"),
            home.join("data"),
            prefix.join("etc"),
            prefix.join("tmp"),
            tmp.clone(),
            bin.clone(),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(home.join("data/note.txt"), "keep me\n").unwrap();
        fs::write(prefix.join("etc/profile.sample"), "prefix\n").unwrap();
        fs::write(
            home.join(".config/termux-sync/config.json"),
            r#"{
  "storage": "local",
  "local_path": "~/backups",
  "gdrive_folder_id": "",
  "github_token": "secret-token",
  "github_repo": "",
  "github_branch": "main",
  "compression": "gz",
  "exclude_patterns": [".cache"],
  "max_backups": 5,
  "encrypt": false,
  "encrypt_password": "secret-pass"
}"#,
        )
        .unwrap();
        fs::write(
            home.join(".config/termux-sync/sync.log"),
            "[2025-01-01 00:00:00] [INFO] fixture\n",
        )
        .unwrap();
        for (name, body) in [
            (
                "dpkg",
                "#!/bin/sh\nprintf 'bash\\tinstall\\ncoreutils\\tinstall\\n'\n",
            ),
            ("pkg", "#!/bin/sh\nexit 0\n"),
            ("rclone", "#!/bin/sh\nexit 0\n"),
        ] {
            let path = bin.join(name);
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self {
            temp,
            home,
            prefix,
            tmp,
            bin,
        }
    }

    fn run(&self, args: &[&str], stdin: Option<&str>) -> Output {
        self.run_env(args, stdin, &[])
    }

    fn run_env(&self, args: &[&str], stdin: Option<&str>, extra_env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_termux-sync-rs"));
        command
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PREFIX", &self.prefix)
            .env("TMPDIR", &self.tmp)
            .env("COLUMNS", "200")
            .env("NO_COLOR", "1")
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let mut child = command.spawn().unwrap();
        if let Some(input) = stdin {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        child.wait_with_output().unwrap()
    }

    fn config(&self) -> Value {
        serde_json::from_slice(
            &fs::read(self.home.join(".config/termux-sync/config.json")).unwrap(),
        )
        .unwrap()
    }

    fn backup_result(&self, output: &Output) -> String {
        assert!(
            output.status.success(),
            "backup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Ok(event) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if event.get("event").and_then(Value::as_str) == Some("result") {
                return event
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap()
                    .to_string();
            }
        }
        panic!("backup result event missing");
    }

    fn write_corrupt_backup(&self, name: &str) {
        let dir = self.home.join("backups").join(name);
        fs::create_dir_all(&dir).unwrap();
        let corrupt = b"not a tar";
        fs::write(dir.join("home.tar.gz"), corrupt).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(corrupt);
        let checksum = format!("{:x}", hasher.finalize());
        let manifest = serde_json::json!({
            "name": name,
            "files": {
                "home": {
                    "archive": "home.tar.gz",
                    "size": corrupt.len(),
                    "checksum": checksum,
                }
            },
            "packages": [],
        });
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    fn write_partial_backup(&self, name: &str) {
        let dir = self.home.join("backups").join(name);
        fs::create_dir_all(&dir).unwrap();
        let archive_path = dir.join("home.tar.gz");
        let file = fs::File::create(&archive_path).unwrap();
        let mut builder = Builder::new(file);
        // A valid directory and file, followed by a traversal member that is
        // refused during extraction (a warning) while the rest still extracts.
        builder
            .append(
                &raw_header("home/", 0, EntryType::Directory),
                Cursor::new(Vec::<u8>::new()),
            )
            .unwrap();
        builder
            .append(
                &raw_header("home/ok.txt", 3, EntryType::Regular),
                Cursor::new(b"ok\n".to_vec()),
            )
            .unwrap();
        builder
            .append(
                &raw_header("../evil.txt", 1, EntryType::Regular),
                Cursor::new(b"x".to_vec()),
            )
            .unwrap();
        builder.into_inner().unwrap();
        let bytes = fs::read(&archive_path).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let checksum = format!("{:x}", hasher.finalize());
        let manifest = serde_json::json!({
            "name": name,
            "files": {
                "home": {
                    "archive": "home.tar.gz",
                    "size": bytes.len(),
                    "checksum": checksum,
                }
            },
            "packages": [],
        });
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    fn write_fifo_backup(&self, name: &str) {
        let dir = self.home.join("backups").join(name);
        fs::create_dir_all(&dir).unwrap();
        let archive_path = dir.join("home.tar.gz");
        let file = fs::File::create(&archive_path).unwrap();
        let mut builder = Builder::new(file);
        // Valid directory and file plus a fifo entry, which extraction skips
        // with an ignorable warning (so restore still succeeds).
        builder
            .append(
                &raw_header("home/", 0, EntryType::Directory),
                Cursor::new(Vec::<u8>::new()),
            )
            .unwrap();
        builder
            .append(
                &raw_header("home/ok.txt", 3, EntryType::Regular),
                Cursor::new(b"ok\n".to_vec()),
            )
            .unwrap();
        builder
            .append(
                &raw_header("home/fifo", 0, EntryType::Fifo),
                Cursor::new(Vec::<u8>::new()),
            )
            .unwrap();
        builder.into_inner().unwrap();
        let bytes = fs::read(&archive_path).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let checksum = format!("{:x}", hasher.finalize());
        let manifest = serde_json::json!({
            "name": name,
            "files": {
                "home": {
                    "archive": "home.tar.gz",
                    "size": bytes.len(),
                    "checksum": checksum,
                }
            },
            "packages": [],
        });
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }
}

fn raw_header(name: &str, size: u64, kind: EntryType) -> Header {
    let mut header = Header::new_gnu();
    header.set_size(size);
    let mode = if kind.is_dir() {
        0o755
    } else if kind.is_symlink() {
        0o777
    } else {
        0o644
    };
    header.set_mode(mode);
    header.set_entry_type(kind);
    header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
    header.set_cksum();
    header
}

#[test]
fn backup_list_restore_and_delete_local() {
    let fx = Fixture::new();
    let backup = fx.run(&["--json-events", "backup", "--label", "smoke"], None);
    let name = fx.backup_result(&backup);
    let dir = fx.home.join("backups").join(&name);
    assert!(dir.join("manifest.json").is_file());
    assert!(dir.join("home.tar.gz").is_file());

    let listed = fx.run(&["--json-events", "list"], None);
    assert!(listed.status.success());
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(stdout.contains("\"event\":\"backup\""));
    assert!(stdout.contains(&name));

    fs::remove_file(fx.home.join("data/note.txt")).unwrap();
    let restored = fx.run(&["restore", "--name", &name], Some("y\n"));
    assert!(
        restored.status.success(),
        "restore failed: {}",
        String::from_utf8_lossy(&restored.stdout)
    );
    assert_eq!(
        fs::read_to_string(fx.home.join("data/note.txt")).unwrap(),
        "keep me\n"
    );

    let deleted = fx.run(&["delete", "--name", &name, "--yes"], None);
    assert!(deleted.status.success());
    assert!(!dir.exists());
}

#[test]
fn status_logs_check_and_cache() {
    let fx = Fixture::new();
    let status = fx.run(&["status"], None);
    assert!(status.status.success());
    let text = String::from_utf8_lossy(&status.stdout);
    assert!(text.contains("LOCAL"));
    assert!(text.contains("App version v2.0.0"));

    let logs = fx.run(&["logs", "--lines", "3"], None);
    assert!(logs.status.success());
    assert!(String::from_utf8_lossy(&logs.stdout).contains("fixture"));

    let cleared = fx.run(&["clear-logs", "--yes"], None);
    assert!(cleared.status.success());
    let log = fs::read_to_string(fx.home.join(".config/termux-sync/sync.log")).unwrap();
    assert!(!log.contains("fixture"));
    assert!(log.contains("Log file cleared by user"));

    let check = fx.run(&["check", "~"], None);
    assert!(check.status.success());
    assert!(String::from_utf8_lossy(&check.stdout).contains("DISK CHECK"));

    fs::create_dir_all(fx.home.join(".cache/drop")).unwrap();
    fs::write(fx.home.join(".cache/drop/file"), "x").unwrap();
    let cache = fx.run(&["clear-cache", "--yes"], None);
    assert!(cache.status.success());
    assert!(!fx.home.join(".cache/drop").exists());
}

#[test]
fn export_redacts_and_import_merges() {
    let fx = Fixture::new();
    let export = fx.temp.path().join("export.json");
    let out = fx.run(&["export-config", export.to_str().unwrap()], None);
    assert!(out.status.success());
    let exported = fs::read_to_string(&export).unwrap();
    assert!(exported.contains("*** REDACTED ***"));
    assert!(!exported.contains("secret-token"));
    assert!(!exported.contains("secret-pass"));

    let import = fx.temp.path().join("import.json");
    fs::write(
        &import,
        r#"{"compression":"bz2","max_backups":3,"github_token":"*** REDACTED ***"}"#,
    )
    .unwrap();
    let out = fx.run(&["import-config", import.to_str().unwrap()], None);
    assert!(out.status.success());
    let cfg = fx.config();
    assert_eq!(cfg["compression"], "bz2");
    assert_eq!(cfg["max_backups"], 3);
    assert_eq!(cfg["github_token"], "secret-token");
}

#[test]
fn setup_and_schedule_write_state() {
    let fx = Fixture::new();
    let setup = fx.run(&["setup"], Some("1\n\nbz2\n7\nn\n"));
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stdout)
    );
    assert_eq!(fx.config()["compression"], "bz2");
    assert_eq!(fx.config()["max_backups"], 7);

    let schedule = fx.run(&["schedule"], Some("n\n"));
    assert!(schedule.status.success());
    let cron: Value =
        serde_json::from_slice(&fs::read(fx.home.join(".config/termux-sync/cron.json")).unwrap())
            .unwrap();
    assert_eq!(cron["enabled"], false);
    let boot = fx.home.join(".termux/boot/termux-sync-daemon.sh");
    assert!(boot.is_file());
    assert!(fs::metadata(&boot).unwrap().permissions().mode() & 0o111 != 0);
}

#[test]
fn setup_keeps_stored_token_on_empty_answer() {
    let fx = Fixture::new();
    let out = fx.run(&["setup"], Some("3\n\nowner/repo\n\n\n\nn\n"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(fx.config()["github_token"], "secret-token");
    assert_eq!(fx.config()["github_repo"], "owner/repo");
}

#[test]
fn parse_errors_and_version() {
    let fx = Fixture::new();
    let version = fx.run(&["--version"], None);
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        "termux-sync v2.0.0"
    );

    let unknown = fx.run(&["bogus"], None);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&unknown.stdout).contains("Unknown command"));

    let bad_lines = fx.run(&["logs", "--lines", "x"], None);
    assert_eq!(bad_lines.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad_lines.stdout).contains("invalid int value"));

    let backup = fx.run(&["--json-events", "backup", "--label", "delete"], None);
    let _ = fx.backup_result(&backup);
    let tui_version = fx.run(&["tui", "--version"], None);
    assert!(tui_version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&tui_version.stdout).trim(),
        "termux-sync tui 2.0.0"
    );
    let tui_help = fx.run(&["tui", "--help"], None);
    assert!(tui_help.status.success());
    assert!(String::from_utf8_lossy(&tui_help.stdout).contains("Usage: termux-sync tui"));
    let tui_no_tty = fx.run(&["tui"], None);
    assert_eq!(tui_no_tty.status.code(), Some(2));

    let missing_delete = fx.run(&["delete", "--name", "missing", "--yes"], None);
    assert!(missing_delete.status.success());
    let text = String::from_utf8_lossy(&missing_delete.stdout);
    assert!(text.contains("Failed to delete backup"));
    assert!(!text.contains("FileNotFoundError"));
}

#[test]
fn backup_fails_when_an_archive_fails() {
    let fx = Fixture::new();
    let out = fx.run_env(
        &["backup"],
        None,
        &[("TERMUX_SYNC_TEST_ARCHIVE_FAIL", "home")],
    );
    assert!(!out.status.success(), "backup reported success");
    assert!(String::from_utf8_lossy(&out.stdout).contains("Failed to archive"));
}

#[test]
fn restore_fails_when_extraction_fails() {
    let fx = Fixture::new();
    fx.write_corrupt_backup("termux_backup_bad");
    let out = fx.run(
        &["--json-events", "restore", "--name", "termux_backup_bad"],
        Some("{\"confirm\":\"RESTORE\"}\n"),
    );
    assert!(!out.status.success(), "restore reported success");
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"ok\":false"));
}

#[test]
fn restore_fails_on_partial_extraction() {
    let fx = Fixture::new();
    fx.write_partial_backup("termux_backup_partial");
    let out = fx.run(
        &[
            "--json-events",
            "restore",
            "--name",
            "termux_backup_partial",
        ],
        Some("{\"confirm\":\"RESTORE\"}\n"),
    );
    assert!(!out.status.success(), "restore reported success");
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"ok\":false"));
}

#[test]
fn restore_succeeds_when_only_device_fifo_refused() {
    let fx = Fixture::new();
    fx.write_fifo_backup("termux_backup_fifo");
    let out = fx.run(
        &["--json-events", "restore", "--name", "termux_backup_fifo"],
        Some("{\"confirm\":\"RESTORE\"}\n"),
    );
    assert!(
        out.status.success(),
        "restore failed: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"ok\":true"));
}
