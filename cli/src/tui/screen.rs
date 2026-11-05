//! The terminal's side of review: what its screen keeps beside the review ([`Screen`]), and how
//! the screen's messages reach the review. A key becomes the action the key map gives it, and a
//! click the action of the row or button under it, which `update::update` takes as it would any
//! UI's; the editor's typing, the box of keys, the terminal's size and the spinner are the
//! screen's own. After every message the detail and the lists are scrolled to fit, whatever moved
//! the selection.

use crossterm::event::KeyCode;
use jiff::{SignedDuration, Timestamp};
use ratatui::layout::{Position, Rect};

use super::keymap::{self, Press};
use super::layout::{self, EDIT_KEYS, HELP, Item};
use super::view;
use subcheck_core::llm::Spent;
use subcheck_core::review::model::{EntryKey, Pane, Review};
use subcheck_core::review::update::{self, Action, Effect, Input};

/// What the screen's loop takes: an input for `update::update`, one of the terminal's events, or
/// the spinner's tick.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // as `Input`, which it carries: a read-back is rare
pub enum Msg {
    /// A job's step, or an effect's result. Never an action: one comes as the key or click that
    /// asks for it, which the screen's own state follows (a double-click, the cost line).
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

/// The spinner beside a stage under way, a step per tick.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The one-line editor `e` opens, while the review edits an entry ([`Review::editing`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editor {
    pub text: String,
    /// In chars.
    pub cursor: usize,
}

/// What the terminal's screen keeps beside the review, which knows nothing of it: the terminal's
/// size, how far each pane is scrolled, the first click of a double-click, the box of keys, the
/// cost line review opens with, the editor's text, and the spinner.
pub struct Screen {
    /// The screen, as the terminal last reported it: where a click lands depends on it.
    pub area: Rect,
    /// The list's first shown entry.
    pub scroll: usize,
    /// The files list's first shown line.
    pub file_scroll: usize,
    /// How far the wheel has scrolled the detail pane, in lines; back to 0 on a new selection.
    pub detail_scroll: u16,
    /// The last click on the list, while it could still be the first of a double-click.
    pub last_click: Option<(EntryKey, Timestamp)>,
    /// Whether the box of keys `?` opens is showing.
    pub help: bool,
    /// Whether the status line, with no message of its own, says what the run's LLM calls cost:
    /// from opening until the first key, click or turn of the wheel.
    pub show_cost: bool,
    /// The editor, while the review edits an entry: its text and cursor are the screen's.
    pub editor: Option<Editor>,
    /// The loop's ticks while a job runs (`Msg::Tick`): the spinner's step.
    pub ticks: usize,
}

impl Default for Screen {
    /// The screen review opens on: not sized yet, nothing scrolled, the cost line showing.
    fn default() -> Screen {
        Screen {
            area: Rect::default(),
            scroll: 0,
            file_scroll: 0,
            detail_scroll: 0,
            last_click: None,
            help: false,
            show_cost: true,
            editor: None,
            ticks: 0,
        }
    }
}

impl Screen {
    /// A screen of `area`, scrolled to show `m`'s selection as the view will draw it: a click maps
    /// rows with `scroll` before any message has moved it.
    pub fn set_area(&mut self, m: &Review, area: Rect) {
        self.area = area;
        self.scroll = self.list_scroll(m, area);
    }

    /// The list's scroll offset on a screen of `area`: `scroll`, moved just enough to show `m`'s
    /// selection. The view draws with it and `screen::update` keeps it.
    pub fn list_scroll(&self, m: &Review, area: Rect) -> usize {
        let rows = layout::list_rows(&layout::panes(area)).height;
        let len = m.entries().len();
        layout::keep_in_view(self.scroll, m.selected, usize::from(rows), len)
    }

    /// The files list's scroll offset on a screen of `area`, as [`Screen::list_scroll`] is
    /// the findings'.
    pub fn files_scroll(&self, m: &Review, area: Rect) -> usize {
        let rows = layout::list_rows(&layout::panes(area)).height;
        layout::keep_in_view(
            self.file_scroll,
            m.file_selected,
            usize::from(rows),
            m.progress.files.len(),
        )
    }

    /// The buttons at the end of the status line `status`, when they show there: the editor's while
    /// it is open, beside its prompt and [`layout::EDIT_FIELD`] columns of text; else, or while a
    /// question takes the line from the editor, `? help`, beside the message.
    pub fn buttons(&self, m: &Review, status: Rect) -> Option<&'static [Item]> {
        let (items, left) = match &self.editor {
            Some(_) if m.confirm.is_none() => (
                EDIT_KEYS,
                layout::width(layout::PROMPT) + layout::EDIT_FIELD,
            ),
            _ => (
                HELP,
                self.status_text(m, status).map_or(0, |s| layout::width(&s)),
            ),
        };
        layout::right_shown(status, items, left).then_some(items)
    }

    /// The spinner's step, beside what a job is doing.
    pub fn spinner(&self) -> char {
        SPINNER[self.ticks % SPINNER.len()]
    }

    /// What the status line `status` says: the review's own text ([`Review::status_text`]); else
    /// what the run's LLM calls cost while [`Screen::show_cost`] holds, as much of it as fits
    /// beside `? help`.
    pub fn status_text(&self, m: &Review, status: Rect) -> Option<String> {
        m.status_text().or_else(|| {
            self.show_cost
                .then(|| cost_status(&m.run.spent, status))
                .flatten()
        })
    }
}

