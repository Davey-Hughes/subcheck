//! Review on a thread of its own, for a UI whose own loop must not wait on the store or a job:
//! [`Session`] takes the UI's actions on a queue and runs `update` and the [`Runner`] behind it,
//! and the UI hears what happened as one ordered stream of events.
//!
//! The stream is the only way state reaches a UI. A direct read would wait on the core thread, or
//! read a copy newer than the events still queued, and a splice applied to a newer list than the
//! one it was cut from corrupts the rows. The rows are compared, not reported: `update` would have
//! to name every row it touches, and one it missed would leave the UI silently out of step.

use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use jiff::Timestamp;

use super::jobs::panic_text;
use super::model::{Entry, EntryKey, Review};
use super::runner::{self, Runner};
use super::update::{self, Action, Effect, Input, Task};
use crate::group;
use crate::player::{self, PlayState, PlayerCommand, PlayerError};
use crate::store::{self, Selector, State, Store};

/// What a UI hears, in order ([`Session::next_event`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// What the UI shows changed. `rows` is `None` when the list did not, but never in the
    /// first event.
    Changed {
        rows: Option<Splice>,
        summary: Summary,
    },
    /// For the UI's player, which runs it and says how it went ([`Session::report`]).
    Player(PlayerCommand),
    /// The last event: why review could not go on, if it could not, and what is left to say of the
    /// jobs it ran ([`Review::outro`]). A job's process groups are ended before it comes. After
    /// [`Session::close`], it is `close`'s, with no `fatal` and the outro as of the last step
    /// review finished; unless review, ending by itself as `close` ran, put its own on the stream
    /// first, which says the same of the jobs and can carry review's `fatal`.
    Ended {
        fatal: Option<String>,
        outro: Vec<String>,
    },
}

/// A change to the rows the UI holds: `removed` rows at `at` give way to `inserted`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Splice {
    pub at: usize,
    pub removed: usize,
    pub inserted: Vec<EntryView>,
}

/// What a UI shows beside the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// What the view is called ([`crate::store::Run::shown_name`]).
    pub title: String,
    /// An index into the rows the UI holds; `None` when it holds none.
    pub selected: Option<usize>,
    /// The status line ([`Review::status_text`]).
    pub status: Option<String>,
    /// Findings pending, accepted and rejected ([`Review::counts`]): the whole view's, whatever
    /// the filter shows.
    pub pending: usize,
    pub accepted: usize,
    pub rejected: usize,
    /// Whether a job runs ([`crate::review::progress::Progress::busy`]).
    pub busy: bool,
}

/// One line of the list, as a UI shows it: a row, or a member of an expanded row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryView {
    /// What names it to [`Action::Select`].
    pub key: EntryKey,
    /// Its colour in the list ([`Review::entry_state`]).
    pub state: Option<State>,
    pub is_member: bool,
    /// Whether its members are the lines below it: never for a member, or a row of one.
    pub expanded: bool,
    /// The name its file goes by in the list, of a row's first shown member.
    pub label: String,
    /// The members the filter shows, and all it has. A member counts itself.
    pub shown: usize,
    pub total: usize,
    /// For a row, this, `original`, `wording`, `style` and `track` are its first shown member's.
    pub at_ms: i64,
    pub original: String,
    pub wording: String,
    pub style: String,
    /// What apply did or would do with it ([`Review::row_mark`], [`Review::mark`]).
    pub mark: Option<String>,
    /// A timing finding's track; empty for others.
    pub track: String,
    /// A timing finding's fix in short ([`Review::fix_summary`]).
    pub fix: Option<String>,
}

/// The list as `m` shows it now.
fn entry_views(m: &Review) -> Vec<EntryView> {
    m.entries().into_iter().map(|e| entry_view(m, e)).collect()
}

fn entry_view(m: &Review, e: Entry) -> EntryView {
    let shown = m.targets(e);
    let first = shown[0];
    let f = m.finding(first);
    let (is_member, expanded, total, mark) = match e {
        // As `entries` has it: an expanded row that a re-check left with one member shows none.
        Entry::Row(r) => {
            let total = m.rows[r].members.len();
            let expanded = total > 1 && m.expanded.contains(&r);
            (false, expanded, total, m.row_mark(r))
        }
        Entry::Member(_, i) => (true, false, 1, m.mark(i).map(str::to_string)),
    };
    EntryView {
        key: m.entry_key(e),
        state: m.entry_state(e),
        is_member,
        expanded,
        label: m.label(&f.file),
        shown: shown.len(),
        total,
        at_ms: m.start_ms(first),
        original: f.original.clone(),
        wording: m.wording(first).to_string(),
        style: f.style.clone(),
        mark,
        track: m.track_name(first).to_string(),
        fix: f.timing.is_some().then(|| m.fix_summary(&shown)),
    }
}

/// What `m` shows beside a list of `rows` rows.
fn summary(m: &Review, rows: usize) -> Summary {
    let (pending, accepted, rejected) = m.counts();
    Summary {
        title: m.run.shown_name(),
        selected: (m.selected < rows).then_some(m.selected),
        status: m.status_text(),
        pending,
        accepted,
        rejected,
        busy: m.progress.busy(),
    }
}

