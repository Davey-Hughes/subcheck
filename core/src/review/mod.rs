//! Review: accept, reject or reword each finding of a run, and open its moment in a player. A pure
//! Elm-style core (`model`, `update`) takes a UI's actions; the store and the jobs are at its edge
//! (`runner`, `jobs`). Each UI maps its own keys and clicks to those actions, and runs the player
//! commands `update` gives it on a player of its own; the terminal's UI is `subcheck_cli::tui`.

pub mod jobs;
pub mod model;
pub mod progress;
pub mod runner;
pub mod update;

use crate::scan;
use crate::store::{Run, Selector, Store};

/// What the screen opens on when a scan has nothing to check (`s`): with `--run` (`into`), the run;
/// else the files under the paths it was given, as they are. `None` when there is nothing to show,
/// and `scan`'s line says why.
pub fn nothing_to_check(
    store: &Store,
    s: &scan::Summary,
    into: Option<&str>,
) -> anyhow::Result<Option<Run>> {
    if s.files == 0 {
        return Ok(None);
    }
    let selector = match into {
        Some(name) => Selector::Run(name.to_string()),
        None => Selector::Paths(s.paths.clone()),
    };
    let run = store.view(&selector)?;
    Ok((!run.findings.is_empty()).then_some(run))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scan_with_nothing_to_check_shows_its_paths_or_its_run() {
        use crate::store::{Attempt, CheckedWith, Made, RunHeader};
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_at(&dir.path().join("subcheck.db")).unwrap();
        let show = dir.path().join("Show");
        std::fs::create_dir(&show).unwrap();
        let e01 = show.join("E01.mkv");
        let header = RunHeader {
            started: jiff::Timestamp::UNIX_EPOCH,
            backend: "claude".into(),
            command: vec![],
            prompt_version: 1,
            version: "0".into(),
            langs: vec!["eng".into()],
        };
        let scan = store
            .begin_scan(&header, std::slice::from_ref(&e01))
            .unwrap();
        let path = e01.to_string_lossy();
        let attempt = Attempt {
            scan: scan.id,
            path: &path,
            size: 1,
            mtime: jiff::Timestamp::UNIX_EPOCH,
            tracks: &[],
            truncated: None,
            at: jiff::Timestamp::UNIX_EPOCH,
        };
        let save = |store: &mut Store, findings| {
            let with = CheckedWith::new("claude", &["eng".into()], 1);
            let made = Made {
                with,
                lines: 1,
                findings,
            };
            store.save(&attempt, vec![made]).unwrap();
        };
        let finding = crate::store::Finding {
            file: path.to_string(),
            ..model::tests_support::finding(0, 1, "a", "b", 0)
        };
        save(&mut store, vec![finding]);
        let s = scan::Summary {
            files: 1,
            cached: 1,
            paths: vec![show.clone()],
            ..scan::Summary::default()
        };
        let shown = nothing_to_check(&store, &s, None).unwrap().unwrap();
        assert_eq!(
            shown.called(),
            crate::list::shell_quote(&show.to_string_lossy())
        );
        assert_eq!(shown.findings.len(), 1);
        let run = nothing_to_check(&store, &s, Some(&scan.name))
            .unwrap()
            .unwrap();
        assert_eq!(run.called(), format!("run {}", scan.name));
        // No MKV files, or no findings in them: the line says so, not the screen.
        let none = scan::Summary {
            files: 0,
            ..s.clone()
        };
        assert!(nothing_to_check(&store, &none, None).unwrap().is_none());
        save(&mut store, vec![]);
        assert!(nothing_to_check(&store, &s, None).unwrap().is_none());
    }
}
