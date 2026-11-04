//! The terminal's side of review's messages. A key becomes the action the key map gives it, and a
//! click the action of the row or button under it, which `update::update` takes as it would any
//! UI's; the editor's typing, the box of keys, the terminal's size and the spinner are the
//! screen's own. After every message the detail and the lists are scrolled to fit, whatever moved
//! the selection.

use crossterm::event::KeyCode;
use jiff::{SignedDuration, Timestamp};
use ratatui::layout::{Position, Rect};

use super::keymap::{self, Press};
use super::layout;
use super::model::{Entry, Model, Pane};
use super::update::{self, Action, Effect, Input};
use super::view;

/// What the screen's loop takes: an input for `update::update`, one of the terminal's events, or
/// the spinner's tick.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // as `Input`, which it carries: a read-back is rare
pub enum Msg {
    /// A job's step, or an effect's result.
    Input(Input),
    Key(KeyCode),
    /// Ctrl-U: in the editor, delete back to the start.
    KillLine,
    /// Ctrl-C or Ctrl-D.
    Interrupt,
    /// The terminal's new size.
    Resize(u16, u16),
    /// A mouse action at a column and row of the screen.
    Mouse(Mouse, u16, u16),
    /// No message came for a quarter of a second while a job runs: the spinner turns.
    Tick,
}

