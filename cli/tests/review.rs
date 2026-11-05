//! The review's decisions through a real store: written by `tui::run_effect`, read back as the
//! run's view and by `list`. The screen itself is not drawn here; `tui::view` has its own tests.

use std::collections::{HashMap, VecDeque};

use crossterm::event::KeyCode;
use subcheck_cli::mpv::Player;
use subcheck_cli::tui::run_effect;
use subcheck_cli::tui::screen::{self, Msg, Screen};
use subcheck_core::list::{self, Filter};
use subcheck_core::review::model::Review;
use subcheck_core::review::runner::Runner;
use subcheck_core::store::{
    Attempt, CheckedWith, Finding, Made, RunHeader, ScanRow, State, Store, TrackRecord,
};
use subcheck_core::text::Occurrence;

const FILE: &str = "/m/K-On!/K-On! - E01 - Episode.mkv";
const OTHER: &str = "/m/K-On!/K-On! - E02 - Episode.mkv";

fn finding(file: &str, original: &str, suggestion: &str) -> Finding {
    Finding {
        id: 0,
        file: file.into(),
        style: "Dialogue".into(),
        original: original.into(),
        suggestion: suggestion.into(),
        reason: "typo".into(),
        source: "llm".into(),
        occurrences: vec![Occurrence {
            track: 2,
            start_ms: 10_000,
            end_ms: 12_000,
        }],
        before: vec![],
        after: vec![],
        timing: None,
    }
}

/// An LLM check of `path` by `scan`, as the scan saves it, with `findings`.
fn checked(store: &mut Store, scan: &ScanRow, path: &str, findings: Vec<Finding>) {
    let tracks = [TrackRecord {
        id: 2,
        name: Some("English".into()),
        language: "eng".into(),
        language_bcp47: None,
        sid: 1,
        default: true,
        read: "cues".into(),
    }];
    let attempt = Attempt {
        scan: scan.id,
        path,
        size: 1,
        mtime: jiff::Timestamp::UNIX_EPOCH,
        tracks: &tracks,
        truncated: None,
        at: jiff::Timestamp::UNIX_EPOCH,
    };
    let with = CheckedWith::new("claude", &["eng".into()], 1);
    store
        .save(
            &attempt,
            vec![Made {
                with,
                lines: 10,
                findings,
            }],
        )
        .unwrap();
}

/// The event loop's inner part: a message, then every effect it leads to, run for real.
fn send(m: &mut Review, s: &mut Screen, msg: Msg, runner: &mut Runner<Msg>, player: &mut Player) {
    let now = jiff::Timestamp::now();
    let mut queue: VecDeque<_> = screen::update(m, s, msg, now).into();
    while let Some(effect) = queue.pop_front() {
        let input = run_effect(effect, runner, player);
        queue.extend(screen::update(m, s, input.into(), now));
    }
}

/// A runner writing through a connection of its own to `store`'s database, which each test reads
/// back through `store`.
fn runner_on(store: &Store) -> Runner<Msg> {
    // No test here starts a scan, so the runner's jobs send nothing and the receiver can go.
    let (tx, _) = std::sync::mpsc::channel();
    // Where apply would write its tracks: no test here starts apply, so nothing goes there.
    Runner::new(store.reopen().unwrap(), "/no-state".into(), tx)
}

#[test]
fn decisions_made_in_review_are_in_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open_at(&dir.path().join("subcheck.db")).unwrap();
    let header = RunHeader {
        started: jiff::Timestamp::UNIX_EPOCH,
        backend: "claude".into(),
        command: vec![],
        prompt_version: 1,
        version: "0".into(),
        langs: vec!["eng".into()],
    };
    let scan = store
        .begin_scan(&header, &[FILE.into(), OTHER.into()])
        .unwrap();
    checked(
        &mut store,
        &scan,
        FILE,
        vec![finding(FILE, "gets trough", "gets through")],
    );
    checked(
        &mut store,
        &scan,
        OTHER,
        vec![
            finding(OTHER, "gets trough", "gets through"),
            finding(OTHER, "befor it", "before it"),
        ],
    );
    let mut model = Review::new(store.run_view(&scan).unwrap(), HashMap::new());
    let mut screen = Screen::default();
    let mut player = Player::new(dir.path().join("no-mpv"), dir.path().join("mpv.sock"));
    let mut runner = runner_on(&store);
    let key = |c| Msg::Key(KeyCode::Char(c));

    // Accept the group of two, then reword the other finding.
    send(&mut model, &mut screen, key('a'), &mut runner, &mut player);
    send(&mut model, &mut screen, key('e'), &mut runner, &mut player);
    send(
        &mut model,
        &mut screen,
        Msg::KillLine,
        &mut runner,
        &mut player,
    );
    for c in "Before it.".chars() {
        send(&mut model, &mut screen, key(c), &mut runner, &mut player);
    }
    send(
        &mut model,
        &mut screen,
        Msg::Key(KeyCode::Enter),
        &mut runner,
        &mut player,
    );

    let run = store.run_view(&scan).unwrap();
    assert_eq!([1, 2, 3].map(|id| run.state_of(id)), [State::Accepted; 3]);
    assert_eq!(run.decisions[&3].edited.as_deref(), Some("Before it."));
    let text = list::render(&run, Filter::Only(State::Accepted), false);
    assert!(
        text.contains("      - befor it\n      + Before it.\n"),
        "{text}"
    );

    // Undo is a write too, and the newest decision wins on reading back.
    send(&mut model, &mut screen, key('u'), &mut runner, &mut player);
    let run = store.run_view(&scan).unwrap();
    assert_eq!(run.state_of(3), State::Pending);
    assert_eq!(run.state_of(1), State::Accepted);

    // With no mpv to start, `o` says so in the status line.
    model.filter = Filter::All;
    send(&mut model, &mut screen, key('o'), &mut runner, &mut player);
    assert!(
        model
            .status
            .as_deref()
            .is_some_and(|s| s.contains("not found")),
        "{:?}",
        model.status
    );
}

