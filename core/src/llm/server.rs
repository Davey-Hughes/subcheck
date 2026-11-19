//! An LLM server named by a settings entry with a `url`, which subcheck calls over its OpenAI API.
//!
//! A call posts a chat to `<url>/chat/completions`: the model, [`SYSTEM`] as the system message and
//! the prompt as the user's, no streaming, then the entry's `request` fields over the top. Thinking
//! is off unless the entry leaves it to the server: with it on, gemma4 can spend its whole budget
//! thinking and reply with nothing. Servers read the switch in different places, so both are sent:
//! `reasoning_effort`, and `enable_thinking` for the model's chat template.

use std::fmt::Write as _;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value};
use ureq::http::{Response, Uri};
use ureq::tls::{RootCerts, TlsConfig};
use ureq::{Agent, Body, RequestBuilder};

use super::key::{self, Key};
use super::{Answer, Output, SYSTEM, Used};
use crate::settings::{KeySource, ServerEntry, Thinking};

#[cfg(test)]
mod live_replies;

/// How long the start-up check waits for the list of models. A server that is up answers at once;
/// a scan should soon say that one is stuck, not wait the limit a chunk has.
const MODELS_TIMEOUT: Duration = Duration::from_secs(30);

/// A settings entry's server, as a backend calls it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    /// The entry's name, which the backend's label starts with.
    pub name: String,
    pub entry: ServerEntry,
    /// The model the calls ask for: the entry's, or the one a run recorded. `None` until the
    /// server says which it serves, when the entry names none.
    pub model: Option<String>,
    /// The key the calls send: `None` for an entry without one, and until it is fetched.
    key: Option<Key>,
}

impl Server {
    pub(super) fn new(name: &str, entry: &ServerEntry, model: Option<&str>) -> Server {
        Server {
            name: name.to_string(),
            entry: entry.clone(),
            model: model.map(ToString::to_string),
            key: None,
        }
    }

    /// One call: the prompt sent as a chat, the reply read as an OpenAI chat completion. An error
    /// has the key hidden, since a server may echo back the key it refused.
    pub(super) fn call(&self, prompt: &str, timeout: Duration) -> Answer {
        let Some(model) = &self.model else {
            return Answer {
                reply: Err("the model is not settled".into()),
                used: Used::Untracked,
            };
        };
        let url = &self.entry.url;
        let body = body(model, prompt, self.entry.thinking, &self.entry.request).to_string();
        let request =
            match self.prepared(agent(url).post(format!("{url}/chat/completions")), timeout) {
                Ok(request) => request.header("Content-Type", "application/json"),
                Err(why) => {
                    return Answer {
                        reply: Err(why),
                        used: Used::Untracked,
                    };
                }
            };
        let mut answer = match request.send(body).and_then(status_and_text) {
            Ok((status, text)) if (200..300).contains(&status) => self.read(text),
            Ok((status, text)) => {
                let mut reply = error_text(status, &self.hidden(text));
                self.refused(status, &mut reply);
                Answer {
                    reply: Err(reply),
                    used: Used::Unreported,
                }
            }
            Err(e) => {
                let (why, used) = failed(url, "chat/completions", &e, timeout);
                Answer {
                    reply: Err(why),
                    used,
                }
            }
        };
        // What ureq says of a failure, and an error `read` keeps as it was, have not been hidden.
        answer.reply = answer.reply.map_err(|why| self.hidden(why));
        answer
    }

    /// The server readied for calls, as [`Backend::ready`](super::Backend::ready) says, with the
    /// warnings to show. An error has the key hidden, since a server may echo back the key it
    /// refused.
    pub(super) fn ready(mut self) -> Result<(Server, Vec<String>), String> {
        self.key = key::fetch(&self.entry.key)?;
        let url = &self.entry.url;
        let model = self
            .models()
            .and_then(|ids| settle(url, self.model.as_deref(), &ids));
        let model = model.map_err(|why| self.hidden(why))?;
        self.model = Some(model);
        let warning = self
            .key
            .as_ref()
            .and_then(|_| key_warning(&self.label(), &self.entry.url));
        Ok((self, warning.into_iter().collect()))
    }

