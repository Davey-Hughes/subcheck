//! `subcheck scan`: find MKVs, read their text subtitle tracks, send the lines to the LLM in
//! chunks, and record every suggested fix in the store.
//!
//! One reader thread reads files in order (the array's disks are the bottleneck, so never two at
//! once), `-j` workers run the LLM, and the calling thread collects: it saves a file, in one
//! transaction, the moment every one of its lines has an answer, and each LLM call's usage as it
//! comes.

use std::collections::HashMap;
use std::io::{self, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use jiff::Timestamp;

use crate::blocks::{self, ReadPath};
use crate::list::shell_quote;
use crate::llm::{self, Answer, Backend, Spent, Suggestion, TooBig, Usage, Used, Via};
use crate::mkv::{self, Layout, TRACK_TYPE_SUBTITLE, Track, Truncated};
use crate::settings::Settings;
use crate::store::{
    self, Attempt, Check, CheckedWith, FileRecord, Finding, LlmCall, Made, Record, RunHeader,
    Saved, ScanRow, Store, TrackRecord,
};
use crate::subs::Kind;
use crate::text::{self, Line};
use crate::timing::{self, record, shared};

/// The subtitle languages a scan checks unless told otherwise.
pub const DEFAULT_LANG: &str = "eng";

pub struct Options {
    pub backend: Backend,
    /// `--chunk-lines`: the lines in a chunk, at most as many as fit the context the server
    /// reports, with no limit of 400, and exactly these with no size known
    /// ([`Options::lines_per_chunk`]). `None` when it is not given, and for a recheck, so the
    /// entry's `chunk_lines` or the backend sizes the chunks.
    pub chunk_lines: Option<usize>,
    pub jobs: usize,
    /// ISO 639-2 codes; a track matches by `Language` or by its BCP 47 primary subtag.
    pub langs: Vec<String>,
    pub force: bool,
    pub verbose: bool,
    /// The run to add to (`--run`), by name, instead of starting a run.
    pub into: Option<String>,
}

impl Options {
    /// Lines per chunk: `--chunk-lines`, else the entry's `chunk_lines`, cut to what fits, else
    /// fitted, for [`Options::jobs`] calls at once ([`Backend::lines_per_chunk`]).
    pub fn lines_per_chunk(&self) -> usize {
        self.backend.lines_per_chunk(self.jobs, self.chunk_lines)
    }

    /// The backend's warning of its context: that it did not say its size
    /// ([`Backend::size_warning`]), unless `--chunk-lines` sets the size; or that the entry's own
    /// cap on its replies leaves a call no room for a line ([`Backend::room_warning`]), which no
    /// chunk size changes.
    pub fn size_warning(&self) -> Option<String> {
        let unknown = match self.chunk_lines {
            Some(_) => None,
            None => self.backend.size_warning(self.jobs),
        };
        unknown.or_else(|| self.backend.room_warning(self.jobs))
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Summary {
    pub run: String,
    pub files: usize,
    pub checked: usize,
    pub failed: usize,
    pub cached: usize,
    pub findings: usize,
    /// Of the findings, those of the timing check.
    pub timing: usize,
    /// What this scan's LLM calls used.
    pub spent: Spent,
    /// The paths the scan was given, absolute: what `subcheck review` takes to show their files.
    pub paths: Vec<PathBuf>,
}

/// What a scan tells the hook [`execute`] is given, as it goes: enough for review's screen to show
/// each file's progress, and to take each record as the store shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The files to check, in the order they are read: once, first.
    Queued(Vec<PathBuf>),
    /// The reader has started on a file.
    Reading(PathBuf),
    /// Of the LLM chunks that answer a file's lines, how many have answered: said once the file is
    /// read, then as each answers. A line already sent for an earlier file counts that file's
    /// chunk. Not said of a file with no lines for the LLM.
    Asking {
        path: PathBuf,
        answered: usize,
        chunks: usize,
    },
    /// Records just saved, in order: an LLM call's usage, or a file's record (of the checks this
    /// scan made, or of its failure), its findings, and the decisions carried over to them.
    Appended(Vec<Record>),
    /// A file done, after its records: why it failed, if it did; its findings; and the lines just
    /// written to the log for it.
    Finished {
        path: PathBuf,
        error: Option<String>,
        findings: usize,
        lines: Vec<String>,
    },
}

/// What [`plan`] found.
pub enum Planned {
    /// Nothing to check; the summary says why (no files, every one unchanged since an earlier
    /// scan, or with `--run`, nothing the run needs).
    Nothing(Summary),
    Scan(Plan),
}

/// A scan ready to run: its run made (or, with `--run`, found), with every file it names.
pub struct Plan {
    start: Start,
    summary: Summary,
}

impl Plan {
    /// The run the scan records under.
    pub fn scan(&self) -> &ScanRow {
        &self.start.scan
    }

    /// The files the scan checks, in the order it reads them, each with the checks it makes.
    pub fn todo(&self) -> &[(PathBuf, Vec<Check>)] {
        &self.start.todo
    }

    /// Whether some file the scan checks needs the LLM's check.
    pub fn needs_llm(&self) -> bool {
        needs_llm(&self.start.todo)
    }
}

/// `paths` expanded to MKV files: files as given, directories walked recursively for `*.mkv`
/// (any case), skipping hidden entries (`.Recycle.Bin`, `.Trash-1000`, ...). Sorted, absolute.
/// Also returns a warning for each directory or entry that could not be read: it is skipped, so
/// one locked directory cannot stop a scan of a whole share.
pub fn mkv_files(paths: &[PathBuf]) -> io::Result<(Vec<PathBuf>, Vec<String>)> {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for p in paths {
        let p = crate::store::absolute(p)?;
        if p.is_dir() {
            walk(&p, &mut out, &mut warnings);
        } else if p.is_file() {
            out.push(p);
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{}: no such file or directory", p.display()),
            ));
        }
    }
    out.sort();
    out.dedup();
    Ok((out, warnings))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, warnings: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            warnings.push(format!("{}: skipped: {e}", dir.display()));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                warnings.push(format!("{}: skipped an entry: {e}", dir.display()));
                continue;
            }
        };
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => walk(&path, out, warnings),
            Ok(_) => {
                if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("mkv"))
                {
                    out.push(path);
                }
            }
            Err(e) => warnings.push(format!("{}: skipped: {e}", path.display())),
        }
    }
}

/// Whether a track is in one of `langs`. Matroska's rule: LanguageBCP47, when present, overrides
/// Language, and an absent Language means "eng", so a track tagged only `ja` in BCP 47 is not
/// English.
pub fn wanted_language(track: &Track, langs: &[String]) -> bool {
    langs
        .iter()
        .any(|want| match track.language_bcp47.as_deref() {
            Some(tag) => {
                let primary = tag.split('-').next().unwrap_or(tag);
                primary.eq_ignore_ascii_case(want)
                    || crate::iso639::alpha_2(want)
                        .is_some_and(|code| primary.eq_ignore_ascii_case(code))
            }
            None => track.language == *want,
        })
}

/// A file read and turned into lines, or the reason it could not be.
struct Prepared {
    path: PathBuf,
    size: u64,
    mtime: Timestamp,
    /// The checks made of it.
    checks: Vec<Check>,
    tracks: Vec<TrackRecord>,
    /// For the LLM; empty when it does not check the file.
    lines: Vec<Line>,
    /// The timing check's findings, numbered 0, and its notes (always shown).
    timing: Vec<Finding>,
    timing_notes: Vec<String>,
    error: Option<String>,
    /// The file is shorter than its Segment says. Its check is never fresh, so every scan says so
    /// until the copy is replaced; nor is one with a track that could not be read.
    truncated: Option<Truncated>,
    /// Shown with `-v`.
    notes: Vec<String>,
}

/// A file's size and modification time: what a check's freshness and review compare.
pub fn file_stamp(path: &Path) -> io::Result<(u64, Timestamp)> {
    let meta = std::fs::metadata(path)?;
    let mtime = Timestamp::try_from(meta.modified()?).map_err(io::Error::other)?;
    Ok((meta.len(), mtime))
}

fn prepare(path: &Path, langs: &[String], checks: &[Check]) -> Prepared {
    let mut p = Prepared {
        path: path.to_path_buf(),
        size: 0,
        mtime: Timestamp::UNIX_EPOCH,
        checks: checks.to_vec(),
        tracks: Vec::new(),
        lines: Vec::new(),
        timing: Vec::new(),
        timing_notes: Vec::new(),
        error: None,
        truncated: None,
        notes: Vec::new(),
    };
    match file_stamp(path) {
        Ok((size, mtime)) => (p.size, p.mtime) = (size, mtime),
        Err(e) => {
            p.error = Some(e.to_string());
            return p;
        }
    }
    let (r, layout) = match mkv::open(path) {
        Ok(x) => x,
        Err(e) => {
            p.error = Some(e.to_string());
            return p;
        }
    };
    p.truncated = layout.truncated;
    // The wanted tracks, in file order, with mpv's sid for each.
    let mut wanted = Vec::new();
    let mut sid = 0;
    for track in &layout.tracks {
        if track.kind == TRACK_TYPE_SUBTITLE {
            sid += 1;
        }
        if let Some(kind) = Kind::of(track)
            && wanted_language(track, langs)
        {
            wanted.push((track, kind, sid));
        }
    }
    let numbers: Vec<u64> = wanted.iter().map(|(t, _, _)| t.number).collect();
    let before = r.bytes_read();
    let reads = blocks::read_each(&r, &layout, &numbers);
    let mut events: Vec<(usize, Vec<text::Event>)> = Vec::new();
    for ((track, kind, sid), read) in wanted.into_iter().zip(reads) {
        let named = format!("track {} {}", track.id, track.shown_name());
        let read = match read {
            Ok(t) => {
                let path = match t.path {
                    ReadPath::Cues => "cues",
                    ReadPath::Walk => "walk",
                    ReadPath::Empty => "empty",
                };
                p.notes
                    .push(format!("{named}: {} events via {path}", t.blocks.len()));
                let mine = text::events(
                    track.id,
                    kind,
                    &t.blocks,
                    layout.timestamp_scale,
                    track.default_duration,
                );
                events.push((track.id, mine));
                path.to_string()
            }
            // An unsupported encoding loses that track, not the file.
            Err(e) => {
                p.notes.push(format!("{named}: skipped: {e}"));
                format!("skipped: {e}")
            }
        };
        p.tracks.push(TrackRecord {
            id: track.id,
            name: track.name.clone(),
            language: track.language.clone(),
            language_bcp47: track.language_bcp47.clone(),
            sid,
            default: track.default,
            read,
        });
    }
    p.notes
        .push(format!("{} bytes read", r.bytes_read() - before));
    if checks.contains(&Check::Timing) && !events.is_empty() {
        match timing_findings(path, &layout, &events) {
            Ok((found, notes)) => (p.timing, p.timing_notes) = (found, notes),
            Err(reason) => p.notes.push(format!("timing: unsupported: {reason}")),
        }
    }
    if checks.contains(&Check::Llm) {
        p.lines = text::lines(events.into_iter().flat_map(|(_, e)| e).collect());
    }
    p
}

