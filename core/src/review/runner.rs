//! What review's tasks do ([`Runner`]): the store's reads and writes, and the jobs that check files
//! again and apply fixes, each on a thread of its own ([`super::jobs`]). Nothing here knows the
//! UI: the jobs send their inputs to the channel the runner was given, the one `update`'s
//! inputs come in on, the terminal's loop's or a session's.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use anyhow::Context;

use super::jobs;
use super::model::{self, FileNote};
use super::progress;
use super::update::{Input, Job, Switch, Task};
use crate::llm::Backend;
use crate::scan::{self, file_stamp};
use crate::store::{Run, Selector, ShowFolder, Store};

/// Runs `update`'s tasks for a UI: through `store`, the connection review writes through; with
/// `state`, the state directory, where apply writes the fixed tracks; and with `tx`, a sender on
/// the channel `update`'s inputs come in on (the terminal's loop's, or a session's), where the
/// jobs it starts send what they do. `T` is what that channel carries: an [`Input`], or a message
/// of that loop's own that one becomes.
pub struct Runner<T> {
    store: Store,
    state: PathBuf,
    tx: Sender<T>,
}

impl<T: From<Input> + Send + 'static> Runner<T> {
    pub fn new(store: Store, state: PathBuf, tx: Sender<T>) -> Runner<T> {
        Runner { store, state, tx }
    }

    /// The store's show folders ([`Store::show_folders`]), read through the connection review's
    /// tasks write through.
    pub fn show_folders(&self) -> rusqlite::Result<Vec<ShowFolder>> {
        self.store.show_folders()
    }

    /// Run `task` and say how it went. A job it starts (`Recheck`, `StartApply`) runs on a
    /// thread of its own, which sends the rest to the channel.
    pub fn run(&mut self, task: Task) -> Input {
        let Runner { store, state, tx } = self;
        match task {
            Task::Write { purpose, decisions } => {
                let result = store.decide(&decisions).map_err(|e| e.to_string());
                Input::Written {
                    purpose,
                    decisions,
                    result,
                }
            }
            Task::ReadBack(path) => {
                let view = store.file_view(&path).map_err(|e| e.to_string());
                Input::ReadBack { path, view }
            }
            Task::FindRun { path, run } => {
                let found = match &run {
                    Some(name) => store.scan_named(name),
                    None => store.newest_scan_naming(&path),
                };
                let found = match found {
                    Ok(Some(scan)) => Ok((scan.name, scan.header.backend)),
                    Ok(None) => Err(match run {
                        Some(name) => format!("no run named {name}"),
                        None => format!("{}: no run named it", model::episode(&path)),
                    }),
                    Err(e) => Err(e.to_string()),
                };
                Input::RunFound { path, found }
            }
            Task::Recheck { run, paths } => {
                let (here, missing): (Vec<String>, Vec<String>) =
                    paths.iter().cloned().partition(|p| Path::new(p).is_file());
                let job = if here.is_empty() {
                    Err("nothing to check".into())
                } else {
                    let files: Vec<PathBuf> = here.iter().map(PathBuf::from).collect();
                    recheck(store, tx, &run, &files)
                };
                Input::Started {
                    job,
                    missing,
                    paths,
                }
            }
            Task::StartApply {
                run,
                dry_run,
                karaoke,
            } => Input::ApplyStarted(store.reopen().map_err(|e| e.to_string()).and_then(
                |applying| {
                    jobs::apply(run, dry_run, karaoke, applying, state.clone(), tx.clone())
                        .map_err(|e| e.to_string())
                },
            )),
            Task::Stamp(moment) => Input::Stamped {
                stamp: file_stamp(Path::new(&moment.file)).ok(),
                moment,
            },
            Task::Switch(to) => Input::Switched(
                other_view(store, &to)
                    .map(|run| {
                        let notes = file_notes(&run);
                        (run, notes)
                    })
                    .map_err(|e| format!("{e:#}")),
            ),
        }
    }
}

/// How each of the run's files with findings differs from when it was scanned, by path; files as
/// they were are left out.
pub fn file_notes(run: &Run) -> HashMap<String, FileNote> {
    // Only files with findings ever show a note, and on a network share each check is a round trip.
    let shown: HashSet<&str> = run.findings.iter().map(|f| f.file.as_str()).collect();
    let mut notes = HashMap::new();
    // The newest record of each file wins: a file a run checked twice keeps its last stamp.
    for f in run.files.iter().filter(|f| shown.contains(f.path.as_str())) {
        let note = match file_stamp(Path::new(&f.path)) {
            Ok((size, mtime)) if size == f.size && mtime == f.mtime => None,
            Ok(_) => Some(FileNote::Changed),
            Err(_) => Some(FileNote::Missing),
        };
        match note {
            Some(n) => notes.insert(f.path.clone(), n),
            None => notes.remove(&f.path),
        };
    }
    notes
}

