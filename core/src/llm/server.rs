//! An LLM server subcheck calls itself: one a settings entry names by its `url`, called over its
//! OpenAI API, or Ollama for `ollama:<model>`, called over Ollama's own.
//!
//! A call posts a chat to `<url>/chat/completions`: the model, [`SYSTEM`] as the system message and
//! the prompt as the user's, no streaming, then the entry's `request` fields over the top. Thinking
//! is off unless the entry leaves it to the server: with it on, gemma4 can spend its whole budget
//! thinking and reply with nothing. Servers read the switch in different places, so both are sent:
//! `reasoning_effort`, and `enable_thinking` for the model's chat template.
//!
//! `ollama:<model>` posts the same chat to Ollama's `<url without /v1>/api/chat` instead, with
//! thinking off and `truncate: false`: Ollama's OpenAI API cuts a prompt longer than the context to
//! fit and answers the rest without a word, where its own refuses it, so the chunk can be split.

use std::fmt::{self, Write as _};
use std::io::ErrorKind;
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value};
use ureq::http::{Response, Uri};
use ureq::tls::{RootCerts, TlsConfig};
use ureq::{Agent, Body, RequestBuilder};

use super::key::{self, Key};
use super::{Answer, Budget, Output, SYSTEM, TooBig, Usage, Used, Wait, Waits, classify};
use crate::settings::{KeySource, ServerEntry, Thinking};

#[cfg(test)]
mod live_replies;

/// How long the start-up check waits for the list of models. A server that is up answers at once;
/// a scan should soon say that one is stuck, not wait the limit a chunk has.
const MODELS_TIMEOUT: Duration = Duration::from_secs(30);

/// How many tokens a server's context holds, as far as it says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// What the server's list of models, or Ollama's `/api/ps`, gives the settled model.
    Tokens(u64),
    /// Ollama, with the model not loaded, so loading it would let `/api/ps` say.
    NotLoaded,
    /// The server does not say, or has not been asked.
    Unknown,
}

/// A settings entry's server, or Ollama's for `ollama:<model>`, as a backend calls it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    /// The entry's name, which the backend's label starts with; `ollama:<model>` for Ollama's,
    /// which is its label.
    pub name: String,
    pub entry: ServerEntry,
    /// The model the calls ask for: the entry's, or the one a run recorded. `None` until the
    /// server says which it serves, when the entry names none.
    pub model: Option<String>,
    /// The key the calls send, which a 401 can change under them ([`Server::asked`]).
    key: SharedKey,
    /// Read by `ready`, and again by `sized` once the model is loaded.
    context: Context,
    api: Api,
}

/// What a server's next request does about a key.
#[derive(Debug, Clone, PartialEq, Eq)]
enum KeyState {
    /// It sends none: an entry without one, or a server not readied.
    None,
    /// The key it sends.
    Sending(Key),
    /// It is not sent: the server answered 401 to the key, and there is no other to send in its
    /// place. Boxed, so that what a refusal keeps adds nothing to a [`Server`], which a backend
    /// holds by value.
    Refused(Box<Refusal>),
}

/// A key a server refused, and what the server's calls fail with from then on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Refusal {
    /// Kept to hide it in what else is said of the server.
    key: Key,
    /// The refusal's words, the key hidden.
    why: String,
}

/// A server's [`KeyState`], behind a lock: a scan's calls share the server, so the key one of them
/// fetched again is the one the rest send, and a key refused for one is sent by none. The lock is
/// held only to read the state or to change it, never while a request is in flight or a key
/// command runs, which would hold every other call behind a passphrase window.
struct SharedKey(Mutex<KeyState>);

impl SharedKey {
    fn new(state: KeyState) -> Self {
        Self(Mutex::new(state))
    }

    /// A copy of the state as it is now.
    fn get(&self) -> KeyState {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.clone()
    }

    /// Makes the state `next` while the key it sends is still `sent`, the one a request was
    /// answered for, and gives the state as it is left. A state another call has changed since
    /// stays: it holds a key that answer says nothing of, or a refusal already.
    fn replace(&self, sent: &Key, next: KeyState) -> KeyState {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(&*state, KeyState::Sending(key) if key == sent) {
            *state = next;
        }
        state.clone()
    }
}

/// A copy of the state, not a share of it: a backend cloned for another scan is refused, or given
/// a new key, by its own calls.
impl Clone for SharedKey {
    fn clone(&self) -> Self {
        Self::new(self.get())
    }
}

/// By what the states hold. Each is copied in turn, so the two locks are never held together,
/// and a server compared with itself does not wait on its own lock.
impl PartialEq for SharedKey {
    fn eq(&self, other: &Self) -> bool {
        let mine = self.get();
        mine == other.get()
    }
}

impl Eq for SharedKey {}

/// The state alone, which shows no key: [`Key`]'s `Debug` hides it, and a refusal's message has
/// it hidden.
impl fmt::Debug for SharedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(f)
    }
}

/// A request to the server that [`Server::asked`] may send twice: `<base>/<path>`, a `POST` of
/// `body` as JSON when there is one, else a `GET`, answered within `timeout`.
struct Ask<'a> {
    base: &'a str,
    path: &'a str,
    body: Option<&'a str>,
    timeout: Duration,
    /// The wait the request belongs to, which is under way again once a key command its 401 ran
    /// has ended: the check's, for `/models`. `None` for a chat, which waits on the LLM, and for
    /// a request sent other than through [`Server::asked`], whose 401 runs nothing.
    during: Option<&'a Wait>,
}

/// A wait reported as started, and reported as ended once this is dropped: on every way out of
/// what it names, a `?` included, so nothing that shows waits keeps showing one that is over.
struct Waiting<'a>(Waits<'a>);

impl<'a> Waiting<'a> {
    /// `wait` reported through `waits` as started.
    fn start(waits: Waits<'a>, wait: Wait) -> Self {
        waits(Some(wait));
        Waiting(waits)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        (self.0)(None);
    }
}

/// How a request sent as the key's rule says ([`Server::asked`]) ended. Each message has hidden
/// the key its own request sent, whatever the server sends since.
enum Told {
    /// A 2xx answer's text as the server sent it, and the key its request sent, which hides what
    /// is said of the text.
    Answer(String, Option<Key>),
    /// Any other status: its text as the server sent it, which says what kind of refusal it is,
    /// and what to say of it.
    Status {
        status: u16,
        text: String,
        why: String,
    },
    /// No answer: why, and what the request may have used.
    Nothing(String, Used),
    /// Nothing was sent, and why: the key is refused, or is not fetched.
    Unsent(String),
}

/// What a 401 or 403 for a request that sent a key says after the server's words.
const CHECK_KEY: &str = "; check the entry's key";

/// Which of a server's APIs its calls go through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Api {
    /// `<url>/chat/completions`, which every server an entry names speaks.
    OpenAi,
    /// Ollama's own `<url without /v1>/api/chat`, for `ollama:<model>`.
    Ollama,
}

impl Server {
    pub(super) fn new(name: &str, entry: &ServerEntry, model: Option<&str>) -> Server {
        Server {
            name: name.to_string(),
            entry: entry.clone(),
            model: model.map(ToString::to_string),
            key: SharedKey::new(KeyState::None),
            context: Context::Unknown,
            api: Api::OpenAi,
        }
    }

    /// `ollama:<model>`'s server: Ollama with its OpenAI API at `url`, as [`ollama_url`] gives
    /// it, which lists the models, its own API beside it taking the calls. No key, thinking off,
    /// nothing added to a request, and the whole context to each call: Ollama gives each request
    /// it serves at once a context of its own. Its label is `ollama:<model>` as given, whatever
    /// model it settles on: a run is found again, and its checks kept fresh, by its label.
    pub(super) fn ollama(model: &str, url: &str) -> Server {
        let entry = ServerEntry {
            url: url.to_string(),
            model: Some(model.to_string()),
            key: KeySource::None,
            thinking: Thinking::Off,
            request: Map::new(),
            shared_context: false,
        };
        Server {
            api: Api::Ollama,
            ..Server::new(&format!("ollama:{model}"), &entry, Some(model))
        }
    }

    /// How many tokens the server's context holds, as far as it says: `Unknown` until the server
    /// is readied.
    pub fn context(&self) -> Context {
        self.context
    }

    /// The cap the entry's `request` puts on every reply, with the field that sets it to a whole
    /// number: `max_tokens` or `max_completion_tokens` ([`REPLY_CAPS`]), the larger when it sets
    /// both. It is sent in place of the cap a call would send ([`body`]), so a reply may run that
    /// far.
    pub(super) fn own_cap(&self) -> Option<(&'static str, u64)> {
        REPLY_CAPS
            .into_iter()
            .filter_map(|field| Some((field, self.entry.request.get(field)?.as_u64()?)))
            .max_by_key(|&(_, cap)| cap)
    }

    /// One call: the prompt sent as a chat, its reply capped at `budget`'s cap when that is
    /// `Some`, and read as an OpenAI chat completion, or as Ollama's own API answers. A refusal
    /// for the context, or a prompt the server may have cut ([`Server::cut_at`]), is
    /// [`TooBig::Prompt`]; a reply cut off where the call sent a cap says which
    /// ([`Server::cut_off`]). An error has the key hidden, since a server may echo back the key it
    /// refused. The request goes as the key's rule says ([`Server::asked`]): a call whose key is
    /// refused sends nothing, so it has nothing to count. A 2xx marks the key as one that has
    /// worked ([`key::worked`]), which no other request's does. The one wait it reports through
    /// `waits` is a key command a 401 runs.
    pub(super) fn call(
        &self,
        prompt: &str,
        timeout: Duration,
        budget: Option<Budget>,
        waits: Waits<'_>,
    ) -> Answer {
        let max_reply = budget.map(Budget::cap);
        let Some(model) = &self.model else {
            return Answer {
                reply: Err("the model is not settled".into()),
                used: Used::Untracked,
                too_big: None,
            };
        };
        let (base, path) = self.endpoint();
        let body = match self.api {
            Api::OpenAi => body(
                model,
                prompt,
                self.entry.thinking,
                max_reply,
                &self.entry.request,
            ),
            Api::Ollama => ollama_body(model, prompt, max_reply),
        }
        .to_string();
        let ask = Ask {
            base,
            path,
            body: Some(&body),
            timeout,
            during: None,
        };
        let says = |status: u16, text: &str| {
            let mut why = error_text(status, text);
            if status == 404 {
                self.pull_hint(&mut why);
            }
            why
        };
        match self.asked(&ask, says, waits) {
            Told::Answer(text, key) => {
                // Only a chat's answer shows that a key works. Some servers (llama-server,
                // OpenRouter) list their models without checking the key: a wrong key marked at
                // `/models` would have its first chat's 401 run the key command a second time,
                // for a key that never worked.
                if let Some(sent) = &key {
                    key::worked(&self.entry.key, sent);
                }
                let mut answer = self.read(text, key.as_ref());
                // A cap the call sent may be the length limit the reply met, not the server's, so
                // the failure names it and what sets it.
                if answer.too_big == Some(TooBig::Reply)
                    && let Some(why) = self.cut_off(budget)
                {
                    answer.reply = Err(why);
                }
                if let Used::Reported(usage) = &answer.used
                    && let Some((limit, whose)) = self.cut_at(usage.input_tokens, budget)
                {
                    answer.reply = Err(format!(
                        "the prompt filled {} of {whose} {limit} tokens, so the server may have \
                         cut it",
                        usage.input_tokens
                    ));
                    answer.too_big = Some(TooBig::Prompt);
                }
                // An error `read` keeps as it was has not been hidden.
                answer.reply = answer.reply.map_err(|why| hide(key.as_ref(), why));
                answer
            }
            Told::Status { status, text, why } => Answer {
                reply: Err(why),
                used: Used::Unreported,
                // Read from the text as sent: a key that is a word of the refusal would hide it.
                too_big: refused_for_context(status, &text).then_some(TooBig::Prompt),
            },
            Told::Nothing(why, used) => Answer {
                reply: Err(why),
                used,
                too_big: None,
            },
            Told::Unsent(why) => Answer {
                reply: Err(why),
                used: Used::Untracked,
                too_big: None,
            },
        }
    }

    /// `ask` sent with the key, as the key's rule says, `says` wording an answer that is not 2xx
    /// from its status and its text, key hidden.
    ///
    /// A 2xx changes nothing about the key here: whether it shows that the key works is the
    /// caller's to say, and only a chat's does ([`Server::call`]). A 401 says the key is not
    /// accepted, so [`key::refetch`] is asked for another: with one, the calls send it from now on
    /// and `ask` is sent once more, that answer being the outcome; with none, or a key command
    /// that fails, the key is refused, and until the server is readied again nothing is sent:
    /// every call fails at once with this answer's words. That holds for a call whose 401 comes
    /// after another's refusal too: the key fetched for it is not sent. A key that has not
    /// worked is not fetched again, which would be a second passphrase window for a key likely
    /// to be the same, so the request sent once more is the last this call sends: a 401 for it
    /// asks [`key::refetch`] too, which refuses its key, or gives the next call the key fetched
    /// since another call's request made this one work. A 403 is left to its own call: a server
    /// can refuse one request that way (a chunk it moderates, a model the key may not use), so
    /// this scan's calls go on with the key, and the next scan or recheck runs the key command
    /// again ([`key::forbidden`]). Both say to check the entry's key. A request that sent no key
    /// changes nothing and says nothing of one. A key command a 401 runs is reported through
    /// `waits` ([`Server::once`]).
    fn asked(&self, ask: &Ask<'_>, says: impl Fn(u16, &str) -> String, waits: Waits<'_>) -> Told {
        let key = match self.sending() {
            Ok(key) => key,
            Err(why) => return Told::Unsent(why),
        };
        match self.once(ask, key, &says, waits) {
            (_, Some(fetched)) => self.once(ask, Some(fetched), &says, waits).0,
            (told, None) => told,
        }
    }

    /// One request of [`Server::asked`]: `ask` sent with `key`, and what its answer does about
    /// the key. With how it ended, the key a 401 fetched to send in its place, when the server
    /// still sends one. A key command the 401 runs is reported through `waits` as it starts, and
    /// once it ends, the wait `ask` belongs to as under way again, or none.
    fn once(
        &self,
        ask: &Ask<'_>,
        key: Option<Key>,
        says: &impl Fn(u16, &str) -> String,
        waits: Waits<'_>,
    ) -> (Told, Option<Key>) {
        let source = &self.entry.key;
        match (self.send(ask, key.as_ref()), key) {
            (Ok((status, text)), key) if (200..300).contains(&status) => {
                (Told::Answer(text, key), None)
            }
            (Ok((status, text)), None) => {
                let why = says(status, &text);
                (Told::Status { status, text, why }, None)
            }
            (Ok((status, text)), Some(sent)) => {
                // Hidden before `says` cuts the server's words, so no part of a key is left that
                // `hide` cannot find.
                let mut why = says(status, &sent.hide(&text));
                let fetched = match status {
                    401 => {
                        why.push_str(CHECK_KEY);
                        // The state's lock is not held here: the command can wait on a
                        // passphrase window.
                        let running = |on: bool| {
                            waits(if on {
                                Some(self.key_wait())
                            } else {
                                ask.during.cloned()
                            });
                        };
                        key::refetch(source, &sent, &running).unwrap_or_else(|failed| {
                            let _ = write!(why, "; {failed}");
                            None
                        })
                    }
                    403 => {
                        key::forbidden(source, &sent);
                        why.push_str(CHECK_KEY);
                        None
                    }
                    _ => None,
                };
                // Hidden again: words read out of the server's JSON can hold a key that the
                // text's escapes kept from the first pass.
                let why = sent.hide(&why);
                if status != 401 {
                    return (Told::Status { status, text, why }, None);
                }
                let next = match &fetched {
                    Some(new) => KeyState::Sending(new.clone()),
                    None => KeyState::Refused(Box::new(Refusal {
                        key: sent.clone(),
                        why: why.clone(),
                    })),
                };
                // A server another call's refusal has stopped sends nothing more, the key
                // fetched here included: its calls stop together.
                let stopped = matches!(self.key.replace(&sent, next), KeyState::Refused(_));
                let fetched = fetched.filter(|_| !stopped);
                (Told::Status { status, text, why }, fetched)
            }
            (Err(e), key) => {
                let (why, used) = failed(ask.base, ask.path, &e, ask.timeout);
                (Told::Nothing(hide(key.as_ref(), why), used), None)
            }
        }
    }

