//! Review state: a run's findings grouped into rows, what the list shows, and what is under way.

use std::collections::hash_map::Entry as Slot;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use jiff::Timestamp;

use super::progress::{Progress, Stopping};
use super::status::Status;
use crate::apply::patch::Outcome;
use crate::list::{self, Filter};
use crate::mkv;
use crate::player::Moment;
use crate::store::{Decision, Finding, Run, State, TrackRecord};
use crate::timing::frame_count;

/// How review ends now, whichever UI ends it ([`Review::ending`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ending {
    /// The process groups review's jobs started end (`group::end`): a job runs, whose mkvmerge or
    /// LLM calls would outlive the process, or review failed ([`Review::fatal`]: a job's thread,
    /// or the UI's own input), after which nothing it started is known to be over.
    pub groups: bool,
    /// Its outro is said ([`Review::outro`]): on a clean end, or when the end stops a job, which
    /// the outro says how to finish. An error that stopped none speaks for itself.
    pub outro: bool,
}

/// Findings decided together, as one row. Repeats are common: typos with the same original and
/// suggestion (an OP lyric is a finding in every episode), and timing findings on the same-named
/// track of one folder (a season's track), whatever each member's shift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Indexes into `run.findings`, in file order, then time order.
    pub members: Vec<usize>,
}

/// One line of the list: a row, or one member of an expanded row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Entry {
    Row(usize),
    /// The row, and the member's index into `run.findings`.
    Member(usize, usize),
}

impl Entry {
    pub fn row(self) -> usize {
        match self {
            Entry::Row(r) | Entry::Member(r, _) => r,
        }
    }
}

/// How a file differs from when it was scanned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileNote {
    Changed,
    Missing,
}

/// How the file at `path` differs from `run`'s record of it, by its size and mtime now
/// (`stamp`), none when they could not be read: missing, changed since it was scanned, or as it
/// was (`None`). A file the run checked twice is compared with its last record, the newest; one
/// the run has no record of is as it was, having nothing to differ from.
pub fn note_for(run: &Run, path: &str, stamp: Option<(u64, Timestamp)>) -> Option<FileNote> {
    let Some((size, mtime)) = stamp else {
        return Some(FileNote::Missing);
    };
    let record = run.files.iter().rev().find(|f| f.path == path)?;
    (record.size != size || record.mtime != mtime).then_some(FileNote::Changed)
}

/// Which list the left pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Findings,
    /// Each file of the run, with where the scan, or apply, is with it (`ShowPane`).
    Files,
}

/// A question on the status line, waiting for its `Answer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    /// Quit, stopping the scan or apply.
    Quit,
    /// Check `path` again into run `run`, with its `backend`: `Recheck`'s question, in the
    /// files view.
    Recheck {
        path: String,
        run: String,
        backend: String,
    },
    /// Apply the view's accepted fixes: `Apply`'s question.
    Apply,
}

/// The open question as data, for a UI to word ([`Review::question`]): what a yes does, with what
/// its words need. Files are named as the list names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Question {
    /// Quit, stopping the job under way.
    Quit(Stopping),
    /// Check file `label` again, with `backend`, the LLM its run was scanned with.
    Recheck { label: String, backend: String },
    /// Apply the view's accepted fixes: `fixes` of them, in `files` files, `applied` of them in
    /// their files already.
    Apply {
        fixes: usize,
        files: usize,
        applied: usize,
    },
}

/// What the screen's last apply or dry run said of a finding, for its mark ([`Review::mark`]).
/// Outside a dry run, a fix apply made or found made has none: the store's `applied` speaks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    /// Left for a fix by hand, and why.
    ByHand(String),
    NotFound,
    WouldApply,
    /// A dry run found the fix in its file already.
    AlreadyApplied,
    WouldBeByHand(String),
    WouldNotBeFound,
    /// Apply failed on its file, and why: the fix is not in it.
    Failed(String),
    /// A dry run failed on its file, and why.
    WouldFail(String),
}

impl Mark {
    /// The mark for what apply (with `dry_run`, a dry run) said of a fix; outside a dry run, `None`
    /// for a fix it made or found made.
    pub fn of(outcome: &Outcome, dry_run: bool) -> Option<Mark> {
        Some(match (dry_run, outcome) {
            (false, Outcome::ByHand(why)) => Mark::ByHand(why.clone()),
            (false, Outcome::NotFound) => Mark::NotFound,
            (false, _) => return None,
            (true, Outcome::AlreadyApplied) => Mark::AlreadyApplied,
            (true, Outcome::ByHand(why)) => Mark::WouldBeByHand(why.clone()),
            (true, Outcome::NotFound) => Mark::WouldNotBeFound,
            (true, _) => Mark::WouldApply,
        })
    }

    /// What it says, without apply's reason.
    pub fn kind(&self) -> MarkKind {
        match self {
            Mark::ByHand(_) => MarkKind::ByHand,
            Mark::NotFound => MarkKind::NotFound,
            Mark::WouldApply => MarkKind::WouldApply,
            Mark::AlreadyApplied => MarkKind::AlreadyApplied,
            Mark::WouldBeByHand(_) => MarkKind::WouldBeByHand,
            Mark::WouldNotBeFound => MarkKind::WouldNotBeFound,
            Mark::Failed(_) => MarkKind::Failed,
            Mark::WouldFail(_) => MarkKind::WouldFail,
        }
    }

    /// Whether a dry run made it: it says what apply would do with the decision as it was, so a
    /// new decision drops it.
    pub fn dry_run(&self) -> bool {
        matches!(
            self,
            Mark::WouldApply
                | Mark::AlreadyApplied
                | Mark::WouldBeByHand(_)
                | Mark::WouldNotBeFound
                | Mark::WouldFail(_)
        )
    }

    /// Why the fix is (or would be) left for a hand fix, or why apply failed on its file, for the
    /// detail pane.
    pub fn reason(&self) -> Option<String> {
        match self {
            Mark::ByHand(why) => Some(format!("left for a fix by hand: {why}")),
            Mark::WouldBeByHand(why) => Some(format!("would be left for a fix by hand: {why}")),
            Mark::Failed(why) => Some(format!("apply failed on its file: {why}")),
            Mark::WouldFail(why) => Some(format!("apply would fail on its file: {why}")),
            _ => None,
        }
    }
}

/// A finding's mark as the list and the detail show it ([`Review::marked`]): what the screen's
/// last apply or dry run said of it, else that its fix is in its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marked {
    Said(Mark),
    Applied,
}

impl Marked {
    /// What it says, without apply's reason: what the list counts.
    pub fn kind(&self) -> MarkKind {
        match self {
            Marked::Said(mark) => mark.kind(),
            Marked::Applied => MarkKind::Applied,
        }
    }

    /// The mark as the list shows it.
    pub fn text(&self) -> &'static str {
        self.kind().text()
    }
}

/// A finding's mark as the list counts it ([`Review::mark_counts`]): what it says, without apply's
/// reason, so that two findings left by hand for different reasons count as one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl MarkKind {
    /// The mark as the list shows it.
    pub fn text(self) -> &'static str {
        match self {
            MarkKind::ByHand => "by hand",
            MarkKind::NotFound => "not found",
            MarkKind::WouldApply => "would apply",
            MarkKind::AlreadyApplied => "already applied",
            MarkKind::WouldBeByHand => "would be by hand",
            MarkKind::WouldNotBeFound => "would not be found",
            MarkKind::Failed => "failed",
            MarkKind::WouldFail => "would fail",
            MarkKind::Applied => "applied",
        }
    }
}

/// An entry's marks as the list shows them ([`Review::mark_counts`]), of the `shown` findings it
/// stands for: the one alone when all of them have it, else each with how many have it
/// (`applied 5 · by hand 1`); none with none.
pub fn marks_text(marks: &[(MarkKind, usize)], shown: usize) -> Option<String> {
    match marks {
        [] => None,
        [(kind, n)] if *n == shown => Some(kind.text().to_string()),
        _ => {
            let parts: Vec<String> = marks
                .iter()
                .map(|(kind, n)| format!("{} {n}", kind.text()))
                .collect();
            Some(parts.join(" · "))
        }
    }
}

/// What one `Undo` takes back: each finding a decision changed, with the decision it had before.
pub type UndoStep = Vec<(u64, Option<Decision>)>;

/// An entry by its findings' IDs, which outlast a remake of the rows ([`Review::refresh`]): a row
/// by every member's, so that any one still there finds it; a member by its own. A UI names an
/// entry by its key (`Action::Select`), since a scan can remake the rows before review takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKey {
    Row(Vec<u64>),
    Member(u64),
}

/// An edit under way ([`Review::editing`]): the entry it was opened on, and what it decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub entry: EntryKey,
    /// The findings `to_decide` gave the entry when the edit opened, or those a re-check put in
    /// their place ([`Review::refresh`]), by ID. The edit decides these and no others: a re-check
    /// can add members to the row or decide the others by inheritance while the editor is open,
    /// and the text was written for these.
    pub findings: Vec<u64>,
}

