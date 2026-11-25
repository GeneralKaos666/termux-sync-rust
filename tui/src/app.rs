use std::sync::mpsc::{self, Receiver, TryRecvError};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{ListState, TableState};

use crate::data::{self, Backup, Snapshot};
use crate::history::{HistoryEntry, HistoryStore};
use crate::ops::{Kind, Operation, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    Operation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputSource {
    Log,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Actions,
    Backups,
    Output,
}

impl Focus {
    pub fn next(self) -> Self {
        match self {
            Focus::Actions => Focus::Backups,
            Focus::Backups => Focus::Output,
            Focus::Output => Focus::Actions,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Focus::Actions => Focus::Output,
            Focus::Backups => Focus::Actions,
            Focus::Output => Focus::Backups,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Focus::Actions => "Actions",
            Focus::Backups => "Backups",
            Focus::Output => "Output",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Backup,
    Restore,
    Delete,
    List,
    Logs,
    ClearLogs,
    Status,
    DiskCheck,
    ExportConfig,
    ImportConfig,
    Setup,
    Schedule,
    Daemon,
    ClearCache,
    Refresh,
    Help,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputPrompt {
    BackupLabel,
    ExportConfig,
    ImportConfig,
    CheckTarget,
}

impl InputPrompt {
    pub fn title(self) -> &'static str {
        match self {
            InputPrompt::BackupLabel => "Backup label",
            InputPrompt::ExportConfig => "Export config",
            InputPrompt::ImportConfig => "Import config",
            InputPrompt::CheckTarget => "Disk check target",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            InputPrompt::BackupLabel => "Used in the manifest and backup name.",
            InputPrompt::ExportConfig => "Tokens and passwords are redacted by the CLI.",
            InputPrompt::ImportConfig => "Path to a previously exported config file.",
            InputPrompt::CheckTarget => "Blank checks all paths. You can type a path or ~.",
        }
    }

    pub fn seed(self) -> &'static str {
        match self {
            InputPrompt::BackupLabel => "",
            InputPrompt::ExportConfig => "~/termux-sync-config-export.json",
            InputPrompt::ImportConfig => "",
            InputPrompt::CheckTarget => "",
        }
    }

    pub fn max_len(self) -> usize {
        match self {
            InputPrompt::BackupLabel => 64,
            InputPrompt::ExportConfig | InputPrompt::ImportConfig | InputPrompt::CheckTarget => 256,
        }
    }

    pub fn cancel_status(self) -> &'static str {
        match self {
            InputPrompt::BackupLabel => "backup cancelled",
            InputPrompt::ExportConfig => "export cancelled",
            InputPrompt::ImportConfig => "import cancelled",
            InputPrompt::CheckTarget => "disk check cancelled",
        }
    }
}

pub struct ActionItem {
    pub glyph: &'static str,
    pub ascii: &'static str,
    pub label: &'static str,
    pub action: Action,
}

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

pub enum Task {
    Refresh,
    Command(Vec<String>),
    ListBackups,
    Status(data::Config),
}

pub enum WorkerMsg {
    Refreshed(Snapshot),
    Output { title: String, text: String },
    Backups(Vec<Backup>),
    Status(data::StatusReport),
}

#[derive(Default)]
pub struct Worker {
    rx: Option<Receiver<Result<WorkerMsg, String>>>,
    busy: bool,
}

pub struct TaskOverlay {
    pub label: String,
    pub started_at: u64,
    pub done: bool,
    pub done_at: u64,
    pub visible: bool,
}

impl Worker {
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    pub fn start(&mut self, task: Task) -> bool {
        if self.busy {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = match task {
                Task::Refresh => Ok(WorkerMsg::Refreshed(data::load_snapshot())),
                Task::ListBackups => crate::ops::collect_backups().map(WorkerMsg::Backups),
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

    pub fn poll(&mut self) -> Option<Result<WorkerMsg, String>> {
        let rx = self.rx.as_ref()?;
        match rx.try_recv() {
            Ok(message) => {
                self.rx = None;
                self.busy = false;
                Some(message)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                self.busy = false;
                Some(Err("background task stopped unexpectedly".to_string()))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HeaderAreas {
    pub count: Rect,
    pub warning: Rect,
    pub status: Rect,
}

#[derive(Debug, Clone)]
pub struct HandoffInfo {
    pub action: Action,
    pub title: &'static str,
    pub detail: &'static str,
}

pub struct App {
    pub snapshot: Snapshot,
    pub action_state: ListState,
    pub backup_state: TableState,
    pub backup_filter: String,
    pub backup_filtering: bool,
    pub focus: Focus,
    pub output: Vec<String>,
    pub output_offset: usize,
    pub output_source: OutputSource,
    pub log_lines: usize,
    pub status: String,
    pub show_help: bool,
    pub show_status: bool,
    pub status_scroll: usize,
    pub status_scroll_max: usize,
    pub status_report: Option<data::StatusReport>,
    pub show_daemon: bool,
    pub daemon_scroll: usize,
    pub daemon_scroll_max: usize,
    pub daemon_log: Vec<String>,
    pub daemon_boot_present: bool,
    pub preserve_output: bool,
    pub should_quit: bool,
    pub pending: Option<Vec<String>>,
    pub worker: Worker,
    pub tick: u64,
    pub help_opened_at: u64,
    pub task: Option<TaskOverlay>,
    pub loading: bool,
    pub help_scroll: usize,
    pub history: Vec<HistoryEntry>,
    pub show_history: bool,
    pub history_selected: usize,
    pub reduce_motion: bool,
    pub ascii: bool,
    pub screen: Screen,
    pub operation: Option<Operation>,
    pub op_result_handled: bool,
    pub pending_select: Option<String>,
    pub pending_confirm: Option<Action>,
    pub pending_input: Option<InputPrompt>,
    pub pending_handoff: Option<HandoffInfo>,
    pub handoff_yes: Rect,
    pub handoff_no: Rect,
    pub input_text: String,
    pub input_yes: Rect,
    pub input_no: Rect,
    pub confirm_yes: Rect,
    pub confirm_no: Rect,
    pub header_areas: HeaderAreas,
    pub stat_areas: [Rect; 4],
    pub actions_area: Rect,
    pub backups_area: Rect,
    pub output_area: Rect,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let mut app = Self::with_snapshot(Snapshot::default());
        app.history = HistoryStore::load(&data::history_path());
        app.loading = true;
        app.status = "loading backups…".to_string();
        app.refresh();
        app.status = "loading backups…".to_string();
        app
    }

    pub fn with_snapshot(snapshot: Snapshot) -> Self {
        let mut action_state = ListState::default();
        action_state.select(Some(0));
        let mut backup_state = TableState::default();
        if !snapshot.backups.is_empty() {
            backup_state.select(Some(0));
        }
        let output = snapshot.log.clone();
        let status = snapshot
            .warning
            .clone()
            .unwrap_or_else(|| "ready".to_string());

        Self {
            snapshot,
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
        }
    }

    pub fn open_help(&mut self) {
        self.show_help = true;
        self.help_opened_at = self.tick;
        self.help_scroll = 0;
    }

    pub fn open_status_report(&mut self, report: data::StatusReport) {
        self.status_report = Some(report);
        self.show_status = true;
        self.status_scroll = 0;
    }

    pub fn open_daemon(&mut self) {
        self.daemon_log = data::daemon_activity(&data::tail_log(500));
        self.daemon_boot_present = data::boot_script_path().is_file();
        self.show_daemon = true;
        self.daemon_scroll = 0;
    }

    pub fn help_progress(&self) -> f32 {
        if !self.show_help || self.reduce_motion {
            return 1.0;
        }
        progress(self.tick, self.help_opened_at, 4)
    }

    pub fn task_progress(&self) -> f32 {
        match &self.task {
            Some(task) if !self.reduce_motion => {
                progress(self.tick, task.started_at.saturating_add(3), 2)
            }
            _ => 1.0,
        }
    }

    pub fn needs_tick(&self) -> bool {
        self.worker.is_busy()
            || self.task.is_some()
            || self.help_progress() < 1.0
            || self
                .operation
                .as_ref()
                .is_some_and(|operation| operation.is_running())
    }

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

    pub fn spinner(&self) -> Option<&'static str> {
        if self.reduce_motion {
            return None;
        }
        if !self.worker.is_busy()
            && self.pending.is_none()
            && !self
                .operation
                .as_ref()
                .is_some_and(|operation| operation.is_running())
        {
            return None;
        }
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        Some(FRAMES[(self.tick as usize / 2) % FRAMES.len()])
    }

    pub fn filtered_backup_indices(&self) -> Vec<usize> {
        let filter = self.backup_filter.to_lowercase();
        self.snapshot
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
            .collect()
    }

    pub fn selected_backup(&self) -> Option<&Backup> {
        let position = self.backup_state.selected()?;
        let index = *self.filtered_backup_indices().get(position)?;
        self.snapshot.backups.get(index)
    }

    #[cfg(test)]
    pub fn handle_key(&mut self, code: KeyCode) {
        self.handle_key_with(code, KeyModifiers::NONE);
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        self.handle_key_with(key.code, key.modifiers);
    }

    fn handle_key_with(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if modifiers.contains(KeyModifiers::CONTROL)
            && matches!(code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            if let Some(operation) = &mut self.operation {
                if operation.is_done() {
                    self.should_quit = true;
                } else if !operation.request_cancel() {
                    operation.notice = "Restore in progress — cannot cancel safely".to_string();
                }
                return;
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
            return;
        }
        if let Some(action) = self.pending_confirm {
            match code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    self.pending_confirm = None;
                    self.execute_confirmed_action(action);
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => {
                    self.pending_confirm = None;
                    self.status = "cancelled".to_string();
                }
                _ => {}
            }
            return;
        }
        if self.pending_handoff.is_some() {
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
            return;
        }
        if self.pending_input.is_some() {
            self.handle_input_key(code, modifiers);
            return;
        }
        if self.backup_filtering {
            self.handle_backups_key(code);
            return;
        }
        if self.show_history {
            match code {
                KeyCode::Esc | KeyCode::Char('h') | KeyCode::Char('q') => {
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
            return;
        }
        if self.show_status {
            match code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
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
            return;
        }
        if self.show_daemon {
            match code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
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
            return;
        }
        if self.operation.is_some() {
            self.handle_operation_key(code, modifiers);
            return;
        }

        if self.pending.is_some() {
            return;
        }
        if self.show_help {
            match code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                    self.show_help = false;
                    self.help_scroll = 0;
                }
                KeyCode::Down | KeyCode::Char('j') => self.help_scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1);
                }
                KeyCode::PageDown => self.help_scroll += 5,
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(5),
                _ => {}
            }
            return;
        }

        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.open_help(),
            KeyCode::Tab => self.switch_focus(self.focus.next()),
            KeyCode::BackTab => self.switch_focus(self.focus.previous()),
            KeyCode::Char('R') => self.refresh(),
            KeyCode::Char('L') => self.run(Action::List),
            KeyCode::Char('s') => self.run(Action::Status),
            KeyCode::Char('h') => {
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
        if modifiers.contains(KeyModifiers::CONTROL)
            && matches!(code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            self.pending_input = None;
            self.input_text.clear();
            self.should_quit = true;
            return;
        }
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
        let ctrl_c = modifiers.contains(KeyModifiers::CONTROL)
            && matches!(code, KeyCode::Char('c') | KeyCode::Char('C'));
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
                        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => {
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
                    KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('q')
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
                    KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('q') => {
                        self.notice_operation("Restore in progress — cannot cancel safely");
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if let Some(operation) = &mut self.operation {
                            operation.log_offset = operation.log_offset.saturating_add(1);
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if let Some(operation) = &mut self.operation {
                            operation.log_offset = operation.log_offset.saturating_sub(1);
                        }
                    }
                    KeyCode::PageUp => {
                        if let Some(operation) = &mut self.operation {
                            operation.log_offset = operation.log_offset.saturating_add(10);
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
                            operation.log_offset = usize::MAX / 2;
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
            KeyCode::Char('q') | KeyCode::Char('c') if plain => {
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
            KeyCode::Char('q') | KeyCode::Char('c') if plain => {
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
                operation.diff_scroll = operation.diff_scroll.saturating_add(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                operation.diff_scroll = operation.diff_scroll.saturating_sub(1)
            }
            KeyCode::PageUp => operation.diff_scroll = operation.diff_scroll.saturating_add(10),
            KeyCode::PageDown => operation.diff_scroll = operation.diff_scroll.saturating_sub(10),
            KeyCode::Char('G') | KeyCode::End => operation.diff_scroll = 0,
            KeyCode::Char('g') | KeyCode::Home => operation.diff_scroll = usize::MAX / 2,
            _ => {}
        }
    }

    fn notice_operation(&mut self, message: &str) {
        if let Some(operation) = &mut self.operation {
            operation.notice = message.to_string();
        }
    }

    fn start_operation(&mut self, kind: Kind, name: &str, args: Vec<String>) {
        if !crate::ops::supports_events() {
            self.status =
                "installed termux-sync is too old for the TUI (missing --json-events). Update it."
                    .to_string();
            return;
        }
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
                .map(|operation| operation.name.clone())
                .unwrap_or_default();
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
                        .map(|result| result.name.clone())
                        .unwrap_or_default()
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
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0),
            };
            self.history.insert(0, entry);
            self.history.truncate(20);
            let _ = HistoryStore::save(&data::history_path(), &self.history);
        }
        if let Some(name) = success_name {
            if !name.is_empty() {
                self.pending_select = Some(name);
            }
            self.refresh();
        }
        changed
    }

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

    pub fn handle_mouse(&mut self, column: u16, row: u16) {
        let point = Position::new(column, row);
        if self.show_status {
            return;
        }
        if self.show_daemon {
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
        if self.operation.is_some() {
            return;
        }
        if self.pending.is_some() || self.task.is_some() || self.show_help {
            return;
        }

        if self.header_areas.warning.contains(point) {
            self.run(Action::Logs);
            return;
        }
        if self.header_areas.count.contains(point) {
            self.switch_focus(Focus::Backups);
            return;
        }
        if self.header_areas.status.contains(point) {
            self.run(Action::Logs);
            return;
        }
        for (index, area) in self.stat_areas.iter().enumerate() {
            if area.contains(point) {
                if index == 3 {
                    self.switch_focus(Focus::Actions);
                    self.status = format!("AUTO: {}", self.snapshot.config.schedule);
                } else {
                    self.switch_focus(Focus::Backups);
                }
                return;
            }
        }

        if self.actions_area.contains(point) {
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
            return;
        }

        if self.backups_area.contains(point) {
            self.switch_focus(Focus::Backups);
            let first_data = self.backups_area.y.saturating_add(2);
            if row >= first_data {
                let index = (row - first_data) as usize + self.backup_state.offset();
                if index < self.filtered_backup_indices().len() {
                    self.backup_state.select(Some(index));
                }
            }
            return;
        }

        if self.output_area.contains(point) {
            self.switch_focus(Focus::Output);
        }
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
            KeyCode::Up | KeyCode::Char('k') => self.output_offset += 1,
            KeyCode::Down | KeyCode::Char('j') => {
                self.output_offset = self.output_offset.saturating_sub(1);
            }
            KeyCode::PageUp => self.output_offset += 10,
            KeyCode::PageDown => self.output_offset = self.output_offset.saturating_sub(10),
            KeyCode::Char('[') => self.adjust_log_lines(-200),
            KeyCode::Char(']') => self.adjust_log_lines(200),
            KeyCode::Char('g') | KeyCode::Home => self.output_offset = usize::MAX / 2,
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

    pub fn run(&mut self, action: Action) {
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
            Action::List => {
                if crate::ops::supports_events() {
                    self.start_task(Task::ListBackups);
                } else {
                    self.start_task(Task::Command(vec!["list".to_string()]));
                    self.status =
                        "termux-sync is too old for structured list; using text output".to_string();
                }
            }
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
            Action::Quit => self.should_quit = true,
        }
    }

    fn execute_confirmed_action(&mut self, action: Action) {
        self.execute_confirmed_action_with(action, crate::ops::supports_yes());
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
            self.status = label.clone();
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

    pub fn refresh(&mut self) {
        if self.snapshot.config.storage != "local" && crate::ops::supports_events() {
            self.start_task(Task::ListBackups);
        } else {
            self.start_task(Task::Refresh);
        }
    }

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
                self.open_status_report(report);
                self.status = "status ready".to_string();
            }
            Ok(WorkerMsg::Output { title, text }) => {
                self.output_source = OutputSource::Command;
                self.output.clear();
                self.output.push(format!("$ termux-sync {title}"));
                self.output.extend(text.lines().map(str::to_string));
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
            Err(err) => {
                self.output_source = OutputSource::Command;
                self.output.clear();
                self.output.extend(err.lines().map(str::to_string));
                self.output_offset = 0;
                self.switch_focus(Focus::Output);
                self.status = "command failed".to_string();
            }
        }
        true
    }

    pub fn apply_backup_list(&mut self, backups: Vec<Backup>) {
        let count = backups.len();
        self.snapshot.backups = backups;
        self.snapshot.warning = None;
        self.backup_filter.clear();
        self.backup_filtering = false;
        self.backup_state.select((count > 0).then_some(0));
        self.switch_focus(Focus::Backups);
        self.status = format!("{count} backup(s)");
        self.op_result_handled = false;
    }

    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        let selected_name = self
            .pending_select
            .take()
            .or_else(|| self.selected_backup().map(|backup| backup.name.clone()));
        let keep_output = std::mem::take(&mut self.preserve_output);

        self.loading = false;
        if !keep_output {
            self.status = snapshot
                .warning
                .clone()
                .unwrap_or_else(|| "refreshed".to_string());
        }
        self.snapshot = snapshot;
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

pub fn restore_command(name: &str) -> Vec<String> {
    vec![
        "restore".to_string(),
        "--name".to_string(),
        name.to_string(),
    ]
}

pub fn export_config_command(path: &str) -> Vec<String> {
    path_command("export-config", path)
}

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

pub fn check_command(target: &str) -> Vec<String> {
    let target = target.trim();
    if target.is_empty() {
        vec!["check".to_string()]
    } else {
        path_command("check", target)
    }
}

pub fn clear_logs_command(yes_supported: bool) -> Vec<String> {
    let mut args = vec!["clear-logs".to_string()];
    if yes_supported {
        args.push("--yes".to_string());
    }
    args
}

pub fn should_refresh_after(title: &str) -> bool {
    ["delete", "clear-cache", "clear-logs", "import-config"]
        .iter()
        .any(|prefix| title.starts_with(prefix))
}

pub fn input_command(prompt: InputPrompt, text: &str) -> Option<Vec<String>> {
    let value = text.trim();
    match prompt {
        InputPrompt::BackupLabel => None,
        InputPrompt::ExportConfig => (!value.is_empty()).then(|| export_config_command(value)),
        InputPrompt::ImportConfig => (!value.is_empty()).then(|| import_config_command(value)),
        InputPrompt::CheckTarget => Some(check_command(value)),
    }
}

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
    use crate::data::Config;
    use crate::ops::{Kind, Status, test_operation, test_plan};

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
            warning: None,
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
        assert_eq!(app.filtered_backup_indices(), vec![1]);
        app.backup_filter = "backup-one".into();
        assert_eq!(app.filtered_backup_indices(), vec![0]);
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
        assert!(app.input_text.is_empty());
        app.handle_key(KeyCode::Esc);

        app.run(Action::DiskCheck);
        assert_eq!(app.pending_input, Some(InputPrompt::CheckTarget));
        assert!(app.input_text.is_empty());
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
        app.snapshot.warning = Some("remote".into());
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
        assert!(app.snapshot.warning.is_none());
        assert_eq!(app.focus, Focus::Backups);
    }
}