/// The timing check of one file's tracks (mkvmerge ID and events each): its findings, numbered
/// 0, and a note for each track its shared lines contradict, or whose times point two ways and that
/// follows no other track. Err: why the video cannot stand in for the cuts, which is no failure of
/// the file.
fn timing_findings(
    path: &Path,
    layout: &Layout,
    tracks: &[(usize, Vec<text::Event>)],
) -> Result<(Vec<Finding>, Vec<String>), String> {
    let video = timing::video(layout)?;
    let measured: Vec<timing::Analysis> = tracks
        .iter()
        .map(|(_, events)| timing::analyse(&video, events))
        .collect();
    let analysed: Vec<(usize, &[text::Event], &timing::Analysis)> = tracks
        .iter()
        .zip(&measured)
        .map(|((id, events), a)| (*id, events.as_slice(), a))
        .collect();
    let said = shared::lines_of(&video, &analysed);
    let views: Vec<record::Track> = analysed
        .iter()
        .zip(&said)
        .map(|(&(id, events, analysis), lines)| record::Track {
            id,
            events,
            analysis,
            lines: lines.as_ref(),
        })
        .collect();
    Ok(record::findings(&path.to_string_lossy(), &video, &views))
}

/// Lines bound for the LLM.
pub struct Chunk {
    pub id: usize,
    pub lines: Vec<Line>,
}

/// Packs lines into chunks, sending each distinct text once per run: a text already queued for
/// an earlier file points at that chunk instead of joining a new one.
pub struct Chunker {
    size: usize,
    next_id: usize,
    queued: HashMap<String, (usize, usize)>,
}

impl Chunker {
    pub fn new(size: usize) -> Chunker {
        Chunker {
            size: size.max(1),
            next_id: 0,
            queued: HashMap::new(),
        }
    }

    /// For each line, the (chunk ID, index in chunk) that will answer it, and the new chunks to
    /// send. A file's new lines never share a chunk with another file's.
    pub fn assign(&mut self, lines: &[Line]) -> (Vec<(usize, usize)>, Vec<Chunk>) {
        let mut refs = Vec::with_capacity(lines.len());
        let mut chunks: Vec<Chunk> = Vec::new();
        for line in lines {
            if let Some(&r) = self.queued.get(&line.text) {
                refs.push(r);
                continue;
            }
            if chunks.last().is_none_or(|c| c.lines.len() >= self.size) {
                chunks.push(Chunk {
                    id: self.next_id,
                    lines: Vec::new(),
                });
                self.next_id += 1;
            }
            let chunk = chunks.last_mut().expect("pushed above");
            let r = (chunk.id, chunk.lines.len());
            chunk.lines.push(line.clone());
            self.queued.insert(line.text.clone(), r);
            refs.push(r);
        }
        (refs, chunks)
    }
}

/// "1 file", "3 files".
pub fn files(n: usize) -> String {
    if n == 1 {
        "1 file".into()
    } else {
        format!("{n} files")
    }
}

/// A file to read, and the checks it needs.
type Todo = (PathBuf, Vec<Check>);

/// Whether some file in `todo` needs the LLM's check.
fn needs_llm(todo: &[Todo]) -> bool {
    todo.iter().any(|(_, checks)| checks.contains(&Check::Llm))
}

/// Each file with the checks it needs, and how many files need none: each file's current check
/// of each kind is fresh ([`Store::is_fresh`]), the LLM's made with the same backend, languages
/// and prompt, the timing check's with the same languages and rules. `--force` needs them all.
fn to_check(all: &[PathBuf], opts: &Options, store: &Store) -> anyhow::Result<(Vec<Todo>, usize)> {
    let withs = [Check::Llm, Check::Timing].map(|c| (c, checked_with(opts, c)));
    let mut cached = 0;
    let mut todo = Vec::new();
    for path in all {
        let key = path.to_string_lossy();
        let stamp = file_stamp(path).ok().filter(|_| !opts.force);
        let mut needed = Vec::new();
        for (check, with) in &withs {
            let fresh = match stamp {
                Some((size, mtime)) => store.is_fresh(&key, size, mtime, with)?,
                None => false,
            };
            if !fresh {
                needed.push(*check);
            }
        }
        if needed.is_empty() {
            cached += 1;
        } else {
            todo.push((path.clone(), needed));
        }
    }
    Ok((todo, cached))
}

fn checked_with(opts: &Options, check: Check) -> CheckedWith {
    match check {
        Check::Llm => CheckedWith::new(&opts.backend.label, &opts.langs, llm::PROMPT_VERSION),
        Check::Timing => CheckedWith::timing(&opts.langs, timing::RULES_VERSION),
    }
}

/// Print every prompt `scan` would send, and run nothing. Files a scan would skip as unchanged are
/// left out here too, unless `--force`.
pub fn dry_run(
    paths: &[PathBuf],
    opts: &Options,
    store: &Store,
    out: &mut dyn Write,
    log: &mut dyn Write,
) -> anyhow::Result<()> {
    let (all, warnings) = mkv_files(paths)?;
    for w in &warnings {
        writeln!(log, "warning: {w}")?;
    }
    let total = all.len();
    let (todo, _) = to_check(&all, opts, store)?;
    let todo: Vec<PathBuf> = todo
        .into_iter()
        .filter(|(_, checks)| checks.contains(&Check::Llm))
        .map(|(path, _)| path)
        .collect();
    let cached = total - todo.len();
    if cached > 0 {
        writeln!(
            log,
            "dry run: {} unchanged since an earlier scan, left out (--force to include)",
            files(cached)
        )?;
    }
    if let Some(w) = opts.size_warning() {
        writeln!(log, "warning: {w}")?;
    }
    // `Backend::ready` sends only GETs, so a dry run never loads a model, and one Ollama has not
    // loaded has no size until a scan loads it.
    if opts.backend.context() == Some(llm::Context::NotLoaded) {
        let size_set = opts.chunk_lines.or(opts.backend.chunk_lines).is_some();
        let (set, then) = if size_set {
            (" as set", "cuts them to what fits")
        } else {
            ("", "fits them to")
        };
        writeln!(
            log,
            "dry run: {}'s model is not loaded, so these chunks are {} lines{set}; a scan loads it \
             and {then} its context",
            opts.backend.label,
            opts.lines_per_chunk()
        )?;
    }
    let mut chunker = Chunker::new(opts.lines_per_chunk());
    for path in todo {
        let p = prepare(&path, &opts.langs, &[Check::Llm]);
        if let Some(t) = p.truncated {
            writeln!(log, "{}", t.warning(&path))?;
        }
        if let Some(e) = &p.error {
            writeln!(out, "== {}: cannot read: {e}", path.display())?;
            continue;
        }
        let (_, chunks) = chunker.assign(&p.lines);
        for c in chunks {
            writeln!(
                out,
                "== {} (chunk {}, {} lines)",
                path.display(),
                c.id,
                c.lines.len()
            )?;
            let refs: Vec<&Line> = c.lines.iter().collect();
            write!(out, "{}", llm::prompt(&refs))?;
        }
    }
    Ok(())
}

/// A chunk on its way to a worker, with the file whose lines it carries.
struct Queued {
    chunk: Chunk,
    file: Arc<str>,
}

struct ChunkDone {
    id: usize,
    result: Result<Vec<Suggestion>, String>,
    /// Each call made for it to count (see [`ask`]).
    calls: Vec<LlmCall>,
}

struct FileJob {
    prepared: Prepared,
    /// For each line, the (chunk ID, index in chunk) whose answer covers it.
    refs: Vec<(usize, usize)>,
}

enum Msg {
    File(Box<FileJob>),
    Chunk(ChunkDone),
}

/// One chunk through the LLM: its suggestions, and each call made for it to count, in the order
/// they went, with how many lines it sent and what it used when its backend said (a failed call
/// costs too).
///
/// A chunk, or a part of one, has two attempts, where a failed run and an unreadable reply both
/// count as failures. A chunk is sent whole: when it was made, it was cut to the lines whose
/// [`llm::estimate`] fits what `budget`, each call's part of the server's context
/// ([`Backend::budget`]), leaves the prompt, whatever size was set
/// ([`Backend::lines_per_chunk`]). One too big for the server all the same is halved, each half
/// asked in turn, and a half's findings keep their own lines: once the server refuses it or may
/// have cut it ([`TooBig::Prompt`]), down to single lines, which are sent whatever the estimate,
/// since only the server knows what fits. A single line it refuses is a failed attempt like any
/// other: llama-server refuses calls in flight that outgrew the cache they share, and the line can
/// fit once the others are done. Each call's reply is capped at the room `budget` leaves it
/// ([`llm::Budget::cap`]); a reply cut off there, or at the server's own length limit
/// ([`TooBig::Reply`]), halves its chunk once only: a model that loops until the limit is cut off
/// at any size, and halving it down to single lines would cost about two calls a line. A half that
/// fails fails its chunk, and the other half is not sent.
fn ask(
    backend: &Backend,
    lines: &[Line],
    budget: Option<llm::Budget>,
) -> (Result<Vec<Suggestion>, String>, Vec<Call>) {
    let mut asker = Asker {
        backend,
        budget,
        calls: Vec::new(),
    };
    let result = asker.part(lines, true);
    (result, asker.calls)
}

/// A call made for a chunk: how many lines it sent, and what it used when its backend said.
type Call = (usize, Option<Usage>);

/// One chunk's calls ([`ask`]).
struct Asker<'a> {
    backend: &'a Backend,
    budget: Option<llm::Budget>,
    /// Each call made so far, in the order they went.
    calls: Vec<Call>,
}