impl From<Input> for Msg {
    fn from(input: Input) -> Msg {
        Msg::Input(input)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mouse {
    Click,
    WheelUp,
    WheelDown,
}

/// Two clicks on one entry within this long are a double-click.
const DOUBLE_CLICK: SignedDuration = SignedDuration::from_millis(500);

/// Lines the detail pane scrolls per notch of the wheel.
const WHEEL_LINES: u16 = 3;

/// Take one message, and give the effects to run for it.
pub fn update(m: &mut Model, msg: Msg, now: Timestamp) -> Vec<Effect> {
    let before = (m.selected_entry(), m.pane, m.file_selected);
    let input = matches!(
        msg,
        Msg::Key(_) | Msg::KillLine | Msg::Interrupt | Msg::Mouse(..)
    );
    // A double-click is two clicks with nothing the user did in between: a key or another click
    // starts over. A job's messages and the spinner's ticks come unasked, so they leave the first
    // click be (`Model::refresh` keeps it on its entry as rows come and go).
    let last_click = if input { m.last_click.take() } else { None };
    let effects = match msg {
        Msg::Input(given) => update::update(m, given, now),
        Msg::Key(key) => key_press(m, key, now),
        Msg::KillLine => {
            kill_line(m);
            Vec::new()
        }
        Msg::Interrupt => act(m, Action::Interrupt, now),
        Msg::Resize(width, height) => {
            m.set_area(Rect::new(0, 0, width, height));
            Vec::new()
        }
        Msg::Mouse(action, x, y) => mouse(m, action, Position::new(x, y), last_click, now),
        Msg::Tick => {
            m.progress.ticks += 1;
            Vec::new()
        }
    };
    // The cost line review opened with ends at the first key (Ctrl-U, Ctrl-C and Ctrl-D too),
    // click or turn of the wheel, the box of keys open or not: once the message is handled, so a
    // click lands on the status line as it was drawn.
    if input {
        m.show_cost = false;
    }
    if (m.selected_entry(), m.pane, m.file_selected) != before {
        m.detail_scroll = 0;
    }
    // No further than the detail's last line, however that moved: the wheel, a wider screen, a
    // decision or a filter that shortened the text.
    m.detail_scroll = m.detail_scroll.min(detail_last(m));
    // The list scrolls no further than it takes to keep the selection in view.
    m.scroll = m.list_scroll(m.area);
    m.file_scroll = m.files_scroll(m.area);
    effects
}

/// `action`, taken by `update::update` as from any UI.
fn act(m: &mut Model, action: Action, now: Timestamp) -> Vec<Effect> {
    update::update(m, Input::Action(action), now)
}

/// A key: what the key map makes of it ([`keymap::press`]).
fn key_press(m: &mut Model, key: KeyCode, now: Timestamp) -> Vec<Effect> {
    match keymap::press(m, key) {
        Press::Act(action) => act(m, action, now),
        Press::OpenHelp => {
            m.help = true;
            act(m, Action::Dismiss, now)
        }
        Press::CloseHelp(then) => {
            m.help = false;
            then.map_or_else(Vec::new, |action| act(m, action, now))
        }
        Press::Type(key) => {
            edit_text(m, key);
            Vec::new()
        }
    }
}

/// Ctrl-U: in the editor, delete back to the start.
fn kill_line(m: &mut Model) {
    if let Some(ed) = &mut m.editor {
        ed.text = ed.text.chars().skip(ed.cursor).collect();
        ed.cursor = 0;
    }
}

/// A key the editor takes: a character typed at the cursor, Backspace or Delete taking one out on
/// either side of it, or a move of the cursor. Any other does nothing.
fn edit_text(m: &mut Model, key: KeyCode) {
    let Some(ed) = &mut m.editor else {
        return;
    };
    let at = |text: &str, cursor: usize| {
        text.char_indices()
            .nth(cursor)
            .map_or(text.len(), |(i, _)| i)
    };
    match key {
        KeyCode::Char(c) => {
            let i = at(&ed.text, ed.cursor);
            ed.text.insert(i, c);
            ed.cursor += 1;
        }
        KeyCode::Backspace if ed.cursor > 0 => {
            ed.cursor -= 1;
            let i = at(&ed.text, ed.cursor);
            ed.text.remove(i);
        }
        KeyCode::Delete if ed.cursor < ed.text.chars().count() => {
            let i = at(&ed.text, ed.cursor);
            ed.text.remove(i);
        }
        KeyCode::Left => ed.cursor = ed.cursor.saturating_sub(1),
        KeyCode::Right => ed.cursor = (ed.cursor + 1).min(ed.text.chars().count()),
        KeyCode::Home => ed.cursor = 0,
        KeyCode::End => ed.cursor = ed.text.chars().count(),
        _ => {}
    }
}

/// The furthest the detail pane scrolls: its last line, wrapped as drawn, on top.
fn detail_last(m: &Model) -> u16 {
    // Inside the border, where the lines wrap.
    let width = layout::panes(m.area).detail.width.saturating_sub(2);
    let lines = view::detail_height(m, width);
    u16::try_from(lines.saturating_sub(1)).unwrap_or(u16::MAX)
}

/// A click does what its key does; the wheel moves the selection over the list and scrolls the
/// detail pane. While the editor is open only its own buttons work, so a stray click cannot lose
/// the edit; while the box of keys shows, a click closes it and the wheel does nothing.
fn mouse(
    m: &mut Model,
    action: Mouse,
    at: Position,
    last_click: Option<(Entry, Timestamp)>,
    now: Timestamp,
) -> Vec<Effect> {
    let p = layout::panes(m.area);
    let press = |m: &mut Model, key: Option<KeyCode>| match key {
        Some(key) => key_press(m, key, now),
        None => Vec::new(),
    };
    if m.help {
        m.help = !matches!(action, Mouse::Click);
        return Vec::new();
    }
    if m.editor.is_some() {
        return match action {
            Mouse::Click => press(m, status_button(m, p.status, at)),
            _ => Vec::new(),
        };
    }
    match action {
        Mouse::WheelUp if p.list.contains(at) => press(m, Some(KeyCode::Up)),
        Mouse::WheelDown if p.list.contains(at) => press(m, Some(KeyCode::Down)),
        Mouse::WheelUp if p.detail.contains(at) => {
            m.detail_scroll = m.detail_scroll.saturating_sub(WHEEL_LINES);
            Vec::new()
        }
        Mouse::WheelDown if p.detail.contains(at) => {
            // `screen::update` stops it at the last line.
            m.detail_scroll = m.detail_scroll.saturating_add(WHEEL_LINES);
            Vec::new()
        }
        Mouse::Click if layout::list_rows(&p).contains(at) => {
            let row = usize::from(at.y - layout::list_rows(&p).y);
            match m.pane {
                Pane::Findings => click_row(m, m.scroll + row, last_click, now),
                Pane::Files => {
                    let file = m.file_scroll + row;
                    if file < m.progress.files.len() {
                        act(m, Action::SelectFile(file), now)
                    } else {
                        Vec::new()
                    }
                }
            }
        }
        Mouse::Click => press(m, status_button(m, p.status, at)),
        _ => Vec::new(),
    }
}

/// The key the button at `at` on the status line `status` presses, while the buttons show.
fn status_button(m: &Model, status: Rect, at: Position) -> Option<KeyCode> {
    let items = m.buttons(status).filter(|_| status.contains(at))?;
    layout::bar_hit(items, layout::bar_left(status, items), at.x)
}

/// A click on the list's `index`th entry selects it; the second click of a double-click opens it.
fn click_row(
    m: &mut Model,
    index: usize,
    last_click: Option<(Entry, Timestamp)>,
    now: Timestamp,
) -> Vec<Effect> {
    let Some(&entry) = m.entries().get(index) else {
        return Vec::new();
    };
    let mut effects = act(m, Action::Select(index), now);
    // The wall clock can step back between two clicks: a negative gap is not a short one.
    let double = last_click.is_some_and(|(e, at)| {
        e == entry && (SignedDuration::ZERO..=DOUBLE_CLICK).contains(&now.duration_since(at))
    });
    if double {
        effects.extend(act(m, Action::Play, now));
    } else {
        m.last_click = Some((entry, now));
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::tests_support::{finding, path, sample};
    use crate::review::update::tests::{decided, now, saved, scanning, states};
    use crate::scan::Event;
    use crate::store::State;

    /// `msg` as the screen's loop takes it.
    fn send(m: &mut Model, msg: impl Into<Msg>, now: Timestamp) -> Vec<Effect> {
        update(m, msg.into(), now)
    }

    fn key(m: &mut Model, code: KeyCode) -> Vec<Effect> {
        send(m, Msg::Key(code), now())
    }

    fn press(m: &mut Model, c: char) -> Vec<Effect> {
        key(m, KeyCode::Char(c))
    }

    /// `sample()` on a 110×20 screen: the list's rows are y 1 to 17 from x 1, the detail pane is
    /// x 55 on, and the status line y 19, with `? help` from x 104.
    fn on_screen() -> Model {
        let mut m = sample();
        send(&mut m, Msg::Resize(110, 20), now());
        m
    }

    /// A click at `x`, `y`, `ms` milliseconds after `now()`.
    fn click(m: &mut Model, x: u16, y: u16, ms: i64) -> Vec<Effect> {
        let at = now() + SignedDuration::from_millis(ms);
        send(m, Msg::Mouse(Mouse::Click, x, y), at)
    }

    fn wheel(m: &mut Model, action: Mouse, x: u16, y: u16) {
        assert_eq!(send(m, Msg::Mouse(action, x, y), now()), []);
    }

    #[test]
    fn the_status_line_follows_the_scan_then_gives_its_summary() {
        let status = |m: &Model| m.status_text(Rect::new(0, 0, 160, 1));
        let mut m = scanning();
        assert_eq!(status(&m).as_deref(), Some("scan 0/2 · 0 findings"));
        let failed = Event::Finished {
            path: path(4).into(),
            error: Some("LLM: boom".into()),
            findings: 0,
            lines: vec![],
        };
        send(&mut m, Input::Scan(failed), now());
        assert_eq!(
            status(&m).as_deref(),
            Some("scan 1/2 · 0 findings · 1 failed")
        );
        // A message of its own takes the line until the next key.
        m.status = Some("opened in mpv".into());
        assert_eq!(status(&m).as_deref(), Some("opened in mpv"));
        press(&mut m, 'j');
        let summary = crate::scan::Summary {
            run: "r".into(),
            files: 2,
            checked: 1,
            failed: 1,
            ..crate::scan::Summary::default()
        };
        send(&mut m, Input::ScanEnded(Ok(summary)), now());
        assert_eq!(
            status(&m).as_deref(),
            Some(
                "run r: 2 files, 1 checked, 1 failed, 0 unchanged since an earlier scan; 0 findings"
            )
        );
        send(&mut m, Msg::Tick, now());
        assert_eq!(m.progress.ticks, 1);
    }

    #[test]
    fn tab_shows_the_files_and_enter_goes_to_a_files_first_finding() {
        let mut m = sample();
        m.selected = 1;
        key(&mut m, KeyCode::Tab);
        assert_eq!(m.pane, Pane::Files);
        press(&mut m, 'j');
        assert_eq!(m.file_selected, 1);
        // The decision and mpv keys belong to the findings.
        assert_eq!(press(&mut m, 'a'), []);
        assert_eq!(press(&mut m, 'o'), []);
        // E02's first entry is the lyric's row, which has its finding.
        key(&mut m, KeyCode::Enter);
        assert_eq!((m.pane, m.selected), (Pane::Findings, 0));
        key(&mut m, KeyCode::Tab);
        key(&mut m, KeyCode::Tab);
        assert_eq!(m.pane, Pane::Findings);
    }

    #[test]
    fn editing_accepts_with_the_new_wording() {
        let mut m = sample();
        m.selected = 1;
        press(&mut m, 'e');
        assert_eq!(
            m.editor.as_ref().map(|e| e.text.as_str()),
            Some("Let's have snacks together every day.")
        );
        // Typed keys are text, `q` included.
        key(&mut m, KeyCode::Home);
        key(&mut m, KeyCode::Delete);
        press(&mut m, 'q');
        assert!(!m.should_quit);
        key(&mut m, KeyCode::Backspace);
        press(&mut m, 'l');
        let effects = key(&mut m, KeyCode::Enter);
        assert_eq!(
            decided(&effects),
            [(
                3,
                State::Accepted,
                Some("let's have snacks together every day.")
            )]
        );
        assert!(m.editor.is_none());
    }

    #[test]
    fn in_the_editor_playback_keys_edit() {
        let mut m = sample();
        m.selected = 1;
        press(&mut m, 'e');
        assert_eq!(key(&mut m, KeyCode::Left), []);
        assert_eq!(press(&mut m, ' '), []);
        assert_eq!(press(&mut m, '.'), []);
        assert_eq!(
            m.editor.as_ref().map(|e| e.text.as_str()),
            Some("Let's have snacks together every day ..")
        );
    }

    #[test]
    fn a_click_selects_the_entry_under_it() {
        let mut m = on_screen();
        key(&mut m, KeyCode::Enter);
        // Row 0, its members E01 E02 E03, then row 1: y 3 is E02's member.
        assert_eq!(click(&mut m, 5, 3, 0), []);
        assert_eq!(m.selected_entry(), Some(Entry::Member(0, 1)));
        // The border, the title, below the last entry, the detail pane: nothing.
        for (x, y) in [(0, 2), (5, 0), (5, 9), (70, 2)] {
            click(&mut m, x, y, 1000);
            assert_eq!(m.selected_entry(), Some(Entry::Member(0, 1)), "{x},{y}");
        }
    }

    #[test]
    fn a_click_lands_on_the_entry_drawn_there_when_the_list_is_scrolled() {
        let mut m = crate::review::model::tests_support::many(30);
        send(&mut m, Msg::Resize(110, 20), now());
        key(&mut m, KeyCode::End);
        // 17 rows, ending at entry 29: entry 13 is on the first row.
        assert_eq!(m.scroll, 13);
        click(&mut m, 5, 1, 0);
        assert_eq!(m.selected, 13);
        click(&mut m, 5, 17, 1000);
        assert_eq!(m.selected, 29);
    }

    #[test]
    fn the_first_click_lands_on_the_row_drawn_there_when_review_opens_scrolled() {
        // Review opened on the first pending row, entry 25, before any message: the screen it
        // first draws is scrolled to show it.
        let mut m = crate::review::model::tests_support::many(30);
        m.selected = 25;
        m.set_area(Rect::new(0, 0, 110, 20));
        assert_eq!(m.scroll, 9);
        click(&mut m, 5, 1, 0);
        assert_eq!(m.selected, 9);
    }

    #[test]
    fn a_double_click_opens_and_a_slow_or_moved_one_does_not() {
        let mut m = on_screen();
        let opens = |effects: Vec<Effect>| matches!(effects[..], [Effect::Open(_)]);
        assert_eq!(click(&mut m, 5, 2, 0), []);
        assert!(opens(click(&mut m, 5, 2, 500)), "within 500 ms");
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)));
        // A third click starts over.
        assert_eq!(click(&mut m, 5, 2, 600), []);
        // Too slow.
        assert_eq!(click(&mut m, 5, 2, 2000), []);
        assert_eq!(click(&mut m, 5, 2, 2501), []);
        // Two rows.
        assert_eq!(click(&mut m, 5, 1, 4000), []);
        assert_eq!(click(&mut m, 5, 2, 4100), []);
        // Something in between.
        assert_eq!(click(&mut m, 5, 2, 6000), []);
        key(&mut m, KeyCode::Char('f'));
        assert_eq!(click(&mut m, 5, 2, 6100), []);
        // The clock stepped back between them (NTP, a resume): not a double-click.
        assert_eq!(click(&mut m, 5, 2, 8000), []);
        assert_eq!(click(&mut m, 5, 2, 7000), []);
    }

