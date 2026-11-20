//! The LLM step: a backend is a command that reads the prompt on stdin and prints the reply on
//! stdout, or a server subcheck calls itself ([`Server`]). Presets cover claude and codex (gpt),
//! run as commands, and `ollama:<model>`, Ollama's server; an entry in the settings file or
//! `--llm-cmd` covers anything else. claude, codex, a server and a command that prints an OpenAI
//! chat completion give their reply as JSON, with what the call used.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::group;
use crate::settings::{Entry, EntryKind, MAX_TIMEOUT_MINUTES, Settings};
use crate::text::{Line, timestamp};

mod key;
mod server;
mod wait;
pub(crate) use server::same_model;
pub use server::{Context, Server};
pub use wait::{HOLD, Wait, WaitLines, Waits};

/// Stored with every scan and every LLM check, so findings can be traced to the prompt that
/// produced them.
pub const PROMPT_VERSION: u32 = 1;

/// One chunk's run limit unless the LLM's entry or `--llm-timeout` sets another
/// ([`Backend::timeout`]); a chunk that takes longer is killed, retried once, then failed.
pub const TIMEOUT: Duration = Duration::from_mins(10);

/// Lines per chunk when neither `--chunk-lines`, the LLM's entry nor a server's context says
/// otherwise, and the most a call's budget is fitted with ([`Budget::lines`]).
pub const CHUNK_LINES: usize = 400;

/// Tokens a prompt takes besides its subtitle lines: the instructions, the system message and the
/// chat template around them.
pub const PROMPT_TOKENS: u64 = 400;

/// Tokens one subtitle line takes in a prompt, counted cautiously, so a chunk sized by it fits.
pub const TOKENS_PER_LINE: u64 = 37;

/// LLM calls a scan runs at once, unless `-j` or the LLM's entry in the settings file says.
pub const JOBS: usize = 2;

/// The most room a reply is given unless the entry's `request` sets its own cap ([`Budget::new`]).
/// A reply to [`CHUNK_LINES`] lines names only the lines with a fix, so it needs far less, and many
/// hosted APIs refuse a call that lets its reply take more than they allow, however large the
/// context. A longer reply is cut off at it ([`TooBig::Reply`]) and its chunk split once.
pub const MAX_REPLY: u64 = 4096;

/// The tokens a prompt of `lines` subtitle lines takes, counted cautiously.
pub fn estimate(lines: usize) -> u64 {
    PROMPT_TOKENS.saturating_add((lines as u64).saturating_mul(TOKENS_PER_LINE))
}

/// The smallest share of a context whose [`Budget::lines`] are [`CHUNK_LINES`], the reply's room
/// being `max_tokens` as [`Budget::new`] takes it. With that room, it is a full chunk's
/// [`estimate`] `e` and the room beside it. With the quarter, the prompt keeps
/// `share - share / 4`, three quarters of the share rounded up, which first holds `e` at
/// `(4e - 1) / 3`; with the quarter bounded at [`MAX_REPLY`], it holds `e` from
/// `e + MAX_REPLY`, and whichever comes first is the smallest.
fn full_chunk_context(max_tokens: Option<u64>) -> u64 {
    let full = estimate(CHUNK_LINES);
    match max_tokens {
        Some(room) => full.saturating_add(room),
        None => ((4 * full - 1) / 3).min(full + MAX_REPLY),
    }
}

/// What one call may take of a server's context ([`Backend::budget`]): its share of the context,
/// and of that the room left for its reply, which the context holds too. The prompt gets the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    share: u64,
    /// At most `share`.
    reply: u64,
}

impl Budget {
    /// A call's budget in a share of `share` tokens. The reply's room is `max_tokens`, an entry's
    /// own cap on its replies, when it sets one, at most the whole share: the server lets a reply
    /// run that far, so the prompt fits in what is left. Else it is the last quarter of the share,
    /// at most [`MAX_REPLY`].
    pub fn new(share: u64, max_tokens: Option<u64>) -> Budget {
        let reply = max_tokens.map_or((share / 4).min(MAX_REPLY), |room| room.min(share));
        Budget { share, reply }
    }

    /// The tokens a call may take of the server's context, its prompt and its reply together.
    pub fn share(self) -> u64 {
        self.share
    }

    /// The tokens the prompt may take: the share less the reply's room. A prompt over it, even
    /// one within the share, leaves its reply less room than the call caps it at, and the two
    /// together outgrow the share: vLLM refuses such a call, and on a server whose calls share one
    /// cache it overflows the others'.
    pub fn prompt(self) -> u64 {
        self.share - self.reply
    }

    /// The most lines a chunk can hold: as many as its [`estimate`] fits in [`Budget::prompt`].
    /// At least 1, with no upper limit: a size set for the chunks is cut to it.
    pub fn fit(self) -> usize {
        let room = self.prompt().saturating_sub(PROMPT_TOKENS);
        usize::try_from(room / TOKENS_PER_LINE).map_or(usize::MAX, |n| n.max(1))
    }

    /// The lines a chunk is fitted with when nothing sets its size: [`Budget::fit`], at most
    /// [`CHUNK_LINES`].
    pub fn lines(self) -> usize {
        self.fit().min(CHUNK_LINES)
    }

    /// The most tokens a call's reply may take: the reply's room, at least 1, since a cap of 0
    /// leaves no room for any reply. A server whose calls share one cache, as llama-server's slots
    /// do, fails every call in flight once one reply outgrows its room, and a model that loops
    /// would run on until the context is full; capped, such a reply is cut off at its room
    /// ([`TooBig::Reply`]), and only its own chunk is the worse for it.
    pub fn cap(self) -> u64 {
        self.reply.max(1)
    }
}

/// How many of `jobs` calls at once share `server`'s context: all of them, unless its entry's
/// `shared_context` is `false`.
fn sharing(server: &Server, jobs: usize) -> u64 {
    if server.entry.shared_context {
        jobs.max(1) as u64
    } else {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backend {
    /// What a run records: `claude`, `gpt`, `ollama:<model>`, an entry's name, `<entry>@<model>`
    /// for a server (the entry's name alone while its model is not known), or `cmd:<command>`.
    /// An entry's name has no `@`, so the first `@` always ends it; a model may hold `@`, `/`
    /// or `:`.
    pub label: String,
    pub via: Via,
    /// Lines per chunk as the LLM's entry sets them, at most as many as fit
    /// ([`Backend::lines_per_chunk`]); `None` for the presets, `--llm-cmd` and an entry that does
    /// not say.
    pub chunk_lines: Option<usize>,
    pub jobs: usize,
    /// How long each call, and a model's load, may take: the entry's `timeout_minutes`, else
    /// [`TIMEOUT`].
    pub timeout: Duration,
}

/// What a backend's calls go through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    /// A command that reads the prompt on stdin and prints the reply as `output` says.
    Command { argv: Vec<String>, output: Output },
    /// An OpenAI-API server subcheck calls itself.
    Server(Server),
    /// A run's LLM that cannot be built again: why, as [`Backend::from_label`] says it. A call
    /// answers that, so a file that needs the LLM fails saying why rather than calling something
    /// the run did not use.
    Unknown(String),
}

/// How a backend prints its reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// The reply alone: any `--llm-cmd`, and a settings entry with `output = "text"` or no
    /// `output`.
    Text,
    /// `claude -p --output-format json`: one object, the reply in `result`, with the call's tokens
    /// and its price.
    Claude,
    /// `codex exec --json`: an event a line, the reply in the last agent message, the call's
    /// tokens in `turn.completed`. No price.
    Codex,
    /// An OpenAI chat completion (`/v1/chat/completions`): the reply in the first choice, the
    /// call's tokens in `usage`. No price.
    OpenAi,
}

/// The system prompt for presets that take one; the instructions themselves go on stdin.
const SYSTEM: &str = "You are a careful English proofreader. Follow the reply format exactly.";

/// Why a reply cut off at the server's length limit fails, whichever API it came through.
const CUT_OFF: &str = "the reply was cut off at the server's length limit";

impl Backend {
    /// `claude`, `gpt` or `ollama:<model>`.
    pub fn preset(name: &str) -> Result<Backend, String> {
        let (argv, output): (Vec<&str>, Output) = match name {
            // Without these flags a headless call loads the user's hooks, skills, CLAUDE.md and
            // memory into its context.
            "claude" => (
                vec![
                    "claude",
                    "-p",
                    "--model",
                    "sonnet",
                    "--output-format",
                    "json",
                    "--tools",
                    "",
                    "--setting-sources",
                    "",
                    "--disable-slash-commands",
                    "--strict-mcp-config",
                    "--no-session-persistence",
                    "--system-prompt",
                    SYSTEM,
                ],
                Output::Claude,
            ),
            // `--disable hooks`: the user's Codex hooks (tmux session tracking) would otherwise
            // fire on every chunk.
            "gpt" => (
                vec![
                    "codex",
                    "exec",
                    "--disable",
                    "hooks",
                    "--sandbox",
                    "read-only",
                    "--skip-git-repo-check",
                    "--ephemeral",
                    "--json",
                    "-",
                ],
                Output::Codex,
            ),
            _ => {
                let Some(model) = name.strip_prefix("ollama:").filter(|m| !m.is_empty()) else {
                    return Err(format!(
                        "unknown --llm {name:?}: use claude, gpt, ollama:<model>, or --llm-cmd"
                    ));
                };
                let host = std::env::var("OLLAMA_HOST").ok();
                return Ok(Backend::ollama(model, &server::ollama_url(host.as_deref())));
            }
        };
        Ok(Backend {
            label: name.to_string(),
            via: Via::Command {
                argv: argv.iter().map(ToString::to_string).collect(),
                output,
            },
            chunk_lines: None,
            jobs: JOBS,
            timeout: TIMEOUT,
        })
    }

    /// `ollama:<model>`: Ollama's server, its OpenAI API at `url` ([`server::ollama_url`]).
    fn ollama(model: &str, url: &str) -> Backend {
        let server = Server::ollama(model, url);
        Backend {
            label: server.label(),
            via: Via::Server(server),
            chunk_lines: None,
            jobs: JOBS,
            timeout: TIMEOUT,
        }
    }

    /// Any command, run through `sh -c`.
    pub fn shell(command: &str) -> Backend {
        Backend {
            label: format!("cmd:{command}"),
            via: Via::Command {
                argv: vec!["sh".into(), "-c".into(), command.into()],
                output: Output::Text,
            },
            chunk_lines: None,
            jobs: JOBS,
            timeout: TIMEOUT,
        }
    }

    /// The settings file's entry `name`: its command run as `--llm-cmd`'s is, or its server.
    /// Recorded by the name, and a server's with the model when the entry names one.
    pub fn entry(name: &str, entry: &Entry) -> Backend {
        let (label, via) = match &entry.kind {
            EntryKind::Command { command, output } => {
                let argv = Backend::shell(command).command_line();
                (
                    name.to_string(),
                    Via::Command {
                        argv,
                        output: *output,
                    },
                )
            }
            EntryKind::Server(server) => {
                let server = Server::new(name, server, server.model.as_deref());
                (server.label(), Via::Server(server))
            }
        };
        Backend {
            label,
            via,
            chunk_lines: entry.chunk_lines,
            jobs: entry.jobs.unwrap_or(JOBS),
            timeout: entry.timeout.unwrap_or(TIMEOUT),
        }
    }