/// Review's state, whatever UI shows it: the run's findings in rows, the list as shown and its
/// selection, what is under way, and what the status line says.
// No two flags are cases of one state, which an enum would name. `held` is set only while
// `stamping` is, yet no part of its moment: a newer press replaces the moment and keeps `held`.
#[allow(clippy::struct_excessive_bools)]
pub struct Review {
    pub run: Run,
    pub rows: Vec<Row>,
    /// Each finding's track as the list tells it apart, by index ([`ListTrack`]); worked out once,
    /// as the list asks on every redraw and the session on every step.
    tracks: Vec<Option<ListTrack>>,
    pub expanded: HashSet<usize>,
    pub filter: Filter,
    /// Index into `entries()`.
    pub selected: usize,
    /// The edit under way, from `Edit` to its end. The UI holds the text, which `Reword` brings.
    pub editing: Option<Edit>,
    /// Decisions made in this session, oldest first.
    pub undo: Vec<UndoStep>,
    /// Files with findings that are not as they were scanned, by path.
    pub notes: HashMap<String, FileNote>,
    /// The status message, for each UI to word.
    pub status: Option<Status>,
    pub should_quit: bool,
    /// The name each file goes by in the list.
    pub labels: HashMap<String, String>,
    /// The scan or apply under way, or as it ended; with neither, the run's files as recorded.
    pub progress: Progress,
    pub pane: Pane,
    /// Index into `progress.files`.
    pub file_selected: usize,
    pub confirm: Option<Confirm>,
    /// The screen's apply re-spaces per-letter karaoke, as `apply --karaoke` does (`--karaoke` on
    /// `scan` and `review`).
    pub karaoke: bool,
    /// What the screen's applies and dry runs said of findings, by ID ([`Mark`]). By ID, so they
    /// outlast a remake of the rows and another view.
    pub marks: HashMap<u64, Mark>,
    /// Why the screen ended early: a job's thread panicked, or the terminal could not be read.
    pub fatal: Option<String>,
    /// Where the selection was before the last input that could remake the list around it: a file
    /// read back, or a late join's decision while the selected entry stays in the list; `None`
    /// after any other input. A UI whose list scrolls moves it as far as the selection moved, so
    /// the selection keeps its line.
    pub kept_from: Option<usize>,
    /// Each finding a re-check replaced, by its ID, with the ID of the first that took its place
    /// (one of its file's with the same carry key): a key that named it finds that one, as
    /// `refresh` carries what pointed at it.
    replaced_by: HashMap<u64, u64>,
    /// The file review last asked the player to load, and its size and mtime then, until the
    /// player closes, an open fails, or `Play`'s stat cannot read the size and mtime of the file it
    /// opens: the same file, unchanged since, is sought, not loaded again (slow for a big file
    /// over NFS). A remux in place keeps the path, and after a fix of one letter even the size.
    pub loaded: Option<(String, (u64, Timestamp))>,
    /// The moment of the newest `Play` still waiting for its file's check, until the check
    /// answers ([`Input::Stamped`]) or `ClosePlayer` ends the wait. Only this moment's answer
    /// opens: on a slow share an older press's check can answer after the user has pressed again,
    /// or given up, and opening it then would show a moment they have moved past. Moving the
    /// selection or switching the view keeps it: the press asked for that moment.
    ///
    /// [`Input::Stamped`]: crate::review::update::Input::Stamped
    pub stamping: Option<Moment>,
    /// A Space (`Playback::Pause`) pressed while `Play` waits for its file's check, with no file
    /// of review's in the player ([`Review::loaded`]) for it to toggle, kept for the video that
    /// arrives: the Open the check's answer sends lands it playing ([`PlayerCommand::Open`]'s
    /// `play`). A second Space undoes the first. Set only while [`Review::stamping`] is: the
    /// check's answer takes it, and a file found missing, `ClosePlayer` and a `Play` that opens
    /// nothing drop it, so that no Space acts after the wait it was pressed in. A newer `Play` that
    /// waits keeps it: the Space was for whichever video arrives. With a file loaded, a Space goes
    /// to the player at once, as before `Play`.
    ///
    /// [`PlayerCommand::Open`]: crate::player::PlayerCommand::Open
    pub held: bool,
    /// Whether the newest `ClosePlayer` ended a `Play`'s wait ([`Review::stamping`]). The player's
    /// "nothing open" for a close that did is kept off the status line, even when other news came
    /// first: it would read as if the press had done nothing. Each press sets it and no answer
    /// clears it, as the player answers closes in order: an older press's answer that comes after
    /// a newer press ended a wait is kept off too.
    pub close_ended_wait: bool,
    /// Which view review shows: 0 as it opens, one more at each view switch (`switch`). A view's
    /// file notes come after its rows ([`Input::Noted`]), and a switch can come in between: notes
    /// asked for another view than this one are of another view's files.
    ///
    /// [`Input::Noted`]: crate::review::update::Input::Noted
    pub view_number: u64,
    /// The number of the newest view switch asked for ([`Review::ask_switch`]), until it answers
    /// ([`Input::Switched`]) or a job starts. Only its answer is taken: on a slow share an older
    /// switch's read can answer after the user has asked for another view. Once a job starts, none
    /// is: the job's progress belongs to the view it started in, as a switch asked while it runs is
    /// refused.
    ///
    /// [`Input::Switched`]: crate::review::update::Input::Switched
    pub switching: Option<u64>,
    /// How many view switches have been asked for: each takes the next number, so that an answer
    /// names the request it answers.
    switches: u64,
    /// The files whose note review has set or cleared since it asked for the view's notes: by
    /// `Play`'s check of the file, a read-back (`refresh`) or apply's remux in place. Each is a
    /// newer look at the file than the view's check, which may have read it before: the check's
    /// answer ([`Input::Noted`]) leaves their notes as they are.
    ///
    /// [`Input::Noted`]: crate::review::update::Input::Noted
    pub fresh: HashSet<String>,
    /// The decisions written while a view switch is under way, by finding ID. The switch reads the
    /// store on its own, and may read it before they are written: the view it brings takes these
    /// over its own ([`Review::switch`]).
    decided_meanwhile: HashMap<u64, Decision>,
}

impl Review {
    /// What the terminal gets on leaving the screen: what its scans ended with, then its applies,
    /// files named as the list names them ([`Progress::outro`], [`Progress::apply_outro`]).
    pub fn outro(&self) -> Vec<String> {
        let mut lines = self.progress.outro();
        lines.extend(self.progress.apply_outro(&|p| self.label(p)));
        lines
    }

    /// How review ends now, `failed` when its UI's own loop failed or panicked: the terminal's
    /// loop and a session both end by this. `failed` decides the outro alone: a job the loop
    /// started runs until it reports its end, so it ends the groups whatever ended the loop, and
    /// with none running, the loop's failure has left none behind.
    pub fn ending(&self, failed: bool) -> Ending {
        let job = self.progress.busy();
        let error = failed || self.fatal.is_some();
        Ending {
            groups: job || self.fatal.is_some(),
            outro: job || !error,
        }
    }

    /// Every finding, decided or not, with the first row that has a pending one selected: a
    /// half-reviewed run opens where the work is.
    pub fn new(run: Run, notes: HashMap<String, FileNote>) -> Review {
        let (rows, tracks) = group(&run);
        let labels = labels(&run);
        let progress = Progress::of_run(&run);
        let mut m = Review {
            run,
            rows,
            tracks,
            labels,
            expanded: HashSet::new(),
            filter: Filter::All,
            selected: 0,
            editing: None,
            undo: Vec::new(),
            notes,
            status: None,
            should_quit: false,
            progress,
            pane: Pane::Findings,
            file_selected: 0,
            confirm: None,
            karaoke: false,
            marks: HashMap::new(),
            fatal: None,
            kept_from: None,
            replaced_by: HashMap::new(),
            loaded: None,
            stamping: None,
            held: false,
            close_ended_wait: false,
            view_number: 0,
            switching: None,
            switches: 0,
            fresh: HashSet::new(),
            decided_meanwhile: HashMap::new(),
        };
        m.selected = m
            .entries()
            .iter()
            .position(|&e| m.has_pending(e))
            .unwrap_or(0);
        m
    }

