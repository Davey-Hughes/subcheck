//! What each action, and each result of a job or an effect, does to review's state, and the
//! effects the event loop runs for it. Pure: the store and mpv are touched only by the effects, so
//! every rule here is tested without a terminal. It takes actions, not keys or clicks, so that
//! each UI maps its own to the same rules: the terminal's are in `subcheck_cli::tui`, its `keymap`
//! and `screen`.

use std::collections::HashMap;

use jiff::Timestamp;

use super::model::{Confirm, Edit, Entry, EntryKey, FileNote, Mark, Pane, Review, UndoStep};
use super::progress::{Scan, Stage};
use super::status::{Asked, Failure, Kept, Refusal, Status};
use crate::apply;
use crate::list::{self, Filter};
use crate::player::{self, How, Moment, Opened, PlayState, Playback, PlayerCommand, PlayerError};
use crate::scan;
use crate::store::{Decided, Decision, Record, Run, State};

/// What `update` takes: an action, or what a job or an effect has to say.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // a read-back is rare; boxing it would only add noise
pub enum Input {
    /// What the user asks for, by whatever key, button or click the UI maps to it.
    Action(Action),
    /// A [`Task::Write`] finished: what became of each decision, or why none was written.
    Written {
        purpose: WriteFor,
        decisions: Vec<Decision>,
        result: Result<Vec<Decided>, String>,
    },
    /// A [`Task::ReadBack`] finished: the file as the store shows it, or why it could not be read.
    ReadBack {
        path: String,
        view: Result<Run, String>,
    },
    /// A [`Task::FindRun`] finished: the run's name and backend, or why there is none.
    RunFound {
        path: String,
        found: Result<(String, String), String>,
    },
    /// A [`Task::Recheck`] finished: the scan it started, or why none did; the files it left out,
    /// no longer on disk; and all the files it was asked for (when no scan started, those still
    /// on disk fail with the reason).
    Started {
        job: Result<Job, String>,
        missing: Vec<String>,
        paths: Vec<String>,
    },
    /// A [`Task::Switch`] finished: the view and its files' notes, or why it could not be read.
    Switched(Result<(Run, HashMap<String, FileNote>), String>),
    /// A [`Task::Stamp`] finished: the moment `Play` opens, and its file's size and mtime now, or
    /// none when they could not be read.
    Stamped {
        moment: Moment,
        stamp: Option<(u64, Timestamp)>,
    },
    /// A [`PlayerCommand::Open`] finished: where mpv was left, or why it did not open.
    Opened(Result<Opened, PlayerError>),
    /// A [`PlayerCommand::Playback`] finished: where mpv is now.
    Played(Result<PlayState, PlayerError>),
    /// A [`PlayerCommand::Close`] finished.
    Closed(Result<(), PlayerError>),
    /// A file the player had said was open failed after all: the window's load that outlasted its
    /// wait, then ended on an error.
    PlayerFailed(PlayerError),
    /// What the scan under way just did.
    Scan(scan::Event),
    /// The scan ended: its summary, or why it failed.
    ScanEnded(Result<scan::Summary, String>),
    /// Something the screen cannot go on without failed (a job's thread panicked, the terminal
    /// could not be read): it ends, with this error.
    Fatal(String),
    /// What the screen's apply (or dry run) just did.
    Apply(apply::Event),
    /// A [`Task::StartApply`] finished: apply's thread started, or why it could not.
    ApplyStarted(Result<(), String>),
    /// Apply ended: its summary, or why it failed as a whole.
    ApplyEnded(Result<apply::Summary, String>),
}

/// What a write was for, so its result can be applied.
#[derive(Debug, Clone, PartialEq)]
pub enum WriteFor {
    /// `Accept`, `Reject` and `Reword`, and `Clear`, which puts findings back to pending.
    Decide {
        entry: Entry,
        before: UndoStep,
    },
    Undo,
    /// New findings taking their rows' decisions (see [`inherit`]).
    Inherit {
        before: UndoStep,
    },
}

/// What the user asks of review. Each UI maps its own keys, buttons and clicks to these, so the
/// rules below are the same whichever drives them. While a question is open
/// ([`Review::question`]), every action but `Answer` and `Interrupt` lets it go and does nothing
/// else; what a job or an effect has to say leaves it open, but that a quit question goes once
/// nothing is left to stop, and the check-again question `Recheck` found a run for
/// ([`Input::RunFound`]) takes the place of any other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Quit; while a job runs, ask first.
    Quit,
    /// As `Quit`, asking first while a job runs; but a second, with the question open, quits.
    Interrupt,
    /// Select the entry this key names; the UI works out which. A member of a collapsed row
    /// selects its row. A key whose findings all went, or whose entry the list does not show,
    /// leaves the selection where it is and says so on the status line.
    Select(EntryKey),
    /// Select the entry `by` places on in the list as shown, or back when negative, stopping at
    /// its first and last: at an end, the entry already selected. Worked out from the selection as
    /// review holds it, so that each of several moves, however quickly they come, starts where the
    /// one before ended.
    Move(i32),
    /// Expand the selected row into its members, or collapse it (from the row or a member).
    Toggle,
    /// Show the findings under this filter, the selection kept on its finding's row.
    SetFilter(Filter),
    NextFile,
    PreviousFile,
    Accept,
    Reject,
    /// Put the selected entry's decided findings back to pending.
    Clear,
    Undo,
    /// Start rewording the selected entry: the UI opens its editor on [`Review::edit_wording`].
    Edit,
    /// Accept the edit's findings ([`Edit::findings`]) with this wording.
    Reword(String),
    CancelEdit,
    ShowPane(Pane),
    /// Open the other view of the screen's findings.
    SwitchView,
    /// Open the view these words select (`[run | paths…]`, as `subcheck review` takes them) in
    /// place of the one shown, as `SwitchView` opens the other; not while a job runs.
    OpenView(Vec<String>),
    /// Apply the view's accepted fixes; with `dry_run`, say what would change and write nothing.
    Apply {
        dry_run: bool,
    },
    /// Open the focused finding in mpv.
    Play,
    ClosePlayer,
    Playback(Playback),
    /// Answer the open question: yes, or let it go.
    Answer(bool),
    /// Select the file at this path in the files view (`progress.files`). A path the view does
    /// not have leaves the selection where it is and says so on the status line.
    SelectFile(String),
    /// The findings, on the first entry with one of the selected file's.
    ShowFileFindings,
    /// Check the selected file again, once asked.
    Recheck,
    /// Only lets the status message go: what a UI sends for a key, button or click with no action
    /// of its own.
    Dismiss,
}

/// A scan `Recheck` started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub run: String,
    /// The command that finishes it, should it stop.
    pub resume: String,
}

/// The view `SwitchView`, or `OpenView`, opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Switch {
    /// The folders of the run of this name.
    FoldersOf(String),
    /// The newest run that named the file at this path.
    RunNaming(String),
    /// The view these words select, as `subcheck review` takes them ([`Action::OpenView`]).
    Selector(Vec<String>),
}

/// What `update` asks to have done; each comes back as an [`Input`].
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // effects are few and short-lived, so a box is not worth it
pub enum Effect {
    /// For the store, or a job on a thread of its own.
    Task(Task),
    /// For the UI's player, which runs it and says how it went ([`Input::Opened`],
    /// [`Input::Played`], [`Input::Closed`]).
    Player(PlayerCommand),
}

/// What `update` asks of the store, or of a job on a thread of its own.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // tasks are few and short-lived, so a box is not worth it
pub enum Task {
    /// Record these decisions in the store.
    Write {
        purpose: WriteFor,
        decisions: Vec<Decision>,
    },
    /// Read a file back from the store, as it is now ([`Input::ReadBack`]).
    ReadBack(String),
    /// Find the run `Recheck` checks a file again into: `run`, the one the screen shows, or with
    /// none (a paths view) the newest that named the file ([`Input::RunFound`]).
    FindRun { path: String, run: Option<String> },
    /// Check `paths` again into `run`, as `subcheck scan --run <run> --force <paths>`
    /// does ([`Input::Started`]).
    Recheck { run: String, paths: Vec<String> },
    /// Open another view of the findings, the other one (`SwitchView`) or one a selector names
    /// (`OpenView`), read from the store with each file's note ([`Input::Switched`]).
    Switch(Switch),
    /// Read the size and mtime of the file `Play` opens, for review to say seek or load
    /// ([`Input::Stamped`]): a stat, which can hang on a stalled mount, so a task. A session runs
    /// it on its own thread, not the window's; the terminal runs every task in its loop, which a
    /// stall holds.
    Stamp(Moment),
    /// Apply `run`'s accepted fixes on a thread of their own (with `dry_run`, say what would change
    /// and write nothing); `karaoke` as `apply --karaoke` ([`Input::ApplyStarted`], then
    /// [`Input::Apply`] for each step and [`Input::ApplyEnded`]).
    StartApply {
        run: Run,
        dry_run: bool,
        karaoke: bool,
    },
}

/// What `input` does to `m`, and the effects to run for it.
pub fn update(m: &mut Review, input: Input, now: Timestamp) -> Vec<Effect> {
    m.kept_from = None;
    match input {
        Input::Action(action) => perform(m, action, now),
        Input::Written {
            purpose,
            decisions,
            result,
        } => written(m, purpose, &decisions, result),
        Input::ReadBack { path, view } => read_back(m, &path, view, now),
        Input::RunFound { path, found } => run_found(m, path, found),
        Input::Started {
            job,
            missing,
            paths: p,
        } => started(m, job, &missing, &p),
        Input::Switched(result) => switched(m, result),
        Input::Stamped { moment, stamp } => stamped(m, moment, stamp),
        Input::Opened(result) => {
            m.status = Some(match result {
                Ok(Opened { paused }) => Status::Opened { paused },
                Err(e) => {
                    m.loaded = None;
                    player_status(m, Asked::Open, e)
                }
            });
            Vec::new()
        }
        Input::Played(result) => {
            m.status = Some(match result {
                Ok(p) => Status::Played(p),
                Err(e) => player_status(m, Asked::Playback, e),
            });
            Vec::new()
        }
        // Whatever it says, mpv may no longer have the file review loaded.
        Input::Closed(result) => {
            m.loaded = None;
            m.status = Some(match result {
                Ok(()) => Status::Closed,
                Err(e) => player_status(m, Asked::Close, e),
            });
            Vec::new()
        }
        Input::PlayerFailed(e) => {
            m.loaded = None;
            m.status = Some(player_status(m, Asked::Open, e));
            Vec::new()
        }
        Input::Scan(event) => scanned(m, event),
        Input::ScanEnded(result) => {
            m.progress.scan = Scan::Ended(result);
            let effects = next_again(m);
            settle_quit(m, &effects);
            effects
        }
        Input::Apply(event) => applied(m, event),
        Input::ApplyStarted(Ok(())) => Vec::new(),
        Input::ApplyStarted(Err(e)) | Input::ApplyEnded(Err(e)) => {
            m.progress.apply_ended(Err(e));
            settle_quit(m, &[]);
            Vec::new()
        }
        Input::ApplyEnded(Ok(summary)) => {
            m.progress.apply_ended(Ok(summary));
            settle_quit(m, &[]);
            Vec::new()
        }
        Input::Fatal(error) => {
            m.fatal = Some(error);
            m.should_quit = true;
            Vec::new()
        }
    }
}

/// What `action` does to `m`, and the effects to run for it.
fn perform(m: &mut Review, action: Action, now: Timestamp) -> Vec<Effect> {
    // An open question takes the next action, from whichever UI: an answer, or `Interrupt`, which
    // asks to quit or quits. Any other lets it go and does nothing else, as a no does, so that a
    // click meant for the list cannot act behind a question its user has not read.
    if m.confirm.is_some() && !matches!(action, Action::Answer(_) | Action::Interrupt) {
        m.confirm = None;
        return Vec::new();
    }
    // A message stays until the user does something else. An answer, the end of an edit and
    // `Interrupt` leave it, so a message that came while a question or the editor held the line
    // shows once they are done.
    if !matches!(
        action,
        Action::Answer(_) | Action::Interrupt | Action::Reword(_) | Action::CancelEdit
    ) {
        m.status = None;
    }
    match action {
        Action::Quit => quit_or_ask(m),
        // `Interrupt` asks first while a job runs, as `Quit` does; a second one, with the
        // question open, quits.
        Action::Interrupt => {
            if m.progress.busy() && m.confirm != Some(Confirm::Quit) {
                m.confirm = Some(Confirm::Quit);
            } else {
                m.should_quit = true;
            }
        }
        Action::Select(key) => select(m, &key),
        Action::Move(by) => move_by(m, by),
        Action::Toggle => toggle(m),
        Action::SetFilter(filter) => set_filter(m, filter),
        Action::NextFile => next_file(m),
        Action::PreviousFile => previous_file(m),
        Action::Accept => {
            if let Some(entry) = m.selected_entry() {
                let findings = m.to_decide(entry);
                return decide(m, entry, findings, State::Accepted, None, now);
            }
        }
        Action::Reject => {
            if let Some(entry) = m.selected_entry() {
                let findings = m.to_decide(entry);
                return decide(m, entry, findings, State::Rejected, None, now);
            }
        }
        Action::Clear => {
            if let Some(entry) = m.selected_entry() {
                return clear(m, entry, now);
            }
        }
        Action::Undo => return undo(m, now),
        Action::Edit => open_editor(m),
        Action::Reword(text) => return reword(m, &text, now),
        Action::CancelEdit => m.editing = None,
        Action::ShowPane(pane) => m.pane = pane,
        Action::SwitchView => return switch_view(m),
        Action::OpenView(words) => return open_view(m, words),
        Action::Apply { dry_run } => return apply_key(m, dry_run),
        Action::Play => return open(m),
        Action::ClosePlayer => return vec![Effect::Player(PlayerCommand::Close)],
        Action::Playback(p) => return vec![Effect::Player(PlayerCommand::Playback(p))],
        Action::Answer(yes) => return answer(m, yes),
        Action::SelectFile(path) => select_file(m, &path),
        Action::ShowFileFindings => file_findings(m),
        Action::Recheck => return recheck(m),
        Action::Dismiss => {}
    }
    Vec::new()
}

/// The entry `key` names, where the list shows it; for a member of a collapsed row, that row. One
/// whose findings all went, or that the list no longer shows, is not selected, and the status line
/// says so: the UI that named it saw the list before a scan remade it.
fn select(m: &mut Review, key: &EntryKey) {
    let entries = m.entries();
    let shown = |e: Entry| entries.iter().position(|&n| n == e);
    // A member whose row no longer expands is found by its row, as the selection is on a refresh.
    match m
        .resolve(key)
        .and_then(|e| shown(e).or_else(|| shown(Entry::Row(e.row()))))
    {
        Some(at) => m.selected = at,
        None => m.status = Some(Status::Refused(Refusal::SelectionStays)),
    }
}

/// The entry `by` places from the selected one, clamped to the list's first and last; an empty
/// list has none to move to.
fn move_by(m: &mut Review, by: i32) {
    let Some(last) = m.entries().len().checked_sub(1) else {
        return;
    };
    let step = usize::try_from(by.unsigned_abs()).unwrap_or(usize::MAX);
    let to = if by < 0 {
        m.selected.saturating_sub(step)
    } else {
        m.selected.saturating_add(step)
    };
    m.selected = to.min(last);
}

/// The file at `path` in the files view. One not in it (another view's) is not selected, and the
/// status line says so.
fn select_file(m: &mut Review, path: &str) {
    match m.progress.files.iter().position(|f| f.path == path) {
        Some(at) => m.file_selected = at,
        None => {
            m.status = Some(Status::Refused(Refusal::NotInView {
                label: m.label(path),
            }));
        }
    }
}

