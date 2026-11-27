use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{ListState, TableState};

use crate::tui::data::{self, Backup, Snapshot};
use crate::tui::history::{HistoryEntry, HistoryStore};
use crate::tui::ops::{Kind, Operation, Status};
use crate::tui::ui::UiLayout;

/// Top-level screen shown by the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Panes, overlays, and the status footer.
    Dashboard,
    /// Full-screen view of a running operation.
    Operation,
}

/// Where the Output pane content came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputSource {
    /// Tail of `sync.log`.
    Log,
    /// Captured output of a one-shot CLI command.
    Command,
}

/// Pane that receives navigation keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The actions list.
    Actions,
    /// The backup table.
    Backups,
    /// The output log.
    Output,
}

impl Focus {
    /// Next pane in Tab order, wrapping around.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Focus::Actions => Focus::Backups,
            Focus::Backups => Focus::Output,
            Focus::Output => Focus::Actions,
        }
    }

    /// Previous pane in Tab order, wrapping around.
    #[must_use]
    pub fn previous(self) -> Self {
        match self {
            Focus::Actions => Focus::Output,
            Focus::Backups => Focus::Actions,
            Focus::Output => Focus::Backups,
        }
    }

    /// Header title for the pane.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Focus::Actions => "Actions",
            Focus::Backups => "Backups",
            Focus::Output => "Output",
        }
    }
}

/// User-visible action, bound to a key or run from the Actions pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Prompt for a label and start a backup.
    Backup,
    /// Restore the selected backup.
    Restore,
    /// Ask to delete the selected backup.
    Delete,
    /// List backups, including remote storage.
    List,
    /// Show the tail of `sync.log`.
    Logs,
    /// Ask to clear the log file.
    ClearLogs,
    /// Collect and show the status report.
    Status,
    /// Run a disk-space check.
    DiskCheck,
    /// Prompt for a path and export redacted config.
    ExportConfig,
    /// Prompt for a path and import config.
    ImportConfig,
    /// Open the setup handoff card.
    Setup,
    /// Open the schedule handoff card.
    Schedule,
    /// Show daemon activity and boot status.
    Daemon,
    /// Ask to clear the cache.
    ClearCache,
    /// Reload backups and status.
    Refresh,
    /// Open the help overlay.
    Help,
    /// Exit the dashboard.
    Quit,
}

/// Text prompt currently waiting for input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputPrompt {
    /// Optional label for a new backup.
    BackupLabel,
    /// Destination path for an exported config.
    ExportConfig,
    /// Path to a config export to import.
    ImportConfig,
    /// Path for the disk check, blank to check all.
    CheckTarget,
}

impl InputPrompt {
    /// Overlay title.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            InputPrompt::BackupLabel => "Backup label",
            InputPrompt::ExportConfig => "Export config",
            InputPrompt::ImportConfig => "Import config",
            InputPrompt::CheckTarget => "Disk check target",
        }
    }

    /// One-line explanation shown with the prompt.
    #[must_use]
    pub fn hint(self) -> &'static str {
        match self {
            InputPrompt::BackupLabel => "Used in the manifest and backup name.",
            InputPrompt::ExportConfig => "Tokens and passwords are redacted by the CLI.",
            InputPrompt::ImportConfig => "Path to a previously exported config file.",
            InputPrompt::CheckTarget => "Blank checks all paths. You can type a path or ~.",
        }
    }

    /// Initial text placed in the input.
    #[must_use]
    pub fn seed(self) -> &'static str {
        match self {
            InputPrompt::ExportConfig => "~/termux-sync-config-export.json",
            InputPrompt::BackupLabel | InputPrompt::ImportConfig | InputPrompt::CheckTarget => "",
        }
    }

    /// Maximum accepted input length in bytes.
    #[must_use]
    pub fn max_len(self) -> usize {
        match self {
            InputPrompt::BackupLabel => 64,
            InputPrompt::ExportConfig | InputPrompt::ImportConfig | InputPrompt::CheckTarget => 256,
        }
    }

    /// Footer message when the prompt is cancelled.
    #[must_use]
    pub fn cancel_status(self) -> &'static str {
        match self {
            InputPrompt::BackupLabel => "backup cancelled",
            InputPrompt::ExportConfig => "export cancelled",
            InputPrompt::ImportConfig => "import cancelled",
            InputPrompt::CheckTarget => "disk check cancelled",
        }
    }
}

/// One row in the Actions pane.
pub struct ActionItem {
    /// Unicode glyph shown in normal mode.
    pub glyph: &'static str,
    /// ASCII replacement used with `--ascii`.
    pub ascii: &'static str,
    /// Row label.
    pub label: &'static str,
    /// Action run when the row is activated.
    pub action: Action,
}

/// Actions pane rows in display order.
pub const ACTIONS: &[ActionItem] = &[
    ActionItem {
        glyph: "↑",
        ascii: "+",
        label: "Backup now",
        action: Action::Backup,
    },
    ActionItem {
        glyph: "↺",
        ascii: "<",
        label: "Restore selected",
        action: Action::Restore,
    },
    ActionItem {
        glyph: "✕",
        ascii: "x",
        label: "Delete selected",
        action: Action::Delete,
    },
    ActionItem {
        glyph: "≡",
        ascii: "=",
        label: "List backups",
        action: Action::List,
    },
    ActionItem {
        glyph: "▤",
        ascii: "#",
        label: "Show logs",
        action: Action::Logs,
    },
    ActionItem {
        glyph: "⌫",
        ascii: "-",
        label: "Clear logs",
        action: Action::ClearLogs,
    },
    ActionItem {
        glyph: "◉",
        ascii: "i",
        label: "Status",
        action: Action::Status,
    },
    ActionItem {
        glyph: "◔",
        ascii: "%",
        label: "Disk check",
        action: Action::DiskCheck,
    },
    ActionItem {
        glyph: "⇧",
        ascii: ">",
        label: "Export config",
        action: Action::ExportConfig,
    },
    ActionItem {
        glyph: "⇩",
        ascii: "v",
        label: "Import config",
        action: Action::ImportConfig,
    },
    ActionItem {
        glyph: "⚙",
        ascii: "*",
        label: "Setup",
        action: Action::Setup,
    },
    ActionItem {
        glyph: "◷",
        ascii: "@",
        label: "Schedule",
        action: Action::Schedule,
    },
    ActionItem {
        glyph: "◌",
        ascii: "d",
        label: "Daemon",
        action: Action::Daemon,
    },
    ActionItem {
        glyph: "◇",
        ascii: "c",
        label: "Clear cache",
        action: Action::ClearCache,
    },
    ActionItem {
        glyph: "⟳",
        ascii: "~",
        label: "Refresh",
        action: Action::Refresh,
    },
    ActionItem {
        glyph: "?",
        ascii: "?",
        label: "Help",
        action: Action::Help,
    },
    ActionItem {
        glyph: "↩",
        ascii: "q",
        label: "Quit",
        action: Action::Quit,
    },
];

/// Background job submitted to [`Worker`].
pub enum Task {
    /// Reload the whole snapshot.
    Refresh,
    /// Run one CLI command and capture its output.
    Command(Vec<String>),
    /// List backups through the event protocol.
    ListBackups,
    /// Collect a status report for this config.
    Status(data::Config),
}

/// Result sent back by a [`Worker`] thread.
pub enum WorkerMsg {
    /// A complete snapshot.
    Refreshed(Snapshot),
    /// Captured command output with its title.
    Output {
        /// Title shown on the output pane.
        title: String,
        /// Captured stdout and stderr.
        text: String,
    },
    /// Structured backup list.
    Backups(Vec<Backup>),
    /// Collected status report.
    Status(data::StatusReport),
}

/// Runs one background task at a time and delivers its result.
#[derive(Default)]
pub struct Worker {
    rx: Option<Receiver<Result<WorkerMsg, String>>>,
    busy: bool,
    writes: bool,
}

/// Transient card shown while a foreground command runs.
pub struct TaskOverlay {
    /// Text shown on the card.
    pub label: String,
    /// Tick when the task started.
    pub started_at: u64,
    /// Whether the task finished.
    pub done: bool,
    /// Tick when the task finished.
    pub done_at: u64,
    /// Whether the card has been visible long enough to draw.
    pub visible: bool,
}

impl Worker {
    /// Whether a task is in flight.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Whether the in-flight task runs a command that must not be abandoned.
    #[must_use]
    pub fn writes(&self) -> bool {
        self.busy && self.writes
    }

    /// Spawns `task` unless one is already running, returning whether it started.
    pub fn start(&mut self, task: Task) -> bool {
        if self.busy {
            return false;
        }
        self.writes = matches!(&task, Task::Command(_));
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = match task {
                Task::Refresh => Ok(WorkerMsg::Refreshed(data::load_snapshot())),
                Task::ListBackups => match crate::tui::ops::collect_backups() {
                    Ok(backups) => Ok(WorkerMsg::Backups(backups)),
                    // An old CLI has no event stream: fall back to captured text.
                    Err(_) if !crate::tui::ops::supports_events() => {
                        data::run_captured(&["list".to_string()]).map(|text| WorkerMsg::Output {
                            title: "list".to_string(),
                            text,
                        })
                    }
                    Err(error) => Err(error),
                },
                Task::Status(config) => Ok(WorkerMsg::Status(data::collect_status(&config))),
                Task::Command(args) => data::run_captured(&args).map(|text| WorkerMsg::Output {
                    title: args.join(" "),
                    text,
                }),
            };
            let _ = tx.send(result);
        });
        self.rx = Some(rx);
        self.busy = true;
        true
    }

    /// Non-blocking result check; clears the busy state on a result or disconnect.
    pub fn poll(&mut self) -> Option<Result<WorkerMsg, String>> {
        let rx = self.rx.as_ref()?;
        match rx.try_recv() {
            Ok(message) => {
                self.rx = None;
                self.busy = false;
                self.writes = false;
                Some(message)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                self.busy = false;
                self.writes = false;
                Some(Err("background task stopped unexpectedly".to_string()))
            }
        }
    }
}

/// Clickable rectangles in the header, recorded by the renderer.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeaderAreas {
    /// Backup count tile.
    pub count: Rect,
    /// Warning area.
    pub warning: Rect,
    /// Status area.
    pub status: Rect,
}

/// Card that hands an interactive command back to the terminal.
#[derive(Debug, Clone)]
pub struct HandoffInfo {
    /// Command to hand off.
    pub action: Action,
    /// Card title.
    pub title: &'static str,
    /// One-line description of what the command asks.
    pub detail: &'static str,
}

#[derive(Debug, Default)]
struct FilteredCache {
    filter: String,
    revision: u64,
    count: usize,
    indices: Rc<[usize]>,
}