    /// Take file `path` as the store shows it now (`view`, from `Store::file_view`): its record,
    /// findings, decisions and applied marks replace those the run had, and the rows are made
    /// again. What pointed at a finding that went (the selection, the edit's findings, an expanded
    /// row, the undo steps) moves to the new finding of its carry key, as a key does
    /// ([`Review::resolve`]); the edit's findings and the undo steps' move to every new finding
    /// of it. Failing that, the selection goes to the nearest entry still shown, an edit left with
    /// none of its findings ends and the status line says why, and the rest let go of it; on a list
    /// that was empty, the selection goes to the first row with a pending finding, as on opening.
    /// Where it was is kept in `kept_from`, for a screen to keep it on its line. Returns the new
    /// findings' indexes.
    #[allow(clippy::too_many_lines)] // the swap, then each thing that pointed at the file, in order
    pub fn refresh(&mut self, path: &str, view: Run) -> Vec<usize> {
        let was_at = self.selected;
        let selected = self.selected_key();
        let expanded: Vec<EntryKey> = self
            .expanded
            .iter()
            .map(|&r| self.entry_key(Entry::Row(r)))
            .collect();

        // What the file had, and for each finding that went, the new ones of its key.
        let before: HashSet<u64> = self
            .run
            .findings
            .iter()
            .filter(|f| f.file == path)
            .map(|f| f.id)
            .collect();
        let moved: HashMap<u64, Vec<u64>> = self
            .run
            .findings
            .iter()
            .filter(|f| f.file == path && !view.findings.iter().any(|n| n.id == f.id))
            .map(|f| {
                let key = f.carry_key();
                let to = view
                    .findings
                    .iter()
                    .filter(|n| n.carry_key() == key)
                    .map(|n| n.id)
                    .collect();
                (f.id, to)
            })
            .collect();
        for (&id, to) in &moved {
            if let Some(&first) = to.first() {
                self.replaced_by.insert(id, first);
            }
        }

        self.run.findings.retain(|f| f.file != path);
        for id in &before {
            self.run.decisions.remove(id);
            self.run.applied.remove(id);
        }
        let Run {
            files,
            findings,
            decisions,
            applied,
            ..
        } = view;
        let from = self.run.findings.len();
        self.run.findings.extend(findings);
        self.run.decisions.extend(decisions);
        self.run.applied.extend(applied);
        if let Some(record) = files.into_iter().find(|r| r.path == path) {
            match self.run.files.iter().position(|r| r.path == path) {
                Some(i) => self.run.files[i] = record,
                None => self.run.files.push(record),
            }
        }
        // The store has just read it as it is on disk: newer than the view's check, should that
        // answer later.
        self.notes.remove(path);
        self.fresh.insert(path.to_string());
        let (rows, tracks) = group(&self.run);
        self.rows = rows;
        self.tracks = tracks;
        self.labels = labels(&self.run);

        let index: HashMap<u64, usize> = self
            .run
            .findings
            .iter()
            .enumerate()
            .map(|(i, f)| (f.id, i))
            .collect();
        let now_at = |id: u64| {
            index
                .get(&id)
                .or_else(|| index.get(moved.get(&id)?.first()?))
                .copied()
        };
        self.expanded = expanded
            .iter()
            .filter_map(|h| self.find(h, &now_at))
            .map(Entry::row)
            .collect();
        // A decision on a finding that went goes to every new finding of its carry key, as the
        // store's write does (`Decided::Followed`), or nowhere when there is none.
        let carry = |id: u64| moved.get(&id).cloned().unwrap_or_else(|| vec![id]);
        if let Some(edit) = self.editing.take() {
            let mut seen = HashSet::new();
            let findings: Vec<u64> = edit
                .findings
                .iter()
                .flat_map(|&id| carry(id))
                .filter(|&id| seen.insert(id))
                .collect();
            match self.find(&edit.entry, &now_at) {
                Some(e) if !findings.is_empty() => {
                    self.editing = Some(Edit {
                        entry: self.entry_key(e),
                        findings,
                    });
                }
                _ => {
                    self.status = Some(Status::EditEnded {
                        label: self.label(path),
                    });
                }
            }
        }
        for step in &mut self.undo {
            *step = std::mem::take(step)
                .into_iter()
                .flat_map(|(id, prior)| carry(id).into_iter().map(move |n| (n, prior.clone())))
                .collect();
        }
        self.undo.retain(|step| !step.is_empty());

        let entries = self.entries();
        let shown = |e: Entry| entries.iter().position(|&n| n == e);
        self.selected = match selected {
            // A member whose row no longer expands is found by its row.
            Some(h) => self
                .find(&h, &now_at)
                .and_then(|e| shown(e).or_else(|| shown(Entry::Row(e.row()))))
                .unwrap_or_else(|| was_at.min(entries.len().saturating_sub(1))),
            None => entries
                .iter()
                .position(|&e| self.has_pending(e))
                .unwrap_or(0),
        };
        self.kept_from = Some(was_at);
        (from..self.run.findings.len())
            .filter(|&i| !before.contains(&self.run.findings[i].id))
            .collect()
    }

    /// Take `view`, another view of the findings (`SwitchView`'s other one, `OpenView`'s or
    /// `OpenFolder`'s). Its files' notes come later, once checked: the notes of the view left go,
    /// and the view's number moves on, so that a late answer for the view left is dropped. The
    /// decisions written while the switch was under way are laid over the view's own, which the
    /// store may have been read before them. What pointed at a finding carries over by its ID,
    /// where the view has it: the selection, else the first row with a pending finding, as on
    /// opening; the expanded rows; and the undo steps, less the findings this view does not show
    /// (a step left empty goes). The filter, the pane, the marks and the file the player last
    /// loaded stay. The files view shows the new view's files, the selected file kept where it is
    /// one of them.
    pub fn switch(&mut self, view: Run) {
        let selected = self.selected_key();
        let expanded: Vec<EntryKey> = self
            .expanded
            .iter()
            .map(|&r| self.entry_key(Entry::Row(r)))
            .collect();
        let file = self
            .progress
            .files
            .get(self.file_selected)
            .map(|f| f.path.clone());
        let (rows, tracks) = group(&view);
        self.rows = rows;
        self.tracks = tracks;
        self.labels = labels(&view);
        self.progress.show(&view);
        self.run = view;
        self.notes.clear();
        self.fresh.clear();
        self.view_number += 1;
        self.editing = None;

        let index: HashMap<u64, usize> = self
            .run
            .findings
            .iter()
            .enumerate()
            .map(|(i, f)| (f.id, i))
            .collect();
        for (id, d) in std::mem::take(&mut self.decided_meanwhile) {
            if index.contains_key(&id) {
                self.run.decisions.insert(id, d);
            }
        }
        let at = |id: u64| index.get(&id).copied();
        self.expanded = expanded
            .iter()
            .filter_map(|h| self.find(h, &at))
            .map(Entry::row)
            .collect();
        for step in &mut self.undo {
            step.retain(|(id, _)| index.contains_key(id));
        }
        self.undo.retain(|step| !step.is_empty());
        let entries = self.entries();
        let shown = |e: Entry| entries.iter().position(|&n| n == e);
        self.selected = selected
            .and_then(|h| self.find(&h, &at))
            .and_then(|e| shown(e).or_else(|| shown(Entry::Row(e.row()))))
            .unwrap_or_else(|| {
                entries
                    .iter()
                    .position(|&e| self.has_pending(e))
                    .unwrap_or(0)
            });
        self.file_selected = file
            .and_then(|p| self.progress.files.iter().position(|f| f.path == p))
            .unwrap_or(0);
    }

    /// Number a new view switch: the newest asked, whose answer alone is taken (`switching`).
    /// The decisions written before it are in the store its read finds; those written from here on
    /// are kept for the view it brings (`take_decision`).
    pub fn ask_switch(&mut self) -> u64 {
        self.switches += 1;
        self.switching = Some(self.switches);
        self.decided_meanwhile.clear();
        self.switches
    }

    /// Decision `d`, written to the store: the view shows it, and while a view switch is under way
    /// so will the view the switch brings, whenever its read was ([`Review::switch`]).
    pub fn take_decision(&mut self, d: &Decision) {
        self.run.decisions.insert(d.finding, d.clone());
        if self.switching.is_some() {
            self.decided_meanwhile.insert(d.finding, d.clone());
        }
    }

    /// Entry `e`'s key ([`EntryKey`]). `e` is an entry of the current rows, as `entries()` gives
    /// them: one kept from before a refresh or a switch remade the rows names other findings, or
    /// indexes past them and panics.
    pub fn entry_key(&self, e: Entry) -> EntryKey {
        match e {
            Entry::Row(r) => EntryKey::Row(
                self.rows[r]
                    .members
                    .iter()
                    .map(|&i| self.finding(i).id)
                    .collect(),
            ),
            Entry::Member(_, i) => EntryKey::Member(self.finding(i).id),
        }
    }

    /// The edit's entry and its findings' indexes, as the rows are now; `None` with no edit, or
    /// when its entry or one of its findings is gone. `refresh` keeps an edit's findings in step
    /// with the rows, so one missing here is an edit it did not make: nothing of it is decided.
    pub fn edit_target(&self) -> Option<(Entry, Vec<usize>)> {
        let edit = self.editing.as_ref()?;
        let findings: Vec<usize> = edit
            .findings
            .iter()
            .map(|&id| self.index_of(id))
            .collect::<Option<_>>()?;
        if findings.is_empty() {
            return None;
        }
        Some((self.resolve(&edit.entry)?, findings))
    }

    /// The wording an edit starts from: its first finding's, as [`Review::wording`] gives it;
    /// `None` with no edit, or when what it edits is gone ([`Review::edit_target`]).
    pub fn edit_wording(&self) -> Option<&str> {
        let (_, findings) = self.edit_target()?;
        Some(self.wording(findings[0]))
    }

    /// The selected entry's key; `None` with nothing selected.
    pub fn selected_key(&self) -> Option<EntryKey> {
        self.selected_entry().map(|e| self.entry_key(e))
    }

    /// Where `key`'s entry is among the rows as they are now: its findings found by ID, one a
    /// re-check replaced by its replacement's (`replaced_by`); `None` when they all went.
    pub fn resolve(&self, key: &EntryKey) -> Option<Entry> {
        self.find(key, &|id| self.index_of(id))
    }

    /// Finding `id`'s index, or for one a re-check replaced, its replacement's.
    fn index_of(&self, id: u64) -> Option<usize> {
        // A finding a re-check replaced never comes back, so the chain ends; `take` bounds it
        // should that ever not hold.
        std::iter::successors(Some(id), |id| self.replaced_by.get(id).copied())
            .take(self.replaced_by.len() + 1)
            .find_map(|id| self.run.findings.iter().position(|f| f.id == id))
    }

    /// Where `key`'s entry is among the rows as they are now; `at` gives a finding's index by its
    /// ID, or `None` for one that went.
    fn find(&self, key: &EntryKey, at: &dyn Fn(u64) -> Option<usize>) -> Option<Entry> {
        let row_of = |i: usize| self.rows.iter().position(|r| r.members.contains(&i));
        match key {
            EntryKey::Member(id) => {
                let i = at(*id)?;
                Some(Entry::Member(row_of(i)?, i))
            }
            EntryKey::Row(ids) => ids
                .iter()
                .find_map(|&id| at(id))
                .and_then(row_of)
                .map(Entry::Row),
        }
    }

    /// Whether any finding an entry stands for is pending.
    pub fn has_pending(&self, e: Entry) -> bool {
        self.targets(e)
            .iter()
            .any(|&i| self.state(i) == State::Pending)
    }

