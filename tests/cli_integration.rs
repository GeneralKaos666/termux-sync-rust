//! End-to-end CLI tests against a temporary HOME/PREFIX fixture.
use std::ffi::OsString;
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

    fn run_os(&self, args: &[OsString], stdin: Option<&str>) -> Output {
        let args: Vec<OsString> = args.to_vec();
        self.run_os_env(&args, stdin, &[])
    }

    fn run_env(&self, args: &[&str], stdin: Option<&str>, extra_env: &[(&str, &str)]) -> Output {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        self.run_os_env(&args, stdin, extra_env)
    }

    fn run_os_env(
        &self,
        args: &[OsString],
        stdin: Option<&str>,
        extra_env: &[(&str, &str)],
    ) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_termux-sync"));
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
        if let Some(input) = stdin
            && let Some(mut pipe) = child.stdin.take()
        {
            // A command that never reads stdin closes the pipe early; EPIPE is
            // expected here and must not panic the test.
            let _ = pipe.write_all(input.as_bytes());
        }
        child.wait_with_output().unwrap()
    }

    fn config(&self) -> Value {
        serde_json::from_slice(
            &fs::read(self.home.join(".config/termux-sync/config.json")).unwrap(),
        )
        .unwrap()
    }

    #[expect(
        clippy::unused_self,
        reason = "kept on the fixture so call sites read uniformly"
    )]
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

    fn with_backup(label: &str) -> (Self, String) {
        let fx = Self::new();
        let backup = fx.run(&["--json-events", "backup", "--label", label], None);
        let name = fx.backup_result(&backup);
        (fx, name)
    }

    fn write_corrupt_backup(&self, name: &str) {
        let dir = self.home.join("backups").join(name);
        fs::create_dir_all(&dir).unwrap();
        let corrupt = b"not a tar";
        fs::write(dir.join("home.tar.gz"), corrupt).unwrap();
        self.write_backup_manifest(name, corrupt);
    }

    /// Write a manifest whose `home` archive checksum and size match `archive`.
    fn write_backup_manifest(&self, name: &str, archive: &[u8]) {
        let mut hasher = Sha256::new();
        hasher.update(archive);
        let checksum = format!("{:x}", hasher.finalize());
        let manifest = serde_json::json!({
            "name": name,
            "files": {
                "home": {
                    "archive": "home.tar.gz",
                    "size": archive.len(),
                    "checksum": checksum,
                }
            },
            "packages": [],
        });
        fs::write(
            self.home.join("backups").join(name).join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    fn write_archive_backup(&self, name: &str, entries: &[(&str, u64, EntryType)]) {
        let dir = self.home.join("backups").join(name);
        fs::create_dir_all(&dir).unwrap();
        let archive_path = dir.join("home.tar.gz");
        let file = fs::File::create(&archive_path).unwrap();
        let mut builder = Builder::new(file);
        for (member, size, kind) in entries {
            builder
                .append(
                    &raw_header(member, *size, *kind),
                    Cursor::new(vec![b'x'; *size as usize]),
                )
                .unwrap();
        }
        builder.into_inner().unwrap();
        let bytes = fs::read(&archive_path).unwrap();
        self.write_backup_manifest(name, &bytes);
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
fn backup_writes_manifest_and_archive() {
    let (fx, name) = Fixture::with_backup("smoke");
    let dir = fx.home.join("backups").join(&name);
    assert!(dir.join("manifest.json").is_file());
    assert!(dir.join("home.tar.gz").is_file());

    let backup = fx.run(&["backup", "-l", "from-flag"], None);
    assert!(
        backup.status.success(),
        "backup -l failed: {}",
        String::from_utf8_lossy(&backup.stdout)
    );
}

#[test]
fn list_reports_each_backup_event() {
    let (fx, first) = Fixture::with_backup("first");
    let second = fx.backup_result(&fx.run(&["--json-events", "backup", "--label", "second"], None));

    let listed = fx.run(&["--json-events", "list"], None);
    assert!(listed.status.success());
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(stdout.contains("\"event\":\"backup\""));
    assert!(stdout.contains(&first));
    assert!(stdout.contains(&second));
}

#[test]
fn restore_recovers_a_deleted_file() {
    let (fx, name) = Fixture::with_backup("smoke");
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
}

#[test]
fn delete_removes_the_backup_directory() {
    let (fx, name) = Fixture::with_backup("smoke");
    let dir = fx.home.join("backups").join(&name);
    let deleted = fx.run(&["delete", "--name", &name, "--yes"], None);
    assert!(deleted.status.success());
    assert!(!dir.exists());
}

#[test]
fn status_reports_local_storage_and_version() {
    let fx = Fixture::new();
    let status = fx.run(&["status"], None);
    assert!(status.status.success());
    let text = String::from_utf8_lossy(&status.stdout);
    assert!(text.contains("LOCAL"));
    assert!(text.contains("App version v2.0.0"));
}

#[test]
fn logs_print_recent_lines() {
    let fx = Fixture::new();
    let logs = fx.run(&["logs", "--lines", "3"], None);
    assert!(logs.status.success());
    assert!(String::from_utf8_lossy(&logs.stdout).contains("fixture"));

    assert!(fx.run(&["logs", "--lines=3"], None).status.success());
}

#[test]
fn clear_logs_empties_the_log() {
    let fx = Fixture::new();
    let cleared = fx.run(&["clear-logs", "--yes"], None);
    assert!(cleared.status.success());
    let log = fs::read_to_string(fx.home.join(".config/termux-sync/sync.log")).unwrap();
    assert!(!log.contains("fixture"));
    assert!(log.contains("Log file cleared by user"));
}

#[test]
fn check_reports_disk_check() {
    let fx = Fixture::new();
    let check = fx.run(&["check", "~"], None);
    assert!(check.status.success());
    assert!(String::from_utf8_lossy(&check.stdout).contains("DISK CHECK"));
}

#[test]
fn clear_cache_removes_cache_dir() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.home.join(".cache/drop")).unwrap();
    fs::write(fx.home.join(".cache/drop/file"), "x").unwrap();
    let cache = fx.run(&["clear-cache", "--yes"], None);
    assert!(cache.status.success());
    assert!(!fx.home.join(".cache/drop").exists());
}

#[test]
fn export_config_redacts_secrets() {
    let fx = Fixture::new();
    let export = fx.temp.path().join("export.json");
    let out = fx.run(&["export-config", export.to_str().unwrap()], None);
    assert!(out.status.success());
    let exported = fs::read_to_string(&export).unwrap();
    assert!(exported.contains("*** REDACTED ***"));
    assert!(!exported.contains("secret-token"));
    assert!(!exported.contains("secret-pass"));
}

#[test]
fn import_config_merges_values() {
    let fx = Fixture::new();
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
    assert!(
        !boot.exists(),
        "disabled schedule must remove the boot script"
    );

    let enable = fx.run(&["schedule"], Some("y\n3\n4\n\n"));
    assert!(
        enable.status.success(),
        "{}",
        String::from_utf8_lossy(&enable.stdout)
    );
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
fn setup_recovers_from_broken_config() {
    let fx = Fixture::new();
    fs::write(fx.home.join(".config/termux-sync/config.json"), b"{").unwrap();
    let out = fx.run(&["setup"], Some("1\n\n\n\nn\n"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let _: Value =
        serde_json::from_slice(&fs::read(fx.home.join(".config/termux-sync/config.json")).unwrap())
            .unwrap();
}

#[test]
fn version_prints_release_string() {
    let fx = Fixture::new();
    let version = fx.run(&["--version"], None);
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        "termux-sync v2.0.0"
    );
}

#[test]
fn unknown_command_exits_two() {
    let fx = Fixture::new();
    let unknown = fx.run(&["bogus"], None);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&unknown.stdout).contains("Unknown command"));
}

#[test]
fn bad_lines_value_exits_two() {
    let fx = Fixture::new();
    let bad_lines = fx.run(&["logs", "--lines", "x"], None);
    assert_eq!(bad_lines.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad_lines.stdout).contains("invalid int value"));
}

