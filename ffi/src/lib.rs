//! Review for a UI in another language: core's [`session::Session`] behind UniFFI, whose bindgen
//! writes the other side (uniffi-bindgen-cpp, for the Qt app). Only what a window uses crosses.
//!
//! Core has no UniFFI dependency, so each type that crosses has a mirror here. Each conversion
//! names every variant and field of what it reads, and builds every field of what it makes, so a
//! field added on either side, or a variant added to the side a conversion reads, fails the build
//! here until it is handled. Actions convert both ways, so that an action added to core fails the
//! build here too: every one of them crosses.

use std::ops::Range;
use std::sync::Arc;

use subcheck_core::review::{detail, model, progress, session, status, update};
use subcheck_core::{apply, group, list, llm, mkv, player, scan, store};

uniffi::setup_scaffolding!();

/// One run under review, for a UI on any thread; a UI opens one per process, and switches the view
/// inside it (`Action::OpenView`) rather than open another ([`session::Session`]).
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
    /// then wait their turn. `karaoke`: its apply re-spaces per-letter karaoke, as
    /// `apply --karaoke` does.
    #[uniffi::constructor]
    pub fn open(selector: Vec<String>, karaoke: bool) -> Arc<Session> {
        Arc::new(Session(session::Session::open(selector, karaoke)))
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

    /// End the stream, and stop the session when it can next be stopped. With no job running or
    /// starting, and review not failed, it returns at once, whatever the session is doing.
    /// Otherwise it first ends the process groups, which can wait for a start stuck in exec or a
    /// file to remove on a stalled mount. What was queued before still comes out of `next_event`
    /// first, a player command among them, so a UI ignores what it reads after calling this, but
    /// for the last event, an `Ended`. It is this call's, whose outro says how to finish a job it
    /// stopped and whose `fatal` is none, whatever the groups ended for; unless review, ending by
    /// itself as this ran, put its own first, which can carry review's `fatal`. A later call adds
    /// nothing to the stream, and returns once the groups the first is ending have ended; at once
    /// when the first ended none. Letting the session go after this returns at once.
    pub fn close(&self) {
        self.0.close();
    }
}

/// To call as the app's main returns, once its session is let go of, as the terminal's main does:
/// while a signal is ending the process, it waits for the signal's cleanup to end it, so that the
/// process does not exit first, with a status of its own, leaving a job's process groups running
/// ([`group::finish`]). At once otherwise.
#[uniffi::export]
pub fn finish() {
    group::finish();
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

/// The findings the list shows ([`list::Filter`]): all, or those of one decision.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Pending,
    Accepted,
    Rejected,
}

/// Which list a UI shows ([`model::Pane`]): the findings, or the run's files.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Findings,
    Files,
}

/// What a `Playback` action asks of the player ([`player::Playback`]).
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

/// A moment to open ([`player::Moment`]): the file, where to seek, the subtitles to show.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Moment {
    pub file: String,
    /// Where the player seeks, in µs, as review gives it, for mpv to show the frame review means.
    pub start_us: i64,
    /// mpv's `--sid`; none leaves the choice to the player.
    pub sid: Option<u64>,
}

/// How the player reaches a moment ([`player::How`]): in the file its mpv has open, or by loading
/// the file.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    Seek,
    Load,
}

/// What review asks of the UI's player ([`player::PlayerCommand`]), which says how it went
/// through `Session::report`.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum PlayerCommand {
    /// Open the moment as `how` says, in the steps core's `PlayerCommand::Open` names: pause; for
    /// a seek, in the file mpv has open, the subtitle track and subtitles on, the exact seek, then
    /// pause off if mpv played or `play` is set, or a load instead should any of it be refused or
    /// mpv have another file open; for a load, the file with its start, subtitle track, subtitles
    /// on, every one of `options` and pause as mpv is to end up (off if mpv played or `play` is
    /// set), as options of that file alone.
    Open {
        moment: Moment,
        how: How,
        options: Vec<FileOption>,
        /// Land playing, whatever mpv's own state says: the user pressed Space in subcheck while
        /// this open waited for its file's check, with no file of review's in the player for the
        /// Space to toggle. Unset, the play state is mpv's own, a pause made in its window among
        /// it.
        play: bool,
    },
    /// Pause or seek the open player.
    Playback(Playback),
    /// Close the player, and leave review open.
    Close,
}

/// One line of the list ([`session::EntryView`]), as a UI shows it: a row, or a member of an
/// expanded row.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // as core's, each says a thing of its own
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
    /// What apply did or would do with the findings it shows: each kind with how many have it, in
    /// the order its findings come; none when none has a mark.
    pub marks: Vec<MarkCount>,
    /// A timing finding's track, as the list tells it apart; none for others.
    pub track: Option<ListTrack>,
    /// A timing finding's fixes, of the findings it shows: each with how many have it, the most
    /// common first; none for others.
    pub fixes: Vec<FixCount>,
    /// A scan is checking its file again: any shown member's, for a row.
    pub checking: bool,
    /// How its file differs from when it was scanned: of a row's first shown member, as `label`.
    pub note: Option<FileNote>,
    /// Its shown members' decisions differ, which its `state` does not say of a row with one
    /// pending: never a member's.
    pub mixed: bool,
    /// The check that found it: of a row's first shown member, as `original` is.
    pub check: Check,
    /// The part of `original` that differs from `wording`, in UTF-16 code units, as `Detail`'s.
    pub original_changed: Span,
    /// The part of `wording` that differs from `original`, in UTF-16 code units, as `Detail`'s.
    pub wording_changed: Span,
    /// Why it was found, empty when nothing says: of a row's first shown member, as `label`.
    pub reason: String,
    /// Its file, as the store names it: of a row's first shown member, as `label`.
    pub file: String,
    /// Whether `Action::Edit` on it opens an edit: `Edit`'s own rule, so that a UI greys what
    /// review would refuse.
    pub editable: bool,
}

/// What found a finding ([`store::Check`]): the LLM, or the timing check.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Llm,
    Timing,
}

/// A kind of mark, and how many of the findings an entry shows have it.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkCount {
    pub kind: MarkKind,
    pub findings: u64,
}

/// What a mark says, without apply's reason ([`model::MarkKind`]): what the list counts, so that
/// two findings left by hand for different reasons count as one kind.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    ByHand,
    NotFound,
    WouldApply,
    AlreadyApplied,
    WouldBeByHand,
    WouldNotBeFound,
    Failed,
    WouldFail,
    Applied,
}

/// A timing finding's fix, and how many of the findings an entry shows have it.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixCount {
    pub fix: Fix,
    pub findings: u64,
}

/// A timing finding's track as the list tells it apart ([`model::ListTrack`]): mkvmerge's ID; its
/// own name and language, as `Detail`'s track has them, when its file's record lists the track
/// (`language` none when not); and whether no other track of its file has that name.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct ListTrack {
    pub id: u64,
    pub name: Option<String>,
    pub language: Option<String>,
    pub apart: bool,
}

/// How a file differs from when it was scanned ([`model::FileNote`]).
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileNote {
    Changed,
    Missing,
}

/// A change to the rows the UI holds ([`session::Splice`]): the `removed` rows at `at` give
/// way to `inserted`.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    pub at: u64,
    pub removed: u64,
    pub inserted: Vec<EntryView>,
}

/// What a UI shows beside the list ([`session::Summary`]), sent whole with any change to it.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // as core's, each says a thing of its own
pub struct Summary {
    /// What the view is called.
    pub title: String,
    /// An index into the rows the UI holds; none when it holds none.
    pub selected: Option<u64>,
    /// Review's message, none when it has none: data the UI words, as it does the question and
    /// the job.
    pub status: Option<Status>,
    /// Findings pending, accepted and rejected: the whole view's, whatever the filter shows.
    pub pending: u64,
    pub accepted: u64,
    pub rejected: u64,
    /// Whether a job runs.
    pub busy: bool,
    /// `Play` waits for its file's check, before the player is asked anything: a UI shows the
    /// video loading from the press.
    pub stamping: bool,
    /// The player is asked to open a file right after this change: a UI shows the video loading
    /// from it, as from `stamping`, so that `Play`'s wait is one from the press to the open. A
    /// player Open event uses it up: read it as false from that event on, until a change says true
    /// again, which is sent even when the last change sent also said true.
    pub open_follows: bool,
    pub filter: Filter,
    pub pane: Pane,
    /// The edit under way; none once it ends, or when what it edits has gone.
    pub edit: Option<Editing>,
    /// The open question, which `Answer` answers.
    pub question: Option<Question>,
    /// The job under way, or the last to end, kept until the next starts, or the view changes.
    pub job: Option<Job>,
    /// An index into the files list the UI holds; none when it holds none.
    pub file_selected: Option<u64>,
    /// What the view's LLM calls used, which the terminal's box of keys shows.
    pub spent: Spent,
    /// The view is of paths (a folder's, the newest run's folders, files), not a run's: what
    /// `SwitchView` opens from it, the newest run that named the selected file where a run's view
    /// opens the run's folders.
    pub of_paths: bool,
    /// How many of the UI's actions review has taken. Each one adds to it, so every action is
    /// answered, by the first event to carry its count: a change, or `Event::Answered` when
    /// nothing else changed.
    pub answered: u64,
}

/// An edit under way ([`session::Editing`]).
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Editing {
    /// The entry it was opened on, which `Reword` decides.
    pub entry: EntryKey,
    /// The wording to start from: the user's, else the suggestion.
    pub wording: String,
}

/// Review's message as data, for a UI to word ([`status::Status`]): what an action, a task or the
/// player did, or why an action did nothing. Files by the names the list gives them, but a missing
/// file's path; an error in the words of whoever made it.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// `Accept`, `Reject` or `Reword` put `findings` findings at `state`: `replaced` of them had
    /// the user's wording, which the suggestion replaced; `kept`, the row's members it left.
    Decided {
        state: State,
        findings: u64,
        replaced: u64,
        kept: Option<Kept>,
    },
    /// `Clear` put `findings` findings back to pending.
    Cleared { findings: u64 },
    /// `Undo` put `findings` findings back, in the files `labels` names, each once.
    Undone { findings: u64, labels: Vec<String> },
    /// `findings` new findings of file `label` took their rows' decision, `state` the first's.
    Inherited {
        label: String,
        findings: u64,
        state: State,
    },
    /// A decision's finding was replaced by a check of file `label`: the decision went to the new
    /// one, or with none (`gone`) nowhere.
    Replaced { label: String, gone: bool },
    /// A check of file `label` left the edit with none of its findings, and ended it.
    EditEnded { label: String },
    /// The action did nothing, for this reason.
    Refused { why: Refusal },
    /// File `label` is queued to be checked again once the scan ends.
    QueuedAgain { label: String },
    /// A check again left out file `label`, the first of those no longer on disk.
    NoSuchFile { label: String },
    /// A view switch (`SwitchView`, `OpenView`, `OpenFolder`) is reading the view it opens. The
    /// view shown stays, and works, until it comes.
    OpeningView,
    /// `Play` asks the player to open file `label` at `at_ms`. With `play`, the video lands
    /// playing, for a Space pressed while the open waited; a later Space goes to the player, which
    /// toggles the video as it lands, and the line stops saying so.
    Opening {
        label: String,
        at_ms: i64,
        play: bool,
    },
    /// The check of file `label`, which `Play` waits for, has not answered in a while: its share
    /// is slow or down. The wait goes on.
    NotAnswering { label: String },
    /// `ClosePlayer` ended `Play`'s wait for the check of file `label`: nothing opens when it
    /// answers.
    StoppedWaiting { label: String },
    /// The player opened it, and left mpv paused or playing.
    Opened { paused: bool },
    /// Where playback is after a `Playback`.
    Played { state: PlayState },
    /// The player closed.
    Closed,
    /// The player has no mpv to ask, or none open to close; `asked` is what it was asked.
    NotOpen { asked: Asked },
    /// mpv has no file open.
    NoFile,
    /// mpv is still opening the file.
    StillOpening,
    /// mpv could not open file `label`.
    CouldNotOpen { label: String },
    /// mpv did not answer in the player's time.
    NoAnswer,
    /// mpv closed the connection without answering.
    HungUp,
    /// The video pane could not show video in the player's time.
    NoVideo,
    /// What failed, and its error in the words of whoever made it.
    Failed { what: Failure, error: String },
    /// What a scan warned of as it planned: the first warning, and how many more.
    Warned { first: String, more: u64 },
}

/// The members a row's decision left as they were ([`status::Kept`]): how many, and the decision
/// of the first.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kept {
    pub findings: u64,
    pub state: State,
}

/// Why an action did nothing ([`status::Refusal`]).
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// `Select` named an entry whose findings went, or that the list no longer shows.
    SelectionStays,
    /// `SelectFile` named file `label`, which the files view does not have.
    NotInView { label: String },
    /// `Edit` on a timing fix, which is accepted or rejected, never reworded.
    TimingFix,
    /// `Reword` with no text: the edit goes on.
    EmptyWording,
    /// `Reword` on an edit none of whose findings is left.
    EditOutOfDate,
    /// `Accept`, `Reject` or `Reword`: every finding is `state` already, with that wording.
    Already { state: State },
    /// `Clear` on findings none of which is decided.
    NothingToClear,
    /// `Undo` with no decision of this session left.
    NothingToUndo,
    /// `NextFile` on the last file.
    LastFile,
    /// `PreviousFile` on the first file.
    FirstFile,
    /// `Play` on a file not on disk, at `path`.
    FileMissing { path: String },
    /// `Play` on a timing finding no line of whose track moves.
    NothingToOpen,
    /// A new view, apply or a dry run while the scan runs.
    ScanRunning,
    /// A new view, or a check again, while apply runs.
    ApplyRunning,
    /// `Apply` while apply runs.
    AlreadyApplying,
    /// `Apply` with no accepted fix.
    NoAcceptedFixes,
    /// `Recheck` on file `label`, which the scan is checking now.
    BeingChecked { label: String },
    /// `ShowFileFindings`: the filter shows no finding of file `label`.
    NoFindingsShown { label: String },
}

/// What failed, for `Status::Failed` ([`status::Failure`]).
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Writing the decisions: none was saved.
    Save,
    /// Reading another view: the view stays.
    Switch,
    /// Reading file `label` back from the store.
    ReadBack { label: String },
    /// Checking a file again: finding its run, or starting the scan.
    Recheck,
    /// A request mpv refused.
    Mpv,
    /// Starting mpv, or reaching it.
    Player,
}

/// What the player was asked, for `Status::NotOpen` ([`status::Asked`]).
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    Open,
    Playback,
    Close,
}

