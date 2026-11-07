//! Review for a UI in another language: core's [`session::Session`] behind UniFFI, whose bindgen
//! writes the other side (uniffi-bindgen-cpp, for the Qt app). Only what a window uses crosses.
//!
//! Core has no UniFFI dependency, so each type that crosses has a mirror here. Each conversion
//! names every variant and field of what it reads, and builds every field of what it makes, so a
//! field added on either side, or a variant added to the side a conversion reads, fails the build
//! here until it is handled. A core action with no mirror cannot be sent: [`Action`] is what the
//! window offers.

use std::sync::Arc;

use subcheck_core::player;
use subcheck_core::review::{model, session, update};
use subcheck_core::store;

uniffi::setup_scaffolding!();

/// One run under review, for a UI on any thread; a UI opens one per process ([`session::Session`]).
///
/// `next_event` blocks, so a UI calls it from a thread of its own. On every way out, a UI calls
/// `close`, then waits for that thread to return, then lets go. A `next_event` that waits holds a
/// reference to the session for as long as it waits, so letting go alone ends nothing while one
/// does: the session ends itself only when nobody holds it, and nobody reads it.
#[derive(uniffi::Object)]
pub struct Session(session::Session);

#[uniffi::export]
impl Session {
    /// Open the view `selector` names (`[run | paths…]`, as `subcheck review` takes it). Returns at
    /// once: what goes wrong opening it is the first event, an `Ended`, and actions sent before
    /// then wait their turn. Apply is not offered, so karaoke's re-spacing is off.
    #[uniffi::constructor]
    pub fn open(selector: Vec<String>) -> Arc<Session> {
        Arc::new(Session(session::Session::open(selector, false)))
    }

    /// Queue `action` and return. After the end it goes nowhere.
    pub fn send(&self, action: Action) {
        self.0.send(action.into());
    }

    /// Say how the player's last command went. After the end it goes nowhere.
    pub fn report(&self, report: PlayerReport) {
        self.0.report(report.into());
    }

    /// The next event, waiting for one; none once the session is over, every time and at once.
    pub fn next_event(&self) -> Option<Event> {
        self.0.next_event().map(Event::from)
    }

    /// End the stream at once, whatever the session is doing, and stop the session when it can
    /// next be stopped. What was queued before still comes out of `next_event` first, a player
    /// command among them, so a UI ignores what it reads after calling this.
    pub fn close(&self) {
        self.0.close();
    }
}

/// An entry by its findings' IDs, which outlast a remake of the rows: a row by every member's, so
/// that any one still there finds it, and a member by its own ([`model::EntryKey`]).
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum EntryKey {
    Row(Vec<u64>),
    Member(u64),
}

/// A finding's decision ([`store::State`]).
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Accepted,
    Rejected,
}

/// What a playback key asks of the player ([`player::Playback`]).
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playback {
    /// Pause, or resume.
    Pause,
    /// Seek this many seconds; back when negative.
    Seek(i32),
    /// One frame on, or back; the player pauses there.
    NextFrame,
    PreviousFrame,
}

/// Where playback is, as the player reports it ([`player::PlayState`]).
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayState {
    pub paused: bool,
    pub at_ms: i64,
}

/// A moment to open ([`player::Moment`]): a file, where to start, and the subtitles to show.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Moment {
    pub file: String,
    pub start_ms: i64,
    /// mpv's `--sid`; none leaves the choice to the player.
    pub sid: Option<u64>,
    /// Stay on the frame at `start_ms` (a timing finding's), rather than play from it (a line's).
    pub paused: bool,
}

/// What review asks of the UI's player ([`player::PlayerCommand`]), which says how it went
/// through `Session::report`.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum PlayerCommand {
    Open(Moment),
    /// Pause or seek the open player.
    Playback(Playback),
    /// Close the player, and leave review open.
    Close,
}

