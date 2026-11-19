//! Review on a thread of its own, for a UI whose own loop must not wait on the store or a job:
//! [`Session`] takes the UI's actions on a queue and runs `update` and the [`Runner`] behind it,
//! and the UI hears what happened as one ordered stream of events.
//!
//! The stream is the only way state reaches a UI. A direct read would wait on the core thread, or
//! read a copy newer than the events still queued, and a splice applied to a newer list than the
//! one it was cut from corrupts the rows. The rows are compared, not reported: `update` would have
//! to name every row it touches, and one it missed would leave the UI silently out of step.

use std::collections::{HashSet, VecDeque};
use std::ops::Range;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use jiff::Timestamp;

use super::detail::Detail;
use super::jobs::panic_text;
use super::model::{Entry, EntryKey, FileNote, Fix, ListTrack, MarkKind, Pane, Question, Review};
use super::progress::{ApplyStage, FileProgress, Job, Stage};
use super::runner::{self, Runner};
use super::status::Status;
use super::update::{self, Action, Effect, Input, Task};
use crate::group;
use crate::list::Filter;
use crate::llm::Spent;
use crate::player::{self, PlayState, PlayerCommand, PlayerError};
use crate::store::{self, Check, Run, Selector, ShowFolder, State, Store};

/// What a UI hears, in order ([`Session::next_event`]).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // each is made once and moved to the UI: a box adds only noise
#[allow(clippy::option_option)] // `detail`, `logs`: absent when as last sent; none is a change too
pub enum Event {
    /// What the UI shows changed. `rows`, `files`, `detail`, `logs` and `folders` are `None` when
    /// they did not, but never in the first event, which carries each.
    Changed {
        rows: Option<Splice>,
        summary: Summary,
        /// The selected finding's detail ([`Review::detail`]); none when nothing is selected.
        detail: Option<Option<Detail>>,
        /// The files list's change, as `rows` is the findings list's.
        files: Option<Splice<FileView>>,
        /// What the scan and apply wrote for the selected file; none when the files list is empty.
        logs: Option<Option<FileLogs>>,
        /// The store's show folders, whole: a short list, sent again when any of it changes.
        folders: Option<Vec<FolderView>>,
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

/// A change to a list the UI holds, the rows unless it says so: the `removed` items at `at` give
/// way to `inserted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Splice<T = EntryView> {
    pub at: usize,
    pub removed: usize,
    pub inserted: Vec<T>,
}

// By hand: a derived default would ask the same of the items.
impl<T> Default for Splice<T> {
    fn default() -> Splice<T> {
        Splice {
            at: 0,
            removed: 0,
            inserted: Vec::new(),
        }
    }
}

/// What a UI shows beside the list. Sent whole, with any change to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// What the view is called ([`crate::store::Run::shown_name`]).
    pub title: String,
    /// An index into the rows the UI holds; `None` when it holds none.
    pub selected: Option<usize>,
    /// Review's message ([`Review::status`]), which a UI words, as it does the question and the
    /// job, fields of their own.
    pub status: Option<Status>,
    /// Findings pending, accepted and rejected ([`Review::counts`]): the whole view's, whatever
    /// the filter shows.
    pub pending: usize,
    pub accepted: usize,
    pub rejected: usize,
    /// Whether a job runs ([`crate::review::progress::Progress::busy`]).
    pub busy: bool,
    /// The findings the list shows.
    pub filter: Filter,
    /// Which list the UI shows.
    pub pane: Pane,
    /// The edit under way: none once it ends, or when what it edits has gone.
    pub edit: Option<Editing>,
    /// The open question ([`Review::question`]).
    pub question: Option<Question>,
    /// The job under way, or the last to end ([`Job`]), its files by the list's names for them.
    pub job: Option<Job>,
    /// An index into the files list the UI holds; `None` when it holds none.
    pub file_selected: Option<usize>,
    /// What the view's LLM calls used ([`Run::spent`]), which the terminal's box of keys shows.
    pub spent: Spent,
    /// The view is of paths (a folder's, the newest run's folders, files), not a run's
    /// ([`Run::of_paths`]): what `SwitchView` opens from it, the newest run that named the
    /// selected file where a run's view opens the run's folders.
    pub of_paths: bool,
}

/// An edit under way, for a UI's editor ([`Review::editing`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editing {
    /// The entry it was opened on, which `Reword` decides.
    pub entry: EntryKey,
    /// The wording to start from ([`Review::edit_wording`]).
    pub wording: String,
}

/// One line of the list, as a UI shows it: a row, or a member of an expanded row.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // each says a thing of its own that a UI shows
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
    /// What apply did or would do with the findings it shows ([`Review::mark_counts`]): each kind
    /// with how many have it; none when none has a mark.
    pub marks: Vec<(MarkKind, usize)>,
    /// A timing finding's track, as the list tells it apart; none for others.
    pub track: Option<ListTrack>,
    /// A timing finding's fixes, of the findings it shows ([`Review::fixes`]): each with how many
    /// have it, the most common first; none for others.
    pub fixes: Vec<(Fix, usize)>,
    /// A scan is checking its file again ([`Review::checking_among`]): any shown member's, for
    /// a row.
    pub checking: bool,
    /// How its file differs from when it was scanned: of a row's first shown member, as `label`.
    pub note: Option<FileNote>,
    /// Its shown members' decisions differ ([`Review::shown_state`]): never a member's.
    pub mixed: bool,
    /// The check that found it ([`store::Finding::check`]): of a row's first shown member, as
    /// `original` is.
    pub check: Check,
    /// The part of `original` that differs from `wording`, in characters ([`Review::changes`]),
    /// as the detail has it.
    pub original_changed: Range<usize>,
    /// The part of `wording` that differs from `original`, in characters ([`Review::changes`]),
    /// as the detail has it.
    pub wording_changed: Range<usize>,
}