#[test]
fn a_decision_on_a_finding_checked_again_follows_it_through_the_loop() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open_at(&dir.path().join("subcheck.db")).unwrap();
    let header = RunHeader {
        started: jiff::Timestamp::UNIX_EPOCH,
        backend: "claude".into(),
        command: vec![],
        prompt_version: 1,
        version: "0".into(),
        langs: vec!["eng".into()],
    };
    let scan = store.begin_scan(&header, &[FILE.into()]).unwrap();
    let typo = || vec![finding(FILE, "gets trough", "gets through")];
    checked(&mut store, &scan, FILE, typo());
    let mut model = Review::new(store.run_view(&scan).unwrap(), HashMap::new());
    let mut screen = Screen::default();
    let old = model.run.findings[0].id;
    let mut player = Player::new(dir.path().join("no-mpv"), dir.path().join("mpv.sock"));
    let mut runner = runner_on(&store);
    let key = |c| Msg::Key(KeyCode::Char(c));

    // A second check of the file replaces the first, with the same finding under a new ID: the
    // model's finding is stale.
    checked(&mut store, &scan, FILE, typo());
    let run = store.run_view(&scan).unwrap();
    let new = run.findings[0].id;
    assert_ne!(old, new);

    send(&mut model, &mut screen, key('a'), &mut runner, &mut player);
    let run = store.run_view(&scan).unwrap();
    assert_eq!(run.state_of(new), State::Accepted);
    assert_eq!(run.state_of(old), State::Pending);
    assert!(!run.decisions.contains_key(&old));
    let ids: Vec<u64> = model.run.findings.iter().map(|f| f.id).collect();
    assert_eq!(ids, [new]);
    assert_eq!(model.run.state_of(new), State::Accepted);
    assert_eq!(
        model.status.as_deref(),
        Some("E01 was checked again; your decision went to its new finding")
    );

    send(&mut model, &mut screen, key('u'), &mut runner, &mut player);
    let run = store.run_view(&scan).unwrap();
    assert_eq!(run.state_of(new), State::Pending);
}

#[test]
fn v_switches_between_a_run_and_its_folders_through_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open_at(&dir.path().join("subcheck.db")).unwrap();
    let header = |started: &str| RunHeader {
        started: started.parse().unwrap(),
        backend: "claude".into(),
        command: vec![],
        prompt_version: 1,
        version: "0".into(),
        langs: vec!["eng".into()],
    };
    // A season's scan of both files, then a forced one of FILE alone.
    let season = store
        .begin_scan(
            &header("2026-10-17T10:00:00Z"),
            &[FILE.into(), OTHER.into()],
        )
        .unwrap();
    checked(
        &mut store,
        &season,
        FILE,
        vec![finding(FILE, "gets trough", "gets through")],
    );
    checked(
        &mut store,
        &season,
        OTHER,
        vec![finding(OTHER, "befor it", "before it")],
    );
    let forced = store
        .begin_scan(&header("2026-10-17T11:00:00Z"), &[FILE.into()])
        .unwrap();
    checked(
        &mut store,
        &forced,
        FILE,
        vec![finding(FILE, "gets trough", "gets through")],
    );
    let mut model = Review::new(store.run_view(&forced).unwrap(), HashMap::new());
    let mut screen = Screen::default();
    let mut player = Player::new(dir.path().join("no-mpv"), dir.path().join("mpv.sock"));
    let mut runner = runner_on(&store);
    let key = |c| Msg::Key(KeyCode::Char(c));

    // The forced run shows FILE alone; `v` opens its folder, OTHER's finding with it, and the
    // spinner, a tick along, starts over there.
    assert_eq!(model.run.files.len(), 1);
    send(&mut model, &mut screen, Msg::Tick, &mut runner, &mut player);
    send(&mut model, &mut screen, key('v'), &mut runner, &mut player);
    assert!(model.run.of_paths, "{:?}", model.status);
    assert_eq!(model.run.shown_name(), "K-On!");
    let files: Vec<&str> = model.run.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!((files, screen.ticks), (vec![FILE, OTHER], 0));
    // On OTHER's finding, `v` opens the newest run that named OTHER: the season's.
    model.selected = model
        .entries()
        .iter()
        .position(|&e| model.run.findings[model.rows[e.row()].members[0]].file == OTHER)
        .unwrap();
    send(&mut model, &mut screen, key('v'), &mut runner, &mut player);
    assert!(!model.run.of_paths, "{:?}", model.status);
    assert_eq!(model.run.name, season.name);
}