    /// Finding `i`'s mark: what the screen's last apply or dry run said of it, else
    /// [`Marked::Applied`] when its fix is in its file, else none.
    pub fn marked(&self, i: usize) -> Option<Marked> {
        let id = self.finding(i).id;
        match self.marks.get(&id) {
            Some(mark) => Some(Marked::Said(mark.clone())),
            None => self.run.applied.contains(&id).then_some(Marked::Applied),
        }
    }

    /// Finding `i`'s mark ([`Review::marked`]) as the list shows it.
    pub fn mark(&self, i: usize) -> Option<&'static str> {
        self.marked(i).as_ref().map(Marked::text)
    }

    /// The marks of the findings entry `e` stands for: each kind with how many have it, in the
    /// order its findings come, leaving out those with none.
    pub fn mark_counts(&self, e: Entry) -> Vec<(MarkKind, usize)> {
        let mut counts: Vec<(MarkKind, usize)> = Vec::new();
        for kind in self
            .targets(e)
            .into_iter()
            .filter_map(|i| self.marked(i).as_ref().map(Marked::kind))
        {
            match counts.iter_mut().find(|(k, _)| *k == kind) {
                Some((_, n)) => *n += 1,
                None => counts.push((kind, 1)),
            }
        }
        counts
    }

    /// Row `r`'s mark, over the members the filter shows, as the list shows it ([`marks_text`]).
    pub fn row_mark(&self, r: usize) -> Option<String> {
        marks_text(
            &self.mark_counts(Entry::Row(r)),
            self.shown_members(r).len(),
        )
    }

    /// What `Apply` would apply, as its question counts it: the view's accepted fixes (typos and
    /// timing fixes, whatever the filter), the files that hold them, and of the fixes those in
    /// their files already.
    pub fn apply_counts(&self) -> (usize, usize, usize) {
        let fixes: Vec<&Finding> = self
            .run
            .findings
            .iter()
            .filter(|f| self.run.state_of(f.id) == State::Accepted)
            .collect();
        let paths: HashSet<&str> = fixes.iter().map(|f| f.file.as_str()).collect();
        let already = fixes
            .iter()
            .filter(|f| self.run.applied.contains(&f.id))
            .count();
        (fixes.len(), paths.len(), already)
    }

    /// The open question ([`Review::confirm`]), with what its words need now: the quit question's
    /// counts follow the job as it goes on.
    pub fn question(&self) -> Option<Question> {
        Some(match self.confirm.as_ref()? {
            Confirm::Quit => Question::Quit(self.progress.stopping(&|p| self.label(p))),
            Confirm::Recheck { path, backend, .. } => Question::Recheck {
                label: self.label(path),
                backend: backend.clone(),
            },
            Confirm::Apply => {
                // `paths`, not `files`: clippy's `similar_names` takes that for a slip of `fixes`.
                let (fixes, paths, applied) = self.apply_counts();
                Question::Apply {
                    fixes,
                    files: paths,
                    applied,
                }
            }
        })
    }

    /// The name a file goes by in the list (see [`labels`]).
    pub fn label(&self, file: &str) -> String {
        self.labels
            .get(file)
            .cloned()
            .unwrap_or_else(|| episode(file))
    }

    pub fn finding(&self, i: usize) -> &Finding {
        &self.run.findings[i]
    }

    pub fn state(&self, i: usize) -> State {
        self.run.state_of(self.finding(i).id)
    }

    /// The wording a finding would be fixed with: the reviewer's edit, else the suggestion.
    pub fn wording(&self, i: usize) -> &str {
        let f = self.finding(i);
        self.run
            .decisions
            .get(&f.id)
            .and_then(|d| d.edited.as_deref())
            .unwrap_or(&f.suggestion)
    }

    fn shows(&self, i: usize) -> bool {
        match self.filter {
            Filter::All => true,
            Filter::Only(state) => self.state(i) == state,
        }
    }

    /// The row's members the filter shows: what a decision on the row applies to.
    pub fn shown_members(&self, row: usize) -> Vec<usize> {
        self.rows[row]
            .members
            .iter()
            .copied()
            .filter(|&i| self.shows(i))
            .collect()
    }

    /// The list as shown: each row with a member the filter shows, and under an expanded row its
    /// shown members.
    pub fn entries(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        for (r, row) in self.rows.iter().enumerate() {
            let shown = self.shown_members(r);
            if shown.is_empty() {
                continue;
            }
            out.push(Entry::Row(r));
            if row.members.len() > 1 && self.expanded.contains(&r) {
                out.extend(shown.into_iter().map(|i| Entry::Member(r, i)));
            }
        }
        out
    }

    /// The findings an entry stands for.
    pub fn targets(&self, e: Entry) -> Vec<usize> {
        match e {
            Entry::Row(r) => self.shown_members(r),
            Entry::Member(_, i) => vec![i],
        }
    }

    /// What `Accept`, `Reject` or an edit on an entry decide: of its targets, the pending ones; all
    /// of them when none is, so that a decided group still flips as a whole. A member decided on
    /// its own keeps its decision when its row is decided later.
    pub fn to_decide(&self, e: Entry) -> Vec<usize> {
        let targets = self.targets(e);
        let pending: Vec<usize> = targets
            .iter()
            .copied()
            .filter(|&i| self.state(i) == State::Pending)
            .collect();
        if pending.is_empty() { targets } else { pending }
    }

    pub fn selected_entry(&self) -> Option<Entry> {
        self.entries().get(self.selected).copied()
    }

    /// The finding the detail pane shows and `Play` opens: the selected member, or the selected
    /// row's first shown member.
    pub fn focus(&self) -> Option<usize> {
        self.selected_entry()
            .and_then(|e| self.targets(e).first().copied())
    }

    /// The state of the members the filter shows, when they agree; `None` when they do not.
    pub fn shown_state(&self, row: usize) -> Option<State> {
        let mut states = self.shown_members(row).into_iter().map(|i| self.state(i));
        let first = states.next()?;
        states.all(|s| s == first).then_some(first)
    }

    /// The state an entry's list line goes by: a row that shows a pending member is still to do,
    /// however its decided members differ; else what its shown members agree on, `None` when they
    /// do not.
    pub fn entry_state(&self, e: Entry) -> Option<State> {
        match e {
            Entry::Row(_) if self.has_pending(e) => Some(State::Pending),
            Entry::Row(r) => self.shown_state(r),
            Entry::Member(_, i) => Some(self.state(i)),
        }
    }

    /// Whether a scan is checking again the file of a finding entry `e` stands for, among the
    /// files it is checking (`Progress::checked`), which a caller that asks of every entry works
    /// out once: its findings shown are the earlier check's until the new ones are saved.
    pub fn checking_among(&self, e: Entry, checked: &HashSet<&str>) -> bool {
        self.targets(e)
            .iter()
            .any(|&i| checked.contains(self.finding(i).file.as_str()))
    }

    /// All members' state when they agree; `None` when they do not.
    pub fn row_state(&self, row: usize) -> Option<State> {
        let mut states = self.rows[row].members.iter().map(|&i| self.state(i));
        let first = states.next()?;
        states.all(|s| s == first).then_some(first)
    }

    /// Findings pending, accepted and rejected.
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut n = (0, 0, 0);
        for i in 0..self.run.findings.len() {
            match self.state(i) {
                State::Pending => n.0 += 1,
                State::Accepted => n.1 += 1,
                State::Rejected => n.2 += 1,
            }
        }
        n
    }

    pub fn clamp(&mut self) {
        self.selected = self.selected.min(self.entries().len().saturating_sub(1));
    }

    /// The file an entry belongs to: its first target's.
    pub fn file_of(&self, e: Entry) -> Option<&str> {
        self.targets(e)
            .first()
            .map(|&i| self.finding(i).file.as_str())
    }

    /// Where a finding's line starts, in ms: the occurrence `list --mpv` would open.
    pub fn start_ms(&self, i: usize) -> i64 {
        list::shown_at(&self.run, self.finding(i))
            .0
            .map_or(0, |o| o.start_ms)
    }

    /// A timing finding's track, as the list tells it apart; none for other findings.
    pub fn list_track(&self, i: usize) -> Option<&ListTrack> {
        self.tracks[i].as_ref()
    }

    /// A timing finding's track as the list names it ([`ListTrack::label`]); empty for others.
    pub fn track_name(&self, i: usize) -> String {
        self.list_track(i)
            .map_or_else(String::new, ListTrack::label)
    }

    /// The fixes of timing findings `members`, each with how many have it, the most common first,
    /// those as common in the order they come; none for findings that are not timing findings.
    pub fn fixes(&self, members: &[usize]) -> Vec<(Fix, usize)> {
        let mut counts: Vec<(Fix, usize)> = Vec::new();
        for fix in members.iter().filter_map(|&i| Fix::of(self.finding(i))) {
            match counts.iter_mut().find(|(f, _)| *f == fix) {
                Some((_, n)) => *n += 1,
                None => counts.push((fix, 1)),
            }
        }
        // Stable: equal counts keep the order they came in.
        counts.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
        counts
    }

    /// The fixes of timing findings `members` in short ([`fixes_text`]).
    pub fn fix_summary(&self, members: &[usize]) -> String {
        fixes_text(&self.fixes(members))
    }
}

/// Fixes counted as [`Review::fixes`] counts them, in short: `−1 frame ×23 · partial ×1`, or one
/// `−1 frame` when they all agree.
pub fn fixes_text(fixes: &[(Fix, usize)]) -> String {
    if let [(only, _)] = fixes {
        return only.label();
    }
    let parts: Vec<String> = fixes
        .iter()
        .map(|(fix, n)| format!("{} ×{n}", fix.label()))
        .collect();
    parts.join(" · ")
}