/// Mutable dashboard state shared by the input loop and renderer.
///
/// Fields are public so [`crate::tui::ui`] can read them and store layout rectangles.
pub struct App {
    /// Latest data loaded from disk.
    pub snapshot: Snapshot,
    filtered_cache: RefCell<FilteredCache>,
    backups_revision: u64,
    /// Selection in the Actions pane.
    pub action_state: ListState,
    /// Selection in the Backups pane, indexed over filtered rows.
    pub backup_state: TableState,
    /// Case-insensitive filter for backups.
    pub backup_filter: String,
    /// Whether filter input has focus.
    pub backup_filtering: bool,
    /// Pane that receives navigation keys.
    pub focus: Focus,
    /// Lines in the Output pane.
    pub output: Vec<String>,
    /// Scroll offset from the bottom of `output`.
    pub output_offset: usize,
    /// Whether `output` holds the log tail or command output.
    pub output_source: OutputSource,
    /// How many log lines to keep in the Output pane.
    pub log_lines: usize,
    /// Footer status line.
    pub status: String,
    /// Whether the help overlay is open.
    pub show_help: bool,
    /// Whether the status overlay is open.
    pub show_status: bool,
    /// Scroll offset in the status overlay.
    pub status_scroll: usize,
    /// Last computed maximum scroll for the status overlay.
    pub status_scroll_max: usize,
    /// Report shown by the status overlay.
    pub status_report: Option<data::StatusReport>,
    /// Whether the daemon overlay is open.
    pub show_daemon: bool,
    /// Scroll offset in the daemon overlay.
    pub daemon_scroll: usize,
    /// Last computed maximum scroll for the daemon overlay.
    pub daemon_scroll_max: usize,
    /// Recent daemon log lines.
    pub daemon_log: Vec<String>,
    /// Whether the Termux:Boot script exists.
    pub daemon_boot_present: bool,
    /// Keeps the Output pane after a refresh; used by clear-logs.
    pub preserve_output: bool,
    /// Set when the event loop must exit.
    pub should_quit: bool,
    /// Command waiting to be handed back to the terminal.
    pub pending: Option<Vec<String>>,
    /// Background task runner.
    pub worker: Worker,
    /// Monotonic frame counter used for animation.
    pub tick: u64,
    /// Tick when the help overlay opened.
    pub help_opened_at: u64,
    /// Card shown for a foreground command.
    pub task: Option<TaskOverlay>,
    /// True until the first snapshot arrives.
    pub loading: bool,
    /// Scroll offset in the help overlay.
    pub help_scroll: usize,
    /// Recent operation results, newest first.
    pub history: Vec<HistoryEntry>,
    /// Whether the history overlay is open.
    pub show_history: bool,
    /// Selected row in the history overlay.
    pub history_selected: usize,
    /// Disables animation.
    pub reduce_motion: bool,
    /// Uses ASCII glyphs instead of Unicode.
    pub ascii: bool,
    /// Screen currently rendered.
    pub screen: Screen,
    /// Running operation, when `screen` is [`Screen::Operation`].
    pub operation: Option<Operation>,
    /// Whether the finished operation was recorded in history.
    pub op_result_handled: bool,
    /// Backup name to select after the next refresh.
    pub pending_select: Option<String>,
    /// Action waiting for confirmation.
    pub pending_confirm: Option<Action>,
    /// Prompt waiting for text.
    pub pending_input: Option<InputPrompt>,
    /// Card waiting for handoff confirmation.
    pub pending_handoff: Option<HandoffInfo>,
    /// Yes button rectangle on the handoff card.
    pub handoff_yes: Rect,
    /// No button rectangle on the handoff card.
    pub handoff_no: Rect,
    /// Current text in the input prompt.
    pub input_text: String,
    /// Confirm button rectangle on the input prompt.
    pub input_yes: Rect,
    /// Cancel button rectangle on the input prompt.
    pub input_no: Rect,
    /// Yes button rectangle on the confirmation prompt.
    pub confirm_yes: Rect,
    /// No button rectangle on the confirmation prompt.
    pub confirm_no: Rect,
    /// Header hit-test regions, set by the renderer.
    pub header_areas: HeaderAreas,
    /// Stat tile hit-test regions.
    pub stat_areas: [Rect; 4],
    /// Actions pane rectangle.
    pub actions_area: Rect,
    /// Backups pane rectangle.
    pub backups_area: Rect,
    /// Output pane rectangle.
    pub output_area: Rect,
    /// Max scroll offset of the Output pane, set by the renderer.
    pub output_scroll_max: usize,
    /// Max scroll offset of the help overlay, set by the renderer.
    pub help_scroll_max: usize,
    /// Max scroll offset of the operation log, set by the renderer.
    pub log_scroll_max: usize,
    /// Max scroll offset of the package diff, set by the renderer.
    pub diff_scroll_max: usize,
    /// CLI features probed at startup.
    pub caps: crate::tui::ops::Capabilities,
    caps_rx: Option<Receiver<crate::tui::ops::Capabilities>>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Creates an app, loads history, and starts an initial refresh.
    #[must_use]
    pub fn new() -> Self {
        let (history, history_error) = match HistoryStore::load(&data::history_path()) {
            Ok(entries) => (entries, None),
            Err(error) => (Vec::new(), Some(format!("history: {error}"))),
        };
        let mut app = Self::with_snapshot(Snapshot::default());
        app.history = history;
        app.loading = true;
        app.status = "loading backups…".to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::tui::ops::Capabilities::detect());
        });
        app.caps_rx = Some(rx);
        app.refresh();
        app.status = history_error.unwrap_or_else(|| "loading backups…".to_string());
        app
    }

    /// Creates an app seeded with `snapshot` and no background work.
    #[must_use]
    pub fn with_snapshot(snapshot: Snapshot) -> Self {
        let mut action_state = ListState::default();
        action_state.select(Some(0));
        let mut backup_state = TableState::default();
        if !snapshot.backups.is_empty() {
            backup_state.select(Some(0));
        }
        let output = snapshot.log.clone();
        let status = snapshot
            .warning_text()
            .unwrap_or_else(|| "ready".to_string());

        Self {
            snapshot,
            filtered_cache: RefCell::new(FilteredCache::default()),
            backups_revision: 0,
            action_state,
            backup_state,
            backup_filter: String::new(),
            backup_filtering: false,
            focus: Focus::Actions,
            output,
            output_offset: 0,
            output_source: OutputSource::Log,
            log_lines: 200,
            status,
            show_help: false,
            show_status: false,
            status_scroll: 0,
            status_scroll_max: 0,
            status_report: None,
            show_daemon: false,
            daemon_scroll: 0,
            daemon_scroll_max: 0,
            daemon_log: Vec::new(),
            daemon_boot_present: false,
            preserve_output: false,
            should_quit: false,
            pending: None,
            worker: Worker::default(),
            tick: 0,
            help_opened_at: 0,
            task: None,
            loading: false,
            help_scroll: 0,
            history: Vec::new(),
            show_history: false,
            history_selected: 0,
            reduce_motion: false,
            ascii: false,
            screen: Screen::Dashboard,
            operation: None,
            op_result_handled: false,
            pending_select: None,
            pending_confirm: None,
            pending_input: None,
            pending_handoff: None,
            handoff_yes: Rect::default(),
            handoff_no: Rect::default(),
            input_text: String::new(),
            input_yes: Rect::default(),
            input_no: Rect::default(),
            confirm_yes: Rect::default(),
            confirm_no: Rect::default(),
            header_areas: HeaderAreas::default(),
            stat_areas: [Rect::default(); 4],
            actions_area: Rect::default(),
            backups_area: Rect::default(),
            output_area: Rect::default(),
            output_scroll_max: 0,
            help_scroll_max: 0,
            log_scroll_max: 0,
            diff_scroll_max: 0,
            caps: crate::tui::ops::Capabilities::default(),
            caps_rx: None,
        }
    }

    /// Opens the help overlay from the top, closing any other overlay.
    pub fn open_help(&mut self) {
        self.close_panels();
        self.show_help = true;
        self.help_opened_at = self.tick;
        self.help_scroll = 0;
    }

    /// Opens the status overlay with `report`, closing any other overlay.
    pub fn open_status_report(&mut self, report: data::StatusReport) {
        self.close_panels();
        self.status_report = Some(report);
        self.show_status = true;
        self.status_scroll = 0;
    }

    /// Loads daemon activity and opens the daemon overlay, closing any other.
    pub fn open_daemon(&mut self) {
        self.close_panels();
        self.daemon_log = data::daemon_activity(&data::tail_log(500));
        self.daemon_boot_present = data::boot_script_path().is_file();
        self.show_daemon = true;
        self.daemon_scroll = 0;
    }

    /// Closes every overlay so at most one is ever open.
    fn close_panels(&mut self) {
        self.show_help = false;
        self.show_history = false;
        self.show_status = false;
        self.show_daemon = false;
        self.help_scroll = 0;
        self.status_scroll = 0;
        self.daemon_scroll = 0;
    }

    /// Stores the layout produced by the last frame and clamps scroll offsets.
    pub fn apply_layout(&mut self, layout: UiLayout) {
        self.header_areas = layout.header;
        self.stat_areas = layout.stat_areas;
        self.actions_area = layout.actions_area;
        self.backups_area = layout.backups_area;
        self.output_area = layout.output_area;
        self.confirm_yes = layout.confirm_yes;
        self.confirm_no = layout.confirm_no;
        self.input_yes = layout.input_yes;
        self.input_no = layout.input_no;
        self.handoff_yes = layout.handoff_yes;
        self.handoff_no = layout.handoff_no;

        self.output_scroll_max = layout.output_scroll_max;
        self.help_scroll_max = layout.help_scroll_max;
        self.status_scroll_max = layout.status_scroll_max;
        self.daemon_scroll_max = layout.daemon_scroll_max;
        self.log_scroll_max = layout.log_scroll_max;
        self.diff_scroll_max = layout.diff_scroll_max;

        if self.screen == Screen::Dashboard {
            self.output_offset = self.output_offset.min(self.output_scroll_max);
            self.help_scroll = self.help_scroll.min(self.help_scroll_max);
            self.status_scroll = self.status_scroll.min(self.status_scroll_max);
            self.daemon_scroll = self.daemon_scroll.min(self.daemon_scroll_max);
        }
        if let Some(operation) = &mut self.operation {
            operation.log_offset = operation.log_offset.min(self.log_scroll_max);
            operation.diff_scroll = operation.diff_scroll.min(self.diff_scroll_max);
        }
    }

    /// Applies the CLI capabilities probed at startup. Returns whether they arrived.
    pub fn poll_capabilities(&mut self) -> bool {
        let Some(rx) = &self.caps_rx else {
            return false;
        };
        match rx.try_recv() {
            Ok(caps) => {
                self.caps = caps;
                self.caps_rx = None;
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                // The probe died: keep the optimistic defaults.
                self.caps_rx = None;
                true
            }
        }
    }

    /// Quits unless a background command is still writing to its output pipe.
    fn request_quit(&mut self) {
        if self.worker.writes() {
            self.status = "a command is still running — wait, or press Ctrl+C to quit".to_string();
            return;
        }
        self.should_quit = true;
    }

    /// Animation progress for the help overlay; `1.0` when motion is reduced.
    pub fn help_progress(&self) -> f32 {
        if !self.show_help || self.reduce_motion {
            return 1.0;
        }
        progress(self.tick, self.help_opened_at, 4)
    }

    /// Animation progress for the task card; `1.0` when motion is reduced.
    pub fn task_progress(&self) -> f32 {
        match &self.task {
            Some(task) if !self.reduce_motion => {
                progress(self.tick, task.started_at.saturating_add(3), 2)
            }
            _ => 1.0,
        }
    }

    /// Whether anything on screen still animates and needs frames.
    pub fn needs_tick(&self) -> bool {
        self.worker.is_busy()
            || self.task.is_some()
            || self.help_progress() < 1.0
            || self
                .operation
                .as_ref()
                .is_some_and(super::ops::Operation::is_running)
    }

    /// Advances the frame counter and retires the task card on schedule.
    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        let now = self.tick;
        let mut remove = false;

        if let Some(task) = &mut self.task {
            if !task.visible && now.saturating_sub(task.started_at) >= 3 {
                task.visible = true;
            }
            if task.done && now.saturating_sub(task.done_at) >= 3 {
                remove = true;
            }
        }
        if remove {
            self.task = None;
        }
    }

    /// Current spinner frame, or `None` when idle or motion is reduced.
    pub fn spinner(&self) -> Option<&'static str> {
        if self.reduce_motion {
            return None;
        }
        if !self.worker.is_busy()
            && self.pending.is_none()
            && !self
                .operation
                .as_ref()
                .is_some_and(super::ops::Operation::is_running)
        {
            return None;
        }
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        Some(FRAMES[(self.tick as usize / 2) % FRAMES.len()])
    }

    /// Indices of backups matching the filter, cached until the filter text or
    /// the backup list changes. Called several times per frame and per key press.
    pub fn filtered_backup_indices(&self) -> Rc<[usize]> {
        let mut cache = self.filtered_cache.borrow_mut();
        if cache.filter != self.backup_filter
            || cache.revision != self.backups_revision
            || cache.count != self.snapshot.backups.len()
        {
            let filter = self.backup_filter.to_lowercase();
            cache.indices = self
                .snapshot
                .backups
                .iter()
                .enumerate()
                .filter(|(_, backup)| {
                    filter.is_empty()
                        || backup.name.to_lowercase().contains(&filter)
                        || backup.label.to_lowercase().contains(&filter)
                        || backup.date.to_lowercase().contains(&filter)
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>()
                .into();
            cache.filter.clone_from(&self.backup_filter);
            cache.revision = self.backups_revision;
            cache.count = self.snapshot.backups.len();
        }
        Rc::clone(&cache.indices)
    }

    /// Backup under the selection, honoring the active filter.
    pub fn selected_backup(&self) -> Option<&Backup> {
        let position = self.backup_state.selected()?;
        let index = *self.filtered_backup_indices().get(position)?;
        self.snapshot.backups.get(index)
    }

    /// Routes a key code with no modifiers; used by tests.
    #[cfg(test)]
    pub fn handle_key(&mut self, code: KeyCode) {
        self.handle_key_with(code, KeyModifiers::NONE);
    }

    /// Routes a key press through overlays, then to the focused pane.
    pub fn handle_key_event(&mut self, key: KeyEvent) {
        self.handle_key_with(key.code, key.modifiers);
    }

    fn handle_key_with(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if self.handle_key_ctrl_c(code, modifiers) {
            return;
        }
        // Routing order mirrors the render order: the topmost overlay owns keys.
        if self.pending_handoff.is_some() {
            self.handle_key_handoff(code);
            return;
        }
        if self.pending_input.is_some() {
            self.handle_input_key(code, modifiers);
            return;
        }
        if self.pending_confirm.is_some() {
            self.handle_key_confirm(code);
            return;
        }
        if self.operation.is_some() {
            self.handle_operation_key(code, modifiers);
            return;
        }
        if self.backup_filtering {
            self.handle_backups_key(code);
            return;
        }
        if self.show_daemon {
            self.handle_key_daemon(code);
            return;
        }
        if self.show_status {
            self.handle_key_status(code);
            return;
        }
        if self.show_history {
            self.handle_key_history(code);
            return;
        }
        if self.show_help {
            self.handle_key_help(code);
            return;
        }
        if self.pending.is_some() {
            return;
        }

        self.handle_key_dashboard(code);
    }

    /// Ctrl-C quits, or asks the running operation to cancel. Returns whether the
    /// key was consumed, which is always true when the modifier matches.
    fn handle_key_ctrl_c(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        if !(modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c' | 'C')))
        {
            return false;
        }
        if let Some(operation) = &mut self.operation {
            if operation.is_done() {
                self.should_quit = true;
            } else if !operation.request_cancel() {
                operation.notice = "Restore in progress — cannot cancel safely".to_string();
            }
            return true;
        }
        self.pending = None;
        self.pending_confirm = None;
        self.pending_input = None;
        self.input_text.clear();
        self.pending_handoff = None;
        self.show_history = false;
        self.show_status = false;
        self.show_daemon = false;
        self.should_quit = true;
        true
    }

    fn handle_key_confirm(&mut self, code: KeyCode) {
        let Some(action) = self.pending_confirm else {
            return;
        };
        match code {
            KeyCode::Enter | KeyCode::Char('y') => {
                self.pending_confirm = None;
                self.execute_confirmed_action(action);
            }
            KeyCode::Esc | KeyCode::Char('n' | 'q') => {
                self.pending_confirm = None;
                self.status = "cancelled".to_string();
            }
            _ => {}
        }
    }

    fn handle_key_handoff(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => {
                if let Some(handoff) = self.pending_handoff.take() {
                    match handoff.action {
                        Action::Setup => self.queue_command(vec!["setup"]),
                        Action::Schedule => self.queue_command(vec!["schedule"]),
                        _ => {}
                    }
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.pending_handoff = None;
                self.status = "cancelled".to_string();
            }
            _ => {}
        }
    }

    fn handle_key_history(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc | KeyCode::Char('h' | 'q') => {
                self.show_history = false;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.history_selected = self.history_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.history_selected + 1 < self.history.len() {
                    self.history_selected += 1;
                }
            }
            KeyCode::Char('r') => {
                if let Some(entry) = self.history.get(self.history_selected).cloned() {
                    self.show_history = false;
                    match history_command(&entry) {
                        Some((kind, name, args)) => self.start_operation(kind, &name, args),
                        None => self.status = "cannot retry this entry".to_string(),
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_key_status(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc | KeyCode::Char('q' | '?') => {
                self.show_status = false;
                self.status_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.status_scroll = self
                    .status_scroll
                    .saturating_add(1)
                    .min(self.status_scroll_max);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.status_scroll = self.status_scroll.saturating_sub(1);
            }
            KeyCode::PageDown => {
                self.status_scroll = self
                    .status_scroll
                    .saturating_add(5)
                    .min(self.status_scroll_max);
            }
            KeyCode::PageUp => self.status_scroll = self.status_scroll.saturating_sub(5),
            _ => {}
        }
    }

    fn handle_key_daemon(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc | KeyCode::Char('q' | '?') => {
                self.show_daemon = false;
                self.daemon_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.daemon_scroll = self
                    .daemon_scroll
                    .saturating_add(1)
                    .min(self.daemon_scroll_max);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.daemon_scroll = self.daemon_scroll.saturating_sub(1);
            }
            KeyCode::PageDown => {
                self.daemon_scroll = self
                    .daemon_scroll
                    .saturating_add(5)
                    .min(self.daemon_scroll_max);
            }
            KeyCode::PageUp => self.daemon_scroll = self.daemon_scroll.saturating_sub(5),
            _ => {}
        }
    }

    fn handle_key_help(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc | KeyCode::Char('?' | 'q') => {
                self.show_help = false;
                self.help_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.help_scroll = self.help_scroll.saturating_add(1).min(self.help_scroll_max);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.help_scroll = self.help_scroll.saturating_sub(1);
            }
            KeyCode::PageDown => {
                self.help_scroll = self.help_scroll.saturating_add(5).min(self.help_scroll_max);
            }
            KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(5),
            _ => {}
        }
    }

    /// Global shortcuts, then keys routed to the focused pane.
    fn handle_key_dashboard(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Char('?') => self.open_help(),
            KeyCode::Tab => self.switch_focus(self.focus.next()),
            KeyCode::BackTab => self.switch_focus(self.focus.previous()),
            KeyCode::Char('R') => self.refresh(),
            KeyCode::Char('L') => self.run(Action::List),
            KeyCode::Char('s') => self.run(Action::Status),
            KeyCode::Char('h') => {
                self.close_panels();
                self.show_history = true;
                self.history_selected = 0;
            }
            _ => match self.focus {
                Focus::Actions => self.handle_actions_key(code),
                Focus::Backups => self.handle_backups_key(code),
                Focus::Output => self.handle_output_key(code),
            },
        }
    }

    fn switch_focus(&mut self, focus: Focus) {
        self.focus = focus;
    }

    fn handle_input_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let Some(prompt) = self.pending_input else {
            return;
        };
        match code {
            KeyCode::Enter => self.submit_input(prompt),
            KeyCode::Esc => {
                self.pending_input = None;
                self.input_text.clear();
                self.status = prompt.cancel_status().to_string();
            }
            KeyCode::Backspace => {
                self.input_text.pop();
            }
            KeyCode::Char(ch)
                if !ch.is_control()
                    && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && self.input_text.len() < prompt.max_len() =>
            {
                self.input_text.push(ch);
            }
            _ => {}
        }
    }

    fn submit_input(&mut self, prompt: InputPrompt) {
        if prompt == InputPrompt::BackupLabel {
            let label = self.input_text.trim().to_string();
            self.pending_input = None;
            self.input_text.clear();
            self.start_operation(Kind::Backup, "backup", backup_command(&label));
            return;
        }
        let Some(args) = input_command(prompt, &self.input_text) else {
            self.status = "enter a path".to_string();
            return;
        };
        self.pending_input = None;
        self.input_text.clear();
        self.start_task(Task::Command(args));
    }

    fn handle_operation_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let ctrl_c =
            modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c' | 'C'));
        let Some((status, is_done, diff_open)) = self
            .operation
            .as_ref()
            .map(|operation| (operation.status, operation.is_done(), operation.diff_open))
        else {
            return;
        };

        if ctrl_c {
            if is_done {
                self.should_quit = true;
            } else if !self
                .operation
                .as_mut()
                .is_some_and(Operation::request_cancel)
            {
                self.notice_operation("Restore in progress — cannot cancel safely");
            }
            return;
        }

        match status {
            Status::AwaitingConfirm => {
                if diff_open {
                    self.handle_diff_key(code, modifiers);
                } else {
                    self.handle_confirm_key(code, modifiers);
                }
            }
            Status::Running | Status::Cancelling => {
                if self
                    .operation
                    .as_ref()
                    .is_some_and(|operation| operation.cancel_prompt)
                {
                    match code {
                        KeyCode::Enter | KeyCode::Char('y') => {
                            self.operation.as_mut().map(Operation::request_cancel);
                            if let Some(operation) = &mut self.operation {
                                operation.cancel_prompt = false;
                            }
                        }
                        KeyCode::Esc | KeyCode::Char('n' | 'q') => {
                            if let Some(operation) = &mut self.operation {
                                operation.cancel_prompt = false;
                                operation.notice.clear();
                            }
                        }
                        _ => {}
                    }
                    return;
                }
                match code {
                    KeyCode::Esc | KeyCode::Char('c' | 'q')
                        if self
                            .operation
                            .as_ref()
                            .is_some_and(Operation::cancel_allowed) =>
                    {
                        if let Some(operation) = &mut self.operation {
                            operation.cancel_prompt = true;
                            operation.notice =
                                "Cancel operation and clean partial files? Enter yes · Esc no"
                                    .to_string();
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('c' | 'q') => {
                        self.notice_operation("Restore in progress — cannot cancel safely");
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if let Some(operation) = &mut self.operation {
                            operation.log_offset = operation
                                .log_offset
                                .saturating_add(1)
                                .min(self.log_scroll_max);
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if let Some(operation) = &mut self.operation {
                            operation.log_offset = operation.log_offset.saturating_sub(1);
                        }
                    }
                    KeyCode::PageUp => {
                        if let Some(operation) = &mut self.operation {
                            operation.log_offset = operation
                                .log_offset
                                .saturating_add(10)
                                .min(self.log_scroll_max);
                        }
                    }
                    KeyCode::PageDown => {
                        if let Some(operation) = &mut self.operation {
                            operation.log_offset = operation.log_offset.saturating_sub(10);
                        }
                    }
                    KeyCode::Char('G') | KeyCode::End => {
                        if let Some(operation) = &mut self.operation {
                            operation.log_offset = 0;
                        }
                    }
                    KeyCode::Char('g') | KeyCode::Home => {
                        if let Some(operation) = &mut self.operation {
                            operation.log_offset = self.log_scroll_max;
                        }
                    }
                    _ => {}
                }
            }
            Status::Failed => match code {
                KeyCode::Char('r') => self.retry_operation(),
                KeyCode::Esc | KeyCode::Char('q') => self.back_to_dashboard(),
                _ => {}
            },
            Status::Succeeded | Status::Warnings | Status::Cancelled => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.back_to_dashboard(),
                _ => {}
            },
        }
    }

    fn handle_confirm_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let plain = !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let Some(operation) = &mut self.operation else {
            return;
        };
        match code {
            KeyCode::Enter => {
                if operation.confirm_text.trim() == "RESTORE" {
                    if operation.confirm_restore() {
                        operation.confirm_text.clear();
                        operation.notice.clear();
                    }
                } else {
                    operation.notice = "Type RESTORE exactly to continue".to_string();
                }
            }
            KeyCode::Backspace => {
                operation.confirm_text.pop();
            }
            KeyCode::Esc => {
                operation.request_cancel();
            }
            KeyCode::Char('q' | 'c') if plain => {
                operation.request_cancel();
            }
            KeyCode::Char('d') if plain => {
                operation.diff_open = true;
                operation.diff_scroll = 0;
            }
            KeyCode::Char(ch) if plain && !ch.is_control() && operation.confirm_text.len() < 32 => {
                operation.confirm_text.push(ch);
            }
            _ => {}
        }
    }

    fn handle_diff_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let plain = !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let Some(operation) = &mut self.operation else {
            return;
        };
        if operation.diff_filtering {
            match code {
                KeyCode::Esc | KeyCode::Enter => operation.diff_filtering = false,
                KeyCode::Backspace => {
                    operation.diff_filter.pop();
                }
                KeyCode::Char(ch) if plain && !ch.is_control() => operation.diff_filter.push(ch),
                _ => {}
            }
            operation.diff_scroll = 0;
            return;
        }
        match code {
            KeyCode::Esc => {
                operation.diff_open = false;
                operation.diff_filtering = false;
            }
            KeyCode::Char('q' | 'c') if plain => {
                operation.request_cancel();
            }
            KeyCode::Char('d') if plain => {
                operation.diff_open = false;
                operation.diff_filtering = false;
            }
            KeyCode::Char('/') => {
                operation.diff_filtering = true;
                operation.diff_filter.clear();
                operation.diff_scroll = 0;
            }
            KeyCode::Char('1') => {
                operation.diff_section = 0;
                operation.diff_scroll = 0;
            }
            KeyCode::Char('2') => {
                operation.diff_section = 1;
                operation.diff_scroll = 0;
            }
            KeyCode::Char('3') => {
                operation.diff_section = 2;
                operation.diff_scroll = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                operation.diff_scroll = operation
                    .diff_scroll
                    .saturating_add(1)
                    .min(self.diff_scroll_max);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                operation.diff_scroll = operation.diff_scroll.saturating_sub(1);
            }
            KeyCode::PageUp => {
                operation.diff_scroll = operation
                    .diff_scroll
                    .saturating_add(10)
                    .min(self.diff_scroll_max);
            }
            KeyCode::PageDown => operation.diff_scroll = operation.diff_scroll.saturating_sub(10),
            KeyCode::Char('G') | KeyCode::End => operation.diff_scroll = 0,
            KeyCode::Char('g') | KeyCode::Home => operation.diff_scroll = self.diff_scroll_max,
            _ => {}
        }
    }

    fn notice_operation(&mut self, message: &str) {
        if let Some(operation) = &mut self.operation {
            operation.notice = message.to_string();
        }
    }

    fn start_operation(&mut self, kind: Kind, name: &str, args: Vec<String>) {
        if self.caps.known && !self.caps.events {
            self.status =
                "installed termux-sync is too old for the TUI (missing --json-events). Update it."
                    .to_string();
            return;
        }
        self.close_panels();
        match Operation::start(kind, name, args, self.reduce_motion) {
            Ok(operation) => {
                self.operation = Some(operation);
                self.screen = Screen::Operation;
                self.op_result_handled = false;
                self.status = match kind {
                    Kind::Backup => "backup running".to_string(),
                    Kind::Restore => "restore plan".to_string(),
                };
            }
            Err(error) => {
                self.status = format!("cannot start termux-sync: {error}");
            }
        }
    }

    fn retry_operation(&mut self) {
        let retry = self
            .operation
            .as_ref()
            .map(|operation| (operation.kind, operation.retry_args()));
        if let Some((kind, args)) = retry {
            let name = self
                .operation
                .as_ref()
                .map_or_default(|operation| operation.name.clone());
            self.start_operation(kind, &name, args);
        }
    }

    fn back_to_dashboard(&mut self) {
        if let Some(operation) = &self.operation {
            self.status = match operation.status {
                Status::Failed => "operation failed".to_string(),
                Status::Cancelled => "operation cancelled".to_string(),
                Status::Warnings => "completed with errors".to_string(),
                _ => "operation complete".to_string(),
            };
        }
        self.operation = None;
        self.screen = Screen::Dashboard;
        self.op_result_handled = false;
    }

    /// Drains operation events, records history once, and returns whether state changed.
    pub fn poll_operation(&mut self) -> bool {
        let Some(operation) = &mut self.operation else {
            return false;
        };
        let changed = operation.poll();
        let finished = operation.is_done() && !self.op_result_handled;
        let success_name =
            if finished && matches!(operation.status, Status::Succeeded | Status::Warnings) {
                Some(if operation.name.is_empty() {
                    operation
                        .result
                        .as_ref()
                        .map_or_default(|result| result.name.clone())
                } else {
                    operation.name.clone()
                })
            } else {
                None
            };

        if finished {
            self.op_result_handled = true;
            let result = operation.result.as_ref();
            let kind = match operation.kind {
                Kind::Backup => "backup",
                Kind::Restore => "restore",
            };
            let status = match operation.status {
                Status::Succeeded => "succeeded",
                Status::Warnings => "warnings",
                Status::Failed => "failed",
                Status::Cancelled => "cancelled",
                _ => "unknown",
            };
            let entry = HistoryEntry {
                kind: kind.to_string(),
                name: operation.name.clone(),
                status: status.to_string(),
                size: result.map_or(0, |result| result.size),
                duration_ms: operation.elapsed().as_millis() as u64,
                verified: result.and_then(|result| result.verified),
                at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |duration| duration.as_secs()),
            };
            self.history.insert(0, entry);
            self.history.truncate(20);
        }
        if let Some(name) = success_name {
            if !name.is_empty() {
                self.pending_select = Some(name);
            }
            self.refresh();
        }
        // Save after the refresh so a write failure stays visible in the footer.
        if finished && let Err(error) = HistoryStore::save(&data::history_path(), &mut self.history)
        {
            self.status = format!("history save failed: {error}");
        }
        changed
    }

    /// Routes wheel movement to the active overlay or the pane under the pointer.
    pub fn handle_scroll(&mut self, column: u16, row: u16, up: bool) {
        if self.pending_confirm.is_some()
            || self.pending_input.is_some()
            || self.pending_handoff.is_some()
            || self.show_history
            || self.show_status
            || self.show_daemon
        {
            return;
        }
        if let Some(operation) = &mut self.operation {
            if operation.status == Status::AwaitingConfirm && operation.diff_open {
                operation.diff_scroll = if up {
                    operation.diff_scroll.saturating_add(3)
                } else {
                    operation.diff_scroll.saturating_sub(3)
                };
            } else if operation.is_running() {
                operation.log_offset = if up {
                    operation.log_offset.saturating_add(3)
                } else {
                    operation.log_offset.saturating_sub(3)
                };
            }
            return;
        }
        if self.pending.is_some() || self.task.is_some() || self.show_help {
            return;
        }
        let point = Position::new(column, row);
        if self.actions_area.contains(point) {
            self.move_action(if up { -1 } else { 1 });
            self.switch_focus(Focus::Actions);
        } else if self.backups_area.contains(point) {
            self.move_backup(if up { -1 } else { 1 });
            self.switch_focus(Focus::Backups);
        } else if self.output_area.contains(point) {
            self.output_offset = if up {
                self.output_offset.saturating_add(3)
            } else {
                self.output_offset.saturating_sub(3)
            };
            self.switch_focus(Focus::Output);
        }
    }

    /// Routes a left click to overlays, then to the pane under the pointer.
    pub fn handle_mouse(&mut self, column: u16, row: u16) {
        let point = Position::new(column, row);
        if self.show_status || self.show_daemon || self.show_history {
            return;
        }
        if self.pending_handoff.is_some() {
            if self.handoff_yes.contains(point) {
                if let Some(handoff) = self.pending_handoff.take() {
                    match handoff.action {
                        Action::Setup => self.queue_command(vec!["setup"]),
                        Action::Schedule => self.queue_command(vec!["schedule"]),
                        _ => {}
                    }
                }
            } else if self.handoff_no.contains(point) {
                self.pending_handoff = None;
                self.status = "cancelled".to_string();
            }
            return;
        }
        if self.pending_input.is_some() {
            let Some(prompt) = self.pending_input else {
                return;
            };
            if self.input_yes.contains(point) {
                self.submit_input(prompt);
            } else if self.input_no.contains(point) {
                self.pending_input = None;
                self.input_text.clear();
                self.status = prompt.cancel_status().to_string();
            }
            return;
        }
        if let Some(action) = self.pending_confirm {
            if self.confirm_yes.contains(point) {
                self.pending_confirm = None;
                self.execute_confirmed_action(action);
            } else if self.confirm_no.contains(point) {
                self.pending_confirm = None;
                self.status = "cancelled".to_string();
            }
            return;
        }
        if self.mouse_blocked() {
            return;
        }

        if self.mouse_header(point) {
            return;
        }
        if self.mouse_actions(point, row) {
            return;
        }
        if self.mouse_backups(point, row) {
            return;
        }
        self.mouse_output(point);
    }

    /// An operation or an in-flight job owns every click while it is open.
    fn mouse_blocked(&self) -> bool {
        self.operation.is_some() || self.pending.is_some() || self.task.is_some() || self.show_help
    }

    /// Header and stat tiles. Returns whether the click landed on one.
    fn mouse_header(&mut self, point: Position) -> bool {
        if self.header_areas.warning.contains(point) {
            self.run(Action::Logs);
            return true;
        }
        if self.header_areas.count.contains(point) {
            self.switch_focus(Focus::Backups);
            return true;
        }
        if self.header_areas.status.contains(point) {
            self.run(Action::Logs);
            return true;
        }
        for (index, area) in self.stat_areas.iter().enumerate() {
            if area.contains(point) {
                if index == 3 {
                    self.switch_focus(Focus::Actions);
                    self.status = format!("AUTO: {}", self.snapshot.config.schedule);
                } else {
                    self.switch_focus(Focus::Backups);
                }
                return true;
            }
        }
        false
    }

    /// Actions pane: click a row to run it, or the pane chrome to focus it.
    fn mouse_actions(&mut self, point: Position, row: u16) -> bool {
        if !self.actions_area.contains(point) {
            return false;
        }
        let first_row = self.actions_area.y.saturating_add(1);
        let last_row = self
            .actions_area
            .y
            .saturating_add(self.actions_area.height)
            .saturating_sub(1);
        if row >= first_row && row < last_row {
            let index = (row - first_row) as usize + self.action_state.offset();
            if index < ACTIONS.len() {
                self.action_state.select(Some(index));
                self.switch_focus(Focus::Actions);
                self.run(ACTIONS[index].action);
            }
        } else {
            self.switch_focus(Focus::Actions);
        }
        true
    }

    /// Backups pane: focus it and select the clicked row when it hits the list.
    fn mouse_backups(&mut self, point: Position, row: u16) -> bool {
        if !self.backups_area.contains(point) {
            return false;
        }
        self.switch_focus(Focus::Backups);
        let first_data = self.backups_area.y.saturating_add(2);
        if row >= first_data {
            let index = (row - first_data) as usize + self.backup_state.offset();
            if index < self.filtered_backup_indices().len() {
                self.backup_state.select(Some(index));
            }
        }
        true
    }

    /// Output pane: focus it when clicked.
    fn mouse_output(&mut self, point: Position) -> bool {
        if !self.output_area.contains(point) {
            return false;
        }
        self.switch_focus(Focus::Output);
        true
    }

    fn handle_actions_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_action(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_action(1),
            KeyCode::Enter => self.activate_selected_action(),
            _ => {}
        }
    }

    fn handle_backups_key(&mut self, code: KeyCode) {
        if self.backup_filtering {
            match code {
                KeyCode::Esc => {
                    self.backup_filtering = false;
                    self.backup_filter.clear();
                    self.select_first_filtered_backup();
                }
                KeyCode::Enter => self.backup_filtering = false,
                KeyCode::Backspace => {
                    self.backup_filter.pop();
                    self.select_first_filtered_backup();
                }
                KeyCode::Char(ch) if !ch.is_control() => {
                    self.backup_filter.push(ch);
                    self.select_first_filtered_backup();
                }
                _ => {}
            }
            return;
        }
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_backup(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_backup(1),
            KeyCode::Enter => self.run(Action::Restore),
            KeyCode::Char('d') => self.run(Action::Delete),
            KeyCode::Char('/') => {
                self.backup_filtering = true;
                self.backup_filter.clear();
                self.select_first_filtered_backup();
            }
            _ => {}
        }
    }

    fn select_first_filtered_backup(&mut self) {
        let has_any = !self.filtered_backup_indices().is_empty();
        self.backup_state = {
            let mut state = TableState::default();
            state.select(has_any.then_some(0));
            state
        };
    }

    fn handle_output_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.output_offset = self
                    .output_offset
                    .saturating_add(1)
                    .min(self.output_scroll_max);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.output_offset = self.output_offset.saturating_sub(1);
            }
            KeyCode::PageUp => {
                self.output_offset = self
                    .output_offset
                    .saturating_add(10)
                    .min(self.output_scroll_max);
            }
            KeyCode::PageDown => self.output_offset = self.output_offset.saturating_sub(10),
            KeyCode::Char('[') => self.adjust_log_lines(-200),
            KeyCode::Char(']') => self.adjust_log_lines(200),
            KeyCode::Char('g') | KeyCode::Home => self.output_offset = self.output_scroll_max,
            KeyCode::Char('G') | KeyCode::End => self.output_offset = 0,
            _ => {}
        }
    }

    fn adjust_log_lines(&mut self, delta: isize) {
        self.log_lines = stepped_log_lines(self.log_lines, delta);
        if self.output_source == OutputSource::Log {
            self.output = data::tail_log(self.log_lines);
            self.output_offset = 0;
            self.status = format!("log {} lines", self.output.len());
        } else {
            self.status = format!("log limit {} lines", self.log_lines);
        }
    }

    fn move_action(&mut self, delta: isize) {
        let len = ACTIONS.len();
        if len == 0 {
            return;
        }
        let current = self.action_state.selected().unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len as isize) as usize;
        self.action_state.select(Some(next));
    }

    fn move_backup(&mut self, delta: isize) {
        let len = self.filtered_backup_indices().len();
        if len == 0 {
            return;
        }
        let current = self.backup_state.selected().unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len as isize) as usize;
        self.backup_state.select(Some(next));
    }

    fn activate_selected_action(&mut self) {
        let index = self.action_state.selected().unwrap_or(0);
        if let Some(item) = ACTIONS.get(index) {
            self.run(item.action);
        }
    }

    /// Starts `action`, opening prompts or confirmation where required.
    pub fn run(&mut self, action: Action) {
        self.close_panels();
        match action {
            Action::Backup => {
                self.pending_input = Some(InputPrompt::BackupLabel);
                self.input_text = InputPrompt::BackupLabel.seed().to_string();
                self.status = "label this backup, or press Enter to continue".to_string();
            }
            Action::Restore => {
                if let Some(name) = self.selected_backup().map(|backup| backup.name.clone()) {
                    let args = restore_command(&name);
                    self.start_operation(Kind::Restore, &name, args);
                } else {
                    self.status = "no backup selected".to_string();
                }
            }
            Action::Delete => {
                if self.selected_backup().is_some() {
                    self.pending_confirm = Some(Action::Delete);
                } else {
                    self.status = "no backup selected".to_string();
                }
            }
            Action::List => self.start_task(Task::ListBackups),
            Action::Logs => {
                self.output_source = OutputSource::Log;
                self.output = data::tail_log(self.log_lines);
                self.output_offset = 0;
                self.switch_focus(Focus::Output);
                self.status = format!("showing sync.log · {} lines", self.output.len());
            }
            Action::ClearLogs => self.pending_confirm = Some(Action::ClearLogs),
            Action::Status => self.start_task(Task::Status(self.snapshot.config.clone())),
            Action::DiskCheck => {
                self.pending_input = Some(InputPrompt::CheckTarget);
                self.input_text = InputPrompt::CheckTarget.seed().to_string();
                self.status = "blank checks all paths".to_string();
            }
            Action::ExportConfig => {
                self.pending_input = Some(InputPrompt::ExportConfig);
                self.input_text = InputPrompt::ExportConfig.seed().to_string();
                self.status = "choose where to save the exported config".to_string();
            }
            Action::ImportConfig => {
                self.pending_input = Some(InputPrompt::ImportConfig);
                self.input_text = InputPrompt::ImportConfig.seed().to_string();
                self.status = "enter the path to a config export".to_string();
            }
            Action::Setup => {
                self.pending_handoff = Some(HandoffInfo {
                    action: Action::Setup,
                    title: "Setup",
                    detail: "The setup wizard asks for storage, tokens and paths.",
                });
            }
            Action::Schedule => {
                self.pending_handoff = Some(HandoffInfo {
                    action: Action::Schedule,
                    title: "Auto-backup schedule",
                    detail: "The scheduler asks for a time and save location.",
                });
            }
            Action::Daemon => self.open_daemon(),
            Action::ClearCache => self.pending_confirm = Some(Action::ClearCache),
            Action::Refresh => self.refresh(),
            Action::Help => self.open_help(),
            Action::Quit => self.request_quit(),
        }
    }

    fn execute_confirmed_action(&mut self, action: Action) {
        // Unknown capabilities stay optimistic; the probe only ever removes a feature.
        let yes_supported = !self.caps.known || self.caps.yes;
        self.execute_confirmed_action_with(action, yes_supported);
    }

    fn execute_confirmed_action_with(&mut self, action: Action, yes_supported: bool) {
        match action {
            Action::Delete => {
                if let Some(name) = self.selected_backup().map(|backup| backup.name.clone()) {
                    if yes_supported {
                        self.start_task(Task::Command(vec![
                            "delete".to_string(),
                            "--name".to_string(),
                            name,
                            "--yes".to_string(),
                        ]));
                    } else {
                        self.queue_command(vec!["delete", "--name", &name]);
                        self.status =
                            "termux-sync is too old for in-TUI delete; using CLI confirmation"
                                .to_string();
                    }
                }
            }
            Action::ClearCache => {
                if yes_supported {
                    self.start_task(Task::Command(vec![
                        "clear-cache".to_string(),
                        "--yes".to_string(),
                    ]));
                } else {
                    self.queue_command(vec!["clear-cache"]);
                    self.status =
                        "termux-sync is too old for in-TUI clear-cache; using CLI confirmation"
                            .to_string();
                }
            }
            Action::ClearLogs => {
                if yes_supported {
                    self.start_task(Task::Command(clear_logs_command(true)));
                } else {
                    self.queue_command(vec!["clear-logs"]);
                    self.status =
                        "termux-sync is too old for in-TUI clear-logs; using CLI confirmation"
                            .to_string();
                }
            }
            _ => {}
        }
    }

    fn queue_command(&mut self, args: Vec<&str>) {
        let argv: Vec<String> = args.into_iter().map(str::to_string).collect();
        self.status = format!("running termux-sync {}", argv.join(" "));
        self.pending = Some(argv);
    }

    fn start_task(&mut self, task: Task) {
        if self.worker.is_busy() {
            self.status = "background task already running".to_string();
            return;
        }
        let label = match &task {
            Task::Refresh => "refreshing".to_string(),
            Task::Command(args) => format!("termux-sync {}", args.join(" ")),
            Task::ListBackups => "listing backups".to_string(),
            Task::Status(_) => "collecting status".to_string(),
        };
        let show_overlay = matches!(&task, Task::Command(_));
        if self.worker.start(task) {
            self.status.clone_from(&label);
            if show_overlay {
                self.task = Some(TaskOverlay {
                    label,
                    started_at: self.tick,
                    done: false,
                    done_at: 0,
                    visible: false,
                });
            }
        }
    }

    /// Reloads backups and status in the background.
    pub fn refresh(&mut self) {
        if self.snapshot.config.storage != "local" && self.caps.events {
            self.start_task(Task::ListBackups);
        } else {
            self.start_task(Task::Refresh);
        }
    }

    /// Applies a finished worker result and returns whether state changed.
    pub fn poll_worker(&mut self) -> bool {
        let Some(result) = self.worker.poll() else {
            return false;
        };
        let hide_fast_task = self.task.as_ref().is_some_and(|task| !task.visible);
        if hide_fast_task {
            self.task = None;
        } else if let Some(task) = &mut self.task {
            task.done = true;
            task.done_at = self.tick;
        }
        match result {
            Ok(WorkerMsg::Refreshed(snapshot)) => self.apply_snapshot(snapshot),
            Ok(WorkerMsg::Backups(backups)) => self.apply_backup_list(backups),
            Ok(WorkerMsg::Status(report)) => {
                if self.operation.is_some() {
                    // Never stack an overlay on the operation screen; `s` re-collects.
                    self.status = "status ready — press s after the operation".to_string();
                } else {
                    self.open_status_report(report);
                    self.status = "status ready".to_string();
                }
            }
            Ok(WorkerMsg::Output { title, text }) => {
                self.output_source = OutputSource::Command;
                self.output.clear();
                self.output.push(format!("$ termux-sync {title}"));
                let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
                if lines.len() > crate::tui::ops::LOG_LIMIT {
                    lines = lines.split_off(lines.len() - crate::tui::ops::LOG_LIMIT);
                }
                self.output.extend(lines);
                self.output_offset = 0;
                self.switch_focus(Focus::Output);
                self.status = format!("termux-sync {title} finished");
                if title.starts_with("clear-logs") {
                    self.preserve_output = true;
                }
                if should_refresh_after(&title) {
                    self.refresh();
                }
            }
            Err(err) => self.apply_worker_error(err),
        }
        true
    }

    /// Shows a failed background command and clears the initial loading state.
    fn apply_worker_error(&mut self, err: String) {
        self.loading = false;
        self.output_source = OutputSource::Command;
        self.output.clear();
        self.output.extend(err.lines().map(str::to_string));
        self.output_offset = 0;
        self.switch_focus(Focus::Output);
        self.status = "command failed".to_string();
    }

    /// Replaces the backup list and reselects the first row.
    pub fn apply_backup_list(&mut self, backups: Vec<Backup>) {
        let count = backups.len();
        self.loading = false;
        self.snapshot.backups = backups;
        self.backups_revision += 1;
        self.snapshot.warning = Vec::new();
        self.backup_filter.clear();
        self.backup_filtering = false;
        self.backup_state.select((count > 0).then_some(0));
        self.switch_focus(Focus::Backups);
        self.status = format!("{count} backup(s)");
        self.op_result_handled = false;
    }

    /// Replaces the snapshot, keeping the previous selection when possible.
    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        let selected_name = self
            .pending_select
            .take()
            .or_else(|| self.selected_backup().map(|backup| backup.name.clone()));
        let keep_output = std::mem::take(&mut self.preserve_output);

        self.loading = false;
        if !keep_output {
            self.status = snapshot
                .warning_text()
                .unwrap_or_else(|| "refreshed".to_string());
        }
        self.snapshot = snapshot;
        self.backups_revision += 1;
        if !keep_output {
            self.output = data::tail_log(self.log_lines);
            self.output_source = OutputSource::Log;
            self.output_offset = 0;
        }

        if self.snapshot.backups.is_empty() {
            self.backup_state.select(None);
        } else {
            let full_index = selected_name
                .and_then(|name| {
                    self.snapshot
                        .backups
                        .iter()
                        .position(|backup| backup.name == name)
                })
                .unwrap_or(0);
            if !self.filtered_backup_indices().contains(&full_index) {
                self.backup_filter.clear();
            }
            let position = self
                .filtered_backup_indices()
                .iter()
                .position(|index| *index == full_index)
                .unwrap_or(0);
            self.backup_state.select(Some(position));
        }
    }
}