/// The view [`Task::Switch`] asks for, read from the store: a run's folders, the newest run that
/// named a file, what a selector's words select, as `Session::open` reads its first, or a folder's
/// files. The words are read here, not in `update`: a word that is a path on disk is a path.
fn other_view(store: &Store, to: &Switch) -> anyhow::Result<Run> {
    match to {
        Switch::FoldersOf(name) => {
            let scan = store
                .scan_named(name)?
                .with_context(|| format!("no run named {name}"))?;
            store.folders_view(&scan)
        }
        Switch::RunNaming(path) => {
            let scan = store
                .newest_scan_naming(path)?
                .with_context(|| format!("{}: no run named it", model::episode(path)))?;
            Ok(store.run_view(&scan)?)
        }
        Switch::Selector(words) => store.view(&Selector::parse(words)?),
        Switch::Folder(folder) => store.folder_view(Path::new(folder)),
    }
}

/// Check `files` again into run `run`, as `subcheck scan --run <run> --force <files>` does: with
/// the run's backend and languages, on a thread of its own that sends to `tx`.
fn recheck<T: From<Input> + Send + 'static>(
    store: &mut Store,
    tx: &Sender<T>,
    run: &str,
    files: &[PathBuf],
) -> Result<Job, String> {
    let scan = store
        .scan_named(run)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no run named {run}"))?;
    // One this subcheck no longer knows would fail every file at the LLM: refused here instead.
    Backend::from_label(&scan.header.backend).map_err(|e| format!("run {run}: {e}"))?;
    let (backend, langs) = scan::resume(&scan, None, None, &[])?;
    let opts = scan::Options {
        chunk_lines: backend.chunk_lines,
        backend,
        jobs: scan::DEFAULT_JOBS,
        langs,
        force: true,
        verbose: false,
        into: Some(run.to_string()),
    };
    let planned = scan::plan(files, &opts, store, &mut io::sink()).map_err(|e| format!("{e:#}"))?;
    let scan::Planned::Scan(plan) = planned else {
        return Err("nothing to check".into());
    };
    let scans = store.reopen().map_err(|e| e.to_string())?;
    jobs::scan(plan, opts, scans, tx.clone()).map_err(|e| e.to_string())?;
    Ok(Job {
        run: run.to_string(),
        resume: progress::again_command(run, files),
    })
}