    /// `ask` sent with `key`, when there is one: the answer's status and text.
    fn send(&self, ask: &Ask<'_>, key: Option<&Key>) -> Result<(u16, String), ureq::Error> {
        let agent = agent(&self.entry.url);
        let url = format!("{}/{}", ask.base, ask.path);
        let answer = match ask.body {
            Some(body) => prepared(agent.post(url), key, ask.timeout)
                .header("Content-Type", "application/json")
                .send(body),
            None => prepared(agent.get(url), key, ask.timeout).call(),
        };
        answer.and_then(status_and_text)
    }

    /// The key a request sends now, `None` for an entry without one; or why nothing is sent: the
    /// refusal of a refused key, or a key not fetched, which the server would refuse.
    fn sending(&self) -> Result<Option<Key>, String> {
        match (self.key.get(), &self.entry.key) {
            (KeyState::Sending(key), _) => Ok(Some(key)),
            (KeyState::Refused(refusal), _) => Err(refusal.why),
            (KeyState::None, KeySource::None) => Ok(None),
            (KeyState::None, _) => Err("the key is not fetched".into()),
        }
    }

    /// Why a reply cut off at its length limit fails, when a call with `budget` sent a cap: the
    /// cap, and what sets it. The entry's own ([`Server::own_cap`]) is sent whenever it sets one;
    /// the server's limit is named beside it when that may have come first: with no `budget`, no
    /// size known, or with a cap over the call's share, which is all the room the reply has
    /// ([`Budget::new`]). Else the cap is subcheck's ([`Budget::cap`]) when there is a `budget`,
    /// which an entry's `request.max_tokens` replaces and `ollama:<model>`, with no entry, keeps.
    /// `None` with neither: the server's limit was the only one.
    fn cut_off(&self, budget: Option<Budget>) -> Option<String> {
        let why = match (self.own_cap(), budget, self.api) {
            (Some((field, cap)), Some(budget), _) if cap <= budget.share() => {
                format!("the reply was cut off at the entry's request.{field} of {cap} tokens")
            }
            (Some((field, cap)), _, _) => format!(
                "the reply was cut off at the entry's request.{field} of {cap} tokens or the \
                 server's length limit"
            ),
            (None, Some(budget), Api::OpenAi) => format!(
                "the reply was cut off at subcheck's cap of {} tokens; the entry's \
                 request.max_tokens sets another",
                budget.cap()
            ),
            (None, Some(budget), Api::Ollama) => format!(
                "the reply was cut off at subcheck's cap of {} tokens",
                budget.cap()
            ),
            (None, None, _) => return None,
        };
        Some(why)
    }

    /// The limit a prompt of `prompt_tokens` filled, so that the server may have cut it
    /// ([`filled`]), and whose it is: with `budget`, the call's share, for a prompt over the room
    /// the budget left it ([`Budget::prompt`]); with none, the whole context, when it is known.
    /// `None` for a prompt taken as whole.
    fn cut_at(&self, prompt_tokens: u64, budget: Option<Budget>) -> Option<(u64, &'static str)> {
        // A server may cut a prompt that fills its context and answer the rest, so a prompt that
        // came to nearly all of the call's share is taken as cut. An entry's own cap under a
        // twentieth of the share leaves the prompt more than 95% of it, and a prompt the sizing
        // chose must not read as cut.
        match (budget, self.context) {
            (Some(budget), _) => (filled(prompt_tokens, budget.share())
                && prompt_tokens > budget.prompt())
            .then_some((budget.share(), "a call's")),
            (None, Context::Tokens(context)) => {
                filled(prompt_tokens, context).then_some((context, "the server's"))
            }
            (None, _) => None,
        }
    }

    /// The server readied for calls, as [`Backend::ready`](super::Backend::ready) says, with the
    /// warnings to show. An error has the key hidden, since a server may echo back the key it
    /// refused. It reports through `waits` the key command while it runs, then the check from
    /// before `/models` is asked to after `/api/ps` is, or to whatever fails first.
    pub(super) fn ready(mut self, waits: Waits<'_>) -> Result<(Server, Vec<String>), String> {
        let key_wait = self.key_wait();
        let running = |on: bool| waits(on.then(|| key_wait.clone()));
        let key = key::fetch(&self.entry.key, &running)?;
        let keyed = key.is_some();
        // The key as fetched, whatever state the server came with: a scan starts unrefused.
        self.key = SharedKey::new(key.map_or(KeyState::None, KeyState::Sending));
        // Worded once the key is fetched, so that the key is hidden from it.
        let line = format!("{}: asking {} what it serves", self.name, self.entry.url);
        let asking = Wait::Asking(self.hidden(line));
        let _checking = Waiting::start(waits, asking.clone());
        let models = self.models(&asking, waits)?;
        let ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
        // The ids are the server's, so the key is hidden: the one sent to `/models`.
        let model = settle(&self.entry.url, self.model.as_deref(), &ids).map_err(|mut why| {
            self.pull_hint(&mut why);
            self.hidden(why)
        })?;
        let context = models
            .iter()
            .find(|m| m.id == model)
            .and_then(Listed::context);
        self.model = Some(model);
        self.context = context.map_or_else(|| self.ps(), Context::Tokens);
        let warning = keyed
            .then(|| key_warning(&self.label(), &self.entry.url))
            .flatten();
        Ok((self, warning.into_iter().collect()))
    }

    /// The server sized, as [`Backend::sized`](super::Backend::sized) says: with a context that is
    /// `NotLoaded`, its model loaded within `timeout` and `/api/ps` asked again. An error has the
    /// key hidden, since a server may echo back the key it refused. The load is reported through
    /// `waits` from before it starts to after `/api/ps` is asked again, or to its failure.
    pub(super) fn sized(mut self, timeout: Duration, waits: Waits<'_>) -> Result<Server, String> {
        if let Some((root, model)) = self.to_load() {
            // The model's name is the server's, so the key is hidden.
            let line = self.hidden(format!("loading {model} into Ollama at {root}"));
            let _loading = Waiting::start(waits, Wait::Loading(line));
            self.load(root, model, timeout)?;
            // A model still not listed once loaded leaves the context `Unknown`, not `NotLoaded`,
            // so nothing loads it again.
            self.context = match self.ps() {
                Context::Tokens(tokens) => Context::Tokens(tokens),
                _ => Context::Unknown,
            };
        }
        Ok(self)
    }

    /// Where [`Server::sized`] has Ollama load a model, and which: Ollama's root and the settled
    /// model, while the context is `NotLoaded`; `None` when sizing loads nothing.
    fn to_load(&self) -> Option<(&str, &str)> {
        match (self.context, root(&self.entry.url), &self.model) {
            (Context::NotLoaded, Some(root), Some(model)) => Some((root, model)),
            _ => None,
        }
    }

    /// The entry's key command running, worded here alone so that readying the server and a
    /// call's 401 name it the same. The line names the entry, never the command, whose text can
    /// hold a key.
    fn key_wait(&self) -> Wait {
        Wait::KeyCommand(format!("{}: waiting for its key command", self.name))
    }

    /// Ollama made to load `model`: `POST <root>/api/generate` naming the model and no prompt,
    /// which loads it and generates nothing. Any 2xx answer within `timeout` is a load. A 401 or
    /// 403 changes nothing about the key: the refusal may be of the path alone, and the chats
    /// settle whether the server takes the key.
    fn load(&self, root: &str, model: &str, timeout: Duration) -> Result<(), String> {
        let key = self.sending()?;
        let key = key.as_ref();
        let body = serde_json::json!({"model": model}).to_string();
        let ask = Ask {
            base: root,
            path: "api/generate",
            body: Some(&body),
            timeout,
            during: None,
        };
        let why = match self.send(&ask, key) {
            Ok((status, _)) if (200..300).contains(&status) => return Ok(()),
            Ok((status, text)) => error_text(status, &hide(key, text)),
            Err(e) => hide(key, failed(root, ask.path, &e, timeout).0),
        };
        Err(format!("Ollama could not load {model}: {why}"))
    }

    /// Where [`Server::ps`] asks Ollama's `/api/ps`: Ollama's root, for `ollama:<model>` and for
    /// an entry whose url is plain `http://`, as Ollama's is on this machine or a LAN. `None` for
    /// an entry over `https://`: a hosted API has no `/api/ps`, and the request would carry its key
    /// to a path it does not serve. `None` too for a url with no root.
    fn ps_root(&self) -> Option<&str> {
        let url = &self.entry.url;
        let plain = url
            .parse::<Uri>()
            .is_ok_and(|uri| uri.scheme_str() == Some("http"));
        (self.api == Api::Ollama || plain).then_some(root(url)?)
    }

    /// What Ollama's `GET <root>/api/ps`, its list of loaded models, says of the settled model's
    /// context; `Unknown` for a server not asked ([`Server::ps_root`]) or any failure. A 401 or
    /// 403 changes nothing about the key: a server that is no Ollama may well refuse the path
    /// alone, and the chats settle whether the server takes the key.
    fn ps(&self) -> Context {
        let (Some(root), Some(model)) = (self.ps_root(), &self.model) else {
            return Context::Unknown;
        };
        let Ok(key) = self.sending() else {
            return Context::Unknown;
        };
        let ask = Ask {
            base: root,
            path: "api/ps",
            body: None,
            timeout: MODELS_TIMEOUT,
            during: None,
        };
        match self.send(&ask, key.as_ref()) {
            Ok((status, text)) if (200..300).contains(&status) => ps_context(&text, model),
            _ => Context::Unknown,
        }
    }

    /// What a backend through this server records: `<name>@<model>`, or the name alone while the
    /// model is not settled and, always, for Ollama's (`ollama:<model>`).
    pub(super) fn label(&self) -> String {
        match (self.api, &self.model) {
            (Api::OpenAi, Some(model)) => format!("{}@{model}", self.name),
            (Api::OpenAi, None) | (Api::Ollama, _) => self.name.clone(),
        }
    }

    /// What sets the lines in a chunk sent to this server: the entry's `chunk_lines`, or
    /// `--chunk-lines` for Ollama's, which has no entry.
    pub(super) fn size_setting(&self) -> &'static str {
        match self.api {
            Api::OpenAi => "chunk_lines",
            Api::Ollama => "--chunk-lines",
        }
    }

    /// Where a call posts its chat, as a run records it.
    pub(super) fn chat_url(&self) -> String {
        let (base, path) = self.endpoint();
        format!("{base}/{path}")
    }

    /// The base and path a call posts to: `<url>` and `chat/completions`, or for Ollama's own API
    /// `<url without /v1>` and `api/chat`.
    fn endpoint(&self) -> (&str, &'static str) {
        let url = self.entry.url.as_str();
        match self.api {
            Api::OpenAi => (url, "chat/completions"),
            // `ollama_url` ends every url it gives in `/v1`.
            Api::Ollama => (root(url).unwrap_or(url), "api/chat"),
        }
    }

    /// `why`, a refusal of a model the server lacks, with how to fetch it, for Ollama's: its API
    /// does not fetch a model it lacks. An entry's server need not be Ollama, so its errors stay as
    /// the server words them.
    fn pull_hint(&self, why: &mut String) {
        if let (Api::Ollama, Some(model)) = (self.api, &self.entry.model) {
            let _ = write!(why, "; ollama pull {model} fetches it");
        }
    }

    /// The models `GET <url>/models` lists, asked as the key's rule says ([`Server::asked`]). An
    /// error has hidden the key its request sent. `asking` is the check's wait, under way again
    /// once a key command a 401 runs through `waits` has ended.
    fn models(&self, asking: &Wait, waits: Waits<'_>) -> Result<Vec<Listed>, String> {
        let url = &self.entry.url;
        let ask = Ask {
            base: url,
            path: "models",
            body: None,
            timeout: MODELS_TIMEOUT,
            during: Some(asking),
        };
        let says = |status: u16, text: &str| match server_said(text) {
            Some(said) => format!("HTTP {status} from {url}/models: {said}"),
            None => format!("HTTP {status} from {url}/models"),
        };
        match self.asked(&ask, says, waits) {
            Told::Answer(text, key) => {
                // Read before the key is hidden: a model's id may hold it.
                let Ok(list) = serde_json::from_str::<ModelList>(&text) else {
                    let text = hide(key.as_ref(), text);
                    let first = text.trim().lines().next().unwrap_or("(empty)");
                    return Err(format!("{url}/models gave no list of models: {first:.80}"));
                };
                Ok(list.data)
            }
            Told::Status { why, .. } | Told::Nothing(why, _) | Told::Unsent(why) => Err(why),
        }
    }

    /// A 2xx reply read as a chat completion, or as Ollama's own API answers. What succeeds keeps
    /// the key as the server sent it: a key can be a plain word, which a correction may hold, and
    /// hiding it would change the correction or break the JSON around it. An error is read again
    /// from the text with `key`, the one the request sent, hidden, so a message cut through an
    /// echoed key keeps no part of it that `hide` cannot find.
    fn read(&self, text: String, key: Option<&Key>) -> Answer {
        let read = |text: &str| match self.api {
            Api::OpenAi => Output::OpenAi.read(text),
            Api::Ollama => read_ollama(text),
        };
        let answer = read(&text);
        if answer.reply.is_ok() {
            return answer;
        }
        let hidden = read(&hide(key, text)).reply;
        Answer {
            // Hiding the key can change what the JSON says, so an error stays one.
            reply: hidden.and(answer.reply),
            used: answer.used,
            too_big: answer.too_big,
        }
    }

    /// `text` with the server's key hidden, the one it sends or the one refused, when it has
    /// one: for text that comes of no request of its own.
    fn hidden(&self, text: String) -> String {
        match self.key.get() {
            KeyState::Sending(key) => key.hide(&text),
            KeyState::Refused(refusal) => refusal.key.hide(&text),
            KeyState::None => text,
        }
    }
}

/// `request` with what every request to a server carries: `timeout`, and `key` when there is
/// one.
fn prepared<B>(
    request: RequestBuilder<B>,
    key: Option<&Key>,
    timeout: Duration,
) -> RequestBuilder<B> {
    let request = request.config().timeout_global(Some(timeout)).build();
    match key {
        Some(key) => request.header("Authorization", key.bearer()),
        None => request,
    }
}

/// `text` with `key` hidden, when there is one.
fn hide(key: Option<&Key>, text: String) -> String {
    match key {
        Some(key) => key.hide(&text),
        None => text,
    }
}