/// What the run's LLM calls cost, for the status line `status`: all of it if it fits beside
/// `? help`, else without the tokens, else the price alone, which is what it is there for. The
/// shortest when none fits. `None` with no calls recorded.
fn cost_status(spent: &Spent, status: Rect) -> Option<String> {
    let calls = spent.calls_part()?;
    let (tokens, price) = (spent.tokens_part(), spent.price_part());
    let shapes = [
        vec![Some(calls.clone()), tokens, price.clone()],
        vec![Some(calls), price.clone()],
        vec![price],
    ];
    let lines: Vec<String> = shapes
        .into_iter()
        .filter_map(|parts| {
            let parts: Vec<String> = parts.into_iter().flatten().collect();
            (!parts.is_empty()).then(|| format!("LLM: {}", parts.join(" · ")))
        })
        .collect();
    lines
        .iter()
        .find(|line| layout::right_shown(status, HELP, layout::width(line)))
        .or(lines.last())
        .cloned()
}

/// Take one message, and give the effects to run for it.
pub fn update(m: &mut Review, s: &mut Screen, msg: Msg, now: Timestamp) -> Vec<Effect> {
    let before = (m.selected_entry(), m.pane, m.file_selected);
    let by_user = matches!(
        msg,
        Msg::Key(_) | Msg::KillLine | Msg::Interrupt | Msg::Mouse(..)
    );
    // A double-click is two clicks with nothing the user did in between: a key or another click
    // starts over. A job's messages and the spinner's ticks come unasked, so they leave the first
    // click be (`follow` keeps it on its entry as rows come and go).
    let last_click = if by_user { s.last_click.take() } else { None };
    let effects = match msg {
        Msg::Input(input) => follow(m, s, input, now),
        Msg::Key(key) => key_press(m, s, key, now),
        Msg::KillLine => {
            kill_line(s);
            Vec::new()
        }
        Msg::Interrupt => act(m, Action::Interrupt, now),
        Msg::Resize(width, height) => {
            s.set_area(m, Rect::new(0, 0, width, height));
            Vec::new()
        }
        Msg::Mouse(kind, x, y) => mouse(m, s, kind, Position::new(x, y), last_click, now),
        Msg::Tick => {
            s.ticks += 1;
            Vec::new()
        }
    };
    // The editor shows while the review edits an entry: it opens on the edit's wording
    // (`Review::edit_wording`), the cursor at its end, and closes when the edit ends, however it
    // ended (a refresh can end it).
    match (m.editing.is_some(), s.editor.is_some()) {
        (true, false) => {
            s.editor = m.edit_wording().map(|text| Editor {
                text: text.to_string(),
                cursor: text.chars().count(),
            });
        }
        (false, true) => s.editor = None,
        _ => {}
    }
    // The cost line review opened with ends at the first key (Ctrl-U, Ctrl-C and Ctrl-D too),
    // click or turn of the wheel, the box of keys open or not: once the message is handled, so a
    // click lands on the status line as it was drawn.
    if by_user {
        s.show_cost = false;
    }
    if (m.selected_entry(), m.pane, m.file_selected) != before {
        s.detail_scroll = 0;
    }
    // No further than the detail's last line, however that moved: the wheel, a wider screen, a
    // decision or a filter that shortened the text.
    s.detail_scroll = s.detail_scroll.min(detail_last(m, s));
    // The list scrolls no further than it takes to keep the selection in view.
    s.scroll = s.list_scroll(m, s.area);
    s.file_scroll = s.files_scroll(m, s.area);
    effects
}

/// A job's step or an effect's result, taken by `update::update`; then the screen follows what it
/// did to the list.
fn follow(m: &mut Review, s: &mut Screen, input: Input, now: Timestamp) -> Vec<Effect> {
    let switched = matches!(input, Input::Switched(Ok(_)));
    let effects = update::update(m, input, now);
    if switched {
        // Another view: its lists and its detail start at the top, no click came in it yet, and
        // the spinner starts over.
        s.scroll = 0;
        s.file_scroll = 0;
        s.detail_scroll = 0;
        s.last_click = None;
        s.ticks = 0;
        return effects;
    }
    // The rows came or went around the selection: the list scrolls as far as the selection moved,
    // so the selection stays on the line where the user was looking.
    if let Some(was) = m.kept_from {
        s.scroll = (s.scroll + m.selected).saturating_sub(was);
    }
    // The first click of a double-click stays on its entry, which the rows may have moved.
    s.last_click = s
        .last_click
        .take()
        .and_then(|(key, at)| Some((m.entry_key(m.resolve(&key)?), at)));
    effects
}

/// `action`, taken by `update::update` as from any UI.
fn act(m: &mut Review, action: Action, now: Timestamp) -> Vec<Effect> {
    update::update(m, Input::Action(action), now)
}

/// A key: what the key map makes of it ([`keymap::press`]).
fn key_press(m: &mut Review, s: &mut Screen, key: KeyCode, now: Timestamp) -> Vec<Effect> {
    match keymap::press(m, s, key) {
        Press::Act(action) => act(m, action, now),
        Press::OpenHelp => {
            s.help = true;
            act(m, Action::Dismiss, now)
        }
        Press::CloseHelp(then) => {
            s.help = false;
            then.map_or_else(Vec::new, |action| act(m, action, now))
        }
        Press::Type(key) => {
            edit_text(s, key);
            Vec::new()
        }
    }
}