/// One line of the list ([`session::EntryView`]), as a UI shows it: a row, or a member of an
/// expanded row.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct EntryView {
    /// What names it to `Action::Select`.
    pub key: EntryKey,
    /// Its colour in the list: a row with anything pending is pending, however its decided
    /// members differ; else what its shown members agree on, and none when they do not.
    pub state: Option<State>,
    pub is_member: bool,
    /// Whether its members are the lines below it: never for a member, or a row of one.
    pub expanded: bool,
    /// The name its file goes by in the list, of a row's first shown member.
    pub label: String,
    /// The members the filter shows, and all it has. A member counts itself.
    pub shown: u64,
    pub total: u64,
    /// For a row, this, `original`, `wording`, `style` and `track` are its first shown member's.
    pub at_ms: i64,
    pub original: String,
    pub wording: String,
    pub style: String,
    /// What apply did or would do with it, if anything.
    pub mark: Option<String>,
    /// A timing finding's track; empty for others.
    pub track: String,
    /// A timing finding's fix in short; none for others.
    pub fix: Option<String>,
}

/// A change to the rows the UI holds ([`session::Splice`]): the `removed` rows at `at` give
/// way to `inserted`.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    pub at: u64,
    pub removed: u64,
    pub inserted: Vec<EntryView>,
}

/// What a UI shows beside the list ([`session::Summary`]).
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// What the view is called.
    pub title: String,
    /// An index into the rows the UI holds; none when it holds none.
    pub selected: Option<u64>,
    /// The status line; none when review has nothing to say there.
    pub status: Option<String>,
    /// Findings pending, accepted and rejected: the whole view's, whatever the filter shows.
    pub pending: u64,
    pub accepted: u64,
    pub rejected: u64,
    /// Whether a job runs.
    pub busy: bool,
}

/// What a UI hears, in order, from `Session::next_event` ([`session::Event`]).
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// What the UI shows changed. `rows` is none when the list did not, but never in the first
    /// event, which is a splice from an empty list even for a run with no findings.
    Changed {
        rows: Option<Splice>,
        summary: Summary,
    },
    /// For the UI's player, which runs it and says how it went through `Session::report`.
    Player(PlayerCommand),
    /// The last event: why review could not go on, if it could not, and what is left to say of the
    /// jobs it ran. A job's process groups are ended before it comes.
    Ended {
        fatal: Option<String>,
        outro: Vec<String>,
    },
}

/// What the window asks of review ([`update::Action`]): the actions it offers, and no others.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Select the entry this key names. A member of a collapsed row selects its row; a key whose
    /// entry the list does not show leaves the selection where it is, and the status line says so.
    Select {
        key: EntryKey,
    },
    /// Expand the selected row into its members, or collapse it (from the row or a member).
    Toggle,
    Accept,
    Reject,
    Undo,
    /// Open the selected entry's moment in the player.
    Play,
    ClosePlayer,
    Playback {
        playback: Playback,
    },
    /// Quit. No action here starts a job, so review never asks first.
    Quit,
}

/// What the UI's player says of a command it ran ([`session::PlayerReport`]). A failed play is
/// a variant of its own: UniFFI has no `Result` to carry it in, and `Played` carries a state.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum PlayerReport {
    /// The moment opened, or why not.
    Opened { error: Option<String> },
    /// Where playback is after a `Playback`.
    Played { state: PlayState },
    /// Why a `Playback` failed.
    PlayFailed { error: String },
    /// The player closed, or why not.
    Closed { error: Option<String> },
}

/// A position or a count, as UniFFI carries it: it has no `usize`, and no Rust target's is wider
/// than a u64.
fn wide(n: usize) -> u64 {
    n as u64
}

/// `None` as success, as the player's reports say it.
fn result(error: Option<String>) -> Result<(), String> {
    error.map_or(Ok(()), Err)
}

impl From<model::EntryKey> for EntryKey {
    fn from(key: model::EntryKey) -> EntryKey {
        match key {
            model::EntryKey::Row(ids) => EntryKey::Row(ids),
            model::EntryKey::Member(id) => EntryKey::Member(id),
        }
    }
}

impl From<EntryKey> for model::EntryKey {
    fn from(key: EntryKey) -> model::EntryKey {
        match key {
            EntryKey::Row(ids) => model::EntryKey::Row(ids),
            EntryKey::Member(id) => model::EntryKey::Member(id),
        }
    }
}

impl From<store::State> for State {
    fn from(state: store::State) -> State {
        match state {
            store::State::Pending => State::Pending,
            store::State::Accepted => State::Accepted,
            store::State::Rejected => State::Rejected,
        }
    }
}

