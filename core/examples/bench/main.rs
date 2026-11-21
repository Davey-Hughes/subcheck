//! The LLM benchmark: scans bench.mkv, an invented episode with typos planted in it and lines that
//! only look wrong, through each LLM named, and scores each LLM's findings against the episode's
//! answer key (answers.toml), a row of the table each.
//!
//! ```text
//! cargo run -p subcheck-core --example bench -- [-v] <llm>...
//! ```
//!
//! An LLM is named as `subcheck scan --llm` takes it: a preset, or an entry in the settings file.
//! Warnings, and the lines naming a wait that lasts (a key command, a server's check, Ollama
//! loading a model), go to stderr. `-v` lists, after the table, each LLM's misses, its other fixes,
//! its false alarms and its other findings under its row's number, and shows on stderr what the
//! scans say of each file as they go.
//!
//! Each LLM's scan goes into a store of its own in a scratch directory, deleted when the bench
//! ends, by Ctrl+C too. A second Ctrl+C can end the bench while it deletes, and leaves what
//! remains in `$TMPDIR` or `/tmp`: a `.tmp…` directory, its name ending in `.removing-<pid>` when
//! a Ctrl+C began the deletion. The bench never opens the store `subcheck` keeps, so it leaves no
//! runs there.

mod score;
#[cfg(test)]
mod script;

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use subcheck_core::group;
use subcheck_core::llm::{self, Backend, Spent, WaitLines, Waits};
use subcheck_core::scan::{self, DEFAULT_LANG, Event, Options, Planned};
use subcheck_core::settings::Settings;
use subcheck_core::store::Store;

use score::{Key, Score, score};

const MKV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/bench.mkv");
const ANSWERS: &str = include_str!("answers.toml");
const USAGE: &str = "usage: bench [-v] <llm>...";

#[derive(Debug)]
struct Args {
    verbose: bool,
    llms: Vec<String>,
}

/// `-v` (or `--verbose`) anywhere, and the LLMs, at least one.
fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut verbose = false;
    let mut llms = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-v" | "--verbose" => verbose = true,
            flag if flag.starts_with('-') => return Err(USAGE.into()),
            llm => llms.push(llm.to_string()),
        }
    }
    if llms.is_empty() {
        return Err(USAGE.into());
    }
    Ok(Args { verbose, llms })
}

/// One LLM's row of the table.
#[derive(Debug)]
struct Row {
    /// The label its run recorded, else, when it failed before one, the name as given.
    llm: String,
    /// 0 when it never got that far.
    jobs: usize,
    /// 0 when it never got that far.
    lines_per_call: usize,
    outcome: Result<Measured, String>,
}

#[derive(Debug)]
struct Measured {
    score: Score,
    /// The scan from start to end. The server check, and the load subcheck has Ollama make, come
    /// before it, as they are not the LLM's speed; a server that loads its model on the first call
    /// counts that load here.
    time: Duration,
    spent: Spent,
}

/// Each of `llms` scanning bench.mkv, as `subcheck scan` scans, into a store of its own,
/// `scratch/row-<n>.db`, so no run of one is another's cache; its findings scored against `key`.
/// Warnings go to `warn`, each wait is reported through `waits`, and what the scans say of each
/// file goes to `log`. An LLM that fails gets its row saying why, and the rest still run.
fn bench(
    llms: &[String],
    settings: &Settings,
    key: &Key,
    scratch: &Path,
    warn: &mut dyn Write,
    waits: Waits<'_>,
    log: &mut dyn Write,
) -> Vec<Row> {
    let mut rows = Vec::new();
    for (n, name) in (1..).zip(llms) {
        let mut row = Row {
            llm: name.clone(),
            jobs: 0,
            lines_per_call: 0,
            outcome: Err(String::new()),
        };
        let store = scratch.join(format!("row-{n}.db"));
        row.outcome = measure(&mut row, settings, key, &store, warn, waits, log)
            .map_err(|e| format!("{e:#}"));
        rows.push(row);
    }
    rows
}

