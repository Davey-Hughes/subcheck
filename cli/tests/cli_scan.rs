//! `subcheck scan` and `subcheck list` end to end, with core/tests/fake-llm.sh as the LLM, a
//! temporary directory as XDG_STATE_HOME, a database in it as SUBCHECK_DB, and a settings file in
//! it, absent until a test writes one, as SUBCHECK_CONFIG.

use subcheck_core::test_support as common;

use std::cell::Cell;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::{fitting_lines, fixture};
use subcheck_core::list::shell_quote;
use subcheck_core::llm::{Usage, estimate};
use subcheck_core::store::{self, Check, LlmCall, Selector, State, Store};

struct Env {
    state: tempfile::TempDir,
    log: PathBuf,
    /// The copies [`Env::typos`] has made.
    copies: Cell<usize>,
}

impl Env {
    fn new() -> Env {
        let state = tempfile::tempdir().unwrap();
        let log = state.path().join("llm-calls.log");
        Env {
            state,
            log,
            copies: Cell::new(0),
        }
    }

    /// The binary with `args`, this test's state directory, database, call log and settings file,
    /// then `extra_env`, which can change any of them.
    fn run(&self, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_subcheck"));
        cmd.args(args)
            .env("XDG_STATE_HOME", self.state.path())
            .env("SUBCHECK_DB", self.db())
            .env("FAKE_LLM_LOG", &self.log)
            .env("SUBCHECK_CONFIG", self.config());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    /// `subcheck`, with the fake LLM as `scan`'s `--llm-cmd`.
    fn subcheck(&self, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
        let fake = format!("'{}'", fake_llm());
        let mut args = args.to_vec();
        if args.first() == Some(&"scan") {
            args.extend(["--llm-cmd", &fake]);
        }
        self.run(&args, extra_env)
    }

    /// `subcheck scan --llm <llm>`, as [`Env::scan_plain`] runs it.
    fn scan_preset(&self, llm: &str, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
        let mut args = args.to_vec();
        args.extend(["--llm", llm]);
        self.scan_plain(&args, extra_env)
    }

    /// `subcheck scan` with neither `--llm` nor `--llm-cmd`, and the fake LLM first on the PATH
    /// by the names the presets run ([`Env::fake_bin`]), so a scan that falls back to claude never
    /// runs the real one.
    fn scan_plain(&self, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
        let path = format!(
            "{}:{}",
            self.fake_bin().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut all = vec!["scan"];
        all.extend_from_slice(args);
        let mut env = vec![("PATH", path.as_str())];
        env.extend_from_slice(extra_env);
        self.run(&all, &env)
    }

    /// A directory of links to the fake LLM named `claude`, `codex` and `openai`.
    fn fake_bin(&self) -> PathBuf {
        let fake = common::fake_llm();
        let bin = self.state.path().join("bin");
        if !bin.exists() {
            std::fs::create_dir(&bin).unwrap();
            for name in ["claude", "codex", "openai"] {
                std::os::unix::fs::symlink(&fake, bin.join(name)).unwrap();
            }
        }
        bin
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    /// SUBCHECK_DB: not where the state directory would put it, so a command not given it is seen
    /// to miss it.
    fn db(&self) -> PathBuf {
        self.state.path().join("subcheck.db")
    }

    /// SUBCHECK_CONFIG, written only by [`Env::settings`]: no test reads the developer's own file.
    fn config(&self) -> PathBuf {
        self.state.path().join("config.toml")
    }

    /// Write `text` as the settings file.
    fn settings(&self, text: &str) {
        std::fs::write(self.config(), text).unwrap();
    }

    /// A copy of typos.mkv, `<state>/show/E<n>.mkv`, by a name no earlier call gave: a file no
    /// scan has checked yet.
    fn typos(&self) -> String {
        let n = self.copies.get() + 1;
        self.copies.set(n);
        let show = self.state.path().join("show");
        std::fs::create_dir_all(&show).unwrap();
        let to = show.join(format!("E{n}.mkv"));
        std::fs::copy(fixture("typos.mkv"), &to).unwrap();
        to.to_string_lossy().into_owned()
    }

    fn store(&self) -> Store {
        Store::open_at(&self.db()).unwrap()
    }

    fn newest_run(&self) -> store::Run {
        self.store().view(&Selector::Newest).unwrap()
    }

    /// The newest run's LLM calls, as recorded.
    fn recorded_calls(&self) -> Vec<LlmCall> {
        let store = self.store();
        let scan = store.newest_scan().unwrap().unwrap();
        store.usage(scan.id).unwrap()
    }

    /// Change the database as a test needs.
    fn sql(&self, sql: &str) {
        rusqlite::Connection::open(self.db())
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }

    /// How many runs the database has.
    fn runs(&self) -> i64 {
        rusqlite::Connection::open(self.db())
            .unwrap()
            .query_row("SELECT COUNT(*) FROM scans", [], |r| r.get(0))
            .unwrap()
    }
}

fn path(name: &str) -> String {
    fixture(name).to_string_lossy().into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The newest run, as the store has it.
fn newest(env: &Env) -> store::ScanRow {
    env.store().newest_scan().unwrap().unwrap()
}

#[test]
fn scan_records_the_planted_typos() {
    let env = Env::new();
    let out = env.subcheck(&["scan", &path("typos.mkv")], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "1 files, 1 checked, 0 failed, 0 unchanged since an earlier scan; 2 findings"
        ),
        "{}",
        stderr(&out)
    );

    let run = env.newest_run();
    assert_eq!(
        run.header.as_ref().unwrap().prompt_version,
        subcheck_core::llm::PROMPT_VERSION
    );
    let pairs: Vec<(&str, &str)> = run
        .findings
        .iter()
        .map(|f| (f.original.as_str(), f.suggestion.as_str()))
        .collect();
    assert_eq!(
        pairs,
        [
            (
                "We should leave befor it rains.",
                "We should leave before it rains."
            ),
            ("go trough it", "go through it"),
        ]
    );
    let karaoke = &run.findings[1];
    assert_eq!(karaoke.style, "Song");
    assert_eq!(
        karaoke.before,
        ["We should leave befor it rains.", "Nothing wrong here."]
    );
    assert_eq!(karaoke.after, ["The last line."]);
    assert!(
        run.findings
            .iter()
            .all(|f| run.state_of(f.id) == State::Pending)
    );

    let file = &run.files[0];
    assert_eq!((file.error.as_deref(), file.lines), (None, 4));
    assert_eq!(file.tracks[0].sid, 1);
    assert_eq!(file.tracks[0].read, "cues");
}

#[test]
fn the_claude_and_gpt_presets_read_their_replies_from_json() {
    for llm in ["claude", "gpt"] {
        let env = Env::new();
        let out = env.scan_preset(llm, &[&path("typos.mkv")], &[]);
        assert!(out.status.success(), "{llm}: {}", stderr(&out));
        let run = env.newest_run();
        let pairs: Vec<(&str, &str, &str)> = run
            .findings
            .iter()
            .map(|f| {
                (
                    f.original.as_str(),
                    f.suggestion.as_str(),
                    f.reason.as_str(),
                )
            })
            .collect();
        assert_eq!(
            pairs,
            [
                (
                    "We should leave befor it rains.",
                    "We should leave before it rains.",
                    "planted typo"
                ),
                ("go trough it", "go through it", "planted typo"),
            ],
            "{llm}"
        );
        assert_eq!(run.header.unwrap().backend, llm);
    }
}

#[test]
fn the_fake_claude_escapes_every_control_character_in_its_json() {
    use std::io::Write as _;
    let env = Env::new();
    let mut child = Command::new(env.fake_bin().join("claude"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // A CRLF subtitle line keeps its CR; a stray control character rides along. ESC, not \x01:
    // that is bash's own quoting byte, which bash 3.2 mishandles in `read` and `${s:i:1}`.
    let prompt = "L1 [0:00:01.00] (D) leave befor it\r now\u{1b}\n";
    child
        .stdin
        .take()
        .unwrap()
        .write_all(prompt.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let reply: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    assert_eq!(
        reply["result"],
        "L1\tleave before it\r now\u{1b}\tplanted typo"
    );
}

/// An LLM call of typos.mkv's four lines, as the store records it.
fn call(usage: Option<Usage>) -> LlmCall {
    LlmCall {
        file: path("typos.mkv"),
        lines: 4,
        usage,
    }
}

#[test]
fn each_llm_call_is_recorded_with_what_it_used_and_the_scan_says_what_they_cost() {
    let env = Env::new();
    let typos = path("typos.mkv");
    let out = env.scan_preset("claude", &[&typos], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    // The summary, then what the calls cost; the fake claude's price, 1.25 cents a call.
    assert!(
        stderr(&out).contains(
            "; 2 findings\nLLM: 1 call, 1.20k in / 40 out tokens, $0.013 at API prices\n"
        ),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        env.recorded_calls(),
        [call(Some(Usage {
            input_tokens: 1203,
            output_tokens: 40,
            cache_read_tokens: Some(200),
            cache_write_tokens: Some(1000),
            cost_usd: Some(0.0125),
        }))]
    );
    assert_eq!(env.newest_run().spent.calls, 1);
    let list = env.subcheck(&["list"], &[]);
    let text = String::from_utf8_lossy(&list.stdout);
    assert_eq!(
        text.lines().nth(1),
        Some("LLM: 1 call, 1.20k in / 40 out tokens, $0.013 at API prices"),
        "{text}"
    );

    // codex reports tokens and no price: none is shown.
    let env = Env::new();
    let out = env.scan_preset("gpt", &[&typos], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("\nLLM: 1 call, 1.50k in / 40 out tokens\n"),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        env.recorded_calls(),
        [call(Some(Usage {
            input_tokens: 1500,
            output_tokens: 40,
            cache_read_tokens: Some(1000),
            ..Usage::default()
        }))]
    );

    // A command LLM reports nothing, and nothing is said.
    let env = Env::new();
    let out = env.subcheck(&["scan", &typos], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stderr(&out).contains("LLM:"), "{}", stderr(&out));
    assert_eq!(env.recorded_calls(), []);
}

#[test]
fn a_failed_call_is_recorded_too_and_so_is_a_call_adding_to_a_run() {
    let env = Env::new();
    let typos = path("typos.mkv");
    // A reply the parser cannot read, twice: both calls cost.
    let out = env.scan_preset(
        "claude",
        &[&typos],
        &[("FAKE_LLM_REPLY", "These lines look fine to me.")],
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("\nLLM: 2 calls, 2.41k in / 80 out tokens, $0.025 at API prices\n"),
        "{}",
        stderr(&out)
    );
    let run = env.newest_run();
    assert!(run.files[0].error.is_some());
    assert_eq!(run.spent.calls, 2);

    // Adding the failed file to the run again: the run's calls are both scans'.
    let out = env.scan_preset("claude", &[&typos, "--run", &run.name], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "; 2 findings\nLLM: 1 call, 1.20k in / 40 out tokens, $0.013 at API prices\n"
        ),
        "{}",
        stderr(&out)
    );
    assert_eq!(env.newest_run().spent.calls, 3);
}

#[test]
fn a_call_that_says_nothing_of_what_it_used_is_counted_and_said_so() {
    let typos = path("typos.mkv");
    // The fake claude replies in plain text, not its JSON; or fails with nothing on stdout. Twice
    // either way (a call and its retry): two calls, each counted with no numbers.
    for (fake, failed) in [
        ("FAKE_LLM_PLAIN", "FAILED: LLM: reply is not claude's JSON"),
        (
            "FAKE_LLM_FAIL",
            "FAILED: LLM: claude exited with exit status: 3: fake-llm: told to fail\n",
        ),
    ] {
        let env = Env::new();
        let out = env.scan_preset("claude", &[&typos], &[(fake, "1")]);
        assert!(!out.status.success());
        let err = stderr(&out);
        assert!(err.contains(failed), "{fake}: {err}");
        assert!(
            err.contains("\nLLM: 2 calls (none reported usage)\n"),
            "{fake}: {err}"
        );
        assert_eq!(env.recorded_calls(), [call(None), call(None)], "{fake}");
    }

    // A command LLM that fails is not counted: it never reports.
    let env = Env::new();
    let out = env.subcheck(&["scan", &typos], &[("FAKE_LLM_FAIL", "1")]);
    assert!(
        stderr(&out).contains("FAILED: LLM: sh exited"),
        "{}",
        stderr(&out)
    );
    assert!(!stderr(&out).contains("\nLLM:"), "{}", stderr(&out));
}

#[test]
fn scan_records_timing_findings_beside_the_llms() {
    let env = Env::new();
    let out = env.subcheck(&["scan", &path("timing.mkv")], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("unchanged since an earlier scan; 2 findings (2 timing)"),
        "{}",
        stderr(&out)
    );
    let run = env.newest_run();
    let found: Vec<(usize, &str, &str, &str)> = run
        .findings
        .iter()
        .map(|f| {
            (
                f.timing.as_ref().unwrap().track,
                f.original.as_str(),
                f.suggestion.as_str(),
                f.reason.as_str(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                2,
                "1 frame late",
                "shift 1 frame earlier",
                "cuts: 85 times, +1 in 2 of 2 windows"
            ),
            (
                4,
                "1 frame late",
                "shift 1 frame earlier",
                "shares 34 times with track 2, all at the same frame"
            ),
        ]
    );
    assert!(run.findings.iter().all(|f| f.source == "timing"));
    assert_eq!(run.files[0].checks, [Check::Llm, Check::Timing]);
    assert_eq!(run.header.unwrap().langs, ["eng"]);
}

/// Every timing check as rules of a later version leave it: no longer fresh.
const TIMING_STALE: &str = "UPDATE checks SET version = 0 WHERE kind = 'timing'";

#[test]
fn a_file_whose_timing_check_is_stale_gets_that_check_alone() {
    let env = Env::new();
    assert!(
        env.subcheck(&["scan", &path("timing.mkv")], &[])
            .status
            .success()
    );
    env.sql(TIMING_STALE);
    let calls = env.calls().len();

    let out = env.subcheck(&["scan", &path("timing.mkv")], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("timing.mkv: timing only, 2 findings"),
        "{}",
        stderr(&out)
    );
    assert_eq!(env.calls().len(), calls, "the LLM check stands");
    // The new run shows the file as it is: its LLM check from the first scan, its timing check now.
    let run = env.newest_run();
    assert_eq!(run.files[0].checks, [Check::Llm, Check::Timing]);
    assert_eq!(run.findings.len(), 2);

    let out = env.subcheck(&["scan", &path("timing.mkv")], &[]);
    assert!(
        stderr(&out).contains(&format!(
            "Nothing to check: 1 unchanged (subcheck review '{}')",
            path("timing.mkv")
        )),
        "{}",
        stderr(&out)
    );
}

#[test]
fn scan_run_makes_only_the_checks_the_files_need() {
    let env = Env::new();
    let files = [path("timing.mkv"), path("typos.mkv")];
    let scan = |extra: &[&str]| {
        let mut args = vec!["scan", files[0].as_str(), files[1].as_str()];
        args.extend(extra);
        env.subcheck(&args, &[])
    };
    assert!(scan(&[]).status.success());
    env.sql(TIMING_STALE);
    let run = env.newest_run();
    // typos.mkv's two typos, timing.mkv's two timing findings.
    assert_eq!(run.findings.len(), 4);
    let last_id = run.findings.iter().map(|f| f.id).max().unwrap();
    let calls = env.calls().len();

    let out = scan(&["--run", &run.name]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!(
            "run {}: added to 2 files, 0 failed; 2 findings (2 timing)",
            run.name
        )),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        env.calls().len(),
        calls,
        "no LLM call: the files' LLM checks stand"
    );
    let run = env.newest_run();
    // timing.mkv's timing findings, made again, replace the old: new IDs.
    let added: Vec<(u64, &str)> = run
        .findings
        .iter()
        .filter(|f| f.id > last_id)
        .map(|f| (f.id, f.source.as_str()))
        .collect();
    assert_eq!(added, [(last_id + 1, "timing"), (last_id + 2, "timing")]);
    assert_eq!(run.findings.len(), 4);
    assert!(
        run.files
            .iter()
            .all(|f| f.checks == [Check::Llm, Check::Timing])
    );
    // typos.mkv's record keeps the lines its LLM check counted.
    let typos = run.files.iter().find(|f| f.path.ends_with("typos.mkv"));
    assert_eq!(typos.unwrap().lines, 4);

    let out = scan(&["--run", &run.name]);
    assert!(
        stderr(&out).contains(&format!(
            "Nothing to add to run {}: 2 files with every check already",
            run.name
        )),
        "{}",
        stderr(&out)
    );
    assert_eq!(env.runs(), 1);
}

#[test]
fn scan_run_refuses_another_llm_or_languages_and_checks_a_changed_file_again() {
    let env = Env::new();
    let (_copy, file) = common::copied("timing.mkv");
    let file = file.to_string_lossy().into_owned();
    assert!(env.subcheck(&["scan", &file], &[]).status.success());
    let name = env.newest_run().name;

    // With --force, the file's checks are made again, under the run.
    let out = env.subcheck(&["scan", &file, "--run", &name, "--force"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!(
            "run {name}: added to 1 file, 0 failed; 2 findings (2 timing)"
        )),
        "{}",
        stderr(&out)
    );
    // Still not with --dry-run.
    let out = env.subcheck(&["scan", &file, "--run", &name, "--dry-run"], &[]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));

    let out = env.scan_preset("gpt", &[&file, "--run", &name], &[]);
    assert!(
        stderr(&out).contains(&format!("run {name} was scanned with cmd:")),
        "{}",
        stderr(&out)
    );
    let out = env.subcheck(&["scan", &file, "--run", &name, "--lang", "und"], &[]);
    assert!(
        stderr(&out).contains(&format!(
            "run {name} checked --lang eng; leave out --lang to use it"
        )),
        "{}",
        stderr(&out)
    );

    // A file changed since the run is checked again, under it.
    let later = std::time::SystemTime::now() + std::time::Duration::from_mins(1);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let out = env.subcheck(&["scan", &file, "--run", &name], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!(
            "run {name}: added to 1 file, 0 failed; 2 findings (2 timing)"
        )),
        "{}",
        stderr(&out)
    );

    // A backend this subcheck does not know, with no command recorded, stops a --run only where a
    // file needs the LLM: here a file new to the run.
    env.sql(&format!(
        "UPDATE scans SET backend = 'gemini', command = '[]' WHERE name = '{name}'"
    ));
    let out = env.scan_plain(&[&file, &path("typos.mkv"), "--run", &name], &[]);
    assert!(
        stderr(&out).contains(&format!(
            "run {name} was scanned with gemini, which this subcheck does not know"
        )),
        "{}",
        stderr(&out)
    );
}

#[test]
fn scan_run_takes_in_a_file_another_run_checked_without_checking_it_again() {
    let env = Env::new();
    assert!(
        env.subcheck(&["scan", &path("typos.mkv")], &[])
            .status
            .success()
    );
    let first = env.newest_run();
    assert!(
        env.subcheck(&["scan", &path("timing.mkv")], &[])
            .status
            .success()
    );
    let other = env.newest_run().name;
    assert_ne!(other, first.name);
    let calls = env.calls().len();
    let typos = |run: &store::Run| -> Vec<u64> {
        run.findings
            .iter()
            .filter(|f| f.file == path("typos.mkv"))
            .map(|f| f.id)
            .collect()
    };

    // typos.mkv's checks stand: it joins the other run with nothing made again, and both runs show
    // it the same.
    let out = env.subcheck(&["scan", &path("typos.mkv"), "--run", &other], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!(
            "Nothing to add to run {other}: 1 file with every check already"
        )),
        "{}",
        stderr(&out)
    );
    assert_eq!(env.calls().len(), calls);
    let joined = env.store().view(&Selector::Run(other.clone())).unwrap();
    assert_eq!(joined.files.len(), 2);
    assert_eq!(typos(&joined), typos(&first));

    // Its LLM check gone stale (a new prompt, say): made again, under the other run; the first
    // run shows the new findings too.
    env.sql("UPDATE checks SET version = 0 WHERE kind = 'llm'");
    let out = env.subcheck(&["scan", &path("typos.mkv"), "--run", &other], &[]);
    assert!(
        stderr(&out).contains(&format!(
            "run {other}: added to 1 file, 0 failed; 2 findings\n"
        )),
        "{}",
        stderr(&out)
    );
    assert_eq!(env.calls().len(), calls + 1);
    let store = env.store();
    let (one, two) = (
        store.view(&Selector::Run(first.name.clone())).unwrap(),
        store.view(&Selector::Run(other)).unwrap(),
    );
    assert_eq!(typos(&one), typos(&two));
    assert_ne!(typos(&one), typos(&first));
}

#[test]
fn a_failed_attempt_replaces_nothing_and_the_next_scan_makes_only_the_missing_check() {
    use std::os::unix::fs::PermissionsExt;
    if rustix::process::geteuid().is_root() {
        return; // root reads a 0o000 file anyway (CI containers run as root)
    }
    let env = Env::new();
    let (_copy, file) = common::copied("timing.mkv");
    let name = file.to_string_lossy().into_owned();
    assert!(env.subcheck(&["scan", &name], &[]).status.success());
    env.sql(TIMING_STALE);
    let calls = env.calls().len();

    let mode = |m| std::fs::Permissions::from_mode(m);
    std::fs::set_permissions(&file, mode(0o000)).unwrap();
    let run = env.newest_run().name;
    let out = env.subcheck(&["scan", &name, "--run", &run], &[]);
    std::fs::set_permissions(&file, mode(0o644)).unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("FAILED"), "{}", stderr(&out));
    // The file shows failed, over its checks and findings, which stand.
    let shown = env.newest_run();
    let record = &shown.files[0];
    assert!(record.error.is_some(), "{record:?}");
    assert_eq!(record.checks, [Check::Llm, Check::Timing]);
    assert_eq!(shown.findings.len(), 2);

    // So the next --run makes the timing check alone, and the LLM's is not made again.
    let out = env.subcheck(&["scan", &name, "--run", &run], &[]);
    assert!(
        stderr(&out).contains("added to 1 file, 0 failed; 2 findings (2 timing)"),
        "{}",
        stderr(&out)
    );
    assert_eq!(env.calls().len(), calls);
    assert_eq!(env.newest_run().files[0].error, None);
}

#[test]
fn an_empty_track_costs_no_llm_call() {
    let env = Env::new();
    let out = env.subcheck(&["scan", &path("empty.mkv")], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("empty.mkv: 0 lines, 0 findings"),
        "{}",
        stderr(&out)
    );
    assert_eq!(env.calls(), [] as [String; 0]);
    assert_eq!(env.newest_run().files[0].tracks[0].read, "empty");
}

#[test]
fn an_unchanged_file_is_skipped_until_forced() {
    let env = Env::new();
    assert!(
        env.subcheck(&["scan", &path("typos.mkv")], &[])
            .status
            .success()
    );
    assert_eq!(env.calls().len(), 1);

    let out = env.subcheck(&["scan", &path("typos.mkv")], &[]);
    assert!(
        stderr(&out).contains(&format!(
            "Nothing to check: 1 unchanged (subcheck review '{}')\n",
            path("typos.mkv")
        )),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        env.calls().len(),
        1,
        "an unchanged file must not reach the LLM"
    );

    let out = env.subcheck(&["scan", &path("typos.mkv"), "--force"], &[]);
    assert!(stderr(&out).contains("1 checked"), "{}", stderr(&out));
    assert_eq!(env.calls().len(), 2);
}

#[test]
fn a_failed_force_of_an_unchanged_file_is_retried_by_the_next_plain_scan() {
    let env = Env::new();
    assert!(
        env.subcheck(&["scan", &path("typos.mkv")], &[])
            .status
            .success()
    );
    let out = env.subcheck(
        &["scan", &path("typos.mkv"), "--force"],
        &[("FAKE_LLM_FAIL", "1")],
    );
    assert!(!out.status.success());
    let calls = env.calls().len();

    // The views show the file failed, so the next plain scan checks it, not "Nothing to check".
    let out = env.subcheck(&["scan", &path("typos.mkv")], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        !stderr(&out).contains("Nothing to check"),
        "{}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("1 checked"), "{}", stderr(&out));
    assert_eq!(env.calls().len(), calls + 1);
    assert_eq!(env.newest_run().files[0].error, None);
}

#[test]
fn a_truncated_file_is_warned_about_on_every_scan() {
    let env = Env::new();
    // One byte short: the Tags at the end are cut, the subtitles are all there.
    let (_copy, file) = common::copied("typos.mkv");
    let full = std::fs::metadata(&file).unwrap().len();
    common::cut(&file, full - 1);
    let file = file.to_string_lossy().into_owned();
    let warning = format!(
        "warning: {file}: truncated: file has {} of the {full} bytes",
        full - 1
    );
    for _ in 0..2 {
        let out = env.subcheck(&["scan", &file], &[]);
        assert!(out.status.success(), "{}", stderr(&out));
        assert!(stderr(&out).contains(&warning), "{}", stderr(&out));
        assert!(stderr(&out).contains("1 checked"), "{}", stderr(&out));
    }
    // Never fresh: the second scan checked it again (an_unchanged_file_is_skipped_until_forced
    // shows the whole file is skipped).
    assert_eq!(env.calls().len(), 2);
    // The store keeps it, for list.
    assert!(env.newest_run().files[0].truncated.is_some());
    let list = env.subcheck(&["list"], &[]);
    assert!(
        String::from_utf8_lossy(&list.stdout).contains(&format!(
            "Truncated (replace these copies):\n  {file}: file has {} of the {full} bytes",
            full - 1
        )),
        "{}",
        String::from_utf8_lossy(&list.stdout)
    );
    let out = env.subcheck(&["scan", "--dry-run", &file], &[]);
    assert!(stderr(&out).contains(&warning), "{}", stderr(&out));
}

#[test]
fn a_scan_with_nothing_to_check_records_no_run() {
    let env = Env::new();
    assert!(
        env.subcheck(&["scan", &path("typos.mkv")], &[])
            .status
            .success()
    );
    assert!(
        env.subcheck(&["scan", &path("typos.mkv")], &[])
            .status
            .success()
    );
    assert_eq!(
        env.runs(),
        1,
        "an empty run would hide the real one from `subcheck list`"
    );
    assert_eq!(env.newest_run().findings.len(), 2);
}

#[test]
fn changing_the_languages_checks_a_file_again() {
    let env = Env::new();
    assert!(
        env.subcheck(&["scan", &path("typos.mkv")], &[])
            .status
            .success()
    );
    let out = env.subcheck(
        &["scan", &path("typos.mkv"), "--lang", "eng", "--lang", "und"],
        &[],
    );
    assert!(stderr(&out).contains("1 checked"), "{}", stderr(&out));
    assert_eq!(env.calls().len(), 2);
}

#[test]
fn dry_run_leaves_out_files_a_scan_would_skip() {
    let env = Env::new();
    assert!(
        env.subcheck(&["scan", &path("typos.mkv")], &[])
            .status
            .success()
    );
    let out = env.subcheck(&["scan", &path("typos.mkv"), "--dry-run"], &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("(chunk"), "{stdout}");
    assert!(
        stderr(&out).contains("1 file unchanged since an earlier scan"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn an_llm_failure_fails_the_file_after_one_retry_and_is_checked_again() {
    let env = Env::new();
    let out = env.subcheck(&["scan", &path("typos.mkv")], &[("FAKE_LLM_FAIL", "1")]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("FAILED: LLM: sh exited with exit status: 3: fake-llm: told to fail"),
        "{}",
        stderr(&out)
    );
    assert_eq!(env.calls().len(), 2, "one call plus one retry");
    let run = env.newest_run();
    assert!(run.files[0].error.is_some());

    // A failed check is never fresh: the next scan tries again.
    let out = env.subcheck(&["scan", &path("typos.mkv")], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(env.newest_run().findings.len(), 2);
}

#[test]
fn a_reply_with_neither_findings_nor_none_is_a_failure() {
    let env = Env::new();
    let out = env.subcheck(
        &["scan", &path("typos.mkv")],
        &[("FAKE_LLM_REPLY", "These lines look fine to me.")],
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("reply has no findings and no NONE"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn lines_shared_between_files_are_sent_once() {
    let env = Env::new();
    // ass-plain's one track (language und) and multi's English Full track hold the same five
    // lines; multi's Signs track changes one of them. Six distinct lines in all.
    let out = env.subcheck(
        &[
            "scan",
            &path("ass-plain.mkv"),
            &path("multi.mkv"),
            "--lang",
            "eng",
            "--lang",
            "und",
        ],
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    // Two workers: the calls can land in either order.
    let mut calls = env.calls();
    calls.sort();
    assert_eq!(calls, ["1 lines", "5 lines"]);
    let run = env.newest_run();
    assert_eq!(
        run.files.iter().map(|f| f.lines).collect::<Vec<_>>(),
        [5, 6]
    );
}

#[test]
fn a_track_tagged_only_in_bcp_47_is_named_by_it() {
    let env = Env::new();
    // multi.mkv's track 3 (`jpn`, `ja`, unnamed) loses its Language to a Void of the same length.
    let language = [0x22, 0xB5, 0x9C, 0x83, b'j', b'p', b'n'];
    let (_dir, file) = common::patched("multi.mkv", &language, 1, |b| {
        b.copy_from_slice(&[0xEC, 0x85, 0, 0, 0, 0, 0]);
    });
    let file = file.to_string_lossy();
    let out = env.subcheck(&["scan", &file, "--lang", "jpn", "-v"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("    track 3 (ja): "),
        "{}",
        stderr(&out)
    );
    let run = env.newest_run();
    let tracks = &run.files[0].tracks;
    assert_eq!(
        tracks
            .iter()
            .map(|t| (t.id, t.language.as_str(), t.language_bcp47.as_deref()))
            .collect::<Vec<_>>(),
        [(3, "eng", Some("ja"))]
    );
    assert_eq!(
        subcheck_core::list::track_name_in(tracks, 3),
        Some(("(ja)".into(), false))
    );
}

#[test]
fn a_directory_is_walked_and_a_bad_file_does_not_stop_the_scan() {
    let env = Env::new();
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(fixture("typos.mkv"), dir.path().join("a.mkv")).unwrap();
    std::fs::write(dir.path().join("b.mkv"), "not a matroska file").unwrap();
    let out = env.subcheck(&["scan", &dir.path().to_string_lossy()], &[]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("a.mkv: 4 lines, 2 findings"), "{err}");
    assert!(
        err.contains("b.mkv: FAILED: parse error at byte 0"),
        "{err}"
    );
}

#[test]
fn dry_run_prints_prompts_and_records_nothing() {
    let env = Env::new();
    let out = env.subcheck(&["scan", &path("typos.mkv"), "--dry-run"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("(chunk 0, 4 lines)"), "{stdout}");
    assert!(
        stdout.contains("L3 [0:00:20.00] (Song) go trough it\n"),
        "{stdout}"
    );
    assert_eq!(env.calls(), [] as [String; 0]);
    assert_eq!(env.runs(), 0);
}

#[test]
fn list_shows_the_newest_run_with_mpv_commands() {
    let env = Env::new();
    assert!(
        env.subcheck(&["scan", &path("typos.mkv")], &[])
            .status
            .success()
    );
    let out = env.subcheck(&["list", "--mpv"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("  #1  0:00:01.00  Default  [pending]\n      - We should leave befor it rains.\n      + We should leave before it rains.\n      why: planted typo\n"), "{text}");
    assert!(
        text.contains(&format!(
            "      mpv --start=0:00:18.00 --sid=1 '{}'\n",
            path("typos.mkv")
        )),
        "{text}"
    );

    let out = env.subcheck(&["list", "--accepted"], &[]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("No findings."));
}

#[test]
fn list_without_any_run_says_so() {
    let env = Env::new();
    let out = env.subcheck(&["list"], &[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("no runs yet"), "{}", stderr(&out));
    let out = env.subcheck(&["list", "2026-10-17T14-03-22"], &[]);
    assert_eq!(stderr(&out), "subcheck: no run named 2026-10-17T14-03-22\n");
}

#[test]
fn list_takes_a_run_or_the_files_under_some_paths() {
    let env = Env::new();
    let dir = tempfile::tempdir().unwrap();
    let show = dir.path().join("Show");
    std::fs::create_dir(&show).unwrap();
    for ep in ["E01", "E02"] {
        std::fs::copy(fixture("typos.mkv"), show.join(format!("{ep}.mkv"))).unwrap();
    }
    let show = show.to_string_lossy().into_owned();
    assert!(env.subcheck(&["scan", &show], &[]).status.success());
    let name = env.newest_run().name;
    let first_line = |args: &[&str]| {
        let out = env.subcheck(args, &[]);
        assert!(out.status.success(), "{}", stderr(&out));
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        first_line(&["list", &name]),
        format!(
            "Run {name} (cmd:'{}'): 2 files, 4 findings, 0 failed",
            fake_llm()
        )
    );
    assert_eq!(
        first_line(&["list", &show]),
        format!("'{show}': 2 files, 4 findings, 0 failed")
    );
    let e01 = format!("{show}/E01.mkv");
    assert_eq!(
        first_line(&["list", &e01]),
        format!("'{e01}': 1 files, 2 findings, 0 failed")
    );
    // A directory with nothing recorded under it, and a run name beside a path.
    let empty = dir.path().join("Empty");
    std::fs::create_dir(&empty).unwrap();
    let out = env.subcheck(&["list", &empty.to_string_lossy()], &[]);
    assert_eq!(
        stderr(&out),
        format!("subcheck: nothing recorded under '{}'\n", empty.display())
    );
    let out = env.subcheck(&["list", &show, &name], &[]);
    assert_eq!(
        stderr(&out),
        format!("subcheck: {name}: no such file or directory (a run name goes alone)\n")
    );
}

/// The fake LLM's path, as `--llm-cmd` gives it.
fn fake_llm() -> String {
    common::fake_llm().display().to_string()
}

#[test]
fn the_database_is_where_subcheck_db_says_else_in_the_state_directory() {
    let env = Env::new();
    assert!(!env.subcheck(&["list"], &[]).status.success());
    assert!(env.db().is_file());
    let state_dir = env.state.path().join("subcheck/subcheck.db");
    assert!(!state_dir.exists());
    // Set but empty is not set.
    assert!(
        !env.subcheck(&["list"], &[("SUBCHECK_DB", "")])
            .status
            .success()
    );
    assert!(state_dir.is_file());
}

#[test]
fn a_relative_subcheck_db_is_refused_and_a_later_ones_database_named() {
    let env = Env::new();
    let out = env.subcheck(&["list"], &[("SUBCHECK_DB", "subcheck.db")]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "subcheck: SUBCHECK_DB must be an absolute path\n"
    );
    drop(env.store());
    env.sql("PRAGMA user_version = 2");
    let out = env.subcheck(&["list"], &[]);
    assert_eq!(
        stderr(&out),
        format!(
            "subcheck: opening {} (from SUBCHECK_DB): subcheck.db was made by a later subcheck\n",
            env.db().display()
        )
    );
}

#[test]
fn a_bare_list_shows_the_newest_runs_folders() {
    let env = Env::new();
    let (timing, typos) = (path("timing.mkv"), path("typos.mkv"));
    assert!(
        env.subcheck(&["scan", &timing, &typos], &[])
            .status
            .success()
    );
    assert!(
        env.subcheck(&["scan", &typos, "--force"], &[])
            .status
            .success()
    );
    // The newest run is typos.mkv alone; the folder it sits in holds both.
    assert_eq!(env.newest_run().files.len(), 1);
    let shown = env.store().view(&Selector::NewestFolders).unwrap();
    let files: Vec<&str> = shown.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(files, [timing.as_str(), typos.as_str()]);
    let folder = fixture("typos.mkv")
        .parent()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let list = env.subcheck(&["list"], &[]);
    let text = String::from_utf8_lossy(&list.stdout);
    // The heading is the folder, quoted, then its counts.
    let heading = text.lines().next().unwrap_or_default();
    assert!(heading.starts_with(&shell_quote(&folder)), "{text}");
}

#[test]
fn an_entry_in_the_settings_file_is_an_llm_by_its_name() {
    let env = Env::new();
    env.settings(&format!(
        "[llm.mine]\ncommand = \"'{}'\"\nchunk_lines = 1\n",
        fake_llm()
    ));
    let out = env.scan_preset("mine", &[&env.typos()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let calls = env.calls();
    assert!(
        calls.len() > 1 && calls.iter().all(|c| c == "1 lines"),
        "{calls:?}"
    );
    assert_eq!(
        env.newest_run().header.expect("the run's header").backend,
        "mine"
    );
}

#[test]
fn chunk_lines_on_the_command_line_beats_the_entrys() {
    let env = Env::new();
    env.settings(&format!(
        "[llm.mine]\ncommand = \"'{}'\"\nchunk_lines = 1\n",
        fake_llm()
    ));
    let out = env.scan_preset("mine", &["--chunk-lines", "400", &env.typos()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(env.calls().len(), 1, "{:?}", env.calls());
}

#[test]
fn llm_timeout_takes_one_to_1440_minutes() {
    let env = Env::new();
    // A refused flag ends the command before it opens the store, whose table `runs` reads.
    drop(env.store());
    for minutes in ["0", "1441"] {
        let out = env.subcheck(&["scan", "--llm-timeout", minutes, &env.typos()], &[]);
        assert_eq!(out.status.code(), Some(2), "{minutes}: {}", stderr(&out));
        // clap's words for a value outside the range, not for a flag it does not know.
        let said = format!("'--llm-timeout <MINUTES>': {minutes} is not in 1..=1440");
        assert!(stderr(&out).contains(&said), "{}", stderr(&out));
        assert_eq!(env.runs(), 0, "{minutes}");
    }
    let out = env.subcheck(&["scan", "--llm-timeout", "5", &env.typos()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "1 files, 1 checked, 0 failed, 0 unchanged since an earlier scan; 2 findings"
        ),
        "{}",
        stderr(&out)
    );
    assert_eq!(env.runs(), 1);
}

#[test]
fn the_settings_files_default_is_the_llm_when_none_is_named() {
    let env = Env::new();
    env.settings(&format!(
        "default = \"mine\"\n[llm.mine]\ncommand = \"'{}'\"\n",
        fake_llm()
    ));
    let out = env.scan_plain(&[&env.typos()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        env.newest_run().header.expect("the run's header").backend,
        "mine"
    );
    // A default that names nothing is the file's mistake: no --llm was given.
    env.settings("default = \"nope\"\n");
    let out = env.scan_plain(&[&env.typos()], &[]);
    assert!(!out.status.success());
    let said = stderr(&out);
    assert!(
        said.contains(&format!(
            "{}: default \"nope\" names no LLM: use claude, gpt, ollama:<model>, or an entry in the \
             file",
            env.config().display()
        )) && !said.contains("--llm"),
        "{said}"
    );
    // --llm beats the default, and a name it does not know is its own mistake.
    let out = env.scan_preset("mien", &[&env.typos()], &[]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("unknown --llm \"mien\""),
        "{}",
        stderr(&out)
    );
    // A preset is a default too.
    env.settings("default = \"claude\"\n");
    let out = env.scan_plain(
        &[&env.typos()],
        &[(
            "PATH",
            &format!(
                "{}:{}",
                env.fake_bin().display(),
                std::env::var("PATH").unwrap()
            ),
        )],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        env.newest_run().header.expect("the run's header").backend,
        "claude"
    );
}

#[test]
fn an_entry_printing_openai_json_records_what_its_calls_used() {
    let env = Env::new();
    let openai = env.fake_bin().join("openai");
    env.settings(&format!(
        "[llm.oa]\ncommand = \"'{}'\"\noutput = \"openai\"\n",
        openai.display()
    ));
    assert!(env.scan_preset("oa", &[&env.typos()], &[]).status.success());
    let out = env.subcheck(&["list"], &[]);
    let listed = String::from_utf8_lossy(&out.stdout);
    assert!(
        listed.contains("out tokens") && !listed.contains('$'),
        "{listed}"
    );
}

#[test]
fn scanning_again_with_the_same_entry_checks_nothing_and_a_command_by_hand_checks_again() {
    let env = Env::new();
    let file = env.typos();
    env.settings(&format!("[llm.mine]\ncommand = \"'{}'\"\n", fake_llm()));
    assert!(env.scan_preset("mine", &[&file], &[]).status.success());
    let first = env.calls().len();
    assert!(env.scan_preset("mine", &[&file], &[]).status.success());
    assert_eq!(env.calls().len(), first);
    // `subcheck` adds --llm-cmd with the same command: another label, so checked again.
    assert!(env.subcheck(&["scan", &file], &[]).status.success());
    assert!(env.calls().len() > first);
}

#[test]
fn a_broken_settings_file_stops_a_scan_and_leaves_list_alone() {
    let env = Env::new();
    let file = env.typos();
    assert!(env.subcheck(&["scan", &file], &[]).status.success());
    env.settings("[llm.mine]\ncomand = \"x\"\n");
    let out = env.subcheck(&["scan", "--force", &file], &[]);
    assert!(!out.status.success());
    let said = stderr(&out);
    assert!(
        said.contains(&env.config().display().to_string()) && said.contains("comand"),
        "{said}"
    );
    assert!(env.subcheck(&["list"], &[]).status.success());
}

#[test]
fn the_settings_file_is_where_subcheck_config_says_else_under_xdg_config_home() {
    let env = Env::new();
    let out = env.subcheck(
        &["scan", &env.typos()],
        &[("SUBCHECK_CONFIG", "config.toml")],
    );
    assert!(
        stderr(&out).contains("SUBCHECK_CONFIG must be an absolute path"),
        "{}",
        stderr(&out)
    );
    let xdg = env.state.path().join("xdg");
    std::fs::create_dir_all(xdg.join("subcheck")).unwrap();
    std::fs::write(
        xdg.join("subcheck/config.toml"),
        format!("[llm.mine]\ncommand = \"'{}'\"\n", fake_llm()),
    )
    .unwrap();
    let xdg = xdg.display().to_string();
    let out = env.scan_preset(
        "mine",
        &[&env.typos()],
        &[("SUBCHECK_CONFIG", ""), ("XDG_CONFIG_HOME", &xdg)],
    );
    assert!(out.status.success(), "{}", stderr(&out));
}

fn server_entry(env: &Env, url: &str, extra: &str) {
    env.settings(&format!("[llm.srv]\nurl = \"{url}\"\n{extra}"));
}

#[test]
fn a_server_entry_scans_through_the_server_and_says_its_tokens() {
    let env = Env::new();
    let fake = common::FakeServer::llm(&["fake-model"]);
    server_entry(&env, &fake.url, "");
    let out = env.scan_preset("srv", &[&env.typos()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    // The fake counts a prompt as subcheck estimates it.
    let said = format!(
        "LLM: 1 call, {} in / 40 out tokens",
        estimate(common::TYPOS_LINES)
    );
    assert!(stderr(&out).contains(&said), "{}", stderr(&out));
    assert_eq!(newest(&env).header.backend, "srv@fake-model");
    assert_eq!(
        newest(&env).header.command,
        [format!("{}/chat/completions", fake.url)]
    );
    assert_eq!(env.newest_run().findings.len(), 2);
    let asked: Vec<String> = fake
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    assert_eq!(
        asked,
        ["GET /v1/models", "GET /api/ps", "POST /v1/chat/completions"]
    );
}

#[test]
fn nothing_answering_stops_a_scan_before_any_file_is_read() {
    let env = Env::new();
    let closed = common::closed_port_url();
    server_entry(&env, &closed, "");
    let out = env.scan_preset("srv", &[&env.typos()], &[]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(&format!(
            "srv: nothing answering at {closed}: is the server running?"
        )),
        "{}",
        stderr(&out)
    );
    assert_eq!(env.runs(), 0);
}

#[test]
fn a_dry_run_asks_the_server_for_its_models_and_nothing_else() {
    let env = Env::new();
    let fake = common::FakeServer::llm(&["fake-model"]);
    server_entry(&env, &fake.url, "");
    let out = env.scan_preset("srv", &["--dry-run", &env.typos()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("(chunk 0, "));
    assert!(!fake.requests().is_empty() && fake.requests().iter().all(|r| r.method == "GET"));
}

#[test]
fn an_api_key_reaches_the_server_and_nowhere_else() {
    let key = "sk-test-7c1e";
    let env = Env::new();
    let fake = common::FakeServer::llm(&["m"]);
    server_entry(&env, &fake.url, "api_key_env = \"SUBCHECK_TEST_KEY\"\n");
    let out = env.scan_preset("srv", &[&env.typos()], &[("SUBCHECK_TEST_KEY", key)]);
    assert!(out.status.success(), "{}", stderr(&out));
    let bearer = format!("Bearer {key}");
    assert!(
        fake.requests()
            .iter()
            .all(|r| r.header("authorization") == Some(bearer.as_str()))
    );
    let refusing = common::FakeServer::start(move |r| match r.path.as_str() {
        "/v1/models" => common::models_reply(&["m"]),
        _ => common::Reply::status(
            401,
            format!("{{\"error\":{{\"message\":\"bad key {key}\"}}}}"),
        ),
    });
    server_entry(&env, &refusing.url, "api_key_env = \"SUBCHECK_TEST_KEY\"\n");
    let out = env.scan_preset(
        "srv",
        &["--force", &env.typos()],
        &[("SUBCHECK_TEST_KEY", key)],
    );
    assert!(
        stderr(&out).contains("HTTP 401: bad key <key>; check the entry's key"),
        "{}",
        stderr(&out)
    );
    let mut stored = std::fs::read(env.db()).unwrap();
    stored.extend(std::fs::read(env.db().with_extension("db-wal")).unwrap_or_default());
    for (what, text) in [
        ("stderr", stderr(&out).into_bytes()),
        ("store", stored),
        ("label", newest(&env).header.backend.into_bytes()),
    ] {
        assert!(
            !text.windows(key.len()).any(|w| w == key.as_bytes()),
            "the key is in the {what}"
        );
    }
}

#[test]
fn a_run_resumes_only_while_the_server_serves_its_model() {
    let env = Env::new();
    let file = env.typos();
    let first = common::FakeServer::llm(&["a"]);
    server_entry(&env, &first.url, "");
    assert!(env.scan_preset("srv", &[&file], &[]).status.success());
    let run = newest(&env).name;
    // Nothing for the LLM to do: the server is not asked, so it need not be up.
    server_entry(&env, &common::closed_port_url(), "");
    let out = env.scan_plain(&["--run", &run, &file], &[]);
    assert!(
        out.status.success() && stderr(&out).contains(&format!("Nothing to add to run {run}")),
        "{}",
        stderr(&out)
    );
    let second = common::FakeServer::llm(&["b"]);
    server_entry(&env, &second.url, "");
    for llm in [&[][..], &["--llm", "srv"][..]] {
        let mut args = vec!["--run", run.as_str(), "--force", file.as_str()];
        args.extend_from_slice(llm);
        let out = env.scan_plain(&args, &[]);
        assert!(!out.status.success());
        let said = format!(
            "srv@a: {} does not serve \"a\"; it serves \"b\"",
            second.url
        );
        assert!(stderr(&out).contains(&said), "{}", stderr(&out));
    }
    assert!(second.completions().is_empty());
    server_entry(&env, &first.url, "chunk_lines = 1\n");
    let out = env.scan_plain(&["--run", &run, "--force", &file], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        first.completions().len() > 2,
        "the entry's chunk_lines as the file has it now"
    );
}

#[test]
fn a_server_on_this_machine_is_never_called_through_a_proxy() {
    let env = Env::new();
    let fake = common::FakeServer::llm(&["m"]);
    server_entry(&env, &fake.url, "");
    // Nothing listens on the proxy's port, so a call sent through it fails. NO_PROXY is set empty,
    // naming no host: one in the tests' own environment cannot steer the calls around the proxy.
    let proxy = "http://127.0.0.1:1";
    let mut vars = [
        "http_proxy",
        "HTTP_PROXY",
        "https_proxy",
        "HTTPS_PROXY",
        "ALL_PROXY",
    ]
    .map(|name| (name, proxy))
    .to_vec();
    vars.extend([("NO_PROXY", ""), ("no_proxy", "")]);
    let out = env.scan_preset("srv", &[&env.typos()], &vars);
    assert!(out.status.success(), "{}", stderr(&out));
    let asked: Vec<String> = fake
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    assert_eq!(
        asked,
        ["GET /v1/models", "GET /api/ps", "POST /v1/chat/completions"]
    );
}

/// How many subtitle lines each chat completion sent to `fake` held, in the order they came.
fn sent_lines(fake: &common::FakeServer) -> Vec<usize> {
    fake.completions()
        .iter()
        .map(common::prompt_lines)
        .collect()
}

#[test]
fn a_scan_through_a_server_fits_its_chunks_to_the_context() {
    // 1024 tokens shared by the default 2 jobs.
    let per_call = fitting_lines(1024 / 2);
    assert!(per_call < common::TYPOS_LINES);
    let env = Env::new();
    let fake = common::FakeServer::llm_sized(&["m"], 1024);
    server_entry(&env, &fake.url, "");
    let out = env.scan_preset("srv", &[&env.typos()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let sent = sent_lines(&fake);
    assert!(
        sent.len() > 1 && sent.iter().all(|&n| n <= per_call),
        "{sent:?}"
    );
    // Each reply capped at the last quarter of a call's share.
    let caps: Vec<Option<u64>> = fake
        .completions()
        .iter()
        .map(|r| r.json()["max_tokens"].as_u64())
        .collect();
    assert!(caps.iter().all(|&c| c == Some(1024 / 2 / 4)), "{caps:?}");
    assert_eq!(env.newest_run().findings.len(), 2);
}

#[test]
fn a_size_forced_over_what_fits_is_cut_to_it_and_each_call_capped() {
    // 1400 tokens shared by the default 2 jobs: 700 a call, whose prompt holds fewer than all of
    // typos.mkv's lines, which --chunk-lines asks for.
    let fits = fitting_lines(1400 / 2);
    let rest = common::TYPOS_LINES - fits;
    assert!(fits < common::TYPOS_LINES);
    let env = Env::new();
    let fake = common::FakeServer::llm_sized(&["m"], 1400);
    server_entry(&env, &fake.url, "");
    let lines = common::TYPOS_LINES.to_string();
    let file = env.typos();
    let dry = env.scan_preset("srv", &["--dry-run", "--chunk-lines", &lines, &file], &[]);
    assert!(dry.status.success(), "{}", stderr(&dry));
    let printed: Vec<String> = String::from_utf8_lossy(&dry.stdout)
        .lines()
        .filter_map(|l| {
            l.strip_prefix("== ")?
                .rsplit_once(" (")
                .map(|(_, h)| h.to_string())
        })
        .collect();
    assert_eq!(
        printed,
        [
            format!("chunk 0, {fits} lines)"),
            format!("chunk 1, {rest} lines)")
        ]
    );
    let out = env.scan_preset("srv", &["--chunk-lines", &lines, &file], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let mut sent = sent_lines(&fake);
    sent.sort_unstable();
    assert_eq!(sent, [rest, fits]);
    // Each reply capped at the last quarter of a call's share.
    let caps: Vec<Option<u64>> = fake
        .completions()
        .iter()
        .map(|r| r.json()["max_tokens"].as_u64())
        .collect();
    assert_eq!(caps, [Some(1400 / 2 / 4); 2]);
    assert_eq!(env.newest_run().findings.len(), 2);
}

/// What llama-server answers `r` with for a model that repeats itself until something stops it:
/// a reply cut off at the request's `max_tokens`, or, with none, the 500 it gives once the replies
/// in flight have filled the cache they share.
fn looping(r: &common::Request) -> common::Reply {
    let Some(cap) = r.json()["max_tokens"].as_u64() else {
        let full = std::fs::read_to_string(fixture("llm/llama-server-shared-cache.json")).unwrap();
        return common::Reply::status(500, full);
    };
    let cut = serde_json::json!({
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "L1\tWe should leave before before before"},
            "finish_reason": "length",
        }],
        "usage": {"prompt_tokens": 600, "completion_tokens": cap, "total_tokens": 600 + cap},
    });
    common::Reply::ok(cut.to_string())
}

#[test]
fn a_model_that_runs_to_its_cap_every_time_fails_its_chunk_in_three_calls() {
    // 4096 tokens shared by the default 2 jobs: 2048 a call, which holds all of typos.mkv's lines,
    // and a reply capped at 512.
    assert!(fitting_lines(4096 / 2) >= common::TYPOS_LINES);
    let listed = serde_json::json!({
        "object": "list",
        "data": [{"id": "m", "object": "model", "meta": {"n_ctx": 4096}}],
    })
    .to_string();
    let fake = common::FakeServer::start(move |r| match (r.method.as_str(), r.path.as_str()) {
        ("GET", "/v1/models") => common::Reply::ok(listed.clone()),
        ("POST", "/v1/chat/completions") => looping(r),
        _ => common::Reply::status(404, "{}"),
    });
    let env = Env::new();
    server_entry(&env, &fake.url, "");
    let out = env.scan_preset("srv", &[&env.typos()], &[]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(
            "FAILED: LLM: the reply was cut off at subcheck's cap of 512 tokens; the entry's \
             request.max_tokens sets another"
        ),
        "{}",
        stderr(&out)
    );
    // Cut off whole, then halved once, the first half cut off again and retried; the second half
    // is not sent.
    let half = common::TYPOS_LINES / 2;
    assert_eq!(sent_lines(&fake), [common::TYPOS_LINES, half, half]);
    let used: Vec<(usize, Option<u64>)> = env
        .recorded_calls()
        .iter()
        .map(|c| (c.lines, c.usage.as_ref().map(|u| u.output_tokens)))
        .collect();
    let capped = Some(4096 / 2 / 4);
    assert_eq!(
        used,
        [
            (common::TYPOS_LINES, capped),
            (half, capped),
            (half, capped)
        ]
    );
}

#[test]
fn the_flag_and_the_entry_beat_the_servers_context() {
    // 4096 tokens shared by the default 2 jobs: fitted to them, one chunk holds all the lines
    // typos.mkv sends, and the flag or the entry makes it two.
    assert!(fitting_lines(4096 / 2) >= common::TYPOS_LINES);
    let all = common::TYPOS_LINES;
    let half = all / 2;
    let lines = half.to_string();
    let entry = format!("chunk_lines = {lines}\n");
    for (extra, flag, sent) in [
        ("", &[][..], vec![all]),
        ("", &["--chunk-lines", &lines][..], vec![half, half]),
        (&entry, &[][..], vec![half, half]),
    ] {
        let env = Env::new();
        let fake = common::FakeServer::llm_sized(&["m"], 4096);
        server_entry(&env, &fake.url, extra);
        let file = env.typos();
        let mut args = flag.to_vec();
        args.push(&file);
        let out = env.scan_preset("srv", &args, &[]);
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(sent_lines(&fake), sent, "{extra:?} {flag:?}");
    }
}

#[test]
fn a_server_that_does_not_say_its_size_warns_once_and_sends_400_lines() {
    let env = Env::new();
    let fake = common::FakeServer::llm(&["m"]);
    server_entry(&env, &fake.url, "");
    let out = env.scan_preset("srv", &[&env.typos()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let warning =
        "warning: srv did not say its context size; set chunk_lines if it holds under 38k tokens\n";
    assert_eq!(stderr(&out).matches(warning).count(), 1, "{}", stderr(&out));
    // A chunk of up to 400 lines takes all typos.mkv sends.
    assert_eq!(sent_lines(&fake), [common::TYPOS_LINES]);
}

#[test]
fn an_entrys_cap_that_leaves_no_room_for_a_line_warns_once_and_sends_each_line_alone() {
    // 16384 tokens shared by the default 2 jobs: 8192 a call, all of it the reply's.
    let env = Env::new();
    let fake = common::FakeServer::llm_sized(&["m"], 16384);
    server_entry(&env, &fake.url, "[llm.srv.request]\nmax_tokens = 8192\n");
    let out = env.scan_preset("srv", &[&env.typos()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let warning = "warning: srv's request.max_tokens (8192) leaves no room for lines in a call's \
                   8192 tokens, so each line is sent alone; lower it, or set shared_context = \
                   false or fewer jobs\n";
    assert_eq!(stderr(&out).matches(warning).count(), 1, "{}", stderr(&out));
    assert_eq!(sent_lines(&fake), [1; common::TYPOS_LINES]);
    assert_eq!(env.newest_run().findings.len(), 2);
}

#[test]
fn an_overflowing_chunk_is_split_and_every_finding_lands() {
    let overflow = std::fs::read_to_string(fixture("llm/llama-server-overflow.json")).unwrap();
    let fake = common::FakeServer::start(move |r| match (r.method.as_str(), r.path.as_str()) {
        ("GET", "/v1/models") => common::models_reply(&["m"]),
        ("POST", "/v1/chat/completions") if common::prompt_lines(r) > 1 => {
            common::Reply::status(400, overflow.clone())
        }
        ("POST", "/v1/chat/completions") => common::answered(r),
        _ => common::Reply::status(404, "{}"),
    });
    let env = Env::new();
    server_entry(&env, &fake.url, "");
    let out = env.scan_preset("srv", &["-j", "2", &env.typos()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let mut sent = sent_lines(&fake);
    sent.sort_unstable();
    assert_eq!(sent, [1, 1, 1, 1, 2, 2, 4]);
    let mut recorded: Vec<usize> = env.recorded_calls().iter().map(|c| c.lines).collect();
    recorded.sort_unstable();
    assert_eq!(recorded, sent, "each call recorded with the lines it sent");
    // As many as a scan whose chunk fits finds.
    assert_eq!(env.newest_run().findings.len(), 2);
}

#[test]
fn only_a_scan_that_needs_the_llm_loads_the_model() {
    let loads = |fake: &common::FakeServer| {
        fake.requests()
            .iter()
            .filter(|r| r.method == "POST" && r.path == "/api/generate")
            .count()
    };
    let only_gets = |fake: &common::FakeServer| fake.requests().iter().all(|r| r.method == "GET");
    let env = Env::new();
    let file = env.typos();
    let fake = common::FakeServer::ollama("m", 4096);
    server_entry(&env, &fake.url, "");
    let out = env.scan_preset("srv", &["--dry-run", &file], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(only_gets(&fake), "{:?}", fake.requests());
    let out = env.scan_preset("srv", &[&file], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(loads(&fake), 1, "{:?}", fake.requests());
    assert_eq!(fake.completions().len(), 1);
    let run = newest(&env).name;
    // The same scan through a server whose model is not loaded: nothing to check, so no load.
    let unloaded = common::FakeServer::ollama("m", 4096);
    server_entry(&env, &unloaded.url, "");
    let out = env.scan_preset("srv", &[&file], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(only_gets(&unloaded), "{:?}", unloaded.requests());
    assert_eq!(loads(&fake), 1);
    // Adding to the run, which readies only once a file needs the LLM, then sizes.
    let out = env.scan_plain(&["--run", &run, "--force", &file], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(loads(&unloaded), 1, "{:?}", unloaded.requests());
}

/// `OLLAMA_HOST` for a scan through `ollama:<model>`: `fake`'s address, as Ollama's is written.
fn ollama_host(fake: &common::FakeServer) -> &str {
    fake.url.strip_suffix("/v1").unwrap()
}

#[test]
fn a_scan_says_so_while_ollama_loads_its_model() {
    let env = Env::new();
    let fake = common::FakeServer::ollama("gemma4:latest", 4096);
    let host = [("OLLAMA_HOST", ollama_host(&fake))];
    let loading = format!(
        "loading gemma4:latest into Ollama at {}\n",
        ollama_host(&fake)
    );
    let file = env.typos();
    let out = env.scan_preset("ollama:gemma4", &[&file], &host);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stderr(&out).matches(&loading).count(),
        1,
        "{}",
        stderr(&out)
    );
    // Loaded by that scan, so the next one loads nothing and says nothing of it.
    let out = env.scan_preset("ollama:gemma4", &["--force", &file], &host);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stderr(&out).contains("into Ollama"), "{}", stderr(&out));
}

/// How many times stderr holds `line` and its line end.
fn times_said(out: &Output, line: &str) -> usize {
    stderr(out).matches(&format!("{line}\n")).count()
}

#[test]
fn a_key_command_that_takes_a_while_is_named_on_stderr() {
    let env = Env::new();
    let fake = common::FakeServer::llm(&["m"]);
    let cmd = "sleep 3; echo sk-x";
    server_entry(&env, &fake.url, &format!("api_key_cmd = \"{cmd}\"\n"));
    let out = env.scan_preset("srv", &[&env.typos()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        times_said(&out, "srv: waiting for its key command"),
        1,
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_server_slow_to_say_what_it_serves_is_named_on_stderr() {
    let env = Env::new();
    let fake = common::FakeServer::llm_slow(&["m"], Duration::from_secs(3));
    server_entry(&env, &fake.url, "");
    let out = env.scan_preset("srv", &[&env.typos()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let asking = format!("srv: asking {} what it serves", fake.url);
    assert_eq!(times_said(&out, &asking), 1, "{}", stderr(&out));
}

#[test]
fn waits_that_end_within_a_second_say_nothing() {
    let env = Env::new();
    // The chat answered 3 s late keeps the scan going three holds past both waits' end, so a line
    // still held for either would be on stderr by the time the scan ends.
    let fake = common::FakeServer::start(|r| match (r.method.as_str(), r.path.as_str()) {
        ("GET", "/v1/models") => common::models_reply(&["m"]),
        ("POST", "/v1/chat/completions") => common::answered(r).after(Duration::from_secs(3)),
        _ => common::Reply::status(404, "{}"),
    });
    let cmd = "echo sk-x";
    server_entry(&env, &fake.url, &format!("api_key_cmd = \"{cmd}\"\n"));
    let out = env.scan_preset("srv", &[&env.typos()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    for wait in ["waiting for its key command", "what it serves"] {
        assert!(!stderr(&out).contains(wait), "{}", stderr(&out));
    }
}

#[test]
fn a_key_fetched_again_mid_scan_is_named_on_stderr() {
    let env = Env::new();
    // The first run leaves the marker, so each run after it takes a while and prints the second
    // key.
    let ran = env.state.path().join("ran");
    let cmd = format!(
        "if test -e '{0}'; then sleep 3; echo sk-2; else touch '{0}'; echo sk-1; fi",
        ran.display()
    );
    let first = AtomicBool::new(true);
    let fake = common::FakeServer::start(move |r| match (r.method.as_str(), r.path.as_str()) {
        ("GET", "/v1/models") => common::models_reply(&["m"]),
        ("POST", "/v1/chat/completions") => match r.header("authorization") {
            // An answer marks sk-1 as a key that has worked, which a 401 must find for the key
            // command to run again.
            Some("Bearer sk-1") if first.swap(false, Ordering::SeqCst) => common::answered(r),
            Some("Bearer sk-2") => common::answered(r),
            _ => common::Reply::status(401, ""),
        },
        _ => common::Reply::status(404, "{}"),
    });
    server_entry(&env, &fake.url, &format!("api_key_cmd = \"{cmd}\"\n"));
    let args = ["--chunk-lines", "1", "-j", "1", &env.typos()];
    let out = env.scan_preset("srv", &args, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        times_said(&out, "srv: waiting for its key command"),
        1,
        "{}",
        stderr(&out)
    );
}

#[test]
fn ollama_is_called_through_its_api_with_tokens_and_fitted_chunks() {
    // 700 tokens, each of the default 2 jobs' whole: a chunk fitted to them holds fewer lines than
    // typos.mkv sends, and more than one fitted to half of them.
    let per_call = fitting_lines(700);
    assert!(per_call < common::TYPOS_LINES && per_call > fitting_lines(700 / 2));
    let env = Env::new();
    let fake = common::FakeServer::ollama("gemma4:latest", 700);
    let host = [("OLLAMA_HOST", ollama_host(&fake))];
    let out = env.scan_preset("ollama:gemma4", &[&env.typos()], &host);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        // The fake counts each prompt as subcheck estimates it.
        stderr(&out).contains(&format!(
            "LLM: 2 calls, {} in / 80 out tokens",
            estimate(per_call) + estimate(common::TYPOS_LINES - per_call)
        )),
        "{}",
        stderr(&out)
    );
    assert_eq!(newest(&env).header.backend, "ollama:gemma4");
    let loads = fake
        .requests()
        .iter()
        .filter(|r| r.method == "POST" && r.path == "/api/generate")
        .count();
    assert_eq!(loads, 1, "{:?}", fake.requests());
    let chats = fake.chats();
    let mut sent: Vec<usize> = chats.iter().map(common::prompt_lines).collect();
    sent.sort_unstable();
    assert_eq!(
        sent,
        [common::TYPOS_LINES - per_call, per_call],
        "{chats:?}"
    );
    // Each reply capped at the last quarter of the context, in Ollama's words.
    let caps: Vec<Option<u64>> = chats
        .iter()
        .map(|c| c.json()["options"]["num_predict"].as_u64())
        .collect();
    assert!(caps.iter().all(|&c| c == Some(700 / 4)), "{caps:?}");
    assert!(fake.completions().is_empty(), "{:?}", fake.requests());
    assert_eq!(env.newest_run().findings.len(), 2);
}

#[test]
fn a_run_made_through_ollama_takes_more_files_through_its_api() {
    let env = Env::new();
    let fake = common::FakeServer::ollama("gemma4:latest", 2048);
    let host = [("OLLAMA_HOST", ollama_host(&fake))];
    let out = env.scan_preset("ollama:gemma4", &[&env.typos()], &host);
    assert!(out.status.success(), "{}", stderr(&out));
    let run = newest(&env).name;
    let chats = fake.chats().len();
    let out = env.scan_plain(&["--run", &run, &env.typos()], &host);
    assert!(out.status.success(), "{}", stderr(&out));
    let scan = newest(&env);
    assert_eq!(
        (scan.name, scan.header.backend.as_str()),
        (run, "ollama:gemma4")
    );
    assert!(fake.chats().len() > chats, "{:?}", fake.requests());
    assert!(fake.completions().is_empty(), "{:?}", fake.requests());
}