impl From<player::Playback> for Playback {
    fn from(playback: player::Playback) -> Playback {
        match playback {
            player::Playback::Pause => Playback::Pause,
            player::Playback::Seek(seconds) => Playback::Seek(seconds),
            player::Playback::NextFrame => Playback::NextFrame,
            player::Playback::PreviousFrame => Playback::PreviousFrame,
        }
    }
}

impl From<Playback> for player::Playback {
    fn from(playback: Playback) -> player::Playback {
        match playback {
            Playback::Pause => player::Playback::Pause,
            Playback::Seek(seconds) => player::Playback::Seek(seconds),
            Playback::NextFrame => player::Playback::NextFrame,
            Playback::PreviousFrame => player::Playback::PreviousFrame,
        }
    }
}

impl From<PlayState> for player::PlayState {
    fn from(state: PlayState) -> player::PlayState {
        let PlayState { paused, at_ms } = state;
        player::PlayState { paused, at_ms }
    }
}

impl From<player::Moment> for Moment {
    fn from(moment: player::Moment) -> Moment {
        let player::Moment {
            file,
            start_ms,
            sid,
            paused,
        } = moment;
        Moment {
            file,
            start_ms,
            sid: sid.map(wide),
            paused,
        }
    }
}

impl From<player::PlayerCommand> for PlayerCommand {
    fn from(command: player::PlayerCommand) -> PlayerCommand {
        match command {
            player::PlayerCommand::Open(moment) => PlayerCommand::Open(moment.into()),
            player::PlayerCommand::Playback(playback) => PlayerCommand::Playback(playback.into()),
            player::PlayerCommand::Close => PlayerCommand::Close,
        }
    }
}

impl From<session::EntryView> for EntryView {
    fn from(view: session::EntryView) -> EntryView {
        let session::EntryView {
            key,
            state,
            is_member,
            expanded,
            label,
            shown,
            total,
            at_ms,
            original,
            wording,
            style,
            mark,
            track,
            fix,
        } = view;
        EntryView {
            key: key.into(),
            state: state.map(State::from),
            is_member,
            expanded,
            label,
            shown: wide(shown),
            total: wide(total),
            at_ms,
            original,
            wording,
            style,
            mark,
            track,
            fix,
        }
    }
}

impl From<session::Splice> for Splice {
    fn from(splice: session::Splice) -> Splice {
        let session::Splice {
            at,
            removed,
            inserted,
        } = splice;
        Splice {
            at: wide(at),
            removed: wide(removed),
            inserted: inserted.into_iter().map(EntryView::from).collect(),
        }
    }
}

impl From<session::Summary> for Summary {
    fn from(summary: session::Summary) -> Summary {
        let session::Summary {
            title,
            selected,
            status,
            pending,
            accepted,
            rejected,
            busy,
        } = summary;
        Summary {
            title,
            selected: selected.map(wide),
            status,
            pending: wide(pending),
            accepted: wide(accepted),
            rejected: wide(rejected),
            busy,
        }
    }
}

impl From<session::Event> for Event {
    fn from(event: session::Event) -> Event {
        match event {
            session::Event::Changed { rows, summary } => Event::Changed {
                rows: rows.map(Splice::from),
                summary: summary.into(),
            },
            session::Event::Player(command) => Event::Player(command.into()),
            session::Event::Ended { fatal, outro } => Event::Ended { fatal, outro },
        }
    }
}

impl From<Action> for update::Action {
    fn from(action: Action) -> update::Action {
        match action {
            Action::Select { key } => update::Action::Select(key.into()),
            Action::Toggle => update::Action::Toggle,
            Action::Accept => update::Action::Accept,
            Action::Reject => update::Action::Reject,
            Action::Undo => update::Action::Undo,
            Action::Play => update::Action::Play,
            Action::ClosePlayer => update::Action::ClosePlayer,
            Action::Playback { playback } => update::Action::Playback(playback.into()),
            Action::Quit => update::Action::Quit,
        }
    }
}

