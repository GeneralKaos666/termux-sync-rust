use std::borrow::Cow;
use std::sync::OnceLock;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, Gauge, List, ListItem, Padding, Paragraph, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table, Wrap,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{ACTIONS, Action, App, Focus, OutputSource, Screen};
use crate::data::{backup_age, human_size, parse_log_line};
use crate::ops::{Kind, Operation, Plan, StageState, Status};

pub const MIN_WIDTH: u16 = 48;
pub const MIN_HEIGHT: u16 = 12;
const SPLIT_WIDTH: u16 = 80;

const ACCENT: Color = Color::Cyan;
const SUCCESS: Color = Color::Green;
const WARN: Color = Color::Yellow;
const DANGER: Color = Color::Red;
const MAGENTA: Color = Color::Magenta;
const MUTED: Color = Color::DarkGray;
const GRADIENT_FROM: Color = Color::Rgb(70, 220, 255);
const GRADIENT_TO: Color = Color::Rgb(210, 120, 255);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutPlan {
    TooSmall,
    Single,
    Split,
}

pub fn plan(area: Rect) -> LayoutPlan {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        LayoutPlan::TooSmall
    } else if area.width < SPLIT_WIDTH {
        LayoutPlan::Single
    } else {
        LayoutPlan::Split
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    if app.screen == Screen::Operation
        && let Some(operation) = &app.operation
    {
        render_operation(frame, area, app, operation);
        return;
    }

    app.header_areas = Default::default();
    app.stat_areas = [Rect::default(); 4];
    app.actions_area = Rect::default();
    app.backups_area = Rect::default();
    app.output_area = Rect::default();

    if plan(area) == LayoutPlan::TooSmall {
        render_too_small(frame, area);
        return;
    }

    let show_cards = area.width >= SPLIT_WIDTH && area.height >= 20;

    if show_cards {
        let [hero, cards, body, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .areas(area);

        render_hero(frame, hero, app);
        render_stats(frame, cards, app);
        render_body(frame, body, app);
        render_footer(frame, footer, app);
    } else {
        let [hero, body, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .areas(area);

        render_hero(frame, hero, app);
        render_body(frame, body, app);
        render_footer(frame, footer, app);
    }

    if app.show_help {
        render_help(frame, area, app);
    }

    if app.show_history {
        render_history(frame, area, app);
    }

    if app.show_status {
        let max_scroll = render_status(frame, area, app);
        app.status_scroll_max = max_scroll;
        app.status_scroll = app.status_scroll.min(max_scroll);
    }

    if app.show_daemon {
        render_daemon(frame, area, app);
    }

    if app.task.as_ref().is_some_and(|task| task.visible) {
        render_task(frame, area, app);
    }

    if let Some(action) = app.pending_confirm {
        render_action_confirm(frame, area, app, action);
    }

    if app.pending_input.is_some() {
        render_input_prompt(frame, area, app);
    }

    if app.pending_handoff.is_some() {
        render_handoff(frame, area, app);
    }
}

fn render_handoff(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(handoff) = &app.pending_handoff else {
        return;
    };
    let popup = centered(area, area.width.saturating_sub(6).clamp(36, 64), 9);
    let inner = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    let half = inner.width / 2;
    app.handoff_yes = Rect {
        x: inner.x,
        y: inner.y + inner.height.saturating_sub(1),
        width: half,
        height: 1,
    };
    app.handoff_no = Rect {
        x: inner.x + half,
        y: inner.y + inner.height.saturating_sub(1),
        width: inner.width.saturating_sub(half),
        height: 1,
    };

    let title = handoff.title;
    let detail = handoff.detail;
    let lines = vec![
        Line::from(Span::styled(title, fg(WARN).bold())),
        Line::from(""),
        Line::from(Span::styled(
            "The terminal will be handed to termux-sync.",
            fg(MUTED),
        )),
        Line::from(Span::styled(detail, fg(MUTED))),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Enter ", fg(SUCCESS).bold()),
            Span::styled("continue", fg(MUTED)),
            Span::raw("    "),
            Span::styled(" Esc ", fg(ACCENT).bold()),
            Span::styled("cancel", fg(MUTED)),
        ]),
    ];
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(op_block("Handoff", WARN))
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn render_input_prompt(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(prompt) = app.pending_input else {
        return;
    };
    let popup = centered(area, area.width.saturating_sub(6).clamp(32, 58), 7);
    let inner = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    let half = inner.width / 2;
    app.input_yes = Rect {
        x: inner.x,
        y: inner.y + inner.height.saturating_sub(1),
        width: half,
        height: 1,
    };
    app.input_no = Rect {
        x: inner.x + half,
        y: inner.y + inner.height.saturating_sub(1),
        width: inner.width.saturating_sub(half),
        height: 1,
    };

    let lines = vec![
        Line::from(Span::styled("Input", fg(MUTED))),
        Line::from(vec![
            Span::styled(
                tail_fit(&app.input_text, inner.width.saturating_sub(1) as usize),
                fg(Color::Reset).bold(),
            ),
            Span::styled("█", fg(ACCENT)),
        ]),
        Line::from(Span::styled(
            clip_text(prompt.hint(), inner.width as usize),
            fg(MUTED),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Enter ", fg(SUCCESS).bold()),
            Span::styled("continue", fg(MUTED)),
            Span::raw("    "),
            Span::styled(" Esc ", fg(ACCENT).bold()),
            Span::styled("cancel", fg(MUTED)),
        ]),
    ];
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(op_block(prompt.title(), ACCENT)),
        popup,
    );
}

fn clip_text(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let char_width = ch.width().unwrap_or(0);
        if used + char_width + 1 > width {
            break;
        }
        out.push(ch);
        used += char_width;
    }
    out.push('…');
    out
}

fn tail_fit(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 1;
    for ch in text.chars().rev() {
        let char_width = ch.width().unwrap_or(0);
        if used + char_width > width {
            break;
        }
        out.insert(0, ch);
        used += char_width;
    }
    format!("…{out}")
}

fn render_action_confirm(frame: &mut Frame, area: Rect, app: &mut App, action: Action) {
    let (title, message) = match action {
        Action::Delete => (
            "Delete backup",
            format!(
                "Delete {}?",
                app.selected_backup()
                    .map(|backup| backup.name.as_str())
                    .unwrap_or("the selected backup")
            ),
        ),
        Action::ClearCache => ("Clear cache", "Clear all cache directories?".to_string()),
        Action::ClearLogs => ("Clear logs", "Clear all log entries?".to_string()),
        _ => return,
    };
    let popup = centered(area, area.width.saturating_sub(6).clamp(32, 58), 8);
    let lines = vec![
        Line::from(Span::styled(title, fg(WARN).bold())),
        Line::from(""),
        Line::from(message),
        Line::from(Span::styled("This cannot be undone.", fg(DANGER).bold())),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Enter ", fg(SUCCESS).bold()),
            Span::styled("confirm", fg(MUTED)),
            Span::raw("    "),
            Span::styled(" Esc ", fg(ACCENT).bold()),
            Span::styled("cancel", fg(MUTED)),
        ]),
    ];
    let inner = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    let button_y = inner.y + inner.height.saturating_sub(1);
    let half = inner.width / 2;
    app.confirm_yes = Rect {
        x: inner.x,
        y: button_y,
        width: half,
        height: 1,
    };
    app.confirm_no = Rect {
        x: inner.x + half,
        y: button_y,
        width: inner.width.saturating_sub(half),
        height: 1,
    };

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(op_block(title, WARN))
            .alignment(Alignment::Center),
        popup,
    );
}

fn render_operation(frame: &mut Frame, area: Rect, app: &App, operation: &Operation) {
    let stage_height = if area.width >= 80 { 2 } else { 1 };
    let [header, stages, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(stage_height),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(area);

    render_operation_header(frame, header, operation);
    render_operation_stages(frame, stages, operation);
    if operation.status == Status::AwaitingConfirm && operation.diff_open {
        render_package_diff(frame, body, operation);
    } else {
        match operation.status {
            Status::AwaitingConfirm => render_restore_plan(frame, body, operation),
            Status::Running | Status::Cancelling => {
                render_operation_running(frame, body, app, operation)
            }
            Status::Succeeded | Status::Warnings => render_operation_result(frame, body, operation),
            Status::Failed => render_operation_failure(frame, body, operation),
            Status::Cancelled => render_operation_cancelled(frame, body, operation),
        }
    }
    render_operation_footer(frame, footer, app, operation);
    if operation.cancel_prompt {
        render_cancel_prompt(frame, area, operation);
    }
}

fn render_cancel_prompt(frame: &mut Frame, area: Rect, operation: &Operation) {
    let width = area.width.saturating_sub(4).clamp(30, 56);
    let popup = centered(area, width, 7);
    let what = match operation.kind {
        Kind::Backup => "backup",
        Kind::Restore => "restore",
    };
    let lines = vec![
        Line::from(Span::styled(
            format!("Cancel {what} and clean partial files?"),
            fg(WARN).bold(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Enter ", fg(SUCCESS).bold()),
            Span::styled("yes", fg(MUTED)),
            Span::raw("    "),
            Span::styled(" Esc ", fg(ACCENT).bold()),
            Span::styled("no", fg(MUTED)),
        ]),
    ];
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(op_block("Confirm cancel", WARN))
            .alignment(Alignment::Center),
        popup,
    );
}

fn render_operation_header(frame: &mut Frame, area: Rect, operation: &Operation) {
    let (kind, color) = match operation.kind {
        Kind::Backup => ("BACKUP", ACCENT),
        Kind::Restore => ("RESTORE", WARN),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(color))
        .title(Span::styled(format!(" {kind} "), fg(color).bold()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [left, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(9)]).areas(inner);
    let name = if operation.name.is_empty() {
        "preparing…".to_string()
    } else {
        operation.name.clone()
    };
    let storage = if operation.storage.is_empty() {
        String::new()
    } else {
        format!("  {}", operation.storage.to_uppercase())
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {name}"), fg(SUCCESS).bold()),
            Span::styled(storage, fg(MUTED)),
        ])),
        left,
    );
    frame.render_widget(
        Paragraph::new(fmt_elapsed(operation.elapsed()))
            .alignment(Alignment::Right)
            .style(fg(MUTED)),
        right,
    );
}