    /// The backend with each call's limit `minutes` when that is `Some` (`--llm-timeout`, which
    /// beats the entry's `timeout_minutes`), and as it was when it is `None`. The minutes are kept
    /// within 1 to [`MAX_TIMEOUT_MINUTES`], as the settings file keeps an entry's: more would
    /// overflow the limit, or the clock it is added to when a call starts, and none would fail
    /// every call as it starts.
    #[must_use]
    pub fn limited_to(self, minutes: Option<u64>) -> Backend {
        match minutes {
            Some(minutes) => Backend {
                timeout: Duration::from_mins(minutes.clamp(1, MAX_TIMEOUT_MINUTES)),
                ..self
            },
            None => self,
        }
    }

    /// What `--llm` names: a preset, else an entry in `settings`. The file refuses the presets'
    /// names, and an entry's name has no `:`, so neither can hide the other.
    pub fn named(name: &str, settings: &Settings) -> Result<Backend, String> {
        if let Ok(preset) = Backend::preset(name) {
            return Ok(preset);
        }
        match settings.entries.get(name) {
            Some(entry) => Ok(Backend::entry(name, entry)),
            None => Err(format!(
                "unknown --llm {name:?}: use claude, gpt, ollama:<model>, an entry in {}, or \
                 --llm-cmd",
                settings.path.display()
            )),
        }
    }

    /// The backend a run records, by its label; an entry as `settings` has it now. An entry that
    /// is gone, or is no longer what the run called, is refused, saying which.
    pub fn from_label(label: &str, settings: &Settings) -> Result<Backend, String> {
        if let Some(command) = label.strip_prefix("cmd:") {
            return Ok(Backend::shell(command));
        }
        // Before the split at `@`: an ollama model may hold one.
        if let Ok(preset) = Backend::preset(label) {
            return Ok(preset);
        }
        let path = settings.path.display();
        let (name, model) = match label.split_once('@') {
            Some((name, model)) => (name, Some(model)),
            None => (label, None),
        };
        // The settings file refuses an empty model, so a run never records one.
        if model == Some("") {
            return Err(format!("{label}, which names no model"));
        }
        let Some(entry) = settings.entries.get(name) else {
            return Err(format!(
                "{label}, which this subcheck does not know and {path} does not name"
            ));
        };
        match (&entry.kind, model) {
            (EntryKind::Command { .. }, None) => Ok(Backend::entry(name, entry)),
            (EntryKind::Command { .. }, Some(_)) => {
                Err(format!("{label}, which {path} now has as a command"))
            }
            // A run through a server records its model, so a bare name was a command's.
            (EntryKind::Server(_), None) => {
                Err(format!("{label}, which {path} now has as a server"))
            }
            (EntryKind::Server(server), Some(model)) => match &server.model {
                Some(asked) if !same_model(asked, model) => Err(format!(
                    "{label}, but {path} now has {name} ask for {asked:?}"
                )),
                _ => Ok(Backend {
                    label: label.to_string(),
                    via: Via::Server(Server::new(name, server, Some(model))),
                    ..Backend::entry(name, entry)
                }),
            },
        }
    }

    /// The backend made ready for calls: for a server, its key fetched, `GET <url>/models` asked,
    /// the model settled (the one the entry or the run names, which the server must serve, a name
    /// without a tag as its `:latest`, else the only one it serves) and the label with it,
    /// `<name>@<model>` (`ollama:<model>` keeps its own), as the run records it, and
    /// its [`context`](Backend::context) read, from the list, else, for `ollama:<model>` and for an
    /// entry whose url is plain `http://` and ends `/v1`, from Ollama's
    /// `GET <url without /v1>/api/ps`; with the warnings to show. A command, or an LLM
    /// that cannot be built again, comes back as it is, asking nothing. Every error starts with the
    /// label as it was.
    ///
    /// A new scan runs it before it reads any file, since the label it records waits on the
    /// model; a resumed run only once a file needs the LLM, so a resume with nothing for the LLM
    /// to do needs no server up; a recheck on its job's thread, so a server or a key command that
    /// hangs freezes neither the screen nor the window. It sends only `GET`s, so it never makes a
    /// server load a model; [`Backend::sized`] does.
    ///
    /// A server reports through `waits` its key command while it runs, then its check, each
    /// ended on every path; a command, or an LLM that cannot be built again, reports nothing.
    pub fn ready(self, waits: Waits<'_>) -> Result<(Backend, Vec<String>), String> {
        match self.via {
            Via::Server(server) => {
                let (server, warnings) = server
                    .ready(waits)
                    .map_err(|why| format!("{}: {why}", self.label))?;
                let ready = Backend {
                    label: server.label(),
                    via: Via::Server(server),
                    ..self
                };
                Ok((ready, warnings))
            }
            via => Ok((Backend { via, ..self }, Vec::new())),
        }
    }

    /// How many tokens a server's context holds, as far as it says; `None` for a backend that is
    /// no server.
    pub fn context(&self) -> Option<Context> {
        match &self.via {
            Via::Server(server) => Some(server.context()),
            _ => None,
        }
    }

    /// What each of `jobs` calls at once may take of a server's context ([`Budget`]), when the
    /// server says how many tokens it holds; `None` for any other context, and for a backend that
    /// is no server. The context is divided between the calls unless the entry's `shared_context`
    /// is `false`, as `ollama:<model>`'s is: a server whose calls share one cache, as
    /// llama-server's slots do, fails calls that together outgrow it.
    pub fn budget(&self, jobs: usize) -> Option<Budget> {
        let Via::Server(server) = &self.via else {
            return None;
        };
        match server.context() {
            Context::Tokens(tokens) => Some(Budget::new(
                tokens / sharing(server, jobs),
                server.own_cap().map(|(_, cap)| cap),
            )),
            Context::NotLoaded | Context::Unknown => None,
        }
    }

    /// Lines per chunk with `jobs` calls at once: `forced` (`--chunk-lines`), else the entry's
    /// `chunk_lines`, at most as many as fit each call's budget ([`Backend::budget`],
    /// [`Budget::fit`]) when the server says its size, and as set when it does not; with neither
    /// set, as many as fit, at most [`CHUNK_LINES`] ([`Budget::lines`]), else [`CHUNK_LINES`].
    pub fn lines_per_chunk(&self, jobs: usize, forced: Option<usize>) -> usize {
        // Cut when the chunks are made, not when they are sent, so the parts of a size set too
        // big go to the workers side by side.
        match (forced.or(self.chunk_lines), self.budget(jobs)) {
            (Some(lines), Some(budget)) => lines.min(budget.fit()),
            (Some(lines), None) => lines,
            (None, Some(budget)) => budget.lines(),
            (None, None) => CHUNK_LINES,
        }
    }

    /// The warning for a server that did not say its context size and whose entry sets no
    /// `chunk_lines`: its chunks are [`CHUNK_LINES`] lines, more than a small context holds, so it
    /// names the smallest context that holds one beside its reply's room ([`Budget::new`]) for each
    /// of `jobs` calls at once (for one call, with `shared_context = false` and for
    /// `ollama:<model>`), in k, rounded up, and what sets the size: the entry's `chunk_lines`, or
    /// `--chunk-lines` for `ollama:<model>`, which has no entry. `None` for any other backend: a
    /// model Ollama has not loaded yet gets its size when a scan loads it ([`Backend::sized`]).
    pub fn size_warning(&self, jobs: usize) -> Option<String> {
        let Via::Server(server) = &self.via else {
            return None;
        };
        if server.context() != Context::Unknown || self.chunk_lines.is_some() {
            return None;
        }
        let own_cap = server.own_cap().map(|(_, cap)| cap);
        let k = (full_chunk_context(own_cap) * sharing(server, jobs)).div_ceil(1024);
        Some(format!(
            "{} did not say its context size; set {} if it holds under {k}k tokens",
            server.name,
            server.size_setting()
        ))
    }

    /// The warning for an entry whose own cap on its replies ([`Budget::new`]) leaves a call's
    /// prompt no room for one line's [`estimate`]: every chunk is a single line ([`Budget::fit`]),
    /// a call to each line, whatever sets the chunks' size. It names the cap and the call's share,
    /// and, while the calls share the context, what gives each call more of it. `None` for any
    /// other backend, and while the context is not known.
    pub fn room_warning(&self, jobs: usize) -> Option<String> {
        let Via::Server(server) = &self.via else {
            return None;
        };
        let (field, cap) = server.own_cap()?;
        let budget = self.budget(jobs)?;
        if budget.prompt() >= estimate(1) {
            return None;
        }
        let fix = if sharing(server, jobs) > 1 {
            "lower it, or set shared_context = false or fewer jobs"
        } else {
            "lower it"
        };
        Some(format!(
            "{}'s request.{field} ({cap}) leaves no room for lines in a call's {} tokens, so each \
             line is sent alone; {fix}",
            server.name,
            budget.share()
        ))
    }

    /// The backend with its context known where a load tells it: a server whose context is
    /// [`Context::NotLoaded`] has Ollama load its model (`POST <url without /v1>/api/generate`,
    /// waiting up to [`Backend::timeout`]), then `/api/ps` is asked again: [`Context::Tokens`], else
    /// [`Context::Unknown`]. Anything else comes back as it is, asking nothing. A load takes the
    /// model's memory on the server, which is why [`Backend::ready`] never makes one. Every error
    /// starts with the label. A load is reported through `waits` until it and the second
    /// `/api/ps` are over, or it has failed; nothing else is.
    pub fn sized(self, waits: Waits<'_>) -> Result<Backend, String> {
        match self.via {
            Via::Server(server) => {
                let server = server
                    .sized(self.timeout, waits)
                    .map_err(|why| format!("{}: {why}", self.label))?;
                Ok(Backend {
                    via: Via::Server(server),
                    ..self
                })
            }
            via => Ok(Backend { via, ..self }),
        }
    }

    /// What a run's header records as the backend's command: its argv, or the URL a server's calls
    /// go to; nothing for an LLM that cannot be built again.
    pub fn command_line(&self) -> Vec<String> {
        match &self.via {
            Via::Command { argv, .. } => argv.clone(),
            Via::Server(server) => vec![server.chat_url()],
            Via::Unknown(_) => Vec::new(),
        }
    }

    /// One call: the prompt in, the reply out, read as the backend gives it. A server's call takes
    /// `budget` of its context when that is `Some` ([`Backend::budget`]): its reply is capped at
    /// [`Budget::cap`], and a prompt that came to 95% of [`Budget::share`] or more, and over
    /// [`Budget::prompt`], is [`TooBig::Prompt`]. A command has neither. A server reports
    /// through `waits` a key command its 401 runs; a command, or an LLM that cannot be built
    /// again, reports nothing.
    pub fn call(
        &self,
        prompt: &str,
        timeout: Duration,
        budget: Option<Budget>,
        waits: Waits<'_>,
    ) -> Answer {
        match &self.via {
            Via::Command { argv, output } => call_command(argv, *output, prompt, timeout),
            Via::Server(server) => server.call(prompt, timeout, budget, waits),
            Via::Unknown(why) => Answer {
                reply: Err(why.clone()),
                used: Used::Untracked,
                too_big: None,
            },
        }
    }
}

/// One call of `argv`, its reply read as `output` says.
fn call_command(argv: &[String], output: Output, prompt: &str, timeout: Duration) -> Answer {
    let failed = match run(argv, prompt, timeout, without_terminal) {
        Ok(stdout) => return output.read(&stdout),
        Err(failed) => failed,
    };
    let said = output.read_json(&failed.stdout);
    // The exit status and stderr, then the backend's reason when its JSON gives one: claude out of
    // its session says why there alone, with nothing on stderr.
    let reply = match said.as_ref().and_then(|s| s.reason.as_deref()) {
        Some(why) => format!("{}; {} said: {why:.200}", failed.message, output.name()),
        None => failed.message,
    };
    // Every call that started counts, with what its JSON says it used, if anything.
    let used = match (output, said.and_then(|s| s.usage)) {
        (Output::Text, _) => Used::Untracked,
        _ if !failed.started => Used::Untracked,
        (_, Some(usage)) => Used::Reported(usage),
        (_, None) => Used::Unreported,
    };
    Answer {
        reply: Err(reply),
        used,
        too_big: None,
    }
}