impl Asker<'_> {
    /// `lines`, a chunk or part of one, through the LLM; a reply cut off halves them only when
    /// `may_split_reply`.
    fn part(&mut self, lines: &[Line], may_split_reply: bool) -> Result<Vec<Suggestion>, String> {
        let several = lines.len() > 1;
        let refs: Vec<&Line> = lines.iter().collect();
        let prompt = llm::prompt(&refs);
        let mut last = String::new();
        for _ in 0..2 {
            let Answer {
                reply,
                used,
                too_big,
            } = self
                .backend
                .call(&prompt, self.backend.timeout, self.budget);
            match used {
                Used::Reported(usage) => self.calls.push((lines.len(), Some(usage))),
                Used::Unreported => self.calls.push((lines.len(), None)),
                Used::Untracked => {}
            }
            match (too_big, reply) {
                (Some(TooBig::Prompt), _) if several => {
                    return self.halves(lines, may_split_reply);
                }
                (Some(TooBig::Reply), _) if several && may_split_reply => {
                    return self.halves(lines, false);
                }
                (_, reply) => match reply.and_then(|reply| llm::parse(&reply, &refs)) {
                    Ok(parsed) => return Ok(parsed.suggestions),
                    Err(e) => last = e,
                },
            }
        }
        Err(last)
    }

    /// `lines` as two halves, the first sent first ([`Asker::in_pieces`]).
    fn halves(&mut self, lines: &[Line], may_split_reply: bool) -> Result<Vec<Suggestion>, String> {
        let mid = lines.len() / 2;
        self.in_pieces(lines, vec![0..mid, mid..lines.len()], may_split_reply)
    }

    /// `lines` as `pieces`, each through the LLM in turn; the suggestions keep their lines'
    /// indexes in `lines`. A piece that fails fails them all, and the pieces after it are not sent.
    fn in_pieces(
        &mut self,
        lines: &[Line],
        pieces: Vec<Range<usize>>,
        may_split_reply: bool,
    ) -> Result<Vec<Suggestion>, String> {
        let mut found = Vec::new();
        for piece in pieces {
            let start = piece.start;
            let suggestions = self.part(&lines[piece], may_split_reply)?;
            found.extend(suggestions.into_iter().map(|s| Suggestion {
                index: s.index + start,
                ..s
            }));
        }
        Ok(found)
    }
}

/// A worker: runs chunks through the LLM until the queue closes or the collector hangs up. Each
/// call may take `budget` of the server's context ([`ask`]).
fn work(
    backend: &Backend,
    budget: Option<llm::Budget>,
    queue: &Mutex<mpsc::Receiver<Queued>>,
    out: &mpsc::Sender<Msg>,
) {
    loop {
        let next = queue.lock().map(|rx| rx.recv());
        let Ok(Ok(Queued { chunk, file })) = next else {
            return;
        };
        let (result, made) = ask(backend, &chunk.lines, budget);
        let calls = made
            .into_iter()
            .map(|(lines, usage)| LlmCall {
                file: file.to_string(),
                lines,
                usage,
            })
            .collect();
        if out
            .send(Msg::Chunk(ChunkDone {
                id: chunk.id,
                result,
                calls,
            }))
            .is_err()
        {
            return;
        }
    }
}

/// The reader: files in order, each one's new lines queued as chunks, then the file itself.
fn read(
    todo: &[Todo],
    opts: &Options,
    queue: &mpsc::SyncSender<Queued>,
    out: &mpsc::Sender<Msg>,
    report: &(dyn Fn(Event) + Sync),
) {
    let mut chunker = Chunker::new(opts.lines_per_chunk());
    for (path, checks) in todo {
        report(Event::Reading(path.clone()));
        let prepared = prepare(path, &opts.langs, checks);
        let (refs, chunks) = if prepared.error.is_none() {
            chunker.assign(&prepared.lines)
        } else {
            (Vec::new(), Vec::new())
        };
        let file: Arc<str> = path.to_string_lossy().into();
        for chunk in chunks {
            let file = Arc::clone(&file);
            if queue.send(Queued { chunk, file }).is_err() {
                return;
            }
        }
        if out
            .send(Msg::File(Box::new(FileJob { prepared, refs })))
            .is_err()
        {
            return;
        }
    }
}

/// The backend and languages for adding to the run `scan` (`scan --run`): the run's own, which
/// `llm`, `llm_cmd` and `langs`, when given, must match. A run has one backend and one set of
/// languages. An entry's backend is as `settings` has it now, with the model the run recorded.
pub fn resume(
    scan: &ScanRow,
    llm: Option<&str>,
    llm_cmd: Option<&str>,
    langs: &[String],
    settings: &Settings,
) -> Result<(Backend, Vec<String>), String> {
    let header = &scan.header;
    let given = match (llm_cmd, llm) {
        (Some(cmd), _) => Some(Backend::shell(cmd)),
        (None, Some(name)) => Some(Backend::named(name, settings)?),
        (None, None) => None,
    };
    // A server's label waits on the model it settles, so before then it matches a run through its
    // entry with the model the entry asks for as the server named it ([`llm::same_model`]), or with
    // any model when the entry names none.
    let same_llm = |b: &Backend| {
        b.label == header.backend
            || matches!(&b.via, Via::Server(s) if header.backend.split_once('@').is_some_and(
                |(name, model)| name == s.name
                    && s.model.as_deref().is_none_or(|asked| llm::same_model(asked, model))
            ))
    };
    if given.is_some_and(|b| !same_llm(&b)) {
        return Err(format!(
            "run {} was scanned with {}; leave out --llm and --llm-cmd to use it",
            scan.name, header.backend
        ));
    }
    // An LLM that cannot be built again is refused when a file needs it: the command the run
    // recorded may not work as it did, and an entry changed since is not what the run called.
    let backend = Backend::from_label(&header.backend, settings).unwrap_or_else(|why| Backend {
        label: header.backend.clone(),
        via: Via::Unknown(why),
        chunk_lines: None,
        jobs: llm::JOBS,
        timeout: llm::TIMEOUT,
    });
    let set = |l: &[String]| CheckedWith::new("", l, 0).langs;
    if !langs.is_empty() && set(langs) != set(&header.langs) {
        return Err(format!(
            "run {} checked --lang {}; leave out --lang to use it",
            scan.name,
            header.langs.join(" --lang ")
        ));
    }
    Ok((backend, header.langs.clone()))
}

/// What a scan records under: the files to read and their checks, and the run.
struct Start {
    todo: Vec<Todo>,
    scan: ScanRow,
}

/// A new run for the files that need a check, naming every file found; `None` when none needs
/// one.
fn start_new(
    all: &[PathBuf],
    opts: &Options,
    store: &mut Store,
    summary: &mut Summary,
) -> anyhow::Result<Option<Start>> {
    let (todo, cached) = to_check(all, opts, store)?;
    summary.cached = cached;
    // No run for a scan with nothing to check: an empty run would become the newest and hide the
    // last real one from a plain `subcheck list`.
    if todo.is_empty() {
        return Ok(None);
    }
    let header = RunHeader {
        started: Timestamp::now(),
        backend: opts.backend.label.clone(),
        command: opts.backend.command_line(),
        prompt_version: llm::PROMPT_VERSION,
        version: env!("CARGO_PKG_VERSION").to_string(),
        langs: opts.langs.clone(),
    };
    let scan = store.begin_scan(&header, all)?;
    Ok(Some(Start { todo, scan }))
}

/// Adding to the run `name` (`--run`): every file found joins it, and the checks they need are
/// made under it; `None` when none needs one.
fn start_adding(
    name: &str,
    all: &[PathBuf],
    opts: &Options,
    store: &mut Store,
    summary: &mut Summary,
) -> anyhow::Result<Option<Start>> {
    let scan = store
        .scan_named(name)?
        .ok_or_else(|| anyhow::anyhow!("no run named {name}"))?;
    summary.run.clone_from(&scan.name);
    let (todo, cached) = to_check(all, opts, store)?;
    summary.cached = cached;
    if needs_llm(&todo) {
        let prompt = scan.header.prompt_version;
        if prompt != llm::PROMPT_VERSION {
            anyhow::bail!(
                "run {} was made with prompt version {prompt}, this subcheck has {}: scan the \
                 files it lacks into a new run",
                scan.name,
                llm::PROMPT_VERSION
            );
        }
        if let Via::Unknown(why) = &opts.backend.via {
            anyhow::bail!(
                "run {} was scanned with {why}: scan the files it lacks into a new run",
                scan.name
            );
        }
    }
    store.add_files(scan.id, all)?;
    if todo.is_empty() {
        return Ok(None);
    }
    Ok(Some(Start { todo, scan }))
}

/// The files under `paths` and what each needs, and the run they go under: a new one, or with
/// `--run` (`opts.into`) the run named. Every file found is the run's, skipped ones too, before
/// checking starts. Warnings go to `log`.
pub fn plan(
    paths: &[PathBuf],
    opts: &Options,
    store: &mut Store,
    log: &mut dyn Write,
) -> anyhow::Result<Planned> {
    let (all, warnings) = mkv_files(paths)?;
    for w in &warnings {
        writeln!(log, "warning: {w}")?;
    }
    let mut summary = Summary {
        files: all.len(),
        paths: paths
            .iter()
            .map(|p| store::absolute(p))
            .collect::<io::Result<_>>()?,
        ..Summary::default()
    };
    let start = match &opts.into {
        None => start_new(&all, opts, store, &mut summary)?,
        Some(name) => start_adding(name, &all, opts, store, &mut summary)?,
    };
    Ok(match start {
        None => Planned::Nothing(summary),
        Some(start) => {
            summary.run.clone_from(&start.scan.name);
            Planned::Scan(Plan { start, summary })
        }
    })
}

/// Run `plan`: one reader thread, `-j` workers, and the collector on this thread, which saves
/// each file the moment its lines are all answered. `report` hears each step (see [`Event`]),
/// from the reader's thread and this one.
pub fn execute(
    plan: Plan,
    opts: &Options,
    store: &mut Store,
    log: &mut dyn Write,
    report: &(dyn Fn(Event) + Sync),
) -> anyhow::Result<Summary> {
    let Plan {
        start: Start { todo, scan },
        summary,
    } = plan;
    report(Event::Queued(todo.iter().map(|(p, _)| p.clone()).collect()));
    let mut collector = Collector {
        store,
        scan,
        opts,
        report,
        summary,
        total: todo.len(),
        done: 0,
        answers: HashMap::new(),
        waiting: Vec::new(),
    };
    thread::scope(|scope| -> anyhow::Result<()> {
        // Created inside the scope so an early return drops the receivers: workers and the
        // reader then fail their next send and stop, instead of running the rest of the queue.
        let (queue_tx, queue_rx) = mpsc::sync_channel::<Queued>(opts.jobs.max(1) * 2);
        let queue_rx = Arc::new(Mutex::new(queue_rx));
        let (msg_tx, msg_rx) = mpsc::channel::<Msg>();
        let budget = opts.backend.budget(opts.jobs);
        for _ in 0..opts.jobs.max(1) {
            let queue_rx = Arc::clone(&queue_rx);
            let msg_tx = msg_tx.clone();
            scope.spawn(move || work(&opts.backend, budget, &queue_rx, &msg_tx));
        }
        let todo = &todo;
        scope.spawn(move || read(todo, opts, &queue_tx, &msg_tx, report));
        collector.collect(&msg_rx, log)
    })?;
    Ok(collector.summary)
}