    #[test]
    fn a_double_click_survives_the_scans_messages_between_its_clicks() {
        let opens = |effects: Vec<Effect>| matches!(effects[..], [Effect::Open(_)]);
        // The spinner's tick, which comes after 250 ms without a message while a job runs.
        let mut m = on_screen();
        assert_eq!(click(&mut m, 5, 2, 0), []);
        send(&mut m, Msg::Tick, now());
        assert!(opens(click(&mut m, 5, 2, 300)), "a tick in between");
        // A file's findings, a row of them above the snacks row, which moves down a line: the
        // second click lands on it there.
        let mut m = on_screen();
        assert_eq!(click(&mut m, 5, 2, 0), []);
        let msg = saved(
            &m,
            1,
            vec![finding(5, 1, "An erly line.", "An early line.", 1000)],
        );
        send(&mut m, msg, now());
        assert_eq!(m.entries()[2], Entry::Row(2));
        assert!(opens(click(&mut m, 5, 3, 300)), "a row in above it");
    }

    #[test]
    fn the_wheel_moves_the_selection_over_the_list_and_scrolls_the_detail() {
        let mut m = on_screen();
        wheel(&mut m, Mouse::WheelDown, 10, 5);
        assert_eq!(m.selected, 1);
        wheel(&mut m, Mouse::WheelUp, 10, 5);
        assert_eq!(m.selected, 0);
        // The ×3 row's detail is 11 lines: the wheel stops with the last one on top.
        for _ in 0..5 {
            wheel(&mut m, Mouse::WheelDown, 70, 5);
        }
        assert_eq!(m.detail_scroll, 10);
        wheel(&mut m, Mouse::WheelUp, 70, 5);
        assert_eq!(m.detail_scroll, 7);
        // A new selection starts at the top.
        key(&mut m, KeyCode::Down);
        assert_eq!(m.detail_scroll, 0);
    }