/// Builds a retry command for a history entry; `None` for kinds that cannot be retried.
#[must_use]
pub fn history_command(entry: &HistoryEntry) -> Option<(Kind, String, Vec<String>)> {
    match entry.kind.as_str() {
        "backup" => Some((Kind::Backup, "backup".to_string(), backup_command(""))),
        "restore" => Some((
            Kind::Restore,
            entry.name.clone(),
            restore_command(&entry.name),
        )),
        _ => None,
    }
}

/// CLI arguments for a backup, omitting an empty label.
#[must_use]
pub fn backup_command(label: &str) -> Vec<String> {
    let label = label.trim();
    if label.is_empty() {
        vec!["backup".to_string()]
    } else {
        vec![
            "backup".to_string(),
            "--label".to_string(),
            label.to_string(),
        ]
    }
}

/// CLI arguments for restoring `name`.
#[must_use]
pub fn restore_command(name: &str) -> Vec<String> {
    vec![
        "restore".to_string(),
        "--name".to_string(),
        name.to_string(),
    ]
}

/// CLI arguments for exporting config to `path`.
#[must_use]
pub fn export_config_command(path: &str) -> Vec<String> {
    path_command("export-config", path)
}

/// CLI arguments for importing config from `path`.
#[must_use]
pub fn import_config_command(path: &str) -> Vec<String> {
    path_command("import-config", path)
}