fn render_operation_stages(frame: &mut Frame, area: Rect, operation: &Operation) {
    if area.width < 80 {
        let position = operation
            .current_stage
            .as_ref()
            .and_then(|id| operation.stages.iter().position(|stage| &stage.id == id))
            .map_or(0, |index| index + 1);
        let label = operation
            .current_stage
            .as_ref()
            .and_then(|id| {
                operation
                    .stages
                    .iter()
                    .find(|stage| &stage.id == id)
                    .map(|stage| stage.label.clone())
            })
            .unwrap_or_else(|| "starting".to_string());
        let line = Line::from(vec![
            Span::styled(
                format!(" {}/{} ", position, operation.stages.len()),
                fg(ACCENT).bold(),
            ),
            Span::styled(label, fg(Color::Reset).bold()),
            Span::raw("  "),
            Span::styled(operation.progress_text(), fg(MUTED)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let mut spans = Vec::new();
    for (index, stage) in operation.stages.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("   "));
        }
        let (symbol, color) = stage_symbol(stage.state);
        let style = if stage.state == StageState::Active {
            fg(color).bold()
        } else {
            fg(color)
        };
        spans.push(Span::styled(format!("{symbol} "), style));
        spans.push(Span::styled(stage.label.clone(), style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_operation_running(frame: &mut Frame, area: Rect, app: &App, operation: &Operation) {
    let [gauge_area, detail_area, log_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);

    let title = if operation.protected {
        " Progress · CANNOT CANCEL SAFELY ".to_string()
    } else if operation.status == Status::Cancelling {
        " Cancelling… ".to_string()
    } else {
        " Progress ".to_string()
    };
    let color = if operation.protected { WARN } else { ACCENT };
    let gauge = Gauge::default()
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(fg(color))
                .title(Span::styled(title, fg(color).bold())),
        )
        .gauge_style(fg(color))
        .ratio(operation.overall() as f64)
        .label(format!("{:.0}%", operation.overall() * 100.0));
    frame.render_widget(gauge, gauge_area);

    let marker = if operation.reduce_motion {
        "·"
    } else {
        app.spinner().unwrap_or("·")
    };
    let detail = operation
        .stage_detail
        .as_deref()
        .map(|detail| format!("  {detail}"))
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {marker} "), fg(color).bold()),
            Span::styled(operation.progress_text(), fg(Color::Reset).bold()),
            Span::styled(detail, fg(MUTED)),
        ])),
        detail_area,
    );

    render_operation_log(frame, log_area, operation);
}

fn render_operation_log(frame: &mut Frame, area: Rect, operation: &Operation) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(MUTED))
        .title(Span::styled(" Live log ", fg(MUTED)));
    let inner_height = block.inner(area).height as usize;
    let len = operation.logs.len();
    let max_offset = len.saturating_sub(inner_height);
    let offset = operation.log_offset.min(max_offset);
    let end = len.saturating_sub(offset);
    let start = end.saturating_sub(inner_height);

    let mut lines: Vec<Line> = Vec::with_capacity(end - start);
    for entry in operation.logs.iter().skip(start).take(end - start) {
        lines.push(Line::from(vec![
            level_chip(&entry.level),
            Span::raw(" "),
            Span::raw(crate::data::strip_controls(&entry.message)),
        ]));
    }

    frame.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
    let position = operation
        .logs
        .len()
        .saturating_sub(1)
        .saturating_sub(offset);
    scrollbar(frame, area, operation.logs.len(), inner_height, position);
}