/// Ctrl-U: in the editor, delete back to the start.
fn kill_line(s: &mut Screen) {
    if let Some(ed) = &mut s.editor {
        ed.text = ed.text.chars().skip(ed.cursor).collect();
        ed.cursor = 0;
    }
}

/// A key the editor takes: a character typed at the cursor, Backspace or Delete taking one out on
/// either side of it, or a move of the cursor. Any other does nothing.
fn edit_text(s: &mut Screen, key: KeyCode) {
    let Some(ed) = &mut s.editor else {
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
fn detail_last(m: &Review, s: &Screen) -> u16 {
    // Inside the border, where the lines wrap.
    let width = layout::panes(s.area).detail.width.saturating_sub(2);
    let lines = view::detail_height(m, s.spinner(), width);
    u16::try_from(lines.saturating_sub(1)).unwrap_or(u16::MAX)
}

/// A click does what its key does; the wheel moves the selection over the list and scrolls the
/// detail pane. While the editor is open only its own buttons work, so a stray click cannot lose
/// the edit; while the box of keys shows, a click closes it and the wheel does nothing.
fn mouse(
    m: &mut Review,
    s: &mut Screen,
    kind: Mouse,
    at: Position,
    last_click: Option<(EntryKey, Timestamp)>,
    now: Timestamp,
) -> Vec<Effect> {
    let p = layout::panes(s.area);
    let press = |m: &mut Review, s: &mut Screen, key: Option<KeyCode>| match key {
        Some(key) => key_press(m, s, key, now),
        None => Vec::new(),
    };
    if s.help {
        s.help = !matches!(kind, Mouse::Click);
        return Vec::new();
    }
    if s.editor.is_some() {
        return match kind {
            Mouse::Click => {
                let key = status_button(m, s, p.status, at);
                press(m, s, key)
            }
            _ => Vec::new(),
        };
    }
    match kind {
        Mouse::WheelUp if p.list.contains(at) => press(m, s, Some(KeyCode::Up)),
        Mouse::WheelDown if p.list.contains(at) => press(m, s, Some(KeyCode::Down)),
        Mouse::WheelUp if p.detail.contains(at) => {
            s.detail_scroll = s.detail_scroll.saturating_sub(WHEEL_LINES);
            Vec::new()
        }
        Mouse::WheelDown if p.detail.contains(at) => {
            // `screen::update` stops it at the last line.
            s.detail_scroll = s.detail_scroll.saturating_add(WHEEL_LINES);
            Vec::new()
        }
        Mouse::Click if layout::list_rows(&p).contains(at) => {
            let row = usize::from(at.y - layout::list_rows(&p).y);
            match m.pane {
                Pane::Findings => {
                    let index = s.scroll + row;
                    click_row(m, s, index, last_click, now)
                }
                Pane::Files => match m.progress.files.get(s.file_scroll + row) {
                    Some(file) => {
                        let path = file.path.clone();
                        act(m, Action::SelectFile(path), now)
                    }
                    None => Vec::new(),
                },
            }
        }
        Mouse::Click => {
            let key = status_button(m, s, p.status, at);
            press(m, s, key)
        }
        _ => Vec::new(),
    }
}

/// The key the button at `at` on the status line `status` presses, while the buttons show.
fn status_button(m: &Review, s: &Screen, status: Rect, at: Position) -> Option<KeyCode> {
    let items = s.buttons(m, status).filter(|_| status.contains(at))?;
    layout::bar_hit(items, layout::bar_left(status, items), at.x)
}

/// A click on the list's `index`th entry selects it; the second click of a double-click opens it.
fn click_row(
    m: &mut Review,
    s: &mut Screen,
    index: usize,
    last_click: Option<(EntryKey, Timestamp)>,
    now: Timestamp,
) -> Vec<Effect> {
    let Some(&entry) = m.entries().get(index) else {
        return Vec::new();
    };
    let key = m.entry_key(entry);
    let mut effects = act(m, Action::Select(key.clone()), now);
    // The wall clock can step back between two clicks: a negative gap is not a short one.
    let double = last_click.is_some_and(|(clicked, at)| {
        clicked == key && (SignedDuration::ZERO..=DOUBLE_CLICK).contains(&now.duration_since(at))
    });
    if double {
        effects.extend(act(m, Action::Play, now));
    } else {
        s.last_click = Some((key, now));
    }
    effects
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use subcheck_core::list::Filter;
    use subcheck_core::player::PlayerCommand;
    use subcheck_core::review::model::Entry;
    use subcheck_core::review::model::tests_support::{self, finding, path, sample};
    use subcheck_core::review::update::tests_support::{
        FIXED, TYPO, decide_as, decided, now, saved, scanning, states, taken,
    };
    use subcheck_core::scan::Event;
    use subcheck_core::store::{Run, State};

    /// `msg` as the screen's loop takes it.
    pub(crate) fn send(
        m: &mut Review,
        s: &mut Screen,
        msg: impl Into<Msg>,
        now: Timestamp,
    ) -> Vec<Effect> {
        update(m, s, msg.into(), now)
    }

    fn key(m: &mut Review, s: &mut Screen, code: KeyCode) -> Vec<Effect> {
        send(m, s, Msg::Key(code), now())
    }

    fn press(m: &mut Review, s: &mut Screen, c: char) -> Vec<Effect> {
        key(m, s, KeyCode::Char(c))
    }

    /// `sample()` on a 110×20 screen: the list's rows are y 1 to 17 from x 1, the detail pane is
    /// x 55 on, and the status line y 19, with `? help` from x 104.
    fn on_screen() -> (Review, Screen) {
        let mut m = sample();
        let mut s = Screen::default();
        send(&mut m, &mut s, Msg::Resize(110, 20), now());
        (m, s)
    }

    /// A click at `x`, `y`, `ms` milliseconds after `now()`.
    fn click(m: &mut Review, s: &mut Screen, x: u16, y: u16, ms: i64) -> Vec<Effect> {
        let at = now() + SignedDuration::from_millis(ms);
        send(m, s, Msg::Mouse(Mouse::Click, x, y), at)
    }

    fn wheel(m: &mut Review, s: &mut Screen, kind: Mouse, x: u16, y: u16) {
        assert_eq!(send(m, s, Msg::Mouse(kind, x, y), now()), []);
    }

    #[test]
    fn the_status_line_follows_the_scan_then_gives_its_summary() {
        let status = |m: &Review| m.status_text();
        let mut m = scanning();
        let mut s = Screen::default();
        assert_eq!(status(&m).as_deref(), Some("scan 0/2 · 0 findings"));
        let failed = Event::Finished {
            path: path(4).into(),
            error: Some("LLM: boom".into()),
            findings: 0,
            lines: vec![],
        };
        send(&mut m, &mut s, Input::Scan(failed), now());
        assert_eq!(
            status(&m).as_deref(),
            Some("scan 1/2 · 0 findings · 1 failed")
        );
        // A message of its own takes the line until the next key.
        m.status = Some("opened in mpv".into());
        assert_eq!(status(&m).as_deref(), Some("opened in mpv"));
        press(&mut m, &mut s, 'j');
        let summary = subcheck_core::scan::Summary {
            run: "r".into(),
            files: 2,
            checked: 1,
            failed: 1,
            ..subcheck_core::scan::Summary::default()
        };
        send(&mut m, &mut s, Input::ScanEnded(Ok(summary)), now());
        assert_eq!(
            status(&m).as_deref(),
            Some(
                "run r: 2 files, 1 checked, 1 failed, 0 unchanged since an earlier scan; 0 findings"
            )
        );
        send(&mut m, &mut s, Msg::Tick, now());
        assert_eq!((s.ticks, s.spinner()), (1, '⠙'));
    }

    #[test]
    fn tab_shows_the_files_and_enter_goes_to_a_files_first_finding() {
        let mut m = sample();
        let mut s = Screen::default();
        m.selected = 1;
        key(&mut m, &mut s, KeyCode::Tab);
        assert_eq!(m.pane, Pane::Files);
        press(&mut m, &mut s, 'j');
        assert_eq!(m.file_selected, 1);
        // The decision and mpv keys belong to the findings.
        assert_eq!(press(&mut m, &mut s, 'a'), []);
        assert_eq!(press(&mut m, &mut s, 'o'), []);
        // E02's first entry is the lyric's row, which has its finding.
        key(&mut m, &mut s, KeyCode::Enter);
        assert_eq!((m.pane, m.selected), (Pane::Findings, 0));
        key(&mut m, &mut s, KeyCode::Tab);
        key(&mut m, &mut s, KeyCode::Tab);
        assert_eq!(m.pane, Pane::Findings);
    }

    #[test]
    fn editing_accepts_with_the_new_wording() {
        let mut m = sample();
        let mut s = Screen::default();
        m.selected = 1;
        press(&mut m, &mut s, 'e');
        assert_eq!(
            s.editor.as_ref().map(|e| e.text.as_str()),
            Some("Let's have snacks together every day.")
        );
        // Typed keys are text, `q` included.
        key(&mut m, &mut s, KeyCode::Home);
        key(&mut m, &mut s, KeyCode::Delete);
        press(&mut m, &mut s, 'q');
        assert!(!m.should_quit);
        key(&mut m, &mut s, KeyCode::Backspace);
        press(&mut m, &mut s, 'l');
        let effects = key(&mut m, &mut s, KeyCode::Enter);
        assert_eq!(
            decided(&effects),
            [(
                3,
                State::Accepted,
                Some("let's have snacks together every day.")
            )]
        );
        assert!(s.editor.is_none());
    }

    /// E02 as a re-check reads it back: its record, and `findings`.
    fn e02(findings: Vec<subcheck_core::store::Finding>) -> Input {
        Input::ReadBack {
            path: path(2),
            view: Ok(Run {
                files: vec![tests_support::file(2)],
                findings,
                ..Run::default()
            }),
        }
    }

    /// E02's snacks line, as finding `id`.
    fn snacks(id: u64) -> subcheck_core::store::Finding {
        finding(
            id,
            2,
            "Let's have snacks together everyday.",
            "Let's have snacks together every day.",
            894_190,
        )
    }

    #[test]
    fn a_refresh_keeps_the_editor_on_its_finding_and_closes_it_when_the_finding_went() {
        let mut m = sample();
        let mut s = Screen::default();
        m.selected = 1;
        press(&mut m, &mut s, 'e');
        press(&mut m, &mut s, '!');
        // E02 checked again: its snacks line comes back as finding 9, and the edit goes on.
        let lyric = finding(2, 2, TYPO, FIXED, 165_060);
        send(&mut m, &mut s, e02(vec![lyric.clone(), snacks(9)]), now());
        assert_eq!(
            s.editor.as_ref().map(|e| e.text.as_str()),
            Some("Let's have snacks together every day.!")
        );
        // Checked again without it: the edit ends, and the status line says why.
        send(&mut m, &mut s, e02(vec![lyric]), now());
        assert_eq!((&m.editing, &s.editor), (&None, &None));
        assert_eq!(
            m.status.as_deref(),
            Some("E02 was checked again; its finding changed")
        );
    }

    #[test]
    fn a_row_edit_decides_only_its_finding_once_a_recheck_decides_the_rest_of_its_row() {
        let mut m = sample();
        let mut s = Screen::default();
        // E01's and E03's lyric accepted as written: `e` on the lyric row edits E02's 2.
        decide_as(&mut m, &[1, 4], State::Accepted, None);
        press(&mut m, &mut s, 'e');
        press(&mut m, &mut s, '!');
        // E02 checked again: 2 comes back as 9, which takes the row's decision, so no member of
        // the row is pending.
        let back = e02(vec![finding(9, 2, TYPO, FIXED, 165_060), snacks(3)]);
        let effects = send(&mut m, &mut s, back, now());
        send(&mut m, &mut s, taken(effects), now());
        let reworded = format!("{FIXED}!");
        assert_eq!(
            decided(&key(&mut m, &mut s, KeyCode::Enter)),
            [(9, State::Accepted, Some(reworded.as_str()))]
        );
    }

    #[test]
    fn a_row_edit_ends_when_its_finding_went_though_the_row_stays() {
        let mut m = sample();
        let mut s = Screen::default();
        decide_as(&mut m, &[1, 4], State::Accepted, None);
        press(&mut m, &mut s, 'e');
        // E02 checked again without its lyric: E01's and E03's keep the row in the list.
        send(&mut m, &mut s, e02(vec![snacks(3)]), now());
        assert_eq!(m.entries(), [Entry::Row(0), Entry::Row(1)]);
        assert_eq!((&m.editing, &s.editor), (&None, &None));
        assert_eq!(
            m.status.as_deref(),
            Some("E02 was checked again; its finding changed")
        );
    }

    #[test]
    fn a_row_edit_leaves_out_a_finding_that_went_and_decides_the_rest() {
        let mut m = sample();
        let mut s = Screen::default();
        // The lyric row pending in E01, E02 and E03: `e` edits all three.
        press(&mut m, &mut s, 'e');
        press(&mut m, &mut s, '!');
        // E02 checked again without its lyric: the edit goes on with E01's and E03's.
        send(&mut m, &mut s, e02(vec![snacks(3)]), now());
        assert_eq!(
            m.editing.as_ref().map(|e| e.findings.as_slice()),
            Some(&[1, 4][..])
        );
        assert!(s.editor.is_some());
        let reworded = format!("{FIXED}!");
        assert_eq!(
            decided(&key(&mut m, &mut s, KeyCode::Enter)),
            [
                (1, State::Accepted, Some(reworded.as_str())),
                (4, State::Accepted, Some(reworded.as_str()))
            ]
        );
    }

    #[test]
    fn a_row_edit_leaves_a_member_that_joined_while_it_was_open_pending() {
        let mut m = scanning();
        let mut s = Screen::default();
        press(&mut m, &mut s, 'e');
        press(&mut m, &mut s, '!');
        // The scan saves E04, whose lyric joins the row, pending like the rest.
        let joined = saved(&m, 4, vec![finding(30, 4, TYPO, FIXED, 165_060)]);
        assert_eq!(send(&mut m, &mut s, joined, now()), []);
        let effects = key(&mut m, &mut s, KeyCode::Enter);
        let reworded = format!("{FIXED}!");
        assert_eq!(
            decided(&effects),
            [1, 2, 4].map(|id| (id, State::Accepted, Some(reworded.as_str())))
        );
        send(&mut m, &mut s, taken(effects), now());
        assert_eq!(
            m.status.as_deref(),
            Some("accepted 3 findings, 1 kept as pending")
        );
    }

    #[test]
    fn a_row_edit_decides_its_finding_once_a_recheck_takes_the_row_out_of_the_list() {
        let mut m = sample();
        let mut s = Screen::default();
        // Under Pending, the lyric row shows for E02's finding 2 alone.
        decide_as(&mut m, &[1, 4], State::Accepted, None);
        m.filter = Filter::Only(State::Pending);
        press(&mut m, &mut s, 'e');
        press(&mut m, &mut s, '!');
        // E02 checked again while the editor is open: 2 comes back as 9, which takes the row's
        // decision, so the filter shows none of the row's members.
        let back = e02(vec![finding(9, 2, TYPO, FIXED, 165_060), snacks(3)]);
        let effects = send(&mut m, &mut s, back, now());
        send(&mut m, &mut s, taken(effects), now());
        assert_eq!(m.entries(), [Entry::Row(1)]);
        let reworded = format!("{FIXED}!");
        assert_eq!(
            decided(&key(&mut m, &mut s, KeyCode::Enter)),
            [(9, State::Accepted, Some(reworded.as_str()))]
        );
        assert_eq!((&m.editing, &s.editor), (&None, &None));
    }

    #[test]
    fn a_row_edit_decides_every_new_finding_of_its_findings_carry_key() {
        let mut m = sample();
        let mut s = Screen::default();
        // E02 sings the lyric twice: 2 and 7 have one carry key.
        let lyric = |id, ms| finding(id, 2, TYPO, FIXED, ms);
        let twice = e02(vec![lyric(2, 165_060), lyric(7, 200_000), snacks(3)]);
        send(&mut m, &mut s, twice, now());
        decide_as(&mut m, &[1, 4], State::Accepted, None);
        press(&mut m, &mut s, 'e');
        press(&mut m, &mut s, '!');
        // Checked again, both come back with new IDs, each the replacement of both.
        let back = e02(vec![lyric(9, 165_060), lyric(10, 200_000), snacks(3)]);
        let effects = send(&mut m, &mut s, back, now());
        send(&mut m, &mut s, taken(effects), now());
        let reworded = format!("{FIXED}!");
        assert_eq!(
            decided(&key(&mut m, &mut s, KeyCode::Enter)),
            [
                (9, State::Accepted, Some(reworded.as_str())),
                (10, State::Accepted, Some(reworded.as_str()))
            ]
        );
    }

    #[test]
    fn a_member_edit_decides_its_findings_replacement_once_a_recheck_hides_it() {
        let mut m = sample();
        let mut s = Screen::default();
        // Under Pending, the lyric row expanded shows E02's finding 2 as its one member.
        decide_as(&mut m, &[1, 4], State::Accepted, None);
        m.filter = Filter::Only(State::Pending);
        m.expanded.insert(0);
        m.selected = 1;
        assert_eq!(m.selected_entry(), Some(Entry::Member(0, 1)));
        press(&mut m, &mut s, 'e');
        press(&mut m, &mut s, '!');
        let back = e02(vec![finding(9, 2, TYPO, FIXED, 165_060), snacks(3)]);
        let effects = send(&mut m, &mut s, back, now());
        send(&mut m, &mut s, taken(effects), now());
        assert_eq!(m.entries(), [Entry::Row(1)]);
        let reworded = format!("{FIXED}!");
        assert_eq!(
            decided(&key(&mut m, &mut s, KeyCode::Enter)),
            [(9, State::Accepted, Some(reworded.as_str()))]
        );
    }

    #[test]
    fn in_the_editor_playback_keys_edit() {
        let mut m = sample();
        let mut s = Screen::default();
        m.selected = 1;
        press(&mut m, &mut s, 'e');
        assert_eq!(key(&mut m, &mut s, KeyCode::Left), []);
        assert_eq!(press(&mut m, &mut s, ' '), []);
        assert_eq!(press(&mut m, &mut s, '.'), []);
        assert_eq!(
            s.editor.as_ref().map(|e| e.text.as_str()),
            Some("Let's have snacks together every day ..")
        );
    }

    #[test]
    fn a_click_selects_the_entry_under_it() {
        let (mut m, mut s) = on_screen();
        key(&mut m, &mut s, KeyCode::Enter);
        // Row 0, its members E01 E02 E03, then row 1: y 3 is E02's member.
        assert_eq!(click(&mut m, &mut s, 5, 3, 0), []);
        assert_eq!(m.selected_entry(), Some(Entry::Member(0, 1)));
        // The border, the title, below the last entry, the detail pane: nothing.
        for (x, y) in [(0, 2), (5, 0), (5, 9), (70, 2)] {
            click(&mut m, &mut s, x, y, 1000);
            assert_eq!(m.selected_entry(), Some(Entry::Member(0, 1)), "{x},{y}");
        }
    }

    #[test]
    fn a_click_lands_on_the_entry_drawn_there_when_the_list_is_scrolled() {
        let mut m = subcheck_core::review::model::tests_support::many(30);
        let mut s = Screen::default();
        send(&mut m, &mut s, Msg::Resize(110, 20), now());
        key(&mut m, &mut s, KeyCode::End);
        // 17 rows, ending at entry 29: entry 13 is on the first row.
        assert_eq!(s.scroll, 13);
        click(&mut m, &mut s, 5, 1, 0);
        assert_eq!(m.selected, 13);
        click(&mut m, &mut s, 5, 17, 1000);
        assert_eq!(m.selected, 29);
    }

    #[test]
    fn the_first_click_lands_on_the_row_drawn_there_when_review_opens_scrolled() {
        // Review opened on the first pending row, entry 25, before any message: the screen it
        // first draws is scrolled to show it.
        let mut m = subcheck_core::review::model::tests_support::many(30);
        let mut s = Screen::default();
        m.selected = 25;
        s.set_area(&m, Rect::new(0, 0, 110, 20));
        assert_eq!(s.scroll, 9);
        click(&mut m, &mut s, 5, 1, 0);
        assert_eq!(m.selected, 9);
    }

    #[test]
    fn a_double_click_opens_and_a_slow_or_moved_one_does_not() {
        let (mut m, mut s) = on_screen();
        let opens =
            |effects: Vec<Effect>| matches!(effects[..], [Effect::Player(PlayerCommand::Open(_))]);
        assert_eq!(click(&mut m, &mut s, 5, 2, 0), []);
        assert!(opens(click(&mut m, &mut s, 5, 2, 500)), "within 500 ms");
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)));
        // A third click starts over.
        assert_eq!(click(&mut m, &mut s, 5, 2, 600), []);
        // Too slow.
        assert_eq!(click(&mut m, &mut s, 5, 2, 2000), []);
        assert_eq!(click(&mut m, &mut s, 5, 2, 2501), []);
        // Two rows.
        assert_eq!(click(&mut m, &mut s, 5, 1, 4000), []);
        assert_eq!(click(&mut m, &mut s, 5, 2, 4100), []);
        // Something in between.
        assert_eq!(click(&mut m, &mut s, 5, 2, 6000), []);
        key(&mut m, &mut s, KeyCode::Char('f'));
        assert_eq!(click(&mut m, &mut s, 5, 2, 6100), []);
        // The clock stepped back between them (NTP, a resume): not a double-click.
        assert_eq!(click(&mut m, &mut s, 5, 2, 8000), []);
        assert_eq!(click(&mut m, &mut s, 5, 2, 7000), []);
    }

    #[test]
    fn a_double_click_survives_the_scans_messages_between_its_clicks() {
        let opens =
            |effects: Vec<Effect>| matches!(effects[..], [Effect::Player(PlayerCommand::Open(_))]);
        // The spinner's tick, which comes after 250 ms without a message while a job runs.
        let (mut m, mut s) = on_screen();
        assert_eq!(click(&mut m, &mut s, 5, 2, 0), []);
        send(&mut m, &mut s, Msg::Tick, now());
        assert!(opens(click(&mut m, &mut s, 5, 2, 300)), "a tick in between");
        // A file's findings, a row of them above the snacks row, which moves down a line: the
        // second click lands on it there.
        let (mut m, mut s) = on_screen();
        assert_eq!(click(&mut m, &mut s, 5, 2, 0), []);
        let msg = saved(
            &m,
            1,
            vec![finding(5, 1, "An erly line.", "An early line.", 1000)],
        );
        send(&mut m, &mut s, msg, now());
        assert_eq!(m.entries()[2], Entry::Row(2));
        assert!(opens(click(&mut m, &mut s, 5, 3, 300)), "a row in above it");
    }

    #[test]
    fn a_double_click_survives_its_file_being_checked_again() {
        let opens =
            |effects: Vec<Effect>| matches!(effects[..], [Effect::Player(PlayerCommand::Open(_))]);
        let (mut m, mut s) = on_screen();
        assert_eq!(click(&mut m, &mut s, 5, 2, 0), []);
        // E02 checked again: its snacks line, at y 2, comes back as finding 9.
        let snacks = finding(
            9,
            2,
            "Let's have snacks together everyday.",
            "Let's have snacks together every day.",
            894_190,
        );
        let view = Run {
            files: vec![tests_support::file(2)],
            findings: vec![finding(2, 2, TYPO, FIXED, 165_060), snacks],
            ..Run::default()
        };
        let back = Input::ReadBack {
            path: path(2),
            view: Ok(view),
        };
        send(&mut m, &mut s, back, now());
        assert!(
            opens(click(&mut m, &mut s, 5, 2, 300)),
            "its row, with finding 9"
        );
    }

    #[test]
    fn the_wheel_moves_the_selection_over_the_list_and_scrolls_the_detail() {
        let (mut m, mut s) = on_screen();
        wheel(&mut m, &mut s, Mouse::WheelDown, 10, 5);
        assert_eq!(m.selected, 1);
        wheel(&mut m, &mut s, Mouse::WheelUp, 10, 5);
        assert_eq!(m.selected, 0);
        // The ×3 row's detail is 11 lines: the wheel stops with the last one on top.
        for _ in 0..5 {
            wheel(&mut m, &mut s, Mouse::WheelDown, 70, 5);
        }
        assert_eq!(s.detail_scroll, 10);
        wheel(&mut m, &mut s, Mouse::WheelUp, 70, 5);
        assert_eq!(s.detail_scroll, 7);
        // A new selection starts at the top.
        key(&mut m, &mut s, KeyCode::Down);
        assert_eq!(s.detail_scroll, 0);
    }

    #[test]
    fn a_detail_that_got_shorter_is_scrolled_back_to_its_last_line() {
        let (mut m, mut s) = on_screen();
        // Scrolled for a longer text: a decision or a filter can shorten it while the same entry
        // stays selected.
        s.detail_scroll = 100;
        send(
            &mut m,
            &mut s,
            Input::Played(Err("mpv is not open".into())),
            now(),
        );
        assert_eq!(
            s.detail_scroll, 10,
            "the ×3 row's 11 lines, the last on top"
        );
    }

    #[test]
    fn question_mark_shows_the_keys_and_any_key_closes_them() {
        let (mut m, mut s) = on_screen();
        assert!(!s.help);
        press(&mut m, &mut s, '?');
        assert!(s.help);
        // Closing is all the key does: no decision.
        assert_eq!(press(&mut m, &mut s, 'a'), []);
        assert!(!s.help);
        assert_eq!(states(&m), [State::Pending; 4]);
        press(&mut m, &mut s, '?');
        key(&mut m, &mut s, KeyCode::Esc);
        assert!(!s.help);
        // But `q` quits, as the box says.
        press(&mut m, &mut s, '?');
        press(&mut m, &mut s, 'q');
        assert!(m.should_quit);
        m.should_quit = false;
        // `? help` at the end of the status line opens them; a click anywhere closes them, and the
        // wheel does nothing under them.
        click(&mut m, &mut s, 105, 19, 0);
        assert!(s.help);
        wheel(&mut m, &mut s, Mouse::WheelDown, 10, 5);
        assert_eq!(m.selected, 0);
        click(&mut m, &mut s, 5, 2, 1000);
        assert!(!s.help);
        assert_eq!(m.selected, 0, "the click only closed them");
    }

    #[test]
    fn question_mark_lets_the_message_go_as_any_key_in_a_pane_does() {
        let mut m = sample();
        let mut s = Screen::default();
        m.status = Some("opened in mpv".into());
        press(&mut m, &mut s, '?');
        assert!(s.help);
        assert_eq!(m.status, None);
    }

    #[test]
    fn while_editing_only_the_editors_buttons_work() {
        let (mut m, mut s) = on_screen();
        m.selected = 1;
        press(&mut m, &mut s, 'e');
        assert_eq!(click(&mut m, &mut s, 5, 1, 0), []);
        wheel(&mut m, &mut s, Mouse::WheelUp, 5, 1);
        assert_eq!(click(&mut m, &mut s, 50, 19, 1000), [], "the text");
        assert_eq!(click(&mut m, &mut s, 98, 19, 1500), [], "the separator");
        assert_eq!(m.selected, 1);
        assert!(s.editor.is_some());
        // "⏎ accept · Esc cancel" at the end of the status line: Esc from x 100.
        click(&mut m, &mut s, 100, 19, 2000);
        assert!(s.editor.is_none());
        press(&mut m, &mut s, 'e');
        assert_eq!(
            decided(&click(&mut m, &mut s, 89, 19, 3000)),
            [(3, State::Accepted, None)]
        );
    }

    #[test]
    fn the_selection_keeps_its_place_on_screen_as_entries_come_and_go_above_it() {
        let mut m = tests_support::many(20);
        let mut s = Screen::default();
        // 12 lines: the status line and the list's border leave 9 rows.
        s.set_area(&m, Rect::new(0, 0, 120, 12));
        m.selected = 15;
        s.scroll = s.list_scroll(&m, s.area);
        let on_screen = m.selected - s.scroll;
        let e01 = tests_support::path(1);
        let first = m.run.findings[0].clone();
        let zero = tests_support::finding(100, 1, "Zero.", "Zero!", 500);
        let with_zero = Run {
            files: vec![tests_support::file(1)],
            findings: vec![first.clone(), zero],
            ..Run::default()
        };
        let read_back = |view| Input::ReadBack {
            path: e01.clone(),
            view: Ok(view),
        };
        send(&mut m, &mut s, read_back(with_zero), now());
        assert_eq!(m.selected, 16);
        assert_eq!(m.selected - s.scroll, on_screen);
        let without = Run {
            files: vec![tests_support::file(1)],
            findings: vec![first],
            ..Run::default()
        };
        send(&mut m, &mut s, read_back(without), now());
        assert_eq!(m.selected, 15);
        assert_eq!(m.selected - s.scroll, on_screen);
    }

    #[test]
    fn a_late_join_that_hides_a_row_above_the_selection_keeps_its_row_on_the_same_screen_line() {
        // Ten more rows, pending: the list is taller than the five rows the screen shows.
        let mut m = sample();
        let mut s = Screen::default();
        let more: Vec<subcheck_core::store::Finding> = (0..10_u64)
            .map(|n| {
                let line = format!("Line {n} has an erly typo.");
                finding(
                    10 + n,
                    3,
                    &line,
                    "Fixed.",
                    3_000_000 + 10_000 * i64::try_from(n).unwrap(),
                )
            })
            .collect();
        let view = tests_support::read_back(&m, 3, more);
        m.refresh(&path(3), view);
        decide_as(&mut m, &[1, 2, 4], State::Accepted, None);
        m.filter = Filter::Only(State::Pending);
        m.selected = 8;
        s.set_area(&m, Rect::new(0, 0, 100, 8));
        let line = |m: &Review, s: &Screen| m.selected - s.list_scroll(m, s.area);
        let selected = m.selected_entry();
        assert_eq!(line(&m, &s), 4, "scrolled to the bottom of the screen");
        // E04's lyric joins the decided lyric row: the row shows above the selection for a
        // moment, then the inherited decision hides it again.
        let msg = saved(&m, 4, vec![finding(30, 4, TYPO, FIXED, 165_060)]);
        let effects = send(&mut m, &mut s, msg, now());
        send(&mut m, &mut s, taken(effects), now());
        assert_eq!(m.selected_entry(), selected);
        assert_eq!(m.selected, 8);
        assert_eq!(line(&m, &s), 4, "the selected row did not move on screen");
    }

    #[test]
    fn c_in_the_findings_does_nothing() {
        let mut m = sample();
        let mut s = Screen::default();
        assert_eq!(press(&mut m, &mut s, 'c'), []);
        assert_eq!(m.confirm, None);
    }
}