/// A timing finding's track as the list tells it apart ([`ListTrack::label`]): its ID; its own
/// name and its language, as a detail's track has them, when its file's record lists it; and
/// whether that name is the only one of its file's tracks with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListTrack {
    /// mkvmerge's ID of the track.
    pub id: usize,
    pub name: Option<String>,
    /// None when the record of its file does not list the track.
    pub language: Option<String>,
    /// No other track of its file has its name.
    pub apart: bool,
}

impl ListTrack {
    /// Track `id` among a file record's `tracks`.
    pub fn of(tracks: &[TrackRecord], id: usize) -> ListTrack {
        let Some((t, apart)) = list::track_in(tracks, id) else {
            return ListTrack {
                id,
                name: None,
                language: None,
                apart: false,
            };
        };
        ListTrack {
            id,
            name: mkv::own_name(t.name.as_deref()).map(str::to_string),
            language: Some(
                mkv::shown_language(&t.language, t.language_bcp47.as_deref()).to_string(),
            ),
            apart,
        }
    }

    /// Its name in the list: `English Full`; with its ID before it when the name is not its own
    /// (no name, `track 6 (eng)`, or one another track of the file shares, `track 6 English`), so
    /// that it is told apart in the list as in its row; else its ID alone.
    pub fn label(&self) -> String {
        let id = self.id;
        match (&self.name, &self.language) {
            (Some(name), _) if self.apart => name.clone(),
            (Some(name), _) => format!("track {id} {name}"),
            // By the reports' rule, so that this list and `subcheck list` name it alike.
            (None, Some(language)) => format!("track {id} {}", mkv::shown_name(None, language)),
            (None, None) => format!("track {id}"),
        }
    }
}

/// Each finding's track, by index ([`ListTrack`]), through one look-up of the files by path; none
/// for a finding that is not a timing finding.
fn list_tracks(run: &Run) -> Vec<Option<ListTrack>> {
    let by_path: HashMap<&str, &[TrackRecord]> = run
        .files
        .iter()
        .map(|r| (r.path.as_str(), r.tracks.as_slice()))
        .collect();
    run.findings
        .iter()
        .map(|f| {
            let tracks = by_path.get(f.file.as_str()).copied().unwrap_or(&[]);
            f.timing.as_ref().map(|t| ListTrack::of(tracks, t.track))
        })
        .collect()
}

/// A timing finding's fix, as a member of its row says it ([`Fix::label`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    /// The whole track, by its one stretch's frames, which are late when positive: the fix moves
    /// it earlier.
    Shift(i64),
    /// Stretches of the track, each by its own frames, or none.
    Partial,
}

impl Fix {
    /// `f`'s fix; none for a finding that is not a timing finding.
    pub fn of(f: &Finding) -> Option<Fix> {
        Some(match f.timing.as_ref()?.stretches.as_slice() {
            [whole] => Fix::Shift(whole.frames),
            _ => Fix::Partial,
        })
    }

    /// The fix in a word: `−1 frame` (its lines moved a frame earlier), `+2 frames`, `partial`.
    pub fn label(self) -> String {
        match self {
            Fix::Shift(frames) => {
                let sign = if frames > 0 { '−' } else { '+' };
                format!("{sign}{}", frame_count(frames))
            }
            Fix::Partial => "partial".into(),
        }
    }
}

/// A timing finding's fix in a word ([`Fix::label`]); empty for other findings.
pub fn fix_label(f: &Finding) -> String {
    Fix::of(f).map_or_else(String::new, Fix::label)
}

/// What puts findings in one row (see [`Row`]).
#[derive(PartialEq, Eq, Hash)]
enum Key<'a> {
    Text(&'a str, &'a str),
    /// A folder and a track's [`ListTrack::label`], which carries the ID where the name alone is
    /// not the track's.
    Track(&'a str, &'a str),
}

/// The run's findings as rows (see [`Row`]), each where its first member is; and each finding's
/// track ([`ListTrack`]), by index, whose name in the list timing findings are grouped by.
pub fn group(run: &Run) -> (Vec<Row>, Vec<Option<ListTrack>>) {
    let tracks = list_tracks(run);
    let names: Vec<String> = tracks
        .iter()
        .map(|t| t.as_ref().map_or_else(String::new, ListTrack::label))
        .collect();
    let mut order: Vec<usize> = (0..run.findings.len()).collect();
    order.sort_by_cached_key(|&i| {
        let f = &run.findings[i];
        let start = list::shown_at(run, f).0.map_or(0, |o| o.start_ms);
        (f.file.clone(), start, f.id)
    });
    let mut rows: Vec<Row> = Vec::new();
    let mut by_key: HashMap<Key, usize> = HashMap::new();
    for i in order {
        let f = &run.findings[i];
        let key = if f.timing.is_some() {
            let folder = f.file.rsplit_once('/').map_or("", |(folder, _)| folder);
            Key::Track(folder, names[i].as_str())
        } else {
            Key::Text(f.original.as_str(), f.suggestion.as_str())
        };
        match by_key.entry(key) {
            Slot::Occupied(slot) => rows[*slot.get()].members.push(i),
            Slot::Vacant(slot) => {
                slot.insert(rows.len());
                rows.push(Row { members: vec![i] });
            }
        }
    }
    (rows, tracks)
}

/// The `E02` in a file's stem: `E` and digits, as in `- E02 -` or `S01E02`.
fn episode_token(stem: &str) -> Option<Range<usize>> {
    let bytes = stem.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'E' {
            continue;
        }
        let digits = bytes[i + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        let before = i.checked_sub(1).map(|j| bytes[j]);
        let after = bytes.get(i + 1 + digits);
        // `E` starts the token, or follows a season number (`S01E02`).
        let starts = before.is_none_or(|c| !c.is_ascii_alphanumeric())
            || (before.is_some_and(|c| c.is_ascii_digit()) && stem[..i].contains('S'));
        if digits > 0 && starts && after.is_none_or(|c| !c.is_ascii_alphanumeric()) {
            return Some(i..i + 1 + digits);
        }
    }
    None
}

fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// A file's name in the list: its episode, `E02`, else the start of its file name.
pub fn episode(path: &str) -> String {
    let stem = stem(path);
    match episode_token(stem) {
        Some(token) => stem[token].to_string(),
        None => stem.chars().take(12).collect(),
    }
}

/// A file's name when episodes alone would be ambiguous: its file name up to the episode,
/// `K-On!! - E02`, else the start of its file name.
pub fn long_episode(path: &str) -> String {
    let stem = stem(path);
    match episode_token(stem) {
        Some(token) => stem[..token.end].to_string(),
        None => stem.chars().take(24).collect(),
    }
}

/// A file's longer name with its folder before it, `S2/Show - E01`, for seasons kept in folders
/// of their own under the same file names.
fn folder_episode(path: &str) -> String {
    let folder = path.rsplit('/').nth(1).unwrap_or("");
    format!("{folder}/{}", long_episode(path))
}