/// What `scan` says when it is done, line by line (each to stderr): of a new run, or with
/// `adding` of what it added to one (`--run`).
pub fn summary_lines(s: &Summary, adding: bool) -> Vec<String> {
    if adding {
        return added_lines(s);
    }
    if s.run.is_empty() {
        return vec![if s.files == 0 {
            "Nothing to check: no MKV files found".to_string()
        } else {
            let paths: Vec<String> = s
                .paths
                .iter()
                .map(|p| shell_quote(&p.to_string_lossy()))
                .collect();
            format!(
                "Nothing to check: {} unchanged (subcheck review {})",
                s.cached,
                paths.join(" ")
            )
        }];
    }
    let mut lines = vec![format!(
        "run {}: {} files, {} checked, {} failed, {} unchanged since an earlier scan; {} findings{}",
        s.run,
        s.files,
        s.checked,
        s.failed,
        s.cached,
        s.findings,
        of_timing(s.timing)
    )];
    lines.extend(s.spent.line(", "));
    lines.push(review_hint(&s.run));
    lines
}

/// What `scan --run` says it did.
fn added_lines(s: &Summary) -> Vec<String> {
    let mut left = Vec::new();
    if s.cached > 0 {
        left.push(format!("{} with every check already", files(s.cached)));
    }
    if s.checked + s.failed == 0 {
        return vec![format!(
            "Nothing to add to run {}: {}",
            s.run,
            left.join(", ")
        )];
    }
    let left = if left.is_empty() {
        String::new()
    } else {
        format!(" ({} left out)", left.join(", "))
    };
    let mut lines = vec![format!(
        "run {}: added to {}{left}, {} failed; {} findings{}",
        s.run,
        files(s.checked + s.failed),
        s.failed,
        s.findings,
        of_timing(s.timing)
    )];
    lines.extend(s.spent.line(", "));
    lines.push(review_hint(&s.run));
    lines
}

/// ` (3 timing)`: how many of the findings are the timing check's; nothing when none are.
fn of_timing(n: usize) -> String {
    if n > 0 {
        format!(" ({n} timing)")
    } else {
        String::new()
    }
}

fn review_hint(run: &str) -> String {
    format!("Review them with: subcheck review {run} (or print them: subcheck list {run})")
}

/// Saves each file the moment every one of its lines has an answer.
struct Collector<'a> {
    store: &'a mut Store,
    scan: ScanRow,
    opts: &'a Options,
    /// Told of each file's progress and of every record saved.
    report: &'a (dyn Fn(Event) + Sync),
    summary: Summary,
    total: usize,
    done: usize,
    answers: HashMap<usize, Result<Vec<Suggestion>, String>>,
    waiting: Vec<FileJob>,
}

impl Collector<'_> {
    /// Runs until the reader and every worker have hung up.
    fn collect(&mut self, rx: &mpsc::Receiver<Msg>, log: &mut dyn Write) -> anyhow::Result<()> {
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::File(job) => {
                    let (answered, chunks) = self.asked(&job.refs);
                    if chunks > 0 {
                        (self.report)(Event::Asking {
                            path: job.prepared.path.clone(),
                            answered,
                            chunks,
                        });
                    }
                    self.waiting.push(*job);
                }
                Msg::Chunk(c) => {
                    // Saved as they come, so a scan cut short still says what it used.
                    if !c.calls.is_empty() {
                        for call in &c.calls {
                            self.summary.spent.add(call.usage.as_ref());
                        }
                        self.store.add_usage(self.scan.id, &c.calls)?;
                        (self.report)(Event::Appended(
                            c.calls.into_iter().map(Record::Usage).collect(),
                        ));
                    }
                    let id = c.id;
                    self.answers.insert(id, c.result);
                    for job in self
                        .waiting
                        .iter()
                        .filter(|j| j.refs.iter().any(|&(chunk, _)| chunk == id))
                    {
                        let (answered, chunks) = self.asked(&job.refs);
                        (self.report)(Event::Asking {
                            path: job.prepared.path.clone(),
                            answered,
                            chunks,
                        });
                    }
                }
            }
            while let Some(i) = self.waiting.iter().position(|j| {
                j.refs
                    .iter()
                    .all(|(chunk, _)| self.answers.contains_key(chunk))
            }) {
                let job = self.waiting.remove(i);
                let done = self.finish(job, log)?;
                (self.report)(done);
            }
        }
        for job in std::mem::take(&mut self.waiting) {
            let p = job.prepared;
            let mut said = Vec::new();
            if let Some(t) = p.truncated {
                say(log, &mut said, t.warning(&p.path))?;
            }
            self.summary.failed += 1;
            let error = "no answer for some lines".to_string();
            let path = p.path.to_string_lossy().into_owned();
            self.failed(&p, &path, &error)?;
            say(
                log,
                &mut said,
                format!("{}: FAILED: {error}", p.path.display()),
            )?;
            (self.report)(Event::Finished {
                path: p.path,
                error: Some(error),
                findings: 0,
                lines: said,
            });
        }
        Ok(())
    }

    /// A file whose lines all have their answers: saved in one transaction (its checks, or its
    /// failed attempt), its lines logged, and what it came to, for the hook. A save the database
    /// stays too busy for fails the file: nothing of it is saved, so the next scan checks it again.
    #[allow(clippy::too_many_lines)] // one file's save, then its log, in the order written
    fn finish(&mut self, job: FileJob, log: &mut dyn Write) -> anyhow::Result<Event> {
        let FileJob {
            prepared: mut p,
            refs,
        } = job;
        self.done += 1;
        let name = p
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let progress = format!("[{}/{}] {name}", self.done, self.total);
        let path = p.path.to_string_lossy().into_owned();
        let mut said = Vec::new();
        if let Some(t) = p.truncated {
            say(log, &mut said, t.warning(&p.path))?;
        }
        let llm_error = refs
            .iter()
            .find_map(|(chunk, _)| self.answers[chunk].as_ref().err())
            .map(|e| format!("LLM: {e}"));
        let mut error = p.error.clone().or(llm_error);
        let timing = p.timing.len();
        let mut found = 0;
        if let Some(e) = &error {
            self.failed(&p, &path, e)?;
        } else {
            let mut made = Vec::new();
            for &check in &p.checks {
                let findings = match check {
                    Check::Llm => findings(&self.answers, &p.lines, &refs, &path),
                    Check::Timing => std::mem::take(&mut p.timing),
                };
                made.push(Made {
                    with: checked_with(self.opts, check),
                    lines: p.lines.len(),
                    findings,
                });
            }
            match self.store.save(&self.attempt(&p, &path), made) {
                Ok(Saved { findings, carried }) => {
                    found = findings.len();
                    let mut records = vec![Record::File(record(&p, &path, None))];
                    records.extend(findings.into_iter().map(Record::Finding));
                    records.extend(carried.into_iter().map(Record::Decision));
                    (self.report)(Event::Appended(records));
                }
                Err(e) if store::is_busy(&e) => error = Some(format!("not saved: {e}")),
                Err(e) => return Err(e.into()),
            }
        }
        if let Some(e) = &error {
            self.summary.failed += 1;
            say(log, &mut said, format!("{progress}: FAILED: {e}"))?;
        } else {
            self.summary.checked += 1;
            self.summary.findings += found;
            self.summary.timing += timing;
            let line = if p.checks.contains(&Check::Llm) {
                format!(
                    "{progress}: {} lines, {found} findings{}",
                    p.lines.len(),
                    of_timing(timing)
                )
            } else {
                format!("{progress}: timing only, {found} findings")
            };
            say(log, &mut said, line)?;
        }
        for note in &p.timing_notes {
            say(log, &mut said, format!("    {note}"))?;
        }
        if self.opts.verbose {
            for note in &p.notes {
                say(log, &mut said, format!("    {note}"))?;
            }
        }
        Ok(Event::Finished {
            path: p.path,
            error,
            findings: found,
            lines: said,
        })
    }
}