/// The open question's answer: yes does what it asked; no lets it go.
fn answer(m: &mut Review, yes: bool) -> Vec<Effect> {
    let Some(confirm) = m.confirm.take() else {
        return Vec::new();
    };
    if !yes {
        return Vec::new();
    }
    match confirm {
        Confirm::Quit => {
            m.should_quit = true;
            Vec::new()
        }
        Confirm::Recheck { path, run, .. } => again(m, path, run),
        Confirm::Apply => start_apply(m, false),
    }
}

/// `Edit`: the edit, on the selected entry. A timing fix is accepted or rejected, never reworded.
fn open_editor(m: &mut Review) {
    let Some(entry) = m.selected_entry() else {
        return;
    };
    let targets = m.to_decide(entry);
    if targets.iter().any(|&i| m.finding(i).timing.is_some()) {
        m.status = Some(Status::Refused(Refusal::TimingFix));
        return;
    }
    m.editing = Some(Edit {
        entry: m.entry_key(entry),
        findings: targets.iter().map(|&i| m.finding(i).id).collect(),
    });
}

/// The edit's end: its findings accepted with `text`, or plainly when that is the suggestion,
/// wherever the filter shows them now. Empty text is refused, and the edit goes on.
fn reword(m: &mut Review, text: &str, now: Timestamp) -> Vec<Effect> {
    if m.editing.is_none() {
        return Vec::new();
    }
    if text.trim().is_empty() {
        m.status = Some(Status::Refused(Refusal::EmptyWording));
        return Vec::new();
    }
    let target = m.edit_target();
    m.editing = None;
    // `refresh` keeps an edit's findings in step with the rows and ends one left with none, so
    // only a UI that set `editing` itself finds nothing here.
    let Some((entry, findings)) = target else {
        m.status = Some(Status::Refused(Refusal::EditOutOfDate));
        return Vec::new();
    };
    let edited = (text != m.finding(findings[0]).suggestion).then_some(text);
    decide(m, entry, findings, State::Accepted, edited, now)
}

/// Decisions putting each of `findings` at `state` with that wording, but those that already are;
/// and what each had before, for `Undo`.
fn changes(
    m: &Review,
    findings: Vec<usize>,
    state: State,
    edited: Option<&str>,
    now: Timestamp,
) -> (UndoStep, Vec<Decision>) {
    let mut before = Vec::new();
    let mut decisions = Vec::new();
    for i in findings {
        let id = m.finding(i).id;
        let current = m.run.decisions.get(&id);
        if current.is_some_and(|d| d.state == state && d.edited.as_deref() == edited) {
            continue;
        }
        before.push((id, current.cloned()));
        decisions.push(Decision {
            finding: id,
            state,
            edited: edited.map(str::to_string),
            at: now,
        });
    }
    (before, decisions)
}

/// `Accept`, `Reject` or `Reword` on `entry`: `findings` at `state`, with that wording. For
/// `Accept` and `Reject` they are what the entry decides ([`Review::to_decide`]), for `Reword`
/// the edit's.
fn decide(
    m: &mut Review,
    entry: Entry,
    findings: Vec<usize>,
    state: State,
    edited: Option<&str>,
    now: Timestamp,
) -> Vec<Effect> {
    let (before, decisions) = changes(m, findings, state, edited, now);
    if decisions.is_empty() {
        m.status = Some(Status::Refused(Refusal::Already { state }));
        return Vec::new();
    }
    vec![Effect::Task(Task::Write {
        purpose: WriteFor::Decide { entry, before },
        decisions,
    })]
}

/// `Clear`: each finding `entry` stands for back to pending, but those that are.
fn clear(m: &mut Review, entry: Entry, now: Timestamp) -> Vec<Effect> {
    let decided: Vec<usize> = m
        .targets(entry)
        .into_iter()
        .filter(|&i| m.state(i) != State::Pending)
        .collect();
    let (before, decisions) = changes(m, decided, State::Pending, None, now);
    if decisions.is_empty() {
        m.status = Some(Status::Refused(Refusal::NothingToClear));
        return Vec::new();
    }
    vec![Effect::Task(Task::Write {
        purpose: WriteFor::Decide { entry, before },
        decisions,
    })]
}

/// Decisions that put back what the last decision of this session changed.
fn undo(m: &mut Review, now: Timestamp) -> Vec<Effect> {
    let Some(step) = m.undo.last() else {
        m.status = Some(Status::Refused(Refusal::NothingToUndo));
        return Vec::new();
    };
    let decisions = step
        .iter()
        .map(|(id, before)| Decision {
            finding: *id,
            state: before.as_ref().map_or(State::Pending, |d| d.state),
            edited: before.as_ref().and_then(|d| d.edited.clone()),
            at: now,
        })
        .collect();
    vec![Effect::Task(Task::Write {
        purpose: WriteFor::Undo,
        decisions,
    })]
}

/// A write's result: on success the decisions take effect, and after `Accept`, `Reject`, `Reword`
/// or `Clear` the selection moves on. A decision the store found on a replaced finding went to its
/// new one, or nowhere: its file is read back.
#[allow(clippy::too_many_lines)] // the write's every consequence for the view, in order
fn written(
    m: &mut Review,
    purpose: WriteFor,
    decisions: &[Decision],
    result: Result<Vec<Decided>, String>,
) -> Vec<Effect> {
    let outcomes = match result {
        Ok(outcomes) => outcomes,
        Err(e) => {
            m.status = Some(Status::Failed {
                what: Failure::Save,
                error: e,
            });
            return Vec::new();
        }
    };
    let old = m.selected;
    let shown = m.entries();
    for d in decisions {
        m.run.decisions.insert(d.finding, d.clone());
        // A dry run's mark said what apply would do with the decision as it was.
        if m.marks.get(&d.finding).is_some_and(Mark::dry_run) {
            m.marks.remove(&d.finding);
        }
    }
    let n = decisions.len();
    match purpose {
        WriteFor::Decide { entry, before } => {
            let state = decisions[0].state;
            m.status = Some(if state == State::Pending {
                Status::Cleared { findings: n }
            } else {
                // `Accept` means the suggestion as written, so it replaces a rewording; say so,
                // since `Undo` can bring it back.
                let replaced = before
                    .iter()
                    .zip(decisions)
                    .filter(|((_, was), now)| {
                        now.state == State::Accepted
                            && now.edited.is_none()
                            && was.as_ref().is_some_and(|d| d.edited.is_some())
                    })
                    .count();
                // The members a row's decision left alone, decided the other way on their own (or,
                // for an edit, joined the row after it opened).
                let kept: Vec<State> = m
                    .targets(entry)
                    .into_iter()
                    .map(|i| m.state(i))
                    .filter(|&s| s != state)
                    .collect();
                Status::Decided {
                    state,
                    findings: n,
                    replaced,
                    kept: kept.first().map(|&other| Kept {
                        findings: kept.len(),
                        state: other,
                    }),
                }
            });
            m.undo.push(before);
            advance(m, entry, old, &shown);
        }
        WriteFor::Inherit { before } => {
            m.undo.push(before);
            // The decision can hide a row the new member showed under the filter, or show the
            // member under an expanded row: the selection stays on the entry it was on, wherever
            // that is in the list now, and `kept_from` says where it was.
            if let Some(at) = shown
                .get(old)
                .and_then(|e| m.entries().iter().position(|n| n == e))
            {
                m.kept_from = Some(old);
                m.selected = at;
            }
            let first = &decisions[0];
            let file = m
                .run
                .findings
                .iter()
                .find(|f| f.id == first.finding)
                .map(|f| f.file.clone())
                .unwrap_or_default();
            m.status = Some(Status::Inherited {
                label: m.label(&file),
                findings: n,
                state: first.state,
            });
        }
        WriteFor::Undo => {
            m.undo.pop();
            // Name the episodes taken back, since the undone step may be a late join in a file
            // the user is not looking at.
            m.status = Some(Status::Undone {
                findings: n,
                labels: episodes(m, decisions),
            });
            // Select the row the undone decision was on, if the filter shows it.
            let first = m
                .run
                .findings
                .iter()
                .position(|f| f.id == decisions[0].finding);
            let at = m.entries().iter().position(|&e| match e {
                Entry::Row(r) => first.is_some_and(|i| m.rows[r].members.contains(&i)),
                Entry::Member(..) => false,
            });
            if let Some(at) = at {
                m.selected = at;
            }
        }
    }
    m.clamp();
    replaced(m, decisions, &outcomes)
}

/// The files whose findings a write found replaced: the store put each decision on its finding's
/// new one, or on none. Each is read back, and the status line says which happened.
fn replaced(m: &mut Review, decisions: &[Decision], outcomes: &[Decided]) -> Vec<Effect> {
    let mut files: Vec<String> = Vec::new();
    let mut gone = false;
    for (d, outcome) in decisions.iter().zip(outcomes) {
        if *outcome == Decided::Written {
            continue;
        }
        gone |= *outcome == Decided::Gone;
        if let Some(f) = m.run.findings.iter().find(|f| f.id == d.finding)
            && !files.contains(&f.file)
        {
            files.push(f.file.clone());
        }
    }
    let Some(first) = files.first() else {
        return Vec::new();
    };
    m.status = Some(Status::Replaced {
        label: m.label(first),
        gone,
    });
    files
        .into_iter()
        .map(|p| Effect::Task(Task::ReadBack(p)))
        .collect()
}

