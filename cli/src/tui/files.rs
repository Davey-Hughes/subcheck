//! The files view (`Tab`): one line per file of the run, with where the scan, or apply, is with it;
//! and in the detail pane, what the scan and apply said of the selected one.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use super::screen::Screen;
use subcheck_core::review::model::{Pane, Review};
use subcheck_core::review::progress::{ApplyStage, FileProgress, Stage};

/// The files view of the left pane: one line per file of the run, with its stage, and how many
/// files there are on the bottom border.
pub fn files_pane(m: &Review, s: &Screen, f: &mut Frame, area: Rect) {
    let p = &m.progress;
    let spinner = s.spinner();
    let block = super::view::pane_block(m, spinner, Pane::Files, area)
        .title_bottom(Line::from(format!(" {} files ", p.files.len())).right_aligned());
    if p.files.is_empty() {
        f.render_widget(Paragraph::new("No files yet.").block(block), area);
        return;
    }
    let items: Vec<ListItem> = p
        .files
        .iter()
        .map(|file| ListItem::new(line(m, file, spinner)))
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_symbol("▶ ")
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default()
        .with_selected(Some(m.file_selected))
        .with_offset(s.files_scroll(m, f.area()));
    f.render_stateful_widget(list, area, &mut state);
}

/// A file's line: its name in the list and its stage (apply's when `applying` is set), coloured:
/// done green, failed red, queued grey, under way the terminal's own.
/// An apply failure's paths are written short ([`FileProgress::list_label`]).
fn line(m: &Review, file: &FileProgress, spinner: char) -> Line<'static> {
    let fg = |c| Style::default().fg(c);
    // Apply's stage when `applying` is set, else the scan's.
    let style = match (&file.applying, &file.stage) {
        (Some(ApplyStage::Done { .. }), _) | (None, Stage::Done { .. }) => fg(Color::Green),
        (Some(ApplyStage::Failed(_)), _) | (None, Stage::Failed(_)) => fg(Color::Red),
        (Some(ApplyStage::Queued), _) | (None, Stage::Queued | Stage::QueuedAgain) => {
            fg(Color::DarkGray)
        }
        (Some(ApplyStage::At(_)), _) | (None, Stage::Reading | Stage::Asking { .. }) => {
            Style::default()
        }
    };
    let name = m.label(&file.path);
    let stage = file.list_label(spinner, &name);
    Line::styled(format!("{name}  {stage}"), style)
}

/// The detail pane's lines for the selected file: its path, its stage (`spinner` turning beside
/// one under way), its log, then its apply log ([`FileProgress::apply_log`]).
pub fn detail_lines(m: &Review, spinner: char) -> Vec<Line<'static>> {
    let Some(file) = m.progress.files.get(m.file_selected) else {
        return Vec::new();
    };
    let mut lines = vec![
        Line::styled(file.path.clone(), Style::default().fg(Color::DarkGray)),
        Line::from(file.label(spinner)),
        Line::default(),
    ];
    lines.extend(file.log.iter().map(|l| Line::from(l.clone())));
    if !file.apply_log.is_empty() {
        lines.push(Line::default());
        lines.extend(file.apply_log.iter().map(|l| Line::from(l.clone())));
    }
    lines
}