#[test]
fn tui_version_prints_release_string() {
    let fx = Fixture::new();
    let tui_version = fx.run(&["tui", "--version"], None);
    assert!(tui_version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&tui_version.stdout).trim(),
        "termux-sync tui 2.0.0"
    );
}

#[test]
fn tui_help_prints_usage() {
    let fx = Fixture::new();
    let tui_help = fx.run(&["tui", "--help"], None);
    assert!(tui_help.status.success());
    assert!(String::from_utf8_lossy(&tui_help.stdout).contains("Usage: termux-sync tui"));
}

#[test]
fn tui_without_tty_exits_two() {
    let fx = Fixture::new();
    let tui_no_tty = fx.run(&["tui"], None);
    assert_eq!(tui_no_tty.status.code(), Some(2));
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
    // A valid directory and file, followed by a traversal member that is
    // refused during extraction (a warning) while the rest still extracts.
    fx.write_archive_backup(
        "termux_backup_partial",
        &[
            ("home/", 0, EntryType::Directory),
            ("home/ok.txt", 3, EntryType::Regular),
            ("../evil.txt", 1, EntryType::Regular),
        ],
    );
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
    // Valid directory and file plus a fifo entry, which extraction skips
    // with an ignorable warning (so restore still succeeds).
    fx.write_archive_backup(
        "termux_backup_fifo",
        &[
            ("home/", 0, EntryType::Directory),
            ("home/ok.txt", 3, EntryType::Regular),
            ("home/fifo", 0, EntryType::Fifo),
        ],
    );
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

#[test]
fn schedule_survives_broken_cron_file() {
    let fx = Fixture::new();
    fs::write(fx.home.join(".config/termux-sync/cron.json"), b"[]").unwrap();
    let out = fx.run(&["schedule"], Some("n\n"));
    assert_eq!(out.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
}

#[test]
fn non_utf8_target_is_rejected_without_panic() {
    use std::os::unix::ffi::OsStringExt;
    let fx = Fixture::new();
    let bad = OsString::from_vec(vec![0xff, 0xfe]);
    let out = fx.run_os(&[OsString::from("import-config"), bad], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).contains("valid UTF-8"));
}

#[test]
fn flag_before_tui_is_not_silent_success() {
    let fx = Fixture::new();
    let out = fx.run(&["--debug", "tui"], None);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn debug_flag_shows_trace_lines() {
    let fx = Fixture::new();
    fs::write(
        fx.home.join(".config/termux-sync/sync.log"),
        "[2026-01-01 00:00:00] [TRACE] detail\n",
    )
    .unwrap();
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

#[test]
fn import_config_missing_file_exits_non_zero() {
    let fx = Fixture::new();
    let out = fx.run(&["import-config", "/nope/missing.json"], None);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn restore_missing_backup_exits_non_zero() {
    let fx = Fixture::new();
    let out = fx.run(&["restore", "--name", "nope"], None);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn delete_missing_backup_exits_non_zero() {
    let fx = Fixture::new();
    let out = fx.run(&["delete", "-n", "missing", "--yes"], None);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Backup not found"), "{text}");
    assert!(!text.contains("FileNotFoundError"));
}

#[test]
fn backup_fails_when_verification_fails() {
    let fx = Fixture::new();
    let out = fx.run_env(
        &["--json-events", "backup"],
        None,
        &[("TERMUX_SYNC_TEST_VERIFY_FAIL", "1")],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"ok\":false"));
}