/// `old` into `new` as one splice, the rows between their matching start and their matching end;
/// `None` when they are the same. Two far-apart changes resend the rows between them: a UI applies
/// any splice the same way, and that costs less than a second kind of event would.
fn splice(old: &[EntryView], new: &[EntryView]) -> Option<Splice> {
    let at = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let tail = old[at..]
        .iter()
        .rev()
        .zip(new[at..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let removed = old.len() - at - tail;
    let inserted = &new[at..new.len() - tail];
    (removed > 0 || !inserted.is_empty()).then(|| Splice {
        at,
        removed,
        inserted: inserted.to_vec(),
    })
}

/// What the UI has been told, to work out what to tell it next.
#[derive(Default)]
struct Sent {
    rows: Vec<EntryView>,
    /// `None` until the first event.
    summary: Option<Summary>,
}

impl Sent {
    /// The event that brings the UI up to `m`; `None` when nothing it shows changed. The first
    /// always comes, with a splice even for no rows: it tells the UI the run has opened, and that
    /// a run with no findings has none.
    fn change(&mut self, m: &Review) -> Option<Event> {
        let rows = entry_views(m);
        let summary = summary(m, rows.len());
        let first = self.summary.is_none();
        let splice = splice(&self.rows, &rows);
        if splice.is_none() && self.summary.as_ref() == Some(&summary) {
            return None;
        }
        self.rows = rows;
        self.summary = Some(summary.clone());
        Some(Event::Changed {
            rows: splice.or_else(|| first.then(Splice::default)),
            summary,
        })
    }

    /// Tell `emit` what [`Sent::change`] finds, if it finds anything.
    fn publish(&mut self, m: &Review, emit: &mut dyn FnMut(Event)) {
        if let Some(event) = self.change(m) {
            emit(event);
        }
    }
}

/// What the UI's player says of a [`PlayerCommand`] it ran, or later of a file it had said was
/// open ([`Session::report`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerReport {
    Opened(Result<player::Opened, PlayerError>),
    Played(Result<PlayState, PlayerError>),
    Closed(Result<(), PlayerError>),
    Failed(PlayerError),
}

/// What the core thread takes: an input for `update`, from the UI or a job; or the stop that
/// [`Session::close`] queues.
#[allow(clippy::large_enum_variant)] // as `Input`, which it carries: a read-back is rare
enum Msg {
    Input(Input),
    Stop,
}

impl From<Input> for Msg {
    fn from(input: Input) -> Msg {
        Msg::Input(input)
    }
}

/// What the event channel carries: an event, or [`Session::close`]'s end marker.
enum Item {
    Event(Event),
    End,
}

/// The receiving end of the stream, and whether it is over.
struct Stream {
    items: Receiver<Item>,
    over: bool,
}

/// What [`Session::close`] knows of the core thread without waiting on it: only that thread holds
/// the `Review`, and it can be stuck in a task. Neither holds the lock across slow work.
#[derive(Default)]
struct Shared {
    /// `close` has run: no effect runs after it, and the session's end is its.
    closed: bool,
    /// A job runs or is starting, or review would end its groups anyway: `close` ends them.
    job: bool,
    /// What `close` says of the jobs ([`Review::outro`]), as of the last step the core thread
    /// finished: what the jobs said since, queued behind a task the thread is stuck in, or a job
    /// the step under way is starting, is not in it.
    outro: Vec<String>,
}

impl Shared {
    /// `close`'s part under the lock: whether this is the first `close`, whether a job runs or is
    /// starting (for a later `close`, whether one did for the first), and the outro to end with.
    fn close(&mut self) -> (bool, bool, Vec<String>) {
        let first = !std::mem::replace(&mut self.closed, true);
        (first, self.job, std::mem::take(&mut self.outro))
    }
}

/// `shared`, locked. A panic while it was held left it whole: each change is one assignment.
fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a session says that opens once the process's groups have ended: no job of its could start.
const GROUPS_ENDED: &str =
    "a review in this process stopped a job, or failed, so no other can open here";

/// One run under review, for a UI on any thread. `Send + Sync`: `next_event` blocks, so a UI calls
/// it from a thread of its own, and the others from wherever it likes. One per process at a time:
/// once one has ended the groups (it stopped a job, or failed), [`group::end`] has stopped new
/// process groups for the life of the process, and a session opened after that ends at once.
pub struct Session {
    inputs: Sender<Msg>,
    /// `close`'s way onto the stream. It also means the channel never disconnects, not even once
    /// the core thread has gone, so a `recv` alone would wait forever after [`Event::Ended`]:
    /// `Stream::over` is what ends the stream.
    events: Sender<Item>,
    stream: Mutex<Stream>,
    shared: Arc<Mutex<Shared>>,
}

/// What the core thread serves: all it needs, once the slow part of opening is done.
struct Opened {
    store: Store,
    state: PathBuf,
    review: Review,
}

/// `selector`'s view of `store` as a review ready to serve. The stat of every file with findings
/// (`file_notes`) can be slow, on a network share.
fn opened(
    store: Store,
    state: PathBuf,
    selector: &[String],
    karaoke: bool,
) -> anyhow::Result<Opened> {
    let run = store.view(&Selector::parse(selector)?)?;
    let notes = runner::file_notes(&run);
    let mut review = Review::new(run, notes);
    review.karaoke = karaoke;
    Ok(Opened {
        store,
        state,
        review,
    })
}

impl Session {
    /// Open the view `selector` names (`[run | paths…]`, as `subcheck review` takes it) in the
    /// process's store. Returns at once and cannot fail: what goes wrong opening it is
    /// [`Event::Ended`], as is opening in a process whose groups an earlier session ended; actions
    /// sent before then wait their turn. `karaoke`: its apply re-spaces per-letter karaoke, as
    /// `apply --karaoke` does.
    pub fn open(selector: Vec<String>, karaoke: bool) -> Session {
        Session::start(move || {
            let state = store::state_dir()?;
            let store = Store::open()?;
            opened(store, state, &selector, karaoke)
        })
    }

    /// The session over what `open` makes, on the core thread. `Session::open` reads the
    /// environment and the state directory there, so this is the seam a test comes in by, with a
    /// store and a review of its own.
    fn start(open: impl FnOnce() -> anyhow::Result<Opened> + Send + 'static) -> Session {
        let (inputs, queue) = mpsc::channel();
        let (events, items) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let session = Session {
            inputs: inputs.clone(),
            events: events.clone(),
            stream: Mutex::new(Stream { items, over: false }),
            shared: Arc::clone(&shared),
        };
        // Its jobs would wait for good to start a group: it ends before it opens anything.
        if group::ended() {
            let _ = events.send(Item::Event(Event::Ended {
                fatal: Some(GROUPS_ENDED.into()),
                outro: Vec::new(),
            }));
            return session;
        }
        let on_thread = events.clone();
        let spawned = std::thread::Builder::new()
            .name("review".into())
            .spawn(move || {
                // What `core` catches has a review to say more from. This is the rest: `open`
                // panicking, say. The stream must end whatever happens here: for a UI that never
                // calls `close`, nothing else ends it.
                let last = panic::catch_unwind(AssertUnwindSafe(|| {
                    core(open, &queue, inputs, &on_thread, &shared)
                }))
                .unwrap_or_else(|panic| {
                    Some(Event::Ended {
                        fatal: Some(panicked(panic.as_ref())),
                        outro: Vec::new(),
                    })
                });
                if let Some(last) = last {
                    let _ = on_thread.send(Item::Event(last));
                }
            });
        if let Err(e) = spawned {
            let _ = events.send(Item::Event(Event::Ended {
                fatal: Some(format!("starting review's thread: {e}")),
                outro: Vec::new(),
            }));
        }
        session
    }

    /// Queue `action` and return. After the end it goes nowhere.
    pub fn send(&self, action: Action) {
        let _ = self.inputs.send(Msg::Input(Input::Action(action)));
    }

    /// Say how the player's last command went, as `update` hears it from a player of its own.
    pub fn report(&self, report: PlayerReport) {
        let input = match report {
            PlayerReport::Opened(result) => Input::Opened(result),
            PlayerReport::Played(result) => Input::Played(result),
            PlayerReport::Closed(result) => Input::Closed(result),
            PlayerReport::Failed(error) => Input::PlayerFailed(error),
        };
        let _ = self.inputs.send(Msg::Input(input));
    }

    /// The next event, waiting for one; `None` once the session is over, every time and at once:
    /// after [`Event::Ended`], which [`Session::close`] puts after what was queued before it.
    pub fn next_event(&self) -> Option<Event> {
        let mut stream = self.stream.lock().unwrap_or_else(PoisonError::into_inner);
        if stream.over {
            return None;
        }
        let event = match stream.items.recv() {
            Ok(Item::Event(event)) => Some(event),
            // `self.events` keeps the channel open, so only the marker gets here.
            Ok(Item::End) | Err(_) => None,
        };
        stream.over = matches!(event, None | Some(Event::Ended { .. }));
        event
    }

    /// End the stream and stop the core thread when it can next be stopped; nothing the core
    /// thread was to do runs after this, a job's start among it. This never waits for the core
    /// thread, which can be stuck in a task on a stalled mount: with no job running or starting,
    /// and review not failed, it returns at once. Otherwise it first ends the process groups
    /// ([`group::end`]), so that a process that exits at once leaves none running, and that can
    /// wait, for a start stuck in exec or a file to remove on a stalled mount. Its own
    /// [`Event::Ended`] ends the stream, with the outro, which says how to finish a job it stopped,
    /// and with no `fatal`, whatever the groups ended for; unless review, ending by itself as this
    /// ran, put its own `Ended` on the stream first, which can carry review's `fatal`. A later
    /// call writes nothing, and returns once the groups the first is ending have ended, at once
    /// when the first ended none; letting the session go after a `close` returns at once. The
    /// core thread, if still stuck, ends with the process.
    pub fn close(&self) {
        // The core thread checks `closed` under the same lock before it starts a job, and marks
        // the job first: either it sees `closed` and starts nothing, or this sees the job.
        let (first, job, outro) = lock(&self.shared).close();
        // A later `close` too, so that it keeps the promise: `group::end` runs once, and a second
        // call returns once the first's is done, even one that panicked.
        if job {
            group::end();
        }
        // Only the first `close` writes to the stream: a later one's marker, from another thread,
        // could come before the first's `Ended` and end the stream there, without the outro that
        // says how to finish the job.
        if !first {
            return;
        }
        // Sends alone: `next_event` holds `stream`'s lock while it waits, so taking it here would
        // wait for the very event this ends.
        let _ = self
            .events
            .send(Item::Event(Event::Ended { fatal: None, outro }));
        let _ = self.events.send(Item::End);
        let _ = self.inputs.send(Msg::Stop);
    }
}

impl Drop for Session {
    /// A session let go of ends as a closed one does: the runner keeps the queue open, so the core
    /// thread would otherwise wait on it for good, with any job it started left running. After a
    /// `close`, it returns at once: that `close` ends the groups, and letting go, on whichever
    /// thread does it, must not wait for them on a stalled mount.
    fn drop(&mut self) {
        if lock(&self.shared).closed {
            return;
        }
        self.close();
    }
}

/// What a panic becomes, for [`Event::Ended`].
fn panicked(panic: &(dyn std::any::Any + Send)) -> String {
    format!("review panicked: {}", panic_text(panic))
}

/// The core thread: open, serve, and end; returns the last event. None once `close` has run while
/// review was served, as `close`'s own is then the stream's end; an open that fails returns its
/// end all the same, which no one reads after a `close`.
fn core(
    open: impl FnOnce() -> anyhow::Result<Opened>,
    queue: &Receiver<Msg>,
    inputs: Sender<Msg>,
    events: &Sender<Item>,
    shared: &Mutex<Shared>,
) -> Option<Event> {
    let Opened {
        store,
        state,
        mut review,
    } = match open() {
        Ok(opened) => opened,
        Err(e) => {
            return Some(Event::Ended {
                fatal: Some(format!("{e:#}")),
                outro: Vec::new(),
            });
        }
    };
    let mut runner = Runner::new(store, state, inputs);
    let served = panic::catch_unwind(AssertUnwindSafe(|| {
        serve(&mut review, &mut runner, queue, events, shared);
    }));
    let crash = served.err().map(|panic| panicked(panic.as_ref()));
    // `close` has ended the groups if a job ran, and no job started after it.
    if lock(shared).closed {
        return None;
    }
    // As the terminal's UI ends, on every way out: a job left running would outlive the process,
    // its mkvmerge or LLM calls being process groups of their own. `crash` decides the outro
    // alone, as there ([`Review::ending`]): were a panic in the loop to end the groups, the panic
    // tests run in-process would reach `group::end`, and under `cargo test` every later test that
    // starts a group would then wait forever.
    let ending = review.ending(crash.is_some());
    if ending.groups {
        group::end();
    }
    let fatal = crash.or_else(|| review.fatal.take());
    // After a panic, the outro is caught on its own: should it panic too, the outer catch would
    // report that one, and the first panic's text is the one that says what went wrong. A clean
    // end's is left to the outer catch, which reports its panic as fatal.
    let outro = if !ending.outro {
        Vec::new()
    } else if fatal.is_none() {
        review.outro()
    } else {
        panic::catch_unwind(AssertUnwindSafe(|| review.outro())).unwrap_or_default()
    };
    Some(Event::Ended { fatal, outro })
}

/// Publish the opening state, then take each input until `update` quits or the UI stops it.
fn serve(
    review: &mut Review,
    runner: &mut Runner<Msg>,
    queue: &Receiver<Msg>,
    events: &Sender<Item>,
    shared: &Mutex<Shared>,
) {
    let mut sent = Sent::default();
    let mut emit = |event| {
        let _ = events.send(Item::Event(event));
    };
    // The record as it opens is `Shared::default()`'s: no job, and no outro yet.
    sent.publish(review, &mut emit);
    while !review.should_quit {
        // The runner holds a sender, so `recv` cannot fail while this runs.
        let Ok(Msg::Input(input)) = queue.recv() else {
            break;
        };
        step(review, runner, &mut sent, input, &mut emit, shared);
        // What `close` left queued is dropped.
        if note(review, shared) {
            break;
        }
    }
}

/// `review` as `close` needs it after a step ([`Review::ending`]): whether ending now would end
/// the groups, and the outro. Whether `close` has run.
fn note(review: &Review, shared: &Mutex<Shared>) -> bool {
    let ending = review.ending(false);
    // An outro that panics, which only the end needs, must not end review in the middle: the
    // record keeps none, and the end makes it again, where its panic is reported.
    let outro = if ending.outro {
        panic::catch_unwind(AssertUnwindSafe(|| review.outro())).unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut shared = lock(shared);
    // Once closed, the record stays as the first `close` read it: a later `close` ends the groups
    // by it, and must end none the first left alone, as when review says a job runs whose start
    // `close` stopped.
    if !shared.closed {
        shared.job = ending.groups;
        shared.outro = outro;
    }
    shared.closed
}

/// Whether `effect` may run: none after `close`. A job's start is marked under the lock that
/// `close` takes, before the job starts, so that `close` ends its groups.
fn may_run(effect: &Effect, shared: &Mutex<Shared>) -> bool {
    let mut shared = lock(shared);
    if shared.closed {
        return false;
    }
    if matches!(
        effect,
        Effect::Task(Task::StartApply { .. } | Task::Recheck { .. })
    ) {
        shared.job = true;
    }
    true
}

/// One input: through `update`, the change published before any effect runs, so that a UI shows
/// what is about to happen before it does ("opening E02…" before the player command); then each
/// effect. A task's result goes back through `update` and is published in turn; a player command
/// goes to the UI, whose reply comes later as an input of its own.
fn step(
    review: &mut Review,
    runner: &mut Runner<Msg>,
    sent: &mut Sent,
    input: Input,
    emit: &mut dyn FnMut(Event),
    shared: &Mutex<Shared>,
) {
    let mut effects: VecDeque<Effect> = update::update(review, input, Timestamp::now()).into();
    sent.publish(review, emit);
    while let Some(effect) = effects.pop_front() {
        if !may_run(&effect, shared) {
            return;
        }
        match effect {
            Effect::Task(task) => {
                let result = runner.run(task);
                effects.extend(update::update(review, result, Timestamp::now()));
                sent.publish(review, emit);
            }
            Effect::Player(command) => emit(Event::Player(command)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::ExitStatusExt;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::TryLockError;
    use std::time::{Duration, Instant};

    use rustix::process::{Resource, Rlimit, setrlimit};

    use crate::list::Filter;
    use crate::player::{How, Moment, Playback};
    use crate::review::model::tests_support::{self, finding, many, sample, with_timing};
    use crate::review::model::{Pane, Row};
    use crate::review::progress::Scan;
    use crate::review::update::tests_support::{FIXED, TYPO, decide_as, now, saved, taken};
    use crate::scan;
    use crate::store::{
        Attempt, CheckedWith, Decision, FileRecord, Finding, Made, Record, Run, RunHeader, ScanRow,
    };

    fn act(m: &mut Review, action: Action) -> Vec<Effect> {
        update::update(m, Input::Action(action), now())
    }

    /// `sent` up to `m`, what it says of the rows, and the summary; an event must be due.
    fn changed(sent: &mut Sent, m: &Review) -> (Option<Splice>, Summary) {
        match sent.change(m) {
            Some(Event::Changed { rows, summary }) => (rows, summary),
            other => panic!("expected a change: {other:?}"),
        }
    }

    /// `m` and what has been told of it so far.
    fn told(m: &Review) -> Sent {
        let mut sent = Sent::default();
        sent.change(m);
        sent
    }

    #[test]
    fn the_first_event_is_a_splice_from_an_empty_list() {
        let m = sample();
        let (rows, summary) = changed(&mut Sent::default(), &m);
        let rows = rows.unwrap();
        assert_eq!((rows.at, rows.removed, rows.inserted.len()), (0, 0, 2));
        assert_eq!(rows.inserted, entry_views(&m));
        assert_eq!(
            summary,
            Summary {
                title: "run 2026-10-17T04-39-18".into(),
                selected: Some(0),
                status: None,
                pending: 4,
                accepted: 0,
                rejected: 0,
                busy: false,
            }
        );
    }

    #[test]
    fn a_run_with_no_findings_still_opens_with_a_splice_and_selects_nothing() {
        let run = Run {
            name: "r".into(),
            ..Run::default()
        };
        let m = Review::new(run, HashMap::new());
        let mut sent = Sent::default();
        let (rows, summary) = changed(&mut sent, &m);
        assert_eq!(rows, Some(Splice::default()));
        assert_eq!(summary.selected, None);
        assert_eq!(summary.title, "run r");
        assert_eq!(sent.change(&m), None, "nothing since");
    }

    #[test]
    fn a_filter_that_shows_nothing_empties_the_list_and_selects_nothing() {
        let mut m = sample();
        let mut sent = told(&m);
        act(&mut m, Action::SetFilter(Filter::Only(State::Accepted)));
        let (rows, summary) = changed(&mut sent, &m);
        assert_eq!(
            rows,
            Some(Splice {
                at: 0,
                removed: 2,
                inserted: vec![],
            })
        );
        assert_eq!(summary.selected, None);
    }

    #[test]
    fn rows_and_summary_both_unchanged_is_no_event() {
        let mut m = sample();
        let mut sent = told(&m);
        assert_eq!(sent.change(&m), None);
        assert_eq!(act(&mut m, Action::Dismiss), []);
        assert_eq!(sent.change(&m), None);
        // A row of one has nothing to expand.
        m.selected = 1;
        changed(&mut sent, &m);
        assert_eq!(act(&mut m, Action::Toggle), []);
        assert_eq!(sent.change(&m), None);
        // Selecting what is selected already.
        let snacks = m.selected_key().unwrap();
        assert_eq!(act(&mut m, Action::Select(snacks)), []);
        assert_eq!(sent.change(&m), None);
    }

    #[test]
    fn a_change_to_the_summary_alone_leaves_the_rows_out() {
        let mut m = sample();
        let mut sent = told(&m);
        let snacks = entry_views(&m)[1].key.clone();
        act(&mut m, Action::Select(snacks));
        let (rows, summary) = changed(&mut sent, &m);
        assert_eq!(rows, None);
        assert_eq!(summary.selected, Some(1));
    }

    #[test]
    fn a_decision_changes_its_row_in_place() {
        let mut m = sample();
        let mut sent = told(&m);
        let effects = act(&mut m, Action::Accept);
        // Not written yet: nothing to show.
        assert_eq!(sent.change(&m), None);
        update::update(&mut m, taken(effects), now());
        let (rows, summary) = changed(&mut sent, &m);
        let rows = rows.unwrap();
        assert_eq!((rows.at, rows.removed, rows.inserted.len()), (0, 1, 1));
        assert_eq!(rows.inserted[0].state, Some(State::Accepted));
        assert_eq!(summary.selected, Some(1), "the selection moved on");
        assert_eq!(summary.status.as_deref(), Some("accepted 3 findings"));
        assert_eq!((summary.pending, summary.accepted), (1, 3));
    }

    #[test]
    fn a_row_expanding_brings_its_members_and_collapsing_takes_them() {
        let mut m = sample();
        let mut sent = told(&m);
        act(&mut m, Action::Toggle);
        let (rows, _) = changed(&mut sent, &m);
        let rows = rows.unwrap();
        // The row itself, expanded now, and its three members; the row after it is left out.
        assert_eq!((rows.at, rows.removed, rows.inserted.len()), (0, 1, 4));
        assert!(rows.inserted[0].expanded && !rows.inserted[0].is_member);
        let members: Vec<(bool, &str)> = rows.inserted[1..]
            .iter()
            .map(|v| (v.is_member, v.label.as_str()))
            .collect();
        assert_eq!(members, [(true, "E01"), (true, "E02"), (true, "E03")]);
        act(&mut m, Action::Toggle);
        let (rows, _) = changed(&mut sent, &m);
        let rows = rows.unwrap();
        assert_eq!((rows.at, rows.removed, rows.inserted.len()), (0, 4, 1));
        assert!(!rows.inserted[0].expanded);
    }

    #[test]
    fn a_row_leaves_under_a_filter_when_its_last_shown_member_is_decided() {
        let mut m = sample();
        act(&mut m, Action::SetFilter(Filter::Only(State::Pending)));
        let mut sent = told(&m);
        // The lyric's row, a member of it decided: one fewer shown, the row still there.
        act(&mut m, Action::Toggle);
        changed(&mut sent, &m);
        let e02 = entry_views(&m)[2].key.clone();
        act(&mut m, Action::Select(e02));
        changed(&mut sent, &m);
        let effects = act(&mut m, Action::Reject);
        update::update(&mut m, taken(effects), now());
        let (rows, _) = changed(&mut sent, &m);
        let rows = rows.unwrap();
        // The row's count, and the member gone from under it, as one splice.
        assert_eq!((rows.at, rows.removed, rows.inserted.len()), (0, 3, 2));
        assert_eq!((rows.inserted[0].shown, rows.inserted[0].total), (2, 3));
        // The snacks row, alone in its row: the row goes.
        let snacks = entry_views(&m)[3].key.clone();
        act(&mut m, Action::Select(snacks));
        changed(&mut sent, &m);
        let effects = act(&mut m, Action::Accept);
        update::update(&mut m, taken(effects), now());
        let (rows, summary) = changed(&mut sent, &m);
        assert_eq!(
            rows,
            Some(Splice {
                at: 3,
                removed: 1,
                inserted: vec![],
            })
        );
        assert_eq!(summary.selected, Some(2), "the last entry left");
    }

    #[test]
    fn a_regroup_is_one_splice_over_what_it_moved() {
        let mut m = sample();
        let mut sent = told(&m);
        // E02 checked again: its lyric is gone, its snacks line stays, and a new line comes early
        // in the episode. The lyric's row loses a member and the new row comes after it; the
        // snacks row, last, is left out.
        let mut again = saved(
            &m,
            2,
            vec![finding(6, 2, "An erly line.", "An early line.", 1_000)],
        );
        if let Input::ReadBack { view: Ok(view), .. } = &mut again {
            view.findings.retain(|f| f.id != 2);
        }
        update::update(&mut m, again, now());
        let (rows, _) = changed(&mut sent, &m);
        let rows = rows.unwrap();
        assert_eq!((rows.at, rows.removed, rows.inserted.len()), (0, 1, 2));
        assert_eq!(rows.inserted[0].key, EntryKey::Row(vec![1, 4]));
        assert_eq!(rows.inserted[1].key, EntryKey::Row(vec![6]));
    }

    #[test]
    fn two_far_apart_changes_resend_the_rows_between() {
        let mut m = many(5);
        let mut sent = told(&m);
        decide_as(&mut m, &[2, 4], State::Rejected, None);
        let (rows, _) = changed(&mut sent, &m);
        let rows = rows.unwrap();
        assert_eq!((rows.at, rows.removed, rows.inserted.len()), (1, 3, 3));
    }

    #[test]
    fn a_rows_view_says_what_its_list_line_says() {
        let mut m = sample();
        let views = entry_views(&m);
        let lyric = &views[0];
        assert_eq!(lyric.key, EntryKey::Row(vec![1, 2, 4]));
        assert_eq!(lyric.state, Some(State::Pending));
        assert!(!lyric.is_member && !lyric.expanded);
        assert_eq!(lyric.label, "E01");
        assert_eq!((lyric.shown, lyric.total), (3, 3));
        assert_eq!(lyric.at_ms, 165_060);
        assert_eq!(lyric.original, TYPO);
        assert_eq!(lyric.wording, FIXED);
        assert_eq!(lyric.style, "OP en");
        assert_eq!(lyric.mark, None);
        assert_eq!(lyric.track, "");
        assert_eq!(lyric.fix, None);
        // Decided differently, and a filter leaving two of three: the rule is the list's.
        decide_as(&mut m, &[1], State::Accepted, Some("My own."));
        decide_as(&mut m, &[2], State::Rejected, None);
        decide_as(&mut m, &[4], State::Rejected, None);
        m.run.applied.insert(2);
        let lyric = &entry_views(&m)[0];
        assert_eq!(lyric.state, None, "decided members differ");
        assert_eq!(lyric.wording, "My own.");
        assert_eq!(lyric.mark.as_deref(), Some("applied 1"));
        m.filter = Filter::Only(State::Rejected);
        let lyric = &entry_views(&m)[0];
        assert_eq!(lyric.state, Some(State::Rejected));
        assert_eq!(lyric.label, "E02", "the first it shows");
        assert_eq!((lyric.shown, lyric.total), (2, 3));
    }

    #[test]
    fn a_members_view_is_its_own_finding_and_counts_itself() {
        let mut m = sample();
        m.expanded.insert(0);
        decide_as(&mut m, &[2], State::Accepted, None);
        m.run.applied.insert(2);
        let views = entry_views(&m);
        let e02 = &views[2];
        assert_eq!(e02.key, EntryKey::Member(2));
        assert_eq!(e02.state, Some(State::Accepted));
        assert!(e02.is_member && !e02.expanded);
        assert_eq!(e02.label, "E02");
        assert_eq!((e02.shown, e02.total), (1, 1));
        assert_eq!(e02.mark.as_deref(), Some("applied"));
        assert_eq!(
            views[0].state,
            Some(State::Pending),
            "the row: two still to do"
        );
        assert!(views[0].expanded);
    }

    #[test]
    fn a_timing_rows_view_has_its_track_and_its_fix_and_a_member_its_own() {
        let mut m = with_timing();
        let row = entry_views(&m)[1].clone();
        assert_eq!(row.key, EntryKey::Row(vec![11, 12, 13]));
        assert_eq!(row.track, "English Full");
        assert_eq!(row.fix.as_deref(), Some("−1 frame ×2 · partial ×1"));
        m.expanded.insert(1);
        let partial = entry_views(&m)[3].clone();
        assert_eq!(partial.key, EntryKey::Member(12));
        assert_eq!(partial.track, "English Full");
        assert_eq!(partial.fix.as_deref(), Some("partial"));
    }

    #[test]
    fn a_row_left_with_one_member_is_not_expanded_though_it_is_marked_so() {
        let mut m = many(1);
        m.expanded.insert(0);
        let views = entry_views(&m);
        assert_eq!(views.len(), 1, "as `entries` has it");
        assert!(!views[0].expanded);
    }

    /// How long a test waits for what should come at once: a session that hangs fails its test,
    /// and does not hang the run.
    const LIMIT: Duration = Duration::from_secs(30);

    /// What `f` returns, which must come within [`LIMIT`].
    fn within<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(LIMIT).expect("this should not wait")
    }

    fn next(s: &Arc<Session>) -> Option<Event> {
        let s = Arc::clone(s);
        within(move || s.next_event())
    }

    /// The events up to and including `Ended`, which must come.
    fn to_the_end(s: &Arc<Session>) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            let event = next(s).expect("the stream ended without `Ended`");
            let last = matches!(event, Event::Ended { .. });
            events.push(event);
            if last {
                return events;
            }
        }
    }

    /// The last of `events`, which must be the end: its text and its outro.
    fn end_of(events: &[Event]) -> (Option<&str>, &[String]) {
        let Some(Event::Ended { fatal, outro }) = events.last() else {
            panic!("no end: {events:?}");
        };
        (fatal.as_deref(), outro)
    }

    /// A check of `record`'s file saved into scan `scan`, with `findings`.
    fn save(store: &mut Store, scan: i64, record: &FileRecord, findings: Vec<Finding>) {
        let attempt = Attempt {
            scan,
            path: &record.path,
            size: record.size,
            mtime: record.mtime,
            tracks: &record.tracks,
            truncated: None,
            at: Timestamp::UNIX_EPOCH,
        };
        let made = Made {
            with: CheckedWith::new("claude", &["eng".into()], 1),
            lines: 1,
            findings,
        };
        store.save(&attempt, vec![made]).unwrap();
    }

    /// The header of the runs these tests make.
    fn header() -> RunHeader {
        RunHeader {
            started: Timestamp::UNIX_EPOCH,
            backend: "claude".into(),
            command: vec![],
            prompt_version: 1,
            version: "0".into(),
            langs: vec!["eng".into()],
        }
    }

    fn begin(store: &mut Store, paths: &[PathBuf]) -> ScanRow {
        store.begin_scan(&header(), paths).unwrap()
    }

    /// A database in `dir` with the sample run's findings saved in it (IDs 1 to 4, as the sample's;
    /// two rows), and the run's name.
    fn seeded(dir: &Path) -> (Store, String) {
        let mut store = Store::open_at(&dir.join("subcheck.db")).unwrap();
        let sample = tests_support::run();
        let paths: Vec<PathBuf> = sample
            .files
            .iter()
            .map(|f| PathBuf::from(&f.path))
            .collect();
        let scan = begin(&mut store, &paths);
        for record in &sample.files {
            let of_file = |f: &&Finding| f.file == record.path;
            let findings = sample.findings.iter().filter(of_file).cloned().collect();
            save(&mut store, scan.id, record, findings);
        }
        (store, scan.name)
    }

    /// A session on `seeded`'s run, `change` done to its review as it opens. The sample's files are
    /// not on disk, so their notes are cleared, or `Play` would refuse them.
    fn session(dir: &Path, change: impl FnOnce(&mut Review) + Send + 'static) -> Arc<Session> {
        let (store, name) = seeded(dir);
        let state = dir.join("state");
        Arc::new(Session::start(move || {
            let mut opened = opened(store, state, &[name], false)?;
            opened.review.notes.clear();
            change(&mut opened.review);
            Ok(opened)
        }))
    }

    /// The next event, a change; what it says of the rows, and the summary.
    fn next_change(s: &Arc<Session>) -> (Option<Splice>, Summary) {
        match next(s) {
            Some(Event::Changed { rows, summary }) => (rows, summary),
            other => panic!("expected a change: {other:?}"),
        }
    }

    #[test]
    fn a_session_can_be_shared_between_threads() {
        fn shared<T: Send + Sync>() {}
        shared::<Session>();
    }

    #[test]
    fn opening_reads_the_view_and_its_files_notes_and_takes_karaoke() {
        let dir = tempfile::tempdir().unwrap();
        let (store, name) = seeded(dir.path());
        let state = dir.path().join("state");
        let opened = opened(store, state.clone(), &[name], true).unwrap();
        assert_eq!(opened.state, state);
        assert_eq!(opened.review.rows.len(), 2);
        assert!(opened.review.karaoke);
        // The sample's three files are not on disk.
        assert_eq!(opened.review.notes.len(), 3);
    }

    #[test]
    fn a_run_with_no_findings_opens_on_zero_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_at(&dir.path().join("subcheck.db")).unwrap();
        let name = begin(&mut store, &[]).name;
        let title = format!("run {name}");
        let state = dir.path().join("state");
        let s = Arc::new(Session::start(move || opened(store, state, &[name], false)));
        let (rows, summary) = next_change(&s);
        assert_eq!(rows, Some(Splice::default()));
        assert_eq!(summary.selected, None);
        assert_eq!(summary.title, title);
    }

    #[test]
    fn an_action_sent_before_the_store_has_opened_waits_its_turn() {
        let dir = tempfile::tempdir().unwrap();
        let (open_up, gate) = mpsc::channel::<()>();
        let s = session(dir.path(), move |_| {
            let _ = gate.recv();
        });
        s.send(Action::Accept);
        open_up.send(()).unwrap();
        let (first, summary) = next_change(&s);
        assert_eq!(first.unwrap().inserted.len(), 2);
        assert_eq!(summary.accepted, 0, "the first shows the run as it opened");
        let (rows, summary) = next_change(&s);
        assert_eq!(rows.unwrap().at, 0);
        assert_eq!(summary.accepted, 3);
    }

    #[test]
    fn a_change_is_published_before_its_effects_run_and_a_tasks_result_after_it() {
        let dir = tempfile::tempdir().unwrap();
        // E01 on disk, under its own name, so that the stat `o` makes finds it.
        let e01 = dir.path().join("K-On! - E01 - Episode.mkv");
        std::fs::write(&e01, "1").unwrap();
        let file = e01.to_string_lossy().into_owned();
        let on_disk = file.clone();
        let s = session(dir.path(), move |m| {
            for f in &mut m.run.findings {
                if f.file == tests_support::path(1) {
                    f.file.clone_from(&on_disk);
                }
            }
            for f in &mut m.run.files {
                if f.path == tests_support::path(1) {
                    f.path.clone_from(&on_disk);
                }
            }
        });
        next(&s);
        // A player command: what is about to happen; then, once the core thread has read the
        // file's stamp, which changes nothing shown, the command. The report comes back as an
        // input of its own.
        s.send(Action::Play);
        let (rows, summary) = next_change(&s);
        assert_eq!(rows, None);
        assert_eq!(
            summary.status.as_deref(),
            Some("opening E01 at 0:02:45.060")
        );
        assert_eq!(
            next(&s),
            Some(Event::Player(PlayerCommand::Open {
                moment: Moment {
                    file,
                    start_us: 165_064_500,
                    sid: Some(1),
                },
                how: How::Load,
                options: player::load_options(),
            }))
        );
        s.report(PlayerReport::Opened(Ok(player::Opened { paused: true })));
        let (rows, summary) = next_change(&s);
        assert_eq!(rows, None);
        assert_eq!(summary.status.as_deref(), Some("opened in mpv, paused"));
        // A task whose result changes the rows: the status line cleared first, on its own, while
        // the write is still to run; then the rows, once the store has said.
        s.send(Action::Accept);
        let (rows, summary) = next_change(&s);
        assert_eq!(rows, None);
        assert_eq!(summary.status, None);
        assert_eq!(summary.accepted, 0);
        let (rows, summary) = next_change(&s);
        assert_eq!(rows.unwrap().inserted[0].state, Some(State::Accepted));
        assert_eq!(summary.status.as_deref(), Some("accepted 3 findings"));
        assert_eq!(summary.accepted, 3);
        // The other two commands, and what their reports say.
        s.send(Action::Playback(Playback::Pause));
        let (_, summary) = next_change(&s);
        assert_eq!(summary.status, None);
        assert_eq!(
            next(&s),
            Some(Event::Player(PlayerCommand::Playback(Playback::Pause)))
        );
        s.report(PlayerReport::Played(Ok(PlayState {
            paused: true,
            at_ms: 1000,
        })));
        let (_, summary) = next_change(&s);
        assert_eq!(summary.status.as_deref(), Some("⏸ 0:00:01.000"));
        s.send(Action::ClosePlayer);
        next_change(&s);
        assert_eq!(next(&s), Some(Event::Player(PlayerCommand::Close)));
        s.report(PlayerReport::Closed(Err(PlayerError::NotOpen)));
        let (_, summary) = next_change(&s);
        assert_eq!(summary.status.as_deref(), Some("mpv is not open"));
        // The selection on E02 since the accept, a file that is not on disk: the stat says so,
        // and nothing goes to the player.
        s.send(Action::Play);
        next_change(&s);
        let (_, summary) = next_change(&s);
        assert_eq!(
            summary.status,
            Some(format!("file missing: {}", tests_support::path(2)))
        );
    }

    // Another review in the same session, as `v` swaps the view: B's rows in place of A's, in one
    // splice, under B's name; words that select nothing leave the view as it was, and say why.
    #[test]
    fn open_view_shows_another_run_in_place_of_the_one_shown() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, a) = seeded(dir.path());
        let record = tests_support::file(5);
        let b = begin(&mut store, &[PathBuf::from(&record.path)]);
        save(
            &mut store,
            b.id,
            &record,
            vec![finding(0, 5, TYPO, FIXED, 1_000)],
        );
        let state = dir.path().join("state");
        let s = Arc::new(Session::start(move || opened(store, state, &[a], false)));
        assert_eq!(next_change(&s).0.unwrap().inserted.len(), 2);
        s.send(Action::OpenView(vec![b.name.clone()]));
        let (rows, summary) = next_change(&s);
        let rows = rows.unwrap();
        assert_eq!((rows.at, rows.removed, rows.inserted.len()), (0, 2, 1));
        assert_eq!(summary.title, format!("run {}", b.name));
        s.send(Action::OpenView(vec!["nope".into()]));
        let (rows, summary) = next_change(&s);
        assert_eq!(rows, None);
        assert_eq!(
            summary.status.as_deref(),
            Some("could not switch views: no run named nope")
        );
    }

    // `close`'s locked part first, with the core thread between `o`'s change and its stat: neither
    // the stat nor the player's command it would lead to runs. No job runs, so no group ends in
    // this process.
    #[test]
    fn nothing_the_core_thread_was_to_do_runs_after_close() {
        let dir = tempfile::tempdir().unwrap();
        // E01 on disk, under its own name, so that the stat finds it and the open would go out.
        let e01 = dir.path().join("K-On! - E01 - Episode.mkv");
        std::fs::write(&e01, "1").unwrap();
        let on_disk = e01.to_string_lossy().into_owned();
        let s = session(dir.path(), move |m| {
            for f in &mut m.run.findings {
                if f.file == tests_support::path(1) {
                    f.file.clone_from(&on_disk);
                }
            }
        });
        next(&s);
        let mut held = lock(&s.shared);
        s.send(Action::Play);
        assert_eq!(
            next_change(&s).1.status.as_deref(),
            Some("opening E01 at 0:02:45.060")
        );
        assert!(matches!(held.close(), (true, false, _)));
        drop(held);
        let stopped = Arc::clone(&s);
        within(move || {
            while stopped.inputs.send(Msg::Stop).is_ok() {
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        // With the core thread gone, all it sent is on the channel, and nothing else writes to it:
        // the `close` played here was the first, and a later one writes nothing.
        let stream = s.stream.lock().unwrap();
        let after: Vec<Option<Event>> = stream
            .items
            .try_iter()
            .map(|item| match item {
                Item::Event(event) => Some(event),
                Item::End => None,
            })
            .collect();
        assert_eq!(after, []);
    }

    #[test]
    fn an_input_that_changes_nothing_is_silent_and_a_clean_quit_ends_with_nothing_to_say() {
        let dir = tempfile::tempdir().unwrap();
        let s = session(dir.path(), |_| {});
        next(&s);
        s.send(Action::Dismiss);
        s.send(Action::Quit);
        assert_eq!(
            next(&s),
            Some(Event::Ended {
                fatal: None,
                outro: vec![],
            })
        );
    }

    #[test]
    fn a_failed_open_is_the_one_event_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = seeded(dir.path());
        let state = dir.path().join("state");
        let s = Arc::new(Session::start(move || {
            opened(store, state, &["nope".into()], false)
        }));
        assert_eq!(
            next(&s),
            Some(Event::Ended {
                fatal: Some("no run named nope".into()),
                outro: vec![],
            })
        );
        assert_eq!(next(&s), None);
    }

    #[test]
    fn a_panic_opening_is_a_fatal_end() {
        let s = Arc::new(Session::start(|| panic!("boom")));
        assert_eq!(
            next(&s),
            Some(Event::Ended {
                fatal: Some("review panicked: boom".into()),
                outro: vec![],
            })
        );
        assert_eq!(next(&s), None);
    }

    /// The outro of `events`, which end on a panic of the loop's: `review panicked: `, then the
    /// panic's own text, which is Rust's and not ours to pin past its start.
    fn panicked_end(events: &[Event]) -> &[String] {
        let (fatal, outro) = end_of(events);
        let fatal = fatal.unwrap();
        assert!(
            fatal.starts_with("review panicked: index out of bounds"),
            "{fatal}"
        );
        outro
    }

    #[test]
    fn a_panic_on_the_first_publish_is_a_fatal_end() {
        // A row naming a finding the run does not have: making its view panics.
        let dir = tempfile::tempdir().unwrap();
        let s = session(dir.path(), |review| {
            review.rows.push(Row { members: vec![99] });
        });
        let events = to_the_end(&s);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(panicked_end(&events), Vec::<String>::new());
        assert_eq!(next(&s), None);
    }

    #[test]
    fn a_panic_in_update_is_a_fatal_end() {
        // An undo step with no findings, which `refresh` and `switch` never leave: `update` panics
        // on the write's result, in the loop, after the store has run it.
        let dir = tempfile::tempdir().unwrap();
        let s = session(dir.path(), |review| review.undo = vec![vec![]]);
        let (rows, _) = next_change(&s);
        assert_eq!(rows.unwrap().inserted.len(), 2);
        s.send(Action::Undo);
        let events = to_the_end(&s);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(panicked_end(&events), Vec::<String>::new());
        assert_eq!(next(&s), None);
    }

    #[test]
    fn a_panic_in_the_outro_of_a_clean_end_is_a_fatal_end() {
        // A scan said to have added more files to the run than a count holds: the outro sums them,
        // which overflows and panics in a test build. The status line goes by the dry run that
        // ended after it, so the outro is the first to sum them. Nothing runs and `review.fatal`
        // is empty, so `group::end` stays out of the test process.
        let dir = tempfile::tempdir().unwrap();
        let s = session(dir.path(), |review| {
            let p = &mut review.progress;
            p.start_apply(true, String::new());
            p.apply_ended(Err("stopped".into()));
            p.adding = true;
            p.scan = Scan::Ended(Ok(scan::Summary {
                checked: usize::MAX,
                failed: 1,
                ..scan::Summary::default()
            }));
        });
        next_change(&s);
        // Each step makes the record's outro too: review goes on all the same.
        s.send(Action::Undo);
        assert_eq!(next_change(&s).1.status.as_deref(), Some("nothing to undo"));
        s.send(Action::Dismiss);
        next_change(&s);
        s.send(Action::Quit);
        let events = to_the_end(&s);
        assert_eq!(events.len(), 1, "{events:?}");
        let (fatal, outro) = end_of(&events);
        let fatal = fatal.unwrap();
        assert!(
            fatal.starts_with("review panicked: attempt to add with overflow"),
            "{fatal}"
        );
        assert_eq!(outro, Vec::<String>::new());
        assert_eq!(next(&s), None);
    }

    #[test]
    fn the_stream_stays_over_after_the_end_and_nothing_sent_then_matters() {
        let dir = tempfile::tempdir().unwrap();
        let s = session(dir.path(), |_| {});
        next(&s);
        s.send(Action::Quit);
        assert!(matches!(next(&s), Some(Event::Ended { .. })));
        for _ in 0..3 {
            assert_eq!(next(&s), None);
        }
        s.send(Action::Accept);
        s.report(PlayerReport::Opened(Ok(player::Opened { paused: true })));
        s.close();
        s.close();
        assert_eq!(next(&s), None);
    }

    #[test]
    fn close_ends_the_stream_at_once_whatever_the_core_thread_is_doing() {
        // The core thread stuck opening, as on a stalled mount, so it reads nothing of the queue.
        let (let_go, stuck) = mpsc::channel::<()>();
        let s = Arc::new(Session::start(move || {
            let _ = stuck.recv();
            anyhow::bail!("let go")
        }));
        // A UI's pump, waiting on the stream with its lock held. Nothing is on the stream, so once
        // the pump holds the lock it waits in `recv` with it, and `close` comes after.
        let pump = Arc::clone(&s);
        let (said, waiting) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = said.send(pump.next_event());
        });
        let held = Arc::clone(&s);
        within(move || {
            while !matches!(held.stream.try_lock(), Err(TryLockError::WouldBlock)) {
                std::thread::yield_now();
            }
        });
        let closing = Arc::clone(&s);
        within(move || closing.close());
        // `close`'s own end: review had not opened, so it has nothing to say.
        assert_eq!(
            waiting.recv_timeout(LIMIT).unwrap(),
            Some(Event::Ended {
                fatal: None,
                outro: vec![],
            })
        );
        assert_eq!(next(&s), None);
        // Twice is harmless, and so is what comes after.
        s.close();
        s.send(Action::Quit);
        s.report(PlayerReport::Closed(Ok(())));
        assert_eq!(next(&s), None);
        drop(let_go);
    }

    #[test]
    fn every_splice_applied_to_a_copy_of_the_rows_makes_the_real_list() {
        let dir = tempfile::tempdir().unwrap();
        let (store, name) = seeded(dir.path());
        let db = store.path().to_path_buf();
        let run = name.clone();
        let Opened {
            store,
            state,
            mut review,
        } = opened(store, dir.path().join("state"), &[name], false).unwrap();
        review.notes.clear();
        let (tx, _rx) = mpsc::channel();
        let mut runner = Runner::new(store, state, tx);
        let mut sent = Sent::default();
        let open = Mutex::new(Shared::default());
        let mut copy: Vec<EntryView> = Vec::new();
        // As a UI does it, whatever the event's order.
        let apply = |event: Event, copy: &mut Vec<EntryView>| {
            if let Event::Changed {
                rows: Some(splice), ..
            } = event
            {
                copy.splice(splice.at..splice.at + splice.removed, splice.inserted);
            }
        };
        apply(sent.change(&review).unwrap(), &mut copy);
        assert_eq!(copy, entry_views(&review));
        let mut go =
            |review: &mut Review, sent: &mut Sent, input: Input, copy: &mut Vec<EntryView>| {
                let what = format!("{input:?}");
                let mut heard = Vec::new();
                step(
                    review,
                    &mut runner,
                    sent,
                    input,
                    &mut |e| heard.push(e),
                    &open,
                );
                for event in heard {
                    apply(event, copy);
                }
                assert_eq!(*copy, entry_views(review), "after {what}");
            };
        let only = |state| Action::SetFilter(Filter::Only(state));
        for action in [
            Action::Toggle,
            Action::Select(EntryKey::Member(2)),
            Action::Accept,
            Action::Reject,
            Action::Undo,
            only(State::Pending),
            Action::Accept,
            only(State::Accepted),
            Action::Select(EntryKey::Row(vec![3])),
            Action::SetFilter(Filter::All),
            Action::NextFile,
            Action::PreviousFile,
            Action::Clear,
            Action::Edit,
            Action::Reword("Fixed by hand.".into()),
            Action::Undo,
            Action::Undo,
            Action::Toggle,
            Action::Select(EntryKey::Row(vec![3])),
            Action::Reject,
            only(State::Rejected),
            Action::Accept,
            Action::SetFilter(Filter::All),
            Action::SwitchView,
            Action::Toggle,
            Action::SwitchView,
            Action::Play,
            Action::ClosePlayer,
            Action::ShowPane(Pane::Files),
            Action::ShowPane(Pane::Findings),
            Action::Dismiss,
        ] {
            go(&mut review, &mut sent, Input::Action(action), &mut copy);
        }
        // E02 checked again, on another connection as a scan's thread has: the lyric back, the
        // snacks line gone, a new line early on. The rows are made again from the store.
        let mut scans = Store::open_at(&db).unwrap();
        let scan = scans.scan_named(&run).unwrap().unwrap();
        let record = tests_support::file(2);
        let e02 = vec![
            tests_support::finding(0, 2, TYPO, FIXED, 165_060),
            tests_support::finding(0, 2, "An erly line.", "An early line.", 1_000),
        ];
        save(&mut scans, scan.id, &record, e02);
        let appended = scan::Event::Appended(vec![Record::File(record)]);
        go(&mut review, &mut sent, Input::Scan(appended), &mut copy);
        assert!(copy.iter().any(|v| v.original == "An erly line."));
    }

    const CHILD: &str = "review::session::tests::child_that_ends_a_session";

    /// A database in `dir` with one finding, accepted, on a FIFO no one writes to: a job that opens
    /// it waits there for good, as on a stalled mount, where a dry run on a real file could end
    /// before the test has acted on it.
    fn stalled(dir: &Path) -> (Store, String) {
        let fifo = dir.join("stalled.mkv");
        let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
        assert!(made.success());
        let path = fifo.to_string_lossy().into_owned();
        let mut store = Store::open_at(&dir.join("subcheck.db")).unwrap();
        let scan = begin(&mut store, std::slice::from_ref(&fifo));
        let record = FileRecord {
            path: path.clone(),
            ..tests_support::file(1)
        };
        let finding = Finding {
            file: path,
            ..tests_support::finding(0, 1, TYPO, FIXED, 0)
        };
        save(&mut store, scan.id, &record, vec![finding]);
        let accepted = Decision {
            finding: 1,
            state: State::Accepted,
            edited: None,
            at: Timestamp::UNIX_EPOCH,
        };
        store.decide(&[accepted]).unwrap();
        (store, scan.name)
    }

    /// A session with a dry run under way that never ends (see [`stalled`]), `change` done to its
    /// review as it opens; and the name of its run. Returns once the review says a job runs and
    /// its start is marked, so that a `close` after it sees the job: the change comes before the
    /// effect that starts it.
    fn during_a_job(
        dir: &Path,
        change: impl FnOnce(&mut Review) + Send + 'static,
    ) -> (Arc<Session>, String) {
        let (store, name) = stalled(dir);
        let state = dir.join("state");
        let selector = [name.clone()];
        let s = Arc::new(Session::start(move || {
            let mut opened = opened(store, state, &selector, false)?;
            change(&mut opened.review);
            Ok(opened)
        }));
        s.send(Action::Apply { dry_run: true });
        while !next_change(&s).1.busy {}
        let marked = Arc::clone(&s);
        within(move || {
            while !lock(&marked.shared).job {
                std::thread::yield_now();
            }
        });
        (s, name)
    }

    /// A process group of its own, as a job's mkvmerge or LLM call is. `group::end` kills it, and
    /// nothing else here shows `end` ran: a dry run only reads, so it starts no group. Started once
    /// the session is set up, so that its 20 s are the ending's alone, and a slow machine's setup
    /// cannot run them out, which would look like `end` not running. Its guard is never dropped:
    /// one dropped after `end` would wait forever.
    fn a_group() -> Child {
        let (child, registered) = group::spawn(
            Command::new("sleep")
                .arg("20")
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .unwrap();
        std::mem::forget(registered);
        child
    }

    /// That `group::end` ran: a group's `sleep` died of its SIGKILL, and did not run out.
    fn assert_ended(mut group: Child) {
        let status = within(move || group.wait().unwrap());
        assert_eq!(status.signal(), Some(libc::SIGKILL), "{status}");
    }

    /// Whether `group` dies of a SIGKILL within `limit`.
    fn killed_within(group: &mut Child, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if let Some(status) = group.try_wait().unwrap() {
                return status.signal() == Some(libc::SIGKILL);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// The store in `dir` held by a write of another connection's, as another process can hold
    /// it: review's writes wait for it, up to the store's busy timeout.
    fn hold_the_store(dir: &Path) -> rusqlite::Connection {
        let held = rusqlite::Connection::open(dir.join("subcheck.db")).unwrap();
        held.execute_batch("BEGIN IMMEDIATE").unwrap();
        held
    }

    /// Whether something reads `fifo` within `limit`: the stalled dry run, had it started.
    fn read_within(fifo: &Path, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            // With no reader, a writer's open that does not wait fails.
            let open = OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(fifo);
            if open.is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// A file in `dir` whose making waits until the returned sender sends or goes: `group::end`
    /// removes the files last, under the lock the making holds, so an `end` started after this
    /// waits in it, once the groups are dead, until then.
    fn holding_end(dir: &Path) -> Sender<()> {
        let (entered, inside) = mpsc::channel();
        let (let_go, held) = mpsc::channel::<()>();
        let file = dir.join("being-made");
        std::thread::spawn(move || {
            let made = group::create_removing(&file, |path| {
                let _ = entered.send(());
                let _ = held.recv();
                std::fs::write(path, "")
            });
            // Dropped after `end`, it would wait forever.
            std::mem::forget(made);
        });
        inside.recv_timeout(LIMIT).unwrap();
        let_go
    }

    /// That `outro`, of a job stopped on run `name`, ends on the command that finishes it.
    fn says_how_to_finish(outro: &[String], name: &str) {
        let finish = format!("Finish it with: subcheck apply {name} --dry-run");
        assert_eq!(outro.last(), Some(&finish), "{outro:?}");
    }

    /// Run by `in_a_child`. Each way of ending a session runs here, in a process of its own:
    /// `group::end` holds `group`'s registry for good, process-wide, and `cargo test` runs this
    /// binary's tests in one process, where every later test that starts a group would wait
    /// forever. (nextest gives each test a process, so the local gate alone would not show it.)
    /// The same goes for reading the environment: `Command::env` sets it for the child alone.
    #[test]
    #[ignore = "the child of the tests of how a session ends"]
    #[allow(clippy::too_many_lines)] // an arm for every way a session ends
    fn child_that_ends_a_session() {
        let Some(how) = std::env::var_os("SUBCHECK_TEST_HOW") else {
            return;
        };
        let dir = PathBuf::from(std::env::var_os("SUBCHECK_TEST_DIR").unwrap());
        let dir = dir.as_path();
        match how.to_str().unwrap() {
            "close" => {
                let (s, name) = during_a_job(dir, |_| {});
                // The core thread stuck in a write, on a store another process holds: a message
                // first, so that the write's step shows by letting it go. The dry run's own steps
                // change what shows too, and leave the message.
                let undone = Some("nothing to undo".to_string());
                s.send(Action::Undo);
                while next_change(&s).1.status != undone {}
                let held = hold_the_store(dir);
                s.send(Action::Reject);
                while next_change(&s).1.status == undone {}
                let mut group = a_group();
                let closing = Arc::clone(&s);
                within(move || closing.close());
                // Not when the core thread could next stop, the write's busy timeout away.
                assert!(killed_within(&mut group, Duration::from_secs(2)));
                // What was queued before, then `close`'s end, with how to finish the dry run.
                let events = to_the_end(&s);
                let (fatal, outro) = end_of(&events);
                assert_eq!(fatal, None);
                says_how_to_finish(outro, &name);
                assert_eq!(next(&s), None);
                drop(held);
            }
            "gate" => {
                let (store, name) = stalled(dir);
                let state = dir.join("state");
                let s = Arc::new(Session::start(move || opened(store, state, &[name], false)));
                next(&s);
                // The dry run's start waits on the lock, after the change that says it runs.
                let mut held = lock(&s.shared);
                s.send(Action::Apply { dry_run: true });
                assert!(next_change(&s).1.busy);
                // As `close` does, first: the record is the step before's, with no job.
                assert!(matches!(held.close(), (true, false, _)));
                drop(held);
                // The core thread ends with nothing started: its queue goes with it.
                let stopped = Arc::clone(&s);
                within(move || {
                    while stopped.inputs.send(Msg::Stop).is_ok() {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                });
                // A later `close` goes by what the first read, no job, though review says the dry
                // run whose start the first stopped runs: it ends no group.
                s.close();
                assert!(!read_within(
                    &dir.join("stalled.mkv"),
                    Duration::from_secs(2)
                ));
                assert!(!group::ended());
            }
            "start" => {
                // A run of one file on disk, its LLM `false`, so that no real one is ever asked.
                let file = dir.join("E01.mkv");
                std::fs::write(&file, "1").unwrap();
                let path = file.to_string_lossy().into_owned();
                let mut store = Store::open_at(&dir.join("subcheck.db")).unwrap();
                let header = RunHeader {
                    backend: "cmd:false".into(),
                    ..header()
                };
                let scan = store
                    .begin_scan(&header, std::slice::from_ref(&file))
                    .unwrap();
                let record = FileRecord {
                    path: path.clone(),
                    ..tests_support::file(1)
                };
                let found = Finding {
                    file: path.clone(),
                    ..finding(0, 1, TYPO, FIXED, 0)
                };
                save(&mut store, scan.id, &record, vec![found]);
                let state = dir.join("state");
                let name = scan.name;
                let s = Arc::new(Session::start(move || opened(store, state, &[name], false)));
                next(&s);
                // `c`'s start writes the store on the core thread before any group can start:
                // held, it waits there, its start marked.
                let held = hold_the_store(dir);
                s.send(Action::ShowPane(Pane::Files));
                s.send(Action::SelectFile(path));
                s.send(Action::Recheck);
                s.send(Action::Answer(true));
                let marked = Arc::clone(&s);
                within(move || {
                    while !lock(&marked.shared).job {
                        std::thread::yield_now();
                    }
                });
                let closing = Arc::clone(&s);
                within(move || closing.close());
                assert!(group::ended(), "close saw no job starting");
                drop(held);
            }
            "reopen" => {
                let (s, _) = during_a_job(dir, |_| {});
                s.close();
                let again = Arc::new(Session::start(|| panic!("opened after the groups ended")));
                assert_eq!(
                    next(&again),
                    Some(Event::Ended {
                        fatal: Some(GROUPS_ENDED.into()),
                        outro: vec![],
                    })
                );
                assert_eq!(next(&again), None);
            }
            "close-twice" => {
                let (s, name) = during_a_job(dir, |_| {});
                // The step that started the dry run finished too, so that the record's outro, as of
                // the last step finished, is the one that says how to finish it.
                let noted = Arc::clone(&s);
                within(move || {
                    while lock(&noted.shared).outro.is_empty() {
                        std::thread::yield_now();
                    }
                });
                let let_go = holding_end(dir);
                let mut group = a_group();
                let first = Arc::clone(&s);
                let closing = std::thread::spawn(move || first.close());
                assert!(killed_within(&mut group, LIMIT));
                // A second, on a thread of its own, while the first is still in `group::end`: it
                // waits for the groups the first is ending, as the first does.
                let (started, began) = mpsc::channel();
                let (returned, back) = mpsc::channel();
                let second = Arc::clone(&s);
                let again = std::thread::spawn(move || {
                    let _ = started.send(());
                    second.close();
                    let _ = returned.send(());
                });
                began.recv_timeout(LIMIT).unwrap();
                // Had it returned at once, it would be back well within this.
                assert!(matches!(
                    back.recv_timeout(Duration::from_secs(1)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ));
                let_go.send(()).unwrap();
                back.recv_timeout(LIMIT).unwrap();
                within(move || closing.join().unwrap());
                within(move || again.join().unwrap());
                // What was queued before, then the first's end, with how to finish the dry run:
                // the second wrote nothing.
                let events = to_the_end(&s);
                let (fatal, outro) = end_of(&events);
                assert_eq!(fatal, None);
                says_how_to_finish(outro, &name);
                assert_eq!(next(&s), None);
            }
            "drop-after-close" => {
                let (s, _) = during_a_job(dir, |_| {});
                // A `close` held in `group::end`, as `close-twice`'s first, played in its two
                // parts, as the session must be let go of while it is held: a real `close` holds a
                // reference to it.
                assert!(matches!(lock(&s.shared).close(), (true, true, _)));
                let let_go = holding_end(dir);
                let mut group = a_group();
                let ending = std::thread::spawn(group::end);
                assert!(killed_within(&mut group, LIMIT));
                // Letting go returns while the groups still end.
                let (dropped, gone) = mpsc::channel();
                std::thread::spawn(move || {
                    drop(s);
                    let _ = dropped.send(());
                });
                gone.recv_timeout(LIMIT).unwrap();
                let_go.send(()).unwrap();
                within(move || ending.join().unwrap());
            }
            "drop" => {
                let (s, _) = during_a_job(dir, |_| {});
                let group = a_group();
                drop(s);
                assert_ended(group);
            }
            "quit" => {
                let (s, name) = during_a_job(dir, |_| {});
                let group = a_group();
                s.send(Action::Quit);
                s.send(Action::Answer(true));
                let events = to_the_end(&s);
                let (fatal, outro) = end_of(&events);
                assert_eq!(fatal, None);
                says_how_to_finish(outro, &name);
                assert_ended(group);
            }
            "job-panic" => {
                let (s, name) = during_a_job(dir, |_| {});
                let group = a_group();
                let boom = Input::Fatal("the scan panicked: boom".into());
                s.inputs.send(boom.into()).unwrap();
                let events = to_the_end(&s);
                let (fatal, outro) = end_of(&events);
                assert_eq!(fatal, Some("the scan panicked: boom"));
                says_how_to_finish(outro, &name);
                assert_ended(group);
            }
            "job-panic-idle" => {
                let s = session(dir, |_| {});
                next(&s);
                let group = a_group();
                let boom = Input::Fatal("the scan panicked: boom".into());
                s.inputs.send(boom.into()).unwrap();
                assert_eq!(
                    next(&s),
                    Some(Event::Ended {
                        fatal: Some("the scan panicked: boom".into()),
                        outro: vec![],
                    })
                );
                assert_ended(group);
            }
            "loop-panic" => {
                let (s, name) = during_a_job(dir, |review| review.undo = vec![vec![]]);
                let group = a_group();
                s.send(Action::Undo);
                let events = to_the_end(&s);
                says_how_to_finish(panicked_end(&events), &name);
                assert_ended(group);
            }
            "idle" => {
                let s = session(dir, |_| {});
                next(&s);
                s.send(Action::Quit);
                assert!(matches!(next(&s), Some(Event::Ended { .. })));
                // After `group::end`, this would wait forever.
                within(|| group::output(&mut Command::new("true")).unwrap());
            }
            "open" => {
                let name = std::env::var("SUBCHECK_TEST_RUN").unwrap();
                let s = Arc::new(Session::open(vec![name.clone()], false));
                let (rows, summary) = next_change(&s);
                assert_eq!(rows.unwrap().inserted.len(), 2);
                assert_eq!(summary.title, format!("run {name}"));
            }
            "no-home" => {
                let s = Arc::new(Session::open(vec![], false));
                let events = to_the_end(&s);
                let (fatal, _) = end_of(&events);
                assert_eq!(fatal, Some("HOME is not set to an absolute path"));
            }
            // `open` cannot fail, even with no thread to open on. Read here and not through
            // `within`, which needs a thread too.
            "no-thread" => {
                let none = Rlimit {
                    current: Some(0),
                    maximum: Some(0),
                };
                setrlimit(Resource::Nproc, none).unwrap();
                let s = Session::start(|| anyhow::bail!("never run"));
                let event = s.next_event();
                let Some(Event::Ended {
                    fatal: Some(why), ..
                }) = &event
                else {
                    panic!("{event:?}");
                };
                assert!(why.starts_with("starting review's thread: "), "{why}");
                assert_eq!(s.next_event(), None);
            }
            other => panic!("no such way to end: {other}"),
        }
        // What `in_a_child` checks: a `CHILD` that names no test, or an arm that returns early,
        // would pass with nothing run.
        std::fs::write(dir.join("ran"), "").unwrap();
    }

    /// Run [`child_that_ends_a_session`] the `how` way, with `set` done to its command; it must
    /// pass, and say it ran. A deadline, so that one that hangs fails here.
    fn in_a_child(how: &str, dir: &Path, set: impl FnOnce(&mut Command)) {
        let log = dir.join("child.log");
        let out = std::fs::File::create(&log).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", CHILD, "--ignored", "--nocapture"])
            .env("SUBCHECK_TEST_HOW", how)
            .env("SUBCHECK_TEST_DIR", dir)
            .stdout(Stdio::from(out.try_clone().unwrap()))
            .stderr(Stdio::from(out));
        set(&mut command);
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + 2 * LIMIT;
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
        assert!(dir.join("ran").exists(), "the child ran no arm: {said}");
    }

    /// `in_a_child` in a new temporary directory.
    fn ended(how: &str) {
        let dir = tempfile::tempdir().unwrap();
        in_a_child(how, dir.path(), |_| {});
    }

    #[test]
    fn close_during_a_job_ends_its_groups_before_it_returns_and_says_how_to_finish_it() {
        ended("close");
    }

    #[test]
    fn a_job_that_would_start_after_close_does_not() {
        ended("gate");
    }

    #[test]
    fn close_during_a_jobs_start_ends_its_groups() {
        ended("start");
    }

    #[test]
    fn a_session_opened_after_the_groups_ended_ends_at_once_and_says_why() {
        ended("reopen");
    }

    #[test]
    fn a_second_close_while_the_first_ends_the_groups_leaves_the_stream_to_the_first() {
        ended("close-twice");
    }

    #[test]
    fn a_drop_after_a_close_returns_at_once_while_the_groups_still_end() {
        ended("drop-after-close");
    }

    #[test]
    fn a_session_let_go_of_during_a_job_ends_its_groups() {
        ended("drop");
    }

    #[test]
    fn quitting_during_a_job_ends_its_groups_and_says_how_to_finish_it() {
        ended("quit");
    }

    #[test]
    fn a_jobs_panic_ends_the_session_and_its_groups_and_says_how_to_finish_a_job_it_stopped() {
        ended("job-panic");
    }

    #[test]
    fn a_fatal_input_with_nothing_running_still_ends_the_groups_and_has_no_outro() {
        ended("job-panic-idle");
    }

    #[test]
    fn a_panic_in_the_loop_during_a_job_ends_its_groups() {
        ended("loop-panic");
    }

    #[test]
    fn a_clean_quit_leaves_the_groups_alone() {
        ended("idle");
    }

    #[test]
    fn opening_takes_the_store_the_environment_names() {
        let dir = tempfile::tempdir().unwrap();
        let (_, name) = seeded(dir.path());
        in_a_child("open", dir.path(), |command| {
            command
                .env("SUBCHECK_DB", dir.path().join("subcheck.db"))
                .env("XDG_STATE_HOME", dir.path().join("state"))
                .env("SUBCHECK_TEST_RUN", &name);
        });
    }

    #[test]
    fn a_thread_that_cannot_start_is_a_fatal_end() {
        // Threads count towards RLIMIT_NPROC on Linux alone, and root is not held to it.
        if !cfg!(target_os = "linux") || rustix::process::getuid().is_root() {
            eprintln!("skipped: threads can still start here");
            return;
        }
        ended("no-thread");
    }

    #[test]
    fn opening_with_nowhere_to_keep_state_is_a_fatal_end() {
        let dir = tempfile::tempdir().unwrap();
        in_a_child("no-home", dir.path(), |command| {
            command
                .env_remove("HOME")
                .env_remove("SUBCHECK_DB")
                .env_remove("XDG_STATE_HOME");
        });
    }
}
