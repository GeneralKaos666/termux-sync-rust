use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
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

/// Operation the TUI can run with the event protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Create and upload a backup.
    Backup,
    /// Download and apply a backup.
    Restore,
}

/// Lifecycle state of an [`Operation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Work is in progress.
    Running,
    /// Waiting for the user to type the restore confirmation.
    AwaitingConfirm,
    /// Cancellation was requested and the process is stopping.
    Cancelling,
    /// Finished without warnings.
    Succeeded,
    /// Finished with warnings or partial failures.
    Warnings,
    /// Finished with an error.
    Failed,
    /// Stopped by user request.
    Cancelled,
}

/// Progress of one stage in an operation plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageState {
    /// Not started.
    Pending,
    /// Currently running.
    Active,
    /// Finished successfully.
    Complete,
    /// Finished with an error.
    Failed,
    /// Not run for this operation.
    Skipped,
}

/// One weighted step of an operation's progress bar.
#[derive(Debug, Clone)]
pub struct Stage {
    /// Stable stage identifier used to match progress events.
    pub id: String,
    /// Human-readable stage name.
    pub label: String,
    /// Relative share of the overall progress bar.
    pub weight: f32,
    /// Current state of the stage.
    pub state: StageState,
}

/// Restore plan emitted before any files change.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Backup name.
    pub name: String,
    /// Backup date as shown to the user.
    pub date: String,
    /// Backup label, possibly empty.
    pub label: String,
    /// Storage backend.
    pub storage: String,
    /// CLI version that created the backup.
    pub backup_version: String,
    /// CLI version running the restore.
    pub current_version: String,
    /// Packages recorded in the backup.
    pub packages: usize,
    /// Packages that would be installed, when known.
    pub to_install: Option<usize>,
    /// Packages already at the backup version.
    pub already_installed: Option<usize>,
    /// Packages present only on this device.
    pub device_only: Option<usize>,
    /// Whether the package diff is available.
    pub diff_available: bool,
    /// Archive names in the backup.
    pub archives: Vec<String>,
    /// Total download or restore size in bytes.
    pub size: u64,
    /// Checksum state; the value `failed` blocks confirmation.
    pub checksums: String,
}

/// Final result reported by the CLI.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// True when the operation reported success.
    pub ok: bool,
    /// Backup name, which may have been generated.
    pub name: String,
    /// Bytes processed.
    pub size: u64,
    /// Verification result, `None` when verification did not run.
    pub verified: Option<bool>,
    /// Human-readable verification details.
    pub verify_details: String,
    /// Packages considered.
    pub packages: usize,
    /// Packages installed successfully.
    pub packages_installed: usize,
    /// Packages that failed to install.
    pub packages_failed: Vec<String>,
    /// Archives extracted.
    pub archives: usize,
    /// Archives that failed to extract.
    pub archives_failed: Vec<String>,
    /// Whether warnings were reported.
    pub warnings: bool,
    /// Whether Termux must be restarted.
    pub restart_required: bool,
    /// Reported duration in milliseconds.
    pub duration_ms: u64,
}

/// Error reported by the CLI, with an actionable hint.
#[derive(Debug, Clone)]
pub struct OpError {
    /// What went wrong.
    pub message: String,
    /// Suggested next step.
    pub hint: String,
    /// Process exit code, when the CLI supplied one.
    pub exit_code: Option<i32>,
}

/// One line in an operation's log pane.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Log level such as `INFO` or `WARNING`.
    pub level: String,
    /// Log message.
    pub message: String,
}

/// Package diff streamed in parts before a restore.
#[derive(Debug, Clone, Default)]
pub struct PackageDiff {
    /// Packages to install.
    pub to_install: Vec<String>,
    /// Packages already at the wanted version.
    pub already_installed: Vec<String>,
    /// Packages only on this device.
    pub device_only: Vec<String>,
    /// Received and expected part counts for `to_install`.
    pub to_install_parts: (usize, usize),
    /// Received and expected part counts for `already_installed`.
    pub already_installed_parts: (usize, usize),
    /// Received and expected part counts for `device_only`.
    pub device_only_parts: (usize, usize),
}