/// The LLM `row` names scanning bench.mkv into the store at `store`, in the order cli's `scan`
/// takes each step, minus the screen. The row's llm, jobs and lines/call are filled in as each is
/// learnt, so a row that fails says how far it got. Where `warn`, `waits` and `log` go is
/// [`bench`]'s.
fn measure(
    row: &mut Row,
    settings: &Settings,
    key: &Key,
    store: &Path,
    warn: &mut dyn Write,
    waits: Waits<'_>,
    log: &mut dyn Write,
) -> anyhow::Result<Measured> {
    let backend = Backend::named(&row.llm, settings).map_err(anyhow::Error::msg)?;
    row.jobs = backend.jobs;
    let (backend, warnings) = backend.ready(waits).map_err(anyhow::Error::msg)?;
    for w in &warnings {
        writeln!(warn, "warning: {w}")?;
    }
    row.llm.clone_from(&backend.label);
    let mut opts = Options {
        chunk_lines: None,
        jobs: backend.jobs,
        backend,
        langs: vec![DEFAULT_LANG.to_string()],
        force: true,
        verbose: false,
        into: None,
    };
    let mut store = Store::open_at(store)?;
    // The plan writes warnings alone.
    let plan = match scan::plan(&[PathBuf::from(MKV)], &opts, &mut store, warn)? {
        Planned::Scan(plan) => plan,
        Planned::Nothing(s) => bail!("{}", scan::summary_lines(&s, false).join(" ")),
    };
    opts.backend = opts.backend.sized(waits).map_err(anyhow::Error::msg)?;
    row.lines_per_call = opts.lines_per_chunk();
    if let Some(w) = opts.size_warning() {
        writeln!(warn, "warning: {w}")?;
    }
    // A failed file's error is in its event alone; the summary only counts it.
    let failure = Mutex::new(None);
    let report = |event: Event| match event {
        Event::Finished {
            error: Some(why), ..
        } => {
            failure.lock().unwrap().get_or_insert(why);
        }
        Event::Waiting(wait) => waits(wait),
        _ => {}
    };
    let started = Instant::now();
    let summary = scan::execute(plan, &opts, &mut store, log, &report)?;
    let time = started.elapsed();
    if summary.failed > 0 {
        let why = failure.lock().unwrap().take();
        bail!(why.unwrap_or_else(|| "bench.mkv failed".into()));
    }
    let run = store
        .scan_named(&summary.run)?
        .with_context(|| format!("no run named {}", summary.run))?;
    let findings = store.run_view(&run)?.findings;
    Ok(Measured {
        score: score(key, &findings),
        time,
        spent: summary.spent,
    })
}

const COLUMNS: [&str; 11] = [
    "llm",
    "jobs",
    "lines/call",
    "right",
    "other fix",
    "missed",
    "false alarms",
    "other",
    "time",
    "calls",
    "tokens",
];

/// The table of `rows`, under a line saying what `key` holds; with `verbose`, each measured
/// row's misses, other fixes, false alarms and other findings after it, under the row's number,
/// as an LLM named more than once has rows of one name.
fn render(rows: &[Row], key: &Key, verbose: bool) -> String {
    let mut out = format!(
        "bench.mkv: {} planted typos, {} traps\n\n",
        key.typos.len(),
        key.traps.len()
    );
    let header = (COLUMNS.map(String::from).to_vec(), None);
    let lines: Vec<(Vec<String>, Option<&String>)> = std::iter::once(header)
        .chain(rows.iter().map(cells))
        .collect();
    let mut widths = [0; COLUMNS.len()];
    for (cells, _) in &lines {
        for (width, cell) in widths.iter_mut().zip(cells) {
            *width = (*width).max(cell.chars().count());
        }
    }
    for (cells, failed) in &lines {
        // The LLM's name to the left, and every count and amount to the right, as figures align.
        let padded: Vec<String> = cells
            .iter()
            .zip(widths)
            .enumerate()
            .map(|(i, (cell, width))| match i {
                0 => format!("{cell:<width$}"),
                _ => format!("{cell:>width$}"),
            })
            .collect();
        out.push_str(&padded.join("  "));
        if let Some(why) = failed {
            let _ = write!(out, "  failed: {}", one_line(why));
        }
        out.push('\n');
    }
    if verbose {
        for (n, row) in (1..).zip(rows) {
            let Ok(measured) = &row.outcome else {
                continue;
            };
            let s = &measured.score;
            let _ = writeln!(out, "\n{n}. {}", one_line(&row.llm));
            for typo in &s.missed {
                let _ = writeln!(out, "  missed [{}] {}", typo.kind, typo.line);
            }
            for (typo, f) in &s.other_fix {
                let (kind, line) = (&typo.kind, &typo.line);
                let (fix, why) = (one_line(&f.suggestion), one_line(&f.reason));
                let _ = writeln!(out, "  other fix [{kind}] {line} -> {fix} ({why})");
            }
            for (trap, f) in &s.false_alarms {
                let (kind, line) = (&trap.kind, &trap.line);
                let (fix, why) = (one_line(&f.suggestion), one_line(&f.reason));
                let _ = writeln!(out, "  false alarm [{kind}] {line} -> {fix} ({why})");
            }
            for f in &s.other {
                let line = one_line(&f.line);
                let (fix, why) = (one_line(&f.suggestion), one_line(&f.reason));
                let _ = writeln!(out, "  other {line} -> {fix} ({why})");
            }
        }
    }
    out
}