    /// What a backend through this server records: `<name>@<model>`, or the name alone while the
    /// model is not settled.
    pub(super) fn label(&self) -> String {
        match &self.model {
            Some(model) => format!("{}@{model}", self.name),
            None => self.name.clone(),
        }
    }

    /// The ids `GET <url>/models` lists.
    fn models(&self) -> Result<Vec<String>, String> {
        let url = &self.entry.url;
        let (status, text) = self
            .prepared(agent(url).get(format!("{url}/models")), MODELS_TIMEOUT)?
            .call()
            .and_then(status_and_text)
            .map_err(|e| failed(url, "models", &e, MODELS_TIMEOUT).0)?;
        if !(200..300).contains(&status) {
            let mut why = format!("HTTP {status} from {url}/models");
            if let Some(said) = server_said(&self.hidden(text)) {
                let _ = write!(why, ": {said}");
            }
            self.refused(status, &mut why);
            return Err(why);
        }
        // Read before the key is hidden: a model's id may hold it.
        let Ok(list) = serde_json::from_str::<ModelList>(&text) else {
            let text = self.hidden(text);
            let first = text.trim().lines().next().unwrap_or("(empty)");
            return Err(format!("{url}/models gave no list of models: {first:.80}"));
        };
        Ok(list.data.into_iter().map(|m| m.id).collect())
    }

    /// `request` with what every request to the server carries: `timeout`, and the key when the
    /// entry has one. Refused while that key is not fetched: the server would refuse the request,
    /// and its 401 would drop the kept key.
    fn prepared<B>(
        &self,
        request: RequestBuilder<B>,
        timeout: Duration,
    ) -> Result<RequestBuilder<B>, String> {
        let request = request.config().timeout_global(Some(timeout)).build();
        match (&self.key, &self.entry.key) {
            (Some(key), _) => Ok(request.header("Authorization", key.bearer())),
            (None, KeySource::None) => Ok(request),
            (None, _) => Err("the key is not fetched".into()),
        }
    }

    /// A 2xx reply read as a chat completion. What succeeds keeps the key as the server sent it: a
    /// key can be a plain word, which a correction may hold, and hiding it would change the
    /// correction or break the JSON around it. An error is read again from the text with the key
    /// hidden, so a message cut through an echoed key keeps no part of it that `hide` cannot find.
    fn read(&self, text: String) -> Answer {
        let answer = Output::OpenAi.read(&text);
        if answer.reply.is_ok() {
            return answer;
        }
        let hidden = Output::OpenAi.read(&self.hidden(text)).reply;
        Answer {
            // Hiding the key can change what the JSON says, so an error stays one.
            reply: hidden.and(answer.reply),
            used: answer.used,
        }
    }

    /// `text` with the key hidden, when there is one.
    fn hidden(&self, text: String) -> String {
        match &self.key {
            Some(key) => key.hide(&text),
            None => text,
        }
    }

    /// On a 401 or 403, the key is dropped, so the next fetch runs its command again, and `why`
    /// says to check it.
    fn refused(&self, status: u16, why: &mut String) {
        if matches!(status, 401 | 403) {
            if let Some(key) = &self.key {
                key::forget(&self.entry.key, key);
            }
            why.push_str("; check the entry's key");
        }
    }
}

/// A response's status and body, the body read whatever the status, since an error's says why.
/// The key is left in: each path that turns the body into a message hides it there, before any cut.
fn status_and_text(mut response: Response<Body>) -> Result<(u16, String), ureq::Error> {
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string()?;
    Ok((status, text))
}

/// What `GET <url>/models` answers, as far as it is read: each model's id.
#[derive(Deserialize)]
struct ModelList {
    data: Vec<ModelId>,
}