impl PackageDiff {
    /// Total number of packages across all sections.
    #[must_use]
    pub fn total(&self) -> usize {
        self.to_install.len() + self.already_installed.len() + self.device_only.len()
    }

    /// Whether every section has received all expected parts.
    #[must_use]
    pub fn complete(&self) -> bool {
        let complete = |(got, want): (usize, usize)| want == 0 || got >= want;
        complete(self.to_install_parts)
            && complete(self.already_installed_parts)
            && complete(self.device_only_parts)
    }

    /// Section by index: 0 to install, 1 installed, 2 device-only.
    ///
    /// Indices above 2 fall back to device-only.
    #[must_use]
    pub fn section(&self, index: usize) -> &[String] {
        match index {
            0 => &self.to_install,
            1 => &self.already_installed,
            _ => &self.device_only,
        }
    }

    /// Merges one streamed part into its category, ignoring unknown categories.
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

/// One JSON event from the CLI's `--json-events` stream.
#[derive(Debug, Clone)]
pub enum Event {
    /// Operation started.
    Start {
        /// Backup name.
        name: String,
        /// Storage backend.
        storage: String,
    },
    /// Backup metadata, used by `list`.
    Backup(Backup),
    /// Stage list update.
    Stage {
        /// Stage identifier.
        id: String,
        /// Stage label.
        label: String,
        /// New state for the stage.
        state: StageState,
        /// One-based index in the stage list.
        index: usize,
        /// Total number of stages.
        total: usize,
    },
    /// Progress within the active stage.
    Progress {
        /// Active stage id.
        stage: String,
        /// Units completed so far.
        current: u64,
        /// Total units, when known.
        total: Option<u64>,
        /// Unit name, for example `files`.
        unit: Option<String>,
        /// Extra detail for the current step.
        detail: Option<String>,
    },
    /// A log line emitted during the operation.
    Log {
        /// Log level.
        level: String,
        /// Log message.
        message: String,
    },
    /// Verification state change.
    Verify {
        /// Verification state such as `ok` or `failed`.
        state: String,
        /// Human-readable verification details.
        details: String,
    },
    /// Restore plan.
    Plan(Plan),
    /// One streamed part of the package diff.
    PackageDiff {
        /// Diff category: `to_install`, `already_installed`, or `device_only`.
        category: String,
        /// One-based part number.
        part: usize,
        /// Total number of parts in this category.
        parts: usize,
        /// Packages in this part.
        packages: Vec<String>,
    },
    /// Final result.
    Result(Outcome),
    /// Final error.
    Error(OpError),
    /// Operation was cancelled.
    Cancelled {
        /// Why the operation was cancelled.
        reason: String,
    },
    /// The CLI does not support the requested protocol version.
    Unsupported(u64),
    /// Unrecognized event kind.
    Unknown,
}

/// Parses one JSON event line.
///
/// Unknown event kinds become [`Event::Unknown`]; malformed input returns the serde error.
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

    Ok(match name {
        "start" => parse_start(&value),
        "backup" => parse_backup(&value),
        "stage" => parse_stage(&value),
        "progress" => parse_progress(&value),
        "log" => parse_log(&value),
        "verify" => parse_verify(&value),
        "plan" => parse_plan(&value),
        "package_diff" => parse_package_diff(&value),
        "result" => parse_result(&value),
        "error" => parse_error(&value),
        "cancelled" => parse_cancelled(&value),
        _ => Event::Unknown,
    })
}