/// What can break a line where the table is read, in a terminal or an editor: a line feed, a
/// carriage return, which has the rest of the line written over its start, a vertical tab, a form
/// feed, and Unicode's next line and line and paragraph separators.
const LINE_BREAKS: [char; 7] = [
    '\n', '\r', '\u{0b}', '\u{0c}', '\u{85}', '\u{2028}', '\u{2029}',
];

/// `text` on one line, each line break in it, `\r\n` counting as one, a `; `, so a field holding
/// one cannot split the line it is printed on. All else stays as it is, spaces too: they can be
/// what the LLM changed.
fn one_line(text: &str) -> String {
    text.replace("\r\n", "\n")
        .split(LINE_BREAKS)
        .collect::<Vec<_>>()
        .join("; ")
}

/// `table` written to `out` and flushed. A closed pipe is no error: whoever closed it has read
/// all they wanted.
fn write_table(out: &mut dyn Write, table: &str) -> io::Result<()> {
    match out.write_all(table.as_bytes()).and_then(|()| out.flush()) {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        written => written,
    }
}

/// A row's cells, each column's; a failed row has the first three, and why it failed.
fn cells(row: &Row) -> (Vec<String>, Option<&String>) {
    let mut cells = vec![
        // A server names its model, and the name may hold a line break.
        one_line(&row.llm),
        row.jobs.to_string(),
        row.lines_per_call.to_string(),
    ];
    let measured = match &row.outcome {
        Ok(measured) => measured,
        Err(why) => return (cells, Some(why)),
    };
    let s = &measured.score;
    let counts = [
        s.right.len(),
        s.other_fix.len(),
        s.missed.len(),
        s.false_alarms.len(),
        s.other.len(),
    ];
    cells.extend(counts.map(|n| n.to_string()));
    cells.push(time_cell(measured.time));
    cells.push(measured.spent.calls.to_string());
    // The header already says tokens.
    let tokens = measured.spent.tokens_part().map_or_else(
        || "-".into(),
        |part| part.trim_end_matches(" tokens").to_string(),
    );
    cells.push(tokens);
    (cells, None)
}

/// `41.2s` under a minute, `1m 07s` from one. Cut, not rounded, so 59.99 seconds is never
/// `60.0s`.
fn time_cell(time: Duration) -> String {
    let tenths = time.as_millis() / 100;
    if tenths < 600 {
        return format!("{}.{}s", tenths / 10, tenths % 10);
    }
    let secs = time.as_secs();
    format!("{}m {:02}s", secs / 60, secs % 60)
}

fn main() -> ExitCode {
    let code = run();
    // A signal on the way out still ends the bench as a signal: a return here would end it first,
    // with a status of its own.
    group::finish();
    code
}

