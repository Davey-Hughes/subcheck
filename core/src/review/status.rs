//! What review says on the status line, as data each UI words for itself ([`Review::status`]):
//! what an action, a task or the player did, or why an action did nothing. Files go by the names
//! the list gives them, but where the path itself is the point; an error a task returns keeps its
//! own words, the store's, a job's, mpv's or the OS's.
//!
//! [`Review::status`]: crate::review::model::Review::status

use crate::player::PlayState;
use crate::store::State;

/// The status message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// `Accept`, `Reject` or `Reword` put `findings` findings at `state`: of them `replaced` had
    /// the user's wording, which `Accept`'s suggestion replaced (`Undo` brings it back); `kept`,
    /// the row's members the decision left as they were: decided otherwise, or, for an edit,
    /// joined the row after it opened.
    Decided {
        state: State,
        findings: usize,
        replaced: usize,
        kept: Option<Kept>,
    },
    /// `Clear` put `findings` findings back to pending.
    Cleared { findings: usize },
    /// `Undo` put `findings` findings back as they were, in the files `labels` names, each once,
    /// in the order of the findings.
    Undone {
        findings: usize,
        labels: Vec<String>,
    },
    /// `findings` new findings of file `label` took the decision of their rows, `state` the
    /// first's (`Undo` takes them back).
    Inherited {
        label: String,
        findings: usize,
        state: State,
    },
    /// A decision found its finding replaced by a check of file `label` since: it went to the new
    /// finding, or with none (`gone`) nowhere.
    Replaced { label: String, gone: bool },
    /// A check of file `label` left the edit with none of its findings, and ended it.
    EditEnded { label: String },
    /// The action did nothing, for this reason.
    Refused(Refusal),
    /// File `label` is queued to be checked again once the scan ends.
    QueuedAgain { label: String },
    /// A check again left out file `label`, the first of those no longer on disk.
    NoSuchFile { label: String },
    /// `Play` asks the player to open file `label` at `at_ms`.
    Opening { label: String, at_ms: i64 },
    /// The player opened it, and left mpv paused or playing.
    Opened { paused: bool },
    /// Where playback is after a `Playback`.
    Played(PlayState),
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
    /// The window's video pane could not show video in the player's time.
    NoVideo,
    /// What failed, and its error in the words of whoever made it.
    Failed { what: Failure, error: String },
    /// What the scan warned of as it planned, before the screen opened: the first warning, and
    /// how many more.
    Warned { first: String, more: usize },
}

/// The members a row's decision left as they were: how many, and the decision of the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kept {
    pub findings: usize,
    pub state: State,
}

/// Why an action did nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// `Select` named an entry whose findings went, or that the list no longer shows.
    SelectionStays,
    /// `SelectFile` named file `label`, which the files view does not have.
    NotInView { label: String },
    /// `Edit` on a timing fix, which is accepted or rejected, never reworded.
    TimingFix,
    /// `Reword` with no text: the edit goes on (`CancelEdit` ends it).
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

/// What failed, for [`Status::Failed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Writing the decisions: none was saved.
    Save,
    /// Reading another view (`SwitchView`, `OpenView`, `OpenFolder`): the view stays.
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

/// What the player was asked, for [`Status::NotOpen`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    Open,
    Playback,
    Close,
}