fn render_restore_plan(frame: &mut Frame, area: Rect, operation: &Operation) {
    let Some(plan) = &operation.plan else {
        frame.render_widget(
            Paragraph::new("Preparing restore plan…")
                .block(op_block("Restore plan", WARN))
                .style(fg(MUTED)),
            area,
        );
        return;
    };

    let block = op_block("Restore plan", WARN);
    let compact = block.inner(area).height < 9;
    let mut lines = Vec::new();
    let diff = diff_counts(plan);

    if compact {
        lines.push(warning_line(plan));
        lines.push(confirm_line(operation));
        if block.inner(area).height >= 4 {
            lines.push(Line::from(Span::styled(
                " d  package diff · Esc cancel",
                fg(MUTED),
            )));
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled(" Backup   ", fg(MUTED)),
            Span::styled(plan.name.clone(), fg(Color::Reset).bold()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(" Date     ", fg(MUTED)),
            Span::raw(
                plan.date
                    .replace('T', " ")
                    .chars()
                    .take(16)
                    .collect::<String>(),
            ),
            Span::styled("   Label ", fg(MUTED)),
            Span::raw(if plan.label.is_empty() {
                "—".to_string()
            } else {
                plan.label.clone()
            }),
        ]));
        lines.push(Line::from(vec![
            Span::styled(" Size     ", fg(MUTED)),
            Span::raw(human_size(plan.size)),
            Span::styled("   Storage ", fg(MUTED)),
            Span::raw(plan.storage.to_uppercase()),
            Span::styled("   Archives ", fg(MUTED)),
            Span::raw(plan.archives.len().to_string()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(" Versions ", fg(MUTED)),
            Span::raw(format!("backup {}", plan.backup_version)),
            Span::styled("   current ", fg(MUTED)),
            Span::raw(plan.current_version.clone()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(" Packages ", fg(MUTED)),
            Span::styled(plan.packages.to_string(), fg(SUCCESS).bold()),
            Span::styled(" total · ", fg(MUTED)),
            Span::styled(
                diff.unwrap_or_else(|| "diff unavailable".to_string()),
                fg(MUTED),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(" Device   ", fg(MUTED)),
            Span::styled(
                format!(
                    "{} device-only packages stay installed",
                    plan.device_only.unwrap_or(0)
                ),
                fg(MUTED),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(" Verify   ", fg(MUTED)),
            Span::styled(
                if plan.checksums == "pending" {
                    "checksums verify before extraction"
                } else {
                    &plan.checksums
                },
                fg(MUTED),
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(warning_line(plan));
        lines.push(Line::from(""));
        lines.push(confirm_line(operation));
        lines.push(Line::from(Span::styled(
            " d  package diff · Esc cancel",
            fg(MUTED),
        )));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_package_diff(frame: &mut Frame, area: Rect, operation: &Operation) {
    let Some(plan) = &operation.plan else {
        return;
    };
    let narrow = area.width < 60;
    let show_filter =
        operation.diff_filtering || !operation.diff_filter.is_empty() || area.width >= 60;
    let warning_height = if narrow { 2 } else { 3 };

    let [warning_area, tabs_area, filter_area, list_area] = if show_filter {
        Layout::vertical([
            Constraint::Length(warning_height),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(area)
    } else {
        let [warning, tabs, list] = Layout::vertical([
            Constraint::Length(warning_height),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(area);
        [warning, tabs, Rect::default(), list]
    };

    let warning = if plan.diff_available {
        if narrow {
            Text::from(vec![
                Line::from(Span::styled(
                    format!("! All {} packages reinstalled.", plan.packages),
                    fg(WARN).bold(),
                )),
                Line::from(Span::styled(
                    format!(
                        "  {} already installed; {} device-only stay.",
                        plan.already_installed.unwrap_or(0),
                        plan.device_only.unwrap_or(0)
                    ),
                    fg(MUTED),
                )),
            ])
        } else {
            Text::from(vec![
                Line::from(Span::styled(
                    format!(
                        "! Restore reinstalls all {} packages from this backup.",
                        plan.packages
                    ),
                    fg(WARN).bold(),
                )),
                Line::from(Span::styled(
                    format!(
                        "  {} already-installed packages are reinstalled too.",
                        plan.already_installed.unwrap_or(0)
                    ),
                    fg(WARN),
                )),
                Line::from(Span::styled(
                    format!(
                        "  {} device-only packages stay installed and are not removed.",
                        plan.device_only.unwrap_or(0)
                    ),
                    fg(MUTED),
                )),
            ])
        }
    } else {
        Text::from(vec![
            Line::from(Span::styled(
                format!("! Restore reinstalls all {} packages.", plan.packages),
                fg(WARN).bold(),
            )),
            Line::from(Span::styled(
                "  Current package list unavailable; diff cannot be shown.",
                fg(WARN),
            )),
        ])
    };
    frame.render_widget(
        Paragraph::new(warning).wrap(Wrap { trim: true }),
        warning_area,
    );

    let categories = if narrow {
        [
            ("1 NEW", operation.diff.to_install.len()),
            ("2 PRESENT", operation.diff.already_installed.len()),
            ("3 DEVICE", operation.diff.device_only.len()),
        ]
    } else {
        [
            ("1 TO INSTALL", operation.diff.to_install.len()),
            (
                "2 ALREADY INSTALLED",
                operation.diff.already_installed.len(),
            ),
            ("3 DEVICE ONLY", operation.diff.device_only.len()),
        ]
    };
    let mut tabs = Vec::new();
    for (index, (label, count)) in categories.iter().enumerate() {
        if index > 0 {
            tabs.push(Span::raw(if narrow { "  " } else { "   " }));
        }
        let selected = index == operation.diff_section;
        let style = if selected {
            fg(MAGENTA).bold()
        } else {
            fg(MUTED)
        };
        tabs.push(Span::styled(format!("{label} ({count})"), style));
    }
    frame.render_widget(Paragraph::new(Line::from(tabs)), tabs_area);

    if show_filter {
        let filter = if operation.diff_filtering {
            format!("/ {}{}", operation.diff_filter, "█")
        } else if operation.diff_filter.is_empty() {
            " / to filter · 1/2/3 sections · Esc back".to_string()
        } else {
            format!(" filter: {}", operation.diff_filter)
        };
        frame.render_widget(Paragraph::new(filter).style(fg(MUTED)), filter_area);
    }

    let block = if list_area.height >= 6 {
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(fg(MUTED))
            .title(Span::styled(
                if operation.diff.complete() {
                    format!(" Packages · {} ", operation.diff.total())
                } else {
                    format!(" Packages · {} · partial diff ", operation.diff.total())
                },
                fg(MUTED),
            ))
    } else {
        Block::default()
    };
    let inner_height = block.inner(list_area).height as usize;
    let packages = operation.filtered_diff();
    let len = packages.len();
    let max_offset = len.saturating_sub(inner_height);
    let offset = operation.diff_scroll.min(max_offset);
    let end = len.saturating_sub(offset);
    let start = end.saturating_sub(inner_height);
    let lines: Vec<Line> = packages[start..end]
        .iter()
        .map(|package| Line::from(format!("  {package}")))
        .collect();
    frame.render_widget(Paragraph::new(Text::from(lines)).block(block), list_area);
    let position = packages.len().saturating_sub(1).saturating_sub(offset);
    scrollbar(frame, list_area, packages.len(), inner_height, position);
}

fn render_operation_result(frame: &mut Frame, area: Rect, operation: &Operation) {
    let Some(result) = &operation.result else {
        return;
    };
    let block = op_block("Result", SUCCESS);
    let mut lines = vec![Line::from(Span::styled(
        if !result.ok {
            "FAILED"
        } else if result.warnings {
            "COMPLETED WITH ERRORS"
        } else {
            "SUCCESS"
        },
        fg(if result.warnings || !result.ok {
            WARN
        } else {
            SUCCESS
        })
        .bold(),
    ))];
    lines.push(Line::from(vec![
        Span::styled(" Name      ", fg(MUTED)),
        Span::styled(result.name.clone(), fg(Color::Reset).bold()),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" Size      ", fg(MUTED)),
        Span::raw(human_size(result.size)),
        Span::styled("   Duration ", fg(MUTED)),
        Span::raw(fmt_millis(result.duration_ms)),
    ]));
    if operation.kind == Kind::Backup {
        let (label, color) = match result.verified {
            Some(true) => ("VERIFIED", SUCCESS),
            Some(false) => ("CORRUPT", DANGER),
            None => ("SKIPPED", WARN),
        };
        lines.push(Line::from(vec![
            Span::styled(" Verify    ", fg(MUTED)),
            Span::styled(label, fg(color).bold()),
            Span::styled(format!("  {}", result.verify_details), fg(MUTED)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(" Archives  ", fg(MUTED)),
            Span::styled(
                format!(
                    "{}/{} extracted",
                    result.archives.saturating_sub(result.archives_failed.len()),
                    result.archives
                ),
                fg(if result.archives_failed.is_empty() {
                    SUCCESS
                } else {
                    DANGER
                }),
            ),
            Span::styled("   Packages ", fg(MUTED)),
            Span::styled(
                format!(
                    "{}/{} installed",
                    result.packages_installed, result.packages
                ),
                fg(if result.packages_failed.is_empty() {
                    SUCCESS
                } else {
                    WARN
                }),
            ),
        ]));
        for package in result.packages_failed.iter().take(4) {
            lines.push(Line::from(Span::styled(
                format!("   package failed: {package}"),
                fg(DANGER),
            )));
        }
        for archive in result.archives_failed.iter().take(4) {
            lines.push(Line::from(Span::styled(
                format!("   archive failed: {archive}"),
                fg(DANGER),
            )));
        }
    }
    if result.restart_required {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Close and reopen Termux to use the restored environment.",
            fg(WARN).bold(),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" Esc  dashboard", fg(MUTED))));
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_operation_failure(frame: &mut Frame, area: Rect, operation: &Operation) {
    let block = op_block("Failed", DANGER);
    let mut lines = vec![Line::from(Span::styled(
        "✕ OPERATION FAILED",
        fg(DANGER).bold(),
    ))];
    if let Some(error) = &operation.error {
        lines.push(Line::from(Span::styled(
            error.message.clone(),
            fg(Color::Reset),
        )));
        if !error.hint.is_empty() {
            lines.push(Line::from(Span::styled(error.hint.clone(), fg(MUTED))));
        }
        if let Some(code) = error.exit_code {
            lines.push(Line::from(Span::styled(
                format!("exit code {code}"),
                fg(MUTED),
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " r  retry    Esc  dashboard",
        fg(MUTED),
    )));
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_operation_cancelled(frame: &mut Frame, area: Rect, operation: &Operation) {
    let block = op_block("Cancelled", WARN);
    let mut lines = vec![Line::from(Span::styled(
        "Operation cancelled.",
        fg(WARN).bold(),
    ))];
    if !operation.notice.is_empty() {
        lines.push(Line::from(Span::styled(
            operation.notice.clone(),
            fg(MUTED),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" Esc  dashboard", fg(MUTED))));
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_operation_footer(frame: &mut Frame, area: Rect, app: &App, operation: &Operation) {
    let marker = if operation.reduce_motion {
        "●"
    } else {
        app.spinner().unwrap_or("●")
    };
    let color = if operation.status == Status::Failed {
        DANGER
    } else if operation.status == Status::Succeeded {
        SUCCESS
    } else if operation.status == Status::Warnings
        || operation.status == Status::Cancelled
        || operation.protected
    {
        WARN
    } else {
        ACCENT
    };
    let status = if operation.notice.is_empty() {
        status_text(operation)
    } else {
        operation.notice.clone()
    };
    let line1 = Line::from(vec![
        Span::styled(format!(" {marker} "), fg(color).bold()),
        Span::styled(status, fg(color)),
    ]);

    let hints = match operation.status {
        Status::AwaitingConfirm => "Type RESTORE · Enter confirm · d diff · Esc cancel",
        Status::Running if operation.protected => "CANNOT CANCEL · ↑↓ log",
        Status::Running => "Esc cancel · ↑↓ log · G end",
        Status::Cancelling => "waiting for process to exit…",
        Status::Failed => "r retry · Esc dashboard",
        Status::Succeeded | Status::Warnings | Status::Cancelled => "Esc dashboard",
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            line1,
            Line::from(Span::styled(hints, fg(MUTED))),
        ])),
        area,
    );
}

fn op_block(title: &str, color: Color) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(color))
        .title(Span::styled(format!(" {title} "), fg(color).bold()))
        .padding(Padding::horizontal(1))
}

fn stage_symbol(state: StageState) -> (&'static str, Color) {
    match state {
        StageState::Pending => ("·", MUTED),
        StageState::Active => ("▸", ACCENT),
        StageState::Complete => ("✓", SUCCESS),
        StageState::Failed => ("✕", DANGER),
        StageState::Skipped => ("–", MUTED),
    }
}

fn status_text(operation: &Operation) -> String {
    match operation.status {
        Status::Running => format!("{} running", kind_label(operation.kind)),
        Status::AwaitingConfirm => "restore plan ready".to_string(),
        Status::Cancelling => "cancelling and cleaning up".to_string(),
        Status::Succeeded => format!("{} complete", kind_label(operation.kind)),
        Status::Warnings => "completed with errors".to_string(),
        Status::Failed => "operation failed".to_string(),
        Status::Cancelled => "cancelled".to_string(),
    }
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Backup => "backup",
        Kind::Restore => "restore",
    }
}

fn warning_line(plan: &Plan) -> Line<'static> {
    if plan.diff_available {
        Line::from(Span::styled(
            format!(
                "! Reinstalls all {} packages; {} already installed.",
                plan.packages,
                plan.already_installed.unwrap_or(0)
            ),
            fg(WARN).bold(),
        ))
    } else {
        Line::from(Span::styled(
            format!(
                "! Reinstalls all {} packages. Current package list unavailable.",
                plan.packages
            ),
            fg(WARN).bold(),
        ))
    }
}

fn confirm_line(operation: &Operation) -> Line<'static> {
    let ready = operation.confirm_text.trim() == "RESTORE";
    Line::from(vec![
        Span::styled(" Type RESTORE to continue: ", fg(MUTED)),
        Span::styled(
            format!("{}{}", operation.confirm_text, "█"),
            if ready {
                fg(SUCCESS).bold()
            } else {
                fg(ACCENT)
            },
        ),
    ])
}

fn diff_counts(plan: &Plan) -> Option<String> {
    Some(format!(
        "{} to install · {} already · {} device-only",
        plan.to_install?, plan.already_installed?, plan.device_only?
    ))
}

fn history_age(now: u64, at: u64) -> String {
    let seconds = now.saturating_sub(at);
    if seconds < 60 {
        "now".to_string()
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn fmt_elapsed(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn fmt_millis(millis: u64) -> String {
    fmt_elapsed(std::time::Duration::from_millis(millis))
}

fn render_hero(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(ACCENT))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [left, right] = if area.width < 80 {
        Layout::horizontal([Constraint::Min(0), Constraint::Length(26)]).areas(inner)
    } else {
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(inner)
    };

    let mut title = gradient_spans("TERMUX SYNC");
    if area.width >= 80 {
        title.push(Span::styled("   backup dashboard", fg(MUTED)));
    }
    if area.width >= 100 {
        title.push(Span::styled(
            format!(
                "   {} · max {}",
                app.snapshot.config.compression.to_uppercase(),
                app.snapshot.config.max_backups
            ),
            fg(MUTED),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(title)), left);

    let count = app.snapshot.backups.len();
    let count_label = if app.loading {
        "LOADING".to_string()
    } else if count == 1 {
        "1 BACKUP".to_string()
    } else {
        format!("{count} BACKUPS")
    };
    let count_color = if app.loading {
        ACCENT
    } else if count == 0 {
        MUTED
    } else {
        SUCCESS
    };
    let mut chips = vec![
        badge(&app.snapshot.config.storage.to_uppercase(), ACCENT),
        Span::raw(" "),
        count_text(&count_label, count_color),
    ];
    if !app.snapshot.warning.is_empty() {
        chips.push(Span::raw(" "));
        chips.push(Span::styled("WARN", fg(WARN).bold()));
    }
    app.header_areas.count = Rect {
        x: right.x,
        y: right.y,
        width: right.width.saturating_sub(6),
        height: 1,
    };
    if !app.snapshot.warning.is_empty() {
        app.header_areas.warning = Rect {
            x: right.x + right.width.saturating_sub(5),
            y: right.y,
            width: 5,
            height: 1,
        };
    }
    frame.render_widget(
        Paragraph::new(Line::from(chips)).alignment(Alignment::Right),
        right,
    );
}

fn render_stats(frame: &mut Frame, area: Rect, app: &mut App) {
    let [backups, stored, packages, auto] = Layout::horizontal([Constraint::Fill(1); 4])
        .spacing(1)
        .areas(area);
    app.stat_areas = [backups, stored, packages, auto];

    let total_size: u64 = app.snapshot.backups.iter().map(|backup| backup.size).sum();
    let latest_packages = app
        .snapshot
        .backups
        .first()
        .map_or(0, |backup| backup.packages);
    let placeholder = app.loading.then_some("…".to_string());
    let backups_value = placeholder
        .clone()
        .unwrap_or_else(|| app.snapshot.backups.len().to_string());
    let stored_value = placeholder
        .clone()
        .unwrap_or_else(|| human_size(total_size));
    let packages_value = placeholder
        .clone()
        .unwrap_or_else(|| latest_packages.to_string());
    let auto_value = placeholder.unwrap_or_else(|| app.snapshot.config.schedule.clone());

    stat_card(frame, backups, "BACKUPS", &backups_value, "saved", SUCCESS);
    stat_card(frame, stored, "STORED", &stored_value, "", ACCENT);
    stat_card(
        frame,
        packages,
        "PACKAGES",
        &packages_value,
        "latest",
        MAGENTA,
    );
    stat_card(frame, auto, "AUTO", &auto_value, "", WARN);
}

fn stat_card(frame: &mut Frame, area: Rect, title: &str, value: &str, unit: &str, color: Color) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(color))
        .title(Span::styled(format!(" {title} "), fg(color).bold()));
    let line = Line::from(vec![
        Span::styled(value.to_string(), fg(color).bold()),
        Span::styled(format!(" {unit}"), fg(MUTED)),
    ]);
    frame.render_widget(Paragraph::new(line).block(block), area);
}

fn render_body(frame: &mut Frame, area: Rect, app: &mut App) {
    if plan(frame.area()) == LayoutPlan::Split {
        let [left, right] =
            Layout::horizontal([Constraint::Length(26), Constraint::Min(0)]).areas(area);
        let [backups, output] =
            Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(right);

        render_actions(frame, left, app);
        render_backups(frame, backups, app);
        render_output(frame, output, app);
    } else {
        match app.focus {
            Focus::Actions => render_actions(frame, area, app),
            Focus::Backups => render_backups(frame, area, app),
            Focus::Output => render_output(frame, area, app),
        }
    }
}

fn render_actions(frame: &mut Frame, area: Rect, app: &mut App) {
    app.actions_area = area;
    let inner = area.width.saturating_sub(4) as usize;
    let label_width = inner.saturating_sub(3);
    let items: Vec<ListItem> = ACTIONS
        .iter()
        .map(|item| {
            let glyph = if app.ascii { item.ascii } else { item.glyph };
            let glyph_width = UnicodeWidthStr::width(glyph);
            let padding = 2usize.saturating_sub(glyph_width);
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{}{} ", glyph, " ".repeat(padding)),
                    fg(ACCENT).bold(),
                ),
                Span::raw(format!("{:<width$}", item.label, width = label_width)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(pane_block(
            "Actions",
            None,
            app.focus == Focus::Actions,
            ACCENT,
        ))
        .highlight_style(selection_style(ACCENT));

    frame.render_stateful_widget(list, area, &mut app.action_state);
    scrollbar(
        frame,
        area,
        ACTIONS.len(),
        area.height.saturating_sub(2) as usize,
        app.action_state.selected().unwrap_or(0),
    );
}

fn render_backups(frame: &mut Frame, area: Rect, app: &mut App) {
    app.backups_area = area;
    let visible = app.filtered_backup_indices();
    let mut detail = app.selected_backup().map(|backup| {
        let mut parts = Vec::new();
        if !backup.label.is_empty() {
            parts.push(backup.label.clone());
        }
        parts.push(format!("v{} {}", backup.version, backup.compression));
        parts.join(" · ")
    });
    if !app.backup_filter.is_empty() || app.backup_filtering {
        let count = format!("{}/{}", visible.len(), app.snapshot.backups.len());
        let filter = format!("/{}", app.backup_filter);
        detail = Some(match detail {
            Some(detail) => format!("{count} {filter} · {detail}"),
            None => format!("{count} {filter}"),
        });
    }
    let block = pane_block(
        "Backups",
        detail.as_deref(),
        app.focus == Focus::Backups,
        SUCCESS,
    );

    if app.snapshot.backups.is_empty() || visible.is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let [_, middle, _] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(2),
            Constraint::Fill(1),
        ])
        .areas(inner);
        let message = if !app.snapshot.backups.is_empty() {
            format!("No backup matches '{}'", app.backup_filter)
        } else {
            app.snapshot
                .warning_text()
                .unwrap_or_else(|| "No local backups. Press R to refresh.".to_string())
        };
        let (title, message) = if app.loading {
            (
                format!("{} Loading backups", app.spinner().unwrap_or("◐")),
                "Reading manifests and log…".to_string(),
            )
        } else {
            ("No backups here".to_string(), message)
        };
        let paragraph = Paragraph::new(Text::from(vec![
            Line::from(Span::styled(title, fg(MUTED).bold())),
            Line::from(Span::styled(message, fg(MUTED))),
        ]))
        .alignment(Alignment::Center);
        frame.render_widget(paragraph, middle);
        return;
    }

    let max_size = visible
        .iter()
        .filter_map(|index| app.snapshot.backups.get(*index))
        .map(|backup| backup.size)
        .max()
        .unwrap_or(1)
        .max(1);

    let rows: Vec<Row> = visible
        .iter()
        .filter_map(|index| app.snapshot.backups.get(*index))
        .map(|backup| {
            let label = if backup.label.is_empty() {
                String::new()
            } else {
                format!("  {}", backup.label)
            };
            Row::new(vec![
                Cell::from(format!("{}{}", short_name(&backup.name), label)),
                Cell::from(backup_age(backup)),
                Cell::from(human_size(backup.size)),
                Cell::from(size_bar(backup.size, max_size, 6)).style(fg(SUCCESS)),
                Cell::from(backup.packages.to_string()),
            ])
        })
        .collect();
    let visible_len = visible.len();

    let header = Row::new(["NAME", "AGE", "SIZE", "USED", "PKGS"])
        .style(fg(MUTED).bold())
        .bottom_margin(0);
    let widths = [
        Constraint::Min(12),
        Constraint::Length(5),
        Constraint::Length(9),
        Constraint::Length(6),
        Constraint::Length(4),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(selection_style(SUCCESS));

    frame.render_stateful_widget(table, area, &mut app.backup_state);
    scrollbar(
        frame,
        area,
        visible_len,
        area.height.saturating_sub(3) as usize,
        app.backup_state.selected().unwrap_or(0),
    );
}

fn render_output(frame: &mut Frame, area: Rect, app: &mut App) {
    app.output_area = area;
    let detail = match app.output_source {
        OutputSource::Log => format!("log {}", app.output.len()),
        OutputSource::Command => "command".to_string(),
    };
    let block = pane_block("Output", Some(&detail), app.focus == Focus::Output, MAGENTA);
    let inner = block.inner(area);
    let inner_height = inner.height as usize;
    let len = app.output.len();
    let max_offset = len.saturating_sub(inner_height);
    let offset = app.output_offset.min(max_offset);
    let end = len.saturating_sub(offset);
    let start = end.saturating_sub(inner_height);

    let lines: Vec<Line> = app.output[start..end]
        .iter()
        .map(|line| render_log_line(line))
        .collect();

    frame.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
    let position = app.output.len().saturating_sub(1).saturating_sub(offset);
    scrollbar(frame, area, app.output.len(), inner_height, position);
}

fn render_log_line(line: &str) -> Line<'_> {
    let Some(parts) = parse_log_line(line) else {
        if line.starts_with("$ ") {
            return Line::from(Span::styled(line.to_string(), fg(ACCENT).bold()));
        }
        return Line::from(line.to_string());
    };

    Line::from(vec![
        Span::styled(parts.timestamp, fg(MUTED)),
        Span::raw(" "),
        level_chip(parts.level),
        Span::raw(" "),
        Span::raw(parts.message),
    ])
}

fn level_chip(level: &str) -> Span<'static> {
    let (label, color) = match level {
        "INFO" => ("INFO", SUCCESS),
        "WARN" | "WARNING" => ("WARN", WARN),
        "ERROR" | "FAIL" => ("ERROR", DANGER),
        "DEBUG" => ("DEBUG", MUTED),
        other => (other, MUTED),
    };
    // Known levels reuse static strings; only unknown levels need to allocate.
    let no_color = no_color();
    let text = match (no_color, label) {
        (true, "INFO") => Cow::Borrowed("[INFO]"),
        (true, "WARN") => Cow::Borrowed("[WARN]"),
        (true, "ERROR") => Cow::Borrowed("[ERROR]"),
        (true, "DEBUG") => Cow::Borrowed("[DEBUG]"),
        (true, other) => Cow::Owned(format!("[{other}]")),
        (false, "INFO") => Cow::Borrowed(" INFO "),
        (false, "WARN") => Cow::Borrowed(" WARN "),
        (false, "ERROR") => Cow::Borrowed(" ERROR "),
        (false, "DEBUG") => Cow::Borrowed(" DEBUG "),
        (false, other) => Cow::Owned(format!(" {other} ")),
    };
    if no_color {
        Span::styled(text, Style::new().bold())
    } else {
        Span::styled(
            text,
            Style::new()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        )
    }
}

fn render_footer(frame: &mut Frame, area: Rect, app: &mut App) {
    let (marker, marker_style) = if let Some(frame) = app.spinner() {
        (frame, fg(ACCENT).bold())
    } else if !app.snapshot.warning.is_empty() {
        ("●", fg(WARN))
    } else {
        ("●", fg(SUCCESS))
    };
    let status_style = if !app.snapshot.warning.is_empty() {
        fg(WARN)
    } else {
        fg(MUTED)
    };
    let warning = !app.snapshot.warning.is_empty();
    let mut status_spans = vec![Span::styled(format!(" {marker} "), marker_style)];
    if warning {
        status_spans.push(Span::styled("warning: ", fg(WARN).bold()));
    }
    status_spans.push(Span::styled(app.status.clone(), status_style));
    let status = Line::from(status_spans);

    let wide = area.width >= 80;
    let mut hint_spans = vec![
        badge(&app.focus.title().to_uppercase(), ACCENT),
        Span::raw("  "),
    ];
    for (index, (key, action)) in hints(app.focus, wide).iter().enumerate() {
        if index > 0 {
            hint_spans.push(Span::raw("  "));
        }
        if wide {
            hint_spans.push(keycap(key));
        } else {
            hint_spans.push(Span::styled(*key, fg(ACCENT).bold()));
        }
        if !action.is_empty() {
            hint_spans.push(Span::raw(" "));
            hint_spans.push(Span::styled(*action, fg(MUTED)));
        }
    }

    app.header_areas.status = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![status, Line::from(hint_spans)])),
        area,
    );
}

fn hints(focus: Focus, wide: bool) -> &'static [(&'static str, &'static str)] {
    match (focus, wide) {
        (Focus::Actions, true) => &[
            ("↑↓", "move"),
            ("Enter", "run"),
            ("Tab", "focus"),
            ("L", "list"),
            ("h", "history"),
            ("R", "refresh"),
            ("?", "help"),
            ("q", "quit"),
        ],
        (Focus::Actions, false) => &[
            ("↑↓", ""),
            ("Enter", ""),
            ("Tab", ""),
            ("L", ""),
            ("R", ""),
            ("?", ""),
            ("q", ""),
        ],
        (Focus::Backups, true) => &[
            ("↑↓", "move"),
            ("Enter", "restore"),
            ("d", "delete"),
            ("/", "filter"),
            ("Tab", "focus"),
            ("R", "refresh"),
            ("q", "quit"),
        ],
        (Focus::Backups, false) => &[
            ("↑↓", ""),
            ("Enter", ""),
            ("d", ""),
            ("/", ""),
            ("Tab", ""),
            ("R", ""),
            ("q", ""),
        ],
        (Focus::Output, true) => &[
            ("↑↓", "scroll"),
            ("PgUp/PgDn", "page"),
            ("G", "bottom"),
            ("Tab", "focus"),
            ("?", "help"),
            ("q", "quit"),
        ],
        (Focus::Output, false) => &[("↑↓", ""), ("Tab", ""), ("?", ""), ("q", "")],
    }
}

fn render_too_small(frame: &mut Frame, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(WARN));
    let paragraph = Paragraph::new(Text::from(vec![
        Line::from(Span::styled("terminal too small", fg(WARN).bold())),
        Line::from(Span::styled(
            format!(
                "need {}×{} — current {}×{}",
                MIN_WIDTH, MIN_HEIGHT, area.width, area.height
            ),
            fg(MUTED),
        )),
    ]))
    .alignment(Alignment::Center)
    .block(block);
    let [_, middle, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(4),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(paragraph, middle);
}

fn render_history(frame: &mut Frame, area: Rect, app: &App) {
    let popup = centered(
        area,
        area.width.saturating_sub(4).clamp(40, 92),
        22.min(area.height.saturating_sub(2)),
    );
    let mut lines = vec![Line::from(Span::styled(
        "Recent operations",
        fg(ACCENT).bold(),
    ))];
    if app.history.is_empty() {
        lines.push(Line::from(Span::styled("No operations yet.", fg(MUTED))));
    }
    let rows = popup.height.saturating_sub(5) as usize;
    let start = app.history_selected.saturating_sub(rows.saturating_sub(1));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    for (index, entry) in app.history.iter().enumerate().skip(start).take(rows) {
        let selected = index == app.history_selected;
        let style = if selected {
            fg(ACCENT).bold()
        } else {
            fg(Color::Reset)
        };
        lines.push(Line::from(vec![
            Span::styled(if selected { "▸ " } else { "  " }, style),
            Span::styled(format!("{:<8}", entry.kind), style),
            Span::styled(format!("{:<18}", entry.name), style),
            Span::styled(format!("{:<10}", entry.status), fg(MUTED)),
            Span::styled(
                format!("{:<8}", crate::data::human_size(entry.size)),
                fg(MUTED),
            ),
            Span::styled(fmt_millis(entry.duration_ms), fg(MUTED)),
            Span::styled(format!("  {} ago", history_age(now, entry.at)), fg(MUTED)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑↓ select · r retry · Esc close",
        fg(MUTED),
    )));
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(op_block("History", ACCENT)),
        popup,
    );
}

fn render_status(frame: &mut Frame, area: Rect, app: &App) -> usize {
    let Some(report) = app.status_report.as_ref() else {
        return 0;
    };
    let popup = centered(
        area,
        area.width.saturating_sub(4).clamp(24, 72),
        area.height.saturating_sub(2).clamp(10, 24),
    );
    let mut lines = vec![
        Line::from(Span::styled("STATUS", fg(ACCENT).bold())),
        Line::from(vec![
            Span::styled("Storage      ", fg(MUTED)),
            Span::styled(report.storage.to_uppercase(), fg(Color::Reset).bold()),
        ]),
        Line::from(vec![
            Span::styled("Target       ", fg(MUTED)),
            Span::raw(report.target.clone()),
        ]),
        Line::from(vec![
            Span::styled("Compression  ", fg(MUTED)),
            Span::raw(format!(
                "{} · keep {}",
                report.compression, report.max_backups
            )),
        ]),
        Line::from(vec![
            Span::styled("Auto-backup  ", fg(MUTED)),
            Span::raw(report.schedule.clone()),
        ]),
        Line::from(vec![
            Span::styled("Versions     ", fg(MUTED)),
            Span::raw(format!(
                "TUI {} · CLI {}",
                env!("CARGO_PKG_VERSION"),
                report.cli_version.as_deref().unwrap_or("not found")
            )),
        ]),
        Line::from(vec![
            Span::styled("Config       ", fg(MUTED)),
            Span::raw(report.config_path.clone()),
        ]),
        Line::from(vec![
            Span::styled("Log          ", fg(MUTED)),
            Span::raw(report.log_path.clone()),
        ]),
        Line::from(vec![
            Span::styled("History      ", fg(MUTED)),
            Span::raw(report.history_path.clone()),
        ]),
        Line::from(""),
        Line::from(Span::styled("TOOLS", fg(ACCENT).bold())),
    ];
    for tool in &report.tools {
        let (text, color) = match &tool.path {
            Some(path) => (path.display().to_string(), SUCCESS),
            None if tool.required => ("missing".to_string(), DANGER),
            None => ("missing (optional)".to_string(), MUTED),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{:<12}", tool.name), fg(MUTED)),
            Span::styled(text, fg(color)),
        ]));
    }

    let content_width = popup.width.saturating_sub(4);
    let content_height = popup.height.saturating_sub(2) as usize;
    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    let total = paragraph.line_count(content_width);
    let max_scroll = total.saturating_sub(content_height);
    let scroll = app.status_scroll.min(max_scroll);
    let detail = if max_scroll > 0 {
        "↑↓ scroll · Esc close"
    } else {
        "Esc close"
    };

    frame.render_widget(Clear, popup);
    frame.render_widget(
        paragraph
            .block(pane_block("Status", Some(detail), true, ACCENT))
            .scroll((scroll as u16, 0)),
        popup,
    );
    scrollbar(frame, popup, total, content_height, scroll);
    max_scroll
}

fn render_daemon(frame: &mut Frame, area: Rect, app: &mut App) {
    let popup = centered(
        area,
        area.width.saturating_sub(4).clamp(24, 72),
        area.height.saturating_sub(2).clamp(10, 24),
    );
    let boot = if app.daemon_boot_present {
        format!(
            "present {}",
            crate::data::short_path(&crate::data::boot_script_path())
        )
    } else {
        "missing — run termux-sync schedule to create it".to_string()
    };
    let mut lines = vec![
        Line::from(Span::styled("DAEMON", fg(ACCENT).bold())),
        Line::from(vec![
            Span::styled("Auto-backup     ", fg(MUTED)),
            Span::raw(if app.snapshot.config.schedule == "off" {
                "DISABLED".to_string()
            } else {
                format!("ENABLED {}", app.snapshot.config.schedule)
            }),
        ]),
        Line::from(vec![
            Span::styled("Boot script     ", fg(MUTED)),
            Span::raw(boot),
        ]),
        Line::from(vec![
            Span::styled("Running state   ", fg(MUTED)),
            Span::raw("not detectable from the TUI"),
        ]),
        Line::from(""),
        Line::from(Span::styled("Start manually:", fg(ACCENT).bold())),
        Line::from("termux-sync daemon &"),
        Line::from("Or install Termux:Boot so the boot script starts it."),
        Line::from(""),
        Line::from(Span::styled("RECENT ACTIVITY", fg(ACCENT).bold())),
    ];
    if app.daemon_log.is_empty() {
        lines.push(Line::from(Span::styled(
            "no daemon activity in sync.log",
            fg(MUTED),
        )));
    } else {
        lines.extend(app.daemon_log.iter().map(|line| Line::from(line.clone())));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "The TUI does not start or stop the daemon.",
        fg(WARN),
    )));

    let content_width = popup.width.saturating_sub(4);
    let content_height = popup.height.saturating_sub(2) as usize;
    let total = Paragraph::new(Text::from(lines.clone()))
        .wrap(Wrap { trim: false })
        .line_count(content_width);
    let max_scroll = total.saturating_sub(content_height);
    let scroll = app.daemon_scroll.min(max_scroll);
    app.daemon_scroll = scroll;
    app.daemon_scroll_max = max_scroll;
    let detail = if max_scroll > 0 {
        "↑↓ scroll · Esc close"
    } else {
        "Esc close"
    };

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(pane_block("Daemon", Some(detail), true, ACCENT))
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0)),
        popup,
    );
    scrollbar(frame, popup, total, content_height, scroll);
}