/// What one call used, as its backend reported it. The store keeps it as a `usage` row
/// ([`crate::store::LlmCall`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    /// All of its input, read from a cache or not. (claude's own `input_tokens` leaves out the
    /// cached part; this is that plus both cache counts.)
    pub input_tokens: u64,
    /// With any the model spent thinking.
    pub output_tokens: u64,
    /// Of the input, how much was read from the backend's prompt cache, and how much written to
    /// it, when it says.
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    /// The call's price at the provider's API rates, when the backend says (claude does, codex
    /// does not). On a subscription that is not what is billed.
    pub cost_usd: Option<f64>,
}

/// What a call used, as far as can be told.
#[derive(Debug, Clone, PartialEq)]
pub enum Used {
    Reported(Usage),
    /// It started, and its backend said nothing of what it used: killed at the timeout, printing
    /// nothing or something other than its JSON, or JSON with no usage in it. It may have cost.
    Unreported,
    /// Nothing to count: a backend whose output is [`Output::Text`], which never reports, or a
    /// call that never started.
    Untracked,
}

/// How a call's chunk was too big for it: a smaller one may get a whole reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooBig {
    /// The prompt did not fit the server's context: the server refused it, or its
    /// `prompt_tokens` came to 95% or more of the call's share of the context ([`Budget::share`])
    /// and over the room the budget left the prompt ([`Budget::prompt`]), or to 95% or more of
    /// the whole context for a call with no budget, where the server may have cut it.
    Prompt,
    /// The reply was cut off at the length limit.
    Reply,
}

/// A call's reply, read from what the backend printed.
#[derive(Debug, PartialEq)]
pub struct Answer {
    /// The reply for [`parse`], or why there is none.
    pub reply: Result<String, String>,
    pub used: Used,
    /// `Some` only with a `reply` that is an error saying why.
    pub too_big: Option<TooBig>,
}

/// What a backend's JSON says of a call.
struct Said {
    /// The reply; with none, the error for a backend that exited 0.
    reply: Result<String, String>,
    /// Why it failed, in its own words, when it says.
    reason: Option<String>,
    usage: Option<Usage>,
    too_big: Option<TooBig>,
}

#[derive(Deserialize)]
struct ClaudeReply {
    #[serde(default)]
    is_error: bool,
    result: Option<String>,
    subtype: Option<String>,
    total_cost_usd: Option<f64>,
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)] // claude's names
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

/// The events of `codex exec --json` that are read; the rest are passed over.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum CodexEvent {
    #[serde(rename = "item.completed")]
    ItemCompleted { item: CodexItem },
    #[serde(rename = "turn.completed")]
    TurnCompleted { usage: CodexUsage },
    #[serde(rename = "turn.failed")]
    TurnFailed { error: CodexError },
    /// Not always the end, so no reason for one: codex says so when it reconnects, too.
    #[serde(rename = "error")]
    Error(CodexError),
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct CodexItem {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)] // codex's names
struct CodexUsage {
    /// Cached input included, as OpenAI counts it.
    #[serde(default)]
    input_tokens: u64,
    cached_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    /// Reasoning included.
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Deserialize)]
struct CodexError {
    message: String,
}

impl CodexError {
    /// Its message, or the API's own where codex passes the API's error on as JSON
    /// (`{"type":"error","status":400,"error":{"message":...}}` for a model the account cannot use);
    /// `None` when it says nothing.
    fn said(self) -> Option<String> {
        #[derive(Deserialize)]
        struct Api {
            error: CodexError,
        }
        let message = match serde_json::from_str::<Api>(&self.message) {
            Ok(api) if !api.error.message.trim().is_empty() => api.error.message,
            _ => self.message,
        };
        let message = message.trim();
        (!message.is_empty()).then(|| message.to_string())
    }
}

impl Output {
    /// The reply and what the call used, from what the backend printed.
    pub fn read(self, stdout: &str) -> Answer {
        let what = match self {
            Output::Text => {
                return Answer {
                    reply: Ok(stdout.to_string()),
                    used: Used::Untracked,
                    too_big: None,
                };
            }
            Output::Claude => "claude's JSON",
            Output::Codex => "codex's events",
            Output::OpenAi => "an OpenAI chat completion",
        };
        self.read_json(stdout).map_or_else(
            || {
                let first = stdout.trim().lines().next().unwrap_or("(empty)");
                Answer {
                    reply: Err(format!("reply is not {what}: {first:.80}")),
                    used: Used::Unreported,
                    too_big: None,
                }
            },
            |said| Answer {
                reply: said.reply,
                used: said.usage.map_or(Used::Unreported, Used::Reported),
                too_big: said.too_big,
            },
        )
    }

    /// The backend, as an error names it.
    fn name(self) -> &'static str {
        match self {
            Output::Text | Output::OpenAi => "the command",
            Output::Claude => "claude",
            Output::Codex => "codex",
        }
    }

    /// As [`Output::read`], for a backend that prints JSON; `None` for text, or output that is
    /// not this backend's JSON at all.
    fn read_json(self, stdout: &str) -> Option<Said> {
        match self {
            Output::Text => None,
            Output::Claude => read_claude(stdout),
            Output::Codex => read_codex(stdout),
            Output::OpenAi => read_openai(stdout),
        }
    }
}

#[derive(Deserialize)]
struct OpenAiReply {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
    error: Option<OpenAiError>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: Option<OpenAiMessage>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    prompt_tokens_details: Option<OpenAiPromptDetails>,
}

#[derive(Deserialize)]
struct OpenAiPromptDetails {
    cached_tokens: Option<u64>,
}

/// `error` as servers send it: an object with a message, as OpenAI's does, or a message alone.
#[derive(Deserialize)]
#[serde(untagged)]
enum OpenAiError {
    Said(String),
    Object { message: Option<String> },
}

impl OpenAiError {
    fn message(self) -> Option<String> {
        match self {
            OpenAiError::Said(message)
            | OpenAiError::Object {
                message: Some(message),
            } => Some(message),
            OpenAiError::Object { message: None } => None,
        }
    }
}

/// Whether `cost` can be a price: finite, and not below 0. In claude's reply one that is not is
/// taken as no price at all.
pub fn is_price(cost: f64) -> bool {
    cost.is_finite() && cost >= 0.0
}

fn read_claude(stdout: &str) -> Option<Said> {
    let r: ClaudeReply = serde_json::from_str(stdout).ok()?;
    let says = |why: &String| !why.trim().is_empty();
    let usage = (r.usage.is_some() || r.total_cost_usd.is_some()).then(|| {
        let u = r.usage.as_ref();
        let read = u.and_then(|u| u.cache_read_input_tokens);
        let written = u.and_then(|u| u.cache_creation_input_tokens);
        Usage {
            input_tokens: u
                .map_or(0, |u| u.input_tokens)
                .saturating_add(read.unwrap_or(0))
                .saturating_add(written.unwrap_or(0)),
            output_tokens: u.map_or(0, |u| u.output_tokens),
            cache_read_tokens: read,
            cache_write_tokens: written,
            cost_usd: r.total_cost_usd.filter(|&c| is_price(c)),
        }
    });
    let (reply, reason) = if r.is_error {
        // Its result, else its subtype, which says `success` even of some errors.
        let subtype = r.subtype.filter(|s| says(s) && s != "success");
        let reason = r.result.filter(says).or(subtype);
        let reply = match &reason {
            Some(why) => Err(format!("claude reported an error: {why:.200}")),
            None => Err("claude reported an error".into()),
        };
        (reply, reason)
    } else {
        // As `claude -p` prints it without --output-format.
        let reply = r.result.map(|result| format!("{result}\n"));
        (
            reply.ok_or_else(|| "claude's reply has no result".into()),
            None,
        )
    };
    Some(Said {
        reply,
        reason,
        usage,
        too_big: None,
    })
}

/// `None` when `stdout` is not a JSON object.
fn read_openai(stdout: &str) -> Option<Said> {
    let r: OpenAiReply = serde_json::from_str(stdout).ok()?;
    let usage = r.usage.map(|u| Usage {
        input_tokens: u.prompt_tokens.unwrap_or(0),
        output_tokens: u.completion_tokens.unwrap_or(0),
        cache_read_tokens: u.prompt_tokens_details.and_then(|d| d.cached_tokens),
        cache_write_tokens: None,
        cost_usd: None,
    });
    let reason = r
        .error
        .and_then(OpenAiError::message)
        .filter(|m| !m.trim().is_empty());
    let (stopped, content) = match r.choices.into_iter().next() {
        Some(c) => (c.finish_reason, c.message.and_then(|m| m.content)),
        None => (None, None),
    };
    // A reply the server cut short is no whole reply: taken as one, its chunk would be recorded as
    // checked with nothing found in lines it never reached. Servers word a normal end many ways
    // (`eos_token`, `stop_sequence`, `STOP`, an empty string), so only the reasons known to cut a
    // reply fail.
    let (reply, too_big) = classify(
        reason.clone(),
        stopped.as_deref(),
        &["content_filter"],
        content,
    );
    Some(Said {
        reply,
        reason,
        usage,
        too_big,
    })
}

/// What a chat reply comes to, from what the server said of it: its `error`, when that has any
/// text; else a reply cut off at the length limit, when it `stopped` for `length`
/// ([`TooBig::Reply`]); else one cut short, when it stopped for a reason in `early`; else its
/// `content` with a newline after it, as claude's reader ends its reply. An OpenAI chat completion
/// and Ollama's own chat reply are both read with it, so the two read a reply alike.
fn classify(
    error: Option<String>,
    stopped: Option<&str>,
    early: &[&str],
    content: Option<String>,
) -> (Result<String, String>, Option<TooBig>) {
    if let Some(why) = error.filter(|e| !e.trim().is_empty()) {
        return (Err(format!("the reply is an error: {why:.200}")), None);
    }
    match stopped {
        Some("length") => (Err(CUT_OFF.into()), Some(TooBig::Reply)),
        Some(reason) if early.contains(&reason) => {
            (Err(format!("the reply stopped early: {reason:.200}")), None)
        }
        _ => (
            content
                .map(|c| format!("{c}\n"))
                .ok_or_else(|| "the reply has no content".into()),
            None,
        ),
    }
}

/// `None` when no line is an event.
fn read_codex(stdout: &str) -> Option<Said> {
    let mut seen = false;
    let mut message = None;
    let mut failed = None;
    let mut error = None;
    let mut usage: Option<Usage> = None;
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<CodexEvent>(line) else {
            continue;
        };
        seen = true;
        match event {
            CodexEvent::ItemCompleted { item } if item.kind == "agent_message" => {
                message = item.text;
            }
            CodexEvent::TurnCompleted { usage: u } => {
                let sum = usage.get_or_insert_with(Usage::default);
                sum.input_tokens = sum.input_tokens.saturating_add(u.input_tokens);
                sum.output_tokens = sum.output_tokens.saturating_add(u.output_tokens);
                let add = |a: Option<u64>, b: Option<u64>| match (a, b) {
                    (None, None) => None,
                    _ => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
                };
                sum.cache_read_tokens = add(sum.cache_read_tokens, u.cached_input_tokens);
                sum.cache_write_tokens = add(sum.cache_write_tokens, u.cache_write_input_tokens);
            }
            CodexEvent::TurnFailed { error: e } => failed = e.said().or(failed),
            CodexEvent::Error(e) => error = e.said().or(error),
            CodexEvent::ItemCompleted { .. } | CodexEvent::Other => {}
        }
    }
    // Its turn's failure, else its last error.
    let reason = failed.or(error);
    let reply = match (message, &reason) {
        // As `codex exec` prints its final message without --json.
        (Some(text), _) => Ok(format!("{text}\n")),
        (None, Some(why)) => Err(format!("codex: {why:.200}")),
        (None, None) => Err("codex gave no message".into()),
    };
    seen.then_some(Said {
        reply,
        reason,
        usage,
        too_big: None,
    })
}