/// The list as `m` shows it now.
fn entry_views(m: &Review) -> Vec<EntryView> {
    let checked = m.progress.checked();
    m.entries()
        .into_iter()
        .map(|e| entry_view(m, e, &checked))
        .collect()
}

/// Entry `e` as a UI shows it, given the files a scan is checking (`Progress::checked`).
fn entry_view(m: &Review, e: Entry, checked: &HashSet<&str>) -> EntryView {
    let shown = m.targets(e);
    let first = shown[0];
    let f = m.finding(first);
    let (original_changed, wording_changed) = m.changes(first);
    let (is_member, expanded, total, mixed) = match e {
        // As `entries` has it: an expanded row that a re-check left with one member shows none.
        Entry::Row(r) => {
            let total = m.rows[r].members.len();
            let expanded = total > 1 && m.expanded.contains(&r);
            (false, expanded, total, m.shown_state(r).is_none())
        }
        Entry::Member(..) => (true, false, 1, false),
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
        marks: m.mark_counts(e),
        track: m.list_track(first).cloned(),
        fixes: m.fixes(&shown),
        checking: m.checking_among(e, checked),
        note: m.notes.get(&f.file).copied(),
        mixed,
        check: f.check(),
        original_changed,
        wording_changed,
    }
}

/// One line of the files list, as a UI shows it: a file of the run, and where the scan and apply
/// are with it ([`FileProgress`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileView {
    /// The file as the store names it, which `SelectFile` takes.
    pub path: String,
    /// The name it goes by in the list.
    pub label: String,
    /// Where the scan is with it; for a file not scanned since the view opened, as recorded.
    pub stage: Stage,
    /// Where the latest apply or dry run is with it, which a UI shows in place of `stage`: none
    /// until that apply queues it, and once a later scan checks it again.
    pub applying: Option<ApplyStage>,
    /// Distinct lines checked, from its record.
    pub lines: Option<usize>,
    /// Checked for timing alone, with no lines checked by the LLM to count.
    pub timing_only: bool,
}

impl FileView {
    fn of(m: &Review, file: &FileProgress) -> FileView {
        FileView {
            path: file.path.clone(),
            label: m.label(&file.path),
            stage: file.stage.clone(),
            applying: file.applying.clone(),
            lines: file.lines,
            timing_only: file.timing_only,
        }
    }
}

/// What the scan and the screen's applies wrote for the selected file, for a UI to show beside the
/// files list ([`FileProgress::log`], [`FileProgress::apply_log`]): lines of text, as the jobs
/// wrote them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLogs {
    pub log: Vec<String>,
    pub apply_log: Vec<String>,
}

/// One of the store's show folders, as a UI lists it to open ([`Store::show_folders`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderView {
    /// The folder as the store names its files' paths, which `OpenFolder` takes.
    pub path: String,
    /// What its view is called once open ([`store::path_name`]).
    pub name: String,
    /// How many of the findings recorded under it are pending.
    pub to_review: usize,
    /// The view shown is of paths, one of which is this folder or a path inside it: a folder's
    /// view, the newest run's folders, or a view of files in it. A run's view is no folder's.
    pub open: bool,
}

impl FolderView {
    fn of(folder: &ShowFolder, view: &Run) -> FolderView {
        FolderView {
            path: folder.path.to_string_lossy().into_owned(),
            name: store::path_name(&folder.path),
            to_review: folder.to_review,
            open: view.of_paths && view.paths.iter().any(|p| p.starts_with(&folder.path)),
        }
    }
}

/// The files list as `m` shows it now.
fn file_views(m: &Review) -> Vec<FileView> {
    m.progress
        .files
        .iter()
        .map(|file| FileView::of(m, file))
        .collect()
}

/// The selected file's logs; none when the files list is empty.
fn file_logs(m: &Review) -> Option<FileLogs> {
    let file = m.progress.files.get(m.file_selected)?;
    Some(FileLogs {
        log: file.log.clone(),
        apply_log: file.apply_log.clone(),
    })
}

/// What `m` shows beside a list of `rows` rows and `files` files.
fn summary(m: &Review, rows: usize, files: usize) -> Summary {
    let (pending, accepted, rejected) = m.counts();
    Summary {
        title: m.run.shown_name(),
        selected: (m.selected < rows).then_some(m.selected),
        status: m.status.clone(),
        pending,
        accepted,
        rejected,
        busy: m.progress.busy(),
        filter: m.filter,
        pane: m.pane,
        edit: m.editing.as_ref().and_then(|edit| {
            Some(Editing {
                entry: edit.entry.clone(),
                wording: m.edit_wording()?.to_string(),
            })
        }),
        question: m.question(),
        job: m.progress.job(&|p| m.label(p)),
        file_selected: (m.file_selected < files).then_some(m.file_selected),
        spent: m.run.spent.clone(),
        of_paths: m.run.of_paths,
    }
}