fn path_command(command: &str, path: &str) -> Vec<String> {
    let mut args = vec![command.to_string()];
    if path.starts_with('-') {
        args.push("--".to_string());
    }
    args.push(path.to_string());
    args
}

/// CLI arguments for a disk check; a blank target checks all paths.
#[must_use]
pub fn check_command(target: &str) -> Vec<String> {
    let target = target.trim();
    if target.is_empty() {
        vec!["check".to_string()]
    } else {
        path_command("check", target)
    }
}

/// CLI arguments for clearing logs, adding `--yes` when supported.
#[must_use]
pub fn clear_logs_command(yes_supported: bool) -> Vec<String> {
    let mut args = vec!["clear-logs".to_string()];
    if yes_supported {
        args.push("--yes".to_string());
    }
    args
}

/// Whether a finished command changes data shown on the dashboard.
#[must_use]
pub fn should_refresh_after(title: &str) -> bool {
    ["delete", "clear-cache", "clear-logs", "import-config"]
        .iter()
        .any(|prefix| title.starts_with(prefix))
}

/// Maps a prompt and its text to CLI arguments; `None` when required text is empty.
#[must_use]
pub fn input_command(prompt: InputPrompt, text: &str) -> Option<Vec<String>> {
    let value = text.trim();
    match prompt {
        InputPrompt::BackupLabel => None,
        InputPrompt::ExportConfig => (!value.is_empty()).then(|| export_config_command(value)),
        InputPrompt::ImportConfig => (!value.is_empty()).then(|| import_config_command(value)),
        InputPrompt::CheckTarget => Some(check_command(value)),
    }
}