/// A name per file with findings in the run: the episode; when two files would share one (a run
/// over two seasons has two E01s), the file name up to the episode, for every file; when even
/// those collide, that with its folder.
pub fn labels(run: &Run) -> HashMap<String, String> {
    let mut files: Vec<&str> = run.findings.iter().map(|f| f.file.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    let distinct = |name: fn(&str) -> String| {
        files.iter().map(|f| name(f)).collect::<HashSet<_>>().len() == files.len()
    };
    let name: fn(&str) -> String = if distinct(episode) {
        episode
    } else if distinct(long_episode) {
        long_episode
    } else {
        folder_episode
    };
    files
        .into_iter()
        .map(|f| (f.to_string(), name(f)))
        .collect()
}

/// What differs between two wordings, as byte ranges of each: what remains once the words they
/// share at both ends are set aside. When one side would be empty (a word only added or only
/// removed), a word of context is kept, so both sides show something.
pub fn changed(a: &str, b: &str) -> (Range<usize>, Range<usize>) {
    let (wa, wb) = (words(a), words(b));
    if wa.is_empty() || wb.is_empty() {
        return (0..a.len(), 0..b.len());
    }
    let same = |i: usize, j: usize| a[wa[i].clone()] == b[wb[j].clone()];
    let shortest = wa.len().min(wb.len());
    let mut prefix = (0..shortest).take_while(|&k| same(k, k)).count();
    let mut suffix = (0..shortest - prefix)
        .take_while(|&k| same(wa.len() - 1 - k, wb.len() - 1 - k))
        .count();
    if wa.len() == wb.len() && prefix == wa.len() {
        return (0..0, 0..0);
    }
    if prefix + suffix == shortest {
        if prefix > 0 {
            prefix -= 1;
        } else {
            suffix -= 1;
        }
    }
    let span = |w: &[Range<usize>]| w[prefix].start..w[w.len() - 1 - suffix].end;
    (span(&wa), span(&wb))
}

/// The byte range of each whitespace-separated word.
fn words(s: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(st)) => {
                out.push(st..i);
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(st) = start {
        out.push(st..s.len());
    }
    out
}

/// A small run for the review's tests: the OP lyric "trough" in E01, E02 and E03, and one other
/// finding in E02. Four findings, two rows.
#[cfg(any(test, feature = "test-support"))]
pub mod tests_support {
    use super::{Review, Row};
    use crate::store::{
        Basis, Check, FileRecord, Finding, FixStretch, LineRef, Run, TimingFix, TrackRecord,
    };
    use crate::text::Occurrence;

    pub fn path(ep: u32) -> String {
        format!("/m/K-On!/K-On! - E{ep:02} - Episode.mkv")
    }

    pub fn file(ep: u32) -> FileRecord {
        let track = |id, sid, name: &str, default| TrackRecord {
            id,
            name: Some(name.into()),
            language: "eng".into(),
            language_bcp47: None,
            sid,
            default,
            read: "cues".into(),
        };
        FileRecord {
            path: path(ep),
            size: 100,
            mtime: jiff::Timestamp::UNIX_EPOCH,
            error: None,
            tracks: vec![
                track(5, 1, "English Full", true),
                track(6, 2, "English Signs", false),
            ],
            lines: 400,
            truncated: None,
            checks: vec![Check::Llm],
        }
    }

    pub fn finding(id: u64, ep: u32, original: &str, suggestion: &str, at_ms: i64) -> Finding {
        Finding {
            id,
            file: path(ep),
            style: if original.contains("trough") {
                "OP en".into()
            } else {
                "Dialogue".into()
            },
            original: original.into(),
            suggestion: suggestion.into(),
            reason: if original.contains("trough") {
                "misspelling of \"through\"".into()
            } else {
                "\"everyday\" should be \"every day\"".into()
            },
            source: "llm".into(),
            occurrences: vec![Occurrence {
                track: 5,
                start_ms: at_ms,
                end_ms: at_ms + 2000,
            }],
            before: vec!["The line before.".into()],
            after: vec!["The line after.".into()],
            timing: None,
        }
    }

    pub fn run() -> Run {
        let typo = "The beat gets trough to our mind";
        let fixed = "The beat gets through to our mind";
        Run {
            name: "2026-10-17T04-39-18".into(),
            files: vec![file(1), file(2), file(3)],
            findings: vec![
                finding(1, 1, typo, fixed, 165_060),
                finding(2, 2, typo, fixed, 165_060),
                finding(
                    3,
                    2,
                    "Let's have snacks together everyday.",
                    "Let's have snacks together every day.",
                    894_190,
                ),
                finding(4, 3, typo, fixed, 165_060),
            ],
            ..Run::default()
        }
    }

    /// A timing finding on episode `ep`'s English Full track (track 5): a frame late, or partial
    /// (late, in sync, late).
    pub fn timing(id: u64, ep: u32, partial: bool) -> Finding {
        let line = |start_ms: i64| LineRef {
            raw: format!("0,Dialogue,,0,0,0,,Line at {start_ms}."),
            start_ms,
            end_ms: start_ms + 2000,
        };
        let stretch = |from_ms, to_ms, frames, lines: Option<(i64, i64)>, frame_ms| FixStretch {
            from_ms,
            to_ms,
            frames,
            first: lines.map(|(first, _)| line(first)),
            last: lines.map(|(_, last)| line(last)),
            frame_ms,
        };
        let (original, suggestion, stretches, evidence) = if partial {
            (
                "partial: 1 frame late, in sync, 1 frame late",
                "shift 1 frame earlier: start – 0:07:55.78, 0:15:40.47 – end",
                vec![
                    stretch(None, Some(475_780), 1, Some((1000, 473_780)), Some(246_788)),
                    stretch(Some(483_510), Some(939_000), 0, None, None),
                    stretch(
                        Some(940_470),
                        None,
                        1,
                        Some((940_470, 1_400_000)),
                        Some(1_196_153),
                    ),
                ],
                vec![245_000, 1_195_000],
            )
        } else {
            (
                "1 frame late",
                "shift 1 frame earlier",
                vec![stretch(
                    None,
                    None,
                    1,
                    Some((1000, 1_400_000)),
                    Some(763_763),
                )],
                vec![762_000],
            )
        };
        Finding {
            id,
            file: path(ep),
            style: String::new(),
            original: original.into(),
            suggestion: suggestion.into(),
            reason: "cuts: 345 times, +1 in 5 of 9 windows".into(),
            source: "timing".into(),
            occurrences: evidence
                .into_iter()
                .map(|start_ms| Occurrence {
                    track: 5,
                    start_ms,
                    end_ms: start_ms + 1800,
                })
                .collect(),
            before: vec![],
            after: vec![],
            timing: Some(TimingFix {
                track: 5,
                frame_ns: 41_708_333,
                stretches,
                unshifted: if partial {
                    vec![line(1_300_000)]
                } else {
                    vec![]
                },
                basis: Basis::Cuts {
                    times: 345,
                    agreeing: 5,
                    windows: 9,
                },
            }),
        }
    }

    /// The sample run and timing findings on its English Full track: a frame late in E01 and E03,
    /// partial in E02.
    pub fn with_timing() -> Review {
        let mut run = run();
        run.findings.extend([
            timing(11, 1, false),
            timing(12, 2, true),
            timing(13, 3, false),
        ]);
        Review::new(run, std::collections::HashMap::new())
    }

    /// `n` findings, one per episode, no two alike: `n` rows.
    pub fn many(n: u32) -> Review {
        let run = Run {
            name: "many".into(),
            files: (1..=n).map(file).collect(),
            findings: (1..=n)
                .map(|ep| {
                    let original = format!("Line {ep} has a typo.");
                    let fixed = format!("Line {ep} has no typo.");
                    finding(ep.into(), ep, &original, &fixed, 1000)
                })
                .collect(),
            ..Run::default()
        };
        Review::new(run, std::collections::HashMap::new())
    }

    pub fn sample() -> Review {
        let m = Review::new(run(), std::collections::HashMap::new());
        assert_eq!(
            m.rows,
            [
                Row {
                    members: vec![0, 1, 3]
                },
                Row { members: vec![2] }
            ]
        );
        m
    }

    /// Episode `ep` as the store would read it back once a scan saved `findings` for it: its record
    /// in `m` (else `file(ep)`), its findings so far with their decisions, then `findings`.
    pub fn read_back(m: &Review, ep: u32, findings: Vec<Finding>) -> Run {
        let p = path(ep);
        let record = m
            .run
            .files
            .iter()
            .find(|r| r.path == p)
            .cloned()
            .unwrap_or_else(|| file(ep));
        let mut kept: Vec<Finding> = m
            .run
            .findings
            .iter()
            .filter(|f| f.file == p)
            .cloned()
            .collect();
        let decisions = kept
            .iter()
            .filter_map(|f| Some((f.id, m.run.decisions.get(&f.id)?.clone())))
            .collect();
        kept.extend(findings);
        Run {
            files: vec![record],
            findings: kept,
            decisions,
            ..Run::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::FileRecord;

    fn label(a: &str, b: &str) -> String {
        let (ra, rb) = changed(a, b);
        format!("{} → {}", &a[ra], &b[rb])
    }

    // What every UI's end comes to: a job running ends the groups and says how to finish it; a
    // job's failure ends them too; an error that stopped no job has no outro.
    #[test]
    fn a_job_or_a_fatal_error_ends_the_groups_and_an_error_alone_keeps_no_outro() {
        let mut m = tests_support::sample();
        let end = |groups, outro| Ending { groups, outro };
        assert_eq!(m.ending(false), end(false, true));
        assert_eq!(m.ending(true), end(false, false));
        m.fatal = Some("the scan panicked: boom".into());
        assert_eq!(m.ending(false), end(true, false));
        m.fatal = None;
        m.progress
            .start_apply(true, "subcheck apply r --dry-run".into());
        assert_eq!(m.ending(false), end(true, true));
        assert_eq!(m.ending(true), end(true, true));
    }

    #[test]
    fn note_for_reads_the_last_record_of_a_path() {
        let mut run = tests_support::run();
        let p = tests_support::path(1);
        let FileRecord { size, mtime, .. } = run.files[0].clone();
        let later = mtime + jiff::SignedDuration::from_secs(1);
        assert_eq!(note_for(&run, &p, Some((size, mtime))), None);
        assert_eq!(
            note_for(&run, &p, Some((size + 1, mtime))),
            Some(FileNote::Changed)
        );
        assert_eq!(
            note_for(&run, &p, Some((size, later))),
            Some(FileNote::Changed)
        );
        assert_eq!(note_for(&run, &p, None), Some(FileNote::Missing));
        assert_eq!(
            note_for(&run, "/m/unrecorded.mkv", Some((size, mtime))),
            None
        );
        // Checked twice, the file is compared with what the second check saw.
        run.files.push(FileRecord {
            size: size + 1,
            ..run.files[0].clone()
        });
        assert_eq!(note_for(&run, &p, Some((size + 1, mtime))), None);
        assert_eq!(
            note_for(&run, &p, Some((size, mtime))),
            Some(FileNote::Changed)
        );
    }

    #[test]
    fn changed_words_are_set_apart_from_the_shared_ends() {
        assert_eq!(
            label(
                "The beat gets trough to our mind",
                "The beat gets through to our mind"
            ),
            "trough → through"
        );
        assert_eq!(
            label(
                "Let's have snacks together everyday.",
                "Let's have snacks together every day."
            ),
            "everyday. → every day."
        );
        // Only removed, or only added: one word of context keeps both sides.
        assert_eq!(
            label(
                "We're a making chocolate cake.",
                "We're making chocolate cake."
            ),
            "We're a → We're"
        );
        assert_eq!(
            label(
                "We happen share the same route",
                "We happen to share the same route"
            ),
            "happen → happen to"
        );
        assert_eq!(label("Wel...", "Well..."), "Wel... → Well...");
        assert_eq!(label("a b", "x a b"), "a → x a");
        assert_eq!(changed("same words", "same  words"), (0..0, 0..0));
        assert_eq!(changed("", "new"), (0..0, 0..3));
    }

    #[test]
    fn a_seasons_timing_findings_on_one_track_are_one_row_whatever_their_fix() {
        let m = tests_support::with_timing();
        // The typo rows as before, and the three episodes' track 5 in one row, at its first member.
        assert_eq!(
            m.rows,
            [
                Row {
                    members: vec![0, 1, 3]
                },
                Row {
                    members: vec![4, 5, 6]
                },
                Row { members: vec![2] },
            ]
        );
        assert_eq!(m.track_name(4), "English Full");
        assert_eq!(m.fix_summary(&[4, 5, 6]), "−1 frame ×2 · partial ×1");
        assert_eq!(m.fix_summary(&[4, 6]), "−1 frame");
        // Another folder's track of the same name is a row of its own.
        let mut other = tests_support::with_timing();
        let path = "/m/K-On!!/K-On!! - E03 - Episode.mkv";
        other.run.findings[6].file = path.into();
        other.run.files.push(FileRecord {
            path: path.into(),
            ..tests_support::file(3)
        });
        assert_eq!(group(&other.run).0.len(), 4);
    }

    #[test]
    fn a_files_tracks_that_go_by_one_name_are_rows_apart_with_their_ids() {
        // Tracks 5 and 6 both unnamed (both `eng`), then both named "English".
        for name in [None, Some("English")] {
            let mut m = tests_support::with_timing();
            for file in &mut m.run.files {
                for t in &mut file.tracks {
                    t.name = name.map(Into::into);
                }
            }
            // E01's track 6, a frame late too.
            let mut signs = tests_support::timing(14, 1, false);
            if let Some(t) = &mut signs.timing {
                t.track = 6;
            }
            m.run.findings.push(signs);
            let (rows, tracks) = group(&m.run);
            let timing_rows: Vec<Vec<usize>> = rows
                .into_iter()
                .map(|r| r.members)
                .filter(|members| members.iter().all(|&i| m.run.findings[i].timing.is_some()))
                .collect();
            assert_eq!(timing_rows, [vec![4, 5, 6], vec![7]], "{name:?}");
            let labels = [4, 7].map(|i| tracks[i].as_ref().unwrap().label());
            match name {
                None => assert_eq!(labels, ["track 5 (eng)", "track 6 (eng)"]),
                Some(_) => assert_eq!(labels, ["track 5 English", "track 6 English"]),
            }
        }
    }

    #[test]
    fn an_entrys_state_is_pending_while_anything_it_shows_is_and_none_when_it_shows_a_mix() {
        use crate::review::update::tests_support::decide_as;
        let mut m = tests_support::sample();
        let (row, e01) = (Entry::Row(0), Entry::Member(0, 0));
        assert_eq!(m.entry_state(row), Some(State::Pending));
        // Decided differently, with one still to do: the row is still to do.
        decide_as(&mut m, &[1], State::Accepted, None);
        decide_as(&mut m, &[2], State::Rejected, None);
        assert_eq!(m.entry_state(row), Some(State::Pending));
        assert_eq!(m.entry_state(e01), Some(State::Accepted));
        // Its one pending member hidden by the filter: the row goes by the member it shows.
        m.filter = Filter::Only(State::Rejected);
        assert_eq!(m.entry_state(row), Some(State::Rejected));
        m.filter = Filter::All;
        // None left to do: `None` when they differ, else what they agree on.
        decide_as(&mut m, &[4], State::Rejected, None);
        assert_eq!(m.entry_state(row), None);
        m.filter = Filter::Only(State::Rejected);
        assert_eq!(m.entry_state(row), Some(State::Rejected));
    }

    // A row's mark in the list: the one alone when every member it shows has it; else each with
    // how many have it, in the order its members come, those with none left out.
    #[test]
    fn a_rows_mark_is_one_alone_when_every_member_shown_has_it_else_each_with_its_count() {
        use crate::review::update::tests_support::decide_as;
        let mut m = tests_support::sample();
        assert_eq!(m.row_mark(0), None);
        m.run.applied.extend([1, 2, 4]);
        assert_eq!(m.row_mark(0).as_deref(), Some("applied"));
        m.run.applied.remove(&4);
        assert_eq!(m.row_mark(0).as_deref(), Some("applied 2"));
        // What apply said comes before the store's mark; left by hand for two reasons, one kind.
        m.marks
            .insert(1, Mark::ByHand("two fixes on one line".into()));
        m.marks
            .insert(4, Mark::ByHand("per-letter typesetting".into()));
        assert_eq!(m.row_mark(0).as_deref(), Some("by hand 2 · applied 1"));
        // Of the members the filter shows.
        decide_as(&mut m, &[2], State::Accepted, None);
        m.filter = Filter::Only(State::Pending);
        assert_eq!(m.row_mark(0).as_deref(), Some("by hand"));
    }

    // A timing finding's track as the list names it: by its own name; with its ID before it when
    // another track of its file has the name, or it has none; by its ID alone when its file's
    // record does not list it.
    #[test]
    fn a_timing_findings_track_is_named_by_its_own_name_else_with_its_id() {
        let tracks = made_up_tracks();
        let on = |id| {
            let mut f = tests_support::timing(1, 1, false);
            if let Some(t) = &mut f.timing {
                t.track = id;
            }
            f
        };
        assert_eq!(track_name(&tracks, &on(3)), "Signs");
        assert_eq!(track_name(&tracks, &on(4)), "track 4 English");
        assert_eq!(track_name(&tracks, &on(6)), "track 6 (en-US)");
        assert_eq!(track_name(&tracks, &on(9)), "track 9");
        let line = tests_support::finding(1, 1, "a", "b", 0);
        assert_eq!(track_name(&tracks, &line), "");
    }

    /// A file's subtitle tracks, of each kind of name: one of its own, one two tracks share, and
    /// none, with a BCP 47 tag.
    fn made_up_tracks() -> Vec<TrackRecord> {
        let track = |id, name: Option<&str>, bcp47: Option<&str>| TrackRecord {
            id,
            name: name.map(Into::into),
            language: "eng".into(),
            language_bcp47: bcp47.map(Into::into),
            sid: id - 2,
            default: false,
            read: "cues".into(),
        };
        vec![
            track(3, Some("Signs"), None),
            track(4, Some("English"), None),
            track(5, Some("English"), Some("en-GB")),
            track(6, None, Some("en-US")),
        ]
    }

    /// A timing finding's track among its file record's `tracks`, as the list names it
    /// ([`ListTrack::label`]); empty for other findings.
    fn track_name(tracks: &[TrackRecord], f: &Finding) -> String {
        f.timing
            .as_ref()
            .map_or_else(String::new, |t| ListTrack::of(tracks, t.track).label())
    }

    #[test]
    fn a_timing_findings_track_is_data_its_name_in_the_list_is_worded_from() {
        let tracks = made_up_tracks();
        let named = |id, name: Option<&str>, language: Option<&str>, apart| ListTrack {
            id,
            name: name.map(Into::into),
            language: language.map(Into::into),
            apart,
        };
        assert_eq!(
            ListTrack::of(&tracks, 3),
            named(3, Some("Signs"), Some("eng"), true)
        );
        assert_eq!(
            ListTrack::of(&tracks, 5),
            named(5, Some("English"), Some("en-GB"), false)
        );
        assert_eq!(
            ListTrack::of(&tracks, 6),
            named(6, None, Some("en-US"), false)
        );
        assert_eq!(ListTrack::of(&tracks, 9), named(9, None, None, false));
        // Every form of its name in the list, those `of` never makes too (a name with no
        // language, `apart` with no name), as a UI or a test may build any of them.
        for (track, label) in [
            (named(3, Some("Signs"), Some("eng"), true), "Signs"),
            (named(3, Some("Signs"), None, true), "Signs"),
            (
                named(4, Some("English"), Some("eng"), false),
                "track 4 English",
            ),
            (named(4, Some("English"), None, false), "track 4 English"),
            (named(6, None, Some("en-US"), false), "track 6 (en-US)"),
            (named(6, None, Some("en-US"), true), "track 6 (en-US)"),
            (named(9, None, None, false), "track 9"),
            (named(9, None, None, true), "track 9"),
        ] {
            assert_eq!(track.label(), label, "{track:?}");
        }
        let m = tests_support::with_timing();
        assert_eq!(
            m.list_track(4),
            Some(&ListTrack::of(&m.run.files[0].tracks, 5))
        );
        assert_eq!(m.list_track(0), None);
    }

    // A row's fixes as data: by fix, the most common first, those as common in the order they
    // come; none for a line finding.
    #[test]
    fn a_rows_fixes_are_counted_by_fix_the_most_common_first() {
        let m = tests_support::with_timing();
        assert_eq!(m.fixes(&[5, 4, 6]), [(Fix::Shift(1), 2), (Fix::Partial, 1)]);
        assert_eq!(m.fixes(&[5, 4]), [(Fix::Partial, 1), (Fix::Shift(1), 1)]);
        assert_eq!(m.fixes(&[0]), []);
    }

    #[test]
    fn a_fix_in_a_word_counts_the_frames_its_lines_move() {
        let mut f = tests_support::timing(1, 1, false);
        assert_eq!(fix_label(&f), "−1 frame");
        if let Some(t) = &mut f.timing {
            t.stretches[0].frames = -2;
        }
        assert_eq!(fix_label(&f), "+2 frames");
        assert_eq!(fix_label(&tests_support::timing(1, 1, true)), "partial");
        assert_eq!(fix_label(&tests_support::finding(1, 1, "a", "b", 0)), "");
    }

    #[test]
    fn labels_grow_when_two_files_share_an_episode() {
        let one_show = tests_support::run();
        assert_eq!(labels(&one_show)[&tests_support::path(2)], "E02");
        // A second season has its own E02.
        let mut two_seasons = tests_support::run();
        let other = "/m/K-On!!/K-On!! - E02 - Clean-up.mkv";
        two_seasons.findings[0].file = other.into();
        let names = labels(&two_seasons);
        assert_eq!(names[other], "K-On!! - E02");
        assert_eq!(names[&tests_support::path(2)], "K-On! - E02");
        assert_eq!(names[&tests_support::path(3)], "K-On! - E03");
        // Seasons in folders of their own, with the same file names: the folder tells them apart.
        let mut folders = tests_support::run();
        folders.findings[0].file = "/m/Show/S1/Show - E01.mkv".into();
        folders.findings[1].file = "/m/Show/S2/Show - E01.mkv".into();
        let names = labels(&folders);
        assert_eq!(names["/m/Show/S1/Show - E01.mkv"], "S1/Show - E01");
        assert_eq!(names["/m/Show/S2/Show - E01.mkv"], "S2/Show - E01");
    }

    #[test]
    fn episodes_come_from_the_file_name() {
        assert_eq!(
            episode("/m/K-On!/K-On! - E01 - Disband the Club.mkv"),
            "E01"
        );
        assert_eq!(episode("/m/Show.S01E02.1080p.mkv"), "E02");
        assert_eq!(episode("/m/EVANGELION 1.11.mkv"), "EVANGELION 1");
        assert_eq!(episode("/m/Movie (2010).mkv"), "Movie (2010)");
        assert_eq!(
            long_episode("/m/K-On!!/K-On!! - E01 - Seniors.mkv"),
            "K-On!! - E01"
        );
        assert_eq!(long_episode("/m/Show.S01E02.1080p.mkv"), "Show.S01E02");
    }

    const LYRIC: &str = "The beat gets trough to our mind";
    const FIXED: &str = "The beat gets through to our mind";

    #[test]
    fn an_edits_target_is_all_its_findings_or_nothing() {
        let mut m = tests_support::sample();
        let entry = m.entry_key(Entry::Row(0));
        m.editing = Some(Edit {
            entry: entry.clone(),
            findings: vec![1, 4],
        });
        assert_eq!(m.edit_target(), Some((Entry::Row(0), vec![0, 3])));
        m.editing = Some(Edit {
            entry,
            findings: vec![1, 99, 4],
        });
        assert_eq!(m.edit_target(), None);
    }

    #[test]
    fn a_file_read_back_swaps_its_findings_and_what_pointed_at_one_that_went_moves_to_its_match() {
        use tests_support::{file, finding, path};
        let mut m = tests_support::sample();
        // The lyric's row expanded, its E02 member selected and its key held, as a click holds
        // it; an edit under way on E02's snacks row; an undo step over both of E02's findings; E02
        // changed on disk.
        m.expanded.insert(0);
        m.selected = 2;
        assert_eq!(m.selected_entry(), Some(Entry::Member(0, 1)));
        let clicked = m.entry_key(Entry::Member(0, 1));
        m.editing = Some(Edit {
            entry: m.entry_key(Entry::Row(1)),
            findings: vec![3],
        });
        m.undo = vec![vec![(2, None), (3, None)]];
        m.notes.insert(path(2), FileNote::Changed);
        // E02 checked again: the lyric comes back as 5, the snacks line does not, and a new line
        // early in the episode is 6.
        let view = Run {
            files: vec![file(2)],
            findings: vec![
                finding(5, 2, LYRIC, FIXED, 165_060),
                finding(6, 2, "An erly line.", "An early line.", 1_000),
            ],
            ..Run::default()
        };
        let new = m.refresh(&path(2), view);
        let ids: Vec<u64> = m.run.findings.iter().map(|f| f.id).collect();
        assert_eq!(ids, [1, 4, 5, 6]);
        assert_eq!(new, [2, 3]);
        assert_eq!(
            m.rows,
            [
                Row {
                    members: vec![0, 2, 1]
                },
                Row { members: vec![3] },
            ]
        );
        assert_eq!(m.expanded, HashSet::from([0]));
        assert_eq!(m.selected_entry(), Some(Entry::Member(0, 2)));
        assert_eq!(m.resolve(&clicked), Some(Entry::Member(0, 2)));
        assert_eq!(m.editing, None);
        assert_eq!(
            m.status,
            Some(Status::EditEnded {
                label: "E02".into()
            })
        );
        assert_eq!(m.undo, [vec![(5, None)]]);
        assert_eq!(m.notes.get(&path(2)), None, "just checked as it is");
    }

    #[test]
    fn an_entry_key_finds_its_entry_after_the_rows_are_remade() {
        use tests_support::{file, finding, path};
        let mut m = tests_support::sample();
        // The lyric's row (findings 1, 2 and 4), its E02 member, and the snacks row (3).
        let lyric = m.entry_key(Entry::Row(0));
        let e02 = m.entry_key(Entry::Member(0, 1));
        let snacks = m.entry_key(Entry::Row(1));
        assert_eq!(lyric, EntryKey::Row(vec![1, 2, 4]));
        // E01 read back with a line before its lyric: a row of its own comes first.
        let e01 = Run {
            files: vec![file(1)],
            findings: vec![
                finding(1, 1, LYRIC, FIXED, 165_060),
                finding(6, 1, "Zero.", "Zero!", 500),
            ],
            ..Run::default()
        };
        m.refresh(&path(1), e01);
        assert_eq!(m.resolve(&lyric), Some(Entry::Row(1)));
        assert_eq!(m.resolve(&snacks), Some(Entry::Row(2)));
        // The selection, on the lyric's row, went with it.
        assert_eq!(m.selected_key(), Some(lyric.clone()));
        // E02 checked again: its lyric comes back as 7, and the snacks line does not.
        let e02_again = Run {
            files: vec![file(2)],
            findings: vec![finding(7, 2, LYRIC, FIXED, 165_060)],
            ..Run::default()
        };
        m.refresh(&path(2), e02_again);
        let seven = m.run.findings.iter().position(|f| f.id == 7).unwrap();
        assert_eq!(m.resolve(&e02), Some(Entry::Member(1, seven)));
        assert_eq!(m.resolve(&snacks), None);
        assert_eq!(m.resolve(&lyric), Some(Entry::Row(1)));
        // And once more: the lyric is 8 now, and the key that named 2 follows it through 7.
        let e02_thrice = Run {
            files: vec![file(2)],
            findings: vec![finding(8, 2, LYRIC, FIXED, 165_060)],
            ..Run::default()
        };
        m.refresh(&path(2), e02_thrice);
        let eight = m.run.findings.iter().position(|f| f.id == 8).unwrap();
        assert_eq!(m.resolve(&e02), Some(Entry::Member(1, eight)));
    }

    #[test]
    fn a_row_left_with_no_findings_goes_and_the_selection_falls_to_the_nearest_entry() {
        use tests_support::{file, finding, path};
        let mut m = tests_support::sample();
        m.selected = 1;
        assert_eq!(m.selected_entry(), Some(Entry::Row(1)), "the snacks row");
        // E02 read back with its lyric, unchanged, and no snacks line.
        let view = Run {
            files: vec![file(2)],
            findings: vec![finding(2, 2, LYRIC, FIXED, 165_060)],
            ..Run::default()
        };
        assert_eq!(m.refresh(&path(2), view), Vec::<usize>::new());
        assert_eq!(
            m.rows,
            [Row {
                members: vec![0, 2, 1]
            }]
        );
        assert_eq!(m.selected_entry(), Some(Entry::Row(0)));
    }

    #[test]
    fn files_read_back_into_an_empty_list_select_the_first_pending_row() {
        let mut m = Review::new(
            Run {
                name: "r".into(),
                ..Run::default()
            },
            HashMap::new(),
        );
        let run = tests_support::run();
        for ep in 1..=3 {
            let p = tests_support::path(ep);
            let view = Run {
                files: vec![tests_support::file(ep)],
                findings: run
                    .findings
                    .iter()
                    .filter(|f| f.file == p)
                    .cloned()
                    .collect(),
                ..Run::default()
            };
            m.refresh(&p, view);
        }
        assert_eq!(m.rows, tests_support::sample().rows);
        assert_eq!(m.selected_entry(), Some(Entry::Row(0)));
        assert_eq!(m.label(&tests_support::path(2)), "E02");
    }

    #[test]
    fn switching_views_keeps_what_points_at_findings_both_show() {
        use tests_support::{path, sample};
        // The other view: the sample without E02.
        let without_e02 = |m: &Review| {
            let mut other = m.run.clone();
            other.of_paths = true;
            other.paths = vec!["/m/K-On!".into()];
            other.findings.retain(|f| f.file != path(2));
            other.files.retain(|f| f.path != path(2));
            other
        };
        let mut m = sample();
        // The lyric row (findings 1, 2 and 4) expanded, the selection on its E03 member, a filter,
        // and undo steps on findings 3 and 4, and on 2.
        m.expanded.insert(0);
        m.filter = Filter::Only(State::Pending);
        m.selected = m
            .entries()
            .iter()
            .position(|&e| e == Entry::Member(0, 3))
            .unwrap();
        m.undo = vec![vec![(3, None), (4, None)], vec![(2, None)]];
        let other = without_e02(&m);
        m.switch(other);
        assert!(m.run.of_paths);
        // Findings 1 and 4, still one row, still expanded; the selection on finding 4.
        assert_eq!(
            m.rows,
            [Row {
                members: vec![0, 1]
            }]
        );
        assert!(m.expanded.contains(&0));
        assert_eq!(m.selected_entry(), Some(Entry::Member(0, 1)));
        assert_eq!(m.filter, Filter::Only(State::Pending));
        assert_eq!(m.undo, [vec![(4, None)]]);
        assert_eq!(m.progress.files.len(), 2);
        // A selection whose finding the view does not show goes to the first row with a pending
        // finding.
        let mut m = sample();
        m.selected = m
            .entries()
            .iter()
            .position(|&e| e == Entry::Row(1))
            .unwrap();
        let other = without_e02(&m);
        m.switch(other);
        assert_eq!(m.selected, 0);
    }
}