/// A response's status and body, the body read whatever the status, since an error's says why.
/// The key is left in: each path that turns the body into a message hides it there, before any cut.
fn status_and_text(mut response: Response<Body>) -> Result<(u16, String), ureq::Error> {
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string()?;
    Ok((status, text))
}

/// What `GET <url>/models` answers, as far as it is read: each model's id and context.
#[derive(Deserialize)]
struct ModelList {
    data: Vec<Listed>,
}

/// A model the list names. Each server puts the context in a field of its own; they are read as
/// any JSON, so one that is odd (a string, a null) is no context rather than a list that fails.
#[derive(Deserialize)]
struct Listed {
    id: String,
    /// llama-server's, as `n_ctx`.
    #[serde(default)]
    meta: Value,
    /// vLLM's.
    #[serde(default)]
    max_model_len: Value,
    /// Some hosted APIs'.
    #[serde(default)]
    context_length: Value,
}

impl Listed {
    /// The first field that gives a size: `meta.n_ctx`, `max_model_len`, `context_length`.
    fn context(&self) -> Option<u64> {
        tokens(&self.meta["n_ctx"])
            .or_else(|| tokens(&self.max_model_len))
            .or_else(|| tokens(&self.context_length))
    }
}

/// A context field's size: a whole number above 0. A 0 is no size, as if the field were absent,
/// since no chunk fits a context of nothing.
fn tokens(field: &Value) -> Option<u64> {
    field.as_u64().filter(|&n| n > 0)
}

/// Where Ollama's own API is, beside its OpenAI one: `url` without its `/v1`. A url not ending
/// in `/v1` is not Ollama's, so it has none, and nothing is asked of a path it may not have.
fn root(url: &str) -> Option<&str> {
    url.strip_suffix("/v1")
}

/// What `/api/ps`'s `text` says of `model`'s context: the `context_length` of the loaded model
/// whose `name` or `model` it is, `Unknown` without one that gives a size, `NotLoaded` when no
/// loaded model is it, and `Unknown` for a text that is no list.
fn ps_context(text: &str, model: &str) -> Context {
    let Ok(ps) = serde_json::from_str::<Value>(text) else {
        return Context::Unknown;
    };
    let Some(loaded) = ps["models"].as_array() else {
        return Context::Unknown;
    };
    let is_model = |m: &&Value| m["name"] == model || m["model"] == model;
    match loaded.iter().find(is_model) {
        Some(m) => tokens(&m["context_length"]).map_or(Context::Unknown, Context::Tokens),
        None => Context::NotLoaded,
    }
}

/// The model the calls ask for: `asked` (the entry's or the run's) as the server serves it
/// ([`same_model`]), the name as asked before its `:latest`, else the only model it serves.
fn settle(url: &str, asked: Option<&str>, ids: &[String]) -> Result<String, String> {
    match (asked, ids) {
        (_, []) => Err(format!("{url} serves no model")),
        (Some(asked), _) => ids
            .iter()
            .find(|id| *id == asked)
            .or_else(|| ids.iter().find(|id| same_model(asked, id)))
            .cloned()
            .ok_or_else(|| format!("{url} does not serve {asked:?}; it serves {}", listed(ids))),
        (None, [only]) => Ok(only.clone()),
        (None, _) => Err(format!(
            "{url} serves {}: name one as the entry's model",
            listed(ids)
        )),
    }
}

/// Whether a server that serves `served` serves the model asked for as `asked`: the same name, or
/// `asked` with no tag and `served` its `:latest`, which is how Ollama names a model pulled with
/// none. A run records the name the server gave, so it is found again by an entry that names the
/// model without its tag.
pub(crate) fn same_model(asked: &str, served: &str) -> bool {
    asked == served || (!asked.contains(':') && served.strip_suffix(":latest") == Some(asked))
}

/// Where `ollama:<model>` finds Ollama's OpenAI API: at `host`, which is `OLLAMA_HOST`, read as
/// the `ollama` CLI reads it, then `/v1`. A host with no scheme is `http` on port 11434;
/// `http://` with no port is port 80, and `https://` 443. A path after the host is kept. An empty
/// host is this machine, `127.0.0.1`, as is one no URL can hold; an invalid port is the default.
/// So the address a server binds to (`0.0.0.0`, `:11434`) names the one the CLI calls.
pub(crate) fn ollama_url(host: Option<&str>) -> String {
    let host = host.map_or("", |h| h.trim().trim_matches(['"', '\'']).trim());
    let (scheme, rest, default_port): (_, _, u16) = match host.split_once("://") {
        None => ("http", host, 11434),
        Some(("http", rest)) => ("http", rest, 80),
        Some(("https", rest)) => ("https", rest, 443),
        Some((scheme, rest)) => (scheme, rest, 11434),
    };
    let (hostport, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = if let Some((host, port)) = split_host_port(hostport) {
        (host.to_string(), port.parse().unwrap_or(default_port))
    } else {
        // No port, so all of it is the host: an IP address, bracketed or not, in its usual form.
        let ip = hostport.trim_matches(['[', ']']).parse::<IpAddr>();
        let host = ip.map_or_else(|_| hostport.to_string(), |ip| ip.to_string());
        (host, default_port)
    };
    // An IPv6 address is bracketed, so its colons are not read as the port's.
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    let host = if host.is_empty() || format!("http://{host}/").parse::<Uri>().is_err() {
        "127.0.0.1".to_string()
    } else {
        host
    };
    let path = path.trim_end_matches('/');
    let path = if path.is_empty() {
        String::new()
    } else {
        format!("/{path}")
    };
    format!("{scheme}://{host}:{port}{path}/v1")
}

/// `hostport` split into a host and a port, as Go's `net.SplitHostPort`, which the `ollama` CLI
/// reads `OLLAMA_HOST` with, splits it: `<host>:<port>`, the host bracketed when it is IPv6, the
/// port possibly empty. `None` without a port, and for a host with colons and no brackets, which
/// is an IPv6 address alone.
fn split_host_port(hostport: &str) -> Option<(&str, &str)> {
    if let Some(bracketed) = hostport.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']')?;
        return Some((host, after.strip_prefix(':')?));
    }
    let (host, port) = hostport.rsplit_once(':')?;
    (!host.contains(':')).then_some((host, port))
}

/// Up to ten of `ids`, quoted, then how many more: a cloud API may list hundreds, and an error
/// stays one line.
fn listed(ids: &[String]) -> String {
    let mut listed = ids
        .iter()
        .take(10)
        .map(|id| format!("{id:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    if ids.len() > 10 {
        let _ = write!(listed, " and {} more", ids.len() - 10);
    }
    listed
}

/// The warning for a key sent to `url` over plain `http://` to another machine. Only a warning:
/// a server on a network its owner trusts is theirs to choose.
pub(super) fn key_warning(label: &str, url: &str) -> Option<String> {
    let uri = url.parse::<Uri>().ok()?;
    if uri.scheme_str() != Some("http") || on_this_machine(url) {
        return None;
    }
    let host = uri.host()?;
    Some(format!(
        "{label} sends its key to {host} over plain http://, where anyone on the network between \
         can read it"
    ))
}

/// Why a request to `<url>/<path>` got no answer, and what it may have used: a refused or
/// unresolvable connection cost nothing; anything else, a timeout too, may have.
fn failed(url: &str, path: &str, error: &ureq::Error, timeout: Duration) -> (String, Used) {
    let nothing = || format!("nothing answering at {url}: is the server running?");
    match error {
        ureq::Error::Timeout(_) => (
            format!("{url}/{path} timed out after {}s", timeout.as_secs()),
            Used::Unreported,
        ),
        ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => (nothing(), Used::Untracked),
        ureq::Error::Io(io) if io.kind() == ErrorKind::ConnectionRefused => {
            (nothing(), Used::Untracked)
        }
        _ => (format!("{url}: {error}"), Used::Unreported),
    }
}

/// The agent for calls to `url`, one for this machine and one for the rest, each made once so
/// calls share its connections. A server on this machine is called directly, as most tools call
/// one: a proxy set in the environment for the web would otherwise take calls to 127.0.0.1 unless
/// `NO_PROXY` names it. Others go through the environment's proxy, which honours `NO_PROXY`.
fn agent(url: &str) -> &'static Agent {
    static DIRECT: OnceLock<Agent> = OnceLock::new();
    static PROXIED: OnceLock<Agent> = OnceLock::new();
    let direct = on_this_machine(url);
    let agent = if direct { &DIRECT } else { &PROXIED };
    agent.get_or_init(|| {
        let tls = TlsConfig::builder()
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        // An error status is an answer like any other, so its body, which says why, can be read.
        let config = Agent::config_builder()
            .http_status_as_error(false)
            .tls_config(tls);
        let config = if direct { config.proxy(None) } else { config };
        config.build().new_agent()
    })
}

/// Whether `url`'s host is this machine: `localhost` in any case, a loopback address, or the
/// unspecified one (`0.0.0.0`, `[::]`), which a server's start-up log can print as its address
/// and which a connection takes as this machine.
pub(super) fn on_this_machine(url: &str) -> bool {
    let Ok(uri) = url.parse::<Uri>() else {
        return false;
    };
    let Some(host) = uri.host() else {
        return false;
    };
    // An IPv6 address keeps its brackets in a URI's host.
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
}

/// The fields of an entry's `request` that cap a reply: `max_tokens`, and `max_completion_tokens`,
/// OpenAI's newer name for it, which its reasoning models take in place of it.
const REPLY_CAPS: [&str; 2] = ["max_tokens", "max_completion_tokens"];

/// A call's request body: the chat, the reply's cap as `max_tokens` when there is one and the
/// entry's `request` sets neither of [`REPLY_CAPS`], the switches that turn thinking off when it
/// is, then each field of the entry's `request` over the top.
fn body(
    model: &str,
    prompt: &str,
    thinking: Thinking,
    max_reply: Option<u64>,
    request: &Map<String, Value>,
) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), model.into());
    body.insert("messages".into(), messages(prompt));
    body.insert("stream".into(), false.into());
    // The entry's own cap is the room its reply was sized with (`Server::own_cap`). Sent beside
    // it, the call's could hold the reply to less, and OpenAI's reasoning models refuse a request
    // that has `max_tokens` at all.
    let own_cap = REPLY_CAPS.iter().any(|field| request.contains_key(*field));
    if let Some(cap) = max_reply
        && !own_cap
    {
        body.insert("max_tokens".into(), cap.into());
    }
    if thinking == Thinking::Off {
        body.insert("reasoning_effort".into(), "none".into());
        body.insert(
            "chat_template_kwargs".into(),
            serde_json::json!({"enable_thinking": false}),
        );
    }
    body.extend(request.clone());
    Value::Object(body)
}

/// A call's request body for Ollama's own API: the chat, no streaming, thinking off,
/// `truncate: false`, so a prompt longer than the context is refused rather than cut, and the
/// reply's cap as `options.num_predict` when there is one.
fn ollama_body(model: &str, prompt: &str, max_reply: Option<u64>) -> Value {
    let mut body = serde_json::json!({
        "model": model,
        "messages": messages(prompt),
        "stream": false,
        "think": false,
        "truncate": false,
    });
    if let Some(cap) = max_reply {
        body["options"] = serde_json::json!({"num_predict": cap});
    }
    body
}

/// A call's chat: [`SYSTEM`] as the system message, the prompt as the user's.
fn messages(prompt: &str) -> Value {
    serde_json::json!([
        {"role": "system", "content": SYSTEM},
        {"role": "user", "content": prompt},
    ])
}