/// The run a UI opens on with a scan: the run as the store shows it. A file the scan checks again
/// keeps its findings in the list until the scan saves the new ones (see `Review::refresh`); a file
/// it skips is there from the start.
pub fn opening(store: &Store, plan: &scan::Plan) -> rusqlite::Result<Run> {
    store.run_view(plan.scan())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    use crate::apply;

    #[test]
    fn files_that_changed_or_went_missing_are_noted() {
        let dir = tempfile::tempdir().unwrap();
        let same = dir.path().join("same.mkv");
        let changed = dir.path().join("changed.mkv");
        std::fs::write(&same, b"1234").unwrap();
        std::fs::write(&changed, b"1234").unwrap();
        let record = |p: &Path| {
            let (size, mtime) = file_stamp(p).unwrap();
            crate::store::FileRecord {
                path: p.to_string_lossy().into_owned(),
                size,
                mtime,
                error: None,
                tracks: vec![],
                lines: 0,
                truncated: None,
                checks: vec![crate::store::Check::Llm],
            }
        };
        let gone = |name: &str| crate::store::FileRecord {
            path: dir.path().join(name).to_string_lossy().into_owned(),
            ..record(&same)
        };
        let files = vec![
            record(&same),
            record(&changed),
            gone("gone.mkv"),
            gone("clean-and-gone.mkv"),
        ];
        // Findings in the first three; the fourth had none, so it is never checked.
        let findings = files[..3]
            .iter()
            .map(|f| crate::store::Finding {
                file: f.path.clone(),
                ..model::tests_support::finding(1, 1, "a", "b", 0)
            })
            .collect();
        let run = Run {
            files,
            findings,
            ..Run::default()
        };
        std::fs::write(&changed, b"12345").unwrap();
        let notes = file_notes(&run);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert_eq!(notes[&run.files[1].path], FileNote::Changed);
        assert_eq!(notes[&run.files[2].path], FileNote::Missing);
    }

    // As `Session::open` reads its selector: a run by its name; a path on disk as a path, with
    // what is recorded under it or, with nothing, why; or why there is no such run.
    #[test]
    fn a_selector_opens_the_view_its_words_select_or_says_why_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_at(&dir.path().join("subcheck.db")).unwrap();
        let header = crate::store::RunHeader {
            started: jiff::Timestamp::UNIX_EPOCH,
            backend: "claude".into(),
            command: vec![],
            prompt_version: 1,
            version: "0".into(),
            langs: vec!["eng".into()],
        };
        let name = store.begin_scan(&header, &[]).unwrap().name;
        // A file on disk, a finding recorded in it.
        let file = dir.path().join("E01.mkv");
        std::fs::write(&file, "1").unwrap();
        let path = file.to_string_lossy().into_owned();
        let scan = store
            .begin_scan(&header, std::slice::from_ref(&file))
            .unwrap();
        let record = crate::store::FileRecord {
            path: path.clone(),
            ..model::tests_support::file(1)
        };
        let attempt = crate::store::Attempt {
            scan: scan.id,
            path: &record.path,
            size: record.size,
            mtime: record.mtime,
            tracks: &record.tracks,
            truncated: None,
            at: jiff::Timestamp::UNIX_EPOCH,
        };
        let made = crate::store::Made {
            with: crate::store::CheckedWith::new("claude", &["eng".into()], 1),
            lines: 1,
            findings: vec![crate::store::Finding {
                file: path.clone(),
                ..model::tests_support::finding(0, 1, "a", "b", 0)
            }],
        };
        store.save(&attempt, vec![made]).unwrap();
        let (tx, _rx) = mpsc::channel::<Input>();
        let mut runner = Runner::new(store, dir.path().join("state"), tx);
        let mut open = |words: Vec<String>| match runner.run(Task::Switch(Switch::Selector(words)))
        {
            Input::Switched(view) => view.map(|(run, _)| (run.shown_name(), run.findings.len())),
            other => panic!("{other:?}"),
        };
        assert_eq!(open(vec![name.clone()]), Ok((format!("run {name}"), 0)));
        assert_eq!(open(vec![path]), Ok(("E01.mkv".to_string(), 1)));
        let folder = dir.path().join("empty");
        std::fs::create_dir(&folder).unwrap();
        let folder = folder.to_string_lossy().into_owned();
        let said = open(vec![folder.clone()]).unwrap_err();
        assert!(said.starts_with("nothing recorded under "), "{said}");
        assert!(said.contains(&folder), "{said}");
        assert_eq!(open(vec!["nope".into()]), Err("no run named nope".into()));
    }

    // A folder not on disk, as a share not mounted is: by its path it opens, where the same path
    // as a selector's words names a run.
    #[test]
    fn a_folder_opens_by_its_path_whether_or_not_it_is_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_at(&dir.path().join("subcheck.db")).unwrap();
        let header = crate::store::RunHeader {
            started: jiff::Timestamp::UNIX_EPOCH,
            backend: "claude".into(),
            command: vec![],
            prompt_version: 1,
            version: "0".into(),
            langs: vec!["eng".into()],
        };
        let record = model::tests_support::file(1);
        let scan = store
            .begin_scan(&header, &[PathBuf::from(&record.path)])
            .unwrap();
        let attempt = crate::store::Attempt {
            scan: scan.id,
            path: &record.path,
            size: record.size,
            mtime: record.mtime,
            tracks: &record.tracks,
            truncated: None,
            at: jiff::Timestamp::UNIX_EPOCH,
        };
        let made = crate::store::Made {
            with: crate::store::CheckedWith::new("claude", &["eng".into()], 1),
            lines: 1,
            findings: vec![model::tests_support::finding(0, 1, "a", "b", 0)],
        };
        store.save(&attempt, vec![made]).unwrap();
        let (tx, _rx) = mpsc::channel::<Input>();
        let mut runner = Runner::new(store, dir.path().join("state"), tx);
        let folder = "/m/K-On!".to_string();
        assert!(!Path::new(&folder).exists());
        let mut open = |to: Switch| match runner.run(Task::Switch(to)) {
            Input::Switched(view) => view.map(|(run, _)| (run.shown_name(), run.findings.len())),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            open(Switch::Folder(folder.clone())),
            Ok(("K-On!".to_string(), 1))
        );
        assert_eq!(
            open(Switch::Selector(vec![folder])),
            Err("no run named /m/K-On!".into())
        );
        let said = open(Switch::Folder("/m/Nothing".into())).unwrap_err();
        assert_eq!(said, "nothing recorded under '/m/Nothing'");
    }

    #[test]
    fn a_runner_on_a_channel_of_inputs_gets_its_jobs_inputs_there() {
        // As a UI with no messages of its own would have it: inputs alone on the channel.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_at(&dir.path().join("subcheck.db")).unwrap();
        let (tx, rx) = mpsc::channel::<Input>();
        let mut runner = Runner::new(store, dir.path().join("state"), tx);
        let task = Task::StartApply {
            run: Run::default(),
            dry_run: true,
            karaoke: false,
        };
        assert_eq!(runner.run(task), Input::ApplyStarted(Ok(())));
        // Apply's thread then holds the channel's last sender, so a lost send fails here at once.
        drop(runner);
        // Nothing accepted: the apply ends at once, and says so from its thread.
        assert_eq!(
            rx.recv_timeout(Duration::from_mins(1)).unwrap(),
            Input::ApplyEnded(Ok(apply::Summary::default()))
        );
    }
}