#[derive(Deserialize)]
struct ModelId {
    id: String,
}

/// The model the calls ask for: `asked` (the entry's or the run's) when the server serves it,
/// else the only model it serves.
fn settle(url: &str, asked: Option<&str>, ids: &[String]) -> Result<String, String> {
    match (asked, ids) {
        (_, []) => Err(format!("{url} serves no model")),
        (Some(asked), _) if ids.iter().any(|id| id == asked) => Ok(asked.to_string()),
        (Some(asked), _) => Err(format!(
            "{url} does not serve {asked:?}; it serves {}",
            listed(ids)
        )),
        (None, [only]) => Ok(only.clone()),
        (None, _) => Err(format!(
            "{url} serves {}: name one as the entry's model",
            listed(ids)
        )),
    }
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

/// A call's request body: the chat, the switches that turn thinking off when it is, then each
/// field of the entry's `request` over the top.
fn body(model: &str, prompt: &str, thinking: Thinking, request: &Map<String, Value>) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), model.into());
    body.insert(
        "messages".into(),
        serde_json::json!([
            {"role": "system", "content": SYSTEM},
            {"role": "user", "content": prompt},
        ]),
    );
    body.insert("stream".into(), false.into());
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

/// `HTTP <status>`, then the server's words when it says any.
fn error_text(status: u16, body: &str) -> String {
    match server_said(body) {
        Some(said) => format!("HTTP {status}: {said}"),
        None => format!("HTTP {status}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Instant;

    use super::*;
    use crate::llm::{Backend, TIMEOUT, Usage, Via};
    use crate::settings::{KeySource, Settings};
    use crate::test_support::{FakeServer, Reply, closed_port_url, models_reply};

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
        s.key = key::fetch(source).unwrap();
        Backend {
            via: Via::Server(s),
            ..b
        }
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
        let a = b.call("PROMPT", TIMEOUT);
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
    fn a_call_before_the_key_is_fetched_sends_nothing() {
        let fake = FakeServer::start(|_| Reply::ok(OPENAI));
        let a = backend(&fake.url, "api_key_cmd = \"echo sk-1\"\n").call("P", TIMEOUT);
        assert_eq!(
            (a.reply, a.used),
            (Err("the key is not fetched".into()), Used::Untracked)
        );
        assert!(fake.requests().is_empty());
    }

    #[test]
    fn thinking_left_to_the_server_sends_neither_switch() {
        let fake = FakeServer::start(|_| Reply::ok(OPENAI));
        backend(&fake.url, "thinking = \"server\"\n").call("P", TIMEOUT);
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
        let a = b.call("P", TIMEOUT);
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
            let said = b.call("P", TIMEOUT).reply.unwrap_err();
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
            b.call("P", TIMEOUT).reply.as_deref(),
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
            assert_eq!(b.call("P", TIMEOUT).reply.as_deref(), Ok("NONE\n"), "{key}");
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
        assert!(b.call("P", TIMEOUT).reply.is_err());
    }

    #[test]
    fn a_401_or_403_forgets_the_kept_key() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let cmd = format!("echo x >> '{}'; echo sk-7", count.display());
        let source = KeySource::Command(cmd.clone());
        for status in [401, 403] {
            let fake = FakeServer::start(move |_| Reply::status(status, ""));
            // A call forgets the key of the command its entry names.
            let entry = backend(&fake.url, &format!("api_key_cmd = \"{cmd}\"\n"));
            with_key(entry, &source).call("P", TIMEOUT);
            key::fetch(&source).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 3);
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
            let a = backend(&fake.url, "").call("P", TIMEOUT);
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
            let a = backend(&fake.url, "").call("P", TIMEOUT);
            assert_eq!((a.reply, a.used), (reply, used));
        }
    }

    #[test]
    fn no_server_or_a_slow_one_fails_the_call() {
        let closed = closed_port_url();
        let a = backend(&closed, "").call("P", TIMEOUT);
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
        let a = backend(&slow.url, "").call("P", Duration::from_secs(1));
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
        let (b, warnings) = any(&fake.url).ready().unwrap();
        assert_eq!((b.label.as_str(), warnings.len()), ("any@gemma4", 0));
        assert!(matches!(&b.via, Via::Server(s) if s.model.as_deref() == Some("gemma4")));
        let [r] = &fake.requests()[..] else { panic!() };
        assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/v1/models"));
        let named = FakeServer::start(|_| models_reply(&["x", "m"]));
        assert_eq!(
            backend(&named.url, "").ready().map(|(b, _)| b.label),
            Ok("srv@m".into())
        );
        let shell = Backend::shell("x");
        assert_eq!(shell.clone().ready(), Ok((shell, Vec::new())));
    }

    #[test]
    fn ready_refuses_a_model_the_server_does_not_serve_naming_both() {
        let twelve: Vec<String> = (1..=12).map(|i| format!("m{i}")).collect();
        let ids: Vec<&str> = twelve.iter().map(String::as_str).collect();
        let listed = r#""m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10" and 2 more"#;
        let reply = models_reply(&ids);
        let fake = FakeServer::start(move |_| reply.clone());
        assert_eq!(
            backend(&fake.url, "").ready().unwrap_err(),
            format!(
                "srv@m: {} does not serve \"m\"; it serves {listed}",
                fake.url
            )
        );
        assert_eq!(
            any(&fake.url).ready().unwrap_err(),
            format!(
                "any: {} serves {listed}: name one as the entry's model",
                fake.url
            )
        );
        let none = FakeServer::start(|_| models_reply(&[]));
        assert_eq!(
            any(&none.url).ready().unwrap_err(),
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
                .ready()
                .is_ok()
        );
        assert_eq!(
            Backend::from_label("srv@a", &open)
                .unwrap()
                .ready()
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
            backend(&closed, "").ready().unwrap_err(),
            format!("srv@m: nothing answering at {closed}: is the server running?")
        );
        let failing = FakeServer::start(|_| Reply::status(500, r#"{"error":{"message":"boom"}}"#));
        assert_eq!(
            backend(&failing.url, "").ready().unwrap_err(),
            format!("srv@m: HTTP 500 from {}/models: boom", failing.url)
        );
        let odd = FakeServer::start(|_| Reply::ok("nope"));
        assert_eq!(
            backend(&odd.url, "").ready().unwrap_err(),
            format!("srv@m: {}/models gave no list of models: nope", odd.url)
        );
        let unset = backend(
            &failing.url,
            "api_key_env = \"SUBCHECK_TEST_KEY_NEVER_SET\"\n",
        );
        assert_eq!(
            unset.ready().unwrap_err(),
            "srv@m: api_key_env names SUBCHECK_TEST_KEY_NEVER_SET, which is unset or empty"
        );
        assert_eq!(
            failing.requests().len(),
            1,
            "the key comes before the server is asked"
        );
    }

    #[test]
    fn ready_sends_the_key_and_a_401_forgets_it_and_hides_it() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let cmd = format!("echo x >> '{}'; echo sk-8", count.display());
        let fake = FakeServer::start(|_| {
            Reply::status(401, r#"{"error":{"message":"no such key sk-8"}}"#)
        });
        let b = backend(&fake.url, &format!("api_key_cmd = \"{cmd}\"\n"));
        assert_eq!(
            b.clone().ready().unwrap_err(),
            format!(
                "srv@m: HTTP 401 from {}/models: no such key <key>; check the entry's key",
                fake.url
            )
        );
        assert_eq!(
            fake.requests()[0].header("authorization"),
            Some("Bearer sk-8")
        );
        b.ready().unwrap_err();
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2);
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
            let said = b.ready().unwrap_err();
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
            let ready = Backend::named("srv", &s).unwrap().ready();
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
}