/// The open question as data, for a UI to word ([`model::Question`]): what a yes does, with what
/// its words need. Files by the names the list gives them.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum Question {
    /// Quit, stopping the job under way.
    Quit { stopping: Stopping },
    /// Check file `label` again, with `backend`, the LLM its run was scanned with.
    Recheck { label: String, backend: String },
    /// Apply the view's accepted fixes: `fixes` of them, in `files` files, `applied` of them in
    /// their files already.
    Apply {
        fixes: u64,
        files: u64,
        applied: u64,
    },
}

/// The job a quit would stop, and how far it is ([`progress::Stopping`]).
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum Stopping {
    /// The scan: the files done of those it checks, and the files `Recheck` queued for after it,
    /// which quitting leaves unchecked.
    Scan { done: u64, queued: u64, again: u64 },
    /// Apply, or with `dry_run` a dry run: the files done of those with fixes, and where it is.
    Apply {
        dry_run: bool,
        done: u64,
        queued: u64,
        at: ApplyAt,
    },
}

/// Where apply is ([`progress::ApplyAt`]), a file by the name the list gives it.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum ApplyAt {
    /// A file at a stage.
    On { file: String, stage: ApplyStage },
    /// No file at a stage: the next it goes to.
    Before { file: String },
    /// Past its last file.
    Finishing,
    /// It has not said its files yet.
    Starting,
}

/// Where apply is with a file ([`apply::Stage`]).
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyStage {
    /// Reading its tracks and finding each fix's events.
    Plan,
    /// Writing the fixed tracks and remuxing with mkvmerge.
    Remux,
    /// Checking the remux against the original.
    Check,
    /// Putting the remux in the original's place.
    Swap,
}

/// Where a scan is with a file it checks ([`progress::Checking`]).
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checking {
    Queued,
    Reading,
    /// Read; `answered` of the `chunks` its lines need have come back from the LLM.
    Asking {
        answered: u64,
        chunks: u64,
    },
}

/// A file a scan is on, by the name the list gives it ([`progress::UnderWay`]).
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct UnderWay {
    pub file: String,
    pub stage: Checking,
}

/// What some LLM calls used ([`llm::Spent`]): a scan's, or the view's.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct Spent {
    pub calls: u64,
    /// Of them, those whose backend said nothing of what they used: the sums leave them out.
    pub unreported: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// The calls that reported a price, and its sum in US dollars, at the provider's API rates.
    pub priced: u64,
    pub cost_usd: f64,
}

/// What a scan came to ([`scan::Summary`]).
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct ScanSummary {
    pub run: String,
    pub files: u64,
    pub checked: u64,
    pub failed: u64,
    /// Files unchanged since an earlier scan, so not checked again.
    pub cached: u64,
    pub findings: u64,
    /// Of the findings, those of the timing check.
    pub timing: u64,
    pub spent: Spent,
    /// The paths the scan was given, absolute.
    pub paths: Vec<String>,
}

/// What apply came to ([`apply::Summary`]): fixes made (by a dry run, to make), in their files
/// already, left for a fix by hand, and not found; files remuxed, and files it failed on.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplySummary {
    pub fixed: u64,
    pub already: u64,
    pub by_hand: u64,
    pub not_found: u64,
    pub remuxed: u64,
    pub failed: u64,
}

/// The job under way, or the last to end ([`progress::Job`]), kept until the next starts, or the
/// view changes.
#[derive(uniffi::Enum, Debug, Clone, PartialEq)]
pub enum Job {
    /// A scan running into run `run`; `adding` when it adds to that run, as a re-check does. Of
    /// the files it checks, those done, and of those the ones that failed; the findings it found;
    /// the files it is reading or asking the LLM about; and what its LLM calls have used.
    Scanning {
        run: String,
        adding: bool,
        done: u64,
        queued: u64,
        failed: u64,
        findings: u64,
        under_way: Vec<UnderWay>,
        spent: Spent,
    },
    /// The last scan, done: its run is its summary's.
    ScanDone { adding: bool, summary: ScanSummary },
    /// The last scan, failed, and why.
    ScanFailed {
        run: String,
        adding: bool,
        error: String,
    },
    /// Apply running, or with `dry_run` a dry run: the files done of those with fixes, where it
    /// is, and what the files done came to.
    Applying {
        dry_run: bool,
        done: u64,
        queued: u64,
        at: ApplyAt,
        so_far: ApplySummary,
    },
    /// The last apply or dry run, done.
    ApplyDone {
        dry_run: bool,
        summary: ApplySummary,
    },
    /// The last apply or dry run, failed as a whole, and why.
    ApplyFailed { dry_run: bool, error: String },
}

/// What a UI hears, in order, from `Session::next_event` ([`session::Event`]).
#[derive(uniffi::Enum, Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // as core's, each is made once and moved across
pub enum Event {
    /// What the UI shows changed. `rows`, `files`, `detail`, `logs` and `folders` are none when
    /// they did not, but never in the first event, which carries a splice of each list from empty
    /// even for a run with no findings, and the detail, the logs and the folders even when there
    /// are none.
    Changed {
        rows: Option<Splice>,
        summary: Summary,
        /// The selected finding's detail, when it changed.
        detail: Option<DetailNow>,
        /// The files list's change, as `rows` is the findings list's.
        files: Option<FilesSplice>,
        /// The selected file's logs, when they changed.
        logs: Option<LogsNow>,
        /// The store's show folders, newest first, the whole list when any of it changed.
        folders: Option<Vec<FolderView>>,
    },
    /// Review took one of the UI's actions, and nothing the UI shows changed: the count of actions
    /// taken alone (`Summary::answered`), the rest of the summary as last sent.
    Answered { answered: u64 },
    /// For the UI's player, which runs it and says how it went through `Session::report`.
    Player(PlayerCommand),
    /// The last event: why review could not go on, if it could not, and what is left to say of the
    /// jobs it ran. A job's process groups are ended before it comes.
    Ended {
        fatal: Option<String>,
        outro: Vec<String>,
    },
}

/// What a UI shows of the selected finding beside the list ([`detail::Detail`]), as data each UI
/// words: the one the list has selected, or the first its selected row shows.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Detail {
    /// Its file, as the store names it.
    pub file: String,
    /// The name the list gives that file.
    pub label: String,
    /// What the row is when the list has a row of several selected.
    pub group: Option<Group>,
    /// Where its line starts, on the occurrence the list goes by; none for a timing finding,
    /// whose fix is a whole track's.
    pub at_ms: Option<i64>,
    /// The track that occurrence is on, or a timing finding's track, when the record of its file
    /// names that track.
    pub track: Option<Track>,
    /// Its line's other occurrences; none are counted for a timing finding.
    pub more: u64,
    pub note: Option<FileNote>,
    /// Where the scan checking its file again is with that file.
    pub checking: Option<Checking>,
    /// The scan read a copy of its file that was cut short.
    pub truncated: Option<Truncated>,
    pub state: State,
    /// The wording is the user's, not the suggestion.
    pub edited: bool,
    /// What apply did, or would do, with its fix.
    pub mark: Option<Mark>,
    pub original: String,
    pub wording: String,
    /// The part of `original` that differs from `wording`, in UTF-16 code units, as QML, NSString
    /// and C# count a string; for a timing finding, all of it, as its two sides are descriptions,
    /// not lines of the file.
    pub original_changed: Span,
    /// The part of `wording` that differs from `original`, in UTF-16 code units; for a timing
    /// finding, all of it.
    pub wording_changed: Span,
    /// Why it was found; empty when nothing says.
    pub reason: String,
    pub kind: Kind,
}

/// The selected finding's detail as it is now: none when nothing is selected. A record of its own,
/// so that "unchanged" (no `DetailNow` in a change) and "now none" stay two things in every
/// language a binding is made for: an optional of an optional is one in some, and no type in C#.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct DetailNow {
    pub detail: Option<Detail>,
}

/// The selected file's logs as they are now: none when the files list is empty. A record of its
/// own, as `DetailNow` is.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct LogsNow {
    pub logs: Option<FileLogs>,
}

/// A part of a string, from `start` up to `end`, in UTF-16 code units.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u64,
    pub end: u64,
}

/// A row of several ([`detail::Group`]): the members the filter shows, in the row's order, never
/// none, and how many it hides.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub members: Vec<Member>,
    pub hidden: u64,
}

/// A member of a row ([`detail::Member`]): the name its file goes by in the list, and a timing
/// finding's fix.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub label: String,
    pub fix: Option<Fix>,
}

/// A timing finding's fix ([`model::Fix`]).
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    /// The whole track, by its one stretch's frames, which are late when positive: the fix moves
    /// it earlier.
    Shift { frames: i64 },
    /// Stretches of the track, each by its own frames, or none.
    Partial,
}

/// A track of a finding's file, as its record has it ([`detail::Track`]): mkvmerge's ID, mpv's
/// `sid`, its own name if it has one, and its language (the BCP 47 tag, when it has one).
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub id: u64,
    pub sid: u64,
    pub name: Option<String>,
    pub language: String,
}

/// A copy of a file cut short ([`mkv::Truncated`]): the length its Segment declares, one past its
/// last byte, and the file's own.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Truncated {
    pub declared_end: u64,
    pub file_size: u64,
}

/// What apply said of a finding's fix, else that the fix is in its file ([`model::Marked`]). A
/// reason is apply's own words, as a job's error is.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    /// Left for a fix by hand, and why.
    ByHand {
        why: String,
    },
    NotFound,
    WouldApply,
    /// A dry run found the fix in its file already.
    AlreadyApplied,
    WouldBeByHand {
        why: String,
    },
    WouldNotBeFound,
    /// Apply failed on its file, and why: the fix is not in it.
    Failed {
        why: String,
    },
    /// A dry run failed on its file, and why.
    WouldFail {
        why: String,
    },
    /// Nothing apply said since the view opened: the fix is in its file.
    Applied,
}

/// What differs by kind of finding ([`detail::Kind`]).
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A line: the lines around it, in time order.
    Line {
        before: Vec<String>,
        after: Vec<String>,
    },
    /// A track's timing: each stretch of it, the lines its fix leaves where they are, and where
    /// `Play` opens it.
    Timing {
        stretches: Vec<Stretch>,
        unshifted: Vec<Times>,
        opens: Option<TimingMoment>,
    },
}

/// Part of a track at one offset ([`detail::Stretch`]): how many frames late (early when negative;
/// 0 in sync), from its first line's start to its last line's end; none from the track's start,
/// or to its end.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stretch {
    pub frames: i64,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
}

/// When a line starts and ends ([`detail::Times`]).
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Times {
    pub start_ms: i64,
    pub end_ms: i64,
}

/// Where `Play` opens a timing finding ([`list::TimingMoment`]): on the frame that shows the error,
/// or else on the first line of a shifted stretch; on mpv's `sid` for its track.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingMoment {
    pub at_ms: i64,
    pub sid: Option<u64>,
    pub on_frame: bool,
}

/// One line of the files list ([`session::FileView`]): a file of the run, and where the scan and
/// apply are with it.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FileView {
    /// The file as the store names it, which `SelectFile` takes.
    pub path: String,
    /// The name it goes by in the list.
    pub label: String,
    /// Where the scan is with it; for a file not scanned since the view opened, as recorded.
    pub stage: ScanStage,
    /// Where the latest apply or dry run is with it, which a UI shows in place of `stage`: none
    /// until that apply queues it, and once a later scan checks it again.
    pub applying: Option<FileApply>,
    /// Distinct lines checked, from its record.
    pub lines: Option<u64>,
    /// Checked for timing alone, with no lines checked by the LLM to count.
    pub timing_only: bool,
}

/// Where a file is in the scan ([`progress::Stage`]). Unlike `Checking`, which is a row's and a
/// detail's, it is the files list's, which shows each stage, and a stage left by a scan that ended.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum ScanStage {
    /// Waiting for the reader.
    Queued,
    /// Queued by `Recheck` while a scan runs: checked again when it ends.
    QueuedAgain,
    Reading,
    /// Read; `answered` of the `chunks` its lines need have come back from the LLM.
    Asking {
        answered: u64,
        chunks: u64,
    },
    Done {
        findings: u64,
    },
    /// Why the scan could not check it, in the scan's words.
    Failed {
        error: String,
    },
}

/// Where a file is in the latest apply or dry run ([`progress::ApplyStage`]).
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum FileApply {
    /// It has fixes, and apply has not reached it.
    Queued,
    At {
        stage: ApplyStage,
    },
    /// Done, by a dry run or not; whether it was remuxed, and what became of its fixes.
    Done {
        dry_run: bool,
        remuxed: bool,
        counts: ApplyCounts,
    },
    /// Why apply failed on it, in apply's words.
    Failed {
        error: String,
    },
}

/// What apply said of a file's fixes, counted ([`progress::Counts`]): made (by a dry run, to
/// make), in the file already, left for a fix by hand, and not found.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyCounts {
    pub applied: u64,
    pub already: u64,
    pub by_hand: u64,
    pub not_found: u64,
}

/// A change to the files list the UI holds ([`session::Splice`]), as `Splice` is to the rows.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FilesSplice {
    pub at: u64,
    pub removed: u64,
    pub inserted: Vec<FileView>,
}

/// What the scan and the screen's applies wrote for the selected file ([`session::FileLogs`]):
/// lines of text, in the jobs' words.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FileLogs {
    pub log: Vec<String>,
    pub apply_log: Vec<String>,
}

/// One of the store's show folders ([`session::FolderView`]): the folders every run's files sit
/// in, a folder inside another left out, for a UI to open one with `OpenFolder`.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FolderView {
    /// The folder as the store names its files' paths, which `OpenFolder` takes.
    pub path: String,
    /// What its view is called once open.
    pub name: String,
    /// How many of the findings recorded under it are pending.
    pub to_review: u64,
    /// The view shown is of paths, one of which is this folder or a path inside it.
    pub open: bool,
}