fn render_help(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width.saturating_sub(2).clamp(20, 64);
    let height = area.height.saturating_sub(2).clamp(7, 22);
    let popup = centered(area, width, height);
    let mut lines = vec![Line::from(vec![
        Span::styled("◆ ", fg(ACCENT).bold()),
        Span::styled("HELP", fg(ACCENT).bold()),
    ])];
    for (key, action) in [
        ("↑ / ↓", "move selection"),
        ("Enter", "run action or restore selected backup"),
        ("Click", "run action, select backup, focus pane"),
        ("d", "delete selected backup"),
        ("Tab", "switch pane"),
        ("R", "refresh backups and status"),
        ("L", "list backups, including remote"),
        ("s", "status"),
        ("/", "filter backups"),
        ("h", "operation history"),
        ("g / G", "top / bottom of output"),
        ("[ / ]", "fewer / more log lines (Output pane)"),
        ("?  / Esc", "close this help"),
        ("↑ / ↓", "scroll this help"),
        ("Ctrl+C", "quit immediately"),
        ("q", "quit"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("  {key:<9}"), fg(ACCENT).bold()),
            Span::styled(action, fg(MUTED)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Actions", fg(ACCENT).bold())));
    lines.push(Line::from(Span::styled(
        "  Backup, restore, delete, list, logs, clear logs, status,",
        fg(MUTED),
    )));
    lines.push(Line::from(Span::styled(
        "  disk check, Export/import config, daemon, setup,",
        fg(MUTED),
    )));
    lines.push(Line::from(Span::styled(
        "  schedule, clear cache.",
        fg(MUTED),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Operations", fg(ACCENT).bold())));
    lines.push(Line::from(Span::styled(
        "  Esc cancel · d diff · 1/2/3 section · / filter",
        fg(MUTED),
    )));
    lines.push(Line::from(Span::styled(
        "  r retry · type RESTORE then Enter to confirm",
        fg(MUTED),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Setup and schedule show a handoff card before the terminal changes.",
        fg(MUTED),
    )));

    let inner_height = popup.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(inner_height);
    let scroll = app.help_scroll.min(max_scroll);
    let reveal = if app.reduce_motion {
        lines.len()
    } else {
        (lines.len() as f32 * ease_out(app.help_progress())).ceil() as usize
    };
    let visible: Vec<Line> = lines
        .into_iter()
        .skip(scroll)
        .take(inner_height.min(reveal))
        .collect();
    let detail = (max_scroll > 0).then_some("↑↓ scroll");

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(Text::from(visible)).block(pane_block("Help", detail, true, ACCENT)),
        popup,
    );
}

fn render_task(frame: &mut Frame, area: Rect, app: &App) {
    let Some(task) = &app.task else {
        return;
    };
    let progress = ease_out(app.task_progress());
    let max_width = area.width.saturating_sub(2).max(20) as f32;
    let max_height = area.height.saturating_sub(2).max(6) as f32;
    let width = (32.0 + 32.0 * progress).min(max_width) as u16;
    let height = (6.0 + 2.0 * progress).min(max_height) as u16;
    let popup = centered(area, width, height);

    let (title, marker, color, detail) = if task.done {
        ("DONE", "✓", SUCCESS, "Captured output is ready")
    } else {
        (
            "WORKING",
            app.spinner().unwrap_or("◐"),
            ACCENT,
            "Capturing command output…",
        )
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("◆ ", fg(color).bold()),
            Span::styled(title, fg(color).bold()),
        ]),
        Line::from(vec![
            Span::styled(format!("{marker} "), fg(color).bold()),
            Span::styled(task.label.clone(), fg(SUCCESS).bold()),
        ]),
        Line::from(Span::styled(detail, fg(MUTED))),
        sweep_bar(
            (width.saturating_sub(4) as usize).clamp(8, 24),
            app.tick,
            task.done,
        ),
    ];

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(pane_block(title, None, true, color)),
        popup,
    );
}

fn sweep_bar(width: usize, tick: u64, done: bool) -> Line<'static> {
    let (filled, empty, color) = if done {
        ("▰", "", SUCCESS)
    } else if no_color() {
        ("#", "-", SUCCESS)
    } else {
        ("▰", "▱", ACCENT)
    };
    if done {
        return Line::from(Span::styled(filled.repeat(width), fg(color)));
    }

    let cycle = (width + 6) as u64;
    let head = (tick % cycle) as usize;
    let mut bar = String::with_capacity(width);
    for index in 0..width {
        let lit = if head >= 6 {
            (head - 6..head).contains(&index)
        } else {
            index < head || index >= width.saturating_sub(6 - head)
        };
        bar.push_str(if lit { filled } else { empty });
    }
    Line::from(Span::styled(bar, fg(color)))
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn fg(color: Color) -> Style {
    if no_color() {
        Style::new()
    } else {
        Style::new().fg(color)
    }
}

fn selection_style(color: Color) -> Style {
    if no_color() {
        Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::new()
            .fg(Color::Black)
            .bg(color)
            .add_modifier(Modifier::BOLD)
    }
}

fn scrollbar(frame: &mut Frame, area: Rect, content: usize, viewport: usize, position: usize) {
    if content <= viewport || viewport == 0 {
        return;
    }
    let mut state = ScrollbarState::new(content)
        .position(position.min(content.saturating_sub(1)))
        .viewport_content_length(viewport);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(fg(MUTED))
            .begin_symbol(None)
            .end_symbol(None),
        area,
        &mut state,
    );
}