impl Collector<'_> {
    /// What the scan read of the file, for its save.
    fn attempt<'p>(&self, p: &'p Prepared, path: &'p str) -> Attempt<'p> {
        Attempt {
            scan: self.scan.id,
            path,
            size: p.size,
            mtime: p.mtime,
            tracks: &p.tracks,
            truncated: p.truncated,
            at: Timestamp::now(),
        }
    }

    /// Save a failed attempt at the file's checks, and tell the hook. It replaces nothing: the
    /// file's earlier findings stay current. A database too busy to take it is no further error,
    /// as the file has failed already.
    fn failed(&mut self, p: &Prepared, path: &str, error: &str) -> anyhow::Result<()> {
        let tried: Vec<(CheckedWith, usize)> = p
            .checks
            .iter()
            .map(|&c| (checked_with(self.opts, c), p.lines.len()))
            .collect();
        match self.store.fail(&self.attempt(p, path), &tried, error) {
            Ok(()) => {
                let failed = record(p, path, Some(error));
                (self.report)(Event::Appended(vec![Record::File(failed)]));
                Ok(())
            }
            Err(e) if store::is_busy(&e) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Of the chunks that answer a file's lines (`refs`), how many have answered, and how many
    /// there are.
    fn asked(&self, refs: &[(usize, usize)]) -> (usize, usize) {
        let mut ids: Vec<usize> = refs.iter().map(|&(chunk, _)| chunk).collect();
        ids.sort_unstable();
        ids.dedup();
        let answered = ids
            .iter()
            .filter(|id| self.answers.contains_key(id))
            .count();
        (answered, ids.len())
    }
}

/// The file's record of what this scan made of it, for the hook: the checks it made, or none for a
/// failed attempt, which makes none. Review's model merges it into the file's record as a view
/// would ([`FileRecord::merge`]).
fn record(p: &Prepared, path: &str, error: Option<&str>) -> FileRecord {
    FileRecord {
        path: path.to_string(),
        size: p.size,
        mtime: p.mtime,
        error: error.map(Into::into),
        tracks: p.tracks.clone(),
        lines: p.lines.len(),
        truncated: p.truncated,
        checks: if error.is_some() {
            Vec::new()
        } else {
            p.checks.clone()
        },
    }
}

/// Write `line` to the log, and keep it for the file's [`Event::Finished`].
fn say(log: &mut dyn Write, said: &mut Vec<String>, line: String) -> io::Result<()> {
    writeln!(log, "{line}")?;
    said.push(line);
    Ok(())
}

/// One finding per distinct suggestion for each line, with up to two lines of context either side.
/// Numbered 0: the store gives each its ID as it saves it.
fn findings(
    answers: &HashMap<usize, Result<Vec<Suggestion>, String>>,
    lines: &[Line],
    refs: &[(usize, usize)],
    path: &str,
) -> Vec<Finding> {
    let context = |from: usize, to: usize| -> Vec<String> {
        let to = to.min(lines.len());
        lines[from.min(to)..to]
            .iter()
            .map(|l| l.text.clone())
            .collect()
    };
    let mut out = Vec::new();
    for (i, (line, (chunk, index))) in lines.iter().zip(refs).enumerate() {
        let Some(Ok(suggestions)) = answers.get(chunk) else {
            continue;
        };
        let mut seen: Vec<&str> = Vec::new();
        for s in suggestions.iter().filter(|s| s.index == *index) {
            if seen.contains(&s.corrected.as_str()) {
                continue;
            }
            seen.push(&s.corrected);
            out.push(Finding {
                id: 0,
                file: path.to_string(),
                style: line.style.clone(),
                original: line.text.clone(),
                suggestion: s.corrected.clone(),
                reason: s.reason.clone(),
                source: "llm".into(),
                occurrences: line.occurrences.clone(),
                before: context(i.saturating_sub(2), i),
                after: context(i + 1, i + 3),
                timing: None,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::llm::Context;
    use crate::test_support::{
        FakeServer, Reply, Request, TYPOS_LINES, copied, fitting_lines, fixture, models_reply,
        prompt_lines, scan_row,
    };
    use crate::text::Occurrence;

    fn line(text: &str) -> Line {
        Line {
            style: "D".into(),
            text: text.into(),
            occurrences: vec![Occurrence {
                track: 0,
                start_ms: 0,
                end_ms: 1,
            }],
        }
    }

    fn langs(l: &[&str]) -> Vec<String> {
        l.iter().map(|l| (*l).to_string()).collect()
    }

    #[test]
    fn adding_to_a_run_takes_its_backend_and_languages_unless_they_are_given_alike() {
        let none = Settings::parse(Path::new("/c/config.toml"), "").unwrap();
        let resume = |run: &ScanRow, llm, llm_cmd, langs: &[String]| {
            super::resume(run, llm, llm_cmd, langs, &none)
        };
        let run = scan_row("claude", &["eng", "und"]);
        let (b, l) = resume(&run, None, None, &[]).unwrap();
        assert_eq!((b.label.as_str(), l), ("claude", langs(&["eng", "und"])));
        // The same, given again (in another order).
        let (b, _) = resume(&run, Some("claude"), None, &langs(&["und", "eng"])).unwrap();
        assert_eq!(b.label, "claude");
        assert_eq!(
            resume(&run, Some("gpt"), None, &[]).unwrap_err(),
            "run r was scanned with claude; leave out --llm and --llm-cmd to use it"
        );
        assert_eq!(
            resume(&run, None, None, &langs(&["eng"])).unwrap_err(),
            "run r checked --lang eng --lang und; leave out --lang to use it"
        );
        // A command LLM comes back from its label.
        let cmd = scan_row("cmd:my-llm --x", &["eng"]);
        let (b, _) = resume(&cmd, None, None, &[]).unwrap();
        assert_eq!(b.command_line(), ["sh", "-c", "my-llm --x"]);
        // A backend this subcheck does not know: unknown, refused later if a file needs it,
        // whatever command the run recorded.
        let mut gone = scan_row("gemini", &["eng"]);
        gone.header.command = vec!["gemini".into(), "-p".into()];
        let (b, _) = resume(&gone, None, None, &[]).unwrap();
        assert_eq!(
            (b.label.as_str(), matches!(b.via, Via::Unknown(_))),
            ("gemini", true)
        );
    }

    #[test]
    fn a_run_scanned_with_an_entry_resumes_with_it_while_the_file_has_it() {
        let s = Settings::parse(
            Path::new("/c/config.toml"),
            "[llm.mine]\ncommand = \"x\"\njobs = 3\n",
        )
        .unwrap();
        let run = scan_row("mine", &["eng"]);
        let (b, _) = resume(&run, None, None, &[], &s).unwrap();
        assert_eq!((b.label.as_str(), b.jobs), ("mine", 3));
        assert_eq!(
            resume(&run, Some("mine"), None, &[], &s).unwrap().0.label,
            "mine"
        );
        assert_eq!(
            resume(&run, Some("claude"), None, &[], &s).unwrap_err(),
            "run r was scanned with mine; leave out --llm and --llm-cmd to use it"
        );
        let none = Settings::parse(Path::new("/c/config.toml"), "").unwrap();
        let (b, _) = resume(&run, None, None, &[], &none).unwrap();
        assert_eq!(
            (b.label.as_str(), matches!(b.via, Via::Unknown(_))),
            ("mine", true)
        );
    }

    #[test]
    fn a_run_through_a_server_resumes_by_the_entrys_name_with_the_runs_model() {
        let s = Settings::parse(
            Path::new("/c/config.toml"),
            "[llm.srv]\nurl = \"http://127.0.0.1:1/v1\"\n",
        )
        .unwrap();
        let run = scan_row("srv@a", &["eng"]);
        for llm in [None, Some("srv")] {
            let (b, _) = resume(&run, llm, None, &[], &s).unwrap();
            assert_eq!(b.label, "srv@a", "{llm:?}");
        }
        let pinned = Settings::parse(
            Path::new("/c/config.toml"),
            "[llm.srv]\nurl = \"http://127.0.0.1:1/v1\"\nmodel = \"b\"\n",
        )
        .unwrap();
        assert_eq!(
            resume(&run, Some("srv"), None, &[], &pinned).unwrap_err(),
            "run r was scanned with srv@a; leave out --llm and --llm-cmd to use it"
        );
        let (b, _) = resume(&run, None, None, &[], &pinned).unwrap();
        assert_eq!(
            b.via,
            Via::Unknown("srv@a, but /c/config.toml now has srv ask for \"b\"".into())
        );
    }

    #[test]
    fn an_entrys_untagged_model_settles_to_latest_and_its_run_resumes() {
        let fake = FakeServer::start(|r| match r.path.as_str() {
            "/v1/models" => models_reply(&["gemma4:latest", "x"]),
            _ => Reply::status(404, "{}"),
        });
        let s = Settings::parse(
            Path::new("/c/config.toml"),
            &format!("[llm.srv]\nurl = \"{}\"\nmodel = \"gemma4\"\n", fake.url),
        )
        .unwrap();
        let (b, _) = Backend::named("srv", &s).unwrap().ready().unwrap();
        assert_eq!(b.label, "srv@gemma4:latest");
        assert_eq!(
            Backend::from_label("srv@gemma4:latest", &s).map(|b| b.label),
            Ok("srv@gemma4:latest".into())
        );
        assert_eq!(
            Backend::from_label("srv@gemma4:12b", &s),
            Err("srv@gemma4:12b, but /c/config.toml now has srv ask for \"gemma4\"".into())
        );
        let run = scan_row("srv@gemma4:latest", &["eng"]);
        for llm in [None, Some("srv")] {
            let (b, _) = resume(&run, llm, None, &[], &s).unwrap();
            assert_eq!(b.label, "srv@gemma4:latest", "{llm:?}");
            assert!(
                matches!(&b.via, Via::Server(s) if s.model.as_deref() == Some("gemma4:latest")),
                "{llm:?}"
            );
        }
        assert_eq!(
            resume(
                &scan_row("srv@gemma4:12b", &["eng"]),
                Some("srv"),
                None,
                &[],
                &s
            )
            .unwrap_err(),
            "run r was scanned with srv@gemma4:12b; leave out --llm and --llm-cmd to use it"
        );
    }

    /// Options for a scan through `[llm.srv]` on `server`, `extra` in its table, readied, with
    /// `jobs` calls at once and `flag` as `--chunk-lines`.
    fn through(server: &FakeServer, extra: &str, jobs: usize, flag: Option<usize>) -> Options {
        let text = format!("[llm.srv]\nurl = \"{}\"\n{extra}", server.url);
        let s = Settings::parse(Path::new("/c/config.toml"), &text).unwrap();
        let (backend, _) = Backend::named("srv", &s).unwrap().ready().unwrap();
        with(backend, jobs, flag)
    }

    /// Options for a scan through `backend`, with `jobs` calls at once and `flag` as
    /// `--chunk-lines`.
    fn with(backend: Backend, jobs: usize, flag: Option<usize>) -> Options {
        Options {
            backend,
            chunk_lines: flag,
            jobs,
            langs: langs(&["eng"]),
            force: false,
            verbose: false,
            into: None,
        }
    }

    /// A command entry, `[llm.mine]`.
    fn command_entry() -> Backend {
        let s = Settings::parse(Path::new("/c/config.toml"), "[llm.mine]\ncommand = \"x\"\n");
        Backend::named("mine", &s.unwrap()).unwrap()
    }

    #[test]
    fn chunk_size_is_the_flag_else_the_entrys_at_most_what_fits_else_fitted_else_400() {
        // 4096 tokens shared by 2 jobs: 2048 a call, which fits 30 lines.
        let sized = FakeServer::llm_sized(&["m"], 4096);
        // 16384 tokens shared by 2 jobs: 8192 a call, which fits 155 lines.
        let large = FakeServer::llm_sized(&["m"], 16384);
        // 131072 tokens, each call's whole: 3420 lines fit.
        let huge = FakeServer::llm_sized(&["m"], 131_072);
        let silent = FakeServer::llm(&["m"]);
        let whole = "shared_context = false\n";
        for (server, extra, jobs, flag, lines) in [
            (&sized, "", 1, None, fitting_lines(4096)),
            (&sized, "", 0, None, fitting_lines(4096)),
            (&sized, "", 2, None, fitting_lines(2048)),
            (&sized, whole, 2, None, fitting_lines(4096)),
            // A size set above what fits is cut to it; one below it is kept.
            (&sized, "chunk_lines = 50\n", 2, None, 30),
            (&sized, "chunk_lines = 20\n", 2, None, 20),
            (&sized, "chunk_lines = 50\n", 2, Some(7), 7),
            (&sized, "chunk_lines = 7\n", 2, Some(50), 30),
            (&large, "", 2, Some(2000), 155),
            (&large, "", 2, Some(7), 7),
            // With no limit of 400 on what fits.
            (&huge, whole, 2, Some(600), 600),
            (&huge, whole, 2, None, llm::CHUNK_LINES),
            // With no size known, as set.
            (&silent, "", 2, None, llm::CHUNK_LINES),
            (&silent, "", 2, Some(7), 7),
            (&silent, "", 2, Some(2000), 2000),
            (&silent, "chunk_lines = 2000\n", 2, None, 2000),
        ] {
            assert_eq!(
                through(server, extra, jobs, flag).lines_per_chunk(),
                lines,
                "{extra:?} -j {jobs} --chunk-lines {flag:?}"
            );
        }
        for backend in [
            command_entry(),
            Backend::preset("claude").unwrap(),
            Backend::shell("x"),
        ] {
            let label = backend.label.clone();
            assert_eq!(
                with(backend, 2, None).lines_per_chunk(),
                llm::CHUNK_LINES,
                "{label}"
            );
        }
    }

    #[test]
    fn only_a_server_that_did_not_say_its_size_warns_and_only_when_nothing_sets_the_size() {
        let silent = FakeServer::llm(&["m"]);
        let said = |k: u64| {
            Some(format!(
                "srv did not say its context size; set chunk_lines if it holds under {k}k tokens"
            ))
        };
        for (extra, jobs, flag, warning) in [
            ("", 1, None, said(19)),
            ("", 2, None, said(38)),
            ("shared_context = false\n", 2, None, said(19)),
            ("chunk_lines = 50\n", 2, None, None),
            ("", 2, Some(7), None),
        ] {
            assert_eq!(
                through(&silent, extra, jobs, flag).size_warning(),
                warning,
                "{extra:?} -j {jobs} --chunk-lines {flag:?}"
            );
        }
        // An entry's own max_tokens is the reply's room beside a full chunk: 400 lines' estimate
        // and 8192 come to just under 23k.
        for own in [
            "[llm.srv.request]\nmax_tokens = 8192\n",
            "[llm.srv.request]\nmax_completion_tokens = 8192\n",
        ] {
            assert_eq!(
                through(&silent, own, 1, None).size_warning(),
                said(23),
                "{own:?}"
            );
        }
        let sized = FakeServer::llm_sized(&["m"], 4096);
        assert_eq!(through(&sized, "", 2, None).size_warning(), None);
        let unloaded = FakeServer::ollama("m", 2048);
        let opts = through(&unloaded, "", 2, None);
        assert_eq!(
            (opts.backend.context(), opts.size_warning()),
            (Some(Context::NotLoaded), None)
        );
        assert_eq!(with(command_entry(), 2, None).size_warning(), None);
    }

    #[test]
    fn an_entrys_cap_that_leaves_no_room_for_a_line_warns_whatever_sets_the_chunks() {
        // 16384 tokens shared by 2 jobs: 8192 a call, all of it the reply's under a cap of 8192.
        let server = FakeServer::llm_sized(&["m"], 16384);
        let said = |field: &str, cap: u64, share: u64, fix: &str| {
            Some(format!(
                "srv's request.{field} ({cap}) leaves no room for lines in a call's {share} \
                 tokens, so each line is sent alone; {fix}"
            ))
        };
        let shared = "lower it, or set shared_context = false or fewer jobs";
        let cap = |request: &str| format!("[llm.srv.request]\n{request}\n");
        let whole = |request: &str| format!("shared_context = false\n{}", cap(request));
        // The largest cap that leaves the prompt one line's estimate.
        let fits = 8192 - llm::estimate(1);
        let over = fits + 1;
        for (extra, jobs, flag, warning) in [
            (
                cap("max_tokens = 8192"),
                2,
                None,
                said("max_tokens", 8192, 8192, shared),
            ),
            (
                cap("max_tokens = 8192"),
                2,
                Some(7),
                said("max_tokens", 8192, 8192, shared),
            ),
            (
                cap("max_completion_tokens = 9000"),
                2,
                None,
                said("max_completion_tokens", 9000, 8192, shared),
            ),
            (
                cap(&format!("max_tokens = {over}")),
                2,
                None,
                said("max_tokens", over, 8192, shared),
            ),
            (cap(&format!("max_tokens = {fits}")), 2, None, None),
            (cap("max_tokens = 8192"), 1, None, None),
            (whole("max_tokens = 8192"), 2, None, None),
            (
                whole("max_tokens = 16384"),
                2,
                None,
                said("max_tokens", 16384, 16384, "lower it"),
            ),
            (
                cap("max_tokens = 16384"),
                1,
                None,
                said("max_tokens", 16384, 16384, "lower it"),
            ),
            (String::new(), 2, None, None),
        ] {
            assert_eq!(
                through(&server, &extra, jobs, flag).size_warning(),
                warning,
                "{extra:?} -j {jobs} --chunk-lines {flag:?}"
            );
        }
        // Each line alone, as the warning says, whatever sets the size.
        for flag in [None, Some(7)] {
            let opts = through(&server, &cap("max_tokens = 8192"), 2, flag);
            assert_eq!(opts.lines_per_chunk(), 1, "--chunk-lines {flag:?}");
        }
        // A server that did not say its size gives no share to warn of.
        let silent = FakeServer::llm(&["m"]);
        let opts = through(&silent, &cap("max_tokens = 100000"), 2, Some(7));
        assert_eq!(opts.size_warning(), None);
    }

    /// What `dry_run` writes over a copy of typos.mkv with `opts`: its output, then its log.
    fn dry_run_of_typos(opts: &Options) -> (String, String) {
        let (dir, file) = copied("typos.mkv");
        let store = Store::open_at(&dir.path().join("subcheck.db")).unwrap();
        let (mut out, mut log) = (Vec::new(), Vec::new());
        dry_run(&[file], opts, &store, &mut out, &mut log).unwrap();
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(log).unwrap(),
        )
    }

    #[test]
    fn a_dry_run_through_an_unloaded_ollama_model_says_its_chunks_are_400_lines() {
        let server = FakeServer::ollama("m", 2048);
        let opts = through(&server, "", 2, None);
        let (out, log) = dry_run_of_typos(&opts);
        assert!(
            log.contains(
                "dry run: srv@m's model is not loaded, so these chunks are 400 lines; a scan \
                 loads it and fits them to its context"
            ),
            "{log}"
        );
        assert!(!log.contains("warning: "), "{log}");
        assert!(
            out.contains(&format!("(chunk 0, {TYPOS_LINES} lines)")),
            "{out}"
        );
        let requests = server.requests();
        assert!(requests.iter().all(|r| r.method == "GET"), "{requests:?}");
    }

    #[test]
    fn a_dry_run_through_an_unloaded_ollama_model_with_a_size_set_says_a_scan_cuts_it() {
        let server = FakeServer::ollama("m", 2048);
        for (extra, flag) in [("chunk_lines = 3\n", None), ("", Some(3))] {
            let opts = through(&server, extra, 2, flag);
            let (out, log) = dry_run_of_typos(&opts);
            assert!(
                log.contains(
                    "dry run: srv@m's model is not loaded, so these chunks are 3 lines as set; a \
                     scan loads it and cuts them to what fits its context"
                ),
                "{extra:?} {flag:?}: {log}"
            );
            assert!(out.contains("(chunk 0, 3 lines)"), "{out}");
        }
        let requests = server.requests();
        assert!(requests.iter().all(|r| r.method == "GET"), "{requests:?}");
    }

    #[test]
    fn a_dry_run_prints_the_chunks_a_scan_would_send() {
        let n = fitting_lines(1024 / 2);
        assert!(n < TYPOS_LINES);
        let server = FakeServer::llm_sized(&["m"], 1024);
        let (out, log) = dry_run_of_typos(&through(&server, "", 2, None));
        assert!(out.contains(&format!("(chunk 0, {n} lines)")), "{out}");
        let chunks = out.lines().filter(|l| l.starts_with("== ")).count();
        assert_eq!(chunks, TYPOS_LINES.div_ceil(n), "{out}");
        assert!(!log.contains("dry run: "), "{log}");
    }

    /// What a dry run printed: each chunk's header after the path, `(chunk <id>, <n> lines)`, and
    /// its prompt.
    fn printed(out: &str) -> Vec<(String, String)> {
        let mut chunks: Vec<(String, String)> = Vec::new();
        for l in out.split_inclusive('\n') {
            if l.starts_with("== ") {
                let (_, header) = l.trim_end().rsplit_once(" (").expect("a chunk's header");
                chunks.push((format!("({header}"), String::new()));
            } else {
                chunks.last_mut().expect("a header first").1.push_str(l);
            }
        }
        chunks
    }

    #[test]
    fn a_dry_run_prints_a_forced_size_cut_to_what_fits_as_the_scan_sends_it() {
        // 1400 tokens shared by 2 jobs: 700 a call, whose prompt holds fewer than all of
        // typos.mkv's lines, which the chunks are forced to.
        let fits = fitting_lines(1400 / 2);
        assert!(fits < TYPOS_LINES);
        let server = FakeServer::llm_sized(&["m"], 1400);
        let opts = through(&server, "", 2, Some(TYPOS_LINES));
        assert_eq!(opts.lines_per_chunk(), fits);
        let (out, _) = dry_run_of_typos(&opts);
        let chunks = printed(&out);
        let headers = [
            format!("(chunk 0, {fits} lines)"),
            format!("(chunk 1, {} lines)", TYPOS_LINES - fits),
        ];
        assert_eq!(
            chunks.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(),
            headers,
            "{out}"
        );
        // The prompts the scan sends for the same chunks.
        let lines = prepare(&fixture("typos.mkv"), &opts.langs, &[Check::Llm]).lines;
        let (_, queued) = Chunker::new(opts.lines_per_chunk()).assign(&lines);
        for chunk in &queued {
            let (result, _) = ask(&opts.backend, &chunk.lines, opts.backend.budget(opts.jobs));
            assert!(result.is_ok(), "{result:?}");
        }
        let sent: Vec<String> = server.completions().iter().map(prompt_of).collect();
        assert_eq!(chunks.into_iter().map(|(_, p)| p).collect::<Vec<_>>(), sent);
    }

    #[test]
    fn a_dry_run_through_a_server_that_did_not_say_its_size_warns_before_the_chunks() {
        let server = FakeServer::llm(&["m"]);
        let (out, log) = dry_run_of_typos(&through(&server, "", 1, None));
        assert_eq!(
            log,
            "warning: srv did not say its context size; set chunk_lines if it holds under 19k \
             tokens\n"
        );
        assert!(
            out.contains(&format!("(chunk 0, {TYPOS_LINES} lines)")),
            "{out}"
        );
    }

    #[test]
    fn a_plan_needs_the_llm_only_when_a_file_needs_its_check() {
        let plan = |checks: Vec<Check>| Plan {
            start: Start {
                todo: vec![(PathBuf::from("/m/a.mkv"), checks)],
                scan: scan_row("claude", &["eng"]),
            },
            summary: Summary::default(),
        };
        assert!(!plan(vec![Check::Timing]).needs_llm());
        assert!(plan(vec![Check::Llm, Check::Timing]).needs_llm());
    }

    /// A file as `finish` gets it when only the timing check is made of it and found nothing.
    fn timing_only(path: &str) -> FileJob {
        FileJob {
            prepared: Prepared {
                path: PathBuf::from(path),
                size: 10,
                mtime: Timestamp::UNIX_EPOCH,
                checks: vec![Check::Timing],
                tracks: Vec::new(),
                lines: Vec::new(),
                timing: Vec::new(),
                timing_notes: Vec::new(),
                error: None,
                truncated: None,
                notes: Vec::new(),
            },
            refs: Vec::new(),
        }
    }

    #[test]
    fn a_save_that_stays_busy_fails_that_file_alone_and_the_scan_goes_on() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_at(&dir.path().join("subcheck.db")).unwrap();
        store
            .set_busy_timeout(std::time::Duration::from_millis(50))
            .unwrap();
        let files = [PathBuf::from("/m/a.mkv"), PathBuf::from("/m/b.mkv")];
        let header = scan_row("claude", &["eng"]).header;
        let scan = store.begin_scan(&header, &files).unwrap();
        let opts = Options {
            backend: Backend::shell("true"),
            chunk_lines: Some(10),
            jobs: 1,
            langs: langs(&["eng"]),
            force: false,
            verbose: false,
            into: None,
        };
        let events = Mutex::new(Vec::new());
        let report = |e: Event| events.lock().unwrap().push(e);
        let mut collector = Collector {
            store: &mut store,
            scan,
            opts: &opts,
            report: &report,
            summary: Summary::default(),
            total: 2,
            done: 0,
            answers: HashMap::new(),
            waiting: Vec::new(),
        };
        let mut log = Vec::new();

        // Another connection holds the write lock while the first file is saved.
        let other = rusqlite::Connection::open(dir.path().join("subcheck.db")).unwrap();
        other.execute_batch("BEGIN EXCLUSIVE").unwrap();
        let first = collector.finish(timing_only("/m/a.mkv"), &mut log).unwrap();
        other.execute_batch("ROLLBACK").unwrap();
        let Event::Finished {
            error, findings, ..
        } = first
        else {
            panic!("not a Finished event");
        };
        assert!(error.unwrap().starts_with("not saved: "));
        assert_eq!(findings, 0);

        // The lock is gone: the next file is saved.
        collector.finish(timing_only("/m/b.mkv"), &mut log).unwrap();
        assert_eq!(
            (collector.summary.failed, collector.summary.checked),
            (1, 1)
        );
        let log = String::from_utf8(log).unwrap();
        assert!(log.contains("[1/2] a.mkv: FAILED: not saved: "), "{log}");
        assert!(
            log.contains("[2/2] b.mkv: timing only, 0 findings"),
            "{log}"
        );

        let timing = checked_with(&opts, Check::Timing);
        let mtime = Timestamp::UNIX_EPOCH;
        assert!(!store.is_fresh("/m/a.mkv", 10, mtime, &timing).unwrap());
        assert!(store.is_fresh("/m/b.mkv", 10, mtime, &timing).unwrap());
        let rows: i64 = other
            .query_row(
                "SELECT COUNT(*) FROM checks WHERE path = '/m/a.mkv'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0, "a busy save leaves no attempt behind either");
    }

    #[test]
    fn chunker_splits_by_size_and_sends_each_text_once() {
        let mut c = Chunker::new(2);
        let (refs, chunks) = c.assign(&[line("a"), line("b"), line("c")]);
        assert_eq!(refs, [(0, 0), (0, 1), (1, 0)]);
        assert_eq!(
            chunks.iter().map(|c| c.lines.len()).collect::<Vec<_>>(),
            [2, 1]
        );
        // A second file: "b" was already sent, "d" is new and starts a chunk of its own.
        let (refs, chunks) = c.assign(&[line("b"), line("d")]);
        assert_eq!(refs, [(0, 1), (2, 0)]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].lines[0].text, "d");
    }

    /// llama-server's refusal of a prompt over its context.
    const OVERFLOW: &str = include_str!("../tests/fixtures/llm/llama-server-overflow.json");

    /// llama-server's refusal of calls in flight that outgrew the cache they share.
    const SHARED_CACHE: &str = include_str!("../tests/fixtures/llm/llama-server-shared-cache.json");

    /// Ten lines, the eighth (index 7) with a typo.
    fn ten_lines() -> Vec<Line> {
        (0..10)
            .map(|i| {
                if i == 7 {
                    line("We should leave befor it rains.")
                } else {
                    line(&format!("Line {i}."))
                }
            })
            .collect()
    }

    /// The prompt `r` sent: its last message's text.
    fn prompt_of(r: &Request) -> String {
        r.json()["messages"]
            .as_array()
            .and_then(|m| m.last())
            .and_then(|m| m["content"].as_str())
            .expect("the last message has text content")
            .to_string()
    }

    /// What a model that finds the one typo replies to `r`'s prompt: the `befor` line by the ID
    /// that prompt gives it, corrected, else NONE.
    fn finding(r: &Request) -> String {
        prompt_of(r)
            .lines()
            .filter_map(|l| l.strip_prefix('L'))
            .filter(|l| l.starts_with(|c: char| c.is_ascii_digit()) && l.contains("befor"))
            .find_map(|l| l.split_once(' '))
            .map_or_else(
                || "NONE".to_string(),
                |(id, _)| format!("L{id}\tWe should leave before it rains.\tbefor"),
            )
    }

    /// An OpenAI chat completion of `content`, stopped for `finish`, its prompt `prompt_tokens`
    /// long.
    fn completion(content: &str, finish: &str, prompt_tokens: u64) -> Reply {
        let body = serde_json::json!({
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": content},
                "finish_reason": finish,
            }],
            "usage": {
                "prompt_tokens": prompt_tokens,
                "completion_tokens": 20,
                "total_tokens": prompt_tokens + 20,
            },
        });
        Reply::ok(body.to_string())
    }

    /// A readied backend for a server that lists `m`, with a context of `n_ctx` tokens when given,
    /// and answers each chat completion with `answer` of the lines its prompt sent and the request.
    fn serving(
        n_ctx: Option<u64>,
        answer: impl Fn(usize, &Request) -> Reply + Send + Sync + 'static,
    ) -> Backend {
        let model = match n_ctx {
            Some(n) => serde_json::json!({"id": "m", "object": "model", "meta": {"n_ctx": n}}),
            None => serde_json::json!({"id": "m", "object": "model"}),
        };
        let listed = serde_json::json!({"object": "list", "data": [model]}).to_string();
        let server = FakeServer::start(move |r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/v1/models") => Reply::ok(listed.clone()),
            ("POST", "/v1/chat/completions") => answer(prompt_lines(r), r),
            _ => Reply::status(404, "{}"),
        });
        through(&server, "", 1, None).backend
    }

    /// How many lines each call sent, in the order they went.
    fn sent(calls: &[Call]) -> Vec<usize> {
        calls.iter().map(|c| c.0).collect()
    }

    /// The lines a chunk's suggestions are for.
    fn found_at(result: Result<Vec<Suggestion>, String>) -> Vec<usize> {
        result.unwrap().iter().map(|s| s.index).collect()
    }

    #[test]
    fn each_call_gets_the_backends_limit() {
        let backend = Backend {
            timeout: Duration::from_secs(1),
            ..Backend::shell("sleep 20")
        };
        let began = Instant::now();
        let (result, _) = ask(&backend, &[line("One.")], None);
        assert_eq!(result, Err("sh timed out after 1s".to_string()));
        assert!(began.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_chunk_refused_for_the_context_is_halved_until_it_fits_and_its_findings_keep_their_lines() {
        let backend = serving(None, |lines, r| {
            if lines > 4 {
                Reply::status(400, OVERFLOW)
            } else {
                completion(&finding(r), "stop", 600)
            }
        });
        let (result, calls) = ask(&backend, &ten_lines(), None);
        assert_eq!(sent(&calls), [10, 5, 2, 3, 5, 2, 3]);
        let found = result.unwrap();
        assert_eq!(found.iter().map(|s| s.index).collect::<Vec<_>>(), [7]);
        assert_eq!(found[0].corrected, "We should leave before it rains.");
    }

    #[test]
    fn a_line_that_never_fits_fails_its_chunk_after_its_second_attempt() {
        let backend = serving(None, |_, _| Reply::status(400, OVERFLOW));
        let (result, calls) = ask(&backend, &[line("One."), line("Two.")], None);
        // Halved, then the first half's line refused twice; the second half is not sent.
        assert_eq!(sent(&calls), [2, 1, 1]);
        let error = result.unwrap_err();
        assert!(error.contains("HTTP 400"), "{error}");
    }

    #[test]
    fn a_refused_key_fails_a_chunk_without_a_second_request() {
        let fake = FakeServer::start(|r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/v1/models") => models_reply(&["m"]),
            _ => Reply::status(401, r#"{"error":{"message":"bad key"}}"#),
        });
        let backend = through(&fake, "api_key_env = \"HOME\"\n", 1, None).backend;
        let (result, calls) = ask(&backend, &ten_lines(), None);
        let error = result.unwrap_err();
        assert!(error.contains("HTTP 401"), "{error}");
        // The chunk's second try sends nothing, so it counts as no call.
        assert_eq!(sent(&calls), [10]);
        assert_eq!(fake.completions().len(), 1);
        let (again, calls) = ask(&backend, &ten_lines(), None);
        assert_eq!(again, Err(error));
        assert_eq!(sent(&calls), Vec::<usize>::new());
        assert_eq!(fake.completions().len(), 1);
    }

    #[test]
    fn a_line_refused_once_for_the_context_lands_on_its_second_attempt() {
        // llama-server refuses calls in flight that outgrow the cache they share, which a line
        // sent again once the others are done can fit.
        let refused = std::sync::atomic::AtomicBool::new(false);
        let backend = serving(None, move |_, r| {
            if refused.swap(true, std::sync::atomic::Ordering::SeqCst) {
                completion(&finding(r), "stop", 600)
            } else {
                Reply::status(500, SHARED_CACHE)
            }
        });
        let (result, calls) = ask(&backend, &ten_lines()[7..8], None);
        assert_eq!(sent(&calls), [1, 1]);
        assert_eq!(found_at(result), [0]);
    }

    #[test]
    fn a_reply_cut_off_splits_once_and_a_half_cut_off_again_fails_after_its_retry() {
        let backend = serving(None, |_, r| completion(&finding(r), "length", 600));
        let (result, calls) = ask(&backend, &ten_lines(), None);
        assert_eq!(sent(&calls), [10, 5, 5]);
        assert_eq!(
            result,
            Err("the reply was cut off at the server's length limit".to_string())
        );
    }

    #[test]
    fn a_reply_cut_off_whole_lands_in_halves() {
        let backend = serving(None, |lines, r| {
            let finish = if lines == 10 { "length" } else { "stop" };
            completion(&finding(r), finish, 600)
        });
        let (result, calls) = ask(&backend, &ten_lines(), None);
        assert_eq!(sent(&calls), [10, 5, 5]);
        assert_eq!(found_at(result), [7]);
    }

    /// The smallest share of a context whose budget leaves the prompt room for `lines` lines'
    /// estimate.
    fn holding(lines: usize) -> u64 {
        // The prompt keeps three quarters of a share or more, so twice the estimate holds it.
        (0..=2 * llm::estimate(lines))
            .find(|&share| llm::Budget::new(share, None).prompt() >= llm::estimate(lines))
            .expect("a share that holds them")
    }

    /// Options for a scan through a server whose calls each have `share` tokens, 2 at once, with
    /// `flag` as `--chunk-lines`.
    fn sharing(share: u64, flag: Option<usize>) -> Options {
        through(&FakeServer::llm_sized(&["m"], 2 * share), "", 2, flag)
    }

    #[test]
    fn a_forced_size_is_cut_to_the_lines_whose_estimate_the_prompt_holds() {
        let share = holding(4);
        assert_eq!(sharing(share, Some(4)).lines_per_chunk(), 4);
        // A share a token smaller still holds the estimate, but not with the reply's room beside it.
        assert!(llm::estimate(4) < share);
        assert_eq!(sharing(share - 1, Some(4)).lines_per_chunk(), 3);
    }

    #[test]
    fn a_chunk_forced_over_what_fits_is_cut_when_it_is_made_and_each_cut_goes_whole() {
        let opts = sharing(holding(4), Some(10));
        let (_, chunks) = Chunker::new(opts.lines_per_chunk()).assign(&ten_lines());
        let sizes: Vec<usize> = chunks.iter().map(|c| c.lines.len()).collect();
        assert_eq!(sizes, [4, 4, 2]);
        let budget = opts.backend.budget(opts.jobs);
        let backend = serving(None, |_, r| completion(&finding(r), "stop", 600));
        let found: Vec<Vec<usize>> = chunks
            .iter()
            .map(|chunk| {
                let (result, calls) = ask(&backend, &chunk.lines, budget);
                assert_eq!(sent(&calls), [chunk.lines.len()]);
                found_at(result)
            })
            .collect();
        // The typo on line 7 is found in the second chunk, which starts at line 4.
        assert_eq!(found, [vec![], vec![3], vec![]]);
        // A line over what its budget leaves the prompt, here nothing, is a chunk of its own, sent
        // anyway: only the server can say it does not fit.
        let none = llm::Budget::new(1000, Some(1000));
        assert_eq!(none.fit(), 1);
        let (result, calls) = ask(&backend, &ten_lines()[7..8], Some(none));
        assert_eq!(sent(&calls), [1]);
        assert_eq!(found_at(result), [0]);
    }

    #[test]
    fn a_prompt_the_server_may_have_cut_is_split_like_a_refusal() {
        let backend = serving(Some(1000), |lines, r| {
            let prompt_tokens = if lines > 4 { 990 } else { 600 };
            completion(&finding(r), "stop", prompt_tokens)
        });
        assert_eq!(backend.context(), Some(Context::Tokens(1000)));
        let (result, calls) = ask(&backend, &ten_lines(), None);
        assert_eq!(sent(&calls), [10, 5, 2, 3, 5, 2, 3]);
        assert_eq!(found_at(result), [7]);
        // A prompt the server may have cut still counts what it used.
        assert_eq!(calls[0].1.as_ref().map(|u| u.input_tokens), Some(990));
        // Given a budget, a prompt is measured against the call's share of the context: 8192 of
        // 16384 tokens, with 2 calls at once.
        let backend = serving(Some(16384), |lines, r| {
            let prompt_tokens = if lines > 4 { 8195 } else { 600 };
            completion(&finding(r), "stop", prompt_tokens)
        });
        let (result, calls) = ask(&backend, &ten_lines(), backend.budget(2));
        assert_eq!(sent(&calls), [10, 5, 2, 3, 5, 2, 3]);
        assert_eq!(found_at(result), [7]);
    }

    #[test]
    fn an_entrys_own_max_tokens_is_the_room_its_reply_is_sized_with() {
        // 16384 tokens, each call's whole: the reply's quarter would leave the prompt 12288 of
        // them, and the entry's own max_tokens of 8192 leaves it 8192.
        let server = FakeServer::llm_sized(&["m"], 16384);
        let whole = "shared_context = false\n";
        let own = format!("{whole}[llm.srv.request]\nmax_tokens = 8192\n");
        let quartered = through(&server, whole, 2, None).lines_per_chunk();
        let opts = through(&server, &own, 2, None);
        let n = opts.lines_per_chunk();
        assert!(n < quartered, "{n} of {quartered}");
        assert!(llm::estimate(n) <= 16384 - 8192 && llm::estimate(n + 1) > 16384 - 8192);
        // A max_tokens that is no whole number leaves the reply its quarter.
        let text = format!("{whole}[llm.srv.request]\nmax_tokens = \"8192\"\n");
        assert_eq!(
            through(&server, &text, 2, None).lines_per_chunk(),
            quartered
        );
        // max_completion_tokens, OpenAI's newer name for it, counts the same, and of the two the
        // larger is the room.
        for request in [
            "max_completion_tokens = 8192\n",
            "max_tokens = 4000\nmax_completion_tokens = 8192\n",
            "max_tokens = 8192\nmax_completion_tokens = 4000\n",
        ] {
            let text = format!("{whole}[llm.srv.request]\n{request}");
            assert_eq!(
                through(&server, &text, 2, None).lines_per_chunk(),
                n,
                "{request:?}"
            );
        }
        // A size forced to the lines the quarter would fit is cut to what the entry's own
        // max_tokens leaves, and each call sends that max_tokens.
        let forced = through(&server, &own, 2, Some(quartered));
        assert_eq!(forced.lines_per_chunk(), n);
        let lines: Vec<Line> = (0..quartered)
            .map(|i| line(&format!("Line {i}.")))
            .collect();
        let (_, chunks) = Chunker::new(forced.lines_per_chunk()).assign(&lines);
        let budget = forced.backend.budget(forced.jobs);
        for chunk in &chunks {
            let (result, _) = ask(&forced.backend, &chunk.lines, budget);
            assert!(result.is_ok(), "{result:?}");
        }
        let calls = server.completions();
        assert_eq!(
            calls.iter().map(prompt_lines).collect::<Vec<_>>(),
            [n, quartered - n]
        );
        let caps: Vec<Option<u64>> = calls
            .iter()
            .map(|r| r.json()["max_tokens"].as_u64())
            .collect();
        assert_eq!(caps, [Some(8192), Some(8192)]);
    }

    #[test]
    fn a_call_caps_its_reply_at_the_quarter_of_its_budget_up_to_4096() {
        // 4096 tokens shared by 2 jobs: 2048 a call, the last 512 of them for the reply.
        let sized = FakeServer::llm_sized(&["m"], 4096);
        let silent = FakeServer::llm(&["m"]);
        // 16384 tokens: 8192 a call, 2048 for the reply; with the calls not sharing it, the
        // quarter of the whole is 4096. Of a million tokens, a quarter of a call's share would be
        // over a hundred thousand, more than many hosted APIs let a reply take.
        let large = FakeServer::llm_sized(&["m"], 16384);
        let huge = FakeServer::llm_sized(&["m"], 1 << 20);
        for (server, extra, cap) in [
            (&sized, "", Some(512)),
            (&sized, "[llm.srv.request]\nmax_tokens = 4000\n", Some(4000)),
            (&silent, "", None),
            (&large, "", Some(2048)),
            (&large, "shared_context = false\n", Some(4096)),
            (&huge, "", Some(4096)),
            (&huge, "shared_context = false\n", Some(4096)),
        ] {
            let opts = through(server, extra, 2, None);
            let budget = opts.backend.budget(opts.jobs);
            let (result, _) = ask(&opts.backend, &ten_lines(), budget);
            assert!(result.is_ok(), "{result:?}");
            let body = server.completions().last().expect("a call").json();
            assert_eq!(
                body.get("max_tokens").and_then(serde_json::Value::as_u64),
                cap,
                "{extra:?}"
            );
        }
    }

    fn track(language: &str, bcp47: Option<&str>) -> Track {
        Track {
            id: 0,
            number: 1,
            uid: 1,
            kind: TRACK_TYPE_SUBTITLE,
            codec: "S_TEXT/ASS".into(),
            name: None,
            language: language.into(),
            language_element: true,
            language_bcp47: bcp47.map(Into::into),
            default: true,
            forced: false,
            default_duration: None,
            codec_private: Vec::new(),
            encodings: Vec::new(),
            flags: crate::mkv::Flags::default(),
            video: None,
            audio: None,
        }
    }

    #[test]
    fn language_matches_by_iso_639_2_or_bcp_47() {
        let eng = ["eng".to_string()];
        assert!(wanted_language(&track("eng", None), &eng));
        assert!(wanted_language(&track("und", Some("en-US")), &eng));
        assert!(wanted_language(&track("und", Some("EN-us")), &eng));
        assert!(!wanted_language(&track("und", Some("und")), &eng));
        assert!(!wanted_language(&track("jpn", Some("ja")), &eng));
        assert!(wanted_language(
            &track("und", Some("und")),
            &["und".to_string()]
        ));
        // LanguageBCP47 overrides Language, which defaults to "eng" when absent.
        assert!(!wanted_language(&track("eng", Some("ja")), &eng));
    }

    #[test]
    fn an_unreadable_directory_is_skipped_with_a_warning() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return; // root reads a 0o000 directory anyway (CI containers run as root)
        }
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("locked")).unwrap();
        std::fs::write(d.join("a.mkv"), "").unwrap();
        let mode = |m| std::fs::Permissions::from_mode(m);
        std::fs::set_permissions(d.join("locked"), mode(0o000)).unwrap();
        let result = mkv_files(&[d.to_path_buf()]);
        std::fs::set_permissions(d.join("locked"), mode(0o755)).unwrap();
        let (found, warnings) = result.unwrap();
        assert_eq!(found, [d.join("a.mkv")]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("locked"), "{warnings:?}");
    }

    #[test]
    fn mkv_files_walks_sorted_skipping_hidden_and_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("Show/Specials")).unwrap();
        std::fs::create_dir_all(d.join(".Recycle.Bin")).unwrap();
        for f in [
            "Show/b.mkv",
            "Show/a.MKV",
            "Show/Specials/c.mkv",
            "Show/notes.txt",
            ".Recycle.Bin/x.mkv",
            "Show/.d.mkv",
        ] {
            std::fs::write(d.join(f), "").unwrap();
        }
        let (found, warnings) = mkv_files(&[d.to_path_buf()]).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let names: Vec<_> = found
            .iter()
            .map(|p| p.strip_prefix(d).unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Show/Specials/c.mkv", "Show/a.MKV", "Show/b.mkv"]);
        assert!(mkv_files(&[d.join("missing")]).is_err());
    }

    #[test]
    fn mkv_files_given_a_path_with_dot_or_dotdot_finds_the_same_files() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("x")).unwrap();
        std::fs::create_dir_all(d.join("Show/Specials")).unwrap();
        for f in ["Show/a.mkv", "Show/Specials/b.mkv"] {
            std::fs::write(d.join(f), "").unwrap();
        }
        let plain = mkv_files(&[d.join("Show")]).unwrap().0;
        assert_eq!(plain, [d.join("Show/Specials/b.mkv"), d.join("Show/a.mkv")]);
        for odd in [d.join("x/../Show"), d.join("Show/.")] {
            assert_eq!(mkv_files(&[odd]).unwrap().0, plain);
        }
        // A file named through `..` is recorded at its clean path.
        let file = mkv_files(&[d.join("x/../Show/a.mkv")]).unwrap().0;
        assert_eq!(file, [d.join("Show/a.mkv")]);
    }
}