impl From<PlayerReport> for session::PlayerReport {
    fn from(report: PlayerReport) -> session::PlayerReport {
        match report {
            PlayerReport::Opened { error } => session::PlayerReport::Opened(result(error)),
            PlayerReport::Played { state } => session::PlayerReport::Played(Ok(state.into())),
            PlayerReport::PlayFailed { error } => session::PlayerReport::Played(Err(error)),
            PlayerReport::Closed { error } => session::PlayerReport::Closed(result(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use subcheck_core::review::model::tests_support;
    use subcheck_core::store::{Attempt, CheckedWith, Made, RunHeader, Store};

    /// Each field a value of its own, so a conversion that crossed two would show.
    fn core_view() -> session::EntryView {
        session::EntryView {
            key: model::EntryKey::Row(vec![3, u64::MAX]),
            state: Some(store::State::Rejected),
            is_member: true,
            expanded: false,
            label: "E01 字幕".into(),
            shown: 2,
            total: usize::MAX,
            at_ms: i64::MIN,
            original: "befor".into(),
            wording: "before".into(),
            style: "OP en".into(),
            mark: Some("would rewrite".into()),
            track: "English Full".into(),
            fix: None,
        }
    }

    fn view() -> EntryView {
        EntryView {
            key: EntryKey::Row(vec![3, u64::MAX]),
            state: Some(State::Rejected),
            is_member: true,
            expanded: false,
            label: "E01 字幕".into(),
            shown: 2,
            total: u64::MAX,
            at_ms: i64::MIN,
            original: "befor".into(),
            wording: "before".into(),
            style: "OP en".into(),
            mark: Some("would rewrite".into()),
            track: "English Full".into(),
            fix: None,
        }
    }

    #[test]
    fn a_change_crosses_field_for_field() {
        let event = session::Event::Changed {
            rows: Some(session::Splice {
                at: 7,
                removed: 1,
                inserted: vec![core_view()],
            }),
            summary: session::Summary {
                title: "run r".into(),
                selected: Some(5),
                status: Some("opened in mpv".into()),
                pending: 1,
                accepted: 2,
                rejected: 3,
                busy: true,
            },
        };
        let expected = Event::Changed {
            rows: Some(Splice {
                at: 7,
                removed: 1,
                inserted: vec![view()],
            }),
            summary: Summary {
                title: "run r".into(),
                selected: Some(5),
                status: Some("opened in mpv".into()),
                pending: 1,
                accepted: 2,
                rejected: 3,
                busy: true,
            },
        };
        assert_eq!(Event::from(event), expected);
    }

    #[test]
    fn a_change_with_nothing_set_crosses_as_nothing() {
        let mut core = core_view();
        core.key = model::EntryKey::Member(4);
        core.state = None;
        core.mark = None;
        core.fix = Some(String::new());
        let event = session::Event::Changed {
            rows: None,
            summary: session::Summary {
                title: String::new(),
                selected: None,
                status: None,
                pending: 0,
                accepted: 0,
                rejected: 0,
                busy: false,
            },
        };
        let Event::Changed { rows, summary } = Event::from(event) else {
            panic!("not a change");
        };
        assert_eq!(rows, None);
        assert_eq!((summary.selected, summary.status), (None, None));
        let crossed = EntryView::from(core);
        assert_eq!(crossed.key, EntryKey::Member(4));
        assert_eq!((crossed.state, crossed.mark), (None, None));
        assert_eq!(crossed.fix, Some(String::new()));
    }

    #[test]
    fn each_state_crosses() {
        for (core, ffi) in [
            (store::State::Pending, State::Pending),
            (store::State::Accepted, State::Accepted),
            (store::State::Rejected, State::Rejected),
        ] {
            assert_eq!(State::from(core), ffi);
        }
    }

    #[test]
    fn each_player_command_and_the_end_cross() {
        let open = player::PlayerCommand::Open(player::Moment {
            file: "/m/E01.mkv".into(),
            start_ms: 165_060,
            sid: Some(2),
            paused: true,
        });
        let cases = [
            (
                session::Event::Player(open),
                Event::Player(PlayerCommand::Open(Moment {
                    file: "/m/E01.mkv".into(),
                    start_ms: 165_060,
                    sid: Some(2),
                    paused: true,
                })),
            ),
            (
                session::Event::Player(player::PlayerCommand::Close),
                Event::Player(PlayerCommand::Close),
            ),
            (
                session::Event::Ended {
                    fatal: Some("no run named r".into()),
                    outro: vec!["a".into(), String::new()],
                },
                Event::Ended {
                    fatal: Some("no run named r".into()),
                    outro: vec!["a".into(), String::new()],
                },
            ),
        ];
        for (core, ffi) in cases {
            assert_eq!(Event::from(core), ffi);
        }
        let moment = player::Moment {
            file: String::new(),
            start_ms: -1,
            sid: None,
            paused: false,
        };
        assert_eq!(Moment::from(moment).sid, None);
    }

    #[test]
    fn a_key_and_a_playback_cross_both_ways() {
        for key in [model::EntryKey::Row(vec![1, 2]), model::EntryKey::Member(9)] {
            assert_eq!(model::EntryKey::from(EntryKey::from(key.clone())), key);
        }
        for (core, ffi) in [
            (player::Playback::Pause, Playback::Pause),
            (player::Playback::Seek(-5), Playback::Seek(-5)),
            (player::Playback::NextFrame, Playback::NextFrame),
            (player::Playback::PreviousFrame, Playback::PreviousFrame),
        ] {
            let command = player::PlayerCommand::Playback(core);
            let Event::Player(PlayerCommand::Playback(crossed)) =
                Event::from(session::Event::Player(command))
            else {
                panic!("not a playback");
            };
            assert_eq!(crossed, ffi);
            assert_eq!(player::Playback::from(ffi), core);
        }
    }

    #[test]
    fn each_action_the_window_offers_is_cores() {
        let cases = [
            (
                Action::Select {
                    key: EntryKey::Member(7),
                },
                update::Action::Select(model::EntryKey::Member(7)),
            ),
            (Action::Toggle, update::Action::Toggle),
            (Action::Accept, update::Action::Accept),
            (Action::Reject, update::Action::Reject),
            (Action::Undo, update::Action::Undo),
            (Action::Play, update::Action::Play),
            (Action::ClosePlayer, update::Action::ClosePlayer),
            (
                Action::Playback {
                    playback: Playback::Seek(5),
                },
                update::Action::Playback(player::Playback::Seek(5)),
            ),
            (Action::Quit, update::Action::Quit),
        ];
        for (ffi, core) in cases {
            assert_eq!(update::Action::from(ffi), core);
        }
    }

    #[test]
    fn each_report_is_cores_and_a_failed_play_a_played_error() {
        let state = PlayState {
            paused: true,
            at_ms: 171_060,
        };
        let cases = [
            (
                PlayerReport::Opened { error: None },
                session::PlayerReport::Opened(Ok(())),
            ),
            (
                PlayerReport::Opened {
                    error: Some("mpv not found".into()),
                },
                session::PlayerReport::Opened(Err("mpv not found".into())),
            ),
            (
                PlayerReport::Played { state },
                session::PlayerReport::Played(Ok(player::PlayState {
                    paused: true,
                    at_ms: 171_060,
                })),
            ),
            (
                PlayerReport::PlayFailed {
                    error: "mpv is not open".into(),
                },
                session::PlayerReport::Played(Err("mpv is not open".into())),
            ),
            (
                PlayerReport::Closed { error: None },
                session::PlayerReport::Closed(Ok(())),
            ),
            (
                PlayerReport::Closed {
                    error: Some("mpv crashed".into()),
                },
                session::PlayerReport::Closed(Err("mpv crashed".into())),
            ),
        ];
        for (ffi, core) in cases {
            assert_eq!(session::PlayerReport::from(ffi), core);
        }
    }

    const CHILD: &str = "tests::child_that_opens_a_session";

    /// How long the child may take: a session that hangs fails the test, and does not hang the run.
    const LIMIT: Duration = Duration::from_secs(30);

    /// A database in `dir` holding review's sample run (four findings in two rows, the first of
    /// three members), and the run's name.
    fn seeded(dir: &Path) -> String {
        let mut store = Store::open_at(&dir.join("subcheck.db")).unwrap();
        let sample = tests_support::run();
        let paths: Vec<PathBuf> = sample
            .files
            .iter()
            .map(|f| PathBuf::from(&f.path))
            .collect();
        let header = RunHeader {
            started: jiff::Timestamp::UNIX_EPOCH,
            backend: "claude".into(),
            command: vec![],
            prompt_version: 1,
            version: "0".into(),
            langs: vec!["eng".into()],
        };
        let scan = store.begin_scan(&header, &paths).unwrap();
        for record in &sample.files {
            let attempt = Attempt {
                scan: scan.id,
                path: &record.path,
                size: record.size,
                mtime: record.mtime,
                tracks: &record.tracks,
                truncated: None,
                at: jiff::Timestamp::UNIX_EPOCH,
            };
            let findings = sample
                .findings
                .iter()
                .filter(|f| f.file == record.path)
                .cloned()
                .collect();
            let made = Made {
                with: CheckedWith::new("claude", &["eng".into()], 1),
                lines: 1,
                findings,
            };
            store.save(&attempt, vec![made]).unwrap();
        }
        scan.name
    }

    /// Run by `a_session_crosses_and_comes_back`. `Session::open` finds the store through the
    /// environment, which `Command::env` sets for the child alone: set in the test process, it
    /// would reach every test running beside this one.
    #[test]
    #[ignore = "the child of the round trip"]
    fn child_that_opens_a_session() {
        let Some(dir) = std::env::var_os("SUBCHECK_FFI_TEST_DIR") else {
            return;
        };
        let name = std::env::var("SUBCHECK_FFI_TEST_RUN").unwrap();
        let s = Session::open(vec![name.clone()]);
        let next = || s.next_event().expect("the stream ended before its end");
        let change = || match next() {
            Event::Changed { rows, summary } => (rows, summary),
            other => panic!("not a change: {other:?}"),
        };
        let (rows, summary) = change();
        let rows = rows.unwrap();
        assert_eq!((rows.at, rows.removed, rows.inserted.len()), (0, 0, 2));
        assert_eq!(rows.inserted[0].key, EntryKey::Row(vec![1, 2, 4]));
        assert_eq!(summary.title, format!("run {name}"));
        assert_eq!((summary.selected, summary.pending), (Some(0), 4));
        s.send(Action::Toggle);
        let rows = change().0.unwrap();
        assert_eq!((rows.at, rows.removed, rows.inserted.len()), (0, 1, 4));
        s.send(Action::Select {
            key: EntryKey::Member(2),
        });
        let (rows, summary) = change();
        assert_eq!((rows, summary.selected), (None, Some(2)));
        s.send(Action::Accept);
        let (rows, summary) = change();
        let rows = rows.unwrap();
        assert_eq!((rows.at, rows.removed, rows.inserted.len()), (2, 1, 1));
        assert_eq!(rows.inserted[0].state, Some(State::Accepted));
        assert_eq!((summary.pending, summary.accepted), (3, 1));
        s.report(PlayerReport::Played {
            state: PlayState {
                paused: true,
                at_ms: 171_060,
            },
        });
        assert_eq!(change().1.status.as_deref(), Some("⏸ 0:02:51.060"));
        s.report(PlayerReport::PlayFailed {
            error: "mpv is not open".into(),
        });
        assert_eq!(change().1.status.as_deref(), Some("mpv is not open"));
        s.send(Action::Quit);
        assert_eq!(change().1.status, None);
        let end = Event::Ended {
            fatal: None,
            outro: vec![],
        };
        assert_eq!(next(), end);
        assert_eq!(s.next_event(), None);
        // A second session in one process, which `Session`'s doc rules out, is safe here: neither
        // reaches `group::end`, which is what makes it one per process. What was queued before
        // `close` may still come out, and then nothing.
        let closed = Session::open(vec![name]);
        closed.close();
        while closed.next_event().is_some() {}
        assert_eq!(closed.next_event(), None);
        // What the parent checks: a `CHILD` that names no test would pass with nothing run.
        std::fs::write(PathBuf::from(dir).join("ran"), "").unwrap();
    }

    /// The round trip, in a child of this test binary (`child_that_opens_a_session`).
    #[test]
    fn a_session_crosses_and_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let name = seeded(dir.path());
        let log = dir.path().join("child.log");
        let out = std::fs::File::create(&log).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD, "--ignored", "--nocapture"])
            .env("SUBCHECK_DB", dir.path().join("subcheck.db"))
            .env("XDG_STATE_HOME", dir.path().join("state"))
            .env("SUBCHECK_FFI_TEST_DIR", dir.path())
            .env("SUBCHECK_FFI_TEST_RUN", &name)
            .stdout(Stdio::from(out.try_clone().unwrap()))
            .stderr(Stdio::from(out))
            .spawn()
            .unwrap();
        let deadline = Instant::now() + LIMIT;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let said = std::fs::read_to_string(&log).unwrap();
        assert!(status.is_some_and(|s| s.success()), "{status:?}\n{said}");
        assert!(
            dir.path().join("ran").exists(),
            "the child ran nothing: {said}"
        );
    }
}