fn count_text(text: &str, color: Color) -> Span<'static> {
    if no_color() {
        Span::styled(format!("[{text}]"), Style::new().bold())
    } else {
        Span::styled(text.to_string(), fg(color).bold())
    }
}

fn badge(text: &str, color: Color) -> Span<'static> {
    if no_color() {
        Span::styled(format!("[{text}]"), Style::new().bold())
    } else {
        Span::styled(
            format!(" {text} "),
            Style::new()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        )
    }
}

fn keycap(key: &str) -> Span<'static> {
    if no_color() {
        Span::styled(format!("[{key}]"), Style::new().bold())
    } else {
        Span::styled(
            format!(" {key} "),
            Style::new()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
    }
}

fn gradient_spans(text: &str) -> Vec<Span<'static>> {
    if no_color() {
        return vec![Span::styled(text.to_string(), Style::new().bold())];
    }
    let chars: Vec<char> = text.chars().collect();
    let last = chars.len().saturating_sub(1).max(1) as f32;
    chars
        .into_iter()
        .enumerate()
        .map(|(index, ch)| {
            let t = index as f32 / last;
            let color = lerp_color(GRADIENT_FROM, GRADIENT_TO, t);
            Span::styled(ch.to_string(), Style::new().fg(color).bold())
        })
        .collect()
}

