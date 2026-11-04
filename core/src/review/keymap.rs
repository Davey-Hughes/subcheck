//! Which action each key asks for, as the screen stands. The core takes actions, not keys, so that
//! every UI drives it with its own; this is the terminal's map. Pure, so each key is tested without
//! a terminal.

use crossterm::event::KeyCode;

use super::model::{Pane, Review};
use super::screen::Screen;
use super::update::Action;
use crate::list::Filter;
use crate::player::Playback;
use crate::store::State;

/// Rows a PgUp or PgDn moves.
const PAGE: usize = 10;

/// What a key press does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Press {
    /// An action, for `update`.
    Act(Action),
    /// `?`: the box of keys opens, and the status message goes ([`Action::Dismiss`]), as on any
    /// key in a pane.
    OpenHelp,
    /// The box of keys closes, and then this action, if any.
    CloseHelp(Option<Action>),
    /// A key for the editor's text, which the screen applies to it (`screen`'s `edit_text`).
    Type(KeyCode),
}

/// What `key` does as `m` and its screen `s` stand: an open question takes it first, then the
/// editor, then the box of keys; else the selected pane's map, where a key with no action only
/// lets the message go.
pub fn press(m: &Review, s: &Screen, key: KeyCode) -> Press {
    // The question takes the next key: `y` answers it, any other lets it go.
    if m.confirm.is_some() {
        return Press::Act(Action::Answer(key == KeyCode::Char('y')));
    }
    if let Some(ed) = &s.editor {
        return match key {
            KeyCode::Enter => Press::Act(Action::Reword(ed.text.clone())),
            KeyCode::Esc => Press::Act(Action::CancelEdit),
            _ => Press::Type(key),
        };
    }
    // Any key closes the box of keys and does nothing else; but `q` quits, as the box says.
    // With `Interrupt`, which leaves the status message as the other keys here do: no question is
    // open (it takes keys first), and with none `Interrupt` asks just as `Quit` does.
    if s.help {
        return Press::CloseHelp((key == KeyCode::Char('q')).then_some(Action::Interrupt));
    }
    if key == KeyCode::Char('?') {
        return Press::OpenHelp;
    }
    let action = match m.pane {
        Pane::Findings => findings_key(m, key),
        Pane::Files => files_key(m, key),
    };
    Press::Act(action.unwrap_or(Action::Dismiss))
}

/// The filter `f` moves on to from `filter`: all, then pending, accepted and rejected in turn.
fn next_filter(filter: Filter) -> Filter {
    match filter {
        Filter::All => Filter::Only(State::Pending),
        Filter::Only(State::Pending) => Filter::Only(State::Accepted),
        Filter::Only(State::Accepted) => Filter::Only(State::Rejected),
        Filter::Only(State::Rejected) => Filter::All,
    }
}

/// A key in the findings. A moving key's entry is worked out here, from the selection and the
/// list as shown, so how far a page goes is the screen's to say.
fn findings_key(m: &Review, key: KeyCode) -> Option<Action> {
    let entries = m.entries();
    let selected = m.selected;
    let last = entries.len().saturating_sub(1);
    // The entry a moving key lands on, by its key. An empty list has none to land on: the key only
    // lets the message go.
    let to = |i: usize| {
        entries
            .get(i)
            .map_or(Action::Dismiss, |&e| Action::Select(m.entry_key(e)))
    };
    Some(match key {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Tab => Action::ShowPane(Pane::Files),
        KeyCode::Char('v') => Action::SwitchView,
        KeyCode::Char('A') => Action::Apply { dry_run: false },
        KeyCode::Char('D') => Action::Apply { dry_run: true },
        KeyCode::Down | KeyCode::Char('j') => to((selected + 1).min(last)),
        KeyCode::Up | KeyCode::Char('k') => to(selected.saturating_sub(1)),
        KeyCode::PageDown => to((selected + PAGE).min(last)),
        KeyCode::PageUp => to(selected.saturating_sub(PAGE)),
        KeyCode::Home | KeyCode::Char('g') => to(0),
        KeyCode::End | KeyCode::Char('G') => to(last),
        KeyCode::Char('f') => Action::SetFilter(next_filter(m.filter)),
        KeyCode::Enter => Action::Toggle,
        KeyCode::Char('n') => Action::NextFile,
        KeyCode::Char('p') => Action::PreviousFile,
        KeyCode::Char('a') => Action::Accept,
        KeyCode::Char('r') => Action::Reject,
        KeyCode::Char('e') => Action::Edit,
        KeyCode::Char('x') => Action::Clear,
        KeyCode::Char('u') => Action::Undo,
        KeyCode::Char('o') => Action::Play,
        KeyCode::Char('O') => Action::ClosePlayer,
        KeyCode::Char(' ') => Action::Playback(Playback::Pause),
        KeyCode::Left => Action::Playback(Playback::Seek(-5)),
        KeyCode::Right => Action::Playback(Playback::Seek(5)),
        KeyCode::Char(',') => Action::Playback(Playback::Seek(-1)),
        KeyCode::Char('.') => Action::Playback(Playback::Seek(1)),
        KeyCode::Char('[') => Action::Playback(Playback::PreviousFrame),
        KeyCode::Char(']') => Action::Playback(Playback::NextFrame),
        _ => return None,
    })
}