/// Clamps the log line count to `40..=2000` after adding `delta`.
#[must_use]
pub fn stepped_log_lines(current: usize, delta: isize) -> usize {
    let next = (current as isize + delta).clamp(40, 2000);
    next as usize
}

fn progress(tick: u64, start: u64, ticks: u64) -> f32 {
    if ticks == 0 {
        1.0
    } else {
        (tick.saturating_sub(start) as f32 / ticks as f32).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::data::Config;
    use crate::tui::ops::{Kind, Status, test_operation, test_plan};

    fn snapshot() -> Snapshot {
        Snapshot {
            config: Config::default(),
            backups: vec![
                Backup {
                    name: "backup-one".to_string(),
                    date: "2026-01-01 12:00".to_string(),
                    label: String::new(),
                    packages: 2,
                    size: 1024,
                    version: "1.3.9".to_string(),
                    compression: "GZ".to_string(),
                    modified: None,
                },
                Backup {
                    name: "backup-two".to_string(),
                    date: "2026-01-02 12:00".to_string(),
                    label: "before-upgrade".to_string(),
                    packages: 3,
                    size: 2048,
                    version: "1.3.9".to_string(),
                    compression: "GZ".to_string(),
                    modified: None,
                },
            ],
            log: vec!["ready".to_string()],
            warning: Vec::new(),
        }
    }

    #[test]
    fn navigation_wraps_both_directions() {
        let mut app = App::with_snapshot(snapshot());
        app.handle_key(KeyCode::Up);
        assert_eq!(app.action_state.selected(), Some(ACTIONS.len() - 1));
        app.handle_key(KeyCode::Down);
        assert_eq!(app.action_state.selected(), Some(0));
    }

    #[test]
    fn tab_cycles_focus() {
        let mut app = App::with_snapshot(snapshot());
        assert_eq!(app.focus, Focus::Actions);
        app.handle_key(KeyCode::Tab);
        assert_eq!(app.focus, Focus::Backups);
        app.handle_key(KeyCode::BackTab);
        assert_eq!(app.focus, Focus::Actions);
    }

    #[test]
    fn restore_command_uses_backup_name() {
        assert_eq!(
            restore_command("backup-two"),
            vec![
                "restore".to_string(),
                "--name".to_string(),
                "backup-two".to_string()
            ]
        );
    }

    #[test]
    fn delete_without_backup_reports_status() {
        let mut snapshot = snapshot();
        snapshot.backups.clear();
        let mut app = App::with_snapshot(snapshot);
        app.run(Action::Delete);
        assert!(app.pending.is_none());
        assert_eq!(app.status, "no backup selected");
    }

    #[test]
    fn quit_sets_flag() {
        let mut app = App::with_snapshot(snapshot());
        app.handle_key(KeyCode::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn quit_is_refused_while_a_background_command_runs() {
        let mut app = App::with_snapshot(snapshot());
        app.start_task(Task::Command(vec!["check".to_string()]));
        app.handle_key(KeyCode::Char('q'));
        assert!(!app.should_quit);
        assert!(app.status.contains("still running"));
    }

    #[test]
    fn quit_is_allowed_while_only_a_refresh_runs() {
        let mut app = App::with_snapshot(snapshot());
        app.start_task(Task::Refresh);
        app.handle_key(KeyCode::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn quit_action_is_refused_while_a_background_command_runs() {
        let mut app = App::with_snapshot(snapshot());
        app.start_task(Task::Command(vec!["check".to_string()]));
        app.run(Action::Quit);
        assert!(!app.should_quit);
    }

    #[test]
    fn opening_status_closes_help() {
        let mut app = App::with_snapshot(snapshot());
        app.open_help();
        app.open_status_report(status_report());
        assert!(!app.show_help);
        assert!(app.show_status);
    }

    #[test]
    fn opening_daemon_closes_status() {
        let mut app = App::with_snapshot(snapshot());
        app.open_status_report(status_report());
        app.open_daemon();
        assert!(!app.show_status);
        assert!(app.show_daemon);
    }

    #[test]
    fn keys_follow_the_topmost_overlay() {
        let mut app = App::with_snapshot(snapshot());
        app.status_scroll_max = 3;
        app.show_help = true;
        app.show_status = true;
        app.handle_key(KeyCode::Down);
        assert_eq!(app.status_scroll, 1);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn help_scroll_stops_at_the_rendered_limit() {
        let mut app = App::with_snapshot(snapshot());
        app.help_scroll_max = 3;
        app.open_help();
        for _ in 0..10 {
            app.handle_key(KeyCode::Down);
        }
        assert_eq!(app.help_scroll, 3);
        app.handle_key(KeyCode::Up);
        assert_eq!(app.help_scroll, 2);
    }

    #[test]
    fn output_scroll_stops_at_the_rendered_limit() {
        let mut app = App::with_snapshot(snapshot());
        app.output_scroll_max = 3;
        app.focus = Focus::Output;
        for _ in 0..10 {
            app.handle_key(KeyCode::Up);
        }
        assert_eq!(app.output_offset, 3);
        app.handle_key(KeyCode::Down);
        assert_eq!(app.output_offset, 2);
    }

    #[test]
    fn operation_log_scroll_stops_at_the_rendered_limit() {
        let mut app = App::with_snapshot(snapshot());
        app.log_scroll_max = 2;
        app.operation = Some(test_operation(Kind::Backup));
        app.screen = Screen::Operation;
        for _ in 0..10 {
            app.handle_key(KeyCode::Up);
        }
        assert_eq!(app.operation.as_ref().unwrap().log_offset, 2);
    }

    #[test]
    fn diff_scroll_stops_at_the_rendered_limit() {
        let mut app = App::with_snapshot(snapshot());
        app.diff_scroll_max = 2;
        let mut operation = test_operation(Kind::Restore);
        operation.status = Status::AwaitingConfirm;
        operation.plan = Some(test_plan());
        operation.diff_open = true;
        app.operation = Some(operation);
        app.screen = Screen::Operation;
        for _ in 0..10 {
            app.handle_key(KeyCode::Up);
        }
        assert_eq!(app.operation.as_ref().unwrap().diff_scroll, 2);
    }

    #[test]
    fn backup_list_clears_the_loading_state() {
        let mut app = App::with_snapshot(snapshot());
        app.loading = true;
        app.apply_backup_list(Vec::new());
        assert!(!app.loading);
    }

    #[test]
    fn worker_error_clears_the_loading_state() {
        let mut app = App::with_snapshot(snapshot());
        app.loading = true;
        app.apply_worker_error("boom".to_string());
        assert!(!app.loading);
        assert_eq!(app.status, "command failed");
    }

    #[test]
    fn apply_layout_clamps_scroll_positions() {
        let mut app = App::with_snapshot(snapshot());
        app.output_offset = 50;
        app.help_scroll = 50;
        app.apply_layout(crate::tui::ui::UiLayout {
            output_scroll_max: 2,
            help_scroll_max: 1,
            ..crate::tui::ui::UiLayout::default()
        });
        assert_eq!(app.output_offset, 2);
        assert_eq!(app.help_scroll, 1);
        assert_eq!(app.output_scroll_max, 2);
        assert_eq!(app.help_scroll_max, 1);
    }

    #[test]
    fn operation_layout_keeps_the_dashboard_scroll() {
        let mut app = App::with_snapshot(snapshot());
        app.output_offset = 3;
        app.screen = Screen::Operation;
        app.operation = Some(test_operation(Kind::Backup));
        app.apply_layout(crate::tui::ui::UiLayout::default());
        assert_eq!(app.output_offset, 3);
    }

    #[test]
    fn status_result_does_not_open_over_an_operation() {
        let mut app = App::with_snapshot(snapshot());
        app.operation = Some(test_operation(Kind::Backup));
        app.screen = Screen::Operation;
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(WorkerMsg::Status(status_report()))).unwrap();
        app.worker = Worker {
            rx: Some(rx),
            busy: true,
            writes: false,
        };
        assert!(app.poll_worker());
        assert!(!app.show_status);
        assert!(app.status.contains("after the operation"));
    }

    #[test]
    fn history_overlay_swallows_clicks() {
        let mut app = App::with_snapshot(snapshot());
        app.actions_area = Rect::new(0, 0, 26, 20);
        app.show_history = true;
        let help_row = ACTIONS
            .iter()
            .position(|item| item.action == Action::Help)
            .unwrap() as u16
            + 1;
        app.handle_mouse(2, help_row);
        assert!(!app.show_help);
    }

    fn status_report() -> data::StatusReport {
        data::StatusReport {
            storage: "local".into(),
            target: "~/termux-backups".into(),
            compression: "gz".into(),
            max_backups: 5,
            schedule: "off".into(),
            cli_version: None,
            config_path: "~/.config/termux-sync/config.json".into(),
            log_path: "~/.config/termux-sync/sync.log".into(),
            history_path: "~/.config/termux-sync/tui-history.json".into(),
            tools: Vec::new(),
        }
    }

    #[test]
    fn refresh_keeps_selected_backup() {
        let mut app = App::with_snapshot(snapshot());
        app.backup_state.select(Some(1));
        let mut refreshed = snapshot();
        refreshed.backups.reverse();
        app.apply_snapshot(refreshed);
        assert_eq!(
            app.selected_backup().map(|backup| backup.name.as_str()),
            Some("backup-two")
        );
    }

    #[test]
    fn tick_advances_without_panicking() {
        let mut app = App::with_snapshot(snapshot());
        app.on_tick();
        app.on_tick();
        assert_eq!(app.tick, 2);
        assert!(app.spinner().is_none());
    }

    #[test]
    fn handoff_is_queued_after_confirmation() {
        let mut app = App::with_snapshot(snapshot());
        app.run(Action::Setup);
        assert!(app.pending_handoff.is_some());
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.pending, Some(vec!["setup".to_string()]));
    }

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
        assert!(app.status.starts_with("showing sync.log · "));
        assert!(app.status.ends_with(" lines"));
    }

    #[test]
    fn scroll_moves_the_action_selection() {
        let mut app = App::with_snapshot(snapshot());
        app.actions_area = Rect::new(0, 0, 26, 20);
        app.handle_scroll(2, 5, true);
        assert_eq!(app.action_state.selected(), Some(ACTIONS.len() - 1));
        app.handle_scroll(2, 5, false);
        assert_eq!(app.action_state.selected(), Some(0));
    }

    #[test]
    fn delete_and_clear_cache_confirm_inside_the_tui() {
        let mut app = App::with_snapshot(snapshot());
        app.run(Action::Delete);
        assert_eq!(app.pending_confirm, Some(Action::Delete));
        app.pending_confirm = None;

        app.run(Action::ClearCache);
        assert_eq!(app.pending_confirm, Some(Action::ClearCache));
    }

    #[test]
    fn setup_action_opens_handoff_card_instead_of_prompting_inline() {
        let mut app = App::with_snapshot(snapshot());
        app.run(Action::Setup);
        assert!(app.pending_handoff.is_some());
        assert!(app.pending.is_none());
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.pending, Some(vec!["setup".to_string()]));
    }

    #[test]
    fn backup_filter_matches_name_and_label() {
        let mut app = App::with_snapshot(snapshot());
        app.backup_filter = "upgrade".into();
        assert_eq!(&*app.filtered_backup_indices(), &[1]);
        app.backup_filter = "backup-one".into();
        assert_eq!(&*app.filtered_backup_indices(), &[0]);
    }

    #[test]
    fn filter_keeps_global_shortcuts_out() {
        let mut app = App::with_snapshot(snapshot());
        app.focus = Focus::Backups;
        app.handle_key(KeyCode::Char('/'));
        for ch in "hRqL".chars() {
            app.handle_key(KeyCode::Char(ch));
        }
        assert_eq!(app.backup_filter, "hRqL");
        assert!(!app.show_history);
        assert!(!app.should_quit);
    }

    #[test]
    fn backup_selection_maps_through_filter() {
        let mut app = App::with_snapshot(snapshot());
        app.backup_filter = "upgrade".into();
        app.backup_state.select(Some(0));
        assert_eq!(
            app.selected_backup().map(|backup| backup.name.as_str()),
            Some("backup-two")
        );
    }

    #[test]
    fn ctrl_c_cancels_backup_without_quitting() {
        let mut app = App::with_snapshot(snapshot());
        app.operation = Some(test_operation(Kind::Backup));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(app.operation.as_ref().unwrap().status, Status::Cancelling);
        assert!(!app.should_quit);
    }

    #[test]
    fn ctrl_c_refuses_protected_restore() {
        let mut app = App::with_snapshot(snapshot());
        let mut operation = test_operation(Kind::Restore);
        operation.protected = true;
        app.operation = Some(operation);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.should_quit);
        assert!(
            app.operation
                .as_ref()
                .unwrap()
                .notice
                .contains("cannot cancel")
        );
    }

    #[test]
    fn reduced_motion_disables_every_spinner() {
        let mut app = App::with_snapshot(snapshot());
        app.operation = Some(test_operation(Kind::Backup));
        assert!(app.spinner().is_some());
        app.reduce_motion = true;
        assert!(app.spinner().is_none());
    }

    #[test]
    fn dashboard_status_reflects_failed_operation() {
        let mut app = App::with_snapshot(snapshot());
        let mut operation = test_operation(Kind::Backup);
        operation.status = Status::Failed;
        app.operation = Some(operation);
        app.back_to_dashboard();
        assert_eq!(app.status, "operation failed");
    }

    #[test]
    fn history_retry_maps_kind_to_command() {
        let restore = HistoryEntry {
            kind: "restore".into(),
            name: "backup-two".into(),
            status: "succeeded".into(),
            size: 1,
            duration_ms: 1,
            verified: None,
            at: 1,
        };
        assert_eq!(
            history_command(&restore),
            Some((
                Kind::Restore,
                "backup-two".to_string(),
                restore_command("backup-two")
            ))
        );
        let backup = HistoryEntry {
            kind: "backup".into(),
            ..restore
        };
        assert!(matches!(
            history_command(&backup),
            Some((Kind::Backup, _, _))
        ));
    }

    #[test]
    fn backup_command_includes_clean_label() {
        assert_eq!(
            backup_command("before-upgrade"),
            vec!["backup", "--label", "before-upgrade"]
        );
        assert_eq!(backup_command(""), vec!["backup"]);
    }

    #[test]
    fn backup_action_opens_label_prompt() {
        let mut app = App::with_snapshot(snapshot());
        app.run(Action::Backup);
        assert_eq!(app.pending_input, Some(InputPrompt::BackupLabel));
        assert!(app.operation.is_none());
    }

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
        assert_eq!(app.input_text, "");
        app.handle_key(KeyCode::Esc);

        app.run(Action::DiskCheck);
        assert_eq!(app.pending_input, Some(InputPrompt::CheckTarget));
        assert_eq!(app.input_text, "");
    }

    #[test]
    fn confirmed_delete_falls_back_to_handoff_without_yes() {
        let mut app = App::with_snapshot(snapshot());
        app.execute_confirmed_action_with(Action::Delete, false);
        assert!(app.pending.as_ref().is_some_and(|args| args[0] == "delete"));
        assert!(!app.pending.unwrap().contains(&"--yes".to_string()));

        let mut app = App::with_snapshot(snapshot());
        app.execute_confirmed_action_with(Action::ClearCache, false);
        assert_eq!(app.pending, Some(vec!["clear-cache".to_string()]));
    }

    #[test]
    fn clicking_an_action_runs_it() {
        let mut app = App::with_snapshot(snapshot());
        app.actions_area = Rect::new(0, 0, 26, 20);
        let help_row = ACTIONS
            .iter()
            .position(|item| item.action == Action::Help)
            .unwrap() as u16
            + 1;
        app.handle_mouse(2, help_row);
        assert!(app.show_help);
    }

    #[test]
    fn status_report_opens_and_closes() {
        let mut app = App::with_snapshot(snapshot());
        let report = crate::tui::data::StatusReport {
            storage: "github".into(),
            target: "me/backups @ main".into(),
            compression: "xz".into(),
            max_backups: 5,
            schedule: "daily 04:05".into(),
            cli_version: Some("termux-sync v1.3.9".into()),
            config_path: "~/.config/termux-sync/config.json".into(),
            log_path: "~/.config/termux-sync/sync.log".into(),
            history_path: "~/.config/termux-sync/tui-history.json".into(),
            tools: vec![crate::tui::data::ToolStatus {
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

    #[test]
    fn ascii_action_glyphs_are_unique() {
        let mut seen = Vec::new();
        for item in ACTIONS {
            assert!(
                !seen.contains(&item.ascii),
                "duplicate ASCII glyph {}",
                item.ascii
            );
            seen.push(item.ascii);
        }
    }

    #[test]
    fn leading_dash_paths_are_escaped_before_the_cli_sees_them() {
        assert_eq!(
            export_config_command("-x"),
            vec!["export-config", "--", "-x"]
        );
        assert_eq!(
            import_config_command("--json-events"),
            vec!["import-config", "--", "--json-events"]
        );
        assert_eq!(check_command("-x"), vec!["check", "--", "-x"]);
        assert_eq!(
            export_config_command("~/export.json"),
            vec!["export-config", "~/export.json"]
        );
    }

    #[test]
    fn clear_logs_command_includes_yes_only_when_supported() {
        assert_eq!(
            clear_logs_command(true),
            vec!["clear-logs".to_string(), "--yes".to_string()]
        );
        assert_eq!(clear_logs_command(false), vec!["clear-logs".to_string()]);
    }

    #[test]
    fn refresh_follows_only_commands_that_change_the_snapshot() {
        assert!(should_refresh_after("delete --name b1 --yes"));
        assert!(should_refresh_after("clear-cache --yes"));
        assert!(should_refresh_after("clear-logs --yes"));
        assert!(should_refresh_after("import-config ~/export.json"));
        assert!(!should_refresh_after("export-config ~/export.json"));
        assert!(!should_refresh_after("check ~"));
    }

    #[test]
    fn empty_import_submit_keeps_the_prompt_open() {
        let mut app = App::with_snapshot(snapshot());
        app.run(Action::ImportConfig);
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.pending_input, Some(InputPrompt::ImportConfig));
        assert_eq!(app.status, "enter a path");
        assert!(app.pending.is_none());
    }

    #[test]
    fn modal_scroll_stops_at_the_rendered_limit() {
        let mut app = App::with_snapshot(snapshot());
        app.status_scroll_max = 3;
        app.show_status = true;
        for _ in 0..10 {
            app.handle_key(KeyCode::Down);
        }
        assert_eq!(app.status_scroll, 3);

        app.show_status = false;
        app.show_daemon = true;
        app.daemon_scroll_max = 2;
        for _ in 0..10 {
            app.handle_key(KeyCode::Down);
        }
        assert_eq!(app.daemon_scroll, 2);
    }

    #[test]
    fn apply_snapshot_preserves_clear_logs_output() {
        let mut app = App::with_snapshot(snapshot());
        app.output = vec!["Log file does not exist — nothing to clear.".to_string()];
        app.status = "termux-sync clear-logs finished".to_string();
        app.preserve_output = true;
        app.apply_snapshot(snapshot());
        assert!(
            app.output
                .iter()
                .any(|line| line.contains("nothing to clear"))
        );
        assert_eq!(app.status, "termux-sync clear-logs finished");
        assert!(!app.preserve_output);
    }

    #[test]
    fn daemon_action_opens_a_status_view() {
        let mut app = App::with_snapshot(snapshot());
        app.run(Action::Daemon);
        assert!(app.show_daemon);
        app.handle_key(KeyCode::Esc);
        assert!(!app.show_daemon);
    }

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

    #[test]
    fn clicking_a_backup_row_selects_it() {
        let mut app = App::with_snapshot(snapshot());
        app.backups_area = Rect::new(26, 0, 50, 10);
        app.handle_mouse(30, 3);
        assert_eq!(app.focus, Focus::Backups);
        assert_eq!(app.backup_state.selected(), Some(1));
    }

    #[test]
    fn task_card_waits_before_showing() {
        let mut app = App::with_snapshot(snapshot());
        app.task = Some(TaskOverlay {
            label: "termux-sync list".to_string(),
            started_at: 0,
            done: false,
            done_at: 0,
            visible: false,
        });
        app.on_tick();
        app.on_tick();
        assert!(!app.task.as_ref().unwrap().visible);
        app.on_tick();
        assert!(app.task.as_ref().unwrap().visible);
    }

    #[test]
    fn task_overlay_holds_completion_before_dismissing() {
        let mut app = App::with_snapshot(snapshot());
        app.task = Some(TaskOverlay {
            label: "termux-sync list".to_string(),
            started_at: 0,
            done: false,
            done_at: 0,
            visible: true,
        });
        for _ in 0..5 {
            app.on_tick();
        }
        assert_eq!(app.task_progress(), 1.0);

        app.task.as_mut().unwrap().done = true;
        app.task.as_mut().unwrap().done_at = app.tick;
        app.on_tick();
        app.on_tick();
        assert!(app.task.is_some());
        app.on_tick();
        assert!(app.task.is_none());
    }

    #[test]
    fn ctrl_c_quits_even_with_pending_command() {
        let mut app = App::with_snapshot(snapshot());
        app.run(Action::Setup);
        app.handle_key(KeyCode::Enter);
        assert!(app.pending.is_some());
        app.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
        assert!(app.pending.is_none());
    }

    #[test]
    fn help_scroll_moves_without_leaving_bounds() {
        let mut app = App::with_snapshot(snapshot());
        app.help_scroll_max = 10;
        app.open_help();
        app.handle_key(KeyCode::Down);
        assert_eq!(app.help_scroll, 1);
        app.handle_key(KeyCode::PageDown);
        assert_eq!(app.help_scroll, 6);
        app.handle_key(KeyCode::PageUp);
        app.handle_key(KeyCode::PageUp);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn idle_app_stops_requesting_ticks() {
        let mut app = App::with_snapshot(snapshot());
        for _ in 0..10 {
            if app.needs_tick() {
                app.on_tick();
            }
        }
        assert!(!app.needs_tick());
    }

    #[test]
    fn running_operation_asks_before_cancelling() {
        let mut app = App::with_snapshot(snapshot());
        app.operation = Some(test_operation(Kind::Backup));
        app.screen = Screen::Operation;

        app.handle_key(KeyCode::Esc);
        assert!(app.operation.as_ref().unwrap().cancel_prompt);
        assert_eq!(app.operation.as_ref().unwrap().status, Status::Running);

        app.handle_key(KeyCode::Enter);
        assert_eq!(app.operation.as_ref().unwrap().status, Status::Cancelling);
    }

    #[test]
    fn restore_plan_requires_typed_confirmation() {
        let mut app = App::with_snapshot(snapshot());
        let mut operation = test_operation(Kind::Restore);
        operation.status = Status::AwaitingConfirm;
        operation.plan = Some(test_plan());
        app.operation = Some(operation);
        app.screen = Screen::Operation;

        for ch in "RESTORE".chars() {
            app.handle_key(KeyCode::Char(ch));
        }
        let operation = app.operation.as_ref().unwrap();
        assert_eq!(operation.confirm_text, "RESTORE");

        app.handle_key(KeyCode::Enter);
        let operation = app.operation.as_ref().unwrap();
        assert!(operation.notice.contains("cannot write confirmation"));
    }

    #[test]
    fn protected_restore_refuses_cancel() {
        let mut app = App::with_snapshot(snapshot());
        let mut operation = test_operation(Kind::Restore);
        operation.protected = true;
        app.operation = Some(operation);
        app.screen = Screen::Operation;

        app.handle_key(KeyCode::Esc);
        let operation = app.operation.as_ref().unwrap();
        assert_eq!(operation.status, Status::Running);
        assert!(operation.notice.contains("cannot cancel"));
    }

    #[test]
    fn package_diff_filter_narrows_the_list() {
        let mut app = App::with_snapshot(snapshot());
        let mut operation = test_operation(Kind::Restore);
        operation.status = Status::AwaitingConfirm;
        operation.plan = Some(test_plan());
        operation
            .diff
            .apply("to_install", 1, 1, vec!["git".into(), "python".into()]);
        operation.diff.apply("already_installed", 1, 1, Vec::new());
        operation.diff.apply("device_only", 1, 1, Vec::new());
        app.operation = Some(operation);
        app.screen = Screen::Operation;

        app.handle_key(KeyCode::Char('d'));
        app.handle_key(KeyCode::Char('/'));
        for ch in "git".chars() {
            app.handle_key(KeyCode::Char(ch));
        }
        let operation = app.operation.as_ref().unwrap();
        assert!(operation.diff_open);
        assert!(operation.diff_filtering);
        assert_eq!(operation.filtered_diff(), vec!["git".to_string()]);
    }

    #[test]
    fn help_animation_completes_in_four_ticks() {
        let mut app = App::with_snapshot(snapshot());
        app.open_help();
        assert_eq!(app.help_progress(), 0.0);
        for _ in 0..4 {
            app.on_tick();
        }
        assert_eq!(app.help_progress(), 1.0);
    }

    #[test]
    fn reduced_motion_skips_help_and_task_motion() {
        let mut app = App::with_snapshot(snapshot());
        app.reduce_motion = true;
        app.open_help();
        assert_eq!(app.help_progress(), 1.0);
        app.task = Some(TaskOverlay {
            label: "termux-sync list".to_string(),
            started_at: 0,
            done: false,
            done_at: 0,
            visible: true,
        });
        assert_eq!(app.task_progress(), 1.0);
    }
}

#[cfg(test)]
mod list_apply_tests {
    use super::*;

    #[test]
    fn applying_a_list_replaces_backups_and_clears_warning() {
        let mut app = App::with_snapshot(Snapshot::default());
        app.snapshot.warning = vec!["remote".to_string()];
        app.apply_backup_list(vec![Backup {
            name: "b1".into(),
            date: "2026-01-01".into(),
            label: String::new(),
            packages: 1,
            size: 2,
            version: "1.3.9".into(),
            compression: "GZ".into(),
            modified: None,
        }]);
        assert_eq!(app.snapshot.backups.len(), 1);
        assert_eq!(app.snapshot.warning, [] as [std::string::String; 0]);
        assert_eq!(app.focus, Focus::Backups);
    }
}