fn lerp_color(from: Color, to: Color, t: f32) -> Color {
    let (Color::Rgb(fr, fg, fb), Color::Rgb(tr, tg, tb)) = (from, to) else {
        return from;
    };
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color::Rgb(mix(fr, tr), mix(fg, tg), mix(fb, tb))
}

fn short_name(name: &str) -> &str {
    name.strip_prefix("termux_backup_").unwrap_or(name)
}

fn size_bar(size: u64, max: u64, width: usize) -> String {
    if max == 0 {
        return " ".repeat(width);
    }
    let filled = ((size as f64 / max as f64) * width as f64).round() as usize;
    let filled = filled.clamp(0, width);
    if no_color() {
        format!("{}{}", "#".repeat(filled), "-".repeat(width - filled))
    } else {
        format!("{}{}", "▰".repeat(filled), "▱".repeat(width - filled))
    }
}

fn no_color() -> bool {
    static NO_COLOR: OnceLock<bool> = OnceLock::new();
    *NO_COLOR.get_or_init(|| std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()))
}

fn pane_block(title: &str, detail: Option<&str>, focused: bool, color: Color) -> Block<'static> {
    let style = if focused {
        fg(color).add_modifier(Modifier::BOLD)
    } else if no_color() {
        Style::new()
    } else {
        fg(color).add_modifier(Modifier::DIM)
    };
    let marker = if focused { "▸" } else { " " };
    let mut title_spans = vec![Span::styled(format!(" {marker} {title} "), style)];
    if let Some(detail) = detail {
        title_spans.push(Span::styled(format!("· {detail} "), fg(MUTED)));
    }

    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(title_spans))
        .border_style(style)
        .padding(Padding::horizontal(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Backup, Config, Snapshot};
    use crate::ops::{Kind, Outcome, Status, test_operation, test_plan};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;

    fn app() -> App {
        App::with_snapshot(Snapshot {
            config: Config::default(),
            backups: vec![Backup {
                name: "termux_backup_20260101_120000".to_string(),
                date: "2026-01-01 12:00".to_string(),
                label: "before-upgrade".to_string(),
                packages: 3,
                size: 3072,
                version: "1.3.9".to_string(),
                compression: "GZ".to_string(),
                modified: None,
            }],
            log: vec!["2026-01-01 12:00 [INFO] Backup completed".to_string()],
            warning: Vec::new(),
        })
    }

    fn render_at(width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = app();
        for _ in 0..6 {
            app.on_tick();
        }
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw succeeds");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn layout_plan_has_floor_and_single_pane_band() {
        assert_eq!(plan(Rect::new(0, 0, 47, 12)), LayoutPlan::TooSmall);
        assert_eq!(plan(Rect::new(0, 0, 48, 11)), LayoutPlan::TooSmall);
        assert_eq!(plan(Rect::new(0, 0, 48, 12)), LayoutPlan::Single);
        assert_eq!(plan(Rect::new(0, 0, 79, 24)), LayoutPlan::Single);
        assert_eq!(plan(Rect::new(0, 0, 80, 24)), LayoutPlan::Split);
    }

    #[test]
    fn wide_frame_shows_hero_stats_panes_and_detail() {
        let frame = render_at(100, 30);
        assert!(frame.contains("TERMUX SYNC"));
        assert!(frame.contains("BACKUPS"));
        assert!(frame.contains("STORED"));
        assert!(frame.contains("PACKAGES"));
        assert!(frame.contains("LOCAL"));
        assert!(frame.contains("Actions"));
        assert!(frame.contains("Output"));
        assert!(frame.contains("20260101_120000"));
        assert!(frame.contains("v1.3.9 GZ"));
        assert!(frame.contains("╭"));
        assert!(frame.contains("↑"));
        assert!(frame.contains("≡"));
    }

    #[test]
    fn ascii_mode_uses_ascii_action_icons() {
        let mut app = app();
        app.ascii = true;
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("+  Backup now"));
        assert!(frame.contains("=  List backups"));
        assert!(!frame.contains("≡  List backups"));
    }

    #[test]
    fn header_fits_at_56_columns_with_github_and_warning() {
        let mut app = app();
        app.snapshot.config.storage = "github".into();
        app.snapshot.backups.clear();
        app.snapshot.warning = vec!["GITHUB storage".into()];
        let frame = render_app(&mut app, 56, 24);
        assert!(frame.contains("GITHUB"));
        assert!(frame.contains("0 BACKUPS"));
        assert!(frame.contains("WARN"));
    }

    #[test]
    fn setup_handoff_card_renders() {
        let mut app = app();
        app.run(crate::app::Action::Setup);
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("Handoff"));
        assert!(frame.contains("termux-sync"));
        assert!(frame.contains("Enter"));
    }

    #[test]
    fn history_window_follows_the_selected_entry() {
        let mut app = app();
        app.history = (0..20)
            .map(|n| crate::history::HistoryEntry {
                kind: "backup".into(),
                name: format!("entry-{n}"),
                status: "succeeded".into(),
                size: n,
                duration_ms: n,
                verified: None,
                at: n,
            })
            .collect();
        app.history_selected = 19;
        app.show_history = true;
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("entry-19"));
        assert!(!frame.contains("entry-0"));
    }

    #[test]
    fn backup_scrollbar_renders_when_list_overflows() {
        let mut app = app();
        let base = app.snapshot.backups[0].clone();
        app.snapshot.backups = (0..30)
            .map(|index| crate::data::Backup {
                name: format!("termux_backup_{index:04}"),
                ..base.clone()
            })
            .collect();
        let frame = render_app(&mut app, 100, 24);
        assert!(frame.contains("█") || frame.contains("║"));
    }

    #[test]
    fn history_overlay_renders_entries() {
        let mut app = app();
        app.history = vec![crate::history::HistoryEntry {
            kind: "backup".to_string(),
            name: "termux_backup_test".to_string(),
            status: "succeeded".to_string(),
            size: 1024,
            duration_ms: 2000,
            verified: Some(true),
            at: 1,
        }];
        app.show_history = true;
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("History"));
        assert!(frame.contains("termux_backup_test"));
        assert!(frame.contains("retry"));
    }

    #[test]
    fn backup_filter_render_shows_matching_rows_and_count() {
        let mut app = app();
        app.backup_filter = "upgrade".into();
        app.backup_state.select(Some(0));
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("1/1 /upgrade"));
        assert!(frame.contains("20260101_120000"));
        assert!(frame.contains("before-upgrade"));
    }

    #[test]
    fn backup_label_prompt_renders() {
        let mut app = app();
        app.run(crate::app::Action::Backup);
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("Backup label"));
        assert!(frame.contains("Used in the manifest and backup name."));
        assert!(frame.contains("Enter"));
    }

    #[test]
    fn help_lists_parity_actions() {
        let mut app = app();
        app.open_help();
        for _ in 0..6 {
            app.on_tick();
        }
        app.help_scroll = usize::MAX;
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("Clear logs"));
        assert!(frame.contains("Export/import config"));
        assert!(frame.contains("Daemon"));
    }

    #[test]
    fn status_and_daemon_modals_document_escape_to_close() {
        let mut app = app();
        app.show_status = true;
        app.status_report = Some(crate::data::StatusReport {
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
        });
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("Esc close"));

        app.show_status = false;
        app.open_daemon();
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("Esc close"));
    }

    #[test]
    fn daemon_disabled_schedule_does_not_repeat_off() {
        let mut app = app();
        app.snapshot.config.schedule = "off".into();
        app.open_daemon();
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("DISABLED"));
        assert!(!frame.contains("DISABLED off"));
    }

    #[test]
    fn input_prompt_keeps_buttons_at_minimum_size() {
        let mut app = app();
        app.run(Action::DiskCheck);
        let frame = render_app(&mut app, 48, 12);
        assert!(frame.contains("continue"));
        assert!(frame.contains("cancel"));

        app.input_text = "x".repeat(200);
        let frame = render_app(&mut app, 48, 12);
        assert!(frame.contains("continue"));
        assert!(frame.contains("cancel"));
    }

    #[test]
    fn status_modal_scrolls_to_the_last_tool_at_minimum_size() {
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
            tools: ["python3", "dpkg", "tar", "rclone", "git", "curl"]
                .into_iter()
                .map(|name| crate::data::ToolStatus {
                    name,
                    path: None,
                    required: name != "rclone",
                })
                .collect(),
        });
        app.status_scroll = usize::MAX;
        let frame = render_app(&mut app, 48, 12);
        assert!(frame.contains("curl"));
    }

    #[test]
    fn daemon_modal_scrolls_to_the_safety_line_at_minimum_size() {
        let mut app = app();
        app.open_daemon();
        app.daemon_scroll = usize::MAX;
        let frame = render_app(&mut app, 48, 12);
        assert!(frame.contains("does not start or stop"));
    }

    #[test]
    fn daemon_modal_names_the_command_that_creates_the_boot_script() {
        let mut app = app();
        app.open_daemon();
        app.daemon_boot_present = false;
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("termux-sync schedule"));
    }

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
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("STATUS"));
        assert!(frame.contains("GITHUB"));
        assert!(frame.contains("me/backups @ main"));
        assert!(frame.contains("rclone"));
        assert!(frame.contains("missing"));
    }

    #[test]
    fn output_title_shows_the_visible_log_line_count() {
        let mut app = app();
        app.output = vec!["line".to_string(); 120];
        app.log_lines = 200;
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("log 120"));
        assert!(!frame.contains("log 200"));
    }

    #[test]
    fn daemon_modal_states_it_does_not_control_the_process() {
        let mut app = app();
        app.open_daemon();
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("DAEMON"));
        assert!(frame.contains("termux-sync daemon &"));
        assert!(frame.contains("does not start or stop"));
    }

    #[test]
    fn remote_warning_does_not_claim_list_is_outside() {
        let mut app = app();
        app.snapshot.config.storage = "github".into();
        app.snapshot.warning = vec!["GITHUB storage: press L to list backups in the TUI".into()];
        let frame = render_app(&mut app, 80, 24);
        assert!(!frame.contains("in the terminal"));
    }

    #[test]
    fn warning_state_labels_the_warn_chip_and_status() {
        let mut app = app();
        app.snapshot.warning = vec!["GITHUB storage".to_string()];
        app.status = "GITHUB storage".to_string();
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("WARN"));
        assert!(frame.contains("warning:"));
    }

    #[test]
    fn narrow_frame_drops_cards_but_keeps_hero() {
        let frame = render_at(60, 20);
        assert!(frame.contains("TERMUX SYNC"));
        assert!(frame.contains("ACTIONS"));
        assert!(!frame.contains("STORED"));
        assert!(!frame.contains("Backups"));
    }

    #[test]
    fn output_pane_styles_log_lines() {
        let frame = render_at(100, 30);
        assert!(frame.contains("INFO"));
        assert!(frame.contains("Backup completed"));
    }

    #[test]
    fn too_small_frame_names_the_minimum() {
        let frame = render_at(40, 10);
        assert!(frame.contains("terminal too small"));
        assert!(frame.contains("need 48×12"));
    }

    #[test]
    fn help_overlay_renders_over_dashboard() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = app();
        app.open_help();
        for _ in 0..5 {
            app.on_tick();
        }
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw succeeds");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("HELP"));
        assert!(content.contains("switch pane"));
    }

    #[test]
    fn backup_names_drop_the_app_prefix() {
        assert_eq!(
            short_name("termux_backup_20260101_120000"),
            "20260101_120000"
        );
        assert_eq!(short_name("custom-name"), "custom-name");
    }

    #[test]
    fn task_overlay_renders_for_captured_commands() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = app();
        app.task = Some(crate::app::TaskOverlay {
            label: "termux-sync list".to_string(),
            started_at: 0,
            done: false,
            done_at: 0,
            visible: true,
        });
        app.on_tick();
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw succeeds");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("WORKING"));
        assert!(content.contains("termux-sync list"));
    }

    #[test]
    fn help_scrolls_at_the_minimum_size() {
        let backend = TestBackend::new(48, 12);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = app();
        app.open_help();
        for _ in 0..5 {
            app.on_tick();
        }
        for _ in 0..20 {
            app.handle_key(KeyCode::Down);
        }
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw succeeds");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("clear cache"));
        assert!(content.contains("↑↓ scroll"));
    }

    #[test]
    fn loading_state_names_what_is_happening() {
        let mut app = app();
        app.snapshot.backups.clear();
        app.loading = true;
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw succeeds");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("LOADING"));
        assert!(content.contains("Loading backups"));
    }

    #[test]
    fn standard_size_keeps_all_panes() {
        let frame = render_at(80, 24);
        assert!(frame.contains("TERMUX SYNC"));
        assert!(frame.contains("Actions"));
        assert!(frame.contains("Backups"));
        assert!(frame.contains("Output"));
        assert!(frame.contains("ACTIONS"));
    }

    fn operation_app(kind: Kind, status: Status) -> App {
        let mut app = app();
        let mut operation = test_operation(kind);
        operation.status = status;
        operation.plan = Some(test_plan());
        operation
            .diff
            .apply("to_install", 1, 1, vec!["pkg-a".into(), "pkg-b".into()]);
        operation.diff.apply(
            "already_installed",
            1,
            1,
            vec!["git".into(), "python".into()],
        );
        operation
            .diff
            .apply("device_only", 1, 1, vec!["vim".into()]);
        app.operation = Some(operation);
        app.screen = crate::app::Screen::Operation;
        app
    }

    fn render_app(app: &mut App, width: u16, height: u16) -> String {
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

    #[test]
    fn restore_plan_screen_shows_warning_and_input() {
        let mut app = operation_app(Kind::Restore, Status::AwaitingConfirm);
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("Restore plan"));
        assert!(frame.contains("Type RESTORE"));
        assert!(frame.contains("Reinstalls all 1947 packages"));
        assert!(frame.contains("1827 already installed"));
    }

    #[test]
    fn restore_plan_fits_minimum_with_warning() {
        let mut app = operation_app(Kind::Restore, Status::AwaitingConfirm);
        let frame = render_app(&mut app, 48, 12);
        assert!(frame.contains("Reinstalls all 1947"));
        assert!(frame.contains("Type RESTORE"));
    }

    #[test]
    fn package_diff_screen_has_persistent_warning() {
        let mut app = operation_app(Kind::Restore, Status::AwaitingConfirm);
        app.operation.as_mut().unwrap().diff_open = true;
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("reinstalls all 1947 packages"));
        assert!(frame.contains("device-only"));
        assert!(frame.contains("TO INSTALL"));
        assert!(frame.contains("DEVICE ONLY"));
        assert!(frame.contains("pkg-a"));
    }

    #[test]
    fn protected_restore_screen_says_no_cancel() {
        let mut app = operation_app(Kind::Restore, Status::Running);
        app.operation.as_mut().unwrap().protected = true;
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("CANNOT CANCEL SAFELY"));
        assert!(frame.contains("CANNOT CANCEL"));
    }

    #[test]
    fn restore_result_requires_manual_restart() {
        let mut app = operation_app(Kind::Restore, Status::Succeeded);
        app.operation.as_mut().unwrap().result = Some(Outcome {
            ok: true,
            name: "termux_backup_test".to_string(),
            size: 22_300_000_000,
            packages: 1947,
            packages_installed: 1947,
            archives: 2,
            restart_required: true,
            duration_ms: 3_100,
            ..Outcome::default()
        });
        let frame = render_app(&mut app, 100, 30);
        assert!(frame.contains("SUCCESS"));
        assert!(frame.contains("Close and reopen Termux"));
    }

    #[test]
    fn size_bar_fills_proportionally() {
        assert_eq!(size_bar(0, 100, 4), "▱▱▱▱");
        assert_eq!(size_bar(50, 100, 4), "▰▰▱▱");
        assert_eq!(size_bar(100, 100, 4), "▰▰▰▰");
    }
}