/// The bench of the LLMs the command line names, to its exit status.
fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&args) {
        Ok(args) => args,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    let ready = Settings::load().and_then(|settings| {
        let key = Key::parse(ANSWERS)?;
        // Registered as it is made, so that a signal that ends the bench removes it; and the
        // signals are taken from here on, where a bench of servers alone starts no group to take
        // them.
        let scratch = group::create_removing_dir(tempfile::tempdir)
            .map_err(|e| format!("a scratch directory: {e}"))?;
        Ok((settings, key, scratch))
    });
    let (settings, key, (scratch, removing)) = match ready {
        Ok(ready) => ready,
        Err(e) => {
            eprintln!("bench: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut log: Box<dyn Write> = if args.verbose {
        Box::new(io::stderr())
    } else {
        Box::new(io::sink())
    };
    let mut warn = io::stderr();
    let wait_lines = WaitLines::new(Box::new(io::stderr()), llm::HOLD);
    let rows = bench(
        &args.llms,
        &settings,
        &key,
        scratch.path(),
        &mut warn,
        &|wait| wait_lines.say(wait),
        &mut log,
    );
    // Removed and forgotten with no signal between: one that came after the removal alone would
    // remove whatever had taken the directory's name since.
    removing.forget_after(|| drop(scratch));
    // A closed stdout, as `| head` leaves, ends the table and not the bench: the exit status still
    // says whether every LLM ran.
    if let Err(e) = write_table(&mut io::stdout().lock(), &render(&rows, &key, args.verbose)) {
        eprintln!("bench: writing the table: {e}");
        return ExitCode::FAILURE;
    }
    if rows.iter().any(|row| row.outcome.is_err()) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use subcheck_core::test_support::{
        FakeServer, Reply, Request, chat_reply, closed_port_url, models_reply,
    };

    use super::*;
    use crate::score::Flag;
    use crate::script::dialogue_texts;

    const SCRIPT: &str = include_str!("script.ass");
    /// What the fake LLMs give as each finding's reason.
    const REASON: &str = "made up";

    fn key() -> Key {
        Key::parse(ANSWERS).unwrap()
    }

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    /// The first line of the script that the key does not name and that holds no markup: a line
    /// the LLM that knows the answers changes all the same, as an LLM flags a line with nothing
    /// wrong in it.
    fn plain(key: &Key) -> String {
        let named = |text: &str| {
            key.typos.iter().any(|t| t.line == text) || key.traps.iter().any(|t| t.line == text)
        };
        dialogue_texts(SCRIPT)
            .into_iter()
            .find(|text| !named(text) && !text.contains(['{', '\\']))
            .unwrap()
    }

    /// A prompt's subtitle line, `L<id> [<time>] (<style>) <text>`, as its ID and its text.
    fn subtitle(line: &str) -> Option<(usize, &str)> {
        let (id, rest) = line.strip_prefix('L')?.split_once(" [")?;
        let (_, text) = rest.split_once(") ")?;
        Some((id.parse().ok()?, text))
    }

    /// The reply of an LLM that knows the answers to the prompt `r` sends: each typo's first fix,
    /// but the key's first typo's with ` Really.` added, a fix of another kind; each trap, and
    /// `plain`, with `!` added; NONE when the prompt holds none of them.
    fn knowing(r: &Request, key: &Key, plain: &str) -> Reply {
        let body = r.json();
        let prompt = body["messages"]
            .as_array()
            .and_then(|m| m.last())
            .and_then(|m| m["content"].as_str())
            .unwrap();
        let fix = |text: &str| {
            if let Some(typo) = key.typos.iter().find(|t| t.line == text) {
                let mut fix = typo.fixed[0].clone();
                if *typo == key.typos[0] {
                    fix.push_str(" Really.");
                }
                Some(fix)
            } else if text == plain || key.traps.iter().any(|t| t.line == text) {
                Some(format!("{text}!"))
            } else {
                None
            }
        };
        let found: Vec<String> = prompt
            .lines()
            .filter_map(subtitle)
            .filter_map(|(id, text)| Some(format!("L{id}\t{}\t{REASON}", fix(text)?)))
            .collect();
        if found.is_empty() {
            chat_reply("NONE")
        } else {
            chat_reply(&found.join("\n"))
        }
    }

    /// A server that lists `m` and answers each chat completion with `chat`, and anything else,
    /// Ollama's `/api/ps` among them, with a 404: an empty list of loaded models there would read
    /// as a model not loaded yet, and have the scan load it.
    fn serving(chat: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> String {
        let server = FakeServer::start(move |r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/v1/models") => models_reply(&["m"]),
            ("POST", "/v1/chat/completions") => chat(r),
            _ => Reply::status(404, "{}"),
        });
        server.url
    }

    /// Settings naming `knows`, the LLM that knows the answers; `blank`, which finds nothing;
    /// `down`, with nothing listening; and `refuses`, which fails every chat.
    fn settings() -> Settings {
        let key = key();
        let plain = plain(&key);
        let knows = serving(move |r| knowing(r, &key, &plain));
        let blank = serving(|_| chat_reply("NONE"));
        // Words that name no context, so the scan does not take the chunk for one too big and
        // split it.
        let refuses = serving(|_| Reply::status(500, r#"{"error":{"message":"boom"}}"#));
        let text = format!(
            "[llm.knows]\nurl = \"{knows}\"\n\n[llm.blank]\nurl = \"{blank}\"\n\n\
             [llm.down]\nurl = \"{}\"\n\n[llm.refuses]\nurl = \"{refuses}\"\n",
            closed_port_url()
        );
        Settings::parse(Path::new("/bench/config.toml"), &text).unwrap()
    }

    /// The bench of `knows` twice, then `blank`, in `scratch`.
    fn scored(scratch: &Path) -> Vec<Row> {
        let llms = words(&["knows", "knows", "blank"]);
        bench(
            &llms,
            &settings(),
            &key(),
            scratch,
            &mut io::sink(),
            &|_| {},
            &mut io::sink(),
        )
    }

    #[test]
    fn each_llm_is_scored_in_a_store_of_its_own() {
        let key = key();
        let scratch = tempfile::tempdir().unwrap();
        let rows = scored(scratch.path());
        assert_eq!(rows.len(), 3);
        for row in &rows[..2] {
            let measured = row.outcome.as_ref().unwrap();
            let s = &measured.score;
            assert_eq!(
                (
                    s.right.len(),
                    s.other_fix.len(),
                    s.missed.len(),
                    s.false_alarms.len(),
                    s.other.len()
                ),
                (key.typos.len() - 1, 1, 0, key.traps.len(), 1)
            );
            assert_eq!(
                (row.llm.as_str(), row.jobs, row.lines_per_call),
                ("knows@m", 2, 400)
            );
            assert!(measured.spent.calls >= 1, "{:?}", measured.spent);
        }
        let blank = &rows[2].outcome.as_ref().unwrap().score;
        assert_eq!(blank.missed.len(), key.typos.len());
        assert_eq!(
            (
                blank.right.len(),
                blank.other_fix.len(),
                blank.false_alarms.len(),
                blank.other.len()
            ),
            (0, 0, 0, 0)
        );
        let mut stores: Vec<String> = std::fs::read_dir(scratch.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|e| e == "db"))
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        stores.sort();
        assert_eq!(stores, ["row-1.db", "row-2.db", "row-3.db"]);
    }

    /// The bench of `down`, `refuses` and `knows`, in that order.
    fn with_failures() -> Vec<Row> {
        let scratch = tempfile::tempdir().unwrap();
        let llms = words(&["down", "refuses", "knows"]);
        bench(
            &llms,
            &settings(),
            &key(),
            scratch.path(),
            &mut io::sink(),
            &|_| {},
            &mut io::sink(),
        )
    }

    /// Without `-v` the scans' own log is a sink, and a warning must not go with it.
    #[test]
    fn warnings_go_to_warn_and_what_the_scan_says_of_each_file_to_log() {
        let scratch = tempfile::tempdir().unwrap();
        let (mut warn, mut log) = (Vec::new(), Vec::new());
        let rows = bench(
            &words(&["blank"]),
            &settings(),
            &key(),
            scratch.path(),
            &mut warn,
            &|_| {},
            &mut log,
        );
        assert!(rows[0].outcome.is_ok(), "{:?}", rows[0].outcome);
        let (warn, log) = (
            String::from_utf8(warn).unwrap(),
            String::from_utf8(log).unwrap(),
        );
        assert!(
            warn.contains("warning: blank did not say its context size"),
            "{warn}"
        );
        assert!(!warn.contains("bench.mkv"), "{warn}");
        assert!(log.contains("bench.mkv"), "{log}");
        assert!(!log.contains("warning:"), "{log}");
    }

    #[test]
    fn an_llm_that_fails_gets_its_row_and_the_others_still_run() {
        let rows = with_failures();
        assert_eq!(rows.len(), 3);
        let down = rows[0].outcome.as_ref().unwrap_err();
        assert!(down.contains("nothing answering"), "{down}");
        assert_eq!(rows[0].llm, "down");
        let refused = rows[1].outcome.as_ref().unwrap_err();
        assert!(refused.contains("HTTP 500"), "{refused}");
        assert!(rows[2].outcome.is_ok(), "{:?}", rows[2].outcome);
    }

    /// Whether `line` holds each of `words`, in order.
    fn in_order(line: &str, words: &[&str]) -> bool {
        let mut rest = line;
        words.iter().all(|word| match rest.find(word) {
            Some(at) => {
                rest = &rest[at + word.len()..];
                true
            }
            None => false,
        })
    }

    #[test]
    fn the_table_names_each_column_and_a_failed_row_says_why() {
        let key = key();
        let rows = with_failures();
        let table = render(&rows, &key, false);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(
            lines[0],
            format!(
                "bench.mkv: {} planted typos, {} traps",
                key.typos.len(),
                key.traps.len()
            )
        );
        let header = [
            "llm",
            "jobs",
            "lines/call",
            "right",
            "other fix",
            "missed",
            "false alarms",
            "other",
            "time",
            "calls",
            "tokens",
        ];
        assert!(lines.iter().any(|l| in_order(l, &header)), "{table}");
        let down = lines.iter().find(|l| l.starts_with("down")).unwrap();
        assert!(down.contains("failed: "), "{down}");
        let knows = lines.iter().find(|l| l.starts_with("knows")).unwrap();
        assert!(knows.ends_with(" out"), "{knows}");
        assert!(!knows.contains("tokens"), "{knows}");
        // No line of the details a verbose table adds.
        assert!(!lines.iter().any(|l| l.starts_with("  ")), "{table}");

        let verbose = render(&rows, &key, true);
        let lines: Vec<&str> = verbose.lines().collect();
        let first = &key.typos[0];
        let other_fix = format!(
            "  other fix [{}] {} -> {} Really. ({REASON})",
            first.kind, first.line, first.fixed[0]
        );
        assert!(lines.contains(&other_fix.as_str()), "{verbose}");
        let plain = plain(&key);
        let other = format!("  other {plain} -> {plain}! ({REASON})");
        assert!(lines.contains(&other.as_str()), "{verbose}");
        // A failed row has no details, and the rest keep their row's number.
        assert!(lines.contains(&"3. knows@m"), "{verbose}");
        assert!(!lines.iter().any(|l| l.starts_with("1. ")), "{verbose}");
    }

    #[test]
    fn verbose_lists_misses_and_false_alarms_under_each_rows_number() {
        let key = key();
        let scratch = tempfile::tempdir().unwrap();
        let verbose = render(&scored(scratch.path()), &key, true);
        let lines: Vec<&str> = verbose.lines().collect();
        let under = |heading: &str| {
            let at = lines.iter().position(|l| *l == heading).unwrap();
            lines[at + 1..]
                .iter()
                .take_while(|l| l.starts_with("  "))
                .copied()
                .collect::<Vec<&str>>()
        };
        let trap = &key.traps[0];
        let false_alarm = format!(
            "  false alarm [{}] {} -> {}! ({REASON})",
            trap.kind, trap.line, trap.line
        );
        let typo = &key.typos[1];
        let missed = format!("  missed [{}] {}", typo.kind, typo.line);
        for heading in ["1. knows@m", "2. knows@m"] {
            let details = under(heading);
            assert!(details.contains(&false_alarm.as_str()), "{verbose}");
            assert!(
                !details.iter().any(|l| l.starts_with("  missed ")),
                "{verbose}"
            );
        }
        let blank = under("3. blank@m");
        assert!(blank.contains(&missed.as_str()), "{verbose}");
        assert_eq!(blank.len(), key.typos.len(), "{verbose}");
    }

    /// Two spaces in fields, and a space before a break, as an LLM's fix can be of spacing; and
    /// each kind of line break.
    #[test]
    fn a_line_break_in_a_field_never_splits_its_line() {
        let key = key();
        let (typo, trap) = (key.typos[0].clone(), key.traps[0].clone());
        let flag = Flag {
            line: "seen  once\nand again".into(),
            suggestion: "a  fix\u{2028}in two".into(),
            reason: "made  up \u{2029}there".into(),
        };
        let fixed = Flag {
            line: typo.line.clone(),
            suggestion: "b  fix\u{0b}in two".into(),
            reason: "made\u{0c}up\u{85}here".into(),
        };
        let score = Score {
            right: Vec::new(),
            other_fix: vec![(typo.clone(), fixed)],
            missed: Vec::new(),
            false_alarms: vec![(trap.clone(), flag.clone())],
            other: vec![flag],
        };
        let measured = Measured {
            score,
            time: Duration::ZERO,
            spent: Spent::default(),
        };
        let rows = [
            Row {
                llm: "failed".into(),
                jobs: 0,
                lines_per_call: 0,
                outcome: Err("first  half\r\nsecond\rthird".into()),
            },
            Row {
                llm: "scored\nm".into(),
                jobs: 1,
                lines_per_call: 1,
                outcome: Ok(measured),
            },
        ];
        let table = render(&rows, &key, true);
        let lines: Vec<&str> = table.lines().collect();
        // The llm column is as wide as `scored; m`, the folded label, and no wider.
        assert!(lines[2].starts_with("llm        jobs"), "{table}");
        assert!(lines[4].starts_with("scored; m  "), "{table}");
        assert!(lines.contains(&"2. scored; m"), "{table}");
        let failed = lines.iter().find(|l| l.starts_with("failed")).unwrap();
        assert!(
            failed.ends_with("  failed: first  half; second; third"),
            "{table}"
        );
        let other_fix = format!(
            "  other fix [{}] {} -> b  fix; in two (made; up; here)",
            typo.kind, typo.line
        );
        assert!(lines.contains(&other_fix.as_str()), "{table}");
        let false_alarm = format!(
            "  false alarm [{}] {} -> a  fix; in two (made  up ; there)",
            trap.kind, trap.line
        );
        assert!(lines.contains(&false_alarm.as_str()), "{table}");
        let other = "  other seen  once; and again -> a  fix; in two (made  up ; there)";
        assert!(lines.contains(&other), "{table}");
    }

    /// A writer whose every write fails with its kind of error.
    struct Failing(io::ErrorKind);

    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(self.0.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_closed_pipe_ends_the_table_and_any_other_failure_is_an_error() {
        assert!(write_table(&mut Failing(io::ErrorKind::BrokenPipe), "table\n").is_ok());
        let full = write_table(&mut Failing(io::ErrorKind::StorageFull), "table\n").unwrap_err();
        assert_eq!(full.kind(), io::ErrorKind::StorageFull);
        let mut out = Vec::new();
        write_table(&mut out, "table\n").unwrap();
        assert_eq!(out, b"table\n");
    }

    #[test]
    fn args_are_flags_then_llms() {
        let args = |given: &[&str]| parse_args(&words(given));
        let both = args(&["-v", "a", "b"]).unwrap();
        assert!(both.verbose);
        assert_eq!(both.llms, ["a", "b"]);
        assert!(!args(&["a"]).unwrap().verbose);
        for wrong in [&[][..], &["-x", "a"]] {
            let err = args(wrong).unwrap_err();
            assert!(err.starts_with("usage:"), "{err}");
        }
    }

    #[test]
    fn time_is_tenths_of_a_second_under_a_minute_then_minutes_and_seconds() {
        assert_eq!(time_cell(Duration::from_millis(41_250)), "41.2s");
        assert_eq!(time_cell(Duration::from_millis(59_990)), "59.9s");
        assert_eq!(time_cell(Duration::from_secs(67)), "1m 07s");
    }
}