/// What a UI asks of review ([`update::Action`]): each of core's actions. While a question is
/// open, every action but `Answer` and `Interrupt` lets it go and does nothing else.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Quit; while a job runs, ask first (the summary's `question`).
    Quit,
    /// As `Quit`, asking first while a job runs; but a second, with the question open, quits.
    Interrupt,
    /// Select the entry this key names. A member of a collapsed row selects its row; a key whose
    /// entry the list does not show leaves the selection where it is, and the status line says so.
    Select {
        key: EntryKey,
    },
    /// Select the entry `by` places on in the list as shown, or back when negative, stopping at
    /// its first and last: at an end, the entry already selected. Review works it out from its own
    /// selection, so that each of several quick moves starts where the one before ended.
    Move {
        by: i32,
    },
    /// Expand the selected row into its members, or collapse it (from the row or a member).
    Toggle,
    /// Show the findings under this filter, the selection kept on its finding's row.
    SetFilter {
        filter: Filter,
    },
    /// Select the first entry of the next file in the list, or of the file before.
    NextFile,
    PreviousFile,
    Accept,
    Reject,
    /// Put the selected entry's decided findings back to pending.
    Clear,
    Undo,
    /// Start rewording the selected entry: the summary's `edit` gives the wording to start from.
    Edit,
    /// Accept the edit's findings with this wording.
    Reword {
        wording: String,
    },
    CancelEdit,
    ShowPane {
        pane: Pane,
    },
    /// Open the other view of the findings: from a run's, the run's folders; from a paths view,
    /// the newest run that named the selected file. Not while a job runs.
    SwitchView,
    /// Open the view these words select (`[run | paths…]`, as `Session::open` takes them) in place
    /// of the one shown. Should it fail, the view stays, and the status line says why. Not while a
    /// job runs.
    OpenView {
        selector: Vec<String>,
    },
    /// Open the view of every file recorded under this folder, a `FolderView`'s path, in place of
    /// the one shown, whether or not the folder is on disk: words naming a folder that is not
    /// would name a run. Should it fail, the view stays, and the status line says why. Not while a
    /// job runs.
    OpenFolder {
        folder: String,
    },
    /// Apply the view's accepted fixes, once asked; with `dry_run`, at once, saying what would
    /// change and writing nothing.
    Apply {
        dry_run: bool,
    },
    /// Open the selected entry's moment in the player.
    Play,
    ClosePlayer,
    Playback {
        playback: Playback,
    },
    /// Answer the open question: yes, or let it go.
    Answer {
        yes: bool,
    },
    /// Select the file at this path in the files list. A path the list does not have leaves the
    /// selection where it is, and the status line says so.
    SelectFile {
        path: String,
    },
    /// The findings, on the first entry with one of the selected file's.
    ShowFileFindings,
    /// Check the selected file again, once asked.
    Recheck,
    /// Only lets the status message go: what a UI sends for a key, button or click with no action
    /// of its own.
    Dismiss,
}

/// One of mpv's options for the one file a load opens ([`player::FileOption`]), as `--name=value`
/// names it: words the player hands mpv as they are.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FileOption {
    pub name: String,
    pub value: String,
}

/// What went wrong with a player command ([`player::PlayerError`]), for review to make its message
/// of (`Status`), which each UI words.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum PlayerError {
    /// No mpv to ask, or nothing open in it to close.
    NotOpen,
    /// mpv has no file open.
    NoFile,
    /// mpv is still opening the file: it names it, but has no position yet.
    StillOpening,
    /// mpv could not open this file: its load ended on an error.
    CouldNotOpen { file: String },
    /// mpv did not answer a request within the player's time.
    NoAnswer,
    /// mpv closed the connection without answering.
    HungUp,
    /// The video pane could not show video within the player's time.
    NoVideo,
    /// mpv refused a request: its words.
    Mpv(String),
    /// Anything else: starting mpv, or reaching it.
    Failed(String),
}

/// What the UI's player says of a command it ran, or later of a file it had said was open
/// ([`session::PlayerReport`]). A failure is a variant of its own: UniFFI has no `Result` to
/// carry it in.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum PlayerReport {
    /// The moment opened, and whether mpv was left paused there.
    Opened { paused: bool },
    /// Why an `Open` failed.
    OpenFailed { error: PlayerError },
    /// Where playback is after a `Playback`.
    Played { state: PlayState },
    /// Why a `Playback` failed.
    PlayFailed { error: PlayerError },
    /// The player closed, or why not.
    Closed { error: Option<PlayerError> },
    /// A file the player had said was open failed after all: a load that outlasted the player's
    /// wait for it, then ended on an error.
    Failed { error: PlayerError },
}

/// A position or a count, as UniFFI carries it: it has no `usize`, and no Rust target's is wider
/// than a u64.
fn wide(n: usize) -> u64 {
    n as u64
}