/// The episode labels of the files the decisions are on, each once, in order of first appearance.
fn episodes(m: &Review, decisions: &[Decision]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for d in decisions {
        let file = m
            .run
            .findings
            .iter()
            .find(|f| f.id == d.finding)
            .map(|f| f.file.clone())
            .unwrap_or_default();
        let name = m.label(&file);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// After a decision on `decided`, which was at `old` in `shown` (the list before it): select the
/// first entry after it that is still shown and has a pending finding, passing over a decided
/// row's members. Under a filter of accepted or rejected findings, where nothing shown is
/// pending, the first one still shown. The decided entry may be gone, and with it more than
/// itself: the last shown member of a group takes the group's row along.
fn advance(m: &mut Review, decided: Entry, old: usize, shown: &[Entry]) {
    let now = m.entries();
    let to_do = matches!(m.filter, Filter::All | Filter::Only(State::Pending));
    let next = shown
        .iter()
        .skip(old + 1)
        .filter(|e| {
            !(matches!(decided, Entry::Row(_))
                && matches!(e, Entry::Member(r, _) if *r == decided.row()))
        })
        .filter(|&&e| !to_do || m.has_pending(e))
        .find_map(|e| now.iter().position(|n| n == e));
    m.selected = match next {
        Some(i) => i,
        // Nothing after it: stay on it if it is still there, else on the last entry.
        None => now
            .iter()
            .position(|&e| e == decided)
            .unwrap_or(now.len().saturating_sub(1)),
    };
}

/// The list under `filter`.
fn set_filter(m: &mut Review, filter: Filter) {
    let focus = m.focus();
    m.filter = filter;
    // Stay on the same finding's row when it is still shown.
    let entries = m.entries();
    m.selected = focus
        .and_then(|i| {
            entries
                .iter()
                .position(|&e| matches!(e, Entry::Row(r) if m.rows[r].members.contains(&i)))
        })
        .unwrap_or(0);
}

/// `Toggle`: expand a group into its members, or collapse it (from the row or from a member).
fn toggle(m: &mut Review) {
    match m.selected_entry() {
        Some(Entry::Row(r)) if m.rows[r].members.len() > 1 => {
            if !m.expanded.remove(&r) {
                m.expanded.insert(r);
            }
        }
        Some(Entry::Member(r, _)) => {
            m.expanded.remove(&r);
            if let Some(at) = m.entries().iter().position(|&e| e == Entry::Row(r)) {
                m.selected = at;
            }
        }
        _ => {}
    }
}

fn next_file(m: &mut Review) {
    let entries = m.entries();
    let Some(current) = entries.get(m.selected).and_then(|&e| m.file_of(e)) else {
        return;
    };
    match (m.selected + 1..entries.len()).find(|&i| m.file_of(entries[i]) != Some(current)) {
        Some(i) => m.selected = i,
        None => m.status = Some(Status::Refused(Refusal::LastFile)),
    }
}

fn previous_file(m: &mut Review) {
    let entries = m.entries();
    let file = |i: usize| m.file_of(entries[i]);
    let Some(current) = entries.get(m.selected).and_then(|&e| m.file_of(e)) else {
        return;
    };
    // The nearest entry of another file, then the first entry of that file's run of entries.
    let Some(mut i) = (0..m.selected).rev().find(|&i| file(i) != Some(current)) else {
        m.status = Some(Status::Refused(Refusal::FirstFile));
        return;
    };
    let previous = file(i);
    while i > 0 && file(i - 1) == previous {
        i -= 1;
    }
    m.selected = i;
}

fn open(m: &mut Review) -> Vec<Effect> {
    let Some(i) = m.focus() else {
        return Vec::new();
    };
    let f = m.finding(i);
    if m.notes.get(&f.file) == Some(&FileNote::Missing) {
        let path = f.file.clone();
        m.status = Some(Status::Refused(Refusal::FileMissing { path }));
        return Vec::new();
    }
    // Whether mpv plays once there is the player's to keep, from mpv's own state, a pause made in
    // mpv's window among it, so the moment carries none. A timing finding with a frame that shows
    // its error opens on that frame, sought at the frame's own time: worked out from the frame rate
    // for the frame before a cut, it can be a millisecond off the time the file stores, either way,
    // which the 5 ms that mpv shows a frame from before its target takes in, and a line's 4.5 ms
    // past would not. One with no such frame opens on its first moved line, which, like any line,
    // opens on its first frame.
    let (at_ms, start_us, sid) = if let Some(at) = list::timing_moment(&m.run, f) {
        let start_us = if at.on_frame {
            at.at_ms.saturating_mul(1000)
        } else {
            player::line_start_us(at.at_ms)
        };
        (at.at_ms, start_us, at.sid)
    } else if f.timing.is_some() {
        m.status = Some(Status::Refused(Refusal::NothingToOpen));
        return Vec::new();
    } else {
        let (at, sid) = list::line_moment(&m.run, f);
        (at, player::line_start_us(at), sid)
    };
    let moment = Moment {
        file: f.file.clone(),
        start_us: start_us.max(0),
        sid,
    };
    m.status = Some(Status::Opening {
        label: m.label(&moment.file),
        at_ms,
    });
    vec![Effect::Task(Task::Stamp(moment))]
}

/// `Play`'s file stat, and review's word to the player: a seek when review last had it load this
/// file, unchanged on disk since, else a load. The player checks the seek against what its mpv
/// has open, so a load forgotten too soon costs a reload, never a wrong frame. A file that is not
/// there (whose size and mtime cannot be read) is not opened, as one whose note says it is missing
/// is not: mpv could not read it either, and the terminal's player, which does not wait for a load
/// to land, would say it opened. Its note is left as it is, so that the next `Play` stats the file
/// again: a mount back, it opens.
fn stamped(m: &mut Review, moment: Moment, stamp: Option<(u64, Timestamp)>) -> Vec<Effect> {
    let Some(stamp) = stamp else {
        m.loaded = None;
        m.status = Some(Status::Refused(Refusal::FileMissing { path: moment.file }));
        return Vec::new();
    };
    let now = Some((moment.file.clone(), stamp));
    let how = if m.loaded == now {
        How::Seek
    } else {
        How::Load
    };
    m.loaded = now;
    vec![Effect::Player(PlayerCommand::Open {
        moment,
        how,
        options: player::load_options(),
    })]
}

/// What went wrong with player command `asked`, as review's message: a file by the list's name for
/// it, an error with text in the words of whoever made it.
fn player_status(m: &Review, asked: Asked, e: PlayerError) -> Status {
    match e {
        PlayerError::NotOpen => Status::NotOpen { asked },
        PlayerError::NoFile => Status::NoFile,
        PlayerError::StillOpening => Status::StillOpening,
        PlayerError::CouldNotOpen { file } => Status::CouldNotOpen {
            label: m.label(&file),
        },
        PlayerError::NoAnswer => Status::NoAnswer,
        PlayerError::HungUp => Status::HungUp,
        PlayerError::NoVideo => Status::NoVideo,
        PlayerError::Mpv(error) => Status::Failed {
            what: Failure::Mpv,
            error,
        },
        PlayerError::Failed(error) => Status::Failed {
            what: Failure::Player,
            error,
        },
    }
}

/// `Quit`: quit; while a job runs, ask first.
fn quit_or_ask(m: &mut Review) {
    if m.progress.busy() {
        m.confirm = Some(Confirm::Quit);
    } else {
        m.should_quit = true;
    }
}

/// `Apply` (or with `dry_run`, a dry run): every accepted fix of the view on screen. Not while a
/// scan runs (apply would contend with its reader for the disks, and findings are still coming) or
/// apply does, nor with nothing accepted. `Apply` asks first; a dry run writes nothing, so it
/// starts at once.
fn apply_key(m: &mut Review, dry_run: bool) -> Vec<Effect> {
    if m.progress.running() {
        m.status = Some(Status::Refused(Refusal::ScanRunning));
        return Vec::new();
    }
    if m.progress.applying() {
        m.status = Some(Status::Refused(Refusal::AlreadyApplying));
        return Vec::new();
    }
    if m.apply_counts().0 == 0 {
        m.status = Some(Status::Refused(Refusal::NoAcceptedFixes));
        return Vec::new();
    }
    if dry_run {
        return start_apply(m, true);
    }
    m.confirm = Some(Confirm::Apply);
    Vec::new()
}

/// Start apply (or a dry run) on the view as it stands: decisions made meanwhile count from the
/// next. Running from here, so a `Quit` before its first step still asks.
fn start_apply(m: &mut Review, dry_run: bool) -> Vec<Effect> {
    // A view's name is its run's, or its paths quoted: `subcheck apply` takes either back.
    let mut resume = format!("subcheck apply {}", m.run.name);
    if dry_run {
        resume.push_str(" --dry-run");
    }
    if m.karaoke {
        resume.push_str(" --karaoke");
    }
    m.progress.start_apply(dry_run, resume);
    vec![Effect::Task(Task::StartApply {
        run: m.run.clone(),
        dry_run,
        karaoke: m.karaoke,
    })]
}

/// A step of the screen's apply: the files view follows it. A file done marks its findings (outside
/// a dry run, a fix made, or found made, by the store's mark instead), takes the store's new
/// `applied` marks, and its record takes the size and mtime its checks moved to, so the view
/// matches the store. A file that failed marks the findings it was to fix `failed` (`would fail`),
/// but for those the store has as applied: their fix is in the file.
fn applied(m: &mut Review, event: apply::Event) -> Vec<Effect> {
    m.progress.apply_event(&event);
    let dry_run = m.progress.apply.as_ref().is_some_and(|a| a.dry_run);
    match event {
        apply::Event::Done {
            path,
            outcomes,
            marked,
            restamped,
            ..
        } => {
            for (id, outcome) in &outcomes {
                match Mark::of(outcome, dry_run) {
                    Some(mark) => {
                        m.marks.insert(*id, mark);
                    }
                    None => {
                        m.marks.remove(id);
                    }
                }
            }
            m.run.applied.extend(marked);
            // Every model record for the path takes the new stamp, even if only one check kind
            // moved: only `file_notes` reads it, and another view re-reads the store.
            if let Some((size, mtime)) = restamped {
                for r in m.run.files.iter_mut().filter(|r| r.path == path) {
                    r.size = size;
                    r.mtime = mtime;
                }
            }
        }
        apply::Event::Failed {
            error, findings, ..
        } => {
            for id in findings {
                if m.run.applied.contains(&id) {
                    m.marks.remove(&id);
                } else if dry_run {
                    m.marks.insert(id, Mark::WouldFail(error.clone()));
                } else {
                    m.marks.insert(id, Mark::Failed(error.clone()));
                }
            }
        }
        _ => {}
    }
    Vec::new()
}

/// The view `SwitchView` or `OpenView` asked for has been read, or could not be: take it, or say
/// why the screen is as it was.
fn switched(
    m: &mut Review,
    result: Result<(Run, HashMap<String, FileNote>), String>,
) -> Vec<Effect> {
    match result {
        Ok((run, notes)) => m.switch(run, notes),
        Err(error) => {
            m.status = Some(Status::Failed {
                what: Failure::Switch,
                error,
            });
        }
    }
    Vec::new()
}

/// `SwitchView`: the other view of the screen's findings. From a run's view, the run's folders;
/// from a paths view, the newest run that named the selected file: the selected entry's (a row's
/// first member's), or in the files view, or with nothing selected in the list, the selected
/// file's. Not while a job runs: its progress belongs to the view it started in.
fn switch_view(m: &mut Review) -> Vec<Effect> {
    if refused_while_a_job_runs(m) {
        return Vec::new();
    }
    if !m.run.of_paths {
        return vec![Effect::Task(Task::Switch(Switch::FoldersOf(
            m.run.name.clone(),
        )))];
    }
    let in_list = match m.pane {
        Pane::Findings => m.selected_entry().map(|e| {
            let i = match e {
                Entry::Row(r) => m.rows[r].members[0],
                Entry::Member(_, i) => i,
            };
            m.run.findings[i].file.clone()
        }),
        Pane::Files => None,
    };
    in_list
        .or_else(|| {
            m.progress
                .files
                .get(m.file_selected)
                .map(|f| f.path.clone())
        })
        .map(|p| Effect::Task(Task::Switch(Switch::RunNaming(p))))
        .into_iter()
        .collect()
}

/// The view `words` select, in place of the one shown; not while a job runs, as for `SwitchView`.
fn open_view(m: &mut Review, words: Vec<String>) -> Vec<Effect> {
    if refused_while_a_job_runs(m) {
        return Vec::new();
    }
    vec![Effect::Task(Task::Switch(Switch::Selector(words)))]
}

/// Whether a new view is refused, for a scan or apply that runs: its progress belongs to the view
/// it started in. The status line says which.
fn refused_while_a_job_runs(m: &mut Review) -> bool {
    let running = if m.progress.running() {
        Refusal::ScanRunning
    } else if m.progress.applying() {
        Refusal::ApplyRunning
    } else {
        return false;
    };
    m.status = Some(Status::Refused(running));
    true
}

/// `Recheck` on the selected file: find the run it is checked again into, then ask. A file the scan
/// is checking is being checked already, and one `Recheck` queued is queued already.
fn recheck(m: &mut Review) -> Vec<Effect> {
    // A check would read a file apply may be remuxing or swapping.
    if m.progress.applying() {
        m.status = Some(Status::Refused(Refusal::ApplyRunning));
        return Vec::new();
    }
    let Some(file) = m.progress.files.get(m.file_selected) else {
        return Vec::new();
    };
    let path = file.path.clone();
    let queued_again = file.stage == Stage::QueuedAgain;
    let label = m.label(&path);
    if m.progress.checking(&path).is_some() {
        m.status = Some(Status::Refused(Refusal::BeingChecked { label }));
        return Vec::new();
    }
    if queued_again {
        m.status = Some(Status::QueuedAgain { label });
        return Vec::new();
    }
    let run = (!m.run.of_paths).then(|| m.run.name.clone());
    vec![Effect::Task(Task::FindRun { path, run })]
}

/// A yes to checking `path` again into `run`: at once with no scan running, else once it ends.
fn again(m: &mut Review, path: String, run: String) -> Vec<Effect> {
    if !m.progress.running() {
        return vec![Effect::Task(Task::Recheck {
            run,
            paths: vec![path],
        })];
    }
    m.status = Some(Status::QueuedAgain {
        label: m.label(&path),
    });
    m.progress.queue_again(&path, &run);
    Vec::new()
}

/// The run `Recheck` checks a file again into, and its LLM, for the question; or why there is none.
fn run_found(m: &mut Review, path: String, found: Result<(String, String), String>) -> Vec<Effect> {
    match found {
        Ok((run, backend)) => m.confirm = Some(Confirm::Recheck { path, run, backend }),
        Err(error) => {
            m.status = Some(Status::Failed {
                what: Failure::Recheck,
                error,
            });
        }
    }
    Vec::new()
}

/// The files `Recheck` queued into the first run among them, as one scan; the rest wait for it
/// to end.
fn next_again(m: &mut Review) -> Vec<Effect> {
    m.progress
        .next_batch()
        .map(|(run, paths)| Effect::Task(Task::Recheck { run, paths }))
        .into_iter()
        .collect()
}

/// A scan `Recheck` started, or why none did: then the files asked for that are still on disk fail
/// with the reason (so `Recheck` works on them again), and the next run's queued files go. A file
/// left out is no longer on disk.
fn started(
    m: &mut Review,
    job: Result<Job, String>,
    missing: &[String],
    paths: &[String],
) -> Vec<Effect> {
    let effects = match job {
        Ok(job) => {
            m.progress.start(&job.run, job.resume, true);
            Vec::new()
        }
        Err(e) => {
            for p in paths.iter().filter(|p| !missing.contains(p)) {
                m.progress.fail(p, &e);
            }
            m.status = Some(Status::Failed {
                what: Failure::Recheck,
                error: e,
            });
            next_again(m)
        }
    };
    for p in missing {
        m.progress.fail(p, "no such file");
    }
    if let Some(p) = missing.first() {
        m.status = Some(Status::NoSuchFile { label: m.label(p) });
    }
    settle_quit(m, &effects);
    effects
}

/// A question to stop a job stays while a queued batch takes over: `effects` start it, and
/// effects run before the next input is taken. With nothing left to run it goes, so the status line
/// shows how the job ended, or why the last batch did not start, and the next `Quit` quits at once.
fn settle_quit(m: &mut Review, effects: &[Effect]) {
    if m.confirm == Some(Confirm::Quit) && !m.progress.busy() && effects.is_empty() {
        m.confirm = None;
    }
}

/// `ShowFileFindings`: the findings, on the first entry with one of the selected file's, if the
/// filter shows one.
fn file_findings(m: &mut Review) {
    let Some(path) = m
        .progress
        .files
        .get(m.file_selected)
        .map(|f| f.path.clone())
    else {
        return;
    };
    let first = m
        .entries()
        .into_iter()
        .position(|e| m.targets(e).iter().any(|&i| m.finding(i).file == path));
    match first {
        Some(at) => {
            m.selected = at;
            m.pane = Pane::Findings;
        }
        None => {
            m.status = Some(Status::Refused(Refusal::NoFindingsShown {
                label: m.label(&path),
            }));
        }
    }
}

/// A step of the scan: its progress; and of the records it appended, its LLM calls' cost, and a
/// failed attempt's record, which leaves the file's findings standing. A file it saved is read back
/// from the store, which brings its findings with the decisions carried to them.
fn scanned(m: &mut Review, event: scan::Event) -> Vec<Effect> {
    m.progress.scan_event(&event);
    let scan::Event::Appended(records) = event else {
        return Vec::new();
    };
    let mut effects = Vec::new();
    for r in records {
        match r {
            Record::File(f) if f.error.is_none() => {
                effects.push(Effect::Task(Task::ReadBack(f.path)));
            }
            r @ (Record::File(_) | Record::Usage(_)) => m.run.add(r),
            // They come with the read-back, as the store has them by then.
            Record::Finding(_) | Record::Decision(_) => {}
        }
    }
    effects
}

/// A file read back from the store: its findings swapped in ([`Review::refresh`]), and the new ones
/// taking their rows' decisions ([`inherit`]).
fn read_back(m: &mut Review, path: &str, view: Result<Run, String>, now: Timestamp) -> Vec<Effect> {
    match view {
        Ok(view) => {
            let new = m.refresh(path, view);
            inherit(m, &new, now)
        }
        Err(error) => {
            let label = m.label(path);
            m.status = Some(Status::Failed {
                what: Failure::ReadBack { label },
                error,
            });
            Vec::new()
        }
    }
}

/// The decision each of the `new` findings takes from its row: a typo finding whose row's other
/// members are all decided alike (the same state, the same wording or none) takes theirs. A
/// timing finding takes none: each episode's fix can differ. Nor does one that came with a
/// decision carried over from the file's earlier check of the same line and fix. One undo step for
/// them all.
fn inherit(m: &Review, new: &[usize], now: Timestamp) -> Vec<Effect> {
    let mut before = Vec::new();
    let mut decisions = Vec::new();
    for &i in new {
        let f = m.finding(i);
        if f.timing.is_some() || m.run.decisions.contains_key(&f.id) {
            continue;
        }
        let Some(row) = m.rows.iter().find(|r| r.members.contains(&i)) else {
            continue;
        };
        let mut others = row
            .members
            .iter()
            .filter(|j| !new.contains(j))
            .map(|&j| m.run.decisions.get(&m.finding(j).id));
        let Some(Some(first)) = others.next() else {
            continue;
        };
        let alike =
            others.all(|d| d.is_some_and(|d| d.state == first.state && d.edited == first.edited));
        if first.state == State::Pending || !alike {
            continue;
        }
        let id = m.finding(i).id;
        before.push((id, None));
        decisions.push(Decision {
            finding: id,
            state: first.state,
            edited: first.edited.clone(),
            at: now,
        });
    }
    if decisions.is_empty() {
        return Vec::new();
    }
    vec![Effect::Task(Task::Write {
        purpose: WriteFor::Inherit { before },
        decisions,
    })]
}

/// What review's tests share, in this library and outside it: tests outside it reach this through
/// the `test-support` feature, as they do `crate::test_support`.
#[cfg(any(test, feature = "test-support"))]
pub mod tests_support {
    use std::path::PathBuf;

    use jiff::Timestamp;

    use super::{Effect, Input, Task, update};
    use crate::review::model::Review;
    use crate::review::model::tests_support::{self, path, sample};
    use crate::scan::Event;
    use crate::store::{Decided, Decision, State};

    pub fn now() -> Timestamp {
        "2026-10-17T12:00:00Z".parse().unwrap()
    }

    /// What the store says to a [`Task::Write`] when it takes every decision.
    pub fn taken(effects: Vec<Effect>) -> Input {
        let [Effect::Task(Task::Write { purpose, decisions })] =
            <[Effect; 1]>::try_from(effects).unwrap()
        else {
            panic!("expected one write");
        };
        let n = decisions.len();
        Input::Written {
            purpose,
            decisions,
            result: Ok(vec![Decided::Written; n]),
        }
    }

    pub fn decided(effects: &[Effect]) -> Vec<(u64, State, Option<&str>)> {
        match effects {
            [Effect::Task(Task::Write { decisions, .. })] => decisions
                .iter()
                .map(|d| (d.finding, d.state, d.edited.as_deref()))
                .collect(),
            _ => panic!("expected one write: {effects:?}"),
        }
    }

    /// Episode `ep` read back once `scan` saved `findings` (see `model::tests_support::read_back`).
    pub fn saved(m: &Review, ep: u32, findings: Vec<crate::store::Finding>) -> Input {
        Input::ReadBack {
            path: path(ep),
            view: Ok(tests_support::read_back(m, ep, findings)),
        }
    }

    pub const TYPO: &str = "The beat gets trough to our mind";
    pub const FIXED: &str = "The beat gets through to our mind";

    pub fn decide_as(m: &mut Review, ids: &[u64], state: State, edited: Option<&str>) {
        for &id in ids {
            m.run.decisions.insert(
                id,
                Decision {
                    finding: id,
                    state,
                    edited: edited.map(Into::into),
                    at: now(),
                },
            );
        }
    }

    /// The sample run with a scan of E04 and E05 under way.
    pub fn scanning() -> Review {
        let mut m = sample();
        let queued = vec![PathBuf::from(path(4)), PathBuf::from(path(5))];
        update(&mut m, Input::Scan(Event::Queued(queued)), now());
        m
    }

    pub fn states(m: &Review) -> Vec<State> {
        m.run
            .findings
            .iter()
            .map(|f| m.run.state_of(f.id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::apply::patch::Outcome;
    use crate::review::model::{Mark, Question};
    use crate::review::progress::{ApplyAt, Stopping};

    use super::tests_support::{
        FIXED, TYPO, decide_as, decided, now, saved, scanning, states, taken,
    };
    use crate::review::model::tests_support::{self, finding, path, sample};
    use crate::scan::Event;
    use crate::store::{Decided, Record};

    /// `action` done to `m`: the effects it asks for.
    fn act(m: &mut Review, action: Action) -> Vec<Effect> {
        update(m, Input::Action(action), now())
    }

    /// The message after `findings` findings were put at `state`.
    fn was_decided(state: State, findings: usize, replaced: usize, kept: Option<Kept>) -> Status {
        Status::Decided {
            state,
            findings,
            replaced,
            kept,
        }
    }

    /// The message as `Play` asks to open file `label` at `at_ms`.
    fn opening(label: &str, at_ms: i64) -> Status {
        Status::Opening {
            label: label.into(),
            at_ms,
        }
    }

    /// The scan a quit would stop: `done` of `queued` files, and `again` queued after it.
    fn scan_on(done: usize, queued: usize, again: usize) -> Stopping {
        Stopping::Scan {
            done,
            queued,
            again,
        }
    }

    /// Apply a quit would stop (with `dry_run`, a dry run): on E01, its one file, at `stage`.
    fn applying_on(dry_run: bool, stage: apply::Stage) -> Stopping {
        Stopping::Apply {
            dry_run,
            done: 0,
            queued: 1,
            at: ApplyAt::On {
                file: "E01".into(),
                stage,
            },
        }
    }

    /// Run a [`Task::Write`] as if the store took it, and feed the result back.
    fn written(m: &mut Review, effects: Vec<Effect>) {
        update(m, taken(effects), now());
    }

    /// What `scan` appends for episode `ep`: its file record, then its findings.
    fn appended(ep: u32, findings: Vec<crate::store::Finding>) -> Input {
        let mut records = vec![Record::File(tests_support::file(ep))];
        records.extend(findings.into_iter().map(Record::Finding));
        Input::Scan(Event::Appended(records))
    }

    #[test]
    fn a_typo_joining_a_row_decided_one_way_takes_that_decision_and_u_takes_it_back() {
        let mut m = sample();
        let mine = "The beat gets through to our minds";
        decide_as(&mut m, &[1, 2, 4], State::Accepted, Some(mine));
        let msg = saved(&m, 4, vec![finding(5, 4, TYPO, FIXED, 165_060)]);
        let effects = update(&mut m, msg, now());
        assert_eq!(decided(&effects), [(5, State::Accepted, Some(mine))]);
        written(&mut m, effects);
        assert_eq!(m.run.state_of(5), State::Accepted);
        assert_eq!(
            m.status,
            Some(Status::Inherited {
                label: "E04".into(),
                findings: 1,
                state: State::Accepted,
            })
        );
        let effects = act(&mut m, Action::Undo);
        assert_eq!(decided(&effects), [(5, State::Pending, None)]);
    }

    #[test]
    fn a_typo_that_comes_with_a_decision_carried_over_keeps_it_over_its_rows() {
        // E04 checked again: its lyric comes back rejected, carried over from its earlier check,
        // into a row the other episodes accepted.
        let mut m = sample();
        decide_as(&mut m, &[1, 2, 4], State::Accepted, None);
        let carried = Decision {
            finding: 5,
            state: State::Rejected,
            edited: None,
            at: now(),
        };
        let mut view = tests_support::read_back(&m, 4, vec![finding(5, 4, TYPO, FIXED, 165_060)]);
        view.decisions.insert(5, carried);
        let effects = update(
            &mut m,
            Input::ReadBack {
                path: path(4),
                view: Ok(view),
            },
            now(),
        );
        assert_eq!(effects, []);
        assert_eq!(m.run.state_of(5), State::Rejected);
    }

    #[test]
    fn a_late_join_that_takes_its_rows_decision_leaves_the_selection_on_its_row() {
        // A third row, E03's, below the snacks row: one for the selection to slip onto.
        let mut m = sample();
        let view = tests_support::read_back(
            &m,
            3,
            vec![finding(5, 3, "An erly line.", "An early line.", 2_000_000)],
        );
        m.refresh(&path(3), view);
        decide_as(&mut m, &[1, 2, 4], State::Accepted, None);
        // Pending only: the lyric's row is hidden until E04's lyric shows it for a moment.
        m.filter = Filter::Only(State::Pending);
        m.selected = 0;
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)));
        let msg = saved(&m, 4, vec![finding(6, 4, TYPO, FIXED, 165_060)]);
        let effects = update(&mut m, msg, now());
        written(&mut m, effects);
        assert_eq!(m.entries(), [Entry::Row(1), Entry::Row(2)]);
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)), "the snacks row");
        // Accepted only, the lyric's row expanded and the snacks row selected: the decision shows
        // the new member above it.
        let mut m = sample();
        decide_as(&mut m, &[1, 2, 3, 4], State::Accepted, None);
        m.filter = Filter::Only(State::Accepted);
        m.expanded.insert(0);
        m.selected = 4;
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)));
        let msg = saved(&m, 4, vec![finding(5, 4, TYPO, FIXED, 165_060)]);
        let effects = update(&mut m, msg, now());
        written(&mut m, effects);
        assert_eq!(m.entries().len(), 6);
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)), "the snacks row");
    }

    #[test]
    fn u_in_the_files_view_takes_a_late_join_back_and_names_its_episode() {
        let mut m = sample();
        decide_as(&mut m, &[1, 2, 4], State::Accepted, None);
        let msg = saved(&m, 4, vec![finding(5, 4, TYPO, FIXED, 165_060)]);
        let effects = update(&mut m, msg, now());
        written(&mut m, effects);
        m.pane = Pane::Files;
        let effects = act(&mut m, Action::Undo);
        assert_eq!(decided(&effects), [(5, State::Pending, None)]);
        written(&mut m, effects);
        assert_eq!(m.run.state_of(5), State::Pending);
        assert_eq!(
            m.status,
            Some(Status::Undone {
                findings: 1,
                labels: vec!["E04".into()],
            })
        );
        assert_eq!(m.pane, Pane::Files);
    }

    #[test]
    fn an_undo_across_two_episodes_names_both_in_order_of_first_appearance() {
        let mut m = sample();
        decide_as(&mut m, &[1, 2, 4], State::Accepted, None);
        let msg = saved(&m, 4, vec![finding(5, 4, TYPO, FIXED, 165_060)]);
        let effects = update(&mut m, msg, now());
        written(&mut m, effects);
        let msg = saved(&m, 5, vec![finding(6, 5, TYPO, FIXED, 165_060)]);
        let effects = update(&mut m, msg, now());
        written(&mut m, effects);
        // One step taking back findings of E05 and E04 at once.
        m.undo.push(vec![(6, None), (5, None)]);
        let effects = act(&mut m, Action::Undo);
        written(&mut m, effects);
        assert_eq!(
            m.status,
            Some(Status::Undone {
                findings: 2,
                labels: vec!["E05".into(), "E04".into()],
            })
        );
    }

    #[test]
    fn a_new_finding_stays_pending_in_a_mixed_row_and_in_a_timing_row() {
        let mut m = sample();
        decide_as(&mut m, &[1], State::Accepted, None);
        decide_as(&mut m, &[2], State::Rejected, None);
        let msg = saved(&m, 4, vec![finding(5, 4, TYPO, FIXED, 165_060)]);
        let effects = update(&mut m, msg, now());
        assert_eq!(effects, []);
        assert_eq!(m.run.state_of(5), State::Pending);
        // A season's track accepted at −1 frame: E04's fix may differ, so it waits.
        let mut m = tests_support::with_timing();
        decide_as(&mut m, &[11, 12, 13], State::Accepted, None);
        let msg = saved(&m, 4, vec![tests_support::timing(14, 4, false)]);
        let effects = update(&mut m, msg, now());
        assert_eq!(effects, []);
        assert_eq!(m.run.state_of(14), State::Pending);
    }

    #[test]
    fn a_new_finding_stays_pending_in_a_row_accepted_in_two_wordings_or_with_one_pending() {
        let join = |m: &mut Review| {
            let msg = saved(m, 4, vec![finding(5, 4, TYPO, FIXED, 165_060)]);
            update(m, msg, now())
        };
        // Every member accepted, E03's in words of the user's own: no one wording to take.
        let mut m = sample();
        decide_as(&mut m, &[1, 2], State::Accepted, None);
        let mine = "The beat gets through to our minds";
        decide_as(&mut m, &[4], State::Accepted, Some(mine));
        assert_eq!(join(&mut m), []);
        assert_eq!(m.run.state_of(5), State::Pending);
        // E01 and E02 accepted, E03 not yet decided.
        let mut m = sample();
        decide_as(&mut m, &[1, 2], State::Accepted, None);
        assert_eq!(join(&mut m), []);
        assert_eq!(m.run.state_of(5), State::Pending);
    }

    #[test]
    fn quitting_while_the_scan_runs_asks_first() {
        let mut m = scanning();
        act(&mut m, Action::Quit);
        assert!(!m.should_quit);
        assert_eq!(m.question(), Some(Question::Quit(scan_on(0, 2, 0))));
        act(&mut m, Action::Answer(false));
        assert!(!m.should_quit && m.confirm.is_none());
        act(&mut m, Action::Quit);
        act(&mut m, Action::Answer(true));
        assert!(m.should_quit && m.progress.running());
        // `Interrupt` asks too, and a second one quits.
        let mut m = scanning();
        act(&mut m, Action::Interrupt);
        assert!(!m.should_quit);
        act(&mut m, Action::Interrupt);
        assert!(m.should_quit);
        // Once the scan has ended, `Quit` quits at once.
        let mut m = scanning();
        update(
            &mut m,
            Input::ScanEnded(Ok(crate::scan::Summary::default())),
            now(),
        );
        act(&mut m, Action::Quit);
        assert!(m.should_quit);
    }

    /// Each question review asks, open on the sample with a scan under way, over an edit, as
    /// `Interrupt` mid-edit opens one, and with a message that came while it was open.
    fn asking() -> Vec<Review> {
        let recheck = Confirm::Recheck {
            path: path(2),
            run: "r".into(),
            backend: "claude".into(),
        };
        [Confirm::Quit, recheck, Confirm::Apply]
            .into_iter()
            .map(|confirm| {
                let mut m = scanning();
                act(&mut m, Action::Edit);
                assert!(m.editing.is_some());
                m.confirm = Some(confirm);
                m.status = Some(Status::Opened { paused: true });
                m
            })
            .collect()
    }

    // As a no: the question goes, and nothing else moves, the message included, which shows once
    // the question is gone.
    #[test]
    fn under_a_question_any_action_but_an_answer_or_interrupt_only_lets_it_go() {
        let actions = [
            Action::Quit,
            Action::Select(EntryKey::Row(vec![3])),
            Action::Move(1),
            Action::Toggle,
            Action::SetFilter(Filter::Only(State::Accepted)),
            Action::NextFile,
            Action::PreviousFile,
            Action::Accept,
            Action::Reject,
            Action::Clear,
            Action::Undo,
            Action::Edit,
            Action::Reword("Fixed.".into()),
            Action::CancelEdit,
            Action::ShowPane(Pane::Files),
            Action::SwitchView,
            Action::Apply { dry_run: true },
            Action::Play,
            Action::ClosePlayer,
            Action::Playback(Playback::Pause),
            Action::SelectFile(path(3)),
            Action::ShowFileFindings,
            Action::Recheck,
            Action::Dismiss,
        ];
        let seen = |m: &Review| {
            (
                m.selected,
                m.expanded.clone(),
                m.filter,
                m.pane,
                m.file_selected,
                m.editing.clone(),
                m.status.clone(),
                m.should_quit,
            )
        };
        for action in actions {
            for mut m in asking() {
                let before = seen(&m);
                let asked = m.confirm.clone();
                let effects = act(&mut m, action.clone());
                let what = format!("{action:?} under {asked:?}");
                assert_eq!(effects, [], "{what}");
                assert_eq!(m.confirm, None, "{what}");
                assert_eq!(seen(&m), before, "{what}");
            }
        }
    }

    // They come unasked: a question the user has not answered stays.
    #[test]
    fn what_a_job_or_the_player_says_leaves_a_question_open() {
        let inputs = [
            Input::Scan(Event::Queued(vec![PathBuf::from(path(4))])),
            Input::Apply(apply::Event::Queued(vec![path(1)])),
            Input::ReadBack {
                path: path(2),
                view: Err("database is locked".into()),
            },
            Input::Switched(Err("disk I/O error".into())),
            Input::Opened(Ok(Opened { paused: true })),
            Input::Played(Ok(PlayState {
                paused: true,
                at_ms: 1000,
            })),
            Input::Closed(Err(PlayerError::NotOpen)),
            Input::PlayerFailed(PlayerError::CouldNotOpen { file: path(1) }),
        ];
        for input in inputs {
            for mut m in asking() {
                let asked = m.confirm.clone();
                let what = format!("{input:?} under {asked:?}");
                update(&mut m, input.clone(), now());
                assert_eq!(m.confirm, asked, "{what}");
            }
        }
    }

    #[test]
    fn a_quit_question_goes_when_the_scan_ends() {
        let mut m = scanning();
        act(&mut m, Action::Quit);
        assert!(m.confirm.is_some());
        let summary = crate::scan::Summary {
            run: "r".into(),
            files: 2,
            checked: 2,
            ..crate::scan::Summary::default()
        };
        update(&mut m, Input::ScanEnded(Ok(summary)), now());
        // Nothing left to stop: the line gives the summary, and `Quit` quits at once.
        assert_eq!((&m.confirm, &m.status), (&None, &None));
        assert_eq!(
            m.progress.line().as_deref(),
            Some(
                "run r: 2 files, 2 checked, 0 failed, 0 unchanged since an earlier scan; 0 findings"
            )
        );
        act(&mut m, Action::Quit);
        assert!(m.should_quit);
    }

    #[test]
    fn a_quit_question_stays_while_a_queued_batch_takes_over_and_goes_after_the_last() {
        let mut m = scanning();
        for (ep, run) in [(1, "r"), (2, "s")] {
            m.confirm = Some(Confirm::Recheck {
                path: path(ep),
                run: run.into(),
                backend: "claude".into(),
            });
            assert_eq!(act(&mut m, Action::Answer(true)), []);
        }
        act(&mut m, Action::Quit);
        let ended = update(
            &mut m,
            Input::ScanEnded(Ok(scan::Summary::default())),
            now(),
        );
        assert_eq!(
            ended,
            [Effect::Task(Task::Recheck {
                run: "r".into(),
                paths: vec![path(1)],
            })]
        );
        assert_eq!(m.confirm, Some(Confirm::Quit));
        let job = Job {
            run: "r".into(),
            resume: "subcheck scan --run r --force /m/E01.mkv".into(),
        };
        update(
            &mut m,
            Input::Started {
                job: Ok(job),
                missing: vec![],
                paths: vec![path(1)],
            },
            now(),
        );
        update(
            &mut m,
            Input::Scan(Event::Queued(vec![PathBuf::from(path(1))])),
            now(),
        );
        // The question now asks about the batch that took over.
        assert_eq!(m.question(), Some(Question::Quit(scan_on(0, 1, 1))));
        // r's batch ends and s's cannot start: nothing is left to stop, so the question goes.
        update(
            &mut m,
            Input::ScanEnded(Ok(scan::Summary::default())),
            now(),
        );
        assert_eq!(m.confirm, Some(Confirm::Quit));
        update(
            &mut m,
            Input::Started {
                job: Err("run s: no LLM is called gpt".into()),
                missing: vec![],
                paths: vec![path(2)],
            },
            now(),
        );
        assert_eq!(m.confirm, None);
        // s's files failed with the reason: the outro does not list them as left unchecked.
        assert!(
            !m.progress
                .outro()
                .iter()
                .any(|l| l.starts_with("Not checked again"))
        );
        act(&mut m, Action::Quit);
        assert!(m.should_quit);
    }

    #[test]
    fn a_quit_question_stays_when_a_queued_batch_cannot_start_and_another_goes() {
        let mut m = scanning();
        for (ep, run) in [(1, "r"), (2, "s")] {
            m.confirm = Some(Confirm::Recheck {
                path: path(ep),
                run: run.into(),
                backend: "claude".into(),
            });
            assert_eq!(act(&mut m, Action::Answer(true)), []);
        }
        act(&mut m, Action::Quit);
        update(
            &mut m,
            Input::ScanEnded(Ok(scan::Summary::default())),
            now(),
        );
        let effects = update(
            &mut m,
            Input::Started {
                job: Err("run r: no LLM is called gpt".into()),
                missing: vec![],
                paths: vec![path(1)],
            },
            now(),
        );
        assert_eq!(
            effects,
            [Effect::Task(Task::Recheck {
                run: "s".into(),
                paths: vec![path(2)],
            })]
        );
        assert_eq!(m.confirm, Some(Confirm::Quit));
        let job = Job {
            run: "s".into(),
            resume: "subcheck scan --run s --force /m/E02.mkv".into(),
        };
        update(
            &mut m,
            Input::Started {
                job: Ok(job),
                missing: vec![],
                paths: vec![path(2)],
            },
            now(),
        );
        // A yes over it quits, stopping the batch that took over.
        act(&mut m, Action::Answer(true));
        assert!(m.should_quit && m.progress.running());
    }

    #[test]
    fn a_fatal_message_ends_the_screen_with_it() {
        let mut m = scanning();
        update(
            &mut m,
            Input::Fatal("the scan panicked: boom".into()),
            now(),
        );
        assert!(m.should_quit);
        assert_eq!(m.fatal.as_deref(), Some("the scan panicked: boom"));
    }

    #[test]
    fn a_decision_on_a_group_covers_every_member_and_moves_on() {
        let mut m = sample();
        let effects = act(&mut m, Action::Accept);
        assert_eq!(
            decided(&effects),
            [
                (1, State::Accepted, None),
                (2, State::Accepted, None),
                (4, State::Accepted, None)
            ]
        );
        written(&mut m, effects);
        assert_eq!(
            states(&m),
            [
                State::Accepted,
                State::Accepted,
                State::Pending,
                State::Accepted
            ]
        );
        assert_eq!(m.status, Some(was_decided(State::Accepted, 3, 0, None)));
        // The decided row stays in the list; the selection moved on to the next.
        assert_eq!(m.entries(), [Entry::Row(0), Entry::Row(1)]);
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)));
    }

    #[test]
    fn under_all_the_selection_moves_past_the_decided_row() {
        let mut m = sample();
        m.filter = Filter::All;
        let effects = act(&mut m, Action::Reject);
        written(&mut m, effects);
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)));
        // Nothing left to change: no write.
        m.selected = 0;
        assert_eq!(act(&mut m, Action::Reject), []);
        assert_eq!(
            m.status,
            Some(Status::Refused(Refusal::Already {
                state: State::Rejected
            }))
        );
    }

    #[test]
    fn after_a_decision_the_selection_skips_rows_already_decided() {
        let mut m = crate::review::model::tests_support::many(4);
        m.selected = 1;
        let effects = act(&mut m, Action::Reject);
        written(&mut m, effects);
        assert_eq!(m.selected, 2, "E03, the next row");
        m.selected = 0;
        let effects = act(&mut m, Action::Accept);
        written(&mut m, effects);
        assert_eq!(m.selected, 2, "past E02, decided");
        // Nothing pending below the last row: the selection stays on it.
        m.selected = 3;
        let effects = act(&mut m, Action::Accept);
        written(&mut m, effects);
        assert_eq!(m.selected, 3);
    }

    #[test]
    fn under_a_decided_filter_a_decision_moves_to_the_row_that_takes_its_place() {
        let mut m = crate::review::model::tests_support::many(4);
        for id in 1..=4 {
            m.run.decisions.insert(
                id,
                Decision {
                    finding: id,
                    state: State::Rejected,
                    edited: None,
                    at: now(),
                },
            );
        }
        m.filter = Filter::Only(State::Rejected);
        m.selected = 1;
        let effects = act(&mut m, Action::Accept);
        written(&mut m, effects);
        // Nothing here is pending: E02 left, and E03 took its place.
        assert_eq!(m.selected_entry(), Some(Entry::Row(2)));
    }

    #[test]
    fn deciding_a_group_member_by_member_lands_on_the_next_row() {
        // A third row, so a selection that runs one row too far shows.
        let mut run = crate::review::model::tests_support::run();
        run.findings
            .push(crate::review::model::tests_support::finding(
                5,
                3,
                "A later line.",
                "A later line!",
                1_000_000,
            ));
        let mut m = Review::new(run, std::collections::HashMap::new());
        assert_eq!(m.rows.len(), 3);
        act(&mut m, Action::Toggle);
        m.selected = 1;
        for _ in 0..3 {
            let effects = act(&mut m, Action::Accept);
            written(&mut m, effects);
        }
        // The last member took its row with it; the row after the group is next, not skipped.
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)));
    }

    #[test]
    fn a_timing_fix_is_accepted_or_rejected_not_edited() {
        let mut m = crate::review::model::tests_support::with_timing();
        m.selected = 1;
        assert_eq!(act(&mut m, Action::Edit), []);
        assert_eq!(m.editing, None);
        assert_eq!(m.status, Some(Status::Refused(Refusal::TimingFix)));
        // Accepting the row accepts each member's own fix.
        assert_eq!(
            decided(&act(&mut m, Action::Accept)),
            [
                (11, State::Accepted, None),
                (12, State::Accepted, None),
                (13, State::Accepted, None)
            ]
        );
    }

    #[test]
    fn accepting_over_a_rewording_says_the_wording_went() {
        let mut m = sample();
        m.selected = 1;
        act(&mut m, Action::Edit);
        let effects = act(&mut m, Action::Reword("Snacks every day!".into()));
        written(&mut m, effects);
        assert_eq!(m.status, Some(was_decided(State::Accepted, 1, 0, None)));
        m.filter = Filter::All;
        m.selected = 1;
        let effects = act(&mut m, Action::Accept);
        assert_eq!(decided(&effects), [(3, State::Accepted, None)]);
        written(&mut m, effects);
        assert_eq!(m.status, Some(was_decided(State::Accepted, 1, 1, None)));
    }

    #[test]
    fn undo_puts_back_every_member_and_selects_the_row() {
        let mut m = sample();
        let effects = act(&mut m, Action::Accept);
        written(&mut m, effects);
        let effects = act(&mut m, Action::Undo);
        assert_eq!(
            decided(&effects),
            [
                (1, State::Pending, None),
                (2, State::Pending, None),
                (4, State::Pending, None)
            ]
        );
        written(&mut m, effects);
        assert_eq!(states(&m), [State::Pending; 4]);
        assert_eq!(m.selected_entry(), Some(Entry::Row(0)));
        assert_eq!(
            m.status,
            Some(Status::Undone {
                findings: 3,
                labels: vec!["E01".into(), "E02".into(), "E03".into()],
            })
        );
        assert_eq!(act(&mut m, Action::Undo), []);
        assert_eq!(m.status, Some(Status::Refused(Refusal::NothingToUndo)));
    }

    #[test]
    fn x_clears_an_entry_back_to_pending_and_u_puts_it_back() {
        let mut m = sample();
        let effects = act(&mut m, Action::Accept);
        written(&mut m, effects);
        m.selected = 0;
        let effects = act(&mut m, Action::Clear);
        assert_eq!(
            decided(&effects),
            [
                (1, State::Pending, None),
                (2, State::Pending, None),
                (4, State::Pending, None)
            ]
        );
        written(&mut m, effects);
        assert_eq!(states(&m), [State::Pending; 4]);
        assert_eq!(m.status, Some(Status::Cleared { findings: 3 }));
        assert_eq!(
            m.selected_entry(),
            Some(Entry::Row(1)),
            "on to the next pending row, as after a and r"
        );
        assert_eq!(act(&mut m, Action::Clear), []);
        assert_eq!(m.status, Some(Status::Refused(Refusal::NothingToClear)));
        let effects = act(&mut m, Action::Undo);
        assert_eq!(
            decided(&effects),
            [
                (1, State::Accepted, None),
                (2, State::Accepted, None),
                (4, State::Accepted, None)
            ]
        );
    }

    #[test]
    fn x_on_a_member_clears_it_alone_and_on_a_row_only_the_decided() {
        let mut m = sample();
        let effects = act(&mut m, Action::Accept);
        written(&mut m, effects);
        m.selected = 0;
        act(&mut m, Action::Toggle);
        m.selected = 2;
        assert_eq!(m.selected_entry(), Some(Entry::Member(0, 1)));
        let effects = act(&mut m, Action::Clear);
        assert_eq!(decided(&effects), [(2, State::Pending, None)]);
        written(&mut m, effects);
        m.selected = 0;
        assert_eq!(
            decided(&act(&mut m, Action::Clear)),
            [(1, State::Pending, None), (4, State::Pending, None)]
        );
    }

    #[test]
    fn undo_restores_an_earlier_wording() {
        let mut m = sample();
        m.selected = 1;
        act(&mut m, Action::Edit);
        let effects = act(&mut m, Action::Reword("Snacks every day!".into()));
        written(&mut m, effects);
        m.filter = Filter::All;
        m.selected = 1;
        let effects = act(&mut m, Action::Reject);
        written(&mut m, effects);
        let effects = act(&mut m, Action::Undo);
        assert_eq!(
            decided(&effects),
            [(3, State::Accepted, Some("Snacks every day!"))]
        );
    }

    #[test]
    fn a_member_is_decided_alone_and_the_row_decides_the_rest() {
        let mut m = sample();
        m.filter = Filter::Only(State::Pending);
        act(&mut m, Action::Toggle);
        assert_eq!(
            m.entries(),
            [
                Entry::Row(0),
                Entry::Member(0, 0),
                Entry::Member(0, 1),
                Entry::Member(0, 3),
                Entry::Row(1)
            ]
        );
        m.selected = 2;
        let effects = act(&mut m, Action::Reject);
        assert_eq!(decided(&effects), [(2, State::Rejected, None)]);
        written(&mut m, effects);
        // E02's member left the pending list; E03's took its place.
        assert_eq!(m.selected_entry(), Some(Entry::Member(0, 3)));
        assert_eq!(m.row_state(0), None, "mixed");
        m.selected = 0;
        let effects = act(&mut m, Action::Accept);
        assert_eq!(
            decided(&effects),
            [(1, State::Accepted, None), (4, State::Accepted, None)],
            "only the members the filter shows"
        );
    }

    #[test]
    fn a_row_decision_keeps_a_member_decided_alone() {
        let mut m = sample();
        m.filter = Filter::All;
        act(&mut m, Action::Toggle);
        m.selected = 2;
        let effects = act(&mut m, Action::Reject);
        written(&mut m, effects);
        // The row shows E02's rejection too, but `Accept` on it decides only the pending members.
        m.selected = 0;
        let effects = act(&mut m, Action::Accept);
        assert_eq!(
            decided(&effects),
            [(1, State::Accepted, None), (4, State::Accepted, None)]
        );
        written(&mut m, effects);
        let kept = Kept {
            findings: 1,
            state: State::Rejected,
        };
        assert_eq!(
            m.status,
            Some(was_decided(State::Accepted, 2, 0, Some(kept)))
        );
        // None pending: the row flips as a whole.
        m.selected = 0;
        let effects = act(&mut m, Action::Reject);
        assert_eq!(
            decided(&effects),
            [(1, State::Rejected, None), (4, State::Rejected, None)]
        );
        written(&mut m, effects);
        assert_eq!(m.status, Some(was_decided(State::Rejected, 2, 0, None)));
    }

    #[test]
    fn editing_a_row_rewords_its_pending_members() {
        let mut run = crate::review::model::tests_support::run();
        run.findings[1].original = "Let's have snacks together everyday.".into();
        run.findings[1].suggestion = "Let's have snacks together every day.".into();
        run.findings[1].occurrences[0].start_ms = 894_190;
        let mut m = Review::new(run, std::collections::HashMap::new());
        m.filter = Filter::All;
        // E02's two "everyday" findings are one row; reject the first alone.
        let row = m.rows.iter().position(|r| r.members == [1, 2]).unwrap();
        m.expanded.insert(row);
        m.selected = m
            .entries()
            .iter()
            .position(|&e| e == Entry::Member(row, 1))
            .unwrap();
        let effects = act(&mut m, Action::Reject);
        written(&mut m, effects);
        m.selected = m
            .entries()
            .iter()
            .position(|&e| e == Entry::Row(row))
            .unwrap();
        act(&mut m, Action::Edit);
        assert_eq!(
            decided(&act(&mut m, Action::Reword("Snacks every day!".into()))),
            [(3, State::Accepted, Some("Snacks every day!"))]
        );
    }

    #[test]
    fn an_edit_equal_to_the_suggestion_is_a_plain_accept() {
        let mut m = sample();
        m.selected = 1;
        act(&mut m, Action::Edit);
        let effects = act(
            &mut m,
            Action::Reword("Let's have snacks together every day.".into()),
        );
        assert_eq!(decided(&effects), [(3, State::Accepted, None)]);
    }

    #[test]
    fn an_empty_edit_is_refused_and_esc_cancels() {
        let mut m = sample();
        act(&mut m, Action::Edit);
        assert_eq!(act(&mut m, Action::Reword(String::new())), []);
        assert!(m.editing.is_some());
        assert_eq!(m.status, Some(Status::Refused(Refusal::EmptyWording)));
        act(&mut m, Action::CancelEdit);
        assert!(m.editing.is_none());
        assert_eq!(states(&m), [State::Pending; 4]);
    }

    // An edit none of whose findings is left, which only a UI that set it itself can hold:
    // nothing is decided, and the edit ends.
    #[test]
    fn a_reword_with_no_finding_left_decides_nothing_and_says_so() {
        let mut m = sample();
        m.editing = Some(Edit {
            entry: EntryKey::Member(99),
            findings: vec![99],
        });
        assert_eq!(act(&mut m, Action::Reword("Fixed.".into())), []);
        assert_eq!(m.editing, None);
        assert_eq!(m.status, Some(Status::Refused(Refusal::EditOutOfDate)));
    }

    #[test]
    fn a_files_findings_the_filter_hides_are_not_shown_and_the_status_line_says_so() {
        let mut m = sample();
        m.filter = Filter::Only(State::Rejected);
        m.pane = Pane::Files;
        assert_eq!(act(&mut m, Action::ShowFileFindings), []);
        assert_eq!(m.pane, Pane::Files);
        assert_eq!(
            m.status,
            Some(Status::Refused(Refusal::NoFindingsShown {
                label: "E01".into()
            }))
        );
    }

    #[test]
    fn a_failed_write_changes_nothing() {
        let mut m = sample();
        let Some(Effect::Task(Task::Write { purpose, decisions })) =
            act(&mut m, Action::Accept).pop()
        else {
            panic!("expected a write");
        };
        update(
            &mut m,
            Input::Written {
                purpose,
                decisions,
                result: Err("disk full".into()),
            },
            now(),
        );
        assert_eq!(states(&m), [State::Pending; 4]);
        assert_eq!(m.undo.len(), 0);
        assert_eq!(
            m.status,
            Some(Status::Failed {
                what: Failure::Save,
                error: "disk full".into(),
            })
        );
    }

    #[test]
    fn the_filter_cycles_and_keeps_the_selected_row() {
        let mut m = sample();
        assert_eq!(m.filter, Filter::All);
        m.selected = 1;
        act(&mut m, Action::SetFilter(Filter::Only(State::Pending)));
        assert_eq!(m.filter, Filter::Only(State::Pending));
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)));
        act(&mut m, Action::SetFilter(Filter::Only(State::Accepted)));
        assert_eq!(m.filter, Filter::Only(State::Accepted));
        assert_eq!(m.entries(), []);
        act(&mut m, Action::SetFilter(Filter::Only(State::Rejected)));
        act(&mut m, Action::SetFilter(Filter::All));
        assert_eq!(m.filter, Filter::All);
    }

    #[test]
    fn review_opens_on_every_finding_at_the_first_pending_row() {
        let accepted = |id| Decision {
            finding: id,
            state: State::Accepted,
            edited: None,
            at: now(),
        };
        let opened = |ids: &[u64]| {
            let mut run = crate::review::model::tests_support::run();
            for &id in ids {
                run.decisions.insert(id, accepted(id));
            }
            Review::new(run, std::collections::HashMap::new())
        };
        let m = opened(&[1, 2, 4]);
        assert_eq!(m.entries(), [Entry::Row(0), Entry::Row(1)]);
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)));
        // Nothing pending: the top.
        assert_eq!(opened(&[1, 2, 3, 4]).selected, 0);
    }

    #[test]
    fn n_and_p_jump_between_files() {
        let mut m = sample();
        act(&mut m, Action::NextFile);
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)));
        act(&mut m, Action::NextFile);
        assert_eq!(m.status, Some(Status::Refused(Refusal::LastFile)));
        act(&mut m, Action::PreviousFile);
        assert_eq!(m.selected_entry(), Some(Entry::Row(0)));
        act(&mut m, Action::PreviousFile);
        assert_eq!(m.status, Some(Status::Refused(Refusal::FirstFile)));
        // Expanded, the group's members are in their own files.
        act(&mut m, Action::Toggle);
        act(&mut m, Action::NextFile);
        assert_eq!(m.selected_entry(), Some(Entry::Member(0, 1)));
        act(&mut m, Action::Select(EntryKey::Row(vec![3])));
        act(&mut m, Action::PreviousFile);
        assert_eq!(
            m.selected_entry(),
            Some(Entry::Member(0, 3)),
            "E02's row is after E03's member"
        );
    }

    #[test]
    fn enter_on_a_member_collapses_its_group() {
        let mut m = sample();
        act(&mut m, Action::Toggle);
        act(&mut m, Action::Select(EntryKey::Member(1)));
        act(&mut m, Action::Toggle);
        assert_eq!(m.entries(), [Entry::Row(0), Entry::Row(1)]);
        assert_eq!(m.selected_entry(), Some(Entry::Row(0)));
    }

    #[test]
    fn o_opens_a_line_on_its_first_frame_on_the_default_track() {
        let mut m = sample();
        let moment = Moment {
            file: path(1),
            start_us: 165_064_500,
            sid: Some(1),
        };
        let effects = act(&mut m, Action::Play);
        assert_eq!(effects, [Effect::Task(Task::Stamp(moment.clone()))]);
        assert_eq!(m.status, Some(opening("E01", 165_060)));
        assert_eq!(
            stat(&mut m, effects, Some(stamp(1))),
            open(moment.clone(), How::Load)
        );
        // Again on the same line: the same moment, so that mpv goes back to its first frame.
        let effects = act(&mut m, Action::Play);
        assert_eq!(
            stat(&mut m, effects, Some(stamp(1))),
            open(moment, How::Seek)
        );
    }

    /// The player command review sends once `Play`'s stat (the one task in `effects`)
    /// reads `stamp`.
    fn stat(m: &mut Review, effects: Vec<Effect>, stamp: Option<(u64, Timestamp)>) -> Vec<Effect> {
        let [Effect::Task(Task::Stamp(moment))] = <[Effect; 1]>::try_from(effects).unwrap() else {
            panic!("expected the stat of the file to open");
        };
        update(m, Input::Stamped { moment, stamp }, now())
    }

    /// The stamp of a file `size` bytes long.
    fn stamp(size: u64) -> (u64, Timestamp) {
        (size, now())
    }

    fn open(moment: Moment, how: How) -> Vec<Effect> {
        vec![Effect::Player(PlayerCommand::Open {
            moment,
            how,
            options: player::load_options(),
        })]
    }

    #[test]
    fn o_opens_a_timing_finding_on_the_frame_that_shows_it() {
        let mut m = crate::review::model::tests_support::with_timing();
        m.selected = 1;
        assert_eq!(
            act(&mut m, Action::Play),
            [Effect::Task(Task::Stamp(Moment {
                file: path(1),
                start_us: 763_763_000,
                sid: Some(1),
            }))]
        );
        assert_eq!(m.status, Some(opening("E01", 763_763)));
    }

    // With no frame that shows its error, it opens on its first moved line as a line opens: on
    // that line's first frame.
    #[test]
    fn a_timing_finding_with_no_frame_opens_on_its_first_moved_lines_first_frame() {
        let mut m = crate::review::model::tests_support::with_timing();
        for f in &mut m.run.findings {
            if let Some(t) = &mut f.timing {
                for s in &mut t.stretches {
                    s.frame_ms = None;
                }
            }
        }
        m.selected = 1;
        assert_eq!(
            act(&mut m, Action::Play),
            [Effect::Task(Task::Stamp(Moment {
                file: path(1),
                start_us: 1_004_500,
                sid: Some(1),
            }))]
        );
        assert_eq!(m.status, Some(opening("E01", 1_000)));
    }

    // mpv counts a time before 0:00 back from the end.
    #[test]
    fn a_line_before_the_start_opens_at_the_start() {
        let mut m = sample();
        m.run.findings[0].occurrences[0].start_ms = -35;
        let effects = act(&mut m, Action::Play);
        let [Effect::Task(Task::Stamp(moment))] = &effects[..] else {
            panic!("expected the stat: {effects:?}");
        };
        assert_eq!(moment.start_us, 0);
    }

    // The file review last had the player load, unchanged on disk since, is sought; a load
    // otherwise: another file, the same one changed, or one never loaded.
    #[test]
    fn the_same_file_unchanged_since_its_load_is_sought() {
        let mut m = sample();
        let how = |m: &mut Review, read: (u64, Timestamp)| {
            let effects = act(m, Action::Play);
            match &stat(m, effects, Some(read))[..] {
                [Effect::Player(PlayerCommand::Open { how, .. })] => *how,
                other => panic!("expected an open: {other:?}"),
            }
        };
        assert_eq!(how(&mut m, stamp(1)), How::Load);
        assert_eq!(how(&mut m, stamp(1)), How::Seek);
        // A remux in place: the same path, another size; or, one letter fixed, the same size and a
        // later mtime.
        assert_eq!(how(&mut m, stamp(2)), How::Load);
        assert_eq!(how(&mut m, stamp(2)), How::Seek);
        let later = (2, now() + jiff::SignedDuration::from_secs(1));
        assert_eq!(how(&mut m, later), How::Load);
        assert_eq!(how(&mut m, later), How::Seek);
        act(&mut m, Action::NextFile);
        assert_eq!(how(&mut m, later), How::Load);
        act(&mut m, Action::PreviousFile);
        assert_eq!(how(&mut m, later), How::Load);
    }

    // What review forgets lets the next `Play` load: any close, and any open that failed, at once
    // or later.
    #[test]
    fn review_forgets_its_load_once_the_player_closes_or_an_open_fails() {
        let gone = PlayerError::CouldNotOpen { file: path(1) };
        for (after, said) in [
            (Input::Closed(Ok(())), Status::Closed),
            (
                Input::Closed(Err(PlayerError::NotOpen)),
                Status::NotOpen {
                    asked: Asked::Close,
                },
            ),
            (Input::Opened(Err(PlayerError::NoAnswer)), Status::NoAnswer),
            (
                Input::PlayerFailed(gone),
                Status::CouldNotOpen {
                    label: "E01".into(),
                },
            ),
        ] {
            let mut m = sample();
            let effects = act(&mut m, Action::Play);
            stat(&mut m, effects, Some(stamp(1)));
            update(&mut m, Input::Opened(Ok(Opened { paused: true })), now());
            let effects = act(&mut m, Action::Play);
            assert!(matches!(
                stat(&mut m, effects, Some(stamp(1)))[..],
                [Effect::Player(PlayerCommand::Open { how: How::Seek, .. })]
            ));
            update(&mut m, after, now());
            assert_eq!(m.status.as_ref(), Some(&said));
            let effects = act(&mut m, Action::Play);
            assert!(
                matches!(
                    stat(&mut m, effects, Some(stamp(1)))[..],
                    [Effect::Player(PlayerCommand::Open { how: How::Load, .. })]
                ),
                "after {said:?}"
            );
        }
    }

    // As a file whose note says it is missing is not opened: mpv could not read it either.
    #[test]
    fn a_file_that_cannot_be_read_now_is_not_opened() {
        let mut m = sample();
        let effects = act(&mut m, Action::Play);
        stat(&mut m, effects, Some(stamp(1)));
        let effects = act(&mut m, Action::Play);
        assert_eq!(stat(&mut m, effects, None), []);
        assert_eq!(
            m.status,
            Some(Status::Refused(Refusal::FileMissing { path: path(1) }))
        );
        assert_eq!(m.loaded, None);
    }

    // Each report's message, with what was asked and the file by the list's name; an error with
    // text keeps its words.
    #[test]
    fn each_thing_a_player_reports_is_its_message() {
        let error = |e| Input::Opened(Err(e));
        let failed = |what, error: &str| Status::Failed {
            what,
            error: error.into(),
        };
        for (input, said) in [
            (
                Input::Opened(Ok(Opened { paused: true })),
                Status::Opened { paused: true },
            ),
            (
                Input::Opened(Ok(Opened { paused: false })),
                Status::Opened { paused: false },
            ),
            (
                error(PlayerError::NotOpen),
                Status::NotOpen { asked: Asked::Open },
            ),
            (error(PlayerError::NoFile), Status::NoFile),
            (error(PlayerError::StillOpening), Status::StillOpening),
            (
                error(PlayerError::CouldNotOpen { file: path(2) }),
                Status::CouldNotOpen {
                    label: "E02".into(),
                },
            ),
            (error(PlayerError::NoAnswer), Status::NoAnswer),
            (error(PlayerError::HungUp), Status::HungUp),
            (error(PlayerError::NoVideo), Status::NoVideo),
            (
                error(PlayerError::Mpv("invalid parameter".into())),
                failed(Failure::Mpv, "invalid parameter"),
            ),
            (
                error(PlayerError::Failed(
                    "mpv not found: install mpv to open findings".into(),
                )),
                failed(
                    Failure::Player,
                    "mpv not found: install mpv to open findings",
                ),
            ),
            (
                Input::Played(Err(PlayerError::NotOpen)),
                Status::NotOpen {
                    asked: Asked::Playback,
                },
            ),
            (
                Input::Closed(Err(PlayerError::NotOpen)),
                Status::NotOpen {
                    asked: Asked::Close,
                },
            ),
            (
                Input::PlayerFailed(PlayerError::CouldNotOpen { file: path(1) }),
                Status::CouldNotOpen {
                    label: "E01".into(),
                },
            ),
        ] {
            let mut m = sample();
            update(&mut m, input, now());
            assert_eq!(m.status, Some(said));
        }
    }

    #[test]
    fn a_timing_finding_with_no_line_of_its_track_to_open_says_so() {
        let mut m = crate::review::model::tests_support::with_timing();
        for f in &mut m.run.findings {
            if let Some(t) = &mut f.timing {
                for s in &mut t.stretches {
                    (s.first, s.last, s.frame_ms) = (None, None, None);
                }
            }
        }
        m.selected = 1;
        assert_eq!(act(&mut m, Action::Play), []);
        assert_eq!(m.status, Some(Status::Refused(Refusal::NothingToOpen)));
    }

    #[test]
    fn a_missing_file_is_not_opened() {
        let mut m = sample();
        m.notes.insert(path(1), FileNote::Missing);
        assert_eq!(act(&mut m, Action::Play), []);
        assert_eq!(
            m.status,
            Some(Status::Refused(Refusal::FileMissing { path: path(1) }))
        );
    }

    #[test]
    fn playback_keys_ask_mpv_and_show_where_it_is() {
        let mut m = sample();
        let playback = |p| vec![Effect::Player(PlayerCommand::Playback(p))];
        assert_eq!(
            act(&mut m, Action::Playback(Playback::Pause)),
            playback(Playback::Pause)
        );
        assert_eq!(
            act(&mut m, Action::Playback(Playback::Seek(-5))),
            playback(Playback::Seek(-5))
        );
        assert_eq!(
            act(&mut m, Action::Playback(Playback::Seek(5))),
            playback(Playback::Seek(5))
        );
        assert_eq!(
            act(&mut m, Action::Playback(Playback::Seek(-1))),
            playback(Playback::Seek(-1))
        );
        assert_eq!(
            act(&mut m, Action::Playback(Playback::Seek(1))),
            playback(Playback::Seek(1))
        );
        let at = |paused| PlayState {
            paused,
            at_ms: 171_060,
        };
        update(&mut m, Input::Played(Ok(at(true))), now());
        assert_eq!(m.status, Some(Status::Played(at(true))));
        update(&mut m, Input::Played(Ok(at(false))), now());
        assert_eq!(m.status, Some(Status::Played(at(false))));
        update(&mut m, Input::Played(Err(PlayerError::NotOpen)), now());
        assert_eq!(
            m.status,
            Some(Status::NotOpen {
                asked: Asked::Playback
            })
        );
    }

    #[test]
    fn capital_o_closes_mpv_and_says_so() {
        let mut m = sample();
        assert_eq!(
            act(&mut m, Action::ClosePlayer),
            [Effect::Player(PlayerCommand::Close)]
        );
        update(&mut m, Input::Closed(Ok(())), now());
        assert_eq!(m.status, Some(Status::Closed));
        update(&mut m, Input::Closed(Err(PlayerError::NotOpen)), now());
        assert_eq!(
            m.status,
            Some(Status::NotOpen {
                asked: Asked::Close
            })
        );
    }

    #[test]
    fn q_and_ctrl_c_quit() {
        let mut m = sample();
        act(&mut m, Action::Quit);
        assert!(m.should_quit);
        let mut m = sample();
        act(&mut m, Action::Interrupt);
        assert!(m.should_quit);
    }

    #[test]
    fn an_action_clears_the_status_message_but_an_answer_an_edits_end_and_ctrl_c_leave_it() {
        let said = || Some(Status::Opened { paused: false });
        for action in [
            Action::Select(EntryKey::Row(vec![3])),
            Action::Toggle,
            Action::ShowPane(Pane::Files),
            Action::Dismiss,
        ] {
            let mut m = sample();
            m.status = said();
            act(&mut m, action.clone());
            assert_eq!(m.status, None, "{action:?}");
        }
        let mut m = sample();
        m.selected = 1;
        act(&mut m, Action::Edit);
        m.status = said();
        act(&mut m, Action::CancelEdit);
        assert_eq!(m.status, said(), "CancelEdit");
        act(&mut m, Action::Edit);
        assert_eq!(m.status, None, "Edit");
        m.status = said();
        assert_eq!(
            decided(&act(&mut m, Action::Reword("Snacks!".into()))),
            [(3, State::Accepted, Some("Snacks!"))]
        );
        assert_eq!(m.status, said(), "Reword");
        m.confirm = Some(Confirm::Apply);
        act(&mut m, Action::Answer(false));
        assert_eq!(m.status, said(), "Answer");
        act(&mut m, Action::Interrupt);
        assert!(m.should_quit);
        assert_eq!(m.status, said(), "Interrupt");
    }

    #[test]
    fn a_move_steps_through_the_list_as_shown_and_stops_at_its_ends() {
        let mut m = tests_support::many(30);
        m.selected = 15;
        for (by, to) in [
            (1, 16),
            (-10, 6),
            (-10, 0),
            (-1, 0),
            (10, 10),
            (i32::MAX, 29),
            (1, 29),
            (i32::MIN, 0),
        ] {
            assert_eq!(act(&mut m, Action::Move(by)), []);
            assert_eq!(m.selected, to, "by {by}");
        }
    }

    #[test]
    fn a_move_counts_an_expanded_rows_members_as_entries() {
        let mut m = sample();
        act(&mut m, Action::Toggle);
        act(&mut m, Action::Move(1));
        assert_eq!(m.selected_entry(), Some(Entry::Member(0, 0)));
        act(&mut m, Action::Move(3));
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)));
    }

    #[test]
    fn a_move_at_an_end_or_in_an_empty_list_still_lets_a_message_go() {
        let mut m = sample();
        m.selected = 1;
        m.status = Some(Status::Closed);
        act(&mut m, Action::Move(1));
        assert_eq!((m.selected, &m.status), (1, &None));
        // Nothing in the sample is rejected: under that filter the list is empty.
        act(&mut m, Action::SetFilter(Filter::Only(State::Rejected)));
        assert_eq!(m.entries(), []);
        m.status = Some(Status::Closed);
        act(&mut m, Action::Move(-1));
        assert_eq!((m.selected, &m.status), (0, &None));
    }

    #[test]
    fn selecting_an_entry_that_went_selects_nothing_and_says_so() {
        let mut m = sample();
        // From the snacks row, E02's lyric while its row is collapsed: its row, with nothing said.
        act(&mut m, Action::Select(EntryKey::Row(vec![3])));
        act(&mut m, Action::Select(EntryKey::Member(2)));
        assert_eq!(m.selected_entry(), Some(Entry::Row(0)));
        assert_eq!(m.status, None);
        // The lyric's row expanded with its E03 member selected, and E02's file: neither at 0, so
        // a selection that moved would show.
        act(&mut m, Action::Toggle);
        act(&mut m, Action::Select(EntryKey::Member(4)));
        act(&mut m, Action::SelectFile(path(2)));
        assert_eq!(m.selected_entry(), Some(Entry::Member(0, 3)));
        assert_eq!(m.file_selected, 1);
        let snacks = m.entry_key(Entry::Row(1));
        // E02 checked again without its snacks line: that row went.
        let view = Run {
            files: vec![tests_support::file(2)],
            findings: vec![finding(2, 2, TYPO, FIXED, 165_060)],
            ..Run::default()
        };
        update(
            &mut m,
            Input::ReadBack {
                path: path(2),
                view: Ok(view),
            },
            now(),
        );
        // E03's member stays selected, its finding now at index 1.
        let e03 = Some(Entry::Member(0, 1));
        assert_eq!(m.selected_entry(), e03);
        assert_eq!(act(&mut m, Action::Select(snacks)), []);
        assert_eq!(m.selected_entry(), e03);
        assert_eq!(m.status, Some(Status::Refused(Refusal::SelectionStays)));
        // A file of another view, likewise.
        let elsewhere = "/m/Other/Other - E07.mkv".to_string();
        assert_eq!(act(&mut m, Action::SelectFile(elsewhere)), []);
        assert_eq!(m.file_selected, 1);
        assert_eq!(
            m.status,
            Some(Status::Refused(Refusal::NotInView {
                label: "E07".into()
            }))
        );
    }

    #[test]
    fn a_file_the_scan_saved_is_read_back_and_one_it_failed_keeps_its_findings() {
        let mut m = sample();
        let effects = update(
            &mut m,
            appended(2, vec![finding(9, 2, TYPO, FIXED, 165_060)]),
            now(),
        );
        assert_eq!(effects, [Effect::Task(Task::ReadBack(path(2)))]);
        assert_eq!(
            m.run.findings.len(),
            4,
            "its findings come with the read-back"
        );
        let failed = crate::store::FileRecord {
            error: Some("LLM: boom".into()),
            ..tests_support::file(2)
        };
        let effects = update(
            &mut m,
            Input::Scan(Event::Appended(vec![Record::File(failed)])),
            now(),
        );
        assert_eq!(effects, []);
        assert_eq!(m.run.findings.len(), 4);
        assert_eq!(m.run.files[1].error.as_deref(), Some("LLM: boom"));
    }

    #[test]
    fn a_decision_the_store_found_replaced_reads_its_file_back_and_says_so() {
        let mut m = sample();
        m.selected = 1;
        let effects = act(&mut m, Action::Accept);
        let [Effect::Task(Task::Write { purpose, decisions })] =
            <[Effect; 1]>::try_from(effects).unwrap()
        else {
            panic!("expected one write");
        };
        let followed = Input::Written {
            purpose: purpose.clone(),
            decisions: decisions.clone(),
            result: Ok(vec![Decided::Followed(vec![9])]),
        };
        assert_eq!(
            update(&mut m, followed, now()),
            [Effect::Task(Task::ReadBack(path(2)))]
        );
        assert_eq!(
            m.status,
            Some(Status::Replaced {
                label: "E02".into(),
                gone: false
            })
        );
        let gone = Input::Written {
            purpose,
            decisions,
            result: Ok(vec![Decided::Gone]),
        };
        assert_eq!(
            update(&mut m, gone, now()),
            [Effect::Task(Task::ReadBack(path(2)))]
        );
        assert_eq!(
            m.status,
            Some(Status::Replaced {
                label: "E02".into(),
                gone: true
            })
        );
    }

    #[test]
    fn a_file_that_cannot_be_read_back_says_so_and_stays_as_it_was() {
        let mut m = sample();
        let effects = update(
            &mut m,
            Input::ReadBack {
                path: path(2),
                view: Err("database is locked".into()),
            },
            now(),
        );
        assert_eq!(effects, []);
        assert_eq!(
            m.status,
            Some(Status::Failed {
                what: Failure::ReadBack {
                    label: "E02".into()
                },
                error: "database is locked".into(),
            })
        );
        assert_eq!(m.run.findings.len(), 4);
    }

    /// The sample run's files view, on E02.
    fn on_e02_in_files() -> Review {
        let mut m = sample();
        act(&mut m, Action::ShowPane(Pane::Files));
        m.file_selected = 1;
        m
    }

    #[test]
    fn c_on_a_file_finds_its_run_then_asks() {
        let mut m = on_e02_in_files();
        assert_eq!(
            act(&mut m, Action::Recheck),
            [Effect::Task(Task::FindRun {
                path: path(2),
                run: Some("2026-10-17T04-39-18".into()),
            })]
        );
        let found = Input::RunFound {
            path: path(2),
            found: Ok(("2026-10-17T04-39-18".into(), "claude".into())),
        };
        update(&mut m, found, now());
        assert_eq!(
            m.question(),
            Some(Question::Recheck {
                label: "E02".into(),
                backend: "claude".into(),
            })
        );
        // A no lets it go.
        assert_eq!(act(&mut m, Action::Answer(false)), []);
        assert_eq!(m.confirm, None);
        // A paths view has no run of its own: the store finds the newest that named the file.
        let mut m = on_e02_in_files();
        m.run.of_paths = true;
        assert_eq!(
            act(&mut m, Action::Recheck),
            [Effect::Task(Task::FindRun {
                path: path(2),
                run: None,
            })]
        );
        // None found: no question, and why, in the runner's words.
        let none = Input::RunFound {
            path: path(2),
            found: Err("E02: no run named it".into()),
        };
        update(&mut m, none, now());
        assert_eq!(m.confirm, None);
        assert_eq!(
            m.status,
            Some(Status::Failed {
                what: Failure::Recheck,
                error: "E02: no run named it".into(),
            })
        );
    }

    #[test]
    fn y_with_no_scan_running_checks_the_file_again_at_once() {
        let mut m = on_e02_in_files();
        m.confirm = Some(Confirm::Recheck {
            path: path(2),
            run: "r".into(),
            backend: "claude".into(),
        });
        assert_eq!(
            act(&mut m, Action::Answer(true)),
            [Effect::Task(Task::Recheck {
                run: "r".into(),
                paths: vec![path(2)],
            })]
        );
        let job = Job {
            run: "r".into(),
            resume: "subcheck scan --run r --force /m/E02.mkv".into(),
        };
        update(
            &mut m,
            Input::Started {
                job: Ok(job),
                missing: vec![],
                paths: vec![path(2)],
            },
            now(),
        );
        assert!(m.progress.running());
        assert_eq!(m.progress.run, "r");
    }

    #[test]
    fn y_while_a_scan_runs_queues_the_file_and_the_queue_starts_a_run_at_a_time_when_it_ends() {
        let mut m = scanning();
        for (ep, run) in [(1, "r"), (2, "s"), (3, "r")] {
            m.confirm = Some(Confirm::Recheck {
                path: path(ep),
                run: run.into(),
                backend: "claude".into(),
            });
            assert_eq!(act(&mut m, Action::Answer(true)), []);
        }
        assert_eq!(
            m.status,
            Some(Status::QueuedAgain {
                label: "E03".into()
            })
        );
        let labels: Vec<String> = m.progress.files[..3].iter().map(|f| f.label('⠙')).collect();
        assert_eq!(labels, ["queued again"; 3]);
        let ended = update(
            &mut m,
            Input::ScanEnded(Ok(scan::Summary::default())),
            now(),
        );
        assert_eq!(
            ended,
            [Effect::Task(Task::Recheck {
                run: "r".into(),
                paths: vec![path(1), path(3)],
            })]
        );
        assert_eq!(m.progress.again, [(path(2), "s".to_string())]);
    }

    #[test]
    fn c_on_a_file_being_checked_or_queued_again_says_so() {
        let mut m = scanning();
        act(&mut m, Action::ShowPane(Pane::Files));
        m.file_selected = 3;
        assert_eq!(m.progress.files[3].path, path(4));
        assert_eq!(act(&mut m, Action::Recheck), []);
        assert_eq!(
            m.status,
            Some(Status::Refused(Refusal::BeingChecked {
                label: "E04".into()
            }))
        );
        m.file_selected = 1;
        m.progress.queue_again(&path(2), "r");
        assert_eq!(act(&mut m, Action::Recheck), []);
        assert_eq!(
            m.status,
            Some(Status::QueuedAgain {
                label: "E02".into()
            })
        );
    }

    #[test]
    fn a_file_no_longer_on_disk_is_not_checked_and_the_status_line_says_so() {
        let mut m = on_e02_in_files();
        let effects = update(
            &mut m,
            Input::Started {
                job: Err("nothing to check".into()),
                missing: vec![path(2)],
                paths: vec![path(2)],
            },
            now(),
        );
        assert_eq!(effects, []);
        assert_eq!(
            m.status,
            Some(Status::NoSuchFile {
                label: "E02".into()
            })
        );
        assert_eq!(m.progress.files[1].label('⠙'), "✗ FAILED: no such file");
        assert!(!m.progress.running());
    }

    #[test]
    fn a_queued_check_that_cannot_start_fails_its_files_and_the_next_run_goes() {
        let mut m = scanning();
        for (ep, run) in [(1, "r"), (2, "s")] {
            m.confirm = Some(Confirm::Recheck {
                path: path(ep),
                run: run.into(),
                backend: "claude".into(),
            });
            assert_eq!(act(&mut m, Action::Answer(true)), []);
        }
        let ended = update(
            &mut m,
            Input::ScanEnded(Ok(scan::Summary::default())),
            now(),
        );
        assert_eq!(
            ended,
            [Effect::Task(Task::Recheck {
                run: "r".into(),
                paths: vec![path(1)],
            })]
        );
        let why = "run r: no LLM is called gpt";
        let effects = update(
            &mut m,
            Input::Started {
                job: Err(why.into()),
                missing: vec![],
                paths: vec![path(1)],
            },
            now(),
        );
        assert_eq!(
            effects,
            [Effect::Task(Task::Recheck {
                run: "s".into(),
                paths: vec![path(2)],
            })]
        );
        assert_eq!(m.progress.files[0].label('⠙'), format!("✗ FAILED: {why}"));
        assert!(
            !m.progress.outro().iter().any(|l| l.contains("FAILED")),
            "the earlier scan's outro does not list a file its recheck never started"
        );
        assert_eq!(
            m.status,
            Some(Status::Failed {
                what: Failure::Recheck,
                error: why.into(),
            })
        );
        act(&mut m, Action::ShowPane(Pane::Files));
        m.file_selected = 0;
        assert!(matches!(
            act(&mut m, Action::Recheck).as_slice(),
            [Effect::Task(Task::FindRun { .. })]
        ));
    }

    #[test]
    fn ctrl_c_over_cs_question_asks_to_quit_first() {
        let mut m = scanning();
        m.confirm = Some(Confirm::Recheck {
            path: path(1),
            run: "r".into(),
            backend: "claude".into(),
        });
        act(&mut m, Action::Interrupt);
        assert!(!m.should_quit);
        assert_eq!(m.confirm, Some(Confirm::Quit));
        act(&mut m, Action::Interrupt);
        assert!(m.should_quit);
    }

    #[test]
    fn v_asks_for_a_runs_folders_or_the_run_that_named_the_selected_file() {
        let mut m = sample();
        assert_eq!(
            act(&mut m, Action::SwitchView),
            [Effect::Task(Task::Switch(Switch::FoldersOf(
                m.run.name.clone()
            )))]
        );
        // In a paths view, the selected row's first member's file.
        m.run.of_paths = true;
        m.selected = 0;
        assert_eq!(
            act(&mut m, Action::SwitchView),
            [Effect::Task(Task::Switch(Switch::RunNaming(path(1))))]
        );
        // In the files view, the selected file.
        act(&mut m, Action::ShowPane(Pane::Files));
        m.file_selected = 2;
        assert_eq!(
            act(&mut m, Action::SwitchView),
            [Effect::Task(Task::Switch(Switch::RunNaming(path(3))))]
        );
    }

    #[test]
    fn v_with_nothing_selected_in_the_list_follows_the_files_selected_file() {
        let mut m = sample();
        m.run.of_paths = true;
        // Nothing in the sample is rejected: under that filter the list is empty.
        m.filter = Filter::Only(State::Rejected);
        m.file_selected = 1;
        assert_eq!(m.selected_entry(), None);
        assert_eq!(
            act(&mut m, Action::SwitchView),
            [Effect::Task(Task::Switch(Switch::RunNaming(path(2))))]
        );
    }

    #[test]
    fn v_while_a_scan_runs_says_so_and_asks_for_nothing() {
        let mut m = scanning();
        assert_eq!(act(&mut m, Action::SwitchView), []);
        assert_eq!(m.status, Some(Status::Refused(Refusal::ScanRunning)));
    }

    #[test]
    fn open_view_asks_for_the_view_its_words_select_but_not_while_a_job_runs() {
        let words = vec!["r".to_string(), "/m".to_string()];
        let mut m = sample();
        assert_eq!(
            act(&mut m, Action::OpenView(words.clone())),
            [Effect::Task(Task::Switch(Switch::Selector(words.clone())))]
        );
        let mut m = scanning();
        assert_eq!(act(&mut m, Action::OpenView(words.clone())), []);
        assert_eq!(m.status, Some(Status::Refused(Refusal::ScanRunning)));
        let mut m = accepted();
        act(&mut m, Action::Apply { dry_run: true });
        assert_eq!(act(&mut m, Action::OpenView(words.clone())), []);
        assert_eq!(m.status, Some(Status::Refused(Refusal::ApplyRunning)));
        // Under a question it only lets the question go, as every action but an answer does.
        let mut m = sample();
        m.confirm = Some(Confirm::Apply);
        assert_eq!(act(&mut m, Action::OpenView(words)), []);
        assert_eq!(m.confirm, None);
    }

    // Its path and stamp say what mpv has open whatever the view: the next `Play` on it seeks.
    #[test]
    fn another_view_keeps_the_file_the_player_last_loaded() {
        let mut m = sample();
        m.loaded = Some((path(1), (1, now())));
        let view = tests_support::run();
        update(&mut m, Input::Switched(Ok((view, HashMap::new()))), now());
        assert_eq!(m.loaded, Some((path(1), (1, now()))));
    }

    #[test]
    fn a_view_that_cannot_be_read_leaves_the_screen_as_it_was() {
        let mut m = sample();
        let before = m.run.clone();
        update(&mut m, Input::Switched(Err("disk I/O error".into())), now());
        assert_eq!(m.run, before);
        assert_eq!(
            m.status,
            Some(Status::Failed {
                what: Failure::Switch,
                error: "disk I/O error".into(),
            })
        );
    }

    /// The sample with findings 1 (E01) and 3 (E02) accepted, 3's fix in its file already.
    fn accepted() -> Review {
        let mut m = sample();
        for id in [1, 3] {
            m.run.decisions.insert(
                id,
                Decision {
                    finding: id,
                    state: State::Accepted,
                    edited: None,
                    at: now(),
                },
            );
        }
        m.run.applied.insert(3);
        m
    }

    /// Apply done with the file at `path`.
    fn file_done(
        path: String,
        outcomes: Vec<(u64, Outcome)>,
        marked: Vec<u64>,
        restamped: Option<(u64, Timestamp)>,
    ) -> Input {
        Input::Apply(apply::Event::Done {
            path,
            outcomes,
            remuxed: restamped.is_some(),
            marked,
            restamped,
            text: String::new(),
        })
    }

    #[test]
    fn a_asks_with_the_counts_then_applies_the_view_as_it_stands() {
        let mut m = accepted();
        assert_eq!(act(&mut m, Action::Apply { dry_run: false }), []);
        assert_eq!(
            m.question(),
            Some(Question::Apply {
                fixes: 2,
                files: 2,
                applied: 1,
            })
        );
        // A no lets it go.
        assert_eq!(act(&mut m, Action::Answer(false)), []);
        assert_eq!(m.confirm, None);
        assert!(!m.progress.busy());
        act(&mut m, Action::Apply { dry_run: false });
        let effects = act(&mut m, Action::Answer(true));
        assert_eq!(
            effects,
            [Effect::Task(Task::StartApply {
                run: m.run.clone(),
                dry_run: false,
                karaoke: false,
            })]
        );
        assert!(m.progress.applying());
        assert_eq!(
            m.progress.apply.as_ref().unwrap().resume,
            "subcheck apply 2026-10-17T04-39-18"
        );
    }

    #[test]
    fn d_starts_a_dry_run_at_once_with_the_screens_karaoke() {
        let mut m = accepted();
        m.karaoke = true;
        let effects = act(&mut m, Action::Apply { dry_run: true });
        assert_eq!(
            effects,
            [Effect::Task(Task::StartApply {
                run: m.run.clone(),
                dry_run: true,
                karaoke: true,
            })]
        );
        assert_eq!(
            m.progress.apply.as_ref().unwrap().resume,
            "subcheck apply 2026-10-17T04-39-18 --dry-run --karaoke"
        );
    }

    #[test]
    fn a_and_d_wait_for_the_scan_and_for_apply_and_need_an_accepted_fix() {
        let mut m = sample();
        assert_eq!(act(&mut m, Action::Apply { dry_run: false }), []);
        assert_eq!(m.status, Some(Status::Refused(Refusal::NoAcceptedFixes)));
        let mut m = accepted();
        m.progress
            .start("r", "subcheck scan --run r /m".into(), false);
        for dry_run in [false, true] {
            assert_eq!(act(&mut m, Action::Apply { dry_run }), []);
            assert_eq!(m.status, Some(Status::Refused(Refusal::ScanRunning)));
        }
        let mut m = accepted();
        act(&mut m, Action::Apply { dry_run: true });
        for dry_run in [false, true] {
            assert_eq!(act(&mut m, Action::Apply { dry_run }), []);
            assert_eq!(m.status, Some(Status::Refused(Refusal::AlreadyApplying)));
        }
        // From the files view too.
        act(&mut m, Action::ShowPane(Pane::Files));
        assert_eq!(act(&mut m, Action::Apply { dry_run: false }), []);
        assert_eq!(m.status, Some(Status::Refused(Refusal::AlreadyApplying)));
    }

    #[test]
    fn v_and_c_wait_for_apply_and_q_asks_while_it_runs() {
        let mut m = accepted();
        act(&mut m, Action::Apply { dry_run: true });
        assert_eq!(act(&mut m, Action::SwitchView), []);
        assert_eq!(m.status, Some(Status::Refused(Refusal::ApplyRunning)));
        act(&mut m, Action::ShowPane(Pane::Files));
        assert_eq!(act(&mut m, Action::Recheck), []);
        assert_eq!(m.status, Some(Status::Refused(Refusal::ApplyRunning)));
        act(&mut m, Action::Quit);
        assert_eq!(m.confirm, Some(Confirm::Quit));
        update(
            &mut m,
            Input::Apply(apply::Event::Queued(vec![path(1)])),
            now(),
        );
        update(
            &mut m,
            Input::Apply(apply::Event::Stage(path(1), apply::Stage::Plan)),
            now(),
        );
        assert_eq!(
            m.question(),
            Some(Question::Quit(applying_on(true, apply::Stage::Plan)))
        );
        // Apply ending lets the question go: nothing is left to stop.
        update(
            &mut m,
            Input::ApplyEnded(Ok(apply::Summary::default())),
            now(),
        );
        assert_eq!(m.confirm, None);
        assert!(!m.should_quit);
    }

    #[test]
    fn ctrl_c_or_ctrl_d_while_applying_asks_as_q_does_and_y_stops_it() {
        // Ctrl-C and Ctrl-D are both `Action::Interrupt` (`subcheck_cli::tui`'s `map_event`, then
        // its `screen::update`).
        let applying = || {
            let mut m = accepted();
            act(&mut m, Action::Apply { dry_run: false });
            act(&mut m, Action::Answer(true));
            update(
                &mut m,
                Input::Apply(apply::Event::Queued(vec![path(1)])),
                now(),
            );
            update(
                &mut m,
                Input::Apply(apply::Event::Stage(path(1), apply::Stage::Remux)),
                now(),
            );
            m
        };
        let mut m = applying();
        act(&mut m, Action::Interrupt);
        assert!(!m.should_quit);
        assert_eq!(
            m.question(),
            Some(Question::Quit(applying_on(false, apply::Stage::Remux)))
        );
        act(&mut m, Action::Answer(false));
        assert!(!m.should_quit && m.confirm.is_none());
        act(&mut m, Action::Interrupt);
        act(&mut m, Action::Answer(true));
        // Quitting while apply runs is what stops it: the loop that runs `update` ends its groups.
        assert!(m.should_quit && m.progress.applying());
        // A second one, with the question open, quits too.
        let mut m = applying();
        act(&mut m, Action::Interrupt);
        act(&mut m, Action::Interrupt);
        assert!(m.should_quit);
    }

    #[test]
    fn a_file_done_marks_its_findings_and_takes_the_stores_marks_and_stamp() {
        let mut m = accepted();
        act(&mut m, Action::Apply { dry_run: true });
        update(
            &mut m,
            Input::Apply(apply::Event::Queued(vec![path(1), path(2)])),
            now(),
        );
        update(
            &mut m,
            file_done(path(1), vec![(1, Outcome::Patched)], vec![], None),
            now(),
        );
        update(
            &mut m,
            file_done(path(2), vec![(3, Outcome::AlreadyApplied)], vec![], None),
            now(),
        );
        assert_eq!(m.marks.get(&1), Some(&Mark::WouldApply));
        // Findings 1 and 3 are at indexes 0 and 2.
        assert_eq!(m.mark(0), Some("would apply"));
        assert_eq!(m.mark(2), Some("already applied"));
        update(
            &mut m,
            Input::ApplyEnded(Ok(apply::Summary::default())),
            now(),
        );
        // Then for real: E01's fix made, E02's left by hand.
        act(&mut m, Action::Apply { dry_run: false });
        act(&mut m, Action::Answer(true));
        update(
            &mut m,
            Input::Apply(apply::Event::Queued(vec![path(1), path(2)])),
            now(),
        );
        let stamp: (u64, Timestamp) = (123, "2026-10-30T09:00:00Z".parse().unwrap());
        update(
            &mut m,
            file_done(path(1), vec![(1, Outcome::Patched)], vec![1], Some(stamp)),
            now(),
        );
        update(
            &mut m,
            file_done(
                path(2),
                vec![(3, Outcome::ByHand("a karaoke row".into()))],
                vec![],
                None,
            ),
            now(),
        );
        assert_eq!(m.marks.get(&1), None);
        assert!(m.run.applied.contains(&1));
        assert_eq!(m.mark(0), Some("applied"));
        assert_eq!(m.mark(2), Some("by hand"));
        assert_eq!((m.run.files[0].size, m.run.files[0].mtime), stamp);
        assert_eq!(m.run.files[1].size, 100, "E02 was not swapped");
    }

    /// Apply failed on the file at `path`, whose job was to fix `findings`: a backup in the way.
    fn file_failed(path: String, findings: Vec<u64>) -> Input {
        Input::Apply(apply::Event::Failed {
            error: format!("{path}.orig exists: an earlier backup is never overwritten"),
            path,
            text: String::new(),
            findings,
        })
    }

    #[test]
    fn a_failed_file_marks_its_findings_but_those_applied_already() {
        // Findings 1 (E01) and 3 (E02) accepted; 3 is in its file already.
        let mut m = accepted();
        act(&mut m, Action::Apply { dry_run: false });
        act(&mut m, Action::Answer(true));
        update(
            &mut m,
            Input::Apply(apply::Event::Queued(vec![path(1), path(2)])),
            now(),
        );
        update(&mut m, file_failed(path(1), vec![1]), now());
        update(&mut m, file_failed(path(2), vec![3]), now());
        let why = format!(
            "{}.orig exists: an earlier backup is never overwritten",
            path(1)
        );
        assert_eq!(m.marks.get(&1), Some(&Mark::Failed(why)));
        // Findings 1 and 3 are at indexes 0 and 2.
        assert_eq!(m.mark(0), Some("failed"));
        assert_eq!(m.marks.get(&3), None);
        assert_eq!(m.mark(2), Some("applied"));
        // A row whose episodes differ counts them. Row 0 is the OP lyric: findings 1 (failed),
        // 2 (applied, once the store says so) and 4 (no mark).
        m.run.applied.insert(2);
        assert_eq!(m.row_mark(0).as_deref(), Some("failed 1 · applied 1"));
    }

    #[test]
    fn a_dry_run_marks_a_failed_files_findings_would_fail_and_a_decision_drops_it() {
        let mut m = accepted();
        act(&mut m, Action::Apply { dry_run: true });
        update(
            &mut m,
            Input::Apply(apply::Event::Queued(vec![path(1)])),
            now(),
        );
        update(&mut m, file_failed(path(1), vec![1]), now());
        assert_eq!(m.mark(0), Some("would fail"));
        update(
            &mut m,
            Input::ApplyEnded(Ok(apply::Summary::default())),
            now(),
        );
        // Row 0 is the OP lyric (findings 1, 2 and 4): `Reject` rejects its pending 2 and 4, and a
        // dry run's mark on 2 goes with its old decision.
        m.marks.insert(2, Mark::WouldFail("disk full".into()));
        m.selected = 0;
        let effects = act(&mut m, Action::Reject);
        written(&mut m, effects);
        assert_eq!(m.marks.get(&2), None);
    }

    #[test]
    fn a_decision_change_drops_a_dry_run_mark_and_keeps_an_applys() {
        let mut m = accepted();
        m.marks.insert(2, Mark::WouldApply);
        m.marks.insert(3, Mark::ByHand("a karaoke row".into()));
        // Row 0 is the OP lyric (findings 1, 2 and 4: `Reject` rejects the pending 2 and 4); row 1
        // the snacks typo (3, accepted).
        m.selected = 0;
        let effects = act(&mut m, Action::Reject);
        written(&mut m, effects);
        assert_eq!(m.marks.get(&2), None);
        m.selected = 1;
        let effects = act(&mut m, Action::Reject);
        written(&mut m, effects);
        assert_eq!(m.marks.get(&3), Some(&Mark::ByHand("a karaoke row".into())));
    }

    #[test]
    fn a_dry_run_marks_a_fix_not_found_or_left_by_hand_as_would_be() {
        // Findings 1 (E01) and 3 (E02) accepted, at indexes 0 and 2.
        let mut m = accepted();
        act(&mut m, Action::Apply { dry_run: true });
        update(
            &mut m,
            Input::Apply(apply::Event::Queued(vec![path(1), path(2)])),
            now(),
        );
        update(
            &mut m,
            file_done(path(1), vec![(1, Outcome::NotFound)], vec![], None),
            now(),
        );
        update(
            &mut m,
            file_done(
                path(2),
                vec![(3, Outcome::ByHand("a karaoke row".into()))],
                vec![],
                None,
            ),
            now(),
        );
        assert_eq!(m.marks.get(&1), Some(&Mark::WouldNotBeFound));
        assert_eq!(m.mark(0), Some("would not be found"));
        assert_eq!(
            m.marks.get(&3),
            Some(&Mark::WouldBeByHand("a karaoke row".into()))
        );
        assert_eq!(m.mark(2), Some("would be by hand"));
    }

    #[test]
    fn an_apply_that_cannot_start_says_why() {
        let mut m = accepted();
        act(&mut m, Action::Apply { dry_run: true });
        update(&mut m, Input::ApplyStarted(Err("no threads".into())), now());
        assert!(!m.progress.busy());
        assert_eq!(m.status, None);
        assert_eq!(
            m.progress.line().as_deref(),
            Some("apply failed: no threads")
        );
    }
}