/// `old` into `new` as one splice, the items between their matching start and their matching end;
/// `None` when they are the same. Two far-apart changes resend the items between them: a UI applies
/// any splice the same way, and that costs less than a second kind of event would.
fn splice<T: PartialEq + Clone>(old: &[T], new: &[T]) -> Option<Splice<T>> {
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

/// What the UI has been told, to work out what to tell it next; and the show folders as the store
/// last gave them, which each change says with the view's own marked open.
#[derive(Default)]
struct Sent {
    rows: Vec<EntryView>,
    files: Vec<FileView>,
    /// `None` until the first event.
    summary: Option<Summary>,
    detail: Option<Detail>,
    logs: Option<FileLogs>,
    folders: Vec<FolderView>,
    shelf: Vec<ShowFolder>,
}

impl Sent {
    /// The event that brings the UI up to `m`; `None` when nothing it shows changed. The first
    /// always comes, with a splice of each list even for none, and the detail, the logs and the
    /// folders even when there are none: it tells the UI the run has opened, and what it starts at.
    fn change(&mut self, m: &Review) -> Option<Event> {
        let rows = entry_views(m);
        let files = file_views(m);
        let summary = summary(m, rows.len(), files.len());
        let detail = m.detail();
        let logs = file_logs(m);
        let folders: Vec<FolderView> = self
            .shelf
            .iter()
            .map(|f| FolderView::of(f, &m.run))
            .collect();
        let first = self.summary.is_none();
        let row_splice = splice(&self.rows, &rows).or_else(|| first.then(Splice::default));
        let file_splice = splice(&self.files, &files).or_else(|| first.then(Splice::default));
        let detail_now = (first || detail != self.detail).then(|| detail.clone());
        let logs_now = (first || logs != self.logs).then(|| logs.clone());
        let folders_now = (first || folders != self.folders).then(|| folders.clone());
        if row_splice.is_none()
            && file_splice.is_none()
            && detail_now.is_none()
            && logs_now.is_none()
            && folders_now.is_none()
            && self.summary.as_ref() == Some(&summary)
        {
            return None;
        }
        self.rows = rows;
        self.files = files;
        self.summary = Some(summary.clone());
        self.detail = detail;
        self.logs = logs;
        self.folders = folders;
        Some(Event::Changed {
            rows: row_splice,
            summary,
            detail: detail_now,
            files: file_splice,
            logs: logs_now,
            folders: folders_now,
        })
    }

    /// The show folders as the store gives them now. A read that fails leaves the list as it
    /// was, empty if no read has succeeded yet, and says nothing: the list only offers views to
    /// open, and review goes on without it.
    fn read(&mut self, folders: rusqlite::Result<Vec<ShowFolder>>) {
        if let Ok(folders) = folders {
            self.shelf = folders;
        }
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
#[allow(clippy::large_enum_variant)] // as `Event`, which it carries
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
    sent.read(runner.show_folders());
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

/// Whether the show folders are read again after `input`: a decision written, whose finding a
/// folder's count holds; a check or an apply ended, or another view taken, after which what
/// another process recorded since shows too.
fn moves_folders(input: &Input) -> bool {
    matches!(
        input,
        Input::Written { result: Ok(_), .. }
            | Input::ScanEnded(_)
            | Input::ApplyEnded(_)
            | Input::Switched(Ok(_))
    )
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
    let moved = moves_folders(&input);
    let mut effects: VecDeque<Effect> = update::update(review, input, Timestamp::now()).into();
    if moved {
        sent.read(runner.show_folders());
    }
    sent.publish(review, emit);
    while let Some(effect) = effects.pop_front() {
        if !may_run(&effect, shared) {
            return;
        }
        match effect {
            Effect::Task(task) => {
                let result = runner.run(task);
                let moved = moves_folders(&result);
                effects.extend(update::update(review, result, Timestamp::now()));
                if moved {
                    sent.read(runner.show_folders());
                }
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

    use crate::apply;
    use crate::list::Filter;
    use crate::player::{How, Moment, Playback};
    use crate::review::model::tests_support::{self, finding, many, sample, with_timing};
    use crate::review::model::{Mark, Row};
    use crate::review::progress::{ApplyAt, Counts, Scan};
    use crate::review::status::{Asked, Failure, Refusal};
    use crate::review::update::tests_support::{FIXED, TYPO, decide_as, now, saved, taken};
    use crate::scan;
    use crate::store::{
        Attempt, CheckedWith, Decision, FileRecord, Finding, Made, Record, Run, RunHeader, ScanRow,
    };

    fn act(m: &mut Review, action: Action) -> Vec<Effect> {
        update::update(m, Input::Action(action), now())
    }

    /// The message once `findings` findings are accepted, none of them reworded, none kept.
    fn accepted(findings: usize) -> Status {
        Status::Decided {
            state: State::Accepted,
            findings,
            replaced: 0,
            kept: None,
        }
    }

    /// The message as `Play` asks to open E01's first finding.
    fn opening_e01() -> Status {
        Status::Opening {
            label: "E01".into(),
            at_ms: 165_060,
        }
    }

    /// `sent` up to `m`, what it says of the rows, and the summary; an event must be due.
    fn changed(sent: &mut Sent, m: &Review) -> (Option<Splice>, Summary) {
        match sent.change(m) {
            Some(Event::Changed { rows, summary, .. }) => (rows, summary),
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
                filter: Filter::All,
                pane: Pane::Findings,
                edit: None,
                question: None,
                job: None,
                file_selected: Some(0),
                spent: Spent::default(),
                of_paths: false,
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
    fn the_first_event_carries_the_files_the_detail_and_the_selected_files_logs() {
        let m = sample();
        let Some(Event::Changed {
            summary,
            detail,
            files,
            logs,
            ..
        }) = Sent::default().change(&m)
        else {
            panic!("no change");
        };
        let files = files.unwrap();
        assert_eq!((files.at, files.removed), (0, 0));
        let labels: Vec<&str> = files.inserted.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, ["E01", "E02", "E03"]);
        assert_eq!(
            files.inserted[1],
            FileView {
                path: tests_support::path(2),
                label: "E02".into(),
                stage: Stage::Done { findings: 2 },
                applying: None,
                lines: Some(400),
                timing_only: false,
            }
        );
        assert_eq!(summary.file_selected, Some(0));
        assert_eq!(detail, Some(m.detail()));
        assert_eq!(detail.flatten().unwrap().label, "E01");
        assert_eq!(
            logs,
            Some(Some(FileLogs {
                log: vec![
                    "400 lines, 1 finding".into(),
                    "    track 5 English Full: cues".into(),
                    "    track 6 English Signs: cues".into(),
                ],
                apply_log: vec![],
            }))
        );
        // A run with no files: each part says so, and nothing is selected in either list.
        let run = Run {
            name: "r".into(),
            ..Run::default()
        };
        let empty = Review::new(run, HashMap::new());
        let Some(Event::Changed {
            summary,
            detail,
            files,
            logs,
            ..
        }) = Sent::default().change(&empty)
        else {
            panic!("no change");
        };
        assert_eq!(files, Some(Splice::default()));
        assert_eq!((detail, logs), (Some(None), Some(None)));
        assert_eq!(summary.file_selected, None);
    }

    /// Two show folders as the store would give them: S1, with three to review, and S2.
    fn shelf() -> Vec<ShowFolder> {
        vec![
            ShowFolder {
                path: "/m/S1".into(),
                to_review: 3,
            },
            ShowFolder {
                path: "/m/S2".into(),
                to_review: 0,
            },
        ]
    }

    // The folders, whole, in the first event even when there are none, and again only when what
    // the store gives or which the view shows changes them.
    #[test]
    fn the_folders_come_whole_first_and_again_when_they_change() {
        let mut m = sample();
        let mut sent = Sent::default();
        let folders = |sent: &mut Sent, m: &Review| match sent.change(m) {
            Some(Event::Changed { folders, .. }) => folders,
            other => panic!("expected a change: {other:?}"),
        };
        assert_eq!(folders(&mut sent, &m), Some(vec![]));
        sent.read(Ok(shelf()));
        let s1 = |to_review, open| FolderView {
            path: "/m/S1".into(),
            name: "S1".into(),
            to_review,
            open,
        };
        let s2 = FolderView {
            path: "/m/S2".into(),
            name: "S2".into(),
            to_review: 0,
            open: false,
        };
        assert_eq!(folders(&mut sent, &m), Some(vec![s1(3, false), s2.clone()]));
        // The store read again: unchanged is no event; a read that fails keeps them.
        sent.read(Ok(shelf()));
        assert_eq!(sent.change(&m), None);
        sent.read(Err(rusqlite::Error::InvalidQuery));
        assert_eq!(sent.change(&m), None);
        // S1's view: S1 is open.
        m.run.of_paths = true;
        m.run.paths = vec!["/m/S1".into()];
        assert_eq!(folders(&mut sent, &m), Some(vec![s1(3, true), s2.clone()]));
        let mut fewer = shelf();
        fewer[0].to_review = 2;
        sent.read(Ok(fewer));
        assert_eq!(folders(&mut sent, &m), Some(vec![s1(2, true), s2]));
    }

    // Open when the view is of paths, one of them the folder or a path inside it: a folder's own
    // view, the newest run's folders, or a view of a file in it; a run's view is no folder's. A
    // folder whose name starts as this one's, or the folder above it, is another.
    #[test]
    fn a_folder_is_open_when_the_view_is_of_it_or_of_a_path_inside_it() {
        let s1 = &shelf()[0];
        let view = |of_paths: bool, paths: &[&str]| Run {
            of_paths,
            paths: paths.iter().map(PathBuf::from).collect(),
            ..Run::default()
        };
        let open = |view: &Run| FolderView::of(s1, view).open;
        assert!(open(&view(true, &["/m/S1"])));
        assert!(open(&view(true, &["/m/S2", "/m/S1/Specials"])));
        assert!(open(&view(true, &["/m/S1/E01.mkv"])));
        assert!(!open(&view(true, &["/m/S1 Season 2", "/m"])));
        assert!(!open(&view(false, &[])));
        assert!(!open(&view(false, &["/m/S1"])));
        assert_eq!(
            FolderView::of(s1, &view(false, &[])),
            FolderView {
                path: "/m/S1".into(),
                name: "S1".into(),
                to_review: 3,
                open: false,
            }
        );
    }

    // A paths view says so, and a run's view does not: what a UI's switch between the two is
    // called depends on it.
    #[test]
    fn the_summary_says_whether_the_view_is_of_paths() {
        let mut m = sample();
        let mut sent = Sent::default();
        assert!(!changed(&mut sent, &m).1.of_paths);
        m.run.of_paths = true;
        assert!(changed(&mut sent, &m).1.of_paths);
    }

    // Each input after which the folders are read again, and some after which they are not: a
    // write that failed, a view that could not be read, a file read back, a scan's step and an
    // action of the user's.
    #[test]
    fn the_folders_are_read_again_after_a_decision_a_jobs_end_or_another_view() {
        let written = |result| Input::Written {
            purpose: update::WriteFor::Undo,
            decisions: vec![],
            result,
        };
        let cases = [
            (written(Ok(vec![])), true),
            (written(Err("locked".into())), false),
            (Input::ScanEnded(Err("no LLM".into())), true),
            (Input::ApplyEnded(Ok(apply::Summary::default())), true),
            (Input::Switched(Ok((Run::default(), HashMap::new()))), true),
            (Input::Switched(Err("disk I/O error".into())), false),
            (
                Input::ReadBack {
                    path: tests_support::path(1),
                    view: Err("gone".into()),
                },
                false,
            ),
            (Input::Scan(scan::Event::Queued(vec![])), false),
            (Input::Action(Action::Dismiss), false),
        ];
        for (input, moves) in cases {
            assert_eq!(moves_folders(&input), moves, "{input:?}");
        }
    }

    #[test]
    fn the_summary_carries_what_the_views_llm_calls_used() {
        let mut m = sample();
        let mut sent = told(&m);
        m.run.spent.add(Some(&crate::llm::Usage {
            input_tokens: 1200,
            output_tokens: 40,
            cost_usd: Some(0.25),
            ..crate::llm::Usage::default()
        }));
        let (_, summary) = changed(&mut sent, &m);
        assert_eq!(summary.spent, m.run.spent);
        assert_eq!((summary.spent.calls, summary.spent.priced), (1, 1));
    }

    #[test]
    fn the_detail_is_sent_when_it_changes_and_none_once_nothing_is_selected() {
        let mut m = sample();
        let mut sent = told(&m);
        // The files list shown: the detail is still the findings list's selection's, unchanged.
        act(&mut m, Action::ShowPane(Pane::Files));
        let Some(Event::Changed {
            summary, detail, ..
        }) = sent.change(&m)
        else {
            panic!("no change");
        };
        assert_eq!((summary.pane, detail), (Pane::Files, None));
        act(&mut m, Action::ShowPane(Pane::Findings));
        changed(&mut sent, &m);
        act(&mut m, Action::Move(1));
        let Some(Event::Changed { detail, .. }) = sent.change(&m) else {
            panic!("no change");
        };
        assert_eq!(
            detail.flatten().unwrap().original,
            "Let's have snacks together everyday."
        );
        // A filter that shows nothing: none, which is a change too.
        act(&mut m, Action::SetFilter(Filter::Only(State::Accepted)));
        let Some(Event::Changed { detail, .. }) = sent.change(&m) else {
            panic!("no change");
        };
        assert_eq!(detail, Some(None));
        assert_eq!(sent.change(&m), None, "nothing since");
    }

    #[test]
    fn the_files_and_the_selected_files_logs_are_sent_when_they_change() {
        let mut m = sample();
        let mut sent = told(&m);
        // Another file selected: its logs, and the list itself left out.
        act(&mut m, Action::SelectFile(tests_support::path(2)));
        let Some(Event::Changed {
            summary,
            files,
            logs,
            ..
        }) = sent.change(&m)
        else {
            panic!("no change");
        };
        assert_eq!((summary.file_selected, files), (Some(1), None));
        assert_eq!(logs.flatten().unwrap().log[0], "400 lines, 2 findings");
        // A scan of E02: its line, and its logs, as it goes.
        m.progress.start("r", "subcheck scan --run r".into(), true);
        let queued = scan::Event::Queued(vec![tests_support::path(2).into()]);
        update::update(&mut m, Input::Scan(queued), now());
        let Some(Event::Changed { files, logs, .. }) = sent.change(&m) else {
            panic!("no change");
        };
        let files = files.unwrap();
        assert_eq!((files.at, files.removed, files.inserted.len()), (1, 1, 1));
        assert_eq!(files.inserted[0].stage, Stage::Queued);
        assert_eq!(logs.flatten().unwrap().log, Vec::<String>::new());
        let done = scan::Event::Finished {
            path: tests_support::path(2).into(),
            error: None,
            findings: 0,
            lines: vec!["E02: no findings".into()],
        };
        update::update(&mut m, Input::Scan(done), now());
        let Some(Event::Changed { files, logs, .. }) = sent.change(&m) else {
            panic!("no change");
        };
        assert_eq!(
            files.unwrap().inserted[0].stage,
            Stage::Done { findings: 0 }
        );
        assert_eq!(logs.flatten().unwrap().log, ["E02: no findings"]);
        // A decision touches neither.
        let effects = act(&mut m, Action::Accept);
        update::update(&mut m, taken(effects), now());
        let Some(Event::Changed { files, logs, .. }) = sent.change(&m) else {
            panic!("no change");
        };
        assert_eq!((files, logs), (None, None));
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
        assert_eq!(summary.status, Some(accepted(3)));
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
        assert_eq!((lyric.marks.as_slice(), &lyric.track), (&[][..], &None));
        assert_eq!(lyric.fixes, []);
        // `trough` and `through`, in characters, as the detail marks them.
        assert_eq!(lyric.check, Check::Llm);
        assert_eq!(
            (
                lyric.original_changed.clone(),
                lyric.wording_changed.clone()
            ),
            (14..20, 14..21)
        );
        let detail = m.detail().unwrap();
        assert_eq!(
            (detail.original_changed, detail.wording_changed),
            (14..20, 14..21)
        );
        // Decided differently, and a filter leaving two of three: the rule is the list's.
        decide_as(&mut m, &[1], State::Accepted, Some("My own."));
        decide_as(&mut m, &[2], State::Rejected, None);
        decide_as(&mut m, &[4], State::Rejected, None);
        m.run.applied.insert(2);
        let lyric = &entry_views(&m)[0];
        assert_eq!(lyric.state, None, "decided members differ");
        assert_eq!(lyric.wording, "My own.");
        assert_eq!(lyric.marks, [(MarkKind::Applied, 1)]);
        // What changed is the user's wording against the line: all of both.
        assert_eq!(
            (
                lyric.original_changed.clone(),
                lyric.wording_changed.clone()
            ),
            (0..TYPO.chars().count(), 0..7)
        );
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
        assert_eq!(e02.marks, [(MarkKind::Applied, 1)]);
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
        let english = ListTrack {
            id: 5,
            name: Some("English Full".into()),
            language: Some("eng".into()),
            apart: true,
        };
        assert_eq!(row.track.as_ref(), Some(&english));
        assert_eq!(row.fixes, [(Fix::Shift(1), 2), (Fix::Partial, 1)]);
        // A timing finding's two sides are descriptions: all of each is marked.
        assert_eq!(row.check, Check::Timing);
        assert_eq!(
            (row.original_changed.clone(), row.wording_changed.clone()),
            (
                0..row.original.chars().count(),
                0..row.wording.chars().count()
            )
        );
        m.expanded.insert(1);
        let partial = entry_views(&m)[3].clone();
        assert_eq!(partial.key, EntryKey::Member(12));
        assert_eq!(partial.track, Some(english));
        assert_eq!(partial.fixes, [(Fix::Partial, 1)]);
    }

    // A row's marks as data: by kind, so that two findings left by hand for different reasons
    // count as one, with members that have none left out; a member's own, counting itself.
    #[test]
    fn a_rows_marks_count_each_kind_its_shown_members_have() {
        let mut m = sample();
        m.marks
            .insert(1, Mark::ByHand("two fixes on one line".into()));
        m.marks
            .insert(2, Mark::ByHand("the fix has markup characters".into()));
        let views = entry_views(&m);
        assert_eq!(views[0].marks, [(MarkKind::ByHand, 2)]);
        m.run.applied.insert(4);
        m.expanded.insert(0);
        let views = entry_views(&m);
        assert_eq!(
            views[0].marks,
            [(MarkKind::ByHand, 2), (MarkKind::Applied, 1)]
        );
        assert_eq!(views[3].marks, [(MarkKind::Applied, 1)]);
        // The filter's members alone.
        decide_as(&mut m, &[4], State::Accepted, None);
        m.filter = Filter::Only(State::Accepted);
        assert_eq!(entry_views(&m)[0].marks, [(MarkKind::Applied, 1)]);
    }

    #[test]
    fn a_row_left_with_one_member_is_not_expanded_though_it_is_marked_so() {
        let mut m = many(1);
        m.expanded.insert(0);
        let views = entry_views(&m);
        assert_eq!(views.len(), 1, "as `entries` has it");
        assert!(!views[0].expanded);
    }

    #[test]
    fn a_views_file_being_checked_again_its_note_and_its_members_differing_are_data() {
        let mut m = sample();
        m.expanded.insert(0);
        m.notes.insert(tests_support::path(1), FileNote::Missing);
        m.notes.insert(tests_support::path(3), FileNote::Changed);
        decide_as(&mut m, &[1], State::Accepted, None);
        // E02 read again: the lyric's row, which shows it, its member E02, and E02's snacks row.
        m.progress
            .scan_event(&scan::Event::Queued(vec![tests_support::path(2).into()]));
        m.progress
            .scan_event(&scan::Event::Reading(tests_support::path(2).into()));
        let views = entry_views(&m);
        let what = |v: &EntryView| (v.label.clone(), v.checking, v.note, v.mixed);
        assert_eq!(
            views.iter().map(what).collect::<Vec<_>>(),
            [
                ("E01".into(), true, Some(FileNote::Missing), true),
                ("E01".into(), false, Some(FileNote::Missing), false),
                ("E02".into(), true, None, false),
                ("E03".into(), false, Some(FileNote::Changed), false),
                ("E02".into(), true, None, false),
            ]
        );
        // Still to do, as a member is pending, though its decided member differs.
        assert_eq!(views[0].state, Some(State::Pending));
        // Shown alone, E01 is the row: not being checked, and one decision.
        m.filter = Filter::Only(State::Accepted);
        let row = &entry_views(&m)[0];
        assert_eq!((row.checking, row.mixed), (false, false));
    }

    #[test]
    fn the_summary_says_the_filter_the_pane_the_edit_and_the_question_as_each_changes() {
        let mut m = sample();
        let mut sent = told(&m);
        act(&mut m, Action::SetFilter(Filter::Only(State::Pending)));
        assert_eq!(
            changed(&mut sent, &m).1.filter,
            Filter::Only(State::Pending)
        );
        act(&mut m, Action::SetFilter(Filter::All));
        changed(&mut sent, &m);
        act(&mut m, Action::ShowPane(Pane::Files));
        assert_eq!(changed(&mut sent, &m).1.pane, Pane::Files);
        act(&mut m, Action::ShowPane(Pane::Findings));
        changed(&mut sent, &m);
        // An edit starts from the wording its findings have: the user's, here.
        decide_as(&mut m, &[1, 2, 4], State::Accepted, Some("My own."));
        changed(&mut sent, &m);
        act(&mut m, Action::Edit);
        assert_eq!(
            changed(&mut sent, &m).1.edit,
            Some(Editing {
                entry: EntryKey::Row(vec![1, 2, 4]),
                wording: "My own.".into(),
            })
        );
        act(&mut m, Action::CancelEdit);
        assert_eq!(changed(&mut sent, &m).1.edit, None);
        act(&mut m, Action::Apply { dry_run: false });
        assert_eq!(
            changed(&mut sent, &m).1.question,
            Some(Question::Apply {
                fixes: 3,
                files: 3,
                applied: 0,
            })
        );
        act(&mut m, Action::Answer(false));
        assert_eq!(changed(&mut sent, &m).1.question, None);
        assert_eq!(sent.change(&m), None, "nothing since");
    }

    // The job crosses as data, for each UI to word: the summary leaves it out of the message, as it
    // runs and once it has ended.
    #[test]
    fn the_summary_carries_the_job_and_its_status_is_reviews_message_alone() {
        let mut m = sample();
        let mut sent = told(&m);
        decide_as(&mut m, &[3], State::Accepted, None);
        changed(&mut sent, &m);
        act(&mut m, Action::Apply { dry_run: true });
        let summary = changed(&mut sent, &m).1;
        assert_eq!(m.progress.line().as_deref(), Some("dry run 0/0"));
        assert_eq!(summary.status, None);
        assert_eq!(
            summary.job,
            Some(Job::Applying {
                dry_run: true,
                done: 0,
                queued: 0,
                at: ApplyAt::Starting,
                so_far: apply::Summary::default(),
            })
        );
        update::update(&mut m, Input::ApplyEnded(Err("no threads".into())), now());
        assert_eq!(
            changed(&mut sent, &m).1.job,
            Some(Job::Applied {
                dry_run: true,
                ended: Err("no threads".into()),
            })
        );
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
            Some(Event::Changed { rows, summary, .. }) => (rows, summary),
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
        // E01 on disk, under its own name, so that the stat `Play` makes finds it.
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
        assert_eq!(summary.status, Some(opening_e01()));
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
        assert_eq!(summary.status, Some(Status::Opened { paused: true }));
        // A task whose result changes the rows: the status line cleared first, on its own, while
        // the write is still to run; then the rows, once the store has said.
        s.send(Action::Accept);
        let (rows, summary) = next_change(&s);
        assert_eq!(rows, None);
        assert_eq!(summary.status, None);
        assert_eq!(summary.accepted, 0);
        let (rows, summary) = next_change(&s);
        assert_eq!(rows.unwrap().inserted[0].state, Some(State::Accepted));
        assert_eq!(summary.status, Some(accepted(3)));
        assert_eq!(summary.accepted, 3);
        // The other two commands, and what their reports say.
        s.send(Action::Playback(Playback::Pause));
        assert_eq!(
            next(&s),
            Some(Event::Player(PlayerCommand::Playback(Playback::Pause)))
        );
        s.report(PlayerReport::Played(Ok(PlayState {
            paused: true,
            at_ms: 1000,
        })));
        let (_, summary) = next_change(&s);
        assert_eq!(
            summary.status,
            Some(Status::Played(PlayState {
                paused: true,
                at_ms: 1000,
            }))
        );
        s.send(Action::ClosePlayer);
        next_change(&s);
        assert_eq!(next(&s), Some(Event::Player(PlayerCommand::Close)));
        s.report(PlayerReport::Closed(Err(PlayerError::NotOpen)));
        let (_, summary) = next_change(&s);
        assert_eq!(
            summary.status,
            Some(Status::NotOpen {
                asked: Asked::Close
            })
        );
        // The selection on E02 since the accept, a file that is not on disk: the stat says so,
        // and nothing goes to the player.
        s.send(Action::Play);
        next_change(&s);
        let (_, summary) = next_change(&s);
        assert_eq!(
            summary.status,
            Some(Status::Refused(Refusal::FileMissing {
                path: tests_support::path(2)
            }))
        );
    }

    // Another review in the same session, as `SwitchView` or `OpenView` swaps the view: B's rows in
    // place of A's, in one splice, under B's name; words that select nothing leave the view as it
    // was, and say why.
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
            summary.status,
            Some(Status::Failed {
                what: Failure::Switch,
                error: "no run named nope".into(),
            })
        );
    }

    // The store's show folders, as review opens and again in the change that shows a decision
    // written, its count read then; a folder's view, opened by its path though it is not on disk,
    // marks that folder open, and the store is read again for it: a decision another process
    // wrote meanwhile is in the count.
    #[test]
    fn the_folders_come_as_review_opens_and_again_once_a_decision_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let s = session(dir.path(), |_| {});
        let k_on = |to_review, open| FolderView {
            path: "/m/K-On!".into(),
            name: "K-On!".into(),
            to_review,
            open,
        };
        let changed = |s: &Arc<Session>| match next(s) {
            Some(Event::Changed {
                summary, folders, ..
            }) => (summary, folders),
            other => panic!("expected a change: {other:?}"),
        };
        assert_eq!(changed(&s).1, Some(vec![k_on(4, false)]));
        s.send(Action::Accept);
        let (summary, folders) = changed(&s);
        assert_eq!(summary.accepted, 3);
        assert_eq!(folders, Some(vec![k_on(1, false)]));
        // The last pending finding, E02's, rejected through a connection of its own, as another
        // process would: nothing here reads the store for it until the view changes.
        let mut other = Store::open_at(&dir.path().join("subcheck.db")).unwrap();
        other
            .decide(&[Decision {
                finding: 3,
                state: State::Rejected,
                edited: None,
                at: Timestamp::UNIX_EPOCH,
            }])
            .unwrap();
        // The accept's message let go, while the view is still to read: the folders as they were.
        s.send(Action::OpenFolder("/m/K-On!".into()));
        let (summary, folders) = changed(&s);
        assert_eq!((summary.status, folders), (None, None));
        let (summary, folders) = changed(&s);
        assert_eq!(summary.title, "K-On!");
        assert_eq!(folders, Some(vec![k_on(0, true)]));
    }

    // `close`'s locked part first, with the core thread between `Play`'s change and its stat:
    // neither the stat nor the player's command it would lead to runs. No job runs, so no group
    // ends in this process.
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
        assert_eq!(next_change(&s).1.status, Some(opening_e01()));
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
        assert_eq!(
            next_change(&s).1.status,
            Some(Status::Refused(Refusal::NothingToUndo))
        );
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

    /// What a UI holds of a session: each list, each splice applied to it as it comes, and the
    /// detail and the logs as last sent.
    #[derive(Debug, Default, PartialEq)]
    struct Held {
        rows: Vec<EntryView>,
        files: Vec<FileView>,
        detail: Option<Detail>,
        logs: Option<FileLogs>,
    }

    impl Held {
        /// What `m` shows now, as a UI that has applied every change would hold it.
        fn of(m: &Review) -> Held {
            Held {
                rows: entry_views(m),
                files: file_views(m),
                detail: m.detail(),
                logs: file_logs(m),
            }
        }

        /// `event` taken in, as a UI does, whatever its order.
        fn take(&mut self, event: Event) {
            let Event::Changed {
                rows,
                detail,
                files,
                logs,
                ..
            } = event
            else {
                return;
            };
            if let Some(s) = rows {
                self.rows.splice(s.at..s.at + s.removed, s.inserted);
            }
            if let Some(s) = files {
                self.files.splice(s.at..s.at + s.removed, s.inserted);
            }
            if let Some(detail) = detail {
                self.detail = detail;
            }
            if let Some(logs) = logs {
                self.logs = logs;
            }
        }
    }

    // Each kind of action but three (`Quit` and `Interrupt` end review, `Playback` is the
    // player's), and no yes to `Recheck` or `Apply`, which would start a real job: a scan's
    // inputs and a dry run's follow the actions, fed in as their jobs send them.
    #[test]
    #[allow(clippy::too_many_lines)] // the actions, a scan's and a dry run's inputs, a read-back
    fn every_change_applied_to_what_a_ui_holds_makes_both_lists_the_detail_and_the_logs() {
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
        let mut held = Held::default();
        held.take(sent.change(&review).unwrap());
        assert_eq!(held, Held::of(&review));
        let mut go = |review: &mut Review, sent: &mut Sent, input: Input, held: &mut Held| {
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
                held.take(event);
            }
            assert_eq!(*held, Held::of(review), "after {what}");
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
            Action::SelectFile(tests_support::path(3)),
            Action::SelectFile(tests_support::path(2)),
            Action::ShowFileFindings,
            Action::ShowPane(Pane::Findings),
            Action::Dismiss,
            Action::Move(1),
            Action::Edit,
            Action::CancelEdit,
            Action::ShowPane(Pane::Files),
        ] {
            go(&mut review, &mut sent, Input::Action(action), &mut held);
        }
        // Each question opens, and a no lets it go.
        for (action, asks) in [
            (Action::Recheck, true),
            (Action::Answer(false), false),
            (Action::ShowPane(Pane::Findings), false),
            (Action::Apply { dry_run: false }, true),
            (Action::Answer(false), false),
        ] {
            let what = format!("{action:?}");
            go(&mut review, &mut sent, Input::Action(action), &mut held);
            assert_eq!(review.question().is_some(), asks, "after {what}");
        }
        // The run's view opened again.
        for action in [Action::OpenView(vec![run.clone()]), Action::Move(-1)] {
            go(&mut review, &mut sent, Input::Action(action), &mut held);
        }
        // A scan of E02 and E03: the rows of their findings turn as it checks them, the lyric's
        // still for E03 once E02 is done, and none once the scan has failed with E03 left queued.
        let turning = |held: &Held| held.rows.iter().filter(|v| v.checking).count();
        let queued = scan::Event::Queued(vec![
            tests_support::path(2).into(),
            tests_support::path(3).into(),
        ]);
        go(&mut review, &mut sent, Input::Scan(queued), &mut held);
        let all = turning(&held);
        assert!(all > 1, "{held:?}");
        let reading = scan::Event::Reading(tests_support::path(2).into());
        go(&mut review, &mut sent, Input::Scan(reading), &mut held);
        let done = scan::Event::Finished {
            path: tests_support::path(2).into(),
            error: None,
            findings: 0,
            lines: vec!["E02: no findings".into()],
        };
        go(&mut review, &mut sent, Input::Scan(done), &mut held);
        assert!((1..all).contains(&turning(&held)), "{held:?}");
        let failed = Input::ScanEnded(Err("the store is locked".into()));
        go(&mut review, &mut sent, failed, &mut held);
        assert_eq!(turning(&held), 0, "{held:?}");
        // A dry run of E02's fix: its file's stage and log, and its findings' marks.
        review
            .progress
            .start_apply(true, format!("subcheck apply {run} --dry-run"));
        let dry = [
            apply::Event::Queued(vec![tests_support::path(2)]),
            apply::Event::Stage(tests_support::path(2), apply::Stage::Plan),
            apply::Event::Done {
                path: tests_support::path(2),
                outcomes: vec![(2, apply::patch::Outcome::Patched)],
                remuxed: false,
                marked: vec![],
                restamped: None,
                text: "E02: would apply 1".into(),
            },
        ];
        for event in dry {
            go(&mut review, &mut sent, Input::Apply(event), &mut held);
        }
        let ended = Input::ApplyEnded(Ok(apply::Summary {
            fixed: 1,
            ..apply::Summary::default()
        }));
        go(&mut review, &mut sent, ended, &mut held);
        assert_eq!(
            held.files[1].applying,
            Some(ApplyStage::Done {
                dry_run: true,
                remuxed: false,
                counts: Counts {
                    applied: 1,
                    ..Counts::default()
                },
            })
        );
        assert_eq!(
            held.logs.as_ref().unwrap().apply_log,
            ["E02: would apply 1"]
        );
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
        go(&mut review, &mut sent, Input::Scan(appended), &mut held);
        assert!(held.rows.iter().any(|v| v.original == "An erly line."));
        // A check's end, then an apply's, as they come from the queue: each reads the folders
        // again before the change it leads to, which carries them, so that decisions another
        // process made meanwhile, here on a connection of their own, are in the counts. Every
        // finding to pending, then to rejected, so that each moves.
        let ids: Vec<u64> = review.run.findings.iter().map(|f| f.id).collect();
        let mut other = Store::open_at(&db).unwrap();
        for (state, input) in [
            (
                State::Pending,
                Input::ScanEnded(Ok(scan::Summary::default())),
            ),
            (
                State::Rejected,
                Input::ApplyEnded(Ok(apply::Summary::default())),
            ),
        ] {
            let before = sent.shelf.clone();
            let decisions: Vec<Decision> = ids
                .iter()
                .map(|&finding| Decision {
                    finding,
                    state,
                    edited: None,
                    at: Timestamp::UNIX_EPOCH,
                })
                .collect();
            other.decide(&decisions).unwrap();
            let now = other.show_folders().unwrap();
            assert_ne!(before, now);
            go(&mut review, &mut sent, input, &mut held);
            assert_eq!(sent.shelf, now);
            let told: Vec<FolderView> =
                now.iter().map(|f| FolderView::of(f, &review.run)).collect();
            assert_eq!(sent.folders, told);
        }
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
                let undone = Some(Status::Refused(Refusal::NothingToUndo));
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
                // `Recheck`'s start writes the store on the core thread before any group can start:
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