/// A key in the files view: moving over the files, `⏎` to a file's findings, `Tab` back. The
/// decision and mpv keys belong to the findings, except `u`: the status line of a late join
/// says "u undoes" wherever the user is, so it undoes here too. `c` checks the selected file again.
/// `A` and `D` apply, as in the findings.
fn files_key(m: &Review, key: KeyCode) -> Option<Action> {
    let files = &m.progress.files;
    let selected = m.file_selected;
    let last = files.len().saturating_sub(1);
    // The file a moving key lands on, by its path; with no files, none.
    let to = |i: usize| {
        files
            .get(i)
            .map_or(Action::Dismiss, |f| Action::SelectFile(f.path.clone()))
    };
    Some(match key {
        KeyCode::Char('u') => Action::Undo,
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Tab => Action::ShowPane(Pane::Findings),
        KeyCode::Char('v') => Action::SwitchView,
        KeyCode::Char('A') => Action::Apply { dry_run: false },
        KeyCode::Char('D') => Action::Apply { dry_run: true },
        KeyCode::Down | KeyCode::Char('j') => to((selected + 1).min(last)),
        KeyCode::Up | KeyCode::Char('k') => to(selected.saturating_sub(1)),
        KeyCode::PageDown => to((selected + PAGE).min(last)),
        KeyCode::PageUp => to(selected.saturating_sub(PAGE)),
        KeyCode::Home | KeyCode::Char('g') => to(0),
        KeyCode::End | KeyCode::Char('G') => to(last),
        KeyCode::Enter => Action::ShowFileFindings,
        KeyCode::Char('c') => Action::Recheck,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::tests_support::{many, path, sample};
    use crate::review::model::{Confirm, EntryKey};
    use crate::review::screen::Editor;

    /// 30 rows, one per episode, the 16th selected: each move lands on an entry of its own.
    fn mid_list() -> Review {
        let mut m = many(30);
        m.selected = 15;
        m
    }

    /// Selecting `many`'s row of episode `ep`, whose one finding has that ID.
    fn row(ep: u64) -> Action {
        Action::Select(EntryKey::Row(vec![ep]))
    }

    /// Selecting `many`'s file of episode `ep`.
    fn file(ep: u32) -> Action {
        Action::SelectFile(path(ep))
    }

    #[test]
    fn each_findings_key_gives_its_action() {
        let m = mid_list();
        let s = Screen::default();
        let keys = [
            (KeyCode::Char('q'), Action::Quit),
            (KeyCode::Tab, Action::ShowPane(Pane::Files)),
            (KeyCode::Char('v'), Action::SwitchView),
            (KeyCode::Char('A'), Action::Apply { dry_run: false }),
            (KeyCode::Char('D'), Action::Apply { dry_run: true }),
            (KeyCode::Down, row(17)),
            (KeyCode::Char('j'), row(17)),
            (KeyCode::Up, row(15)),
            (KeyCode::Char('k'), row(15)),
            (KeyCode::PageDown, row(26)),
            (KeyCode::PageUp, row(6)),
            (KeyCode::Home, row(1)),
            (KeyCode::Char('g'), row(1)),
            (KeyCode::End, row(30)),
            (KeyCode::Char('G'), row(30)),
            (
                KeyCode::Char('f'),
                Action::SetFilter(Filter::Only(State::Pending)),
            ),
            (KeyCode::Enter, Action::Toggle),
            (KeyCode::Char('n'), Action::NextFile),
            (KeyCode::Char('p'), Action::PreviousFile),
            (KeyCode::Char('a'), Action::Accept),
            (KeyCode::Char('r'), Action::Reject),
            (KeyCode::Char('e'), Action::Edit),
            (KeyCode::Char('x'), Action::Clear),
            (KeyCode::Char('u'), Action::Undo),
            (KeyCode::Char('o'), Action::Play),
            (KeyCode::Char('O'), Action::ClosePlayer),
            (KeyCode::Char(' '), Action::Playback(Playback::Pause)),
            (KeyCode::Left, Action::Playback(Playback::Seek(-5))),
            (KeyCode::Right, Action::Playback(Playback::Seek(5))),
            (KeyCode::Char(','), Action::Playback(Playback::Seek(-1))),
            (KeyCode::Char('.'), Action::Playback(Playback::Seek(1))),
            (
                KeyCode::Char('['),
                Action::Playback(Playback::PreviousFrame),
            ),
            (KeyCode::Char(']'), Action::Playback(Playback::NextFrame)),
            // The files view's keys, and keys with no action: the message goes, nothing else.
            (KeyCode::Char('c'), Action::Dismiss),
            (KeyCode::Char('y'), Action::Dismiss),
            (KeyCode::Char('z'), Action::Dismiss),
            (KeyCode::Esc, Action::Dismiss),
            (KeyCode::Backspace, Action::Dismiss),
            (KeyCode::F(5), Action::Dismiss),
        ];
        for (key, action) in keys {
            assert_eq!(press(&m, &s, key), Press::Act(action), "{key:?}");
        }
        assert_eq!(press(&m, &s, KeyCode::Char('?')), Press::OpenHelp);
    }

    #[test]
    fn moving_keys_stop_at_the_lists_ends() {
        let mut m = many(30);
        let s = Screen::default();
        m.selected = 0;
        for key in [KeyCode::Up, KeyCode::Char('k'), KeyCode::PageUp] {
            assert_eq!(press(&m, &s, key), Press::Act(row(1)), "{key:?}");
        }
        m.selected = 29;
        for key in [KeyCode::Down, KeyCode::Char('j'), KeyCode::PageDown] {
            assert_eq!(press(&m, &s, key), Press::Act(row(30)), "{key:?}");
        }
        // Nothing in the sample is rejected: under that filter the list is empty.
        let mut m = sample();
        let s = Screen::default();
        m.filter = Filter::Only(State::Rejected);
        for key in [KeyCode::Char('j'), KeyCode::Char('G'), KeyCode::PageDown] {
            assert_eq!(press(&m, &s, key), Press::Act(Action::Dismiss), "{key:?}");
        }
        let mut m = many(30);
        let s = Screen::default();
        m.pane = Pane::Files;
        m.file_selected = 29;
        assert_eq!(press(&m, &s, KeyCode::Char('j')), Press::Act(file(30)));
        m.file_selected = 0;
        assert_eq!(press(&m, &s, KeyCode::Char('k')), Press::Act(file(1)));
    }

    #[test]
    fn each_files_key_gives_its_action() {
        let mut m = many(30);
        let s = Screen::default();
        m.pane = Pane::Files;
        m.file_selected = 15;
        let keys = [
            (KeyCode::Char('u'), Action::Undo),
            (KeyCode::Char('q'), Action::Quit),
            (KeyCode::Tab, Action::ShowPane(Pane::Findings)),
            (KeyCode::Char('v'), Action::SwitchView),
            (KeyCode::Char('A'), Action::Apply { dry_run: false }),
            (KeyCode::Char('D'), Action::Apply { dry_run: true }),
            (KeyCode::Down, file(17)),
            (KeyCode::Char('j'), file(17)),
            (KeyCode::Up, file(15)),
            (KeyCode::Char('k'), file(15)),
            (KeyCode::PageDown, file(26)),
            (KeyCode::PageUp, file(6)),
            (KeyCode::Home, file(1)),
            (KeyCode::Char('g'), file(1)),
            (KeyCode::End, file(30)),
            (KeyCode::Char('G'), file(30)),
            (KeyCode::Enter, Action::ShowFileFindings),
            (KeyCode::Char('c'), Action::Recheck),
            // The decision and mpv keys belong to the findings.
            (KeyCode::Char('a'), Action::Dismiss),
            (KeyCode::Char('r'), Action::Dismiss),
            (KeyCode::Char('e'), Action::Dismiss),
            (KeyCode::Char('x'), Action::Dismiss),
            (KeyCode::Char('o'), Action::Dismiss),
            (KeyCode::Char('O'), Action::Dismiss),
            (KeyCode::Char(' '), Action::Dismiss),
            (KeyCode::Left, Action::Dismiss),
            (KeyCode::Right, Action::Dismiss),
            (KeyCode::Char(','), Action::Dismiss),
            (KeyCode::Char('.'), Action::Dismiss),
            (KeyCode::Char('['), Action::Dismiss),
            (KeyCode::Char(']'), Action::Dismiss),
            (KeyCode::Char('n'), Action::Dismiss),
            (KeyCode::Char('p'), Action::Dismiss),
            (KeyCode::Char('f'), Action::Dismiss),
        ];
        for (key, action) in keys {
            assert_eq!(press(&m, &s, key), Press::Act(action), "{key:?}");
        }
        assert_eq!(press(&m, &s, KeyCode::Char('?')), Press::OpenHelp);
    }

    #[test]
    fn with_a_question_open_y_answers_and_any_other_key_lets_it_go() {
        let mut m = sample();
        let mut s = Screen::default();
        m.confirm = Some(Confirm::Quit);
        assert_eq!(
            press(&m, &s, KeyCode::Char('y')),
            Press::Act(Action::Answer(true))
        );
        let others = [
            KeyCode::Char('n'),
            KeyCode::Char('Y'),
            KeyCode::Char('q'),
            KeyCode::Char('?'),
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Down,
        ];
        for key in others {
            assert_eq!(
                press(&m, &s, key),
                Press::Act(Action::Answer(false)),
                "{key:?}"
            );
        }
        // The question takes the key before the editor does, and before the box of keys.
        s.editor = Some(Editor {
            text: "Snacks!".into(),
            cursor: 7,
        });
        assert_eq!(
            press(&m, &s, KeyCode::Enter),
            Press::Act(Action::Answer(false))
        );
        s.editor = None;
        s.help = true;
        assert_eq!(
            press(&m, &s, KeyCode::Char('y')),
            Press::Act(Action::Answer(true))
        );
    }

    #[test]
    fn in_the_editor_enter_rewords_esc_cancels_and_other_keys_type() {
        let m = sample();
        let s = Screen {
            editor: Some(Editor {
                text: "Snacks!".into(),
                cursor: 3,
            }),
            ..Screen::default()
        };
        assert_eq!(
            press(&m, &s, KeyCode::Enter),
            Press::Act(Action::Reword("Snacks!".into()))
        );
        assert_eq!(press(&m, &s, KeyCode::Esc), Press::Act(Action::CancelEdit));
        // The panes' keys and `?` are text here.
        let typed = [
            KeyCode::Char('q'),
            KeyCode::Char('a'),
            KeyCode::Char('?'),
            KeyCode::Char(' '),
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Tab,
        ];
        for key in typed {
            assert_eq!(press(&m, &s, key), Press::Type(key), "{key:?}");
        }
    }

    #[test]
    fn with_the_box_of_keys_open_a_key_closes_it_and_q_quits() {
        let m = sample();
        let s = Screen {
            help: true,
            ..Screen::default()
        };
        assert_eq!(
            press(&m, &s, KeyCode::Char('q')),
            Press::CloseHelp(Some(Action::Interrupt))
        );
        let others = [
            KeyCode::Char('a'),
            KeyCode::Char('?'),
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Down,
        ];
        for key in others {
            assert_eq!(press(&m, &s, key), Press::CloseHelp(None), "{key:?}");
        }
    }

    #[test]
    fn f_cycles_all_pending_accepted_rejected() {
        let mut filter = Filter::All;
        let mut seen = Vec::new();
        for _ in 0..4 {
            filter = next_filter(filter);
            seen.push(filter);
        }
        assert_eq!(
            seen,
            [
                Filter::Only(State::Pending),
                Filter::Only(State::Accepted),
                Filter::Only(State::Rejected),
                Filter::All
            ]
        );
    }
}
