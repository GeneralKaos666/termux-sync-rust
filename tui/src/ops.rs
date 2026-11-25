use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::data::{self, Backup};

const MAX_LINE: usize = 64 * 1024;
pub(crate) const LOG_LIMIT: usize = 200;
const STDERR_LIMIT: usize = 20;
const CANCEL_TIMEOUT: Duration = Duration::from_secs(10);

enum LineRead {
    Eof,
    Line,
    Oversize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Backup,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    AwaitingConfirm,
    Cancelling,
    Succeeded,
    Warnings,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageState {
    Pending,
    Active,
    Complete,
    Failed,
    Skipped,
}

#[derive(Debug, Clone)]
pub struct Stage {
    pub id: String,
    pub label: String,
    pub weight: f32,
    pub state: StageState,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub name: String,
    pub date: String,
    pub label: String,
    pub storage: String,
    pub backup_version: String,
    pub current_version: String,
    pub packages: usize,
    pub to_install: Option<usize>,
    pub already_installed: Option<usize>,
    pub device_only: Option<usize>,
    pub diff_available: bool,
    pub archives: Vec<String>,
    pub size: u64,
    pub checksums: String,
}

#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub ok: bool,
    pub name: String,
    pub size: u64,
    pub verified: Option<bool>,
    pub verify_details: String,
    pub packages: usize,
    pub packages_installed: usize,
    pub packages_failed: Vec<String>,
    pub archives: usize,
    pub archives_failed: Vec<String>,
    pub warnings: bool,
    pub restart_required: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct OpError {
    pub message: String,
    pub hint: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: String,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct PackageDiff {
    pub to_install: Vec<String>,
    pub already_installed: Vec<String>,
    pub device_only: Vec<String>,
    pub to_install_parts: (usize, usize),
    pub already_installed_parts: (usize, usize),
    pub device_only_parts: (usize, usize),
}

impl PackageDiff {
    pub fn total(&self) -> usize {
        self.to_install.len() + self.already_installed.len() + self.device_only.len()
    }

    pub fn complete(&self) -> bool {
        let complete = |(got, want): (usize, usize)| want == 0 || got >= want;
        complete(self.to_install_parts)
            && complete(self.already_installed_parts)
            && complete(self.device_only_parts)
    }

    pub fn section(&self, index: usize) -> &[String] {
        match index {
            0 => &self.to_install,
            1 => &self.already_installed,
            _ => &self.device_only,
        }
    }

    pub fn apply(&mut self, category: &str, part: usize, parts: usize, packages: Vec<String>) {
        match category {
            "to_install" => {
                self.to_install_parts = (self.to_install_parts.0 + 1, parts.max(part));
                self.to_install.extend(packages);
            }
            "already_installed" => {
                self.already_installed_parts =
                    (self.already_installed_parts.0 + 1, parts.max(part));
                self.already_installed.extend(packages);
            }
            "device_only" => {
                self.device_only_parts = (self.device_only_parts.0 + 1, parts.max(part));
                self.device_only.extend(packages);
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Start {
        name: String,
        storage: String,
    },
    Backup(Backup),
    Stage {
        id: String,
        label: String,
        state: StageState,
        index: usize,
        total: usize,
    },
    Progress {
        stage: String,
        current: u64,
        total: Option<u64>,
        unit: Option<String>,
        detail: Option<String>,
    },
    Log {
        level: String,
        message: String,
    },
    Verify {
        state: String,
        details: String,
    },
    Plan(Plan),
    PackageDiff {
        category: String,
        part: usize,
        parts: usize,
        packages: Vec<String>,
    },
    Result(Outcome),
    Error(OpError),
    Cancelled {
        reason: String,
    },
    Unsupported(u64),
    Unknown,
}

pub fn parse_event(line: &str) -> Result<Event, String> {
    let value: Value = serde_json::from_str(line).map_err(|error| error.to_string())?;
    if value
        .get("v")
        .and_then(Value::as_u64)
        .is_some_and(|version| version > 1)
    {
        return Ok(Event::Unsupported(
            value.get("v").and_then(Value::as_u64).unwrap_or(0),
        ));
    }
    let name = value
        .get("event")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing event field".to_string())?;
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(data::strip_controls)
            .unwrap_or_default()
    };
    let number = |key: &str| value.get(key).and_then(Value::as_u64);

    Ok(match name {
        "start" => Event::Start {
            name: text("name"),
            storage: text("storage"),
        },
        "backup" if data::is_safe_backup_name(&text("name")) => Event::Backup(Backup {
            name: text("name"),
            date: text("date").replace('T', " ").chars().take(16).collect(),
            label: text("label"),
            packages: number("packages").unwrap_or(0) as usize,
            size: number("size").unwrap_or(0),
            version: text("version"),
            compression: text("compression").to_uppercase(),
            modified: None,
        }),
        "backup" => Event::Unknown,
        "stage" => Event::Stage {
            id: text("id"),
            label: text("label"),
            state: match value.get("state").and_then(Value::as_str).unwrap_or("") {
                "start" => StageState::Active,
                "complete" => StageState::Complete,
                "failed" => StageState::Failed,
                "skipped" => StageState::Skipped,
                _ => StageState::Pending,
            },
            index: number("index").unwrap_or(0) as usize,
            total: number("total").unwrap_or(0) as usize,
        },
        "progress" => Event::Progress {
            stage: text("stage"),
            current: number("current").unwrap_or(0),
            total: number("total"),
            unit: value
                .get("unit")
                .and_then(Value::as_str)
                .map(str::to_string),
            detail: value
                .get("detail")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        "log" => Event::Log {
            level: text("level"),
            message: text("message"),
        },
        "verify" => Event::Verify {
            state: text("state"),
            details: text("details"),
        },
        "plan" => Event::Plan(Plan {
            name: text("name"),
            date: text("date"),
            label: text("label"),
            storage: text("storage"),
            backup_version: data::strip_controls(
                value
                    .get("versions")
                    .and_then(|versions| versions.get("backup"))
                    .and_then(Value::as_str)
                    .unwrap_or("—"),
            ),
            current_version: data::strip_controls(
                value
                    .get("versions")
                    .and_then(|versions| versions.get("current"))
                    .and_then(Value::as_str)
                    .unwrap_or("—"),
            ),
            packages: number("packages").unwrap_or(0) as usize,
            to_install: number("to_install").map(|n| n as usize),
            already_installed: number("already_installed").map(|n| n as usize),
            device_only: number("device_only").map(|n| n as usize),
            diff_available: value
                .get("diff_available")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            archives: value
                .get("archives")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            size: number("size").unwrap_or(0),
            checksums: text("checksums"),
        }),
        "package_diff" => Event::PackageDiff {
            category: text("category"),
            part: number("part").unwrap_or(1) as usize,
            parts: number("parts").unwrap_or(1) as usize,
            packages: value
                .get("packages")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        },
        "result" => Event::Result(Outcome {
            ok: value.get("ok").and_then(Value::as_bool).unwrap_or(false),
            name: text("name"),
            size: number("size").unwrap_or(0),
            verified: value.get("verified").and_then(Value::as_bool),
            verify_details: text("verify_details"),
            packages: number("packages").unwrap_or(0) as usize,
            packages_installed: number("packages_installed").unwrap_or(0) as usize,
            packages_failed: string_list(&value, "packages_failed"),
            archives: number("archives").unwrap_or(0) as usize,
            archives_failed: string_list(&value, "archives_failed"),
            warnings: value
                .get("warnings")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            restart_required: value
                .get("restart_required")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            duration_ms: number("duration_ms").unwrap_or(0),
        }),
        "error" => Event::Error(OpError {
            message: text("message"),
            hint: text("hint"),
            exit_code: number("exit_code").map(|n| n as i32),
        }),
        "cancelled" => Event::Cancelled {
            reason: text("reason"),
        },
        _ => Event::Unknown,
    })
}

fn string_list(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

enum Msg {
    Event(Box<Event>),
    Malformed(String),
    Stderr(String),
    Eof,
}

pub fn collect_backups() -> Result<Vec<Backup>, String> {
    collect_backups_with(&data::cli_bin())
}

pub fn collect_backups_with(bin: &std::path::Path) -> Result<Vec<Backup>, String> {
    let output = std::process::Command::new(bin)
        .args(["list", "--json-events"])
        .output()
        .map_err(|error| format!("cannot start termux-sync: {error}"))?;
    if !output.status.success() {
        return Err(format!("termux-sync list exited with {}", output.status));
    }
    let mut backups = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Ok(Event::Backup(backup)) = parse_event(line) {
            backups.push(backup);
        }
    }
    Ok(backups)
}

pub fn supports_flag(flag: &str) -> bool {
    let mut command = data::cli_command(&["logs".to_string(), flag.to_string()]);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(test)]
fn supports_flag_with(bin: &std::path::Path, flag: &str) -> bool {
    std::process::Command::new(bin)
        .args(["logs", flag])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn read_event_line<R: BufRead>(
    reader: &mut R,
    limit: usize,
    out: &mut String,
) -> io::Result<LineRead> {
    out.clear();
    let read = {
        let mut limited = std::io::Read::take(&mut *reader, limit as u64 + 1);
        limited.read_line(out)?
    };
    if read == 0 {
        return Ok(LineRead::Eof);
    }
    if out.len() > limit {
        let mut sink = String::new();
        reader.read_line(&mut sink)?;
        return Ok(LineRead::Oversize);
    }
    Ok(LineRead::Line)
}

pub fn supports_events() -> bool {
    supports_flag("--json-events")
}

pub fn supports_yes() -> bool {
    supports_flag("--yes")
}

pub struct Operation {
    pub kind: Kind,
    pub name: String,
    pub storage: String,
    pub status: Status,
    pub stages: Vec<Stage>,
    pub current_stage: Option<String>,
    pub stage_current: u64,
    pub stage_total: Option<u64>,
    pub stage_unit: Option<String>,
    pub stage_detail: Option<String>,
    pub verify: Option<(String, String)>,
    pub plan: Option<Plan>,
    pub diff: PackageDiff,
    pub logs: VecDeque<LogEntry>,
    pub result: Option<Outcome>,
    pub error: Option<OpError>,
    pub notice: String,
    pub confirm_text: String,
    pub diff_open: bool,
    pub diff_section: usize,
    pub diff_filter: String,
    pub diff_filtering: bool,
    pub diff_scroll: usize,
    pub log_offset: usize,
    pub protected: bool,
    pub cancel_prompt: bool,
    pub reduce_motion: bool,
    pub started: Instant,
    args: Vec<String>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    rx: Option<Receiver<Msg>>,
    stderr_tail: VecDeque<String>,
    cancel_sent_at: Option<Instant>,
    exited_at: Option<Instant>,
    exit_status: Option<ExitStatus>,
    eof: bool,
}

impl Operation {
    pub fn start(
        kind: Kind,
        name: &str,
        args: Vec<String>,
        reduce_motion: bool,
    ) -> std::io::Result<Self> {
        let mut command = data::cli_command(&args);
        command.arg("--json-events");
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }

        let mut child = command.spawn()?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdin = child.stdin.take();
        let (tx, rx) = mpsc::channel();

        if let Some(stdout) = stdout {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut line = String::new();
                loop {
                    match read_event_line(&mut reader, MAX_LINE, &mut line) {
                        Ok(LineRead::Eof) => break,
                        Ok(LineRead::Oversize) => {
                            let _ = tx.send(Msg::Malformed("event line exceeded 64 KiB".into()));
                        }
                        Ok(LineRead::Line) => {
                            let message = match parse_event(line.trim_end()) {
                                Ok(event) => Msg::Event(Box::new(event)),
                                Err(error) => Msg::Malformed(error),
                            };
                            let _ = tx.send(message);
                        }
                        Err(error) => {
                            let _ = tx.send(Msg::Malformed(error.to_string()));
                            break;
                        }
                    }
                }
                let _ = tx.send(Msg::Eof);
            });
        }

        if let Some(stderr) = stderr {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    let _ = tx.send(Msg::Stderr(line));
                }
            });
        }

        let stages = stage_defs(kind)
            .into_iter()
            .map(|(id, label, weight)| Stage {
                id: id.to_string(),
                label: label.to_string(),
                weight,
                state: StageState::Pending,
            })
            .collect();

        Ok(Self {
            kind,
            name: name.to_string(),
            storage: String::new(),
            status: Status::Running,
            stages,
            current_stage: None,
            stage_current: 0,
            stage_total: None,
            stage_unit: None,
            stage_detail: None,
            verify: None,
            plan: None,
            diff: PackageDiff::default(),
            logs: VecDeque::new(),
            result: None,
            error: None,
            notice: String::new(),
            confirm_text: String::new(),
            diff_open: false,
            diff_section: 0,
            diff_filter: String::new(),
            diff_filtering: false,
            diff_scroll: 0,
            log_offset: 0,
            protected: false,
            cancel_prompt: false,
            reduce_motion,
            started: Instant::now(),
            args,
            child: Some(child),
            stdin,
            rx: Some(rx),
            stderr_tail: VecDeque::new(),
            cancel_sent_at: None,
            exited_at: None,
            exit_status: None,
            eof: false,
        })
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        let mut messages = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(message) => messages.push(message),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        for message in messages {
            match message {
                Msg::Event(event) => {
                    self.apply(*event);
                    changed = true;
                }
                Msg::Malformed(error) => {
                    self.push_log("WARNING", format!("event parse error: {error}"));
                    changed = true;
                }
                Msg::Stderr(line) => {
                    if !line.trim().is_empty() {
                        self.stderr_tail.push_back(line);
                        while self.stderr_tail.len() > STDERR_LIMIT {
                            self.stderr_tail.pop_front();
                        }
                        changed = true;
                    }
                }
                Msg::Eof => {
                    self.eof = true;
                }
            }
        }
        if disconnected {
            self.eof = true;
        }

        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.exit_status = Some(status);
                    if self.exited_at.is_none() {
                        self.exited_at = Some(Instant::now());
                    }
                    changed = true;
                }
                Ok(None) => {}
                Err(error) => {
                    self.error = Some(OpError {
                        message: format!("could not wait for termux-sync: {error}"),
                        hint: String::new(),
                        exit_code: None,
                    });
                    self.status = Status::Failed;
                    changed = true;
                }
            }
        }

        if self.status == Status::Cancelling
            && self
                .cancel_sent_at
                .is_some_and(|sent| sent.elapsed() > CANCEL_TIMEOUT)
        {
            self.kill_forcefully();
            self.cancel_sent_at = None;
            self.notice = "process did not stop; killed. Cleanup may be incomplete.".into();
        }

        if self.exited_at.is_some()
            && (self.eof
                || self
                    .exited_at
                    .is_some_and(|at| at.elapsed() > Duration::from_millis(250)))
            && !self.is_done()
        {
            self.finalize_exit();
            changed = true;
        }

        changed
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Start { name, storage } => {
                if !name.is_empty() {
                    self.name = name;
                }
                self.storage = storage;
            }
            Event::Stage {
                id,
                label,
                state,
                index,
                total,
            } => {
                if !id.is_empty() {
                    if (id.starts_with("extract:") || id == "packages" || id == "cleanup")
                        && state == StageState::Active
                        && self.kind == Kind::Restore
                    {
                        self.protected = true;
                    }
                    if !self.stages.iter().any(|stage| stage.id == id) {
                        self.stages.push(Stage {
                            id: id.clone(),
                            label: label.clone(),
                            weight: 0.0,
                            state: StageState::Pending,
                        });
                    }
                    if let Some(stage) = self.stages.iter_mut().find(|stage| stage.id == id) {
                        stage.state = state;
                        if !label.is_empty() {
                            stage.label = label.clone();
                        }
                    }
                    let definition = stage_defs(self.kind);
                    let stage_label = definition
                        .iter()
                        .find(|(stage_id, _, _)| *stage_id == id)
                        .map(|(_, label, _)| *label)
                        .unwrap_or(label.as_str())
                        .to_string();
                    self.current_stage = Some(id);
                    self.stage_current = 0;
                    self.stage_total = None;
                    self.stage_unit = None;
                    self.stage_detail = None;
                    self.push_log(
                        "DEBUG",
                        format!(
                            "stage {}/{} {}: {}",
                            index,
                            total,
                            stage_label,
                            state_label(state)
                        ),
                    );
                }
            }
            Event::Progress {
                stage,
                current,
                total,
                unit,
                detail,
            } => {
                self.current_stage = Some(stage);
                self.stage_current = current;
                self.stage_total = total;
                self.stage_unit = unit;
                self.stage_detail = detail;
            }
            Event::Log { level, message } => self.push_log(&level, message),
            Event::Verify { state, details } => self.verify = Some((state, details)),
            Event::Plan(plan) => {
                self.name = plan.name.clone();
                self.storage = plan.storage.clone();
                self.plan = Some(plan);
                self.status = Status::AwaitingConfirm;
            }
            Event::PackageDiff {
                category,
                part,
                parts,
                packages,
            } => self.diff.apply(&category, part, parts, packages),
            Event::Result(outcome) => {
                self.name = if outcome.name.is_empty() {
                    self.name.clone()
                } else {
                    outcome.name.clone()
                };
                self.status = if !outcome.ok {
                    Status::Failed
                } else if outcome.warnings {
                    Status::Warnings
                } else {
                    Status::Succeeded
                };
                if !outcome.ok {
                    self.error = Some(OpError {
                        message: "termux-sync reported failure".to_string(),
                        hint: outcome.verify_details.clone(),
                        exit_code: None,
                    });
                }
                self.result = Some(outcome);
                self.protected = false;
                self.cancel_prompt = false;
            }
            Event::Error(error) => {
                self.status = Status::Failed;
                self.error = Some(error);
            }
            Event::Cancelled { reason } => {
                self.status = Status::Cancelled;
                if !reason.is_empty() {
                    self.notice = format!("cancelled: {reason}");
                }
            }
            Event::Unsupported(version) => {
                self.status = Status::Failed;
                self.error = Some(OpError {
                    message: format!("event protocol v{version} is newer than this TUI supports"),
                    hint: "Update termux-sync-tui.".to_string(),
                    exit_code: None,
                });
            }
            Event::Backup(_) => {}
            Event::Unknown => {}
        }
    }

    fn finalize_exit(&mut self) {
        let code = self.exit_status.and_then(|status| status.code());
        if self.status == Status::Cancelling {
            self.status = Status::Cancelled;
            return;
        }
        if (self.status == Status::Cancelled || code == Some(130)) && !self.protected {
            self.status = Status::Cancelled;
            return;
        }
        let tail = self
            .stderr_tail
            .iter()
            .rev()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        let message = if self.protected {
            "Restore stopped during a protected phase; partial changes may exist.".to_string()
        } else {
            "termux-sync exited without a result".to_string()
        };
        self.error = Some(OpError {
            message,
            hint: if tail.is_empty() {
                "Check the log and try again.".to_string()
            } else {
                tail
            },
            exit_code: code,
        });
        self.status = Status::Failed;
    }

    pub fn request_cancel(&mut self) -> bool {
        if self.status == Status::AwaitingConfirm {
            let sent = self.send_stdin("{\"confirm\":\"CANCEL\"}");
            if sent {
                self.cancel_sent_at = Some(Instant::now());
            }
            self.status = Status::Cancelling;
            return true;
        }
        if !self.cancel_allowed() {
            return false;
        }
        self.signal_group(libc::SIGINT);
        self.cancel_sent_at = Some(Instant::now());
        self.status = Status::Cancelling;
        true
    }

    pub fn confirm_restore(&mut self) -> bool {
        if self.status != Status::AwaitingConfirm {
            return false;
        }
        if self
            .plan
            .as_ref()
            .is_some_and(|plan| plan.checksums == "failed")
        {
            self.notice = "checksums failed; restore blocked".to_string();
            return false;
        }
        if !self.send_stdin("{\"confirm\":\"RESTORE\"}") {
            self.notice = "cannot write confirmation to termux-sync".to_string();
            return false;
        }
        self.status = Status::Running;
        true
    }

    fn send_stdin(&mut self, line: &str) -> bool {
        let Some(stdin) = &mut self.stdin else {
            return false;
        };
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .is_ok()
    }

    fn signal_group(&self, signal: i32) {
        if let Some(child) = &self.child {
            let pid = child.id() as i32;
            unsafe {
                libc::kill(-pid, signal);
            }
        }
    }

    fn kill_forcefully(&mut self) {
        self.signal_group(libc::SIGKILL);
    }

    pub fn cancel_allowed(&self) -> bool {
        match self.status {
            Status::AwaitingConfirm => true,
            Status::Running => self.kind == Kind::Backup || !self.protected,
            _ => false,
        }
    }

    pub fn is_done(&self) -> bool {
        matches!(
            self.status,
            Status::Succeeded | Status::Warnings | Status::Failed | Status::Cancelled
        )
    }

    pub fn is_running(&self) -> bool {
        matches!(
            self.status,
            Status::Running | Status::AwaitingConfirm | Status::Cancelling
        )
    }

    pub fn overall(&self) -> f32 {
        let total: f32 = self
            .stages
            .iter()
            .filter(|stage| stage.state != StageState::Skipped)
            .map(|stage| stage.weight)
            .sum();
        if total <= 0.0 {
            return 0.0;
        }
        let mut done = 0.0;
        for stage in &self.stages {
            match stage.state {
                StageState::Complete => done += stage.weight,
                StageState::Active => {
                    let fraction = match self.stage_total {
                        Some(total) if total > 0 => {
                            (self.stage_current as f32 / total as f32).clamp(0.0, 1.0)
                        }
                        _ => 0.0,
                    };
                    done += stage.weight * fraction;
                }
                _ => {}
            }
        }
        (done / total).clamp(0.0, 1.0)
    }

    pub fn progress_text(&self) -> String {
        let unit = self.stage_unit.as_deref().unwrap_or("");
        match (self.stage_current, self.stage_total) {
            (current, Some(total)) if total > 0 => {
                format!("{} / {} {}", human_count(current), human_count(total), unit)
            }
            (current, _) if current > 0 => format!("{} {}", human_count(current), unit),
            _ => "working".to_string(),
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn retry_args(&self) -> Vec<String> {
        self.args.clone()
    }

    pub fn push_log(&mut self, level: &str, message: impl Into<String>) {
        self.logs.push_back(LogEntry {
            level: level.to_string(),
            message: message.into(),
        });
        while self.logs.len() > LOG_LIMIT {
            self.logs.pop_front();
        }
    }

    pub fn filtered_diff(&self) -> Vec<String> {
        let filter = self.diff_filter.to_lowercase();
        self.diff
            .section(self.diff_section)
            .iter()
            .filter(|package| filter.is_empty() || package.to_lowercase().contains(&filter))
            .cloned()
            .collect()
    }
}

fn stage_defs(kind: Kind) -> Vec<(&'static str, &'static str, f32)> {
    match kind {
        Kind::Backup => vec![
            ("preflight", "Preflight", 2.0),
            ("packages", "Packages", 5.0),
            ("archive:home", "Home", 50.0),
            ("archive:usr_etc", "Config", 13.0),
            ("manifest", "Manifest", 5.0),
            ("upload", "Upload", 20.0),
            ("verify", "Verify", 5.0),
        ],
        Kind::Restore => vec![
            ("metadata", "Metadata", 2.0),
            ("download", "Download", 25.0),
            ("verify", "Verify", 15.0),
            ("extract:home", "Home", 20.0),
            ("extract:usr_etc", "Config", 5.0),
            ("packages", "Packages", 28.0),
            ("cleanup", "Cleanup", 5.0),
        ],
    }
}

fn state_label(state: StageState) -> &'static str {
    match state {
        StageState::Pending => "pending",
        StageState::Active => "start",
        StageState::Complete => "complete",
        StageState::Failed => "failed",
        StageState::Skipped => "skipped",
    }
}

