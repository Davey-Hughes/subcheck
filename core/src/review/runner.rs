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
use crate::llm::Via;
use crate::scan::{self, file_stamp};
use crate::settings::Settings;
use crate::store::{Run, ScanRow, Selector, ShowFolder, Store};

/// Runs `update`'s tasks for a UI: through `store`, the connection review writes through; with
/// `state`, the state directory, where apply writes the fixed tracks; and with `tx`, a sender on
/// the channel `update`'s inputs come in on (the terminal's loop's, or a session's), where the
/// jobs it starts send what they do. `T` is what that channel carries: an [`Input`], or a message
/// of that loop's own that one becomes.
pub struct Runner<T> {
    store: Store,
    state: PathBuf,
    tx: Sender<T>,
    /// The settings file a recheck reads; `None` for this process's ([`Settings::load`]).
    settings: Option<PathBuf>,
}

impl<T: From<Input> + Send + 'static> Runner<T> {
    pub fn new(store: Store, state: PathBuf, tx: Sender<T>) -> Runner<T> {
        Runner {
            store,
            state,
            tx,
            settings: None,
        }
    }

    /// This runner, its rechecks reading the settings file at `path` rather than this process's.
    /// For tests, which run on threads of one process: `SUBCHECK_CONFIG` set in it would reach
    /// every test beside this one, and left unset lets the developer's own file in.
    #[must_use]
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_settings_at(self, path: PathBuf) -> Runner<T> {
        Runner {
            settings: Some(path),
            ..self
        }
    }

    /// The store's show folders ([`Store::show_folders`]), read through the connection review's
    /// tasks write through.
    pub fn show_folders(&self) -> rusqlite::Result<Vec<ShowFolder>> {
        self.store.show_folders()
    }

    /// Run `task` and say how it went. A job it starts (`Recheck`, `StartApply`) runs on a
    /// thread of its own, which sends the rest to the channel.
    pub fn run(&mut self, task: Task) -> Input {
        let Runner {
            store,
            state,
            tx,
            settings,
        } = self;
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
                    recheck(store, tx, settings.as_deref(), &run, &files)
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

/// The options to check run `run`'s files again with: its backend and languages, and that
/// backend's jobs and chunk size as `settings` have them now.
fn recheck_options(
    scan: &ScanRow,
    run: &str,
    settings: &Settings,
) -> Result<scan::Options, String> {
    let (backend, langs) = scan::resume(scan, None, None, &[], settings)?;
    // An LLM that cannot be built again would fail every file at the LLM: refused here instead.
    if let Via::Unknown(why) = &backend.via {
        return Err(format!("run {run} was scanned with {why}"));
    }
    Ok(scan::Options {
        chunk_lines: None,
        jobs: backend.jobs,
        backend,
        langs,
        force: true,
        verbose: false,
        into: Some(run.to_string()),
    })
}

/// Check `files` again into run `run`, as `subcheck scan --run <run> --force <files>` does: with
/// the run's backend and languages, on a thread of its own that sends to `tx`. An entry is as the
/// settings file at `settings` has it now, or this process's file when that is `None`. The server
/// and the key command are asked, and a model loaded, on the job's thread ([`jobs::recheck`]), never
/// here: this runs on the screen's loop or the window's core thread, and a server or a key command
/// that hangs, or a model's load, would freeze the screen or the window.
fn recheck<T: From<Input> + Send + 'static>(
    store: &mut Store,
    tx: &Sender<T>,
    settings: Option<&Path>,
    run: &str,
    files: &[PathBuf],
) -> Result<Job, String> {
    // Read each time, so an edit to the file reaches the next recheck.
    let settings = match settings {
        Some(path) => Settings::read(path)?,
        None => Settings::load()?,
    };
    let scan = store
        .scan_named(run)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no run named {run}"))?;
    let opts = recheck_options(&scan, run, &settings)?;
    let planned = scan::plan(files, &opts, store, &mut io::sink()).map_err(|e| format!("{e:#}"))?;
    let scan::Planned::Scan(plan) = planned else {
        return Err("nothing to check".into());
    };
    let scans = store.reopen().map_err(|e| e.to_string())?;
    jobs::recheck(plan, opts, scans, tx.clone()).map_err(|e| e.to_string())?;
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
    use std::time::{Duration, Instant};

    use crate::apply;
    use crate::llm;
    use crate::test_support::{
        FakeServer, TYPOS_LINES, closed_port_url, copied, fake_llm, fitting_lines, prompt_lines,
        scan_row,
    };

    #[test]
    fn a_recheck_takes_the_entrys_jobs_and_chunk_lines_as_the_settings_have_them_now() {
        let s = Settings::parse(
            Path::new("/c/config.toml"),
            "[llm.mine]\ncommand = \"x\"\njobs = 4\nchunk_lines = 150\n",
        )
        .unwrap();
        let opts = recheck_options(&scan_row("mine", &["eng"]), "r", &s).unwrap();
        assert_eq!(
            (
                opts.backend.label.as_str(),
                opts.jobs,
                opts.chunk_lines,
                opts.lines_per_chunk(),
                opts.force,
                opts.into.as_deref()
            ),
            ("mine", 4, None, 150, true, Some("r"))
        );
        let opts = recheck_options(&scan_row("cmd:my-llm", &["eng"]), "r", &s).unwrap();
        assert_eq!(
            (opts.jobs, opts.lines_per_chunk()),
            (llm::JOBS, llm::CHUNK_LINES)
        );
        let none = Settings::parse(Path::new("/c/config.toml"), "").unwrap();
        assert_eq!(
            recheck_options(&scan_row("mine", &["eng"]), "r", &none).err(),
            Some("run r was scanned with mine, which this subcheck does not know and /c/config.toml does not name".into())
        );
    }

    /// A store in a temp dir holding a run labelled `label`, begun on a copy of typos.mkv, and
    /// `settings` in config.toml beside it; a runner whose rechecks read that file, the channel its
    /// jobs send to, and the task that checks the copy again into the run.
    fn rechecking(
        label: &str,
        settings: &str,
    ) -> (
        tempfile::TempDir,
        Runner<Input>,
        mpsc::Receiver<Input>,
        Task,
    ) {
        let (dir, file) = copied("typos.mkv");
        let mut store = Store::open_at(&dir.path().join("subcheck.db")).unwrap();
        let header = scan_row(label, &["eng"]).header;
        let run = store
            .begin_scan(&header, std::slice::from_ref(&file))
            .unwrap()
            .name;
        let config = dir.path().join("config.toml");
        std::fs::write(&config, settings).unwrap();
        let (tx, rx) = mpsc::channel::<Input>();
        let runner = Runner::new(store, dir.path().join("state"), tx).with_settings_at(config);
        let task = Task::Recheck {
            run,
            paths: vec![file.to_string_lossy().into_owned()],
        };
        (dir, runner, rx, task)
    }

    // The settings file the runner was given, not this process's: one without the run's entry
    // refuses the recheck, naming that file, and one with it runs the entry's command.
    #[test]
    fn a_recheck_reads_the_settings_file_the_runner_was_given() {
        let entry = format!("[llm.mine]\ncommand = \"'{}'\"\n", fake_llm().display());
        let (dir, runner, rx, task) = rechecking("mine", &entry);
        let Task::Recheck { run, .. } = task.clone() else {
            unreachable!()
        };
        let started = |input: Input| match input {
            Input::Started { job, .. } => job.map(|j| j.run),
            other => panic!("{other:?}"),
        };
        let none = dir.path().join("none.toml");
        let mut runner = runner.with_settings_at(none.clone());
        assert_eq!(
            started(runner.run(task.clone())),
            Err(format!(
                "run {run} was scanned with mine, which this subcheck does not know and {} does \
                 not name",
                none.display()
            ))
        );
        let mut runner = runner.with_settings_at(dir.path().join("config.toml"));
        assert_eq!(started(runner.run(task)), Ok(run));
        let ended = loop {
            if let Input::ScanEnded(ended) = rx.recv_timeout(Duration::from_mins(1)).unwrap() {
                break ended.unwrap();
            }
        };
        // The fake LLM, the entry's command, finds the fixture's two typos.
        assert_eq!((ended.checked, ended.failed, ended.findings), (1, 0, 2));
    }

    #[test]
    fn a_recheck_takes_a_server_entrys_jobs_and_chunk_lines_and_the_runs_model() {
        let s = Settings::parse(
            Path::new("/c/config.toml"),
            "[llm.srv]\nurl = \"http://127.0.0.1:1/v1\"\njobs = 5\nchunk_lines = 120\n",
        )
        .unwrap();
        let opts = recheck_options(&scan_row("srv@m", &["eng"]), "r", &s).unwrap();
        assert_eq!(
            (
                opts.backend.label.as_str(),
                opts.jobs,
                opts.lines_per_chunk()
            ),
            ("srv@m", 5, 120)
        );
    }

    // The runner is the screen's loop or the window's core thread: it answers at once, however long
    // the server takes to say what it serves.
    #[test]
    fn a_recheck_asks_the_server_on_its_own_thread() {
        let server = FakeServer::llm_slow(&["m"], Duration::from_secs(3));
        let settings = format!("[llm.srv]\nurl = \"{}\"\nchunk_lines = 1\n", server.url);
        let (_dir, mut runner, rx, recheck_task) = rechecking("srv@m", &settings);
        let started = Instant::now();
        let Input::Started { job, .. } = runner.run(recheck_task) else {
            panic!()
        };
        assert!(
            job.is_ok() && started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        let summary = loop {
            if let Input::ScanEnded(ended) = rx.recv_timeout(Duration::from_secs(60)).unwrap() {
                break ended.unwrap();
            }
        };
        assert_eq!(summary.findings, 2);
        assert!(
            server.requests().iter().any(|r| r.path == "/v1/models"),
            "the start-up check ran"
        );
        assert!(
            server.completions().len() > 2,
            "chunk_lines = 1: a call a line"
        );
    }

    /// The inputs a job sends to `rx`, up to and with `Input::ScanEnded`.
    fn until_ended(rx: &mpsc::Receiver<Input>) -> Vec<Input> {
        let mut inputs = Vec::new();
        loop {
            let input = rx.recv_timeout(Duration::from_mins(1)).unwrap();
            let ended = matches!(input, Input::ScanEnded(_));
            inputs.push(input);
            if ended {
                return inputs;
            }
        }
    }

    #[test]
    fn a_recheck_fits_its_chunks_to_the_servers_context() {
        // 1024 tokens shared by the default 2 jobs.
        let per_call = fitting_lines(1024 / 2);
        assert!(per_call < TYPOS_LINES);
        let server = FakeServer::llm_sized(&["m"], 1024);
        let settings = format!("[llm.srv]\nurl = \"{}\"\n", server.url);
        let (_dir, mut runner, rx, recheck_task) = rechecking("srv@m", &settings);
        let Input::Started { job, .. } = runner.run(recheck_task) else {
            panic!()
        };
        assert!(job.is_ok(), "{job:?}");
        let inputs = until_ended(&rx);
        let Some(Input::ScanEnded(Ok(summary))) = inputs.last() else {
            panic!("{inputs:?}")
        };
        assert_eq!(summary.findings, 2);
        let sent: Vec<usize> = server.completions().iter().map(prompt_lines).collect();
        assert!(
            sent.len() > 1 && sent.iter().all(|&n| n <= per_call),
            "{sent:?}"
        );
    }

    #[test]
    fn a_recheck_cuts_the_entrys_chunk_lines_to_what_fits_the_servers_context() {
        // 1400 tokens shared by the default 2 jobs: 700 a call, which holds fewer lines than the
        // entry's chunk_lines.
        let fits = fitting_lines(1400 / 2);
        assert!(fits < TYPOS_LINES);
        let server = FakeServer::llm_sized(&["m"], 1400);
        let settings = format!(
            "[llm.srv]\nurl = \"{}\"\nchunk_lines = {TYPOS_LINES}\n",
            server.url
        );
        let (_dir, mut runner, rx, recheck_task) = rechecking("srv@m", &settings);
        let Input::Started { job, .. } = runner.run(recheck_task) else {
            panic!()
        };
        assert!(job.is_ok(), "{job:?}");
        let inputs = until_ended(&rx);
        let Some(Input::ScanEnded(Ok(summary))) = inputs.last() else {
            panic!("{inputs:?}")
        };
        assert_eq!(summary.findings, 2);
        let mut sent: Vec<usize> = server.completions().iter().map(prompt_lines).collect();
        sent.sort_unstable();
        assert_eq!(sent, [TYPOS_LINES - fits, fits]);
    }

    #[test]
    fn a_recheck_through_a_server_that_does_not_say_its_size_warns_before_it_scans() {
        let server = FakeServer::llm(&["m"]);
        recheck_warns(
            &format!("[llm.srv]\nurl = \"{}\"\n", server.url),
            "warning: srv did not say its context size; set chunk_lines if it holds under 38k \
             tokens",
        );
    }

    #[test]
    fn a_recheck_through_an_entry_whose_cap_leaves_no_room_for_a_line_warns_before_it_scans() {
        // 16384 tokens shared by the default 2 jobs: 8192 a call, all of it the reply's.
        let server = FakeServer::llm_sized(&["m"], 16384);
        recheck_warns(
            &format!(
                "[llm.srv]\nurl = \"{}\"\n[llm.srv.request]\nmax_tokens = 8192\n",
                server.url
            ),
            "warning: srv's request.max_tokens (8192) leaves no room for lines in a call's 8192 \
             tokens, so each line is sent alone; lower it, or set shared_context = false or fewer \
             jobs",
        );
    }

    /// A recheck of typos.mkv's run through `srv@m` with `settings` sends `warning` once, as an
    /// `Input::Warned` before the scan's first step, and ends well.
    fn recheck_warns(settings: &str, warning: &str) {
        let (_dir, mut runner, rx, recheck_task) = rechecking("srv@m", settings);
        let Input::Started { job, .. } = runner.run(recheck_task) else {
            panic!()
        };
        assert!(job.is_ok(), "{job:?}");
        let inputs = until_ended(&rx);
        let warned = Input::Warned(warning.into());
        let at = |wanted: &dyn Fn(&Input) -> bool| inputs.iter().position(wanted);
        let first_warning = at(&|i| matches!(i, Input::Warned(_)));
        let first_step = at(&|i| matches!(i, Input::Scan(_)));
        assert!(
            matches!((first_warning, first_step), (Some(w), Some(s)) if w < s),
            "{inputs:?}"
        );
        let warnings: Vec<&Input> = inputs
            .iter()
            .filter(|i| matches!(i, Input::Warned(_)))
            .collect();
        assert_eq!(warnings, [&warned]);
        assert!(
            matches!(inputs.last(), Some(Input::ScanEnded(Ok(_)))),
            "{inputs:?}"
        );
    }

    #[test]
    fn a_recheck_that_finds_no_server_ends_as_a_failed_scan() {
        let closed = closed_port_url();
        let settings = format!("[llm.srv]\nurl = \"{closed}\"\n");
        let (_dir, mut runner, rx, recheck_task) = rechecking("srv@m", &settings);
        let Input::Started { job, .. } = runner.run(recheck_task) else {
            panic!()
        };
        assert!(job.is_ok());
        // The first input is the end: nothing was queued or checked before the check failed.
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(30)).unwrap(),
            Input::ScanEnded(Err(format!(
                "srv@m: nothing answering at {closed}: is the server running?"
            )))
        );
    }

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