/// What Ollama's `/api/chat` answers, as far as it is read.
#[derive(Deserialize)]
struct OllamaReply {
    message: Option<OllamaMessage>,
    done_reason: Option<String>,
    /// All of the prompt, the part read from the cache with it.
    prompt_eval_count: Option<u64>,
    prompt_eval_cached_count: Option<u64>,
    eval_count: Option<u64>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct OllamaMessage {
    content: Option<String>,
}

/// A 2xx answer of Ollama's `/api/chat`, read as an OpenAI chat completion is ([`classify`]):
/// the reply with a newline after it, and a reply cut off at the length limit
/// [`TooBig::Reply`]; the tokens as Ollama counts them, unreported without `prompt_eval_count`.
fn read_ollama(text: &str) -> Answer {
    let Ok(r) = serde_json::from_str::<OllamaReply>(text) else {
        let first = text.trim().lines().next().unwrap_or("(empty)");
        return Answer {
            reply: Err(format!("reply is not an Ollama chat reply: {first:.80}")),
            used: Used::Unreported,
            too_big: None,
        };
    };
    let used = r.prompt_eval_count.map_or(Used::Unreported, |input| {
        Used::Reported(Usage {
            input_tokens: input,
            output_tokens: r.eval_count.unwrap_or(0),
            cache_read_tokens: r.prompt_eval_cached_count,
            cache_write_tokens: None,
            cost_usd: None,
        })
    });
    // Ollama gives no reason that cuts a reply short but the length limit.
    let (reply, too_big) = classify(
        r.error,
        r.done_reason.as_deref(),
        &[],
        r.message.and_then(|m| m.content),
    );
    Answer {
        reply,
        used,
        too_big,
    }
}

/// The server's own words in an error body, cut at 200 characters: `error.message`, as OpenAI
/// sends it, else `error` when it is a string, else a top-level `message`, else `detail`, as
/// vLLM's FastAPI words some (`Not Found` for a url without `/v1`), else the body's first line
/// that has any text; `None` for a body with none. Servers word their errors each their own way,
/// and a proxy in front of one may answer in plain text or HTML.
fn server_said(body: &str) -> Option<String> {
    let json = serde_json::from_str::<Value>(body).ok();
    let said = json.as_ref().and_then(|v| {
        [
            &v["error"]["message"],
            &v["error"],
            &v["message"],
            &v["detail"],
        ]
        .into_iter()
        .find_map(|m| m.as_str().map(str::trim).filter(|m| !m.is_empty()))
    });
    let said = said.or_else(|| body.lines().map(str::trim).find(|l| !l.is_empty()))?;
    Some(format!("{said:.200}"))
}

/// Whether a non-2xx answer refused the prompt as too big for the context: llama-server's
/// `exceed_context_size_error` type, OpenAI's (and OpenRouter's) `context_length_exceeded` code,
/// or the server's words, as vLLM and Ollama word theirs. That takes in llama-server's 500 for
/// calls in flight that outgrew the context they share, which a smaller chunk can fit too.
fn refused_for_context(status: u16, body: &str) -> bool {
    if (200..300).contains(&status) {
        return false;
    }
    let json = serde_json::from_str::<Value>(body).unwrap_or_default();
    let error = &json["error"];
    if error["type"] == "exceed_context_size_error" || error["code"] == "context_length_exceeded" {
        return true;
    }
    server_said(body).is_some_and(|said| {
        let said = said.to_lowercase();
        [
            "context length",
            "context size",
            "context window",
            "maximum context",
        ]
        .iter()
        .any(|words| said.contains(words))
    })
}

/// Whether a prompt of `prompt_tokens` filled `limit` tokens, a call's share of a context or the
/// whole of it: 95% of it or more, close enough that the server may have cut the prompt to fit, and
/// with little room left for a reply.
fn filled(prompt_tokens: u64, limit: u64) -> bool {
    // Widened, since both counts come from the server and either times 20 could overflow a u64.
    u128::from(prompt_tokens) * 20 >= u128::from(limit) * 19
}

/// `HTTP <status>`, then the server's words when it says any.
fn error_text(status: u16, body: &str) -> String {
    match server_said(body) {
        Some(said) => format!("HTTP {status}: {said}"),
        None => format!("HTTP {status}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::time::Instant;

    use super::*;
    use crate::llm::{Backend, JOBS, TIMEOUT, Usage, Via};
    use crate::settings::{KeySource, Settings};
    use crate::test_support::{
        FakeServer, Reply, Request, closed_port_url, fitting_lines, models_reply, ps_reply,
    };

    const OPENAI: &str = include_str!("../../tests/fixtures/llm/openai.json");

    fn backend(url: &str, extra: &str) -> Backend {
        let text = format!("[llm.srv]\nurl = \"{url}\"\nmodel = \"m\"\n{extra}");
        Backend::named(
            "srv",
            &Settings::parse(Path::new("/c/config.toml"), &text).unwrap(),
        )
        .unwrap()
    }

    fn with_key(b: Backend, source: &KeySource) -> Backend {
        let Via::Server(mut s) = b.via else {
            unreachable!()
        };
        let key = key::fetch(source, &|_| {}).unwrap();
        s.key = SharedKey::new(key.map_or(KeyState::None, KeyState::Sending));
        Backend {
            via: Via::Server(s),
            ..b
        }
    }

    /// The server `b` calls.
    fn server(b: &Backend) -> &Server {
        let Via::Server(s) = &b.via else {
            unreachable!()
        };
        s
    }

    /// An entry `srv` at `url` whose key command adds a line to `dir`'s `count` each run, then
    /// runs `print`; not readied, so the command has not run.
    fn entry_by(url: &str, dir: &Path, print: &str) -> (Backend, impl Fn() -> usize + use<>) {
        let count = dir.join("count");
        let cmd = format!("echo x >> '{}'; {print}", count.display());
        let entry = backend(url, &format!("api_key_cmd = \"{cmd}\"\n"));
        let runs = move || std::fs::read_to_string(&count).unwrap().lines().count();
        (entry, runs)
    }

    /// [`entry_by`] at `fake`, readied, so its first key is fetched and no chat has shown that it
    /// works.
    fn readied_by(
        fake: &FakeServer,
        dir: &Path,
        print: &str,
    ) -> (Backend, impl Fn() -> usize + use<>) {
        let (entry, runs) = entry_by(&fake.url, dir, print);
        let (b, _) = entry.ready(&|_| {}).unwrap();
        (b, runs)
    }

    /// Each wait `run` reports through the [`Waits`] it is given, in the order reported.
    fn heard(run: impl FnOnce(Waits<'_>)) -> Vec<Option<Wait>> {
        let heard = Mutex::new(Vec::new());
        run(&|wait| heard.lock().unwrap().push(wait));
        heard.into_inner().unwrap()
    }

    /// `srv`'s key command running, as its waits name it.
    fn key_command() -> Wait {
        Wait::KeyCommand("srv: waiting for its key command".into())
    }

    /// `srv` asking the server at `url` what it serves, as its waits name it.
    fn asking(url: &str) -> Wait {
        Wait::Asking(format!("srv: asking {url} what it serves"))
    }

    /// [`readied_by`], with its first key marked as one that has worked, as a first good chat
    /// leaves it. Marked by hand, not by a chat: most of these fakes refuse every chat, and a chat
    /// sent here would be among the ones a test counts.
    fn keyed_by(
        fake: &FakeServer,
        dir: &Path,
        print: &str,
    ) -> (Backend, impl Fn() -> usize + use<>) {
        let (b, runs) = readied_by(fake, dir, print);
        let KeyState::Sending(sent) = server(&b).key.get() else {
            unreachable!()
        };
        key::worked(&server(&b).entry.key, &sent);
        (b, runs)
    }

    /// An entry `srv` at `fake` whose key command prints `key`'s contents, a line added to `count`
    /// each run; readied, its first key marked as one that has worked ([`keyed_by`]).
    fn keyed(fake: &FakeServer, dir: &Path) -> (Backend, PathBuf, impl Fn() -> usize) {
        let key = dir.join("key");
        let (b, runs) = keyed_by(fake, dir, &format!("cat '{}'", key.display()));
        (b, key, runs)
    }

    /// `b`'s entry at `url`, not readied.
    fn moved(b: &Backend, url: &str) -> Backend {
        let s = server(b);
        let entry = ServerEntry {
            url: url.to_string(),
            ..s.entry.clone()
        };
        Backend {
            via: Via::Server(Server::new(&s.name, &entry, Some("m"))),
            ..b.clone()
        }
    }

    /// A server that lists `m` whatever the key, and answers each chat with `chat` of its request.
    /// It has no other path, so `chat` sees no request but the calls'.
    fn chatting(chat: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> FakeServer {
        FakeServer::start(move |r| match r.path.as_str() {
            "/v1/models" => models_reply(&["m"]),
            "/v1/chat/completions" => chat(r),
            _ => Reply::status(404, "{}"),
        })
    }

    /// [`chatting`], with `meanwhile` run on the backend put in the cell before the first chat is
    /// answered: what other workers' calls do while that request is in flight.
    fn chatting_while(
        meanwhile: impl Fn(&Backend) + Send + Sync + 'static,
        chat: impl Fn(&Request) -> Reply + Send + Sync + 'static,
    ) -> (FakeServer, Arc<OnceLock<Backend>>) {
        let cell = Arc::new(OnceLock::new());
        let (theirs, first) = (Arc::clone(&cell), AtomicBool::new(true));
        let fake = chatting(move |r| {
            if first.swap(false, Ordering::SeqCst) {
                meanwhile(theirs.get().unwrap());
            }
            chat(r)
        });
        (fake, cell)
    }

    /// The `Authorization` header of each chat `fake` was sent, in the order they were answered.
    fn bearers(fake: &FakeServer) -> Vec<String> {
        let sent = fake.completions();
        sent.iter()
            .map(|r| r.header("authorization").unwrap_or_default().to_string())
            .collect()
    }

    /// Each request `fake` has had, as `<method> <path>`.
    fn asked(fake: &FakeServer) -> Vec<String> {
        fake.requests()
            .iter()
            .map(|r| format!("{} {}", r.method, r.path))
            .collect()
    }

    /// The backend of an entry that names no model.
    fn any(url: &str) -> Backend {
        let text = format!("[llm.any]\nurl = \"{url}\"\n");
        Backend::named(
            "any",
            &Settings::parse(Path::new("/c/config.toml"), &text).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn a_call_posts_the_prompt_as_chat_and_reads_the_reply_and_its_tokens() {
        let fake = FakeServer::start(|_| Reply::ok(OPENAI));
        let b = backend(
            &fake.url,
            "[llm.srv.request]\ntemperature = 1.0\nreasoning_effort = \"low\"\n",
        );
        let a = b.call("PROMPT", TIMEOUT, None, &|_| {});
        assert_eq!(a.reply.as_deref(), Ok("NONE\n"));
        assert_eq!(
            a.used,
            Used::Reported(Usage {
                input_tokens: 624,
                output_tokens: 5,
                cache_read_tokens: Some(512),
                cache_write_tokens: None,
                cost_usd: None
            })
        );
        let [r] = &fake.requests()[..] else {
            panic!("{:?}", fake.requests())
        };
        assert_eq!(
            (r.method.as_str(), r.path.as_str()),
            ("POST", "/v1/chat/completions")
        );
        assert_eq!(
            (r.header("content-type"), r.header("authorization")),
            (Some("application/json"), None)
        );
        assert_eq!(
            r.json(),
            serde_json::json!({
                "model": "m",
                "messages": [{"role": "system", "content": SYSTEM}, {"role": "user", "content": "PROMPT"}],
                "stream": false,
                "reasoning_effort": "low",
                "chat_template_kwargs": {"enable_thinking": false},
                "temperature": 1.0,
            })
        );
    }

    #[test]
    fn a_reply_is_capped_when_the_call_says_and_an_entrys_request_beats_the_cap() {
        let fake = FakeServer::start(|_| Reply::ok(OPENAI));
        // A quarter of 1024 tokens.
        let capped = Some(Budget::new(1024, None));
        let b = backend(&fake.url, "");
        b.call("P", TIMEOUT, capped, &|_| {});
        b.call("P", TIMEOUT, None, &|_| {});
        backend(&fake.url, "[llm.srv.request]\nmax_tokens = 4000\n").call(
            "P",
            TIMEOUT,
            capped,
            &|_| {},
        );
        let caps: Vec<Option<u64>> = fake
            .requests()
            .iter()
            .map(|r| r.json().get("max_tokens").and_then(Value::as_u64))
            .collect();
        assert_eq!(caps, [Some(256), None, Some(4000)]);
        // OpenAI's newer name for the cap stands in place of the call's, which is not sent beside it.
        let newer = FakeServer::start(|_| Reply::ok(OPENAI));
        backend(
            &newer.url,
            "[llm.srv.request]\nmax_completion_tokens = 4000\n",
        )
        .call("P", TIMEOUT, capped, &|_| {});
        let body = newer.requests()[0].json();
        assert_eq!(
            (
                body.get("max_tokens"),
                body["max_completion_tokens"].as_u64()
            ),
            (None, Some(4000))
        );
    }

    #[test]
    fn a_call_before_the_key_is_fetched_sends_nothing() {
        let fake = FakeServer::start(|_| Reply::ok(OPENAI));
        let a =
            backend(&fake.url, "api_key_cmd = \"echo sk-1\"\n").call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            (a.reply, a.used),
            (Err("the key is not fetched".into()), Used::Untracked)
        );
        assert!(fake.requests().is_empty());
    }

    #[test]
    fn thinking_left_to_the_server_sends_neither_switch() {
        let fake = FakeServer::start(|_| Reply::ok(OPENAI));
        backend(&fake.url, "thinking = \"server\"\n").call("P", TIMEOUT, None, &|_| {});
        let body = fake.requests()[0].json();
        assert_eq!(
            (
                body.get("reasoning_effort"),
                body.get("chat_template_kwargs")
            ),
            (None, None)
        );
    }

    #[test]
    fn a_key_goes_in_the_authorization_header_and_never_in_what_is_said() {
        let fake = FakeServer::start(|_| {
            Reply::status(401, r#"{"error":{"message":"Incorrect API key sk-6"}}"#)
        });
        let b = with_key(
            backend(&fake.url, ""),
            &KeySource::Command("echo sk-6".into()),
        );
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            fake.requests()[0].header("authorization"),
            Some("Bearer sk-6")
        );
        assert_eq!(
            a.reply,
            Err("HTTP 401: Incorrect API key <key>; check the entry's key".into())
        );
        assert_eq!(a.used, Used::Unreported);
        assert!(!format!("{b:?}").contains("sk-6"));
    }

    /// A key longer than any cut a message makes: 300 digits, `1000` to `1074` run together.
    fn long_key() -> String {
        (1000..1075).map(|n| n.to_string()).collect()
    }

    /// Whether `text` holds any 20 characters of `key` in a row.
    fn shows_part_of(text: &str, key: &str) -> bool {
        (0..=key.len() - 20).any(|i| text.contains(&key[i..i + 20]))
    }

    #[test]
    fn a_long_key_echoed_back_is_hidden_before_any_of_it_is_cut() {
        let key = long_key();
        for (status, body) in [
            (
                401,
                format!(r#"{{"error":{{"message":"Invalid token {key}"}}}}"#),
            ),
            (200, format!(r#"{{"error":"Invalid token {key}"}}"#)),
            (200, format!("Invalid token {key}")),
        ] {
            let fake = FakeServer::start(move |_| Reply::status(status, body.clone()));
            let b = with_key(
                backend(&fake.url, ""),
                &KeySource::Command(format!("echo {key}")),
            );
            let said = b.call("P", TIMEOUT, None, &|_| {}).reply.unwrap_err();
            assert!(
                said.contains("Invalid token <key>") && !shows_part_of(&said, &key),
                "{said}"
            );
        }
    }

    /// A local server's key is often a plain word, which a reply can hold.
    #[test]
    fn a_key_in_a_good_replys_content_is_left_as_the_server_sent_it() {
        let body = OPENAI.replace(
            r#""content":"NONE""#,
            r#""content":"L3\tKeep it secret.\tsecert misspelt""#,
        );
        let fake = FakeServer::start(move |_| Reply::ok(body.clone()));
        let b = with_key(
            backend(&fake.url, ""),
            &KeySource::Command("echo secret".into()),
        );
        assert_eq!(
            b.call("P", TIMEOUT, None, &|_| {}).reply.as_deref(),
            Ok("L3\tKeep it secret.\tsecert misspelt\n")
        );
    }

    #[test]
    fn a_key_that_is_a_word_of_a_good_replys_json_still_reads() {
        for key in ["content", "stop"] {
            let fake = FakeServer::start(|_| Reply::ok(OPENAI));
            let b = with_key(
                backend(&fake.url, ""),
                &KeySource::Command(format!("echo {key}")),
            );
            assert_eq!(
                b.call("P", TIMEOUT, None, &|_| {}).reply.as_deref(),
                Ok("NONE\n"),
                "{key}"
            );
        }
    }

    #[test]
    fn a_key_that_is_a_word_of_a_cut_off_replys_json_leaves_it_an_error() {
        let cut = OPENAI.replace(r#""finish_reason":"stop""#, r#""finish_reason":"length""#);
        let fake = FakeServer::start(move |_| Reply::ok(cut.clone()));
        let b = with_key(
            backend(&fake.url, ""),
            &KeySource::Command("echo length".into()),
        );
        assert!(b.call("P", TIMEOUT, None, &|_| {}).reply.is_err());
    }

    #[test]
    fn a_403_has_the_next_fetch_run_the_command_for_a_key_that_has_not_worked() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let cmd = format!("echo x >> '{}'; echo sk-7", count.display());
        let source = KeySource::Command(cmd.clone());
        let fake = FakeServer::start(|_| Reply::status(403, ""));
        // The key of the command the entry names, which no server has taken: the call's 403 marks
        // it, so the next fetch runs the command.
        let entry = backend(&fake.url, &format!("api_key_cmd = \"{cmd}\"\n"));
        with_key(entry, &source).call("P", TIMEOUT, None, &|_| {});
        key::fetch(&source, &|_| {}).unwrap();
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2);
    }

    #[test]
    fn a_401_fetches_a_key_that_worked_again_and_sends_the_call_once_more() {
        let dir = tempfile::tempdir().unwrap();
        let fake = chatting(|r| match r.header("authorization") {
            Some("Bearer sk-new") => Reply::ok(OPENAI),
            _ => Reply::status(401, ""),
        });
        std::fs::write(dir.path().join("key"), "sk-old\n").unwrap();
        let (b, key, runs) = keyed(&fake, dir.path());
        std::fs::write(&key, "sk-new\n").unwrap();
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!(a.reply.as_deref(), Ok("NONE\n"));
        // The refused request is no call of its own: the usage is the second one's.
        assert_eq!(
            a.used,
            Used::Reported(Usage {
                input_tokens: 624,
                output_tokens: 5,
                cache_read_tokens: Some(512),
                cache_write_tokens: None,
                cost_usd: None
            })
        );
        assert_eq!(bearers(&fake), ["Bearer sk-old", "Bearer sk-new"]);
        assert_eq!(runs(), 2);
        assert_eq!(
            b.call("P", TIMEOUT, None, &|_| {}).reply.as_deref(),
            Ok("NONE\n")
        );
        assert_eq!(
            bearers(&fake),
            ["Bearer sk-old", "Bearer sk-new", "Bearer sk-new"]
        );
        assert_eq!(runs(), 2);
    }

    #[test]
    fn a_call_names_the_key_command_a_401_runs_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let fake = chatting(|r| match r.header("authorization") {
            Some("Bearer sk-new") => Reply::ok(OPENAI),
            _ => Reply::status(401, ""),
        });
        std::fs::write(dir.path().join("key"), "sk-old\n").unwrap();
        let (b, key, _) = keyed(&fake, dir.path());
        std::fs::write(&key, "sk-new\n").unwrap();
        let first = heard(|waits| {
            let a = b.call("P", TIMEOUT, None, waits);
            assert_eq!(a.reply.as_deref(), Ok("NONE\n"));
        });
        assert_eq!(first, [Some(key_command()), None]);
        let next = heard(|waits| {
            b.call("P", TIMEOUT, None, waits);
        });
        assert_eq!(next, []);
    }

    #[test]
    fn a_key_only_models_took_is_not_fetched_again_when_its_first_chat_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        // As a server that lists its models without checking the key.
        let fake = chatting(|_| Reply::status(401, ""));
        let (b, runs) = readied_by(&fake, dir.path(), "echo sk-wrong");
        let refusal = Err("HTTP 401; check the entry's key".to_string());
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!((a.reply, a.used), (refusal.clone(), Used::Unreported));
        assert_eq!(runs(), 1);
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!((a.reply, a.used), (refusal, Used::Untracked));
        assert_eq!(fake.completions().len(), 1);
        assert_eq!(runs(), 1);
    }

    #[test]
    fn a_401_for_the_same_key_again_refuses_it_for_the_rest_of_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        let fake = chatting(|_| Reply::status(401, r#"{"error":{"message":"bad key sk-same"}}"#));
        std::fs::write(dir.path().join("key"), "sk-same\n").unwrap();
        let (b, _, runs) = keyed(&fake, dir.path());
        let refusal = Err("HTTP 401: bad key <key>; check the entry's key".to_string());
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!((a.reply, a.used), (refusal.clone(), Used::Unreported));
        assert_eq!(runs(), 2);
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            (a.reply, a.used, a.too_big),
            (refusal, Used::Untracked, None)
        );
        assert_eq!(fake.completions().len(), 1);
        assert_eq!(runs(), 2);
        // A refused key is not left kept, so the next scan runs the command.
        key::fetch(&server(&b).entry.key, &|_| {}).unwrap();
        assert_eq!(runs(), 3);
    }

    #[test]
    fn a_refused_server_still_hides_its_key_and_shows_none() {
        let dir = tempfile::tempdir().unwrap();
        let fake = chatting(|_| Reply::status(401, ""));
        std::fs::write(dir.path().join("key"), "sk-gone\n").unwrap();
        let (b, _, _) = keyed(&fake, dir.path());
        b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!(b.call("P", TIMEOUT, None, &|_| {}).used, Used::Untracked);
        assert_eq!(
            server(&b).hidden("loading sk-gone into Ollama".to_string()),
            "loading <key> into Ollama"
        );
        assert!(!format!("{b:?}").contains("sk-gone"));
    }

    #[test]
    fn a_key_fetched_again_and_refused_on_its_first_request_is_not_fetched_a_third_time() {
        let dir = tempfile::tempdir().unwrap();
        // As a server that echoes the key it refuses.
        let fake = chatting(|r| {
            let sent = r.header("authorization").unwrap_or_default();
            Reply::status(401, format!("no such key: {sent}"))
        });
        // `sk-<runs so far>`: a new key each run.
        let count = dir.path().join("count");
        let print = format!("echo sk-$(grep -c x '{}')", count.display());
        let (b, runs) = keyed_by(&fake, dir.path(), &print);
        // The second request's refusal, hidden with the key that request sent.
        let refusal = Err("HTTP 401: no such key: Bearer <key>; check the entry's key".to_string());
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!((a.reply, a.used), (refusal.clone(), Used::Unreported));
        assert_eq!(bearers(&fake), ["Bearer sk-1", "Bearer sk-2"]);
        assert_eq!(runs(), 2);
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!((a.reply, a.used), (refusal, Used::Untracked));
        assert_eq!(fake.completions().len(), 2);
        assert_eq!(runs(), 2);
    }

    #[test]
    fn a_second_401_takes_a_key_fetched_meanwhile_and_sends_no_third_request() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("key");
        let source = Arc::new(OnceLock::<KeySource>::new());
        // As another worker's call, answered while this one's second request is on its way: the
        // key that request sent works for it, then runs out.
        let (theirs, rotated) = (Arc::clone(&source), file.clone());
        let fake = chatting(move |r| {
            if r.header("authorization") == Some("Bearer sk-2") {
                let source = theirs.get().unwrap();
                let sent = key::fetch(source, &|_| {}).unwrap().unwrap();
                key::worked(source, &sent);
                std::fs::write(&rotated, "sk-3\n").unwrap();
            }
            Reply::status(401, "")
        });
        std::fs::write(&file, "sk-1\n").unwrap();
        let (b, _, runs) = keyed(&fake, dir.path());
        source.set(server(&b).entry.key.clone()).unwrap();
        std::fs::write(&file, "sk-2\n").unwrap();
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            (a.reply, a.used),
            (
                Err("HTTP 401; check the entry's key".to_string()),
                Used::Unreported
            )
        );
        assert_eq!(bearers(&fake), ["Bearer sk-1", "Bearer sk-2"]);
        assert_eq!(runs(), 3);
        b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            bearers(&fake),
            ["Bearer sk-1", "Bearer sk-2", "Bearer sk-3"]
        );
        assert_eq!(runs(), 3);
    }

    #[test]
    fn a_401_for_a_variables_key_refuses_it_at_once() {
        let fake = chatting(|_| Reply::status(401, ""));
        let (b, _) = backend(&fake.url, "api_key_env = \"HOME\"\n")
            .ready(&|_| {})
            .unwrap();
        let refusal = Err("HTTP 401; check the entry's key".to_string());
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!((a.reply, a.used), (refusal.clone(), Used::Unreported));
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!((a.reply, a.used), (refusal, Used::Untracked));
        assert_eq!(fake.completions().len(), 1);
    }

    #[test]
    fn a_key_fetched_for_a_server_refused_meanwhile_is_not_sent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("key");
        let rotated = file.clone();
        let (fake, cell) = chatting_while(
            move |b| {
                // Another worker's call is refused with the same key, which the command prints
                // again, so the server is refused; then another scan's fetch keeps a new key.
                b.call("P", TIMEOUT, None, &|_| {});
                std::fs::write(&rotated, "sk-3\n").unwrap();
                key::fetch(&server(b).entry.key, &|_| {}).unwrap();
            },
            |_| Reply::status(401, ""),
        );
        std::fs::write(&file, "sk-1\n").unwrap();
        let (b, _, runs) = keyed(&fake, dir.path());
        let b = cell.get_or_init(|| b);
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            (a.reply, a.used),
            (
                Err("HTTP 401; check the entry's key".to_string()),
                Used::Unreported
            )
        );
        assert_eq!(bearers(&fake), ["Bearer sk-1", "Bearer sk-1"]);
        assert_eq!(runs(), 3);
    }

    #[test]
    fn a_late_401_for_an_old_key_leaves_a_newer_key_to_be_sent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("key");
        let rotated = file.clone();
        let (fake, cell) = chatting_while(
            move |b| {
                // Another worker's call fetches a new key, which its 403 leaves not known to work;
                // then a 401 for that key at another backend of the same command, as a recheck's
                // can be, leaves nothing kept.
                std::fs::write(&rotated, "sk-2\n").unwrap();
                b.call("P", TIMEOUT, None, &|_| {});
                let KeyState::Sending(new) = server(b).key.get() else {
                    unreachable!()
                };
                assert_eq!(key::refetch(&server(b).entry.key, &new, &|_| {}), Ok(None));
            },
            |r| match r.header("authorization") {
                Some("Bearer sk-2") => Reply::status(403, ""),
                _ => Reply::status(401, ""),
            },
        );
        std::fs::write(&file, "sk-1\n").unwrap();
        let (b, _, runs) = keyed(&fake, dir.path());
        let b = cell.get_or_init(|| b);
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            (a.reply, a.used),
            (
                Err("HTTP 401; check the entry's key".to_string()),
                Used::Unreported
            )
        );
        assert_eq!(
            bearers(&fake),
            ["Bearer sk-1", "Bearer sk-2", "Bearer sk-1"]
        );
        // The old key's refusal says nothing of the new one, which the next call sends.
        b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            bearers(&fake),
            ["Bearer sk-1", "Bearer sk-2", "Bearer sk-1", "Bearer sk-2"]
        );
        assert_eq!(runs(), 2);
    }

    #[test]
    fn a_late_401_for_an_old_key_is_sent_again_with_a_newer_key_a_403_marked() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("key");
        let rotated = file.clone();
        let flagged = AtomicBool::new(false);
        let (fake, cell) = chatting_while(
            move |b| {
                // Another worker's call fetches a new key, and its chunk is refused with it.
                std::fs::write(&rotated, "sk-2\n").unwrap();
                b.call("P", TIMEOUT, None, &|_| {});
            },
            move |r| match r.header("authorization") {
                Some("Bearer sk-2") if !flagged.swap(true, Ordering::SeqCst) => {
                    Reply::status(403, "")
                }
                Some("Bearer sk-2") => Reply::ok(OPENAI),
                _ => Reply::status(401, ""),
            },
        );
        std::fs::write(&file, "sk-1\n").unwrap();
        let (b, _, runs) = keyed(&fake, dir.path());
        let b = cell.get_or_init(|| b);
        assert_eq!(
            b.call("P", TIMEOUT, None, &|_| {}).reply.as_deref(),
            Ok("NONE\n")
        );
        assert_eq!(
            bearers(&fake),
            ["Bearer sk-1", "Bearer sk-2", "Bearer sk-1", "Bearer sk-2"]
        );
        assert_eq!(runs(), 2);
    }

    #[test]
    fn a_403_then_a_401_still_fetches_a_key_that_worked_again() {
        let dir = tempfile::tempdir().unwrap();
        let flagged = AtomicBool::new(false);
        let fake = chatting(move |r| match r.header("authorization") {
            _ if !flagged.swap(true, Ordering::SeqCst) => Reply::status(403, ""),
            Some("Bearer sk-new") => Reply::ok(OPENAI),
            _ => Reply::status(401, ""),
        });
        std::fs::write(dir.path().join("key"), "sk-old\n").unwrap();
        let (b, key, runs) = keyed(&fake, dir.path());
        assert_eq!(
            b.call("P", TIMEOUT, None, &|_| {}).reply,
            Err("HTTP 403; check the entry's key".to_string())
        );
        std::fs::write(&key, "sk-new\n").unwrap();
        assert_eq!(
            b.call("P", TIMEOUT, None, &|_| {}).reply.as_deref(),
            Ok("NONE\n")
        );
        assert_eq!(
            bearers(&fake),
            ["Bearer sk-old", "Bearer sk-old", "Bearer sk-new"]
        );
        assert_eq!(runs(), 2);
    }

    #[test]
    fn a_403_before_any_answer_leaves_a_later_401_to_fetch_the_key_again() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("key");
        let sent = AtomicUsize::new(0);
        let fake = chatting(move |r| {
            match (
                sent.fetch_add(1, Ordering::SeqCst),
                r.header("authorization"),
            ) {
                (0, _) => Reply::status(403, ""),
                (1, _) | (_, Some("Bearer sk-new")) => Reply::ok(OPENAI),
                _ => Reply::status(401, ""),
            }
        });
        std::fs::write(&key, "sk-old\n").unwrap();
        let (b, runs) = readied_by(&fake, dir.path(), &format!("cat '{}'", key.display()));
        assert_eq!(
            b.call("P", TIMEOUT, None, &|_| {}).reply,
            Err("HTTP 403; check the entry's key".to_string())
        );
        assert_eq!(
            b.call("P", TIMEOUT, None, &|_| {}).reply.as_deref(),
            Ok("NONE\n")
        );
        std::fs::write(&key, "sk-new\n").unwrap();
        assert_eq!(
            b.call("P", TIMEOUT, None, &|_| {}).reply.as_deref(),
            Ok("NONE\n")
        );
        assert_eq!(
            bearers(&fake),
            [
                "Bearer sk-old",
                "Bearer sk-old",
                "Bearer sk-old",
                "Bearer sk-new"
            ]
        );
        assert_eq!(runs(), 2);
    }

    #[test]
    fn after_a_403_the_next_ready_runs_the_key_command_again() {
        let dir = tempfile::tempdir().unwrap();
        let fake = chatting(|_| Reply::status(403, ""));
        std::fs::write(dir.path().join("key"), "sk-old\n").unwrap();
        let (b, key, runs) = keyed(&fake, dir.path());
        b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!(runs(), 1);
        std::fs::write(&key, "sk-new\n").unwrap();
        // As the next scan or recheck: a new backend for the same entry.
        moved(&b, &fake.url).ready(&|_| {}).unwrap();
        assert_eq!(runs(), 2);
        let listed: Vec<_> = fake
            .requests()
            .into_iter()
            .filter(|r| r.path == "/v1/models")
            .map(|r| r.header("authorization").unwrap_or_default().to_string())
            .collect();
        assert_eq!(listed, ["Bearer sk-old", "Bearer sk-new"]);
    }

    #[test]
    fn a_403_fails_its_call_and_the_scan_goes_on() {
        let dir = tempfile::tempdir().unwrap();
        let fake = chatting(|_| Reply::status(403, r#"{"error":{"message":"flagged"}}"#));
        std::fs::write(dir.path().join("key"), "sk-ok\n").unwrap();
        let (b, _, runs) = keyed(&fake, dir.path());
        for sent in 1..=2 {
            let a = b.call("P", TIMEOUT, None, &|_| {});
            assert_eq!(
                (a.reply, a.used),
                (
                    Err("HTTP 403: flagged; check the entry's key".to_string()),
                    Used::Unreported
                )
            );
            assert_eq!(fake.completions().len(), sent);
        }
        assert_eq!(runs(), 1);
    }

    #[test]
    fn an_entry_without_a_key_is_not_told_to_check_one() {
        for status in [401, 403] {
            let fake = chatting(move |_| Reply::status(status, r#"{"error":{"message":"no"}}"#));
            let (b, _) = backend(&fake.url, "").ready(&|_| {}).unwrap();
            for sent in 1..=2 {
                let a = b.call("P", TIMEOUT, None, &|_| {});
                assert_eq!(
                    (a.reply, a.used),
                    (Err(format!("HTTP {status}: no")), Used::Unreported)
                );
                assert_eq!(fake.completions().len(), sent, "{status}");
            }
            let refusing = FakeServer::start(move |_| Reply::status(status, ""));
            assert_eq!(
                backend(&refusing.url, "").ready(&|_| {}).unwrap_err(),
                format!("srv@m: HTTP {status} from {}/models", refusing.url)
            );
        }
    }

    #[test]
    fn a_key_command_that_fails_when_run_again_adds_why_to_the_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let fake = chatting(|_| Reply::status(401, r#"{"error":{"message":"bad key"}}"#));
        let count = dir.path().join("count");
        let print = format!(
            "if [ $(wc -l < '{}') -gt 1 ]; then echo locked >&2; exit 2; fi; echo sk-1",
            count.display()
        );
        let (b, runs) = keyed_by(&fake, dir.path(), &print);
        let refusal = Err(
            "HTTP 401: bad key; check the entry's key; api_key_cmd failed: sh exited with exit \
             status: 2: locked"
                .to_string(),
        );
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!((a.reply, a.used), (refusal.clone(), Used::Unreported));
        assert_eq!(runs(), 2);
        let a = b.call("P", TIMEOUT, None, &|_| {});
        assert_eq!((a.reply, a.used), (refusal, Used::Untracked));
        assert_eq!(fake.completions().len(), 1);
        assert_eq!(runs(), 2);
    }

    #[test]
    fn workers_refused_together_run_the_key_command_once() {
        let dir = tempfile::tempdir().unwrap();
        // Neither chat is answered before both have come, so neither call finds the server
        // refused before it sends. A chat with no partner is never answered, so the calls give up
        // long before `TIMEOUT`: one chat or three fail the test within `patience`.
        let patience = Duration::from_secs(30);
        let both = Barrier::new(2);
        let fake = chatting(move |_| {
            both.wait();
            Reply::status(401, "")
        });
        let (b, runs) = keyed_by(&fake, dir.path(), "sleep 0.3; echo sk-same");
        let mut replies = Vec::new();
        let waits = heard(|waits| {
            replies = std::thread::scope(|s| {
                let both = [
                    s.spawn(|| b.call("P", patience, None, waits)),
                    s.spawn(|| b.call("P", patience, None, waits)),
                ];
                both.map(|t| t.join().unwrap().reply).into()
            });
        });
        assert_eq!(
            replies,
            vec![Err("HTTP 401; check the entry's key".to_string()); 2]
        );
        // Both were sent, so both were refused: neither found the server refused already.
        assert_eq!(fake.completions().len(), 2);
        assert_eq!(runs(), 2);
        assert_eq!(waits, [Some(key_command()), None]);
    }

    #[test]
    fn a_clone_of_a_refused_server_is_refused_too_and_changes_apart() {
        let fake = chatting(|_| Reply::status(401, ""));
        let (b, _) = backend(&fake.url, "api_key_env = \"HOME\"\n")
            .ready(&|_| {})
            .unwrap();
        let before = b.clone();
        assert_eq!(before, b);
        b.call("P", TIMEOUT, None, &|_| {});
        assert_ne!(before, b);
        let after = b.clone();
        assert_eq!(after, b);
        assert_eq!(
            after.call("P", TIMEOUT, None, &|_| {}).used,
            Used::Untracked
        );
        assert_eq!(fake.completions().len(), 1);
        // The clone made before the refusal still sends its key, and is refused for itself.
        assert_eq!(
            before.call("P", TIMEOUT, None, &|_| {}).used,
            Used::Unreported
        );
        assert_eq!(fake.completions().len(), 2);
        assert_eq!(before, b);
    }

    #[test]
    fn an_error_status_says_the_servers_message_and_counts_as_unreported() {
        for (status, body, said) in [
            (500, r#"{"error":{"message":"boom"}}"#, "HTTP 500: boom"),
            (
                404,
                r#"{"error":"model 'x' not found, try pulling it first"}"#,
                "HTTP 404: model 'x' not found, try pulling it first",
            ),
            (
                404,
                r#"{"object":"error","message":"The model x does not exist.","code":404}"#,
                "HTTP 404: The model x does not exist.",
            ),
            (404, r#"{"detail":"Not Found"}"#, "HTTP 404: Not Found"),
            (502, "Bad Gateway\n<html>", "HTTP 502: Bad Gateway"),
            (503, "", "HTTP 503"),
        ] {
            let fake = FakeServer::start(move |_| Reply::status(status, body));
            let a = backend(&fake.url, "").call("P", TIMEOUT, None, &|_| {});
            assert_eq!(
                (a.reply, a.used),
                (Err(said.to_string()), Used::Unreported),
                "{body}"
            );
        }
        let long = format!("{{\"error\":{{\"message\":\"{}\"}}}}", "x".repeat(300));
        assert_eq!(error_text(500, &long).len(), "HTTP 500: ".len() + 200);
    }

    #[test]
    fn a_reply_cut_off_without_usage_or_not_json_says_so() {
        let bare = r#"{"choices":[{"message":{"content":"NONE"},"finish_reason":"stop"}]}"#;
        let cut = OPENAI.replace(r#""finish_reason":"stop""#, r#""finish_reason":"length""#);
        let used = Usage {
            input_tokens: 624,
            output_tokens: 5,
            cache_read_tokens: Some(512),
            cache_write_tokens: None,
            cost_usd: None,
        };
        for (body, reply, used) in [
            (bare.to_string(), Ok("NONE\n".to_string()), Used::Unreported),
            (
                cut,
                Err("the reply was cut off at the server's length limit".to_string()),
                Used::Reported(used),
            ),
            (
                "<!DOCTYPE html>".to_string(),
                Err("reply is not an OpenAI chat completion: <!DOCTYPE html>".to_string()),
                Used::Unreported,
            ),
        ] {
            let fake = FakeServer::start(move |_| Reply::ok(body.clone()));
            let a = backend(&fake.url, "").call("P", TIMEOUT, None, &|_| {});
            assert_eq!((a.reply, a.used), (reply, used));
        }
    }

    #[test]
    fn a_reply_cut_off_at_a_known_cap_names_it_and_what_sets_it() {
        let cut = OPENAI.replace(r#""finish_reason":"stop""#, r#""finish_reason":"length""#);
        // 16384 tokens shared by 2 jobs: 8192 a call, its reply capped at 2048.
        let listed =
            serde_json::json!({"data": [{"id": "m", "meta": {"n_ctx": 16384}}]}).to_string();
        let sized = FakeServer::start(move |r| match r.path.as_str() {
            "/v1/models" => Reply::ok(listed.clone()),
            _ => Reply::ok(cut.clone()),
        });
        let cut_off = |url: &str, extra: &str| {
            let (b, _) = backend(url, extra).ready(&|_| {}).unwrap();
            let a = b.call("P", TIMEOUT, b.budget(2), &|_| {});
            assert_eq!(a.too_big, Some(TooBig::Reply), "{extra:?}");
            a.reply.unwrap_err()
        };
        let own = |request: &str| format!("[llm.srv.request]\n{request}\n");
        for (extra, said) in [
            (
                String::new(),
                "the reply was cut off at subcheck's cap of 2048 tokens; the entry's \
                 request.max_tokens sets another",
            ),
            (
                own("max_tokens = 4000"),
                "the reply was cut off at the entry's request.max_tokens of 4000 tokens",
            ),
            (
                own("max_completion_tokens = 4000"),
                "the reply was cut off at the entry's request.max_completion_tokens of 4000 tokens",
            ),
            (
                own("max_tokens = 8192"),
                "the reply was cut off at the entry's request.max_tokens of 8192 tokens",
            ),
            // The entry's cap is what is sent, even over a call's share, but the reply's room is
            // the share, whose end may come first.
            (
                own("max_tokens = 20000"),
                "the reply was cut off at the entry's request.max_tokens of 20000 tokens or the \
                 server's length limit",
            ),
        ] {
            assert_eq!(cut_off(&sized.url, &extra), said, "{extra:?}");
        }
        // With no size known, no cap of subcheck's was sent, and the context's end may have come
        // before the entry's own.
        let silent = FakeServer::start(|r| match r.path.as_str() {
            "/v1/models" => models_reply(&["m"]),
            _ => Reply::ok(
                OPENAI.replace(r#""finish_reason":"stop""#, r#""finish_reason":"length""#),
            ),
        });
        assert_eq!(
            cut_off(&silent.url, ""),
            "the reply was cut off at the server's length limit"
        );
        assert_eq!(
            cut_off(&silent.url, &own("max_tokens = 4000")),
            "the reply was cut off at the entry's request.max_tokens of 4000 tokens or the \
             server's length limit"
        );
        // Ollama's own API, with no entry to set another.
        let (_, ollama) = ollama_answering(Reply::ok(OLLAMA_CHAT_CUT), Some(4096));
        let a = ollama.call("P", TIMEOUT, ollama.budget(2), &|_| {});
        assert_eq!(
            (a.too_big, a.reply),
            (
                Some(TooBig::Reply),
                Err("the reply was cut off at subcheck's cap of 1024 tokens".into())
            )
        );
    }

    #[test]
    fn a_refusal_for_the_context_is_known_by_type_code_or_words() {
        for body in [
            r#"{"error":{"type":"exceed_context_size_error","message":"x"}}"#,
            r#"{"error":{"code":"context_length_exceeded","message":"x"}}"#,
            r#"{"object":"error","message":"This model's maximum context length is 16384 tokens."}"#,
            r#"{"error":"input length exceeds the context length"}"#,
        ] {
            assert!(refused_for_context(400, body), "{body}");
        }
        assert!(refused_for_context(
            500,
            r#"{"error":{"code":500,"message":"Context size has been exceeded.","type":"server_error"}}"#
        ));
        assert!(!refused_for_context(
            400,
            r#"{"error":{"message":"temperature must be at most 2"}}"#
        ));
        assert!(!refused_for_context(500, r#"{"error":{"message":"boom"}}"#));
        assert!(!refused_for_context(
            200,
            r#"{"error":{"type":"exceed_context_size_error"}}"#
        ));
    }

    /// The answer of a server that lists `m` with a context of `n_ctx` tokens (none for `None`) and
    /// answers a chat with the OPENAI fixture's reply, its `prompt_tokens` made `prompt_tokens`,
    /// to a call with the budget of one of `jobs` calls at once (none for `None`).
    fn answered_with(prompt_tokens: u64, n_ctx: Option<u64>, jobs: Option<usize>) -> Answer {
        let listed = match n_ctx {
            Some(n) => serde_json::json!({"data": [{"id": "m", "meta": {"n_ctx": n}}]}),
            None => serde_json::json!({"data": [{"id": "m"}]}),
        }
        .to_string();
        let reply = OPENAI.replace(
            r#""prompt_tokens":624"#,
            &format!(r#""prompt_tokens":{prompt_tokens}"#),
        );
        assert_ne!(reply, OPENAI);
        let fake = FakeServer::start(move |r| match r.path.as_str() {
            "/v1/models" => Reply::ok(listed.clone()),
            "/v1/chat/completions" => Reply::ok(reply.clone()),
            _ => Reply::status(404, ""),
        });
        let (ready, _) = backend(&fake.url, "").ready(&|_| {}).unwrap();
        assert_eq!(
            ready.context(),
            Some(n_ctx.map_or(Context::Unknown, Context::Tokens))
        );
        ready.call(
            "P",
            TIMEOUT,
            jobs.and_then(|jobs| ready.budget(jobs)),
            &|_| {},
        )
    }

    #[test]
    fn a_prompt_that_filled_the_context_is_too_big_for_the_prompt() {
        let a = answered_with(950, Some(1000), None);
        assert_eq!(
            (a.too_big, a.reply),
            (
                Some(TooBig::Prompt),
                Err(
                    "the prompt filled 950 of the server's 1000 tokens, so the server may have cut it"
                        .into()
                )
            )
        );
        assert_eq!(
            a.used,
            Used::Reported(Usage {
                input_tokens: 950,
                output_tokens: 5,
                cache_read_tokens: Some(512),
                cache_write_tokens: None,
                cost_usd: None
            })
        );
        for (prompt_tokens, n_ctx) in [(949, Some(1000)), (950, None)] {
            let a = answered_with(prompt_tokens, n_ctx, None);
            assert_eq!(
                (a.too_big, a.reply.as_deref()),
                (None, Ok("NONE\n")),
                "{prompt_tokens} of {n_ctx:?}"
            );
        }
    }

    #[test]
    fn a_prompt_that_filled_a_calls_share_is_too_big_for_the_prompt() {
        // One of 2 calls in flight sharing a 16384-token context: 8195 tokens, well within the
        // whole context, are over 95% of the call's 8192 and over the 6144 its reply's quarter
        // leaves the prompt.
        let a = answered_with(8195, Some(16384), Some(2));
        assert_eq!(
            (a.too_big, a.reply),
            (
                Some(TooBig::Prompt),
                Err(
                    "the prompt filled 8195 of a call's 8192 tokens, so the server may have cut it"
                        .into()
                )
            )
        );
        // With no budget it is measured against the whole context; under 95% of the share it fits.
        for (prompt_tokens, jobs) in [(8195, None), (7782, Some(2))] {
            let a = answered_with(prompt_tokens, Some(16384), jobs);
            assert_eq!(
                (a.too_big, a.reply.as_deref()),
                (None, Ok("NONE\n")),
                "{prompt_tokens} with {jobs:?} jobs"
            );
        }
    }

    #[test]
    fn a_prompt_within_the_room_its_budget_left_it_is_not_taken_as_cut() {
        // An entry's own cap of 200, under a twentieth of a call's 8192 tokens, leaves the prompt
        // 7992 of them, over 95% of the share.
        let budget = Budget::new(8192, Some(200));
        assert_eq!(budget.prompt(), 7992);
        for (prompt_tokens, too_big, reply) in [
            (7900, None, Ok("NONE\n".to_string())),
            (
                8100,
                Some(TooBig::Prompt),
                Err(
                    "the prompt filled 8100 of a call's 8192 tokens, so the server may have cut it"
                        .to_string(),
                ),
            ),
        ] {
            let body = OPENAI.replace(
                r#""prompt_tokens":624"#,
                &format!(r#""prompt_tokens":{prompt_tokens}"#),
            );
            let fake = FakeServer::start(move |_| Reply::ok(body.clone()));
            let a = backend(&fake.url, "").call("P", TIMEOUT, Some(budget), &|_| {});
            assert_eq!((a.too_big, a.reply), (too_big, reply), "{prompt_tokens}");
        }
    }

    #[test]
    fn no_server_or_a_slow_one_fails_the_call() {
        let closed = closed_port_url();
        let a = backend(&closed, "").call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            (a.reply, a.used),
            (
                Err(format!(
                    "nothing answering at {closed}: is the server running?"
                )),
                Used::Untracked
            )
        );
        let slow = FakeServer::start(|_| Reply::ok(OPENAI).after(Duration::from_secs(5)));
        let started = Instant::now();
        let a = backend(&slow.url, "").call("P", Duration::from_secs(1), None, &|_| {});
        assert_eq!(
            (a.reply, a.used),
            (
                Err(format!("{}/chat/completions timed out after 1s", slow.url)),
                Used::Unreported
            )
        );
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn a_server_on_this_machine_is_called_directly_and_others_through_any_proxy() {
        for (url, here) in [
            ("http://127.0.0.1:8000/v1", true),
            ("http://127.1.2.3/v1", true),
            ("http://LocalHost:11434/v1", true),
            ("http://[::1]:8000/v1", true),
            ("http://0.0.0.0:8000/v1", true),
            ("http://[::]:8000/v1", true),
            ("http://192.168.1.5:8000/v1", false),
            ("https://openrouter.ai/api/v1", false),
            ("http://localhost.example.com/v1", false),
        ] {
            assert_eq!(on_this_machine(url), here, "{url}");
        }
    }

    #[test]
    fn ready_asks_the_server_for_its_model_and_labels_the_backend_with_it() {
        let fake = FakeServer::start(|_| models_reply(&["gemma4"]));
        let (b, warnings) = any(&fake.url).ready(&|_| {}).unwrap();
        assert_eq!((b.label.as_str(), warnings.len()), ("any@gemma4", 0));
        assert!(matches!(&b.via, Via::Server(s) if s.model.as_deref() == Some("gemma4")));
        assert_eq!(asked(&fake), ["GET /v1/models", "GET /api/ps"]);
        let named = FakeServer::start(|_| models_reply(&["x", "m"]));
        assert_eq!(
            backend(&named.url, "").ready(&|_| {}).map(|(b, _)| b.label),
            Ok("srv@m".into())
        );
        let shell = Backend::shell("x");
        assert_eq!(shell.clone().ready(&|_| {}), Ok((shell, Vec::new())));
    }

    #[test]
    fn ready_refuses_a_model_the_server_does_not_serve_naming_both() {
        let twelve: Vec<String> = (1..=12).map(|i| format!("m{i}")).collect();
        let ids: Vec<&str> = twelve.iter().map(String::as_str).collect();
        let listed = r#""m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10" and 2 more"#;
        let reply = models_reply(&ids);
        let fake = FakeServer::start(move |_| reply.clone());
        assert_eq!(
            backend(&fake.url, "").ready(&|_| {}).unwrap_err(),
            format!(
                "srv@m: {} does not serve \"m\"; it serves {listed}",
                fake.url
            )
        );
        assert_eq!(
            any(&fake.url).ready(&|_| {}).unwrap_err(),
            format!(
                "any: {} serves {listed}: name one as the entry's model",
                fake.url
            )
        );
        let none = FakeServer::start(|_| models_reply(&[]));
        assert_eq!(
            any(&none.url).ready(&|_| {}).unwrap_err(),
            format!("any: {} serves no model", none.url)
        );
        let open = Settings::parse(
            Path::new("/c/config.toml"),
            &format!("[llm.srv]\nurl = \"{}\"\n", fake.url),
        )
        .unwrap();
        assert!(
            Backend::from_label("srv@m3", &open)
                .unwrap()
                .ready(&|_| {})
                .is_ok()
        );
        assert_eq!(
            Backend::from_label("srv@a", &open)
                .unwrap()
                .ready(&|_| {})
                .unwrap_err(),
            format!(
                "srv@a: {} does not serve \"a\"; it serves {listed}",
                fake.url
            )
        );
    }

    #[test]
    fn ready_says_why_it_cannot_reach_or_read_the_server() {
        let closed = closed_port_url();
        assert_eq!(
            backend(&closed, "").ready(&|_| {}).unwrap_err(),
            format!("srv@m: nothing answering at {closed}: is the server running?")
        );
        let failing = FakeServer::start(|_| Reply::status(500, r#"{"error":{"message":"boom"}}"#));
        assert_eq!(
            backend(&failing.url, "").ready(&|_| {}).unwrap_err(),
            format!("srv@m: HTTP 500 from {}/models: boom", failing.url)
        );
        let odd = FakeServer::start(|_| Reply::ok("nope"));
        assert_eq!(
            backend(&odd.url, "").ready(&|_| {}).unwrap_err(),
            format!("srv@m: {}/models gave no list of models: nope", odd.url)
        );
        let unset = backend(
            &failing.url,
            "api_key_env = \"SUBCHECK_TEST_KEY_NEVER_SET\"\n",
        );
        assert_eq!(
            unset.ready(&|_| {}).unwrap_err(),
            "srv@m: api_key_env names SUBCHECK_TEST_KEY_NEVER_SET, which is unset or empty"
        );
        assert_eq!(
            failing.requests().len(),
            1,
            "the key comes before the server is asked"
        );
    }

    #[test]
    fn ready_sends_the_key_and_a_401_drops_it_and_hides_it() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let cmd = format!("echo x >> '{}'; echo sk-8", count.display());
        let fake = FakeServer::start(|_| {
            Reply::status(401, r#"{"error":{"message":"no such key sk-8"}}"#)
        });
        let b = backend(&fake.url, &format!("api_key_cmd = \"{cmd}\"\n"));
        assert_eq!(
            b.clone().ready(&|_| {}).unwrap_err(),
            format!(
                "srv@m: HTTP 401 from {}/models: no such key <key>; check the entry's key",
                fake.url
            )
        );
        assert_eq!(
            fake.requests()[0].header("authorization"),
            Some("Bearer sk-8")
        );
        b.ready(&|_| {}).unwrap_err();
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2);
    }

    #[test]
    fn ready_fetches_a_kept_key_that_worked_again_when_models_refuses_it() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("key");
        // An earlier scan's chat is what showed that the key works.
        let taking = chatting(|_| Reply::ok(OPENAI));
        std::fs::write(&key, "sk-old\n").unwrap();
        let (b, runs) = readied_by(&taking, dir.path(), &format!("cat '{}'", key.display()));
        assert_eq!(
            b.call("P", TIMEOUT, None, &|_| {}).reply.as_deref(),
            Ok("NONE\n")
        );
        std::fs::write(&key, "sk-new\n").unwrap();
        let strict = FakeServer::start(|r| match r.header("authorization") {
            Some("Bearer sk-new") => models_reply(&["m"]),
            _ => Reply::status(401, ""),
        });
        moved(&b, &strict.url).ready(&|_| {}).unwrap();
        let listed: Vec<_> = strict
            .requests()
            .into_iter()
            .filter(|r| r.path == "/v1/models")
            .map(|r| r.header("authorization").unwrap_or_default().to_string())
            .collect();
        assert_eq!(listed, ["Bearer sk-old", "Bearer sk-new"]);
        assert_eq!(runs(), 2);
    }

    #[test]
    fn a_401_at_models_names_the_key_command_then_the_check_again() {
        let dir = tempfile::tempdir().unwrap();
        let taking = chatting(|_| Reply::ok(OPENAI));
        std::fs::write(dir.path().join("key"), "sk-old\n").unwrap();
        let (b, key, _) = keyed(&taking, dir.path());
        std::fs::write(&key, "sk-new\n").unwrap();
        let strict = FakeServer::start(|r| match r.header("authorization") {
            Some("Bearer sk-new") => models_reply(&["m"]),
            _ => Reply::status(401, ""),
        });
        let waits = heard(|waits| {
            moved(&b, &strict.url).ready(waits).unwrap();
        });
        let check = Some(asking(&strict.url));
        assert_eq!(waits, [check.clone(), Some(key_command()), check, None]);
    }

    #[test]
    fn ready_names_its_key_command_then_its_check_and_ends_each() {
        let dir = tempfile::tempdir().unwrap();
        let fake = chatting(|_| Reply::ok(OPENAI));
        let (entry, _) = entry_by(&fake.url, dir.path(), "echo sk-1");
        let first = heard(|waits| {
            entry.clone().ready(waits).unwrap();
        });
        assert_eq!(
            first,
            [Some(key_command()), None, Some(asking(&fake.url)), None]
        );
        // The key is kept, so no command runs.
        let again = heard(|waits| {
            entry.ready(waits).unwrap();
        });
        assert_eq!(again, [Some(asking(&fake.url)), None]);
    }

    #[test]
    fn a_check_that_fails_still_ends_its_wait() {
        let closed = closed_port_url();
        let nothing = heard(|waits| {
            backend(&closed, "").ready(waits).unwrap_err();
        });
        assert_eq!(nothing, [Some(asking(&closed)), None]);
        let dir = tempfile::tempdir().unwrap();
        let (entry, _) = entry_by(&closed, dir.path(), "exit 1");
        let failing = heard(|waits| {
            entry.ready(waits).unwrap_err();
        });
        assert_eq!(failing, [Some(key_command()), None]);
    }

    #[test]
    fn ready_hides_a_long_key_echoed_back_before_any_of_it_is_cut() {
        let key = long_key();
        for (status, body) in [
            (
                401,
                format!(r#"{{"error":{{"message":"Invalid token {key}"}}}}"#),
            ),
            (200, format!("Invalid token {key}")),
        ] {
            let fake = FakeServer::start(move |_| Reply::status(status, body.clone()));
            let b = backend(&fake.url, &format!("api_key_cmd = \"echo {key}\"\n"));
            let said = b.ready(&|_| {}).unwrap_err();
            assert!(
                said.contains("Invalid token <key>") && !shows_part_of(&said, &key),
                "{said}"
            );
        }
    }

    #[test]
    fn ready_settles_a_model_whose_id_holds_the_key() {
        let fake = FakeServer::start(|_| models_reply(&["my-secret-model"]));
        for model in ["", "model = \"my-secret-model\"\n"] {
            let text = format!(
                "[llm.srv]\nurl = \"{}\"\napi_key_cmd = \"echo secret\"\n{model}",
                fake.url
            );
            let s = Settings::parse(Path::new("/c/config.toml"), &text).unwrap();
            let ready = Backend::named("srv", &s).unwrap().ready(&|_| {});
            assert_eq!(
                ready.map(|(b, _)| b.label),
                Ok("srv@my-secret-model".into()),
                "{model:?}"
            );
        }
    }

    #[test]
    fn a_key_sent_over_plain_http_to_another_machine_warns() {
        assert_eq!(
            key_warning("srv", "http://192.168.1.5:8000/v1").as_deref(),
            Some(
                "srv sends its key to 192.168.1.5 over plain http://, where anyone on the network between can read it"
            )
        );
        for quiet in [
            "http://127.0.0.1:8000/v1",
            "http://localhost/v1",
            "https://openrouter.ai/api/v1",
        ] {
            assert_eq!(key_warning("srv", quiet), None, "{quiet}");
        }
    }

    #[test]
    fn a_context_is_the_settled_models_own_and_a_whole_number() {
        for (models, context) in [
            (
                r#"{"data":[{"id":"m","context_length":131072}]}"#,
                Context::Tokens(131_072),
            ),
            (
                r#"{"data":[{"id":"m","meta":{"n_ctx":8192},"max_model_len":4096,"context_length":2048}]}"#,
                Context::Tokens(8192),
            ),
            (
                r#"{"data":[{"id":"m","meta":{"n_ctx":"8k"},"max_model_len":4096,"context_length":2048}]}"#,
                Context::Tokens(4096),
            ),
            (
                r#"{"data":[{"id":"m","meta":{"n_ctx":0},"max_model_len":4096}]}"#,
                Context::Tokens(4096),
            ),
            (
                r#"{"data":[{"id":"m","meta":{"n_ctx":0},"max_model_len":0,"context_length":0}]}"#,
                Context::Unknown,
            ),
            (
                r#"{"data":[{"id":"m","max_model_len":"16k","meta":{"n_ctx":null}}]}"#,
                Context::Unknown,
            ),
            (
                r#"{"data":[{"id":"x","max_model_len":4096},{"id":"m"}]}"#,
                Context::Unknown,
            ),
        ] {
            let fake = FakeServer::start(move |r| match r.path.as_str() {
                "/v1/models" => Reply::ok(models),
                _ => Reply::status(404, ""),
            });
            let (ready, _) = backend(&fake.url, "").ready(&|_| {}).unwrap();
            assert_eq!(ready.context(), Some(context), "{models}");
        }
        let zero = FakeServer::start(|r| match r.path.as_str() {
            "/v1/models" => Reply::ok(r#"{"data":[{"id":"m","context_length":0}]}"#),
            _ => ps_reply(&[("m", 2048)]),
        });
        let (ready, _) = backend(&zero.url, "").ready(&|_| {}).unwrap();
        assert_eq!(
            ready.context(),
            Some(Context::Tokens(2048)),
            "a 0 asks /api/ps"
        );
        assert_eq!(backend("http://h/v1", "").context(), Some(Context::Unknown));
        assert_eq!(Backend::shell("x").context(), None);
    }

    #[test]
    fn api_ps_is_asked_only_of_a_url_ending_in_v1() {
        let fake = FakeServer::start(|_| models_reply(&["m"]));
        let root = fake.url.strip_suffix("/v1").unwrap();
        let (ready, _) = backend(&format!("{root}/openai"), "")
            .ready(&|_| {})
            .unwrap();
        assert_eq!(ready.context(), Some(Context::Unknown));
        assert_eq!(asked(&fake), ["GET /openai/models"]);
    }

    #[test]
    fn api_ps_is_asked_of_ollama_and_of_an_entry_over_plain_http_only() {
        let entry = |url: &str| {
            let Via::Server(server) = backend(url, "").via else {
                unreachable!()
            };
            server
        };
        for (url, root) in [
            ("http://127.0.0.1:11434/v1", Some("http://127.0.0.1:11434")),
            (
                "http://192.168.1.5:11434/v1",
                Some("http://192.168.1.5:11434"),
            ),
            ("https://openrouter.ai/api/v1", None),
            ("https://ollama.example.com/v1", None),
            ("http://127.0.0.1:8000", None),
        ] {
            assert_eq!(entry(url).ps_root(), root, "{url}");
        }
        // `OLLAMA_HOST` names Ollama itself, whatever its scheme.
        assert_eq!(
            Server::ollama("m", "https://ollama.example.com/v1").ps_root(),
            Some("https://ollama.example.com")
        );
    }

    #[test]
    fn api_ps_finds_the_model_by_name_or_model() {
        for (ps, context) in [
            (
                r#"{"models":[{"name":"m","context_length":8192}]}"#,
                Context::Tokens(8192),
            ),
            (
                r#"{"models":[{"name":"m:latest","model":"m","context_length":8192}]}"#,
                Context::Tokens(8192),
            ),
            (r#"{"models":[{"name":"m"}]}"#, Context::Unknown),
            (
                r#"{"models":[{"name":"m","context_length":"8k"}]}"#,
                Context::Unknown,
            ),
            (
                r#"{"models":[{"name":"m","context_length":0}]}"#,
                Context::Unknown,
            ),
            (
                r#"{"models":[{"name":"x","context_length":8192}]}"#,
                Context::NotLoaded,
            ),
            (r#"{"models":null}"#, Context::Unknown),
        ] {
            assert_eq!(ps_context(ps, "m"), context, "{ps}");
        }
    }

    #[test]
    fn a_refused_api_ps_is_unknown_and_drops_no_key() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let cmd = format!("echo x >> '{}'; echo sk-9", count.display());
        let fake = FakeServer::start(|r| match r.path.as_str() {
            "/v1/models" => models_reply(&["m"]),
            _ => Reply::status(401, ""),
        });
        let b = backend(&fake.url, &format!("api_key_cmd = \"{cmd}\"\n"));
        for _ in 0..2 {
            let (ready, _) = b.clone().ready(&|_| {}).unwrap();
            assert_eq!(ready.context(), Some(Context::Unknown));
        }
        assert_eq!(
            asked(&fake),
            [
                "GET /v1/models",
                "GET /api/ps",
                "GET /v1/models",
                "GET /api/ps"
            ]
        );
        assert!(
            fake.requests()
                .iter()
                .all(|r| r.header("authorization") == Some("Bearer sk-9"))
        );
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 1);
    }

    #[test]
    fn sized_loads_a_model_ollama_has_not_loaded_then_reads_its_context() {
        let fake = FakeServer::ollama("gemma4:latest", 8192);
        let (ready, _) = any(&fake.url).ready(&|_| {}).unwrap();
        assert_eq!(ready.context(), Some(Context::NotLoaded));
        let root = fake.url.strip_suffix("/v1").unwrap();
        let mut sized = None;
        let loading = heard(|waits| sized = Some(ready.sized(waits).unwrap()));
        let line = format!("loading gemma4:latest into Ollama at {root}");
        assert_eq!(loading, [Some(Wait::Loading(line)), None]);
        let sized = sized.unwrap();
        assert_eq!(sized.context(), Some(Context::Tokens(8192)));
        assert_eq!(
            asked(&fake),
            [
                "GET /v1/models",
                "GET /api/ps",
                "POST /api/generate",
                "GET /api/ps"
            ]
        );
        assert_eq!(
            fake.requests()[2].json(),
            serde_json::json!({"model": "gemma4:latest"})
        );
        let n = fake.requests().len();
        let again = heard(|waits| {
            sized.sized(waits).unwrap();
        });
        assert_eq!(again, []);
        Backend::shell("x").sized(&|_| {}).unwrap();
        assert_eq!(fake.requests().len(), n, "nothing more to ask");
    }

    #[test]
    fn a_model_still_not_listed_once_loaded_has_an_unknown_context() {
        let fake = FakeServer::start(|r| match r.path.as_str() {
            "/v1/models" => models_reply(&["m"]),
            "/api/ps" => ps_reply(&[]),
            _ => Reply::ok("{}"),
        });
        let (ready, _) = any(&fake.url).ready(&|_| {}).unwrap();
        assert_eq!(
            ready.sized(&|_| {}).unwrap().context(),
            Some(Context::Unknown)
        );
    }

    #[test]
    fn a_model_that_will_not_load_stops_the_scan_saying_why() {
        let fake = FakeServer::start(|r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/v1/models") => models_reply(&["m"]),
            ("GET", "/api/ps") => ps_reply(&[]),
            (_, "/api/generate") => Reply::status(
                500,
                r#"{"error":"model requires more system memory (9.1 GiB) than is available (6.0 GiB)"}"#,
            ),
            _ => Reply::status(404, ""),
        });
        let ready = any(&fake.url).ready(&|_| {}).unwrap().0;
        let mut sized = None;
        let loading = heard(|waits| sized = Some(ready.sized(waits)));
        assert_eq!(
            sized.unwrap().unwrap_err(),
            "any@m: Ollama could not load m: HTTP 500: model requires more system memory (9.1 GiB) than is available (6.0 GiB)"
        );
        let root = fake.url.strip_suffix("/v1").unwrap();
        let line = format!("loading m into Ollama at {root}");
        assert_eq!(loading, [Some(Wait::Loading(line)), None]);
        let echoing = FakeServer::start(|r| match r.path.as_str() {
            "/v1/models" => models_reply(&["m"]),
            "/api/ps" => ps_reply(&[]),
            _ => Reply::status(500, r#"{"error":"bad key sk-5"}"#),
        });
        let b = backend(&echoing.url, "api_key_cmd = \"echo sk-5\"\n");
        assert_eq!(
            b.ready(&|_| {}).unwrap().0.sized(&|_| {}).unwrap_err(),
            "srv@m: Ollama could not load m: HTTP 500: bad key <key>"
        );
    }

    #[test]
    fn a_models_load_gets_the_backends_limit() {
        let fake = FakeServer::start(|r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/v1/models") => models_reply(&["m"]),
            ("GET", "/api/ps") => ps_reply(&[]),
            ("POST", "/api/generate") => Reply::ok("{}").after(Duration::from_secs(5)),
            _ => Reply::status(404, ""),
        });
        let backend = Backend {
            timeout: Duration::from_secs(1),
            ..any(&fake.url)
        };
        let began = Instant::now();
        let why = backend
            .ready(&|_| {})
            .unwrap()
            .0
            .sized(&|_| {})
            .unwrap_err();
        assert!(why.ends_with("/api/generate timed out after 1s"), "{why}");
        assert!(began.elapsed() < Duration::from_secs(4));
    }

    const OLLAMA_CHAT: &str = include_str!("../../tests/fixtures/llm/ollama-chat.json");
    const OLLAMA_CHAT_CUT: &str = include_str!("../../tests/fixtures/llm/ollama-chat-cut.json");
    const OLLAMA_UNKNOWN: &str =
        include_str!("../../tests/fixtures/llm/ollama-chat-unknown-model.json");
    const OLLAMA_OVERFLOW: &str = include_str!("../../tests/fixtures/llm/ollama-overflow.json");

    #[test]
    fn ollama_host_gives_the_address_the_ollama_cli_calls() {
        for (host, url) in [
            (None, "http://127.0.0.1:11434/v1"),
            (Some("  "), "http://127.0.0.1:11434/v1"),
            (Some("0.0.0.0"), "http://0.0.0.0:11434/v1"),
            (Some("0.0.0.0:8080"), "http://0.0.0.0:8080/v1"),
            (Some(":11434"), "http://127.0.0.1:11434/v1"),
            (Some("box.lan"), "http://box.lan:11434/v1"),
            (Some("box.lan:x"), "http://box.lan:11434/v1"),
            (Some("http://box.lan"), "http://box.lan:80/v1"),
            (
                Some("https://ollama.example.com/"),
                "https://ollama.example.com:443/v1",
            ),
            (
                Some("http://box.lan:9000/ollama"),
                "http://box.lan:9000/ollama/v1",
            ),
            (Some("::1"), "http://[::1]:11434/v1"),
            (Some("[::1]:11434"), "http://[::1]:11434/v1"),
            (Some("[::1]"), "http://[::1]:11434/v1"),
            (Some(" 'box.lan:9000' "), "http://box.lan:9000/v1"),
            (Some("box lan"), "http://127.0.0.1:11434/v1"),
        ] {
            assert_eq!(ollama_url(host), url, "{host:?}");
        }
    }

    #[test]
    fn ollama_keeps_its_label_and_asks_for_latest_when_untagged() {
        let fake = FakeServer::ollama("gemma4:latest", 4096);
        let b = Backend::ollama("gemma4", &fake.url);
        assert_eq!(
            (b.label.as_str(), b.chunk_lines, b.jobs),
            ("ollama:gemma4", None, JOBS)
        );
        let (ready, warnings) = b.ready(&|_| {}).unwrap();
        assert_eq!((ready.label.as_str(), warnings.len()), ("ollama:gemma4", 0));
        let sized = ready.sized(&|_| {}).unwrap();
        assert_eq!(
            (sized.label.as_str(), sized.context()),
            ("ollama:gemma4", Some(Context::Tokens(4096)))
        );
        // Each call has the whole context.
        assert_eq!(
            (
                sized.lines_per_chunk(1, None),
                sized.lines_per_chunk(2, None)
            ),
            (fitting_lines(4096), fitting_lines(4096))
        );
        assert!(sized.call("P", TIMEOUT, None, &|_| {}).reply.is_ok());
        let chats = fake.chats();
        assert_eq!(chats.len(), 1, "{:?}", asked(&fake));
        assert_eq!(chats[0].json()["model"], "gemma4:latest");
    }

    #[test]
    fn ollama_gives_each_call_the_whole_context() {
        let fake = FakeServer::ollama("gemma4:latest", 16384);
        let (ready, _) = Backend::ollama("gemma4", &fake.url).ready(&|_| {}).unwrap();
        let sized = ready.sized(&|_| {}).unwrap();
        for jobs in [1, 2, 4] {
            assert_eq!(
                sized.budget(jobs),
                Some(Budget::new(16384, None)),
                "-j {jobs}"
            );
        }
    }

    /// `ollama:gemma4` readied on a server that lists `gemma4:latest`, says its context in
    /// `/api/ps` when `context` is `Some`, and answers `/api/chat` with `chat`.
    fn ollama_answering(chat: Reply, context: Option<u64>) -> (FakeServer, Backend) {
        let fake =
            FakeServer::start(
                move |r| match (r.method.as_str(), r.path.as_str(), context) {
                    ("GET", "/v1/models", _) => models_reply(&["gemma4:latest"]),
                    ("GET", "/api/ps", Some(context)) => ps_reply(&[("gemma4:latest", context)]),
                    ("POST", "/api/chat", _) => chat.clone(),
                    _ => Reply::status(404, "{}"),
                },
            );
        let (ready, _) = Backend::ollama("gemma4", &fake.url).ready(&|_| {}).unwrap();
        (fake, ready)
    }

    #[test]
    fn ollama_is_asked_natively_and_its_reply_read() {
        let (fake, b) = ollama_answering(Reply::ok(OLLAMA_CHAT), None);
        let root = fake.url.strip_suffix("/v1").unwrap();
        assert_eq!(b.command_line(), [format!("{root}/api/chat")]);
        let a = b.call("PROMPT", TIMEOUT, None, &|_| {});
        assert_eq!(
            (a.reply.as_deref(), a.too_big),
            (Ok("We should leave before it rains.\n"), None)
        );
        assert_eq!(
            a.used,
            Used::Reported(Usage {
                input_tokens: 47,
                output_tokens: 8,
                cache_read_tokens: Some(0),
                cache_write_tokens: None,
                cost_usd: None
            })
        );
        assert_eq!(
            asked(&fake),
            ["GET /v1/models", "GET /api/ps", "POST /api/chat"]
        );
        let chat = &fake.chats()[0];
        assert_eq!(chat.header("content-type"), Some("application/json"));
        assert_eq!(
            chat.json(),
            serde_json::json!({
                "model": "gemma4:latest",
                "messages": [{"role": "system", "content": SYSTEM}, {"role": "user", "content": "PROMPT"}],
                "stream": false,
                "think": false,
                "truncate": false,
            })
        );
        let cut =
            ollama_answering(Reply::ok(OLLAMA_CHAT_CUT), None)
                .1
                .call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            (cut.too_big, cut.reply),
            (
                Some(TooBig::Reply),
                Err("the reply was cut off at the server's length limit".into())
            )
        );
        assert_eq!(
            cut.used,
            Used::Reported(Usage {
                input_tokens: 47,
                output_tokens: 3,
                cache_read_tokens: Some(42),
                cache_write_tokens: None,
                cost_usd: None
            })
        );
        let over = ollama_answering(Reply::status(400, OLLAMA_OVERFLOW), None)
            .1
            .call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            (over.too_big, over.reply, over.used),
            (
                Some(TooBig::Prompt),
                Err("HTTP 400: request (19523 tokens) exceeds the available context size (16384 tokens), try increasing it".into()),
                Used::Unreported
            )
        );
        // A prompt that fills the context may have been cut, whatever `truncate` says.
        let filled =
            ollama_answering(Reply::ok(OLLAMA_CHAT), Some(49))
                .1
                .call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            (filled.too_big, filled.reply),
            (
                Some(TooBig::Prompt),
                Err(
                    "the prompt filled 47 of the server's 49 tokens, so the server may have cut it"
                        .into()
                )
            )
        );
    }

    #[test]
    fn ollama_caps_a_reply_in_its_own_words() {
        let (fake, b) = ollama_answering(Reply::ok(OLLAMA_CHAT), None);
        // A quarter of 1024 tokens.
        b.call("P", TIMEOUT, Some(Budget::new(1024, None)), &|_| {});
        b.call("P", TIMEOUT, None, &|_| {});
        let options: Vec<Option<Value>> = fake
            .chats()
            .iter()
            .map(|c| c.json().get("options").cloned())
            .collect();
        assert_eq!(
            options,
            [Some(serde_json::json!({"num_predict": 256})), None]
        );
    }

    #[test]
    fn an_ollama_reply_without_counts_content_or_json_says_so() {
        for (body, reply, used) in [
            (
                r#"{"message":{"content":"NONE"},"done_reason":"stop"}"#,
                Ok("NONE\n"),
                Used::Unreported,
            ),
            (
                r#"{"done_reason":"stop","prompt_eval_count":5}"#,
                Err("the reply has no content"),
                Used::Reported(Usage {
                    input_tokens: 5,
                    ..Usage::default()
                }),
            ),
            (
                r#"{"error":"boom"}"#,
                Err("the reply is an error: boom"),
                Used::Unreported,
            ),
            (
                "<!DOCTYPE html>",
                Err("reply is not an Ollama chat reply: <!DOCTYPE html>"),
                Used::Unreported,
            ),
        ] {
            let a = ollama_answering(Reply::ok(body), None)
                .1
                .call("P", TIMEOUT, None, &|_| {});
            assert_eq!(
                (a.reply, a.used, a.too_big),
                (reply.map(String::from).map_err(String::from), used, None),
                "{body}"
            );
        }
    }

    #[test]
    fn a_model_ollama_lacks_says_to_pull_it() {
        let other = FakeServer::start(|_| models_reply(&["other:latest"]));
        assert_eq!(
            Backend::ollama("gemma4", &other.url)
                .ready(&|_| {})
                .unwrap_err(),
            format!(
                "ollama:gemma4: {} does not serve \"gemma4\"; it serves \"other:latest\"; \
                 ollama pull gemma4 fetches it",
                other.url
            )
        );
        let none = FakeServer::start(|_| models_reply(&[]));
        assert_eq!(
            Backend::ollama("gemma4", &none.url)
                .ready(&|_| {})
                .unwrap_err(),
            format!(
                "ollama:gemma4: {} serves no model; ollama pull gemma4 fetches it",
                none.url
            )
        );
        let fake = FakeServer::start(|_| Reply::status(404, OLLAMA_UNKNOWN));
        let a = Backend::ollama("no-such-model", &fake.url).call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            (a.reply, a.used),
            (
                Err("HTTP 404: model 'no-such-model' not found; ollama pull no-such-model fetches it".into()),
                Used::Unreported
            )
        );
        // An entry's server is not told to pull: it need not be Ollama.
        let entry = backend(&fake.url, "").call("P", TIMEOUT, None, &|_| {});
        assert_eq!(
            entry.reply,
            Err("HTTP 404: model 'no-such-model' not found".into())
        );
    }

    #[test]
    fn an_ollama_that_does_not_say_its_size_says_to_set_the_flag() {
        let fake = FakeServer::start(|r| match r.path.as_str() {
            "/v1/models" => models_reply(&["gemma4:latest"]),
            _ => Reply::status(404, "{}"),
        });
        let (b, _) = Backend::ollama("gemma4", &fake.url).ready(&|_| {}).unwrap();
        assert_eq!(b.context(), Some(Context::Unknown));
        assert_eq!(
            b.size_warning(2).as_deref(),
            Some(
                "ollama:gemma4 did not say its context size; set --chunk-lines if it holds under 19k tokens"
            )
        );
    }

    #[test]
    fn an_untagged_model_is_the_one_tagged_latest() {
        for (asked, served, same) in [
            ("gemma4", "gemma4", true),
            ("gemma4", "gemma4:latest", true),
            ("org/gemma4", "org/gemma4:latest", true),
            ("gemma4:latest", "gemma4", false),
            ("gemma4", "gemma4:12b", false),
            ("gemma4:12b", "gemma4:12b:latest", false),
            ("gemma", "gemma4:latest", false),
        ] {
            assert_eq!(same_model(asked, served), same, "{asked} {served}");
        }
        // The name as asked wins over its `:latest`.
        let ids = ["gemma4:latest".to_string(), "gemma4".to_string()];
        assert_eq!(settle("u", Some("gemma4"), &ids), Ok("gemma4".into()));
        assert_eq!(
            settle("u", Some("gemma4"), &ids[..1]),
            Ok("gemma4:latest".into())
        );
    }
}