fn human_count(value: u64) -> String {
    if value >= 1_000_000_000 {
        format!("{:.1}G", value as f64 / 1_000_000_000.0)
    } else if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
pub fn test_operation(kind: Kind) -> Operation {
    let stages = stage_defs(kind)
        .into_iter()
        .map(|(id, label, weight)| Stage {
            id: id.to_string(),
            label: label.to_string(),
            weight,
            state: StageState::Pending,
        })
        .collect();
    Operation {
        kind,
        name: "termux_backup_test".to_string(),
        storage: "local".to_string(),
        status: Status::Running,
        stages,
        current_stage: None,
        stage_current: 0,
        stage_total: None,
        stage_unit: None,
        stage_detail: None,
        verify: None,
        plan: None,
        diff: PackageDiff::default(),
        logs: VecDeque::new(),
        result: None,
        error: None,
        notice: String::new(),
        confirm_text: String::new(),
        diff_open: false,
        diff_section: 0,
        diff_filter: String::new(),
        diff_filtering: false,
        diff_scroll: 0,
        log_offset: 0,
        protected: false,
        cancel_prompt: false,
        reduce_motion: false,
        started: Instant::now(),
        args: Vec::new(),
        child: None,
        stdin: None,
        rx: None,
        stderr_tail: VecDeque::new(),
        cancel_sent_at: None,
        exited_at: None,
        exit_status: None,
        eof: false,
    }
}

#[cfg(test)]
pub fn test_plan() -> Plan {
    Plan {
        name: "termux_backup_test".to_string(),
        date: "2026-11-21T01:17:33".to_string(),
        label: "before-upgrade".to_string(),
        storage: "local".to_string(),
        backup_version: "1.3.9".to_string(),
        current_version: "1.3.9".to_string(),
        packages: 1947,
        to_install: Some(120),
        already_installed: Some(1827),
        device_only: Some(210),
        diff_available: true,
        archives: vec!["home.tar.gz".to_string(), "usr_etc.tar.gz".to_string()],
        size: 22_300_000_000,
        checksums: "pending".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plan_and_diff_and_result() {
        let plan = parse_event(
            r#"{"v":1,"event":"plan","name":"b1","packages":10,"diff_available":true,"to_install":2,"already_installed":6,"device_only":3,"archives":["home.tar.gz"],"size":123,"checksums":"pending","storage":"local","versions":{"backup":"1.3.9","current":"1.3.9"}}"#,
        )
        .unwrap();
        match plan {
            Event::Plan(plan) => {
                assert_eq!(plan.packages, 10);
                assert_eq!(plan.to_install, Some(2));
                assert_eq!(plan.already_installed, Some(6));
                assert_eq!(plan.device_only, Some(3));
                assert_eq!(plan.checksums, "pending");
            }
            other => panic!("unexpected {other:?}"),
        }

        let diff = parse_event(
            r#"{"event":"package_diff","category":"to_install","part":1,"parts":2,"packages":["a","b"]}"#,
        )
        .unwrap();
        assert!(
            matches!(diff, Event::PackageDiff { ref category, .. } if category == "to_install")
        );

        let result = parse_event(
            r#"{"event":"result","ok":true,"name":"b1","size":4,"verified":null,"warnings":true,"packages_failed":["x"],"archives_failed":[],"restart_required":true}"#,
        )
        .unwrap();
        match result {
            Event::Result(result) => {
                assert!(result.ok);
                assert!(result.warnings);
                assert_eq!(result.verified, None);
                assert_eq!(result.packages_failed, vec!["x"]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn newer_protocol_version_is_reported() {
        let event = parse_event(r#"{"v":2,"event":"log","level":"INFO","message":"x"}"#).unwrap();
        assert!(matches!(event, Event::Unsupported(2)));
    }

    #[test]
    fn malformed_event_is_an_error() {
        assert!(parse_event("not json").is_err());
        assert!(parse_event(r#"{"v":1}"#).is_err());
    }

    #[test]
    fn diff_chunks_reassemble_and_detect_partial() {
        let mut diff = PackageDiff::default();
        diff.apply("to_install", 1, 2, vec!["a".into()]);
        assert!(!diff.complete());
        diff.apply("to_install", 2, 2, vec!["b".into()]);
        assert!(diff.complete());
        assert_eq!(diff.total(), 2);
        assert_eq!(diff.section(0), &["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn stage_weights_sum_to_one_hundred() {
        for kind in [Kind::Backup, Kind::Restore] {
            let total: f32 = stage_defs(kind).iter().map(|(_, _, weight)| weight).sum();
            assert_eq!(total, 100.0);
        }
    }
}

#[cfg(test)]
mod list_event_tests {
    use super::*;

    #[test]
    fn parses_backup_list_event() {
        let event = parse_event(r#"{"v":1,"event":"backup","name":"b1","date":"2026-01-01T00:00:00","label":"x","packages":3,"size":10,"version":"1.3.9","compression":"gz","storage":"github"}"#).unwrap();
        assert!(
            matches!(event, Event::Backup(ref backup) if backup.name == "b1" && backup.packages == 3)
        );
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_list_fixture() {
        let text = include_str!("../tests/fixtures/list.jsonl");
        let mut backups = Vec::new();
        let mut result = false;
        for line in text.lines() {
            match parse_event(line).unwrap() {
                Event::Backup(backup) => backups.push(backup),
                Event::Result(_) => result = true,
                _ => {}
            }
        }
        assert_eq!(backups.len(), 2);
        assert_eq!(backups[0].name, "b1");
        assert!(result);
    }

    #[test]
    fn truncated_fixture_does_not_panic() {
        let text = include_str!("../tests/fixtures/malformed.jsonl");
        let parsed: Vec<_> = text.lines().map(parse_event).collect();
        assert!(parsed[0].is_err());
        assert!(parsed[1].is_ok());
    }

    #[test]
    fn flag_probe_rejects_version_only_cli() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("fake");
        fs::write(
            &bin,
            "#!/bin/sh\n[ \"$1\" = logs ] && [ \"$2\" = --json-events ] && exit 0\nexit 2\n",
        )
        .unwrap();
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
    fn backup_list_uses_the_configured_cli() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("fake");
        fs::write(
            &bin,
            "#!/bin/sh\nprintf '%s\\n' '{\"event\":\"backup\",\"name\":\"termux_backup_a\",\"date\":\"2026-01-01\",\"size\":1}'\n",
        )
        .unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let list = collect_backups_with(&bin).unwrap();
        assert_eq!(list[0].name, "termux_backup_a");
    }

    #[test]
    fn oversized_line_is_drained() {
        let mut input = vec![b'x'; 32];
        input.push(b'\n');
        input.extend_from_slice(b"next\n");
        let mut reader = BufReader::new(&input[..]);
        let mut line = String::new();
        assert!(matches!(
            read_event_line(&mut reader, 8, &mut line).unwrap(),
            LineRead::Oversize
        ));
        assert!(matches!(
            read_event_line(&mut reader, 8, &mut line).unwrap(),
            LineRead::Line
        ));
        assert_eq!(line, "next\n");
    }
}