/// What a run's LLM calls used, summed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spent {
    pub calls: usize,
    /// Of them, those whose backend said nothing of what they used: the sums leave them out.
    pub unreported: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// The calls that reported a price, and its sum.
    pub priced: usize,
    pub cost_usd: f64,
}

impl Spent {
    /// One call, with what it used if its backend said.
    pub fn add(&mut self, usage: Option<&Usage>) {
        self.calls += 1;
        let Some(u) = usage else {
            self.unreported += 1;
            return;
        };
        self.input_tokens = self.input_tokens.saturating_add(u.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(u.output_tokens);
        if let Some(cost) = u.cost_usd {
            self.priced += 1;
            self.cost_usd += cost;
        }
    }

    /// `98 calls`, `1.23M in / 31.2k out tokens`, and `$3.21 at API prices` when a call had a
    /// price. Calls that reported nothing are said (`98 calls (2 reported no usage)`), and any
    /// call without a price makes the price a floor (`at least $3.21`). Nothing with no calls.
    pub fn parts(&self) -> Vec<String> {
        self.calls_part()
            .into_iter()
            .chain(self.tokens_part())
            .chain(self.price_part())
            .collect()
    }

    /// `98 calls`, `98 calls (2 reported no usage)`, `98 calls (none reported usage)`; `None` with
    /// no calls.
    pub fn calls_part(&self) -> Option<String> {
        let calls = match self.calls {
            0 => return None,
            1 => "1 call".to_string(),
            n => format!("{n} calls"),
        };
        Some(match self.unreported {
            0 => calls,
            n if n == self.calls => format!("{calls} (none reported usage)"),
            n => format!("{calls} ({n} reported no usage)"),
        })
    }

    /// `1.23M in / 31.2k out tokens`; `None` when no call reported.
    pub fn tokens_part(&self) -> Option<String> {
        (self.calls > self.unreported).then(|| {
            format!(
                "{} in / {} out tokens",
                tokens(self.input_tokens),
                tokens(self.output_tokens)
            )
        })
    }

    /// `$3.21 at API prices`, `at least` that when any call has no price (it reported none, or
    /// nothing at all); `None` when no call has one.
    pub fn price_part(&self) -> Option<String> {
        if self.priced == 0 {
            return None;
        }
        let floor = if self.priced < self.calls {
            "at least "
        } else {
            ""
        };
        Some(format!("{floor}{} at API prices", dollars(self.cost_usd)))
    }

    /// `LLM: ` and the [parts](Spent::parts) joined by `sep`; `None` when no call was recorded (a
    /// run from before calls were, or a backend that reports nothing: `--llm-cmd`).
    pub fn line(&self, sep: &str) -> Option<String> {
        let parts = self.parts();
        (!parts.is_empty()).then(|| format!("LLM: {}", parts.join(sep)))
    }
}

/// A token count to three figures: `980`, `31.2k`, `1.23M`.
fn tokens(n: u64) -> String {
    // A count that rounds to 1000 of a unit is shown in the next.
    let (scale, unit): (u128, &str) = match n {
        0..1_000 => return n.to_string(),
        1_000..999_500 => (1_000, "k"),
        999_500..999_500_000 => (1_000_000, "M"),
        _ => (1_000_000_000, "G"),
    };
    let n = u128::from(n);
    // Decimals for three figures, judged on the value as it will round.
    let rounded = |decimals: u32| {
        let ten = 10u128.pow(decimals);
        (n * ten + scale / 2) / scale
    };
    let hundredths = rounded(2);
    if hundredths < 1_000 {
        return format!("{}.{:02}{unit}", hundredths / 100, hundredths % 100);
    }
    let tenths = rounded(1);
    if tenths < 1_000 {
        return format!("{}.{}{unit}", tenths / 10, tenths % 10);
    }
    format!("{}{unit}", rounded(0))
}

/// Dollars to the cent, or to two figures under ten cents, judged as they round: `$3.21`,
/// `$0.10`, `$0.013`, `$0.0025`; to the millionth at most, so `<$0.000001` below it.
fn dollars(x: f64) -> String {
    if x <= 0.0 {
        return format!("${x:.2}");
    }
    let mut shown = String::new();
    for decimals in 2..=6 {
        shown = format!("{x:.decimals$}");
        // Its figures: what is left after the leading zeros and the point.
        if shown.trim_start_matches(['0', '.']).len() >= 2 {
            break;
        }
    }
    if shown.trim_start_matches(['0', '.']).is_empty() {
        return "<$0.000001".into();
    }
    format!("${shown}")
}

const INSTRUCTIONS: &str = "\
You are proofreading English subtitles for spelling mistakes.

Each line below is one subtitle: an ID, its start time, its style in parentheses, then the text.

Report only:
- misspelled words and typos (\"befor\" for \"before\", \"teh\" for \"the\");
- a real word used in place of the one meant (\"trough\" where \"through\" was meant, \"your\" for \"you're\");
- accidentally doubled or missing words.

Do not report:
- names, honorifics (-san, -chan, -senpai, Sensei) or any other Japanese, romanized or not;
- deliberate stutters, cut-off words and sounds (\"S-Such\", \"Mm-hmm\", \"Whoa\");
- slang, dialect, informal grammar, style, punctuation or capitalization;
- British versus American spelling;
- anything you are not sure is a mistake.

Some lines come from karaoke effects and may be fragments; skip any you cannot read as English.

Reply with one line per mistake and nothing else, fields separated by a TAB:
L<ID><TAB><the whole line, corrected><TAB><a few words on what was wrong>
If there are no mistakes, reply with the single word NONE.

Lines:
";

const CLOSING: &str = "\n(End of lines. Reply only in the format above, or NONE.)\n";

/// The prompt for one chunk. Line IDs are the chunk's own, `L1` to `Ln` in order.
pub fn prompt(lines: &[&Line]) -> String {
    let mut out = String::from(INSTRUCTIONS);
    for (i, line) in lines.iter().enumerate() {
        let _ = writeln!(
            out,
            "L{} [{}] ({}) {}",
            i + 1,
            timestamp(line.start_ms()),
            line.style,
            line.text
        );
    }
    out.push_str(CLOSING);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// Index into the chunk's lines.
    pub index: usize,
    pub corrected: String,
    pub reason: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub suggestions: Vec<Suggestion>,
    /// Finding lines naming an ID the chunk does not have.
    pub unknown_ids: usize,
    /// Findings whose correction equals the original.
    pub no_ops: usize,
}

/// Read a reply leniently: `L<ID>` first, then the corrected line and the reason separated by a
/// TAB. A model that echoes the line's `[time] (style)` prefix gets it stripped (gemma4 does).
/// A reply with neither a finding line nor `NONE` is an error: that is what a backend that only
/// "thought", or printed something else entirely, looks like.
pub fn parse(reply: &str, lines: &[&Line]) -> Result<Reply, String> {
    let mut out = Reply::default();
    let mut saw_none = false;
    let mut saw_finding = false;
    for raw in reply.lines() {
        let raw = raw.trim();
        if is_none(raw) {
            saw_none = true;
            continue;
        }
        let Some(rest) = raw.strip_prefix('L') else {
            continue;
        };
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let Ok(id) = rest[..digits].parse::<usize>() else {
            continue;
        };
        saw_finding = true;
        let Some(line) = id.checked_sub(1).and_then(|i| lines.get(i)) else {
            out.unknown_ids += 1;
            continue;
        };
        let rest = rest[digits..].trim_start_matches([':', ' ', '\t']);
        let mut fields = rest.splitn(2, '\t');
        let corrected = strip_echo(fields.next().unwrap_or_default().trim(), line);
        let reason = fields.next().unwrap_or_default().trim().replace('\t', " ");
        if corrected.is_empty() || corrected == line.text {
            out.no_ops += 1;
            continue;
        }
        out.suggestions.push(Suggestion {
            index: id - 1,
            corrected: corrected.to_string(),
            reason,
        });
    }
    if !saw_finding && !saw_none {
        let first = reply.trim().lines().next().unwrap_or("(empty)");
        return Err(format!("reply has no findings and no NONE: {first:.80}"));
    }
    Ok(out)
}

/// Whether a reply line says "no mistakes": the word NONE alone, or dressed in punctuation or
/// markdown ("NONE.", "**NONE**", "None - no mistakes found."), as small models write it. A word
/// that merely starts with it ("Nonetheless") is not NONE.
fn is_none(line: &str) -> bool {
    let word = line.trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
    word.get(..4)
        .is_some_and(|w| w.eq_ignore_ascii_case("none"))
        && !word[4..].starts_with(|c: char| c.is_ascii_alphanumeric())
}

/// The corrected text without an echoed `[0:02:45.06] (OP en) ` prefix.
fn strip_echo<'a>(s: &'a str, line: &Line) -> &'a str {
    let mut s = s;
    if let Some(rest) = s.strip_prefix('[')
        && let Some(end) = rest.find(']')
        && rest[..end]
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b':' || b == b'.')
    {
        s = rest[end + 1..].trim_start();
    }
    let style = format!("({})", line.style);
    s.strip_prefix(style.as_str()).map_or(s, str::trim_start)
}

/// After the command exits, how long its output may take to end. Something it left running in the
/// background and holding stdout gets this long, then its process group is killed: the reply is
/// already complete by then. Something that left the group, out of reach of the kill, is left
/// holding its pipe: what came by then is the output.
const GRACE: Duration = Duration::from_secs(2);

/// After the group is killed, how long the readers have to take what it wrote before it went.
const DRAIN: Duration = Duration::from_millis(200);

/// Why a command gave no reply, and what it printed on stdout all the same.
#[derive(Debug)]
struct Failed {
    message: String,
    stdout: String,
    /// It started, so it may have cost.
    started: bool,
    /// The code it exited with, when it ran to an exit.
    code: Option<i32>,
}

impl Failed {
    /// Nothing on stdout: it never `started`, or was killed.
    fn with_nothing(message: String, started: bool) -> Failed {
        Failed {
            message,
            stdout: String::new(),
            started,
            code: None,
        }
    }
}

/// Run `argv` with `input` on stdin, returning stdout; an error names the exit status or the
/// timeout, plus the last line of stderr, and keeps what the command printed on stdout.
///
/// `spawn` starts the command in its own process group ([`without_terminal`] for an LLM command
/// and a key command alike; [`group::spawn`] is what the tests of `run` itself pass, which need
/// the group alone), so a timeout kills everything it started (a wrapper script's children, or
/// what dash forks for `sh -c`), not just the direct child. That takes it out of the terminal's
/// foreground group, where Ctrl-C would reach it, so when SIGINT, SIGTERM or SIGHUP ends
/// subcheck, [`group`] kills every group still running.
fn run(
    argv: &[String],
    input: &str,
    timeout: Duration,
    spawn: fn(&mut Command) -> io::Result<(Child, group::Registered)>,
) -> Result<String, Failed> {
    let Some((program, rest)) = argv.split_first() else {
        return Err(Failed::with_nothing("empty command".into(), false));
    };
    let (mut child, _running) = spawn(
        Command::new(program)
            .args(rest)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .map_err(|e| Failed::with_nothing(format!("cannot run {program}: {e}"), false))?;
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let input = input.to_string();
    // Written from its own thread: a child that replies before reading all its input would
    // otherwise deadlock against a full pipe.
    let writer = thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    // Each pipe is read into a buffer kept here, so that what came can be taken without waiting
    // for the read to end, which something holding the pipe open puts off. Once `run` has
    // returned, the buffer with it, what comes is read and dropped.
    let reader = |mut pipe: Box<dyn Read + Send>| {
        let got = Arc::new(Mutex::new(Vec::new()));
        let into: Weak<Mutex<Vec<u8>>> = Arc::downgrade(&got);
        let reading = thread::spawn(move || {
            let mut buf = [0; 8192];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Some(into) = into.upgrade() {
                            into.lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .extend_from_slice(&buf[..n]);
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        });
        (reading, got)
    };
    let (stdout, out) = reader(Box::new(child.stdout.take().expect("stdout was piped")));
    let (stderr, err) = reader(Box::new(child.stderr.take().expect("stderr was piped")));
    let take = |got: &Mutex<Vec<u8>>| {
        let got = std::mem::take(&mut *got.lock().unwrap_or_else(PoisonError::into_inner));
        String::from_utf8_lossy(&got).into_owned()
    };
    // The last line it said on stderr, after `: `; nothing for nothing.
    let said = |err: &Mutex<Vec<u8>>| {
        let err = take(err);
        let last = err.lines().rev().find(|l| !l.trim().is_empty());
        last.map_or_else(String::new, |l| format!(": {:.200}", l.trim()))
    };
    let ended = || stdout.is_finished() && stderr.is_finished();
    let wait = |until: Instant| {
        while !ended() && Instant::now() < until {
            thread::sleep(Duration::from_millis(20));
        }
    };
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| Failed::with_nothing(e.to_string(), true))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            group::kill(&child);
            let _ = child.wait();
            wait(Instant::now() + DRAIN);
            // The pipe threads finish on their own once the group is gone; not joined, in case
            // something left the group and still holds a pipe.
            let message = format!(
                "{program} timed out after {}s{}",
                timeout.as_secs(),
                said(&err)
            );
            return Err(Failed::with_nothing(message, true));
        }
        thread::sleep(Duration::from_millis(50));
    };
    wait(Instant::now() + GRACE);
    if !ended() {
        group::kill(&child);
        wait(Instant::now() + DRAIN);
    }
    // Neither the readers nor the writer is waited for: one still going is held up by something
    // out of the group, and ends with it.
    drop(writer);
    let out = take(&out);
    if status.success() {
        Ok(out)
    } else {
        Err(Failed {
            message: format!("{program} exited with {status}{}", said(&err)),
            stdout: out,
            started: true,
            code: status.code(),
        })
    }
}