/// Wire payloads. Every field is optional and tolerant: a missing or wrongly
/// typed value becomes `None`, which is what the `Value::as_*` lookups these
/// structs replace did.
#[derive(Debug, Default, Deserialize)]
struct StartPayload {
    #[serde(default, deserialize_with = "flex")]
    name: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    storage: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BackupPayload {
    #[serde(default, deserialize_with = "flex")]
    name: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    date: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    label: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    packages: Option<u64>,
    #[serde(default, deserialize_with = "flex")]
    size: Option<u64>,
    #[serde(default, deserialize_with = "flex")]
    version: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    compression: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct StagePayload {
    #[serde(default, deserialize_with = "flex")]
    id: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    label: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    state: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    index: Option<u64>,
    #[serde(default, deserialize_with = "flex")]
    total: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct ProgressPayload {
    #[serde(default, deserialize_with = "flex")]
    stage: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    current: Option<u64>,
    #[serde(default, deserialize_with = "flex")]
    total: Option<u64>,
    #[serde(default, deserialize_with = "flex")]
    unit: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    detail: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct LogPayload {
    #[serde(default, deserialize_with = "flex")]
    level: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    message: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct VerifyPayload {
    #[serde(default, deserialize_with = "flex")]
    state: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    details: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PlanVersionsPayload {
    #[serde(default, deserialize_with = "flex")]
    backup: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    current: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PlanPayload {
    #[serde(default, deserialize_with = "flex")]
    name: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    date: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    label: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    storage: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    versions: Option<PlanVersionsPayload>,
    #[serde(default, deserialize_with = "flex")]
    packages: Option<u64>,
    #[serde(default, deserialize_with = "flex")]
    to_install: Option<u64>,
    #[serde(default, deserialize_with = "flex")]
    already_installed: Option<u64>,
    #[serde(default, deserialize_with = "flex")]
    device_only: Option<u64>,
    #[serde(default, deserialize_with = "flex")]
    diff_available: Option<bool>,
    #[serde(default, deserialize_with = "flex_list")]
    archives: Option<Vec<String>>,
    #[serde(default, deserialize_with = "flex")]
    size: Option<u64>,
    #[serde(default, deserialize_with = "flex")]
    checksums: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PackageDiffPayload {
    #[serde(default, deserialize_with = "flex")]
    category: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    part: Option<u64>,
    #[serde(default, deserialize_with = "flex")]
    parts: Option<u64>,
    #[serde(default, deserialize_with = "flex_list")]
    packages: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct ResultPayload {
    #[serde(default, deserialize_with = "flex")]
    ok: Option<bool>,
    #[serde(default, deserialize_with = "flex")]
    name: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    size: Option<u64>,
    #[serde(default, deserialize_with = "flex")]
    verified: Option<bool>,
    #[serde(default, deserialize_with = "flex")]
    verify_details: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    packages: Option<u64>,
    #[serde(default, deserialize_with = "flex")]
    packages_installed: Option<u64>,
    #[serde(default, deserialize_with = "flex_list")]
    packages_failed: Option<Vec<String>>,
    #[serde(default, deserialize_with = "flex")]
    archives: Option<u64>,
    #[serde(default, deserialize_with = "flex_list")]
    archives_failed: Option<Vec<String>>,
    #[serde(default, deserialize_with = "flex")]
    warnings: Option<bool>,
    #[serde(default, deserialize_with = "flex")]
    restart_required: Option<bool>,
    #[serde(default, deserialize_with = "flex")]
    duration_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct ErrorPayload {
    #[serde(default, deserialize_with = "flex")]
    message: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    hint: Option<String>,
    #[serde(default, deserialize_with = "flex")]
    exit_code: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct CancelledPayload {
    #[serde(default, deserialize_with = "flex")]
    reason: Option<String>,
}

/// Tolerant `Option<T>` field deserializer: a missing or wrongly typed value
/// becomes `None`. Swallowing the error keeps `serde_json`'s map access in a
/// usable state, because each entry is deserialized from its own value.
fn flex<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer).ok().flatten())
}

/// Arrays of strings with non-string entries dropped, matching the old
/// `filter_map(Value::as_str)` walk.
fn flex_list<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<Vec<Value>>::deserialize(deserializer) {
        Ok(Some(items)) => Ok(Some(
            items
                .into_iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect(),
        )),
        _ => Ok(None),
    }
}

/// Decode one payload, falling back to defaults when a field type is hostile.
fn payload<T>(value: &Value) -> T
where
    T: Default + serde::de::DeserializeOwned,
{
    T::deserialize(value).unwrap_or_default()
}

/// Text from a wire field, with control characters stripped.
fn text(value: Option<String>) -> String {
    value.map_or_default(|value| data::strip_controls(&value).into_owned())
}

fn parse_start(value: &Value) -> Event {
    let payload: StartPayload = payload(value);
    Event::Start {
        name: text(payload.name),
        storage: text(payload.storage),
    }
}

fn parse_backup(value: &Value) -> Event {
    let payload: BackupPayload = payload(value);
    let name = text(payload.name);
    if !data::is_safe_backup_name(&name) {
        return Event::Unknown;
    }
    Event::Backup(Backup {
        name,
        date: text(payload.date)
            .replace('T', " ")
            .chars()
            .take(16)
            .collect(),
        label: text(payload.label),
        packages: payload.packages.unwrap_or(0) as usize,
        size: payload.size.unwrap_or(0),
        version: text(payload.version),
        compression: text(payload.compression).to_uppercase(),
        modified: None,
    })
}

fn parse_stage(value: &Value) -> Event {
    let payload: StagePayload = payload(value);
    Event::Stage {
        id: text(payload.id),
        label: text(payload.label),
        state: match payload.state.as_deref().unwrap_or("") {
            "start" => StageState::Active,
            "complete" => StageState::Complete,
            "failed" => StageState::Failed,
            "skipped" => StageState::Skipped,
            _ => StageState::Pending,
        },
        index: payload.index.unwrap_or(0) as usize,
        total: payload.total.unwrap_or(0) as usize,
    }
}

fn parse_progress(value: &Value) -> Event {
    let payload: ProgressPayload = payload(value);
    Event::Progress {
        stage: text(payload.stage),
        current: payload.current.unwrap_or(0),
        total: payload.total,
        unit: payload.unit,
        detail: payload.detail,
    }
}

fn parse_log(value: &Value) -> Event {
    let payload: LogPayload = payload(value);
    Event::Log {
        level: text(payload.level),
        message: text(payload.message),
    }
}

fn parse_verify(value: &Value) -> Event {
    let payload: VerifyPayload = payload(value);
    Event::Verify {
        state: text(payload.state),
        details: text(payload.details),
    }
}

fn parse_plan(value: &Value) -> Event {
    let payload: PlanPayload = payload(value);
    let versions = payload.versions.unwrap_or_default();
    let version_text =
        |value: Option<String>| data::strip_controls(value.as_deref().unwrap_or("—")).into_owned();
    Event::Plan(Plan {
        name: text(payload.name),
        date: text(payload.date),
        label: text(payload.label),
        storage: text(payload.storage),
        backup_version: version_text(versions.backup),
        current_version: version_text(versions.current),
        packages: payload.packages.unwrap_or(0) as usize,
        to_install: payload.to_install.map(|n| n as usize),
        already_installed: payload.already_installed.map(|n| n as usize),
        device_only: payload.device_only.map(|n| n as usize),
        diff_available: payload.diff_available.unwrap_or(false),
        archives: payload.archives.unwrap_or_default(),
        size: payload.size.unwrap_or(0),
        checksums: text(payload.checksums),
    })
}

fn parse_package_diff(value: &Value) -> Event {
    let payload: PackageDiffPayload = payload(value);
    Event::PackageDiff {
        category: text(payload.category),
        part: payload.part.unwrap_or(1) as usize,
        parts: payload.parts.unwrap_or(1) as usize,
        packages: payload.packages.unwrap_or_default(),
    }
}

fn parse_result(value: &Value) -> Event {
    let payload: ResultPayload = payload(value);
    Event::Result(Outcome {
        ok: payload.ok.unwrap_or(false),
        name: text(payload.name),
        size: payload.size.unwrap_or(0),
        verified: payload.verified,
        verify_details: text(payload.verify_details),
        packages: payload.packages.unwrap_or(0) as usize,
        packages_installed: payload.packages_installed.unwrap_or(0) as usize,
        packages_failed: payload.packages_failed.unwrap_or_default(),
        archives: payload.archives.unwrap_or(0) as usize,
        archives_failed: payload.archives_failed.unwrap_or_default(),
        warnings: payload.warnings.unwrap_or(false),
        restart_required: payload.restart_required.unwrap_or(false),
        duration_ms: payload.duration_ms.unwrap_or(0),
    })
}

fn parse_error(value: &Value) -> Event {
    let payload: ErrorPayload = payload(value);
    Event::Error(OpError {
        message: text(payload.message),
        hint: text(payload.hint),
        exit_code: payload.exit_code.map(|n| n as i32),
    })
}

fn parse_cancelled(value: &Value) -> Event {
    let payload: CancelledPayload = payload(value);
    Event::Cancelled {
        reason: text(payload.reason),
    }
}

enum Msg {
    Event(Box<Event>),
    Malformed(String),
    Stderr(String),
    Eof,
}

/// Lists backups through the configured CLI.
pub fn collect_backups() -> Result<Vec<Backup>, String> {
    collect_backups_with(&data::cli_bin())
}

/// Lists backups through a specific binary, keeping only `backup` events.
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

/// Whether the CLI accepts `flag`, cached per process unless `TERMUX_SYNC_BIN` is set.
///
/// Probing runs `termux-sync logs <flag>` with output suppressed.
pub fn supports_flag(flag: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    // Tests point TERMUX_SYNC_BIN at a fixture binary, so keep those probes live.
    let cacheable = std::env::var_os("TERMUX_SYNC_BIN").is_none();
    if cacheable
        && let Ok(map) = cache.lock()
        && let Some(answer) = map.get(flag)
    {
        return *answer;
    }
    let mut command = data::cli_command(&["logs".to_string(), flag.to_string()]);
    let answer = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if cacheable && let Ok(mut map) = cache.lock() {
        map.insert(flag.to_string(), answer);
    }
    answer
}

#[cfg(test)]
fn supports_flag_with(bin: &std::path::Path, flag: &str) -> bool {
    std::process::Command::new(bin)
        .args(["logs", flag])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
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

/// Whether the CLI supports `--json-events`.
#[must_use]
pub fn supports_events() -> bool {
    supports_flag("--json-events")
}

/// Whether the CLI supports `--yes`.
#[must_use]
pub fn supports_yes() -> bool {
    supports_flag("--yes")
}

/// A running CLI child process and the state parsed from its events.
pub struct Operation {
    /// Backup or restore.
    pub kind: Kind,
    /// Backup name supplied at startup.
    pub name: String,
    /// Backend reported by the `start` event.
    pub storage: String,
    /// Current lifecycle state.
    pub status: Status,
    /// Stage list with per-stage state.
    pub stages: Vec<Stage>,
    /// Active stage id.
    pub current_stage: Option<String>,
    /// Progress within the active stage.
    pub stage_current: u64,
    /// Total for the active stage, when known.
    pub stage_total: Option<u64>,
    /// Unit for the active stage, for example `files`.
    pub stage_unit: Option<String>,
    /// Extra detail for the active stage.
    pub stage_detail: Option<String>,
    /// Verification state and details, once reported.
    pub verify: Option<(String, String)>,
    /// Restore plan, once received.
    pub plan: Option<Plan>,
    /// Streamed package diff.
    pub diff: PackageDiff,
    /// Bounded operation log, oldest first.
    pub logs: VecDeque<LogEntry>,
    /// Final outcome, once received.
    pub result: Option<Outcome>,
    /// Final error, once received.
    pub error: Option<OpError>,
    /// Transient message shown in the operation footer.
    pub notice: String,
    /// What the user typed in the restore confirmation.
    pub confirm_text: String,
    /// Whether the diff overlay is open.
    pub diff_open: bool,
    /// Selected diff section (0, 1, or 2).
    pub diff_section: usize,
    /// Current diff filter text.
    pub diff_filter: String,
    /// Whether diff filter input has focus.
    pub diff_filtering: bool,
    /// Scroll offset in the diff overlay.
    pub diff_scroll: usize,
    /// Scroll offset in the log pane.
    pub log_offset: usize,
    /// Whether cancellation is blocked by a protected restore.
    pub protected: bool,
    /// Whether the cancel confirmation is shown.
    pub cancel_prompt: bool,
    /// Disables spinner animation.
    pub reduce_motion: bool,
    /// When the process was spawned.
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
    /// Spawns the CLI with `--json-events` and starts reader threads.
    ///
    /// `name` is shown in the UI; `args` is the command line without the event flag.
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
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    let _ = tx.send(Msg::Stderr(line));
                }
            });
        }

        let stages = stage_defs(kind)
            .iter()
            .copied()
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

    /// Drains child output and updates state, returning whether anything changed.
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
            Event::Start { name, storage } => self.apply_start(name, storage),
            Event::Stage {
                id,
                label,
                state,
                index,
                total,
            } => self.apply_stage(id, label, state, index, total),
            Event::Progress {
                stage,
                current,
                total,
                unit,
                detail,
            } => self.apply_progress(stage, current, total, unit, detail),
            Event::Log { level, message } => self.push_log(&level, message),
            Event::Verify { state, details } => self.apply_verify(state, details),
            Event::Plan(plan) => self.apply_plan(plan),
            Event::PackageDiff {
                category,
                part,
                parts,
                packages,
            } => self.diff.apply(&category, part, parts, packages),
            Event::Result(outcome) => self.apply_result(outcome),
            Event::Error(error) => self.apply_error(error),
            Event::Cancelled { reason } => self.apply_cancelled(reason),
            Event::Unsupported(version) => self.apply_unsupported(version),
            Event::Backup(_) => {}
            Event::Unknown => {}
        }
    }

    fn apply_start(&mut self, name: String, storage: String) {
        if !name.is_empty() {
            self.name = name;
        }
        self.storage = storage;
    }

    fn apply_stage(
        &mut self,
        id: String,
        label: String,
        state: StageState,
        index: usize,
        total: usize,
    ) {
        if id.is_empty() {
            return;
        }
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
            .map_or(label.as_str(), |(_, label, _)| *label)
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

    fn apply_progress(
        &mut self,
        stage: String,
        current: u64,
        total: Option<u64>,
        unit: Option<String>,
        detail: Option<String>,
    ) {
        self.current_stage = Some(stage);
        self.stage_current = current;
        self.stage_total = total;
        self.stage_unit = unit;
        self.stage_detail = detail;
    }

    fn apply_verify(&mut self, state: String, details: String) {
        self.verify = Some((state, details));
    }

    fn apply_plan(&mut self, plan: Plan) {
        self.name = plan.name.clone();
        self.storage = plan.storage.clone();
        self.plan = Some(plan);
        self.status = Status::AwaitingConfirm;
    }

    fn apply_result(&mut self, outcome: Outcome) {
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

    fn apply_error(&mut self, error: OpError) {
        self.status = Status::Failed;
        self.error = Some(error);
    }

    fn apply_cancelled(&mut self, reason: String) {
        self.status = Status::Cancelled;
        if !reason.is_empty() {
            self.notice = format!("cancelled: {reason}");
        }
    }

    fn apply_unsupported(&mut self, version: u64) {
        self.status = Status::Failed;
        self.error = Some(OpError {
            message: format!("event protocol v{version} is newer than this TUI supports"),
            hint: "Update termux-sync-tui.".to_string(),
            exit_code: None,
        });
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

    /// Asks the CLI to stop, returning whether the request was accepted.
    ///
    /// Restores answer the confirmation prompt with `CANCEL`; running work gets `SIGINT`.
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

    /// Sends the typed `RESTORE` confirmation.
    ///
    /// Returns false while not awaiting confirmation, when checksums failed, or when stdin is gone.
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
            // SAFETY: the child was spawned with `process_group(0)` (see `start`),
            // so its pid is also the process-group id and no other process shares
            // the group. `kill` only reads the pid and signal arguments.
            unsafe {
                libc::kill(-pid, signal);
            }
        }
    }

    fn kill_forcefully(&mut self) {
        self.signal_group(libc::SIGKILL);
    }

    /// Whether the current status permits a cancel request.
    ///
    /// Protected restores may not be interrupted.
    #[must_use]
    pub fn cancel_allowed(&self) -> bool {
        match self.status {
            Status::AwaitingConfirm => true,
            Status::Running => self.kind == Kind::Backup || !self.protected,
            _ => false,
        }
    }

    /// Whether the operation reached a terminal status.
    #[must_use]
    pub fn is_done(&self) -> bool {
        matches!(
            self.status,
            Status::Succeeded | Status::Warnings | Status::Failed | Status::Cancelled
        )
    }

    /// Whether the operation is running, awaiting confirmation, or cancelling.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(
            self.status,
            Status::Running | Status::AwaitingConfirm | Status::Cancelling
        )
    }

    /// Weighted progress over non-skipped stages, clamped to `0.0..=1.0`.
    #[must_use]
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

    /// Human-readable progress for the active stage.
    #[must_use]
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

    /// Time since the process was spawned.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Arguments to reuse when retrying this operation.
    #[must_use]
    pub fn retry_args(&self) -> Vec<String> {
        self.args.clone()
    }

    /// Appends a log line, dropping the oldest beyond `LOG_LIMIT`.
    pub fn push_log(&mut self, level: &str, message: impl Into<String>) {
        self.logs.push_back(LogEntry {
            level: level.to_string(),
            message: message.into(),
        });
        while self.logs.len() > LOG_LIMIT {
            self.logs.pop_front();
        }
    }

    /// Packages in the selected section matching the diff filter.
    #[must_use]
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

const BACKUP_STAGES: &[(&str, &str, f32)] = &[
    ("preflight", "Preflight", 2.0),
    ("packages", "Packages", 5.0),
    ("archive:home", "Home", 50.0),
    ("archive:usr_etc", "Config", 13.0),
    ("manifest", "Manifest", 5.0),
    ("upload", "Upload", 20.0),
    ("verify", "Verify", 5.0),
];

const RESTORE_STAGES: &[(&str, &str, f32)] = &[
    ("metadata", "Metadata", 2.0),
    ("download", "Download", 25.0),
    ("verify", "Verify", 15.0),
    ("extract:home", "Home", 20.0),
    ("extract:usr_etc", "Config", 5.0),
    ("packages", "Packages", 28.0),
    ("cleanup", "Cleanup", 5.0),
];

fn stage_defs(kind: Kind) -> &'static [(&'static str, &'static str, f32)] {
    match kind {
        Kind::Backup => BACKUP_STAGES,
        Kind::Restore => RESTORE_STAGES,
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

/// Serializes the tests that mutate process environment variables.
#[cfg(test)]
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Sets an environment variable for one test and removes it again on drop.
#[cfg(test)]
struct ScopedEnv {
    key: &'static str,
}

#[cfg(test)]
impl ScopedEnv {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        // SAFETY: the caller holds `ENV_LOCK`, and no other test in this
        // binary reads or writes this variable.
        unsafe { std::env::set_var(key, value) };
        Self { key }
    }
}

#[cfg(test)]
impl Drop for ScopedEnv {
    fn drop(&mut self) {
        // SAFETY: the caller held `ENV_LOCK` while the variable was set, and
        // the guard drops before the lock.
        unsafe { std::env::remove_var(self.key) };
    }
}

/// Build an inert operation for tests.
#[cfg(test)]
#[must_use]
pub fn test_operation(kind: Kind) -> Operation {
    let stages = stage_defs(kind)
        .iter()
        .copied()
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

/// Build a sample restore plan for tests.
#[cfg(test)]
#[must_use]
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
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _env = ScopedEnv::set("TERMUX_SYNC_BIN", &bin);
        let mut op = Operation::start(Kind::Backup, "x", vec!["backup".into()], false).unwrap();
        op.status = Status::AwaitingConfirm;
        assert!(op.request_cancel());
        assert!(op.cancel_sent_at.is_some());
        op.kill_forcefully();
        let _ = op.poll();
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