    #[test]
    fn a_detail_that_got_shorter_is_scrolled_back_to_its_last_line() {
        let mut m = on_screen();
        // Scrolled for a longer text: a decision or a filter can shorten it while the same entry
        // stays selected.
        m.detail_scroll = 100;
        send(&mut m, Input::Played(Err("mpv is not open".into())), now());
        assert_eq!(
            m.detail_scroll, 10,
            "the ×3 row's 11 lines, the last on top"
        );
    }

    #[test]
    fn question_mark_shows_the_keys_and_any_key_closes_them() {
        let mut m = on_screen();
        assert!(!m.help);
        press(&mut m, '?');
        assert!(m.help);
        // Closing is all the key does: no decision.
        assert_eq!(press(&mut m, 'a'), []);
        assert!(!m.help);
        assert_eq!(states(&m), [State::Pending; 4]);
        press(&mut m, '?');
        key(&mut m, KeyCode::Esc);
        assert!(!m.help);
        // But `q` quits, as the box says.
        press(&mut m, '?');
        press(&mut m, 'q');
        assert!(m.should_quit);
        m.should_quit = false;
        // `? help` at the end of the status line opens them; a click anywhere closes them, and the
        // wheel does nothing under them.
        click(&mut m, 105, 19, 0);
        assert!(m.help);
        wheel(&mut m, Mouse::WheelDown, 10, 5);
        assert_eq!(m.selected, 0);
        click(&mut m, 5, 2, 1000);
        assert!(!m.help);
        assert_eq!(m.selected, 0, "the click only closed them");
    }

    #[test]
    fn while_editing_only_the_editors_buttons_work() {
        let mut m = on_screen();
        m.selected = 1;
        press(&mut m, 'e');
        assert_eq!(click(&mut m, 5, 1, 0), []);
        wheel(&mut m, Mouse::WheelUp, 5, 1);
        assert_eq!(click(&mut m, 50, 19, 1000), [], "the text");
        assert_eq!(click(&mut m, 98, 19, 1500), [], "the separator");
        assert_eq!(m.selected, 1);
        assert!(m.editor.is_some());
        // "⏎ accept · Esc cancel" at the end of the status line: Esc from x 100.
        click(&mut m, 100, 19, 2000);
        assert!(m.editor.is_none());
        press(&mut m, 'e');
        assert_eq!(
            decided(&click(&mut m, 89, 19, 3000)),
            [(3, State::Accepted, None)]
        );
    }

    #[test]
    fn c_in_the_findings_does_nothing() {
        let mut m = sample();
        assert_eq!(press(&mut m, 'c'), []);
        assert_eq!(m.confirm, None);
    }
}