/// How an LLM command and a key command start: with no terminal, and without `GPG_TTY`. A recheck
/// runs both kinds of command while the screen owns the terminal, and the window has no terminal
/// at all, so a command that asks there has to fail at once: in subcheck's session it would stop
/// until its limit. `GPG_TTY` would have gpg-agent put pinentry-curses on subcheck's terminal; the
/// agent can still use the terminal it started from.
fn without_terminal(cmd: &mut Command) -> io::Result<(Child, group::Registered)> {
    group::spawn_without_terminal(cmd.env_remove("GPG_TTY"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::text::Occurrence;

    fn line(style: &str, text: &str, start_ms: i64) -> Line {
        Line {
            style: style.into(),
            text: text.into(),
            occurrences: vec![Occurrence {
                track: 0,
                start_ms,
                end_ms: start_ms + 1000,
            }],
        }
    }

    #[test]
    fn prompt_numbers_lines_from_one_with_time_and_style() {
        let a = line("K1Norma", "befor it came", 779_080);
        let b = line("OP en", "gets trough", 165_060);
        let p = prompt(&[&a, &b]);
        assert!(p.starts_with("You are proofreading English subtitles"));
        assert!(p.contains(
            "\nL1 [0:12:59.08] (K1Norma) befor it came\nL2 [0:02:45.06] (OP en) gets trough\n"
        ));
        assert!(p.ends_with("(End of lines. Reply only in the format above, or NONE.)\n"));
    }

    #[test]
    fn parse_reads_tab_separated_findings() {
        let a = line("D", "befor it came", 0);
        let b = line("D", "fine", 0);
        let reply = "L1\tbefore it came\t\"befor\" misspelled\n";
        let r = parse(reply, &[&a, &b]).unwrap();
        assert_eq!(
            r.suggestions,
            [Suggestion {
                index: 0,
                corrected: "before it came".into(),
                reason: "\"befor\" misspelled".into()
            }]
        );
    }

    #[test]
    fn parse_strips_an_echoed_time_and_style() {
        let a = line("OP en", "gets trough to", 0);
        let reply = "L1 [0:02:45.06] (OP en) gets through to\tthrough for trough\n\n";
        let r = parse(reply, &[&a]).unwrap();
        assert_eq!(r.suggestions[0].corrected, "gets through to");
        assert_eq!(r.suggestions[0].reason, "through for trough");
    }

    #[test]
    fn parse_counts_unknown_ids_and_no_ops() {
        let a = line("D", "fine", 0);
        let reply = "L1\tfine\tnothing\nL9\tghost\tnot a line\n";
        let r = parse(reply, &[&a]).unwrap();
        assert_eq!(r.suggestions, [] as [Suggestion; 0]);
        assert_eq!((r.unknown_ids, r.no_ops), (1, 1));
    }

    #[test]
    fn none_is_a_clean_reply_and_chatter_alone_is_an_error() {
        let a = line("D", "fine", 0);
        assert_eq!(parse("NONE\n", &[&a]).unwrap(), Reply::default());
        assert!(parse("\n\n", &[&a]).is_err());
        assert!(parse("Here are my thoughts about the lines.", &[&a]).is_err());
    }

    #[test]
    fn none_survives_punctuation_and_markdown() {
        let a = line("D", "fine", 0);
        for reply in [
            "NONE.",
            "**NONE**",
            "`NONE`",
            "None - no mistakes found.",
            "  none\n",
        ] {
            assert_eq!(parse(reply, &[&a]), Ok(Reply::default()), "{reply:?}");
        }
        // A word that merely starts with "none" is not NONE.
        assert!(parse("Nonetheless, all fine.", &[&a]).is_err());
    }

    /// A command backend's argv and how it prints its reply.
    fn command(b: &Backend) -> (&[String], Output) {
        match &b.via {
            Via::Command { argv, output } => (argv, *output),
            other => panic!("{other:?}"),
        }
    }

    /// `command` run through `sh -c`, its reply read as `output` says.
    fn shell_as(output: Output, command: &str) -> Backend {
        Backend {
            via: Via::Command {
                argv: Backend::shell(command).command_line(),
                output,
            },
            ..Backend::shell(command)
        }
    }

    #[test]
    fn presets() {
        let claude = Backend::preset("claude").unwrap();
        let (c, output) = command(&claude);
        assert_eq!(&c[..4], ["claude", "-p", "--model", "sonnet"]);
        assert!(c.contains(&"--setting-sources".to_string()));
        assert!(c.windows(2).any(|w| w == ["--output-format", "json"]));
        assert_eq!(output, Output::Claude);
        let gpt = Backend::preset("gpt").unwrap();
        let (g, output) = command(&gpt);
        assert_eq!(&g[..4], ["codex", "exec", "--disable", "hooks"]);
        assert_eq!(&g[g.len() - 2..], ["--json", "-"]);
        assert_eq!(output, Output::Codex);
        let o = Backend::preset("ollama:gemma4:12b-it-qat").unwrap();
        let Via::Server(s) = &o.via else {
            panic!("{o:?}")
        };
        let url = server::ollama_url(std::env::var("OLLAMA_HOST").ok().as_deref());
        assert_eq!(
            (o.label.as_str(), s.entry.url.as_str(), s.model.as_deref()),
            (
                "ollama:gemma4:12b-it-qat",
                url.as_str(),
                Some("gemma4:12b-it-qat")
            )
        );
        assert_eq!((o.chunk_lines, o.jobs), (None, JOBS));
        assert!(Backend::preset("ollama:").is_err());
        assert!(Backend::preset("gemini").is_err());
        assert_eq!(
            Backend::shell("my-llm --x").command_line(),
            ["sh", "-c", "my-llm --x"]
        );
        assert_eq!(command(&Backend::shell("my-llm --x")).1, Output::Text);
    }

    /// A real reply of `claude -p --output-format json` (2.1.304), with fields it does not read
    /// trimmed.
    const CLAUDE: &str = include_str!("../tests/fixtures/llm/claude.json");
    /// A real reply of `codex exec --json` (0.163.0), whole.
    const CODEX: &str = include_str!("../tests/fixtures/llm/codex.jsonl");
    /// An OpenAI chat completion, as the API documents it.
    const OPENAI: &str = include_str!("../tests/fixtures/llm/openai.json");
    /// A real failed call of `codex exec --json` (0.163.0), whole: a model the account cannot use.
    const CODEX_ERROR: &str = include_str!("../tests/fixtures/llm/codex-error.jsonl");

    #[test]
    fn claudes_json_gives_the_reply_as_plain_claude_prints_it_and_what_the_call_used() {
        let a = Output::Claude.read(CLAUDE);
        // `claude -p` without --output-format prints the result and a newline.
        assert_eq!(a.reply.as_deref(), Ok("NONE\n"));
        assert_eq!(
            a.used,
            Used::Reported(Usage {
                // 2 uncached, 622 written to the cache, 0 read from it.
                input_tokens: 624,
                output_tokens: 5,
                cache_read_tokens: Some(0),
                cache_write_tokens: Some(622),
                cost_usd: Some(0.002_542),
            })
        );
    }

    #[test]
    fn a_claude_price_that_is_no_price_is_none_and_the_tokens_are_kept() {
        let negative = CLAUDE.replace(r#""total_cost_usd":0.002542"#, r#""total_cost_usd":-0.5"#);
        let used = Output::Claude.read(&negative).used;
        let u = reported(&used).unwrap();
        assert_eq!((u.cost_usd, u.output_tokens), (None, 5));
        // JSON cannot write the rest, but a price must be a finite one.
        for cost in [f64::NAN, f64::INFINITY, -0.01] {
            assert!(!is_price(cost), "{cost}");
        }
        assert!(is_price(0.0) && is_price(0.002_542));
    }

    #[test]
    fn a_claude_error_fails_the_call_and_keeps_what_it_used() {
        let failed = CLAUDE
            .replace(r#""is_error":false"#, r#""is_error":true"#)
            .replace(
                r#""result":"NONE""#,
                r#""result":"API Error: 529 overloaded""#,
            );
        let a = Output::Claude.read(&failed);
        assert_eq!(
            a.reply,
            Err("claude reported an error: API Error: 529 overloaded".into())
        );
        assert_eq!(reported(&a.used).map(|u| u.output_tokens), Some(5));
        // A blank result and a subtype of `success` say nothing.
        let a = Output::Claude.read(r#"{"is_error":true,"result":"","subtype":"success"}"#);
        assert_eq!(a.reply, Err("claude reported an error".into()));
        let a = Output::Claude.read("Not logged in\n");
        assert_eq!(
            a.reply,
            Err("reply is not claude's JSON: Not logged in".into())
        );
        assert_eq!(a.used, Used::Unreported);
    }

    #[test]
    fn codexs_events_give_the_last_message_and_the_turns_tokens_without_a_price() {
        let a = Output::Codex.read(CODEX);
        assert_eq!(a.reply.as_deref(), Ok("NONE\n"));
        assert_eq!(
            a.used,
            Used::Reported(Usage {
                // codex counts cached input in its input, as this record does.
                input_tokens: 17_413,
                output_tokens: 59,
                cache_read_tokens: Some(12_288),
                cache_write_tokens: Some(0),
                cost_usd: None,
            })
        );
        // An earlier message gives way to the last; a line that is no event is passed over.
        let two = CODEX.replace(
            r#"{"type":"turn.completed""#,
            "not json\n{\"type\":\"item.completed\",\"item\":{\"id\":\"item_1\",\"type\":\"agent_message\",\"text\":\"L1\\tfixed\\twhy\"}}\n{\"type\":\"turn.completed\"",
        );
        assert_eq!(
            Output::Codex.read(&two).reply.as_deref(),
            Ok("L1\tfixed\twhy\n")
        );
    }

    #[test]
    fn codex_with_no_message_fails_the_call_with_its_error() {
        let failed = "{\"type\":\"turn.started\"}\n{\"type\":\"error\",\"message\":\"stream disconnected\"}\n{\"type\":\"turn.failed\",\"error\":{\"message\":\"usage limit reached\"}}\n";
        let a = Output::Codex.read(failed);
        assert_eq!(a.reply, Err("codex: usage limit reached".into()));
        assert_eq!(a.used, Used::Unreported);
        let a = Output::Codex.read("{\"type\":\"turn.started\"}\n");
        assert_eq!(a.reply, Err("codex gave no message".into()));
        // Exiting 0 with no reply: its last error, alone.
        let a = Output::Codex.read("{\"type\":\"error\",\"message\":\"stream disconnected\"}\n");
        assert_eq!(a.reply, Err("codex: stream disconnected".into()));
        let a = Output::Codex.read("");
        assert_eq!(a.reply, Err("reply is not codex's events: (empty)".into()));
    }

    #[test]
    fn an_openai_reply_gives_its_content_and_what_the_call_used() {
        let a = Output::OpenAi.read(OPENAI);
        assert_eq!(a.reply.as_deref(), Ok("NONE\n"));
        assert_eq!(
            a.used,
            Used::Reported(Usage {
                input_tokens: 624,
                output_tokens: 5,
                cache_read_tokens: Some(512),
                cache_write_tokens: None,
                cost_usd: None,
            })
        );
        let a = shell_as(Output::OpenAi, "cat").call(OPENAI, TIMEOUT, None, &|_| {});
        assert_eq!(a.reply.as_deref(), Ok("NONE\n"));
    }

    #[test]
    fn an_openai_reply_cut_off_at_its_length_limit_fails_and_keeps_its_usage() {
        let cut = OPENAI.replace(r#""finish_reason":"stop""#, r#""finish_reason":"length""#);
        let a = Output::OpenAi.read(&cut);
        assert_eq!(
            a.reply,
            Err("the reply was cut off at the server's length limit".into())
        );
        assert_eq!(reported(&a.used).map(|u| u.output_tokens), Some(5));
    }

    #[test]
    fn an_openai_reply_stopped_by_the_content_filter_fails_and_keeps_its_usage() {
        let stopped = OPENAI.replace(
            r#""finish_reason":"stop""#,
            r#""finish_reason":"content_filter""#,
        );
        let a = Output::OpenAi.read(&stopped);
        assert_eq!(
            a.reply,
            Err("the reply stopped early: content_filter".into())
        );
        assert_eq!(reported(&a.used).map(|u| u.output_tokens), Some(5));
        // Any other word a server ends a whole reply with is read as one.
        for end in ["eos_token", "stop_sequence", "STOP", ""] {
            let ended = OPENAI.replace(
                r#""finish_reason":"stop""#,
                &format!(r#""finish_reason":"{end}""#),
            );
            assert_eq!(
                Output::OpenAi.read(&ended).reply.as_deref(),
                Ok("NONE\n"),
                "{end}"
            );
        }
        // A null reason, or none, as some servers send: a whole reply.
        let null = OPENAI.replace(r#""finish_reason":"stop""#, r#""finish_reason":null"#);
        let absent = OPENAI.replace(r#","finish_reason":"stop""#, "");
        assert!(null.contains("null") && !absent.contains("finish_reason"));
        for whole in [&null, &absent] {
            assert_eq!(
                Output::OpenAi.read(whole).reply.as_deref(),
                Ok("NONE\n"),
                "{whole}"
            );
        }
    }

    #[test]
    fn a_reply_cut_off_at_the_length_limit_is_too_big_for_the_reply() {
        let cut = OPENAI.replace(r#""finish_reason":"stop""#, r#""finish_reason":"length""#);
        let cut_off = (
            Some(TooBig::Reply),
            Err("the reply was cut off at the server's length limit".to_string()),
        );
        let a = Output::OpenAi.read(&cut);
        assert_eq!((a.too_big, a.reply), cut_off);
        let a = shell_as(Output::OpenAi, "cat").call(&cut, TIMEOUT, None, &|_| {});
        assert_eq!((a.too_big, a.reply), cut_off);
        let stopped = OPENAI.replace(
            r#""finish_reason":"stop""#,
            r#""finish_reason":"content_filter""#,
        );
        assert_eq!(Output::OpenAi.read(&stopped).too_big, None);
        for (output, stdout) in [
            (Output::OpenAi, OPENAI),
            (Output::Claude, CLAUDE),
            (Output::Codex, CODEX),
            (Output::Text, "NONE\n"),
        ] {
            assert_eq!(output.read(stdout).too_big, None, "{output:?}");
        }
    }

    #[test]
    fn an_openai_reply_without_content_usage_or_json_says_so() {
        let empty = OPENAI.replace(r#""content":"NONE""#, r#""content":null"#);
        assert_eq!(
            Output::OpenAi.read(&empty).reply,
            Err("the reply has no content".into())
        );
        let bare = r#"{"choices":[{"message":{"content":"NONE"},"finish_reason":"stop"}]}"#;
        assert_eq!(Output::OpenAi.read(bare).used, Used::Unreported);
        let a = Output::OpenAi.read("hello\n");
        assert_eq!(
            a.reply,
            Err("reply is not an OpenAI chat completion: hello".into())
        );
        assert_eq!(a.used, Used::Unreported);
    }

    #[test]
    fn an_openai_error_object_is_the_reason_whether_the_command_failed_or_not() {
        let said = r#"{"error":{"message":"model not found"}}"#;
        assert_eq!(
            Output::OpenAi.read(said).reply,
            Err("the reply is an error: model not found".into())
        );
        let a = failing(Output::OpenAi, said, "");
        assert_eq!(
            a.reply,
            Err("sh exited with exit status: 1; the command said: model not found".into())
        );
        assert_eq!(a.used, Used::Unreported);
    }

    #[test]
    fn an_openai_error_that_is_a_string_is_the_reason_too() {
        assert_eq!(
            Output::OpenAi.read(r#"{"error":"model not found"}"#).reply,
            Err("the reply is an error: model not found".into())
        );
    }

    /// A JSON backend that prints `stdout`, says `stderr` (when there is any), and exits 1.
    fn failing(output: Output, stdout: &str, stderr: &str) -> Answer {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        std::fs::write(&out, stdout).unwrap();
        let say = if stderr.is_empty() {
            String::new()
        } else {
            format!("echo '{stderr}' >&2; ")
        };
        let cmd = format!("cat '{}'; {say}exit 1", out.display());
        shell_as(output, &cmd).call("", TIMEOUT, None, &|_| {})
    }

    #[test]
    fn codex_passing_on_the_apis_error_says_the_apis_message() {
        // Its message is the API's error as JSON; the warning before it is an item, passed over.
        let why = "The 'no-such-model-subcheck-test' model is not supported when using Codex with \
                   a ChatGPT account.";
        assert_eq!(
            failing(Output::Codex, CODEX_ERROR, "").reply,
            Err(format!("sh exited with exit status: 1; codex said: {why}"))
        );
        // Exiting 0 with no reply: the API's message, alone.
        assert_eq!(
            Output::Codex.read(CODEX_ERROR).reply,
            Err(format!("codex: {why}"))
        );
        // One with no message of its own: codex's, whole.
        let blank = r#"{"type":"error","status":500,"error":{"message":" "}}"#;
        let event = serde_json::json!({"type": "turn.failed", "error": {"message": blank}});
        assert_eq!(
            Output::Codex.read(&event.to_string()).reply,
            Err(format!("codex: {blank}"))
        );
        // Nothing at all: no reason, and the error before it stands.
        let events = "{\"type\":\"error\",\"message\":\"stream disconnected\"}\n\
                      {\"type\":\"turn.failed\",\"error\":{\"message\":\" \"}}\n";
        assert_eq!(
            Output::Codex.read(events).reply,
            Err("codex: stream disconnected".into())
        );
    }

    #[test]
    fn a_failed_call_says_its_exit_status_and_stderr_and_then_the_backends_reason() {
        let status = "sh exited with exit status: 1: Error: not logged in";
        let codex = |events: &str| failing(Output::Codex, events, "Error: not logged in").reply;
        // No reason in its JSON: the exit status and stderr alone.
        assert_eq!(
            codex(r#"{"type":"thread.started","thread_id":"t"}"#),
            Err(status.to_string())
        );
        // codex's turn.failed, else its last error, after them.
        let failed = r#"{"type":"turn.failed","error":{"message":"usage limit reached"}}"#;
        let error = |m: &str| format!(r#"{{"type":"error","message":"{m}"}}"#);
        assert_eq!(
            codex(failed),
            Err(format!("{status}; codex said: usage limit reached"))
        );
        assert_eq!(
            codex(&format!("{}\n{failed}", error("stream disconnected"))),
            Err(format!("{status}; codex said: usage limit reached"))
        );
        assert_eq!(
            codex(&format!(
                "{}\n{}",
                error("stream disconnected"),
                error("retrying")
            )),
            Err(format!("{status}; codex said: retrying"))
        );
        // claude's non-blank result, else its non-blank subtype unless that is "success".
        let claude = |json: &str| failing(Output::Claude, json, "Error: not logged in").reply;
        let claude_error = |result: &str, subtype: &str| {
            claude(&format!(
                r#"{{"type":"result","is_error":true,"result":"{result}","subtype":"{subtype}"}}"#
            ))
        };
        assert_eq!(
            claude_error("API Error: 529 overloaded", "success"),
            Err(format!("{status}; claude said: API Error: 529 overloaded"))
        );
        assert_eq!(
            claude_error(" ", "error_max_turns"),
            Err(format!("{status}; claude said: error_max_turns"))
        );
        assert_eq!(claude_error("", "success"), Err(status.to_string()));
        // A reply, not an error: no reason, and what the call used is kept.
        let a = failing(Output::Claude, CLAUDE, "Error: not logged in");
        assert_eq!(a.reply, Err(status.to_string()));
        assert_eq!(reported(&a.used).map(|u| u.output_tokens), Some(5));
    }

    #[test]
    fn claude_out_of_its_session_says_why_after_its_exit_status() {
        // Its reason is in its JSON alone, nothing on stderr; what it used is kept.
        let limit = CLAUDE
            .replace(r#""is_error":false"#, r#""is_error":true"#)
            .replace(
                r#""result":"NONE""#,
                r#""result":"You've hit your session limit""#,
            );
        let a = failing(Output::Claude, &limit, "");
        assert_eq!(
            a.reply,
            Err("sh exited with exit status: 1; claude said: You've hit your session limit".into())
        );
        assert_eq!(reported(&a.used).map(|u| u.output_tokens), Some(5));
        // Text on stdout is never taken for a reason.
        let a = Backend::shell("echo NONE; exit 1").call("", TIMEOUT, None, &|_| {});
        assert_eq!(a.reply, Err("sh exited with exit status: 1".into()));
    }

    #[test]
    fn plain_text_is_the_reply_and_says_nothing_of_usage() {
        let a = Output::Text.read("L1\tfixed\twhy\n");
        assert_eq!(
            a,
            Answer {
                reply: Ok("L1\tfixed\twhy\n".into()),
                used: Used::Untracked,
                too_big: None
            }
        );
    }

    #[test]
    fn a_call_reads_its_reply_as_the_backend_prints_it() {
        let a = shell_as(Output::Claude, "cat").call(CLAUDE, TIMEOUT, None, &|_| {});
        assert_eq!(a.reply.as_deref(), Ok("NONE\n"));
        assert!(reported(&a.used).is_some());
        let a = Backend::shell("exit 3").call("", TIMEOUT, None, &|_| {});
        assert!(a.reply.unwrap_err().contains("exit status: 3"));
        assert_eq!(a.used, Used::Untracked);
    }

    #[test]
    fn an_llm_command_has_no_terminal_and_no_gpg_tty() {
        let tty = "if (: </dev/tty) 2>/dev/null; then echo tty; else echo none; fi";
        let (pid, session, a) = crate::test_support::a_commands_session(tty, |cmd| {
            Backend::shell(cmd).call("", TIMEOUT, None, &|_| {})
        });
        // Its own session, which a run with no terminal (CI) can tell apart.
        assert_eq!(session, Ok(pid));
        assert_eq!(a.reply.as_deref(), Ok("none\n"));
        // A call's command has this process's variables, and no test sets one of those: the
        // variable is given to a command started the way a call starts its own.
        let mut sh = Command::new("sh");
        sh.args(["-c", "echo \"${GPG_TTY:-unset}\""])
            .env("GPG_TTY", "/dev/pts/9")
            .stdout(Stdio::piped());
        let (child, _running) = without_terminal(&mut sh).unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "unset\n");
    }

    #[test]
    fn an_llm_command_that_reads_the_terminal_fails_at_once() {
        let began = Instant::now();
        let a = Backend::shell("read x </dev/tty && cat").call(
            "P",
            Duration::from_secs(30),
            None,
            &|_| {},
        );
        let why = a.reply.unwrap_err();
        assert!(why.starts_with("sh exited with"), "{why}");
        assert!(began.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn every_call_a_json_backend_started_is_counted_with_numbers_or_without() {
        let claude = |cmd: &str| shell_as(Output::Claude, cmd);
        // Killed at the timeout.
        let a = claude("sleep 5; echo late").call("", Duration::from_millis(200), None, &|_| {});
        assert!(a.reply.unwrap_err().contains("timed out"));
        assert_eq!(a.used, Used::Unreported);
        // Not its JSON, exiting 0 or not; nothing on stdout; its JSON with no usage in it.
        for cmd in [
            "echo hello",
            "echo hello; exit 1",
            "echo 'Error: not logged in' >&2; exit 1",
            r#"echo '{"type":"result","is_error":true,"result":"no"}'; exit 1"#,
        ] {
            assert_eq!(
                claude(cmd).call("", TIMEOUT, None, &|_| {}).used,
                Used::Unreported,
                "{cmd}"
            );
        }
        let codex = shell_as(
            Output::Codex,
            r#"echo '{"type":"thread.started","thread_id":"t"}'; exit 1"#,
        );
        assert_eq!(
            codex.call("", TIMEOUT, None, &|_| {}).used,
            Used::Unreported
        );
        // It never started: nothing to count.
        let gone = Backend {
            via: Via::Command {
                argv: vec!["/nonexistent/claude".into()],
                output: Output::Claude,
            },
            ..claude("")
        };
        let a = gone.call("", TIMEOUT, None, &|_| {});
        assert!(a.reply.unwrap_err().contains("cannot run"));
        assert_eq!(a.used, Used::Untracked);
        // A backend that never reports: nothing to count, whatever happened.
        for (cmd, timeout) in [
            ("sleep 5; echo late", Duration::from_millis(200)),
            ("exit 1", TIMEOUT),
        ] {
            assert_eq!(
                Backend::shell(cmd).call("", timeout, None, &|_| {}).used,
                Used::Untracked,
                "{cmd}"
            );
        }
    }

    fn reported(used: &Used) -> Option<&Usage> {
        match used {
            Used::Reported(u) => Some(u),
            Used::Unreported | Used::Untracked => None,
        }
    }

    fn usage(input: u64, output: u64, cost: Option<f64>) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cost_usd: cost,
            ..Usage::default()
        }
    }

    #[test]
    fn spent_sums_the_calls_and_their_price_a_floor_when_a_call_has_none() {
        let mut s = Spent::default();
        assert_eq!(s.line(", "), None, "no calls recorded");
        s.add(Some(&usage(1_200_000, 30_000, Some(3.0))));
        s.add(Some(&usage(34_567, 1_234, Some(0.21))));
        assert_eq!(
            s.line(", ").as_deref(),
            Some("LLM: 2 calls, 1.23M in / 31.2k out tokens, $3.21 at API prices")
        );
        assert_eq!(
            s.parts(),
            [
                "2 calls",
                "1.23M in / 31.2k out tokens",
                "$3.21 at API prices"
            ]
        );
        // A call with no price: the sum is short of it, and says so.
        s.add(Some(&usage(10, 1, None)));
        assert_eq!(
            s.line(" · ").as_deref(),
            Some("LLM: 3 calls · 1.23M in / 31.2k out tokens · at least $3.21 at API prices")
        );
        // No call with a price (codex): none.
        let mut codex = Spent::default();
        codex.add(Some(&usage(1500, 40, None)));
        assert_eq!(
            codex.line(", ").as_deref(),
            Some("LLM: 1 call, 1.50k in / 40 out tokens")
        );
        let mut one = Spent::default();
        one.add(Some(&usage(624, 5, Some(0.002_542))));
        assert_eq!(
            one.line(", ").as_deref(),
            Some("LLM: 1 call, 624 in / 5 out tokens, $0.0025 at API prices")
        );
    }

    #[test]
    fn calls_that_reported_nothing_are_counted_and_make_the_price_a_floor() {
        let mut s = Spent::default();
        s.add(Some(&usage(1203, 40, Some(0.0126))));
        s.add(None);
        assert_eq!(
            s.line(", ").as_deref(),
            Some(
                "LLM: 2 calls (1 reported no usage), 1.20k in / 40 out tokens, \
                 at least $0.013 at API prices"
            )
        );
        // None reported: no sums to show.
        let mut none = Spent::default();
        for _ in 0..200 {
            none.add(None);
        }
        assert_eq!(
            none.line(", ").as_deref(),
            Some("LLM: 200 calls (none reported usage)")
        );
    }

    #[test]
    fn token_counts_read_to_three_figures() {
        let cases = [
            (0, "0"),
            (999, "999"),
            (1_000, "1.00k"),
            (1_234, "1.23k"),
            (9_994, "9.99k"),
            (9_995, "10.0k"),
            (31_249, "31.2k"),
            (99_950, "100k"),
            (999_499, "999k"),
            (999_500, "1.00M"),
            (1_234_567, "1.23M"),
            (12_345_678_901, "12.3G"),
            (u64::MAX, "18446744074G"),
        ];
        for (n, shown) in cases {
            assert_eq!(tokens(n), shown, "{n}");
        }
    }

    #[test]
    fn dollars_keep_two_figures_under_ten_cents() {
        let cases = [
            (3.214, "$3.21"),
            (0.1, "$0.10"),
            (0.0126, "$0.013"),
            (0.002_542, "$0.0025"),
            (0.0, "$0.00"),
            // Two figures as rounded, not before.
            (0.0999, "$0.10"),
            (0.009_99, "$0.010"),
            // Under a millionth of a dollar is not free.
            (0.000_003, "$0.000003"),
            (0.000_000_5, "<$0.000001"),
        ];
        for (x, shown) in cases {
            assert_eq!(dollars(x), shown, "{x}");
        }
    }

    #[test]
    fn a_backend_comes_back_from_the_label_a_run_records() {
        for label in [
            "claude",
            "gpt",
            "ollama:gemma4:12b-it-qat",
            "cmd:my-llm --x",
        ] {
            let b = Backend::from_label(label, &settings("")).unwrap();
            assert_eq!(b.label, label);
        }
        assert_eq!(
            Backend::from_label("cmd:my-llm --x", &settings(""))
                .unwrap()
                .command_line(),
            Backend::shell("my-llm --x").command_line()
        );
        assert!(Backend::from_label("gemini", &settings("")).is_err());
    }

    fn settings(text: &str) -> Settings {
        Settings::parse(Path::new("/c/config.toml"), text).unwrap()
    }

    #[test]
    fn an_entry_is_a_backend_by_its_name() {
        let s = settings(
            "[llm.mine]\ncommand = \"my-llm --fast\"\noutput = \"claude\"\njobs = 4\nchunk_lines = 200\n[llm.plain]\ncommand = \"x\"\n",
        );
        assert_eq!(
            Backend::named("mine", &s),
            Ok(Backend {
                label: "mine".into(),
                via: Via::Command {
                    argv: vec!["sh".into(), "-c".into(), "my-llm --fast".into()],
                    output: Output::Claude,
                },
                chunk_lines: Some(200),
                jobs: 4,
                timeout: TIMEOUT,
            })
        );
        let p = Backend::named("plain", &s).unwrap();
        assert_eq!(
            (
                p.chunk_lines,
                p.lines_per_chunk(2, None),
                p.jobs,
                command(&p).1
            ),
            (None, CHUNK_LINES, JOBS, Output::Text)
        );
        for preset in ["claude", "gpt", "ollama:gemma4"] {
            assert_eq!(
                Backend::named(preset, &s),
                Backend::preset(preset),
                "{preset}"
            );
        }
        assert_eq!(Backend::preset("claude").unwrap().jobs, JOBS);
        assert_eq!(Backend::shell("x").jobs, JOBS);
        assert_eq!(
            Backend::named("nope", &s),
            Err("unknown --llm \"nope\": use claude, gpt, ollama:<model>, an entry in /c/config.toml, or --llm-cmd".into())
        );
    }

    #[test]
    fn a_backends_limit_is_its_entrys_else_ten_minutes_and_a_flag_beats_both() {
        let text =
            "[llm.slow]\ncommand = \"cat\"\ntimeout_minutes = 3\n[llm.plain]\ncommand = \"cat\"\n";
        let s = Settings::parse(Path::new("/c/config.toml"), text).unwrap();
        let mins = |m: u64| Duration::from_secs(m * 60);
        assert_eq!(Backend::named("slow", &s).unwrap().timeout, mins(3));
        assert_eq!(Backend::from_label("slow", &s).unwrap().timeout, mins(3));
        assert_eq!(Backend::named("plain", &s).unwrap().timeout, TIMEOUT);
        assert_eq!(Backend::preset("claude").unwrap().timeout, TIMEOUT);
        assert_eq!(Backend::shell("cat").timeout, TIMEOUT);
        let slow = Backend::named("slow", &s).unwrap();
        assert_eq!(slow.clone().limited_to(Some(45)).timeout, mins(45));
        assert_eq!(slow.clone().limited_to(Some(u64::MAX)).timeout, mins(1440));
        assert_eq!(slow.clone().limited_to(Some(0)).timeout, mins(1));
        assert_eq!(slow.limited_to(None).timeout, mins(3));
    }

    #[test]
    fn a_recorded_label_finds_its_backend_again() {
        let s = settings("[llm.mine]\ncommand = \"x\"\n");
        assert_eq!(Backend::from_label("mine", &s), Backend::named("mine", &s));
        assert_eq!(
            Backend::from_label("cmd:my-llm", &s),
            Ok(Backend::shell("my-llm"))
        );
        assert_eq!(Backend::from_label("gpt", &s), Backend::preset("gpt"));
        assert_eq!(
            Backend::from_label("gone", &s),
            Err("gone, which this subcheck does not know and /c/config.toml does not name".into())
        );
    }

    #[test]
    fn a_label_with_nothing_after_the_at_names_no_model() {
        let s = settings("[llm.srv]\nurl = \"http://127.0.0.1:1/v1\"\n");
        assert_eq!(
            Backend::from_label("srv@", &s),
            Err("srv@, which names no model".into())
        );
        assert_eq!(
            Backend::from_label("ollama:m@x", &s),
            Backend::preset("ollama:m@x")
        );
    }

    #[test]
    fn a_server_entry_is_a_backend_labelled_by_its_name_and_model() {
        let s = settings(
            "[llm.srv]\nurl = \"http://127.0.0.1:1/v1\"\nmodel = \"gemma4\"\njobs = 3\n[llm.any]\nurl = \"http://127.0.0.1:1/v1\"\n",
        );
        let b = Backend::named("srv", &s).unwrap();
        assert_eq!(
            (b.label.as_str(), b.jobs, b.chunk_lines),
            ("srv@gemma4", 3, None)
        );
        assert!(
            matches!(&b.via, Via::Server(s) if s.name == "srv" && s.model.as_deref() == Some("gemma4"))
        );
        assert_eq!(b.command_line(), ["http://127.0.0.1:1/v1/chat/completions"]);
        let any = Backend::named("any", &s).unwrap();
        assert_eq!(any.label, "any");
        assert!(matches!(&any.via, Via::Server(s) if s.model.is_none()));
        assert_eq!(Backend::shell("x y").command_line(), ["sh", "-c", "x y"]);
    }

    #[test]
    fn a_budgets_fit_is_the_most_lines_its_prompt_holds_and_its_lines_are_that_up_to_400() {
        for share in [
            0,
            600,
            1024,
            2048,
            4096,
            8192,
            16384,
            32768,
            131_072,
            1 << 20,
        ] {
            let b = Budget::new(share, None);
            let n = b.fit();
            assert!(n >= 1, "{share}");
            if n > 1 {
                assert!(estimate(n) <= b.prompt(), "{share}: {n} lines do not fit");
            }
            assert!(
                estimate(n + 1) > b.prompt(),
                "{share}: room for more than {n}"
            );
            assert_eq!(b.lines(), n.min(CHUNK_LINES), "{share}");
        }
        let fit = |share| Budget::new(share, None).fit();
        assert_eq!(
            (fit(0), fit(2048), fit(8192), fit(131_072)),
            (1, 30, 155, 3420)
        );
        let lines = |share| Budget::new(share, None).lines();
        assert_eq!((lines(0), lines(131_072)), (1, CHUNK_LINES));
        assert_eq!(estimate(10), PROMPT_TOKENS + 10 * TOKENS_PER_LINE);
    }

    #[test]
    fn the_full_chunk_context_is_the_smallest_that_fits_a_full_chunk() {
        for max_tokens in [None, Some(0), Some(8192)] {
            let full = full_chunk_context(max_tokens);
            assert_eq!(
                (
                    Budget::new(full - 1, max_tokens).lines(),
                    Budget::new(full, max_tokens).lines()
                ),
                (CHUNK_LINES - 1, CHUNK_LINES),
                "{max_tokens:?}"
            );
        }
    }

    #[test]
    fn a_budgets_reply_room_is_the_entrys_cap_up_to_the_share_else_its_quarter_up_to_4096() {
        for (share, max_tokens, prompt, cap) in [
            (16384, None, 12288, 4096),
            // A quarter over 4096 is bounded there, however large the share.
            (16388, None, 16388 - 4096, 4096),
            (523_788, None, 523_788 - 4096, 4096),
            (1 << 20, None, (1 << 20) - 4096, 4096),
            // An entry's own cap is the room at any size.
            (1 << 20, Some(8192), (1 << 20) - 8192, 8192),
            (16384, Some(8192), 8192, 8192),
            (16384, Some(20000), 0, 16384),
            (16384, Some(0), 16384, 1),
            (3, None, 3, 1),
        ] {
            let b = Budget::new(share, max_tokens);
            assert_eq!(
                (b.prompt(), b.cap()),
                (prompt, cap),
                "{share} {max_tokens:?}"
            );
        }
    }

    #[test]
    fn a_recorded_server_label_finds_its_entry_and_keeps_its_model() {
        let s = settings(
            "[llm.srv]\nurl = \"http://127.0.0.1:1/v1\"\n[llm.pinned]\nurl = \"http://127.0.0.1:1/v1\"\nmodel = \"b\"\n[llm.cmd]\ncommand = \"x\"\n",
        );
        let b = Backend::from_label("srv@org/model:q4@x", &s).unwrap();
        assert_eq!(b.label, "srv@org/model:q4@x");
        assert!(matches!(&b.via, Via::Server(s) if s.model.as_deref() == Some("org/model:q4@x")));
        assert_eq!(
            Backend::from_label("pinned@b", &s).map(|b| b.label),
            Ok("pinned@b".into())
        );
        for (label, said) in [
            (
                "pinned@a",
                "pinned@a, but /c/config.toml now has pinned ask for \"b\"",
            ),
            ("cmd@a", "cmd@a, which /c/config.toml now has as a command"),
            ("srv", "srv, which /c/config.toml now has as a server"),
            (
                "gone@a",
                "gone@a, which this subcheck does not know and /c/config.toml does not name",
            ),
        ] {
            assert_eq!(
                Backend::from_label(label, &s),
                Err(said.to_string()),
                "{label}"
            );
        }
    }

    #[test]
    fn a_command_reports_no_wait() {
        let heard = Mutex::new(Vec::new());
        let waits: Waits = &|wait| heard.lock().unwrap().push(wait);
        let (b, _) = Backend::shell("cat").ready(waits).unwrap();
        let b = b.sized(waits).unwrap();
        b.call("P", TIMEOUT, None, waits);
        assert_eq!(heard.into_inner().unwrap(), []);
    }

    #[test]
    fn an_llm_that_cannot_be_built_again_reports_no_wait() {
        let heard = Mutex::new(Vec::new());
        let waits: Waits = &|wait| heard.lock().unwrap().push(wait);
        let b = Backend {
            via: Via::Unknown("gone, which …".into()),
            ..Backend::shell("x")
        };
        let (b, _) = b.ready(waits).unwrap();
        let b = b.sized(waits).unwrap();
        b.call("P", TIMEOUT, None, waits);
        assert_eq!(heard.into_inner().unwrap(), []);
    }

    #[test]
    fn a_command_takes_no_cap() {
        let b = shell_as(Output::OpenAi, "cat");
        assert_eq!(
            b.call(OPENAI, TIMEOUT, Some(Budget::new(4, None)), &|_| {}),
            b.call(OPENAI, TIMEOUT, None, &|_| {})
        );
    }

    #[test]
    fn an_unknown_llm_answers_why_and_counts_nothing() {
        let b = Backend {
            via: Via::Unknown("gone, which …".into()),
            ..Backend::shell("x")
        };
        assert_eq!(
            b.call("P", TIMEOUT, None, &|_| {}),
            Answer {
                reply: Err("gone, which …".into()),
                used: Used::Untracked,
                too_big: None
            }
        );
    }

    #[test]
    fn run_passes_stdin_through() {
        let argv = Backend::shell("tr a-z A-Z").command_line();
        assert_eq!(
            run(&argv, "hello\n", TIMEOUT, group::spawn).unwrap(),
            "HELLO\n"
        );
    }

    #[test]
    fn run_reports_exit_status_and_stderr() {
        let argv = Backend::shell("echo boom >&2; exit 3").command_line();
        let err = run(&argv, "", TIMEOUT, group::spawn).unwrap_err().message;
        assert!(
            err.contains("exit status: 3") && err.contains("boom"),
            "{err}"
        );
    }

    #[test]
    fn run_kills_a_command_that_overruns() {
        // Two commands, so every shell forks `sleep` instead of exec'ing it: the grandchild
        // outlives the killed shell and keeps stdout open. dash forks even a lone `sleep 5`.
        let argv = Backend::shell("sleep 5; echo late").command_line();
        let started = Instant::now();
        let err = run(&argv, "", Duration::from_millis(200), group::spawn)
            .unwrap_err()
            .message;
        assert!(err.contains("timed out"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(3));
        // With what it said on stderr before.
        let argv = Backend::shell("echo 'rate limited' >&2; sleep 5").command_line();
        let err = run(&argv, "", Duration::from_secs(1), group::spawn)
            .unwrap_err()
            .message;
        assert!(err.ends_with("timed out after 1s: rate limited"), "{err}");
    }

    #[test]
    fn run_does_not_wait_for_a_background_process_holding_stdout() {
        // The shell exits at once, but `sleep` inherited stdout and keeps it open.
        let argv = Backend::shell("sleep 30 & echo done").command_line();
        let started = Instant::now();
        assert_eq!(run(&argv, "", TIMEOUT, group::spawn).unwrap(), "done\n");
        let took = started.elapsed();
        assert!(took < Duration::from_secs(10), "{took:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn run_does_not_wait_for_a_process_out_of_its_group_holding_stdout() {
        // `setsid` puts `sleep` in a session of its own, out of reach of the group's kill, and it
        // keeps the stdout it inherited open until it ends.
        let argv = Backend::shell("setsid sleep 6 & echo done").command_line();
        let started = Instant::now();
        assert_eq!(run(&argv, "", TIMEOUT, group::spawn).unwrap(), "done\n");
        let took = started.elapsed();
        assert!(took < Duration::from_secs(5), "{took:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn run_kills_the_whole_process_group_on_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let cmd = format!("sleep 30 & echo $! > '{}'; wait", pidfile.display());
        let err = run(
            &Backend::shell(&cmd).command_line(),
            "",
            Duration::from_millis(300),
            group::spawn,
        )
        .unwrap_err()
        .message;
        assert!(err.contains("timed out"), "{err}");
        let pid = std::fs::read_to_string(&pidfile).unwrap();
        thread::sleep(Duration::from_millis(200));
        // Gone, or a zombie nobody has reaped yet (a container's PID 1 may not): either way no
        // longer running.
        let stat =
            std::fs::read_to_string(format!("/proc/{}/stat", pid.trim())).unwrap_or_default();
        let state = stat
            .rsplit(')')
            .next()
            .unwrap_or("")
            .split_whitespace()
            .next();
        assert!(
            matches!(state, None | Some("Z" | "X")),
            "sleep still running: {stat}"
        );
    }

    #[test]
    fn run_survives_a_command_that_ignores_its_input() {
        // 1 MB on stdin to a command that never reads it must not deadlock.
        let argv = Backend::shell("echo done").command_line();
        let big = "x".repeat(1 << 20);
        assert_eq!(run(&argv, &big, TIMEOUT, group::spawn).unwrap(), "done\n");
    }
}