/// `chars`, a range of `s`'s characters as core counts them (Rust's `char`s), in UTF-16 code
/// units, as the UIs count: a character past U+FFFF takes two.
fn utf16(s: &str, chars: &Range<usize>) -> Span {
    let units = |n: usize| s.chars().take(n).map(char::len_utf16).sum::<usize>();
    Span {
        start: wide(units(chars.start)),
        end: wide(units(chars.end)),
    }
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

impl From<list::Filter> for Filter {
    fn from(filter: list::Filter) -> Filter {
        match filter {
            list::Filter::All => Filter::All,
            list::Filter::Only(store::State::Pending) => Filter::Pending,
            list::Filter::Only(store::State::Accepted) => Filter::Accepted,
            list::Filter::Only(store::State::Rejected) => Filter::Rejected,
        }
    }
}

impl From<Filter> for list::Filter {
    fn from(filter: Filter) -> list::Filter {
        match filter {
            Filter::All => list::Filter::All,
            Filter::Pending => list::Filter::Only(store::State::Pending),
            Filter::Accepted => list::Filter::Only(store::State::Accepted),
            Filter::Rejected => list::Filter::Only(store::State::Rejected),
        }
    }
}

impl From<model::Pane> for Pane {
    fn from(pane: model::Pane) -> Pane {
        match pane {
            model::Pane::Findings => Pane::Findings,
            model::Pane::Files => Pane::Files,
        }
    }
}

impl From<Pane> for model::Pane {
    fn from(pane: Pane) -> model::Pane {
        match pane {
            Pane::Findings => model::Pane::Findings,
            Pane::Files => model::Pane::Files,
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

impl From<player::PlayState> for PlayState {
    fn from(state: player::PlayState) -> PlayState {
        let player::PlayState { paused, at_ms } = state;
        PlayState { paused, at_ms }
    }
}

impl From<player::Moment> for Moment {
    fn from(moment: player::Moment) -> Moment {
        let player::Moment {
            file,
            start_us,
            sid,
        } = moment;
        Moment {
            file,
            start_us,
            sid: sid.map(wide),
        }
    }
}

impl From<player::FileOption> for FileOption {
    fn from(option: player::FileOption) -> FileOption {
        let player::FileOption { name, value } = option;
        FileOption { name, value }
    }
}

impl From<player::How> for How {
    fn from(how: player::How) -> How {
        match how {
            player::How::Seek => How::Seek,
            player::How::Load => How::Load,
        }
    }
}

impl From<player::PlayerCommand> for PlayerCommand {
    fn from(command: player::PlayerCommand) -> PlayerCommand {
        match command {
            player::PlayerCommand::Open {
                moment,
                how,
                options,
                play,
            } => PlayerCommand::Open {
                moment: moment.into(),
                how: how.into(),
                options: options.into_iter().map(FileOption::from).collect(),
                play,
            },
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
            marks,
            track,
            fixes,
            checking,
            note,
            mixed,
            check,
            original_changed,
            wording_changed,
            reason,
            file,
            editable,
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
            original_changed: utf16(&original, &original_changed),
            wording_changed: utf16(&wording, &wording_changed),
            original,
            wording,
            style,
            marks: marks
                .into_iter()
                .map(|(kind, n)| MarkCount {
                    kind: kind.into(),
                    findings: wide(n),
                })
                .collect(),
            track: track.map(ListTrack::from),
            fixes: fixes
                .into_iter()
                .map(|(fix, n)| FixCount {
                    fix: fix.into(),
                    findings: wide(n),
                })
                .collect(),
            checking,
            note: note.map(FileNote::from),
            mixed,
            check: check.into(),
            reason,
            file,
            editable,
        }
    }
}

impl From<store::Check> for Check {
    fn from(check: store::Check) -> Check {
        match check {
            store::Check::Llm => Check::Llm,
            store::Check::Timing => Check::Timing,
        }
    }
}

impl From<model::MarkKind> for MarkKind {
    fn from(kind: model::MarkKind) -> MarkKind {
        match kind {
            model::MarkKind::ByHand => MarkKind::ByHand,
            model::MarkKind::NotFound => MarkKind::NotFound,
            model::MarkKind::WouldApply => MarkKind::WouldApply,
            model::MarkKind::AlreadyApplied => MarkKind::AlreadyApplied,
            model::MarkKind::WouldBeByHand => MarkKind::WouldBeByHand,
            model::MarkKind::WouldNotBeFound => MarkKind::WouldNotBeFound,
            model::MarkKind::Failed => MarkKind::Failed,
            model::MarkKind::WouldFail => MarkKind::WouldFail,
            model::MarkKind::Applied => MarkKind::Applied,
        }
    }
}

impl From<model::ListTrack> for ListTrack {
    fn from(track: model::ListTrack) -> ListTrack {
        let model::ListTrack {
            id,
            name,
            language,
            apart,
        } = track;
        ListTrack {
            id: wide(id),
            name,
            language,
            apart,
        }
    }
}

impl From<model::FileNote> for FileNote {
    fn from(note: model::FileNote) -> FileNote {
        match note {
            model::FileNote::Changed => FileNote::Changed,
            model::FileNote::Missing => FileNote::Missing,
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
            stamping,
            open_follows,
            filter,
            pane,
            edit,
            question,
            job,
            file_selected,
            spent,
            of_paths,
            answered,
        } = summary;
        Summary {
            title,
            selected: selected.map(wide),
            status: status.map(Status::from),
            pending: wide(pending),
            accepted: wide(accepted),
            rejected: wide(rejected),
            busy,
            stamping,
            open_follows,
            filter: filter.into(),
            pane: pane.into(),
            edit: edit.map(Editing::from),
            question: question.map(Question::from),
            job: job.map(Job::from),
            file_selected: file_selected.map(wide),
            spent: spent.into(),
            of_paths,
            answered,
        }
    }
}

impl From<session::Editing> for Editing {
    fn from(edit: session::Editing) -> Editing {
        let session::Editing { entry, wording } = edit;
        Editing {
            entry: entry.into(),
            wording,
        }
    }
}

impl From<status::Status> for Status {
    fn from(status: status::Status) -> Status {
        match status {
            status::Status::Decided {
                state,
                findings,
                replaced,
                kept,
            } => Status::Decided {
                state: state.into(),
                findings: wide(findings),
                replaced: wide(replaced),
                kept: kept.map(Kept::from),
            },
            status::Status::Cleared { findings } => Status::Cleared {
                findings: wide(findings),
            },
            status::Status::Undone { findings, labels } => Status::Undone {
                findings: wide(findings),
                labels,
            },
            status::Status::Inherited {
                label,
                findings,
                state,
            } => Status::Inherited {
                label,
                findings: wide(findings),
                state: state.into(),
            },
            status::Status::Replaced { label, gone } => Status::Replaced { label, gone },
            status::Status::EditEnded { label } => Status::EditEnded { label },
            status::Status::Refused(why) => Status::Refused { why: why.into() },
            status::Status::QueuedAgain { label } => Status::QueuedAgain { label },
            status::Status::NoSuchFile { label } => Status::NoSuchFile { label },
            status::Status::OpeningView => Status::OpeningView,
            status::Status::Opening { label, at_ms, play } => {
                Status::Opening { label, at_ms, play }
            }
            status::Status::NotAnswering { label } => Status::NotAnswering { label },
            status::Status::StoppedWaiting { label } => Status::StoppedWaiting { label },
            status::Status::Opened { paused } => Status::Opened { paused },
            status::Status::Played(state) => Status::Played {
                state: state.into(),
            },
            status::Status::Closed => Status::Closed,
            status::Status::NotOpen { asked } => Status::NotOpen {
                asked: asked.into(),
            },
            status::Status::NoFile => Status::NoFile,
            status::Status::StillOpening => Status::StillOpening,
            status::Status::CouldNotOpen { label } => Status::CouldNotOpen { label },
            status::Status::NoAnswer => Status::NoAnswer,
            status::Status::HungUp => Status::HungUp,
            status::Status::NoVideo => Status::NoVideo,
            status::Status::Failed { what, error } => Status::Failed {
                what: what.into(),
                error,
            },
            status::Status::Warned { first, more } => Status::Warned {
                first,
                more: wide(more),
            },
        }
    }
}

impl From<status::Kept> for Kept {
    fn from(kept: status::Kept) -> Kept {
        let status::Kept { findings, state } = kept;
        Kept {
            findings: wide(findings),
            state: state.into(),
        }
    }
}

impl From<status::Refusal> for Refusal {
    fn from(why: status::Refusal) -> Refusal {
        match why {
            status::Refusal::SelectionStays => Refusal::SelectionStays,
            status::Refusal::NotInView { label } => Refusal::NotInView { label },
            status::Refusal::TimingFix => Refusal::TimingFix,
            status::Refusal::EmptyWording => Refusal::EmptyWording,
            status::Refusal::EditOutOfDate => Refusal::EditOutOfDate,
            status::Refusal::Already { state } => Refusal::Already {
                state: state.into(),
            },
            status::Refusal::NothingToClear => Refusal::NothingToClear,
            status::Refusal::NothingToUndo => Refusal::NothingToUndo,
            status::Refusal::LastFile => Refusal::LastFile,
            status::Refusal::FirstFile => Refusal::FirstFile,
            status::Refusal::FileMissing { path } => Refusal::FileMissing { path },
            status::Refusal::NothingToOpen => Refusal::NothingToOpen,
            status::Refusal::ScanRunning => Refusal::ScanRunning,
            status::Refusal::ApplyRunning => Refusal::ApplyRunning,
            status::Refusal::AlreadyApplying => Refusal::AlreadyApplying,
            status::Refusal::NoAcceptedFixes => Refusal::NoAcceptedFixes,
            status::Refusal::BeingChecked { label } => Refusal::BeingChecked { label },
            status::Refusal::NoFindingsShown { label } => Refusal::NoFindingsShown { label },
        }
    }
}

impl From<status::Failure> for Failure {
    fn from(what: status::Failure) -> Failure {
        match what {
            status::Failure::Save => Failure::Save,
            status::Failure::Switch => Failure::Switch,
            status::Failure::ReadBack { label } => Failure::ReadBack { label },
            status::Failure::Recheck => Failure::Recheck,
            status::Failure::Mpv => Failure::Mpv,
            status::Failure::Player => Failure::Player,
        }
    }
}

impl From<status::Asked> for Asked {
    fn from(asked: status::Asked) -> Asked {
        match asked {
            status::Asked::Open => Asked::Open,
            status::Asked::Playback => Asked::Playback,
            status::Asked::Close => Asked::Close,
        }
    }
}

impl From<model::Question> for Question {
    fn from(question: model::Question) -> Question {
        match question {
            model::Question::Quit(stopping) => Question::Quit {
                stopping: stopping.into(),
            },
            model::Question::Recheck { label, backend } => Question::Recheck { label, backend },
            model::Question::Apply {
                fixes,
                files,
                applied,
            } => Question::Apply {
                fixes: wide(fixes),
                files: wide(files),
                applied: wide(applied),
            },
        }
    }
}

impl From<progress::Stopping> for Stopping {
    fn from(stopping: progress::Stopping) -> Stopping {
        match stopping {
            progress::Stopping::Scan {
                done,
                queued,
                again,
            } => Stopping::Scan {
                done: wide(done),
                queued: wide(queued),
                again: wide(again),
            },
            progress::Stopping::Apply {
                dry_run,
                done,
                queued,
                at,
            } => Stopping::Apply {
                dry_run,
                done: wide(done),
                queued: wide(queued),
                at: at.into(),
            },
        }
    }
}

impl From<progress::ApplyAt> for ApplyAt {
    fn from(at: progress::ApplyAt) -> ApplyAt {
        match at {
            progress::ApplyAt::On { file, stage } => ApplyAt::On {
                file,
                stage: stage.into(),
            },
            progress::ApplyAt::Before { file } => ApplyAt::Before { file },
            progress::ApplyAt::Finishing => ApplyAt::Finishing,
            progress::ApplyAt::Starting => ApplyAt::Starting,
        }
    }
}

impl From<apply::Stage> for ApplyStage {
    fn from(stage: apply::Stage) -> ApplyStage {
        match stage {
            apply::Stage::Plan => ApplyStage::Plan,
            apply::Stage::Remux => ApplyStage::Remux,
            apply::Stage::Check => ApplyStage::Check,
            apply::Stage::Swap => ApplyStage::Swap,
        }
    }
}

impl From<progress::Checking> for Checking {
    fn from(checking: progress::Checking) -> Checking {
        match checking {
            progress::Checking::Queued => Checking::Queued,
            progress::Checking::Reading => Checking::Reading,
            progress::Checking::Asking { answered, chunks } => Checking::Asking {
                answered: wide(answered),
                chunks: wide(chunks),
            },
        }
    }
}

impl From<progress::UnderWay> for UnderWay {
    fn from(under_way: progress::UnderWay) -> UnderWay {
        let progress::UnderWay { file, stage } = under_way;
        UnderWay {
            file,
            stage: stage.into(),
        }
    }
}

impl From<llm::Spent> for Spent {
    fn from(spent: llm::Spent) -> Spent {
        let llm::Spent {
            calls,
            unreported,
            input_tokens,
            output_tokens,
            priced,
            cost_usd,
        } = spent;
        Spent {
            calls: wide(calls),
            unreported: wide(unreported),
            input_tokens,
            output_tokens,
            priced: wide(priced),
            cost_usd,
        }
    }
}

impl From<scan::Summary> for ScanSummary {
    fn from(summary: scan::Summary) -> ScanSummary {
        let scan::Summary {
            run,
            files,
            checked,
            failed,
            cached,
            findings,
            timing,
            spent,
            paths,
        } = summary;
        ScanSummary {
            run,
            files: wide(files),
            checked: wide(checked),
            failed: wide(failed),
            cached: wide(cached),
            findings: wide(findings),
            timing: wide(timing),
            spent: spent.into(),
            paths: paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        }
    }
}

impl From<apply::Summary> for ApplySummary {
    fn from(summary: apply::Summary) -> ApplySummary {
        let apply::Summary {
            fixed,
            already,
            by_hand,
            not_found,
            remuxed,
            failed,
        } = summary;
        ApplySummary {
            fixed: wide(fixed),
            already: wide(already),
            by_hand: wide(by_hand),
            not_found: wide(not_found),
            remuxed: wide(remuxed),
            failed: wide(failed),
        }
    }
}

impl From<progress::Job> for Job {
    fn from(job: progress::Job) -> Job {
        match job {
            progress::Job::Scanning {
                run,
                adding,
                done,
                queued,
                failed,
                findings,
                under_way,
                spent,
            } => Job::Scanning {
                run,
                adding,
                done: wide(done),
                queued: wide(queued),
                failed: wide(failed),
                findings: wide(findings),
                under_way: under_way.into_iter().map(UnderWay::from).collect(),
                spent: spent.into(),
            },
            // The run is named again in the summary, which a scan that ended keeps.
            progress::Job::Scanned {
                run: _,
                adding,
                ended: Ok(summary),
            } => Job::ScanDone {
                adding,
                summary: summary.into(),
            },
            progress::Job::Scanned {
                run,
                adding,
                ended: Err(error),
            } => Job::ScanFailed { run, adding, error },
            progress::Job::Applying {
                dry_run,
                done,
                queued,
                at,
                so_far,
            } => Job::Applying {
                dry_run,
                done: wide(done),
                queued: wide(queued),
                at: at.into(),
                so_far: so_far.into(),
            },
            progress::Job::Applied {
                dry_run,
                ended: Ok(summary),
            } => Job::ApplyDone {
                dry_run,
                summary: summary.into(),
            },
            progress::Job::Applied {
                dry_run,
                ended: Err(error),
            } => Job::ApplyFailed { dry_run, error },
        }
    }
}

impl From<session::Event> for Event {
    fn from(event: session::Event) -> Event {
        match event {
            session::Event::Changed {
                rows,
                summary,
                detail,
                files,
                logs,
                folders,
            } => Event::Changed {
                rows: rows.map(Splice::from),
                summary: summary.into(),
                detail: detail.map(|now| DetailNow {
                    detail: now.map(Detail::from),
                }),
                files: files.map(FilesSplice::from),
                logs: logs.map(|now| LogsNow {
                    logs: now.map(FileLogs::from),
                }),
                folders: folders.map(|all| all.into_iter().map(FolderView::from).collect()),
            },
            session::Event::Answered { answered } => Event::Answered { answered },
            session::Event::Player(command) => Event::Player(command.into()),
            session::Event::Ended { fatal, outro } => Event::Ended { fatal, outro },
        }
    }
}

impl From<detail::Detail> for Detail {
    fn from(d: detail::Detail) -> Detail {
        let detail::Detail {
            file,
            label,
            group,
            at_ms,
            track,
            more,
            note,
            checking,
            truncated,
            state,
            edited,
            mark,
            original,
            wording,
            original_changed,
            wording_changed,
            reason,
            kind,
        } = d;
        Detail {
            file,
            label,
            group: group.map(Group::from),
            at_ms,
            track: track.map(Track::from),
            more: wide(more),
            note: note.map(FileNote::from),
            checking: checking.map(Checking::from),
            truncated: truncated.map(Truncated::from),
            state: state.into(),
            edited,
            mark: mark.map(Mark::from),
            original_changed: utf16(&original, &original_changed),
            wording_changed: utf16(&wording, &wording_changed),
            original,
            wording,
            reason,
            kind: kind.into(),
        }
    }
}

impl From<detail::Group> for Group {
    fn from(group: detail::Group) -> Group {
        let detail::Group { members, hidden } = group;
        Group {
            members: members.into_iter().map(Member::from).collect(),
            hidden: wide(hidden),
        }
    }
}

impl From<detail::Member> for Member {
    fn from(member: detail::Member) -> Member {
        let detail::Member { label, fix } = member;
        Member {
            label,
            fix: fix.map(Fix::from),
        }
    }
}

impl From<model::Fix> for Fix {
    fn from(fix: model::Fix) -> Fix {
        match fix {
            model::Fix::Shift(frames) => Fix::Shift { frames },
            model::Fix::Partial => Fix::Partial,
        }
    }
}

impl From<detail::Track> for Track {
    fn from(track: detail::Track) -> Track {
        let detail::Track {
            id,
            sid,
            name,
            language,
        } = track;
        Track {
            id: wide(id),
            sid: wide(sid),
            name,
            language,
        }
    }
}

impl From<mkv::Truncated> for Truncated {
    fn from(truncated: mkv::Truncated) -> Truncated {
        let mkv::Truncated {
            declared_end,
            file_size,
        } = truncated;
        Truncated {
            declared_end,
            file_size,
        }
    }
}

impl From<model::Marked> for Mark {
    fn from(marked: model::Marked) -> Mark {
        match marked {
            model::Marked::Said(model::Mark::ByHand(why)) => Mark::ByHand { why },
            model::Marked::Said(model::Mark::NotFound) => Mark::NotFound,
            model::Marked::Said(model::Mark::WouldApply) => Mark::WouldApply,
            model::Marked::Said(model::Mark::AlreadyApplied) => Mark::AlreadyApplied,
            model::Marked::Said(model::Mark::WouldBeByHand(why)) => Mark::WouldBeByHand { why },
            model::Marked::Said(model::Mark::WouldNotBeFound) => Mark::WouldNotBeFound,
            model::Marked::Said(model::Mark::Failed(why)) => Mark::Failed { why },
            model::Marked::Said(model::Mark::WouldFail(why)) => Mark::WouldFail { why },
            model::Marked::Applied => Mark::Applied,
        }
    }
}

impl From<detail::Kind> for Kind {
    fn from(kind: detail::Kind) -> Kind {
        match kind {
            detail::Kind::Line { before, after } => Kind::Line { before, after },
            detail::Kind::Timing {
                stretches,
                unshifted,
                opens,
            } => Kind::Timing {
                stretches: stretches.into_iter().map(Stretch::from).collect(),
                unshifted: unshifted.into_iter().map(Times::from).collect(),
                opens: opens.map(TimingMoment::from),
            },
        }
    }
}

impl From<detail::Stretch> for Stretch {
    fn from(stretch: detail::Stretch) -> Stretch {
        let detail::Stretch {
            frames,
            from_ms,
            to_ms,
        } = stretch;
        Stretch {
            frames,
            from_ms,
            to_ms,
        }
    }
}

impl From<detail::Times> for Times {
    fn from(times: detail::Times) -> Times {
        let detail::Times { start_ms, end_ms } = times;
        Times { start_ms, end_ms }
    }
}

impl From<list::TimingMoment> for TimingMoment {
    fn from(moment: list::TimingMoment) -> TimingMoment {
        let list::TimingMoment {
            at_ms,
            sid,
            on_frame,
        } = moment;
        TimingMoment {
            at_ms,
            sid: sid.map(wide),
            on_frame,
        }
    }
}

impl From<session::FileView> for FileView {
    fn from(view: session::FileView) -> FileView {
        let session::FileView {
            path,
            label,
            stage,
            applying,
            lines,
            timing_only,
        } = view;
        FileView {
            path,
            label,
            stage: stage.into(),
            applying: applying.map(FileApply::from),
            lines: lines.map(wide),
            timing_only,
        }
    }
}

impl From<progress::Stage> for ScanStage {
    fn from(stage: progress::Stage) -> ScanStage {
        match stage {
            progress::Stage::Queued => ScanStage::Queued,
            progress::Stage::QueuedAgain => ScanStage::QueuedAgain,
            progress::Stage::Reading => ScanStage::Reading,
            progress::Stage::Asking { answered, chunks } => ScanStage::Asking {
                answered: wide(answered),
                chunks: wide(chunks),
            },
            progress::Stage::Done { findings } => ScanStage::Done {
                findings: wide(findings),
            },
            progress::Stage::Failed(error) => ScanStage::Failed { error },
        }
    }
}

impl From<progress::ApplyStage> for FileApply {
    fn from(stage: progress::ApplyStage) -> FileApply {
        match stage {
            progress::ApplyStage::Queued => FileApply::Queued,
            progress::ApplyStage::At(stage) => FileApply::At {
                stage: stage.into(),
            },
            progress::ApplyStage::Done {
                dry_run,
                remuxed,
                counts,
            } => FileApply::Done {
                dry_run,
                remuxed,
                counts: counts.into(),
            },
            progress::ApplyStage::Failed(error) => FileApply::Failed { error },
        }
    }
}

impl From<progress::Counts> for ApplyCounts {
    fn from(counts: progress::Counts) -> ApplyCounts {
        let progress::Counts {
            applied,
            already,
            by_hand,
            not_found,
        } = counts;
        ApplyCounts {
            applied: wide(applied),
            already: wide(already),
            by_hand: wide(by_hand),
            not_found: wide(not_found),
        }
    }
}

impl From<session::Splice<session::FileView>> for FilesSplice {
    fn from(splice: session::Splice<session::FileView>) -> FilesSplice {
        let session::Splice {
            at,
            removed,
            inserted,
        } = splice;
        FilesSplice {
            at: wide(at),
            removed: wide(removed),
            inserted: inserted.into_iter().map(FileView::from).collect(),
        }
    }
}

impl From<session::FileLogs> for FileLogs {
    fn from(logs: session::FileLogs) -> FileLogs {
        let session::FileLogs { log, apply_log } = logs;
        FileLogs { log, apply_log }
    }
}

impl From<session::FolderView> for FolderView {
    fn from(folder: session::FolderView) -> FolderView {
        let session::FolderView {
            path,
            name,
            to_review,
            open,
        } = folder;
        FolderView {
            path,
            name,
            to_review: wide(to_review),
            open,
        }
    }
}

impl From<Action> for update::Action {
    fn from(action: Action) -> update::Action {
        match action {
            Action::Quit => update::Action::Quit,
            Action::Interrupt => update::Action::Interrupt,
            Action::Select { key } => update::Action::Select(key.into()),
            Action::Move { by } => update::Action::Move(by),
            Action::Toggle => update::Action::Toggle,
            Action::SetFilter { filter } => update::Action::SetFilter(filter.into()),
            Action::NextFile => update::Action::NextFile,
            Action::PreviousFile => update::Action::PreviousFile,
            Action::Accept => update::Action::Accept,
            Action::Reject => update::Action::Reject,
            Action::Clear => update::Action::Clear,
            Action::Undo => update::Action::Undo,
            Action::Edit => update::Action::Edit,
            Action::Reword { wording } => update::Action::Reword(wording),
            Action::CancelEdit => update::Action::CancelEdit,
            Action::ShowPane { pane } => update::Action::ShowPane(pane.into()),
            Action::SwitchView => update::Action::SwitchView,
            Action::OpenView { selector } => update::Action::OpenView(selector),
            Action::OpenFolder { folder } => update::Action::OpenFolder(folder),
            Action::Apply { dry_run } => update::Action::Apply { dry_run },
            Action::Play => update::Action::Play,
            Action::ClosePlayer => update::Action::ClosePlayer,
            Action::Playback { playback } => update::Action::Playback(playback.into()),
            Action::Answer { yes } => update::Action::Answer(yes),
            Action::SelectFile { path } => update::Action::SelectFile(path),
            Action::ShowFileFindings => update::Action::ShowFileFindings,
            Action::Recheck => update::Action::Recheck,
            Action::Dismiss => update::Action::Dismiss,
        }
    }
}

/// The other way, which nothing sends: it names every core action, so that one added to core
/// fails the build here until it crosses.
impl From<update::Action> for Action {
    fn from(action: update::Action) -> Action {
        match action {
            update::Action::Quit => Action::Quit,
            update::Action::Interrupt => Action::Interrupt,
            update::Action::Select(key) => Action::Select { key: key.into() },
            update::Action::Move(by) => Action::Move { by },
            update::Action::Toggle => Action::Toggle,
            update::Action::SetFilter(filter) => Action::SetFilter {
                filter: filter.into(),
            },
            update::Action::NextFile => Action::NextFile,
            update::Action::PreviousFile => Action::PreviousFile,
            update::Action::Accept => Action::Accept,
            update::Action::Reject => Action::Reject,
            update::Action::Clear => Action::Clear,
            update::Action::Undo => Action::Undo,
            update::Action::Edit => Action::Edit,
            update::Action::Reword(wording) => Action::Reword { wording },
            update::Action::CancelEdit => Action::CancelEdit,
            update::Action::ShowPane(pane) => Action::ShowPane { pane: pane.into() },
            update::Action::SwitchView => Action::SwitchView,
            update::Action::OpenView(selector) => Action::OpenView { selector },
            update::Action::OpenFolder(folder) => Action::OpenFolder { folder },
            update::Action::Apply { dry_run } => Action::Apply { dry_run },
            update::Action::Play => Action::Play,
            update::Action::ClosePlayer => Action::ClosePlayer,
            update::Action::Playback(playback) => Action::Playback {
                playback: playback.into(),
            },
            update::Action::Answer(yes) => Action::Answer { yes },
            update::Action::SelectFile(path) => Action::SelectFile { path },
            update::Action::ShowFileFindings => Action::ShowFileFindings,
            update::Action::Recheck => Action::Recheck,
            update::Action::Dismiss => Action::Dismiss,
        }
    }
}

impl From<PlayerError> for player::PlayerError {
    fn from(error: PlayerError) -> player::PlayerError {
        match error {
            PlayerError::NotOpen => player::PlayerError::NotOpen,
            PlayerError::NoFile => player::PlayerError::NoFile,
            PlayerError::StillOpening => player::PlayerError::StillOpening,
            PlayerError::CouldNotOpen { file } => player::PlayerError::CouldNotOpen { file },
            PlayerError::NoAnswer => player::PlayerError::NoAnswer,
            PlayerError::HungUp => player::PlayerError::HungUp,
            PlayerError::NoVideo => player::PlayerError::NoVideo,
            PlayerError::Mpv(error) => player::PlayerError::Mpv(error),
            PlayerError::Failed(error) => player::PlayerError::Failed(error),
        }
    }
}

impl From<PlayerReport> for session::PlayerReport {
    fn from(report: PlayerReport) -> session::PlayerReport {
        match report {
            PlayerReport::Opened { paused } => {
                session::PlayerReport::Opened(Ok(player::Opened { paused }))
            }
            PlayerReport::OpenFailed { error } => session::PlayerReport::Opened(Err(error.into())),
            PlayerReport::Played { state } => session::PlayerReport::Played(Ok(state.into())),
            PlayerReport::PlayFailed { error } => session::PlayerReport::Played(Err(error.into())),
            PlayerReport::Closed { error } => {
                session::PlayerReport::Closed(error.map_or(Ok(()), |e| Err(e.into())))
            }
            PlayerReport::Failed { error } => session::PlayerReport::Failed(error.into()),
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

    /// Each field a value of its own, so a conversion that crossed two would show. Characters past
    /// U+FFFF at the start of each line, two in the original and one in the wording, set the UTF-16
    /// code units of its parts apart from their count in characters, and a span counted along one
    /// line apart from the same span counted along the other.
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
            original: "𝄞𝄞 befor".into(),
            wording: "𝄞 before".into(),
            style: "OP en".into(),
            marks: vec![
                (model::MarkKind::WouldBeByHand, 4),
                (model::MarkKind::Applied, 1),
            ],
            track: Some(model::ListTrack {
                id: 6,
                name: Some("English Signs".into()),
                language: Some("en-GB".into()),
                apart: true,
            }),
            fixes: vec![(model::Fix::Shift(-3), 7), (model::Fix::Partial, 2)],
            checking: true,
            note: Some(model::FileNote::Missing),
            mixed: false,
            check: store::Check::Timing,
            original_changed: 3..8,
            wording_changed: 0..8,
            reason: "befor for before".into(),
            file: "/m/E01 字幕.mkv".into(),
            editable: false,
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
            original: "𝄞𝄞 befor".into(),
            wording: "𝄞 before".into(),
            style: "OP en".into(),
            marks: vec![
                MarkCount {
                    kind: MarkKind::WouldBeByHand,
                    findings: 4,
                },
                MarkCount {
                    kind: MarkKind::Applied,
                    findings: 1,
                },
            ],
            track: Some(ListTrack {
                id: 6,
                name: Some("English Signs".into()),
                language: Some("en-GB".into()),
                apart: true,
            }),
            fixes: vec![
                FixCount {
                    fix: Fix::Shift { frames: -3 },
                    findings: 7,
                },
                FixCount {
                    fix: Fix::Partial,
                    findings: 2,
                },
            ],
            checking: true,
            note: Some(FileNote::Missing),
            mixed: false,
            check: Check::Timing,
            original_changed: Span { start: 5, end: 10 },
            wording_changed: Span { start: 0, end: 9 },
            reason: "befor for before".into(),
            file: "/m/E01 字幕.mkv".into(),
            editable: false,
        }
    }

    /// What a scan's LLM calls used, each count its own.
    fn core_spent() -> llm::Spent {
        llm::Spent {
            calls: 7,
            unreported: 1,
            input_tokens: 9000,
            output_tokens: 300,
            priced: 6,
            cost_usd: 0.84,
        }
    }

    fn spent() -> Spent {
        Spent {
            calls: 7,
            unreported: 1,
            input_tokens: 9000,
            output_tokens: 300,
            priced: 6,
            cost_usd: 0.84,
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // every part of a change, each with values of its own
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
                status: Some(status::Status::Decided {
                    state: store::State::Rejected,
                    findings: 4,
                    replaced: 2,
                    kept: Some(status::Kept {
                        findings: 3,
                        state: store::State::Pending,
                    }),
                }),
                pending: 1,
                accepted: 2,
                rejected: 3,
                busy: true,
                stamping: true,
                open_follows: true,
                filter: list::Filter::Only(store::State::Accepted),
                pane: model::Pane::Files,
                edit: Some(session::Editing {
                    entry: model::EntryKey::Member(9),
                    wording: "My own, 字幕.".into(),
                }),
                question: Some(model::Question::Recheck {
                    label: "E02".into(),
                    backend: "claude".into(),
                }),
                job: Some(progress::Job::Scanning {
                    run: "r".into(),
                    adding: true,
                    done: 4,
                    queued: 24,
                    failed: 1,
                    findings: 31,
                    under_way: vec![progress::UnderWay {
                        file: "E05".into(),
                        stage: progress::Checking::Asking {
                            answered: 2,
                            chunks: 3,
                        },
                    }],
                    spent: core_spent(),
                }),
                file_selected: Some(6),
                spent: llm::Spent {
                    calls: 11,
                    unreported: 2,
                    input_tokens: 12_345,
                    output_tokens: 678,
                    priced: 9,
                    cost_usd: 1.5,
                },
                of_paths: false,
                answered: 12,
            },
            detail: Some(Some(core_detail())),
            files: Some(session::Splice {
                at: 2,
                removed: 3,
                inserted: vec![core_file()],
            }),
            logs: Some(Some(session::FileLogs {
                log: vec!["400 lines, 2 findings".into()],
                apply_log: vec!["E02: would apply 1".into()],
            })),
            folders: Some(vec![
                session::FolderView {
                    path: "/m/Yuru Camp".into(),
                    name: "Yuru Camp".into(),
                    to_review: 4,
                    open: true,
                },
                session::FolderView {
                    path: "/m/K-On!".into(),
                    name: "K-On! 字幕".into(),
                    to_review: 0,
                    open: false,
                },
            ]),
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
                status: Some(Status::Decided {
                    state: State::Rejected,
                    findings: 4,
                    replaced: 2,
                    kept: Some(Kept {
                        findings: 3,
                        state: State::Pending,
                    }),
                }),
                pending: 1,
                accepted: 2,
                rejected: 3,
                busy: true,
                stamping: true,
                open_follows: true,
                filter: Filter::Accepted,
                pane: Pane::Files,
                edit: Some(Editing {
                    entry: EntryKey::Member(9),
                    wording: "My own, 字幕.".into(),
                }),
                question: Some(Question::Recheck {
                    label: "E02".into(),
                    backend: "claude".into(),
                }),
                job: Some(Job::Scanning {
                    run: "r".into(),
                    adding: true,
                    done: 4,
                    queued: 24,
                    failed: 1,
                    findings: 31,
                    under_way: vec![UnderWay {
                        file: "E05".into(),
                        stage: Checking::Asking {
                            answered: 2,
                            chunks: 3,
                        },
                    }],
                    spent: spent(),
                }),
                file_selected: Some(6),
                spent: Spent {
                    calls: 11,
                    unreported: 2,
                    input_tokens: 12_345,
                    output_tokens: 678,
                    priced: 9,
                    cost_usd: 1.5,
                },
                of_paths: false,
                answered: 12,
            },
            detail: Some(DetailNow {
                detail: Some(detail()),
            }),
            files: Some(FilesSplice {
                at: 2,
                removed: 3,
                inserted: vec![file()],
            }),
            logs: Some(LogsNow {
                logs: Some(FileLogs {
                    log: vec!["400 lines, 2 findings".into()],
                    apply_log: vec!["E02: would apply 1".into()],
                }),
            }),
            folders: Some(vec![
                FolderView {
                    path: "/m/Yuru Camp".into(),
                    name: "Yuru Camp".into(),
                    to_review: 4,
                    open: true,
                },
                FolderView {
                    path: "/m/K-On!".into(),
                    name: "K-On! 字幕".into(),
                    to_review: 0,
                    open: false,
                },
            ]),
        };
        assert_eq!(Event::from(event), expected);
    }

    /// A line finding's detail, each field a value of its own; a character past U+FFFF before
    /// each part that changed sets its UTF-16 code units apart from its characters, and one in the
    /// original's part sets each range apart from the same range counted along the other string.
    fn core_detail() -> detail::Detail {
        detail::Detail {
            file: "/m/E02.mkv".into(),
            label: "E02".into(),
            group: Some(detail::Group {
                members: vec![
                    detail::Member {
                        label: "E01".into(),
                        fix: None,
                    },
                    detail::Member {
                        label: "E03".into(),
                        fix: Some(model::Fix::Shift(-2)),
                    },
                ],
                hidden: 4,
            }),
            at_ms: Some(165_060),
            track: Some(detail::Track {
                id: 5,
                sid: 1,
                name: Some("English Full".into()),
                language: "en-US".into(),
            }),
            more: 3,
            note: Some(model::FileNote::Changed),
            checking: Some(progress::Checking::Asking {
                answered: 1,
                chunks: 7,
            }),
            truncated: Some(mkv::Truncated {
                declared_end: 2000,
                file_size: 1000,
            }),
            state: store::State::Accepted,
            edited: true,
            mark: Some(model::Marked::Said(model::Mark::WouldBeByHand(
                "two fixes on one line".into(),
            ))),
            original: "🎵 The beat gets 𝄞trough".into(),
            wording: "🎵 The beat gets through it".into(),
            original_changed: 16..23,
            wording_changed: 16..26,
            reason: "misspelling of \"through\"".into(),
            kind: detail::Kind::Line {
                before: vec!["The line before.".into()],
                after: vec!["The line after.".into(), "And the next.".into()],
            },
        }
    }

    fn detail() -> Detail {
        Detail {
            file: "/m/E02.mkv".into(),
            label: "E02".into(),
            group: Some(Group {
                members: vec![
                    Member {
                        label: "E01".into(),
                        fix: None,
                    },
                    Member {
                        label: "E03".into(),
                        fix: Some(Fix::Shift { frames: -2 }),
                    },
                ],
                hidden: 4,
            }),
            at_ms: Some(165_060),
            track: Some(Track {
                id: 5,
                sid: 1,
                name: Some("English Full".into()),
                language: "en-US".into(),
            }),
            more: 3,
            note: Some(FileNote::Changed),
            checking: Some(Checking::Asking {
                answered: 1,
                chunks: 7,
            }),
            truncated: Some(Truncated {
                declared_end: 2000,
                file_size: 1000,
            }),
            state: State::Accepted,
            edited: true,
            mark: Some(Mark::WouldBeByHand {
                why: "two fixes on one line".into(),
            }),
            original: "🎵 The beat gets 𝄞trough".into(),
            wording: "🎵 The beat gets through it".into(),
            original_changed: Span { start: 17, end: 25 },
            wording_changed: Span { start: 17, end: 27 },
            reason: "misspelling of \"through\"".into(),
            kind: Kind::Line {
                before: vec!["The line before.".into()],
                after: vec!["The line after.".into(), "And the next.".into()],
            },
        }
    }

    /// A file of the files list, each field a value of its own.
    fn core_file() -> session::FileView {
        session::FileView {
            path: "/m/E02.mkv".into(),
            label: "E02 字幕".into(),
            stage: progress::Stage::Asking {
                answered: 2,
                chunks: 5,
            },
            applying: Some(progress::ApplyStage::At(apply::Stage::Check)),
            lines: Some(412),
            timing_only: true,
        }
    }

    fn file() -> FileView {
        FileView {
            path: "/m/E02.mkv".into(),
            label: "E02 字幕".into(),
            stage: ScanStage::Asking {
                answered: 2,
                chunks: 5,
            },
            applying: Some(FileApply::At {
                stage: ApplyStage::Check,
            }),
            lines: Some(412),
            timing_only: true,
        }
    }

    #[test]
    fn a_part_of_a_line_crosses_in_utf16_code_units() {
        // `ü` and `字` are one unit each, `𝄞` (U+1D11E) two.
        let s = "ü 字 𝄞 befor 𝄞";
        let chars = 6..11;
        assert_eq!(s.chars().skip(6).take(5).collect::<String>(), "befor");
        assert_eq!(utf16(s, &chars), Span { start: 7, end: 12 });
        assert_eq!(utf16(s, &(0..0)), Span { start: 0, end: 0 });
        assert_eq!(utf16(s, &(0..13)), Span { start: 0, end: 15 });
        // Empty, as where a wording only inserts: after one, and after both, past U+FFFF.
        assert_eq!(utf16(s, &(11..11)), Span { start: 12, end: 12 });
        assert_eq!(utf16(s, &(13..13)), Span { start: 15, end: 15 });
    }

    #[test]
    fn a_timing_findings_kind_crosses() {
        let core = detail::Kind::Timing {
            stretches: vec![
                detail::Stretch {
                    frames: 1,
                    from_ms: None,
                    to_ms: Some(475_780),
                },
                detail::Stretch {
                    frames: -2,
                    from_ms: Some(483_510),
                    to_ms: None,
                },
            ],
            unshifted: vec![detail::Times {
                start_ms: 1_300_000,
                end_ms: 1_302_000,
            }],
            opens: Some(list::TimingMoment {
                at_ms: 246_788,
                sid: Some(3),
                on_frame: true,
            }),
        };
        assert_eq!(
            Kind::from(core),
            Kind::Timing {
                stretches: vec![
                    Stretch {
                        frames: 1,
                        from_ms: None,
                        to_ms: Some(475_780),
                    },
                    Stretch {
                        frames: -2,
                        from_ms: Some(483_510),
                        to_ms: None,
                    },
                ],
                unshifted: vec![Times {
                    start_ms: 1_300_000,
                    end_ms: 1_302_000,
                }],
                opens: Some(TimingMoment {
                    at_ms: 246_788,
                    sid: Some(3),
                    on_frame: true,
                }),
            }
        );
        let off = list::TimingMoment {
            at_ms: 1000,
            sid: None,
            on_frame: false,
        };
        assert_eq!(
            TimingMoment::from(off),
            TimingMoment {
                at_ms: 1000,
                sid: None,
                on_frame: false,
            }
        );
        assert_eq!(Fix::from(model::Fix::Partial), Fix::Partial);
    }

    #[test]
    fn each_mark_crosses() {
        let said = |mark| model::Marked::Said(mark);
        for (core, ffi) in [
            (
                said(model::Mark::ByHand("a".into())),
                Mark::ByHand { why: "a".into() },
            ),
            (said(model::Mark::NotFound), Mark::NotFound),
            (said(model::Mark::WouldApply), Mark::WouldApply),
            (said(model::Mark::AlreadyApplied), Mark::AlreadyApplied),
            (
                said(model::Mark::WouldBeByHand("b".into())),
                Mark::WouldBeByHand { why: "b".into() },
            ),
            (said(model::Mark::WouldNotBeFound), Mark::WouldNotBeFound),
            (
                said(model::Mark::Failed("c".into())),
                Mark::Failed { why: "c".into() },
            ),
            (
                said(model::Mark::WouldFail("d".into())),
                Mark::WouldFail { why: "d".into() },
            ),
            (model::Marked::Applied, Mark::Applied),
        ] {
            assert_eq!(Mark::from(core), ffi);
        }
    }

    #[test]
    fn each_kind_of_mark_crosses() {
        for (core, ffi) in [
            (model::MarkKind::ByHand, MarkKind::ByHand),
            (model::MarkKind::NotFound, MarkKind::NotFound),
            (model::MarkKind::WouldApply, MarkKind::WouldApply),
            (model::MarkKind::AlreadyApplied, MarkKind::AlreadyApplied),
            (model::MarkKind::WouldBeByHand, MarkKind::WouldBeByHand),
            (model::MarkKind::WouldNotBeFound, MarkKind::WouldNotBeFound),
            (model::MarkKind::Failed, MarkKind::Failed),
            (model::MarkKind::WouldFail, MarkKind::WouldFail),
            (model::MarkKind::Applied, MarkKind::Applied),
        ] {
            assert_eq!(MarkKind::from(core), ffi);
        }
    }

    #[test]
    fn each_stage_of_a_file_crosses() {
        for (core, ffi) in [
            (progress::Stage::Queued, ScanStage::Queued),
            (progress::Stage::QueuedAgain, ScanStage::QueuedAgain),
            (progress::Stage::Reading, ScanStage::Reading),
            (
                progress::Stage::Asking {
                    answered: 3,
                    chunks: 4,
                },
                ScanStage::Asking {
                    answered: 3,
                    chunks: 4,
                },
            ),
            (
                progress::Stage::Done { findings: 9 },
                ScanStage::Done { findings: 9 },
            ),
            (
                progress::Stage::Failed("not a Matroska file".into()),
                ScanStage::Failed {
                    error: "not a Matroska file".into(),
                },
            ),
        ] {
            assert_eq!(ScanStage::from(core), ffi);
        }
        let counts = progress::Counts {
            applied: 1,
            already: 2,
            by_hand: 3,
            not_found: 4,
        };
        for (core, ffi) in [
            (progress::ApplyStage::Queued, FileApply::Queued),
            (
                progress::ApplyStage::Done {
                    dry_run: true,
                    remuxed: false,
                    counts,
                },
                FileApply::Done {
                    dry_run: true,
                    remuxed: false,
                    counts: ApplyCounts {
                        applied: 1,
                        already: 2,
                        by_hand: 3,
                        not_found: 4,
                    },
                },
            ),
            (
                progress::ApplyStage::Done {
                    dry_run: false,
                    remuxed: true,
                    counts: progress::Counts::default(),
                },
                FileApply::Done {
                    dry_run: false,
                    remuxed: true,
                    counts: ApplyCounts {
                        applied: 0,
                        already: 0,
                        by_hand: 0,
                        not_found: 0,
                    },
                },
            ),
            (
                progress::ApplyStage::Failed("mkvmerge failed".into()),
                FileApply::Failed {
                    error: "mkvmerge failed".into(),
                },
            ),
        ] {
            assert_eq!(FileApply::from(core), ffi);
        }
    }

    #[test]
    fn each_question_and_where_apply_is_cross() {
        let at = |stage| progress::ApplyAt::On {
            file: "E02".into(),
            stage,
        };
        let crossed = |stage| ApplyAt::On {
            file: "E02".into(),
            stage,
        };
        let stopping = |at| {
            model::Question::Quit(progress::Stopping::Apply {
                dry_run: true,
                done: 1,
                queued: 2,
                at,
            })
        };
        let stops = |at| Question::Quit {
            stopping: Stopping::Apply {
                dry_run: true,
                done: 1,
                queued: 2,
                at,
            },
        };
        for (core, ffi) in [
            (
                model::Question::Quit(progress::Stopping::Scan {
                    done: 3,
                    queued: 9,
                    again: 2,
                }),
                Question::Quit {
                    stopping: Stopping::Scan {
                        done: 3,
                        queued: 9,
                        again: 2,
                    },
                },
            ),
            (
                stopping(at(apply::Stage::Plan)),
                stops(crossed(ApplyStage::Plan)),
            ),
            (
                stopping(at(apply::Stage::Remux)),
                stops(crossed(ApplyStage::Remux)),
            ),
            (
                stopping(at(apply::Stage::Check)),
                stops(crossed(ApplyStage::Check)),
            ),
            (
                stopping(at(apply::Stage::Swap)),
                stops(crossed(ApplyStage::Swap)),
            ),
            (
                stopping(progress::ApplyAt::Before { file: "E03".into() }),
                stops(ApplyAt::Before { file: "E03".into() }),
            ),
            (
                stopping(progress::ApplyAt::Finishing),
                stops(ApplyAt::Finishing),
            ),
            (
                stopping(progress::ApplyAt::Starting),
                stops(ApplyAt::Starting),
            ),
            (
                model::Question::Apply {
                    fixes: 14,
                    files: 11,
                    applied: 3,
                },
                Question::Apply {
                    fixes: 14,
                    files: 11,
                    applied: 3,
                },
            ),
        ] {
            assert_eq!(Question::from(core), ffi);
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // each way a job ends, and the job under way, in turn
    fn each_job_crosses() {
        let so_far = apply::Summary {
            fixed: 1,
            already: 2,
            by_hand: 3,
            not_found: 4,
            remuxed: 5,
            failed: 6,
        };
        let crossed = ApplySummary {
            fixed: 1,
            already: 2,
            by_hand: 3,
            not_found: 4,
            remuxed: 5,
            failed: 6,
        };
        let summary = scan::Summary {
            run: "r".into(),
            files: 12,
            checked: 10,
            failed: 1,
            cached: 2,
            findings: 31,
            timing: 4,
            spent: core_spent(),
            paths: vec!["/m/K-On!".into(), "/m/E01 字幕.mkv".into()],
        };
        for (core, ffi) in [
            (
                progress::Job::Scanned {
                    run: "r".into(),
                    adding: false,
                    ended: Ok(summary),
                },
                Job::ScanDone {
                    adding: false,
                    summary: ScanSummary {
                        run: "r".into(),
                        files: 12,
                        checked: 10,
                        failed: 1,
                        cached: 2,
                        findings: 31,
                        timing: 4,
                        spent: spent(),
                        paths: vec!["/m/K-On!".into(), "/m/E01 字幕.mkv".into()],
                    },
                },
            ),
            (
                progress::Job::Scanned {
                    run: "r".into(),
                    adding: true,
                    ended: Err("the store is locked".into()),
                },
                Job::ScanFailed {
                    run: "r".into(),
                    adding: true,
                    error: "the store is locked".into(),
                },
            ),
            (
                progress::Job::Applying {
                    dry_run: false,
                    done: 3,
                    queued: 9,
                    at: progress::ApplyAt::Before { file: "E04".into() },
                    so_far,
                },
                Job::Applying {
                    dry_run: false,
                    done: 3,
                    queued: 9,
                    at: ApplyAt::Before { file: "E04".into() },
                    so_far: crossed,
                },
            ),
            (
                progress::Job::Applied {
                    dry_run: true,
                    ended: Ok(so_far),
                },
                Job::ApplyDone {
                    dry_run: true,
                    summary: crossed,
                },
            ),
            (
                progress::Job::Applied {
                    dry_run: false,
                    ended: Err("mkvmerge not found".into()),
                },
                Job::ApplyFailed {
                    dry_run: false,
                    error: "mkvmerge not found".into(),
                },
            ),
        ] {
            assert_eq!(Job::from(core), ffi);
        }
        for (core, ffi) in [
            (progress::Checking::Queued, Checking::Queued),
            (progress::Checking::Reading, Checking::Reading),
        ] {
            assert_eq!(Checking::from(core), ffi);
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // every part of a change, each with nothing set
    fn a_change_with_nothing_set_crosses_as_nothing() {
        let mut core = core_view();
        core.key = model::EntryKey::Member(4);
        core.state = None;
        core.marks = vec![];
        core.track = Some(model::ListTrack {
            id: 7,
            name: None,
            language: None,
            apart: false,
        });
        core.fixes = vec![];
        core.check = store::Check::Llm;
        core.original_changed = 0..0;
        core.wording_changed = 0..0;
        core.reason = String::new();
        core.file = String::new();
        let summary = session::Summary {
            title: String::new(),
            selected: None,
            status: None,
            pending: 0,
            accepted: 0,
            rejected: 0,
            busy: false,
            stamping: false,
            open_follows: false,
            filter: list::Filter::All,
            pane: model::Pane::Findings,
            edit: None,
            question: None,
            job: None,
            file_selected: None,
            spent: llm::Spent::default(),
            of_paths: false,
            answered: 0,
        };
        // Unchanged, and now none: the detail unchanged and the logs none, then the other way;
        // the folders unchanged, then none now.
        let change = |detail, logs, folders| {
            Event::from(session::Event::Changed {
                rows: None,
                summary: summary.clone(),
                detail,
                files: None,
                logs,
                folders,
            })
        };
        let Event::Changed {
            rows,
            summary,
            detail,
            files,
            logs,
            folders,
        } = change(None, Some(None), None)
        else {
            panic!("not a change");
        };
        assert_eq!(rows, None);
        assert_eq!(summary.file_selected, None);
        assert_eq!(
            (detail, files, logs, folders),
            (None, None, Some(LogsNow { logs: None }), None)
        );
        let Event::Changed {
            detail,
            logs,
            folders,
            ..
        } = change(Some(None), None, Some(vec![]))
        else {
            panic!("not a change");
        };
        assert_eq!(
            (detail, logs, folders),
            (Some(DetailNow { detail: None }), None, Some(vec![]))
        );
        assert_eq!(summary.spent, Spent::from(llm::Spent::default()));
        assert_eq!((summary.selected, summary.status), (None, None));
        assert_eq!(
            (summary.filter, summary.pane),
            (Filter::All, Pane::Findings)
        );
        assert_eq!(
            (summary.edit, summary.question, summary.job),
            (None, None, None)
        );
        core.checking = false;
        core.note = Some(model::FileNote::Changed);
        core.mixed = true;
        let crossed = EntryView::from(core);
        assert_eq!(crossed.key, EntryKey::Member(4));
        assert_eq!(
            (crossed.state, crossed.marks, crossed.fixes),
            (None, vec![], vec![])
        );
        assert_eq!(
            crossed.track,
            Some(ListTrack {
                id: 7,
                name: None,
                language: None,
                apart: false,
            })
        );
        assert_eq!(
            (crossed.checking, crossed.note, crossed.mixed),
            (false, Some(FileNote::Changed), true)
        );
        assert_eq!(
            (
                crossed.check,
                crossed.original_changed,
                crossed.wording_changed
            ),
            (
                Check::Llm,
                Span { start: 0, end: 0 },
                Span { start: 0, end: 0 }
            )
        );
        assert_eq!(
            (crossed.reason, crossed.file),
            (String::new(), String::new())
        );
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
        let open = |how, play| player::PlayerCommand::Open {
            moment: player::Moment {
                file: "/m/E01.mkv".into(),
                start_us: 165_064_500,
                sid: Some(2),
            },
            how,
            play,
            options: vec![
                player::FileOption {
                    name: "a".into(),
                    value: "1".into(),
                },
                player::FileOption {
                    name: "b".into(),
                    value: String::new(),
                },
            ],
        };
        let crossed = |how, play| {
            Event::Player(PlayerCommand::Open {
                moment: Moment {
                    file: "/m/E01.mkv".into(),
                    start_us: 165_064_500,
                    sid: Some(2),
                },
                how,
                play,
                options: vec![
                    FileOption {
                        name: "a".into(),
                        value: "1".into(),
                    },
                    FileOption {
                        name: "b".into(),
                        value: String::new(),
                    },
                ],
            })
        };
        let cases = [
            (
                session::Event::Answered { answered: 7 },
                Event::Answered { answered: 7 },
            ),
            (
                session::Event::Player(open(player::How::Seek, false)),
                crossed(How::Seek, false),
            ),
            (
                session::Event::Player(open(player::How::Seek, true)),
                crossed(How::Seek, true),
            ),
            (
                session::Event::Player(open(player::How::Load, false)),
                crossed(How::Load, false),
            ),
            (
                session::Event::Player(open(player::How::Load, true)),
                crossed(How::Load, true),
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
            start_us: -1,
            sid: None,
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

    /// Each of core's actions, with a value of its own where it carries one, and its mirror.
    fn every_action() -> Vec<(Action, update::Action)> {
        vec![
            (Action::Quit, update::Action::Quit),
            (Action::Interrupt, update::Action::Interrupt),
            (
                Action::Select {
                    key: EntryKey::Member(7),
                },
                update::Action::Select(model::EntryKey::Member(7)),
            ),
            (Action::Move { by: -3 }, update::Action::Move(-3)),
            (Action::Toggle, update::Action::Toggle),
            (
                Action::SetFilter {
                    filter: Filter::Rejected,
                },
                update::Action::SetFilter(list::Filter::Only(store::State::Rejected)),
            ),
            (Action::NextFile, update::Action::NextFile),
            (Action::PreviousFile, update::Action::PreviousFile),
            (Action::Accept, update::Action::Accept),
            (Action::Reject, update::Action::Reject),
            (Action::Clear, update::Action::Clear),
            (Action::Undo, update::Action::Undo),
            (Action::Edit, update::Action::Edit),
            (
                Action::Reword {
                    wording: "Fixed, 字幕.".into(),
                },
                update::Action::Reword("Fixed, 字幕.".into()),
            ),
            (Action::CancelEdit, update::Action::CancelEdit),
            (
                Action::ShowPane { pane: Pane::Files },
                update::Action::ShowPane(model::Pane::Files),
            ),
            (Action::SwitchView, update::Action::SwitchView),
            (
                Action::OpenView {
                    selector: vec!["r".into(), "/m/E01.mkv".into()],
                },
                update::Action::OpenView(vec!["r".into(), "/m/E01.mkv".into()]),
            ),
            (
                Action::OpenFolder {
                    folder: "/m/Yuru Camp".into(),
                },
                update::Action::OpenFolder("/m/Yuru Camp".into()),
            ),
            (
                Action::Apply { dry_run: true },
                update::Action::Apply { dry_run: true },
            ),
            (Action::Play, update::Action::Play),
            (Action::ClosePlayer, update::Action::ClosePlayer),
            (
                Action::Playback {
                    playback: Playback::Seek(5),
                },
                update::Action::Playback(player::Playback::Seek(5)),
            ),
            (Action::Answer { yes: true }, update::Action::Answer(true)),
            (
                Action::SelectFile {
                    path: "/m/E02.mkv".into(),
                },
                update::Action::SelectFile("/m/E02.mkv".into()),
            ),
            (Action::ShowFileFindings, update::Action::ShowFileFindings),
            (Action::Recheck, update::Action::Recheck),
            (Action::Dismiss, update::Action::Dismiss),
        ]
    }

    #[test]
    fn every_action_crosses_both_ways() {
        let cases = every_action();
        assert_eq!(cases.len(), 28, "core's actions");
        for (ffi, core) in cases {
            assert_eq!(update::Action::from(ffi.clone()), core);
            assert_eq!(Action::from(core), ffi);
        }
        // The other value of each action that takes a flag or a pane.
        for (ffi, core) in [
            (
                Action::Apply { dry_run: false },
                update::Action::Apply { dry_run: false },
            ),
            (Action::Answer { yes: false }, update::Action::Answer(false)),
            (
                Action::ShowPane {
                    pane: Pane::Findings,
                },
                update::Action::ShowPane(model::Pane::Findings),
            ),
        ] {
            assert_eq!(update::Action::from(ffi.clone()), core);
            assert_eq!(Action::from(core), ffi);
        }
    }

    #[test]
    fn each_filter_crosses_both_ways() {
        for (ffi, core) in [
            (Filter::All, list::Filter::All),
            (Filter::Pending, list::Filter::Only(store::State::Pending)),
            (Filter::Accepted, list::Filter::Only(store::State::Accepted)),
            (Filter::Rejected, list::Filter::Only(store::State::Rejected)),
        ] {
            assert_eq!(list::Filter::from(ffi), core);
            assert_eq!(Filter::from(core), ffi);
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
                PlayerReport::Opened { paused: true },
                session::PlayerReport::Opened(Ok(player::Opened { paused: true })),
            ),
            (
                PlayerReport::Opened { paused: false },
                session::PlayerReport::Opened(Ok(player::Opened { paused: false })),
            ),
            (
                PlayerReport::OpenFailed {
                    error: PlayerError::NoVideo,
                },
                session::PlayerReport::Opened(Err(player::PlayerError::NoVideo)),
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
                    error: PlayerError::StillOpening,
                },
                session::PlayerReport::Played(Err(player::PlayerError::StillOpening)),
            ),
            (
                PlayerReport::Closed { error: None },
                session::PlayerReport::Closed(Ok(())),
            ),
            (
                PlayerReport::Closed {
                    error: Some(PlayerError::NotOpen),
                },
                session::PlayerReport::Closed(Err(player::PlayerError::NotOpen)),
            ),
            (
                PlayerReport::Failed {
                    error: PlayerError::NoAnswer,
                },
                session::PlayerReport::Failed(player::PlayerError::NoAnswer),
            ),
        ];
        for (ffi, core) in cases {
            assert_eq!(session::PlayerReport::from(ffi), core);
        }
    }

    // Each message, each reason and each failure, with values unlike each other within each.
    #[test]
    #[allow(clippy::too_many_lines)] // one pair per variant
    fn each_message_crosses() {
        let core = |status: status::Status| Status::from(status);
        let label = |s: &str| s.to_string();
        for (from, to) in [
            (
                status::Status::Decided {
                    state: store::State::Accepted,
                    findings: 9,
                    replaced: 8,
                    kept: Some(status::Kept {
                        findings: 4,
                        state: store::State::Rejected,
                    }),
                },
                Status::Decided {
                    state: State::Accepted,
                    findings: 9,
                    replaced: 8,
                    kept: Some(Kept {
                        findings: 4,
                        state: State::Rejected,
                    }),
                },
            ),
            (
                status::Status::Decided {
                    state: store::State::Rejected,
                    findings: 1,
                    replaced: 0,
                    kept: None,
                },
                Status::Decided {
                    state: State::Rejected,
                    findings: 1,
                    replaced: 0,
                    kept: None,
                },
            ),
            (
                status::Status::Cleared { findings: 7 },
                Status::Cleared { findings: 7 },
            ),
            (
                status::Status::Undone {
                    findings: 5,
                    labels: vec![label("E01"), label("E02")],
                },
                Status::Undone {
                    findings: 5,
                    labels: vec![label("E01"), label("E02")],
                },
            ),
            (
                status::Status::Inherited {
                    label: label("E03"),
                    findings: 2,
                    state: store::State::Rejected,
                },
                Status::Inherited {
                    label: label("E03"),
                    findings: 2,
                    state: State::Rejected,
                },
            ),
            (
                status::Status::Replaced {
                    label: label("E04"),
                    gone: true,
                },
                Status::Replaced {
                    label: label("E04"),
                    gone: true,
                },
            ),
            (
                status::Status::Replaced {
                    label: label("E16"),
                    gone: false,
                },
                Status::Replaced {
                    label: label("E16"),
                    gone: false,
                },
            ),
            (
                status::Status::EditEnded {
                    label: label("E05"),
                },
                Status::EditEnded {
                    label: label("E05"),
                },
            ),
            (
                status::Status::QueuedAgain {
                    label: label("E06"),
                },
                Status::QueuedAgain {
                    label: label("E06"),
                },
            ),
            (
                status::Status::NoSuchFile {
                    label: label("E07"),
                },
                Status::NoSuchFile {
                    label: label("E07"),
                },
            ),
            (status::Status::OpeningView, Status::OpeningView),
            (
                status::Status::Opening {
                    label: label("E08"),
                    at_ms: 165_060,
                    play: false,
                },
                Status::Opening {
                    label: label("E08"),
                    at_ms: 165_060,
                    play: false,
                },
            ),
            (
                status::Status::Opening {
                    label: label("E08"),
                    at_ms: 165_060,
                    play: true,
                },
                Status::Opening {
                    label: label("E08"),
                    at_ms: 165_060,
                    play: true,
                },
            ),
            (
                status::Status::NotAnswering {
                    label: label("E10"),
                },
                Status::NotAnswering {
                    label: label("E10"),
                },
            ),
            (
                status::Status::StoppedWaiting {
                    label: label("E11"),
                },
                Status::StoppedWaiting {
                    label: label("E11"),
                },
            ),
            (
                status::Status::Opened { paused: true },
                Status::Opened { paused: true },
            ),
            (
                status::Status::Opened { paused: false },
                Status::Opened { paused: false },
            ),
            (
                status::Status::Played(player::PlayState {
                    paused: true,
                    at_ms: 171_060,
                }),
                Status::Played {
                    state: PlayState {
                        paused: true,
                        at_ms: 171_060,
                    },
                },
            ),
            (
                status::Status::Played(player::PlayState {
                    paused: false,
                    at_ms: 3_723_004,
                }),
                Status::Played {
                    state: PlayState {
                        paused: false,
                        at_ms: 3_723_004,
                    },
                },
            ),
            (status::Status::Closed, Status::Closed),
            (status::Status::NoFile, Status::NoFile),
            (status::Status::StillOpening, Status::StillOpening),
            (
                status::Status::CouldNotOpen {
                    label: label("E09"),
                },
                Status::CouldNotOpen {
                    label: label("E09"),
                },
            ),
            (status::Status::NoAnswer, Status::NoAnswer),
            (status::Status::HungUp, Status::HungUp),
            (status::Status::NoVideo, Status::NoVideo),
            (
                status::Status::Warned {
                    first: label("warning: /m/locked: skipped"),
                    more: 6,
                },
                Status::Warned {
                    first: label("warning: /m/locked: skipped"),
                    more: 6,
                },
            ),
        ] {
            assert_eq!(core(from), to);
        }
        for (from, to) in [
            (status::Asked::Open, Asked::Open),
            (status::Asked::Playback, Asked::Playback),
            (status::Asked::Close, Asked::Close),
        ] {
            assert_eq!(
                core(status::Status::NotOpen { asked: from }),
                Status::NotOpen { asked: to }
            );
        }
        for (from, to) in [
            (status::Refusal::SelectionStays, Refusal::SelectionStays),
            (
                status::Refusal::NotInView {
                    label: label("E10"),
                },
                Refusal::NotInView {
                    label: label("E10"),
                },
            ),
            (status::Refusal::TimingFix, Refusal::TimingFix),
            (status::Refusal::EmptyWording, Refusal::EmptyWording),
            (status::Refusal::EditOutOfDate, Refusal::EditOutOfDate),
            (
                status::Refusal::Already {
                    state: store::State::Accepted,
                },
                Refusal::Already {
                    state: State::Accepted,
                },
            ),
            (
                status::Refusal::Already {
                    state: store::State::Rejected,
                },
                Refusal::Already {
                    state: State::Rejected,
                },
            ),
            (status::Refusal::NothingToClear, Refusal::NothingToClear),
            (status::Refusal::NothingToUndo, Refusal::NothingToUndo),
            (status::Refusal::LastFile, Refusal::LastFile),
            (status::Refusal::FirstFile, Refusal::FirstFile),
            (
                status::Refusal::FileMissing {
                    path: label("/m/s1/E11.mkv"),
                },
                Refusal::FileMissing {
                    path: label("/m/s1/E11.mkv"),
                },
            ),
            (status::Refusal::NothingToOpen, Refusal::NothingToOpen),
            (status::Refusal::ScanRunning, Refusal::ScanRunning),
            (status::Refusal::ApplyRunning, Refusal::ApplyRunning),
            (status::Refusal::AlreadyApplying, Refusal::AlreadyApplying),
            (status::Refusal::NoAcceptedFixes, Refusal::NoAcceptedFixes),
            (
                status::Refusal::BeingChecked {
                    label: label("E12"),
                },
                Refusal::BeingChecked {
                    label: label("E12"),
                },
            ),
            (
                status::Refusal::NoFindingsShown {
                    label: label("E13"),
                },
                Refusal::NoFindingsShown {
                    label: label("E13"),
                },
            ),
        ] {
            assert_eq!(
                core(status::Status::Refused(from)),
                Status::Refused { why: to }
            );
        }
        for (from, to) in [
            (status::Failure::Save, Failure::Save),
            (status::Failure::Switch, Failure::Switch),
            (
                status::Failure::ReadBack {
                    label: label("E14"),
                },
                Failure::ReadBack {
                    label: label("E14"),
                },
            ),
            (status::Failure::Recheck, Failure::Recheck),
            (status::Failure::Mpv, Failure::Mpv),
            (status::Failure::Player, Failure::Player),
        ] {
            assert_eq!(
                core(status::Status::Failed {
                    what: from,
                    error: label("database is locked"),
                }),
                Status::Failed {
                    what: to,
                    error: label("database is locked"),
                }
            );
        }
    }

    #[test]
    fn each_player_error_is_cores() {
        for (ffi, core) in [
            (PlayerError::NotOpen, player::PlayerError::NotOpen),
            (PlayerError::NoFile, player::PlayerError::NoFile),
            (PlayerError::StillOpening, player::PlayerError::StillOpening),
            (
                PlayerError::CouldNotOpen {
                    file: "/m/E01.mkv".into(),
                },
                player::PlayerError::CouldNotOpen {
                    file: "/m/E01.mkv".into(),
                },
            ),
            (PlayerError::NoAnswer, player::PlayerError::NoAnswer),
            (PlayerError::HungUp, player::PlayerError::HungUp),
            (PlayerError::NoVideo, player::PlayerError::NoVideo),
            (
                PlayerError::Mpv("invalid parameter".into()),
                player::PlayerError::Mpv("invalid parameter".into()),
            ),
            (
                PlayerError::Failed("mpv not found".into()),
                player::PlayerError::Failed("mpv not found".into()),
            ),
        ] {
            assert_eq!(player::PlayerError::from(ffi), core);
        }
    }

    const CHILD: &str = "tests::child_that_opens_a_session";

    /// How long the child may take: a session that hangs fails the test, and does not hang the run.
    const LIMIT: Duration = Duration::from_secs(30);

    /// One of review's sample paths, which are absolute, under `dir` instead of the root.
    fn under(dir: &Path, path: &str) -> String {
        dir.join(path.trim_start_matches('/'))
            .to_string_lossy()
            .into_owned()
    }

    /// A database in `dir` holding review's sample run (four findings in two rows, the first of
    /// three members, in files under `dir` not on disk) and a fifth finding, accepted, in a FIFO
    /// under `dir` that nothing writes to, where a dry run waits for good; and the run's name.
    /// Review's rows are ordered by their files' paths (`review::model::group`), so these paths
    /// alone fix the rows' order: the FIFO's folder sorts before the sample's (`m`), and its row
    /// is first wherever `dir` is.
    fn seeded(dir: &Path) -> String {
        let held = dir.join("fifo");
        std::fs::create_dir(&held).unwrap();
        let fifo = held.join("stalled.mkv");
        let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
        assert!(made.success());
        let stalled = fifo.to_string_lossy().into_owned();
        let mut sample = tests_support::run();
        for file in &mut sample.files {
            file.path = under(dir, &file.path);
        }
        for finding in &mut sample.findings {
            finding.file = under(dir, &finding.file);
        }
        sample.files.push(store::FileRecord {
            path: stalled.clone(),
            ..tests_support::file(4)
        });
        sample.findings.push(store::Finding {
            file: stalled,
            ..tests_support::finding(5, 4, "A line no one reads.", "A line nobody reads.", 1000)
        });
        let mut store = Store::open_at(&dir.join("subcheck.db")).unwrap();
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
        let accepted = store::Decision {
            finding: 5,
            state: store::State::Accepted,
            edited: None,
            at: jiff::Timestamp::UNIX_EPOCH,
        };
        store.decide(&[accepted]).unwrap();
        scan.name
    }

    /// What a UI keeps of a session: each list, each splice applied to it as it comes, the last
    /// summary, the detail, the logs and the folders as last sent, and the player's commands.
    #[derive(Default)]
    struct Ui {
        rows: Vec<EntryView>,
        files: Vec<FileView>,
        summary: Option<Summary>,
        detail: Option<Detail>,
        logs: Option<FileLogs>,
        folders: Vec<FolderView>,
        played: Vec<PlayerCommand>,
    }

    impl Ui {
        /// Take events until `done` holds, after `what`; none may be the end.
        fn until(&mut self, s: &Session, what: &str, done: impl Fn(&Ui) -> bool) {
            let deadline = Instant::now() + LIMIT;
            while !done(self) {
                assert!(Instant::now() < deadline, "never came, after {what}");
                match s.next_event() {
                    Some(Event::Changed {
                        rows,
                        summary,
                        detail,
                        files,
                        logs,
                        folders,
                    }) => {
                        let span = |at: u64, removed: u64| {
                            let at = usize::try_from(at).unwrap();
                            at..at + usize::try_from(removed).unwrap()
                        };
                        if let Some(s) = rows {
                            self.rows.splice(span(s.at, s.removed), s.inserted);
                        }
                        if let Some(s) = files {
                            self.files.splice(span(s.at, s.removed), s.inserted);
                        }
                        if let Some(now) = detail {
                            self.detail = now.detail;
                        }
                        if let Some(now) = logs {
                            self.logs = now.logs;
                        }
                        if let Some(now) = folders {
                            self.folders = now;
                        }
                        self.summary = Some(summary);
                    }
                    Some(Event::Answered { answered }) => {
                        self.summary.as_mut().unwrap().answered = answered;
                    }
                    Some(Event::Player(command)) => self.played.push(command),
                    other => panic!("after {what}: {other:?}"),
                }
            }
        }

        fn summary(&self) -> &Summary {
            self.summary.as_ref().unwrap()
        }

        /// Each folder held: its name, how many to review, and whether it is open.
        fn folders(&self) -> Vec<(&str, u64, bool)> {
            self.folders
                .iter()
                .map(|f| (f.name.as_str(), f.to_review, f.open))
                .collect()
        }

        /// The selected entry, as the rows held say it.
        fn selected(&self) -> &EntryView {
            &self.rows[usize::try_from(self.summary().selected.unwrap()).unwrap()]
        }
    }

    /// Run by `a_session_crosses_and_comes_back`. `Session::open` finds the store through the
    /// environment, which `Command::env` sets for the child alone: set in the test process, it
    /// would reach every test running beside this one. Every action crosses, each with what it
    /// changes, and each splice is applied to the rows held, as a UI applies them.
    #[test]
    #[ignore = "the child of the round trip"]
    #[allow(clippy::too_many_lines)] // every action in turn, each with what it changes
    fn child_that_opens_a_session() {
        let Some(dir) = std::env::var_os("SUBCHECK_FFI_TEST_DIR") else {
            return;
        };
        let name = std::env::var("SUBCHECK_FFI_TEST_RUN").unwrap();
        // A second session in one process, which `Session`'s doc rules out, is safe here: it stops
        // no job, so it ends no groups. What was queued before `close` may still come out, then
        // `close`'s end, with nothing to say of a job, and then nothing. First, as the session
        // below stops one.
        let closed = Session::open(vec![name.clone()], false);
        closed.close();
        let mut last = None;
        while let Some(event) = closed.next_event() {
            last = Some(event);
        }
        let quiet = Event::Ended {
            fatal: None,
            outro: vec![],
        };
        assert_eq!(last, Some(quiet));
        assert_eq!(closed.next_event(), None);

        let s = Session::open(vec![name.clone()], true);
        let mut ui = Ui::default();
        ui.until(&s, "opening", |ui| ui.summary.is_some());
        let keys: Vec<EntryKey> = ui.rows.iter().map(|v| v.key.clone()).collect();
        assert_eq!(
            keys,
            [
                EntryKey::Row(vec![5]),
                EntryKey::Row(vec![1, 2, 4]),
                EntryKey::Row(vec![3])
            ]
        );
        assert_eq!(ui.summary().title, format!("run {name}"));
        assert!(!ui.summary().of_paths);
        assert_eq!((ui.summary().selected, ui.summary().pending), (Some(1), 4));
        // The rows come before their files are checked, the notes once they are: the FIFO is on
        // disk, but not as it was scanned; the sample's files are not.
        assert_eq!((ui.rows[0].note, ui.rows[1].note), (None, None));
        ui.until(&s, "the notes", |ui| ui.rows[1].note.is_some());
        assert_eq!(ui.rows[0].note, Some(FileNote::Changed));
        assert_eq!(ui.rows[1].note, Some(FileNote::Missing));
        // The files in the order the run named them, the first selected; the selected row's
        // detail, a row of three.
        let labels =
            |ui: &Ui| -> Vec<String> { ui.files.iter().map(|f| f.label.clone()).collect() };
        assert_eq!(labels(&ui), ["E01", "E02", "E03", "stalled"]);
        assert_eq!(ui.summary().file_selected, Some(0));
        assert_eq!(ui.logs.as_ref().unwrap().log[0], "1 lines, 1 finding");
        let detail = ui.detail.clone().unwrap();
        assert_eq!(
            (detail.label.as_str(), detail.state),
            ("E01", State::Pending)
        );
        assert_eq!(detail.group.map(|g| g.members.len()), Some(3));
        // The run's folders, in the order it named their files; a run's view is no folder's.
        assert_eq!(ui.folders(), [("K-On!", 4, false), ("fifo", 0, false)]);
        let fifo = Path::new(&dir).join("fifo").to_string_lossy().into_owned();
        assert_eq!(ui.folders[1].path, fifo);

        s.send(Action::Toggle);
        ui.until(&s, "Toggle", |ui| ui.rows.len() == 6);
        s.send(Action::Move { by: 2 });
        ui.until(&s, "Move", |ui| ui.summary().selected == Some(3));
        s.send(Action::Select {
            key: EntryKey::Member(4),
        });
        ui.until(&s, "Select", |ui| ui.summary().selected == Some(4));
        assert_eq!(ui.detail.as_ref().map(|d| d.label.as_str()), Some("E03"));
        s.send(Action::Accept);
        ui.until(&s, "Accept", |ui| ui.summary().accepted == 2);
        assert!(
            ui.rows[1].mixed,
            "the row's members are decided differently"
        );
        // Read again once the decision is written, in the change that shows it.
        assert_eq!(ui.folders(), [("K-On!", 3, false), ("fifo", 0, false)]);
        s.send(Action::Reject);
        ui.until(&s, "Reject", |ui| ui.summary().rejected == 1);
        s.send(Action::Undo);
        ui.until(&s, "Undo", |ui| ui.summary().rejected == 0);
        s.send(Action::Select {
            key: EntryKey::Member(4),
        });
        ui.until(&s, "Select", |ui| ui.summary().selected == Some(4));
        s.send(Action::Clear);
        ui.until(&s, "Clear", |ui| ui.summary().accepted == 1);
        s.send(Action::SetFilter {
            filter: Filter::Accepted,
        });
        ui.until(&s, "SetFilter", |ui| ui.rows.len() == 1);
        assert_eq!(ui.summary().filter, Filter::Accepted);
        s.send(Action::SetFilter {
            filter: Filter::All,
        });
        ui.until(&s, "SetFilter", |ui| ui.rows.len() == 6);
        s.send(Action::NextFile);
        ui.until(&s, "NextFile", |ui| ui.selected().label == "E01");
        s.send(Action::PreviousFile);
        ui.until(&s, "PreviousFile", |ui| ui.selected().label == "stalled");
        s.send(Action::Edit);
        ui.until(&s, "Edit", |ui| ui.summary().edit.is_some());
        s.send(Action::CancelEdit);
        ui.until(&s, "CancelEdit", |ui| ui.summary().edit.is_none());
        s.send(Action::Edit);
        ui.until(&s, "Edit", |ui| ui.summary().edit.is_some());
        let Some(Editing { entry, wording }) = ui.summary().edit.clone() else {
            unreachable!();
        };
        assert_eq!(
            (&entry, &wording),
            (&ui.selected().key, &ui.selected().wording)
        );
        s.send(Action::Reword {
            wording: "Fixed, by hand.".into(),
        });
        ui.until(&s, "Reword", |ui| ui.rows[0].wording == "Fixed, by hand.");
        assert_eq!(ui.summary().edit, None);

        s.send(Action::ShowPane { pane: Pane::Files });
        ui.until(&s, "ShowPane", |ui| ui.summary().pane == Pane::Files);
        s.send(Action::SelectFile {
            path: under(Path::new(&dir), &tests_support::path(2)),
        });
        s.send(Action::Recheck);
        ui.until(&s, "Recheck", |ui| ui.summary().question.is_some());
        assert_eq!(ui.summary().file_selected, Some(1));
        assert_eq!(ui.logs.as_ref().unwrap().log[0], "1 lines, 2 findings");
        assert_eq!(
            ui.summary().question,
            Some(Question::Recheck {
                label: "E02".into(),
                backend: "claude".into(),
            })
        );
        s.send(Action::Answer { yes: false });
        ui.until(&s, "Answer", |ui| ui.summary().question.is_none());
        s.send(Action::ShowFileFindings);
        ui.until(&s, "ShowFileFindings", |ui| {
            ui.summary().pane == Pane::Findings && ui.summary().selected == Some(1)
        });

        // The run's folders, then the run again.
        s.send(Action::SwitchView);
        ui.until(&s, "SwitchView", |ui| {
            !ui.summary().title.starts_with("run ")
        });
        // A paths view lists its files by path: the FIFO's folder first.
        assert_eq!(labels(&ui), ["stalled", "E01", "E02", "E03"]);
        assert_eq!(ui.folders(), [("K-On!", 4, true), ("fifo", 0, true)]);
        assert!(ui.summary().of_paths);
        s.send(Action::OpenFolder {
            folder: fifo.clone(),
        });
        ui.until(&s, "OpenFolder", |ui| ui.summary().title == "fifo");
        assert!(ui.summary().of_paths);
        assert_eq!(labels(&ui), ["stalled"]);
        assert_eq!(ui.folders(), [("K-On!", 4, false), ("fifo", 0, true)]);
        s.send(Action::OpenView {
            selector: vec![name.clone()],
        });
        ui.until(&s, "OpenView", |ui| {
            ui.summary().title == format!("run {name}")
        });
        assert!(!ui.summary().of_paths);

        // The FIFO's finding opens: it is on disk.
        s.send(Action::Select {
            key: EntryKey::Row(vec![5]),
        });
        ui.until(&s, "Select", |ui| {
            ui.selected().key == EntryKey::Row(vec![5])
        });
        s.send(Action::Play);
        ui.until(&s, "Play", |ui| !ui.played.is_empty());
        s.send(Action::ClosePlayer);
        s.send(Action::Playback {
            playback: Playback::Pause,
        });
        ui.until(&s, "ClosePlayer and Playback", |ui| ui.played.len() == 3);
        assert!(matches!(
            ui.played.as_slice(),
            [
                PlayerCommand::Open { how: How::Load, .. },
                PlayerCommand::Close,
                PlayerCommand::Playback(Playback::Pause)
            ]
        ));
        s.report(PlayerReport::Played {
            state: PlayState {
                paused: true,
                at_ms: 171_060,
            },
        });
        ui.until(&s, "Played", |ui| {
            ui.summary().status
                == Some(Status::Played {
                    state: PlayState {
                        paused: true,
                        at_ms: 171_060,
                    },
                })
        });
        s.report(PlayerReport::PlayFailed {
            error: PlayerError::NoFile,
        });
        ui.until(&s, "PlayFailed", |ui| {
            ui.summary().status == Some(Status::NoFile)
        });
        s.send(Action::Dismiss);
        ui.until(&s, "Dismiss", |ui| ui.summary().status.is_none());

        // A dry run of the one accepted fix, which waits on the FIFO.
        s.send(Action::Apply { dry_run: true });
        ui.until(&s, "Apply", |ui| {
            matches!(
                &ui.summary().job,
                Some(Job::Applying {
                    at: ApplyAt::On {
                        stage: ApplyStage::Plan,
                        file,
                    },
                    ..
                }) if file == "stalled"
            )
        });
        assert_eq!(
            ui.files[3].applying,
            Some(FileApply::At {
                stage: ApplyStage::Plan,
            })
        );
        s.send(Action::Interrupt);
        ui.until(&s, "Interrupt", |ui| ui.summary().question.is_some());
        assert!(matches!(
            ui.summary().question,
            Some(Question::Quit {
                stopping: Stopping::Apply { dry_run: true, .. }
            })
        ));
        // Under the question, `Quit` only lets it go; a second `Interrupt` with it open quits.
        s.send(Action::Quit);
        ui.until(&s, "Quit", |ui| ui.summary().question.is_none());
        s.send(Action::Interrupt);
        ui.until(&s, "Interrupt", |ui| ui.summary().question.is_some());
        s.send(Action::Interrupt);
        let mut end = None;
        while let Some(event) = s.next_event() {
            end = Some(event);
        }
        // Opened with karaoke, so the command that finishes the dry run asks for it too.
        let Some(Event::Ended { fatal: None, outro }) = end else {
            panic!("{end:?}");
        };
        assert_eq!(
            outro.last(),
            Some(&format!(
                "Finish it with: subcheck apply {name} --dry-run --karaoke"
            ))
        );
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
