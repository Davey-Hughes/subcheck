//! A fake OpenAI-API server on 127.0.0.1, so no test needs a real model server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::fake_llm;
use crate::llm;

/// One request the server received.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    /// The value of header `name`, whatever case either is written in.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The body as JSON; a test that sends anything else is wrong, so this panics.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).expect("the request body is JSON")
    }
}

/// What the server answers.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub delay: Duration,
}

impl Reply {
    pub fn ok(body: impl Into<String>) -> Reply {
        Reply::status(200, body)
    }

    pub fn status(status: u16, body: impl Into<String>) -> Reply {
        Reply {
            status,
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    /// The same reply, sent only after `delay`.
    #[must_use]
    pub fn after(self, delay: Duration) -> Reply {
        Reply { delay, ..self }
    }
}

/// A server that answers each request with what its route returns, and remembers the requests.
/// Its threads end with the test process.
pub struct FakeServer {
    /// `http://127.0.0.1:<port>/v1`, as a settings entry's `url` is written.
    pub url: String,
    seen: Arc<Mutex<Vec<Request>>>,
}

impl FakeServer {
    pub fn start(route: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://127.0.0.1:{}/v1",
            listener.local_addr().unwrap().port()
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let route = Arc::new(route);
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let route = Arc::clone(&route);
                let log = Arc::clone(&log);
                // A thread to each connection, so a slow reply holds up no other.
                std::thread::spawn(move || serve(stream, &*route, &log));
            }
        });
        FakeServer { url, seen }
    }

    /// A server that lists `models` and answers chat completions as an LLM would, by running
    /// fake-llm.sh as `openai` on the last message's content.
    pub fn llm(models: &[&str]) -> FakeServer {
        FakeServer::llm_slow(models, Duration::ZERO)
    }

    /// [`FakeServer::llm`], answering `/v1/models` only after `models_delay`, as a server that is
    /// slow to say what it serves.
    pub fn llm_slow(models: &[&str], models_delay: Duration) -> FakeServer {
        FakeServer::llm_listing(models_reply(models).after(models_delay))
    }

    /// [`FakeServer::llm`], listing each of `models` as llama-server does, with `n_ctx`, the
    /// tokens its context holds, in `meta`.
    pub fn llm_sized(models: &[&str], n_ctx: u64) -> FakeServer {
        let data: Vec<_> = models
            .iter()
            .map(|id| serde_json::json!({"id": id, "object": "model", "meta": {"n_ctx": n_ctx}}))
            .collect();
        let models = serde_json::json!({"object": "list", "data": data});
        FakeServer::llm_listing(Reply::ok(models.to_string()))
    }

    /// Ollama serving `model`, not loaded, with a context of `context` tokens once it is:
    /// `/v1/models` lists it, and `/api/ps` lists nothing until a `POST /api/generate` for it, then
    /// lists it with `context`. Chat completions are answered as [`FakeServer::llm`] answers them,
    /// and Ollama's own `/api/chat` with the same reply in Ollama's words; a load or a chat for
    /// any other model is a 404, as Ollama answers one it lacks.
    pub fn ollama(model: &str, context: u64) -> FakeServer {
        let listed = models_reply(&[model]);
        let model = model.to_string();
        let loaded = AtomicBool::new(false);
        FakeServer::start(move |r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/v1/models") => listed.clone(),
            ("GET", "/api/ps") if loaded.load(Ordering::SeqCst) => ps_reply(&[(&model, context)]),
            ("GET", "/api/ps") => ps_reply(&[]),
            ("POST", "/api/generate" | "/api/chat") if r.json()["model"] != model.as_str() => {
                let asked = r.json()["model"].as_str().unwrap_or_default().to_string();
                let missing = format!("model '{asked}' not found");
                Reply::status(404, serde_json::json!({"error": missing}).to_string())
            }
            ("POST", "/api/generate") => {
                loaded.store(true, Ordering::SeqCst);
                let done = serde_json::json!({
                    "model": model,
                    "created_at": "2026-11-19T00:00:00Z",
                    "response": "",
                    "done": true,
                    "done_reason": "load",
                });
                Reply::ok(done.to_string())
            }
            ("POST", "/api/chat") => chatted(r),
            ("POST", "/v1/chat/completions") => answered(r),
            _ => Reply::status(404, "{}"),
        })
    }

    /// A server that lists models with `models` and answers chat completions as an LLM would.
    fn llm_listing(models: Reply) -> FakeServer {
        FakeServer::start(move |r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/v1/models") => models.clone(),
            ("POST", "/v1/chat/completions") => answered(r),
            _ => Reply::status(404, "{}"),
        })
    }

    /// Every request so far, in the order they arrived.
    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }

    /// The POSTs to `/v1/chat/completions`, in the order they came.
    pub fn completions(&self) -> Vec<Request> {
        self.posts("/v1/chat/completions")
    }

    /// The POSTs to Ollama's own `/api/chat`, in the order they came.
    pub fn chats(&self) -> Vec<Request> {
        self.posts("/api/chat")
    }

    fn posts(&self, path: &str) -> Vec<Request> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == "POST" && r.path == path)
            .collect()
    }
}

/// The `/v1/models` answer listing `ids`.
pub fn models_reply(ids: &[&str]) -> Reply {
    let data: Vec<_> = ids
        .iter()
        .map(|id| serde_json::json!({"id": id, "object": "model"}))
        .collect();
    Reply::ok(serde_json::json!({"object": "list", "data": data}).to_string())
}

/// Ollama's `/api/ps` answer listing `models` as loaded, each by `name` and `model`, with the
/// tokens its context holds.
pub fn ps_reply(models: &[(&str, u64)]) -> Reply {
    let models: Vec<_> = models
        .iter()
        .map(|(name, context)| {
            serde_json::json!({"name": name, "model": name, "context_length": context})
        })
        .collect();
    Reply::ok(serde_json::json!({"models": models}).to_string())
}

/// fake-llm.sh's chat completion for `r`'s last message ([`completion`]): what
/// [`FakeServer::llm`] answers.
pub fn answered(r: &Request) -> Reply {
    Reply::ok(completion(r).to_string())
}

/// A finished chat completion whose reply is `content`, with 1000 prompt and 50 reply tokens: what
/// a test's own route answers a call with.
pub fn chat_reply(content: &str) -> Reply {
    let completion = serde_json::json!({
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 1000, "completion_tokens": 50, "total_tokens": 1050},
    });
    Reply::ok(completion.to_string())
}

/// fake-llm.sh's chat completion for `r`'s last message, with the prompt counted as subcheck
/// estimates it ([`llm::estimate`]) and none of it cached: fake-llm.sh gives every prompt the same
/// count, too many for a call's share of the small contexts tests fit chunks to, where a count that
/// fills the share is a prompt the server may have cut ([`llm::TooBig::Prompt`]).
fn completion(r: &Request) -> serde_json::Value {
    let mut completion: serde_json::Value =
        serde_json::from_str(&complete(&last_content(r))).expect("fake-llm.sh printed JSON");
    let prompt = llm::estimate(prompt_lines(r));
    let reply = completion["usage"]["completion_tokens"]
        .as_u64()
        .expect("fake-llm.sh counts its reply");
    completion["usage"] = serde_json::json!({
        "prompt_tokens": prompt,
        "completion_tokens": reply,
        "total_tokens": prompt + reply,
    });
    completion
}

/// fake-llm.sh's reply to `r`'s last message as Ollama's `/api/chat` gives it, with the chat
/// completion's tokens as Ollama counts them.
fn chatted(r: &Request) -> Reply {
    let completion = completion(r);
    let reply = serde_json::json!({
        "model": r.json()["model"],
        "created_at": "2026-11-19T00:00:00Z",
        "message": {"role": "assistant", "content": completion["choices"][0]["message"]["content"]},
        "done": true,
        "done_reason": "stop",
        "prompt_eval_count": completion["usage"]["prompt_tokens"],
        "eval_count": completion["usage"]["completion_tokens"],
    });
    Reply::ok(reply.to_string())
}

/// How many lines of `r`'s last message start `L<digit>`: the subtitle lines a prompt sent.
pub fn prompt_lines(r: &Request) -> usize {
    last_content(r)
        .lines()
        .filter(|line| {
            line.strip_prefix('L')
                .and_then(|rest| rest.chars().next())
                .is_some_and(|c| c.is_ascii_digit())
        })
        .count()
}

/// A `/v1` URL on a port nothing listens on. [`FakeServer`] binds port 0, so the OS hands it an
/// ephemeral port, never port 1, and nothing standard listens on port 1 (tcpmux); a port that was
/// bound and let go could be handed to another test's server before the caller connects.
pub fn closed_port_url() -> String {
    "http://127.0.0.1:1/v1".to_string()
}

fn last_content(r: &Request) -> String {
    let body = r.json();
    body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .and_then(|m| m["content"].as_str())
        .expect("the last message has text content")
        .to_string()
}

/// fake-llm.sh's OpenAI reply to `prompt`.
fn complete(prompt: &str) -> String {
    let mut child = Command::new("bash")
        .arg(fake_llm())
        .env("FAKE_LLM_AS", "openai")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(prompt.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "fake-llm.sh failed");
    String::from_utf8(out.stdout).unwrap()
}

fn serve(stream: TcpStream, route: &dyn Fn(&Request) -> Reply, log: &Mutex<Vec<Request>>) {
    let Some(request) = read_request(&stream) else {
        return;
    };
    let reply = route(&request);
    // Recorded before the delay, so a test sees a request whose reply never came.
    log.lock().unwrap().push(request);
    std::thread::sleep(reply.delay);
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.status,
        reason(reply.status),
        reply.body.len()
    );
    let mut stream = stream;
    // The client may have given up (a timeout test), which is not the server's failure.
    let _ = stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(reply.body.as_bytes()));
}

fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line).ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (k, v) = line.split_once(':')?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let len = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Instant;

    fn port(url: &str) -> u16 {
        url.trim_start_matches("http://127.0.0.1:")
            .split('/')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    }

    /// Send `raw` to the server at `url`, and read the whole reply (the server closes).
    fn send(url: &str, raw: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port(url))).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    fn post(url: &str, path: &str, body: &str) -> String {
        send(
            url,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        )
    }

    fn get(url: &str, path: &str) -> String {
        send(url, &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"))
    }

    #[test]
    fn answers_with_the_routes_reply_and_records_the_request() {
        let server = FakeServer::start(|_| Reply::status(418, "{\"a\":1}"));
        let out = post(&server.url, "/v1/thing", "{\"k\":2}");
        assert!(out.starts_with("HTTP/1.1 418 "), "{out}");
        assert!(out.contains("Content-Length: 7\r\n"), "{out}");
        assert!(out.ends_with("\r\n\r\n{\"a\":1}"), "{out}");
        let all = server.requests();
        assert_eq!(all.len(), 1);
        let r = &all[0];
        assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/v1/thing"));
        assert_eq!(r.header("content-LENGTH"), Some("7"));
        assert_eq!(r.body, "{\"k\":2}");
        assert_eq!(r.json()["k"], 2);
    }

    #[test]
    fn a_delayed_reply_is_recorded_at_once_and_holds_up_nothing_else() {
        let server = FakeServer::start(|r| {
            if r.path == "/slow" {
                Reply::ok("slow").after(Duration::from_secs(3))
            } else {
                Reply::ok("fast")
            }
        });
        let url = server.url.clone();
        let started = Instant::now();
        let slow = std::thread::spawn(move || get(&url, "/slow"));
        while server.requests().is_empty() {
            assert!(started.elapsed() < Duration::from_secs(2), "not recorded");
            std::thread::sleep(Duration::from_millis(5));
        }
        let fast = get(&server.url, "/fast");
        assert!(fast.ends_with("fast"), "{fast}");
        assert!(started.elapsed() < Duration::from_secs(2), "waited");
        let slow = slow.join().unwrap();
        assert!(slow.ends_with("slow"), "{slow}");
        assert!(started.elapsed() >= Duration::from_secs(3));
    }

    #[test]
    fn the_llm_server_lists_models_and_answers_completions_through_fake_llm() {
        let server = FakeServer::llm(&["m"]);
        let models = get(&server.url, "/v1/models");
        assert!(models.starts_with("HTTP/1.1 200 "), "{models}");
        let body: serde_json::Value =
            serde_json::from_str(models.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["data"][0]["id"], "m");

        let prompt = "Check these.\nL1 [0:00:01.00] (Default) We should leave befor it rains.\n";
        let request = serde_json::json!({"model": "m", "messages": [
            {"role": "system", "content": "ignored"},
            {"role": "user", "content": prompt},
        ]})
        .to_string();
        let out = post(&server.url, "/v1/chat/completions", &request);
        assert!(out.starts_with("HTTP/1.1 200 "), "{out}");
        let body: serde_json::Value =
            serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            body["choices"][0]["message"]["content"],
            "L1\tWe should leave before it rains.\tplanted typo"
        );
        // The one line's prompt, counted as subcheck estimates it, none of it cached.
        assert_eq!(body["usage"]["prompt_tokens"], llm::estimate(1));
        assert_eq!(body["usage"].get("prompt_tokens_details"), None);
        assert_eq!(server.completions().len(), 1);

        let other = get(&server.url, "/v1/nothing");
        assert!(other.starts_with("HTTP/1.1 404 "), "{other}");
    }

    #[test]
    fn a_closed_port_refuses_connections() {
        let url = closed_port_url();
        assert_eq!(url, "http://127.0.0.1:1/v1");
        assert!(TcpStream::connect(("127.0.0.1", port(&url))).is_err());
    }

    /// The JSON body of a raw HTTP reply.
    fn body_of(reply: &str) -> serde_json::Value {
        serde_json::from_str(reply.split("\r\n\r\n").nth(1).unwrap()).unwrap()
    }

    #[test]
    fn the_ollama_server_lists_its_model_loaded_only_once_asked_to_load_it() {
        let server = FakeServer::ollama("m", 8192);
        assert_eq!(
            body_of(&get(&server.url, "/v1/models"))["data"][0]["id"],
            "m"
        );
        assert_eq!(
            body_of(&get(&server.url, "/api/ps")),
            serde_json::json!({"models": []})
        );
        let other = post(&server.url, "/api/generate", r#"{"model":"x"}"#);
        assert!(other.starts_with("HTTP/1.1 404 "), "{other}");
        assert_eq!(
            body_of(&get(&server.url, "/api/ps")),
            serde_json::json!({"models": []})
        );
        let load = post(&server.url, "/api/generate", r#"{"model":"m"}"#);
        assert_eq!(body_of(&load)["done_reason"], "load");
        assert_eq!(
            body_of(&get(&server.url, "/api/ps")),
            serde_json::json!({"models": [{"name": "m", "model": "m", "context_length": 8192}]})
        );
    }

    #[test]
    fn the_ollama_server_answers_its_own_chat_for_its_model_only() {
        let server = FakeServer::ollama("m", 8192);
        let prompt = "Check these.\nL1 [0:00:01.00] (Default) We should leave befor it rains.\n";
        let chat = |model: &str| {
            serde_json::json!({"model": model, "messages": [
                {"role": "system", "content": "ignored"},
                {"role": "user", "content": prompt},
            ]})
            .to_string()
        };
        let out = post(&server.url, "/api/chat", &chat("m"));
        assert!(out.starts_with("HTTP/1.1 200 "), "{out}");
        let body = body_of(&out);
        assert_eq!(
            (
                &body["message"]["content"],
                &body["done_reason"],
                &body["prompt_eval_count"],
                &body["eval_count"]
            ),
            (
                &serde_json::json!("L1\tWe should leave before it rains.\tplanted typo"),
                &serde_json::json!("stop"),
                &serde_json::json!(llm::estimate(1)),
                &serde_json::json!(40)
            )
        );
        let other = post(&server.url, "/api/chat", &chat("x"));
        assert!(other.starts_with("HTTP/1.1 404 "), "{other}");
        assert_eq!(body_of(&other)["error"], "model 'x' not found");
        let chats = server.chats();
        assert_eq!(
            (
                chats.len(),
                prompt_lines(&chats[0]),
                server.completions().len()
            ),
            (2, 1, 0)
        );
    }

    #[test]
    fn the_sized_llm_server_lists_n_ctx_and_prompt_lines_counts_the_numbered_lines() {
        let server = FakeServer::llm_sized(&["a", "b"], 4096);
        let models = body_of(&get(&server.url, "/v1/models"));
        assert_eq!(
            (
                &models["data"][1]["id"],
                &models["data"][1]["meta"]["n_ctx"]
            ),
            (&serde_json::json!("b"), &serde_json::json!(4096))
        );
        let prompt = "Reply as L<ID>.\nL1 [0:00:01.00] (Default) One.\nLater\nL12 [0:00:02.00] (Default) Two.\n";
        let request = serde_json::json!({"model": "a", "messages": [
            {"role": "system", "content": "L1 L2 L3"},
            {"role": "user", "content": prompt},
        ]})
        .to_string();
        let out = post(&server.url, "/v1/chat/completions", &request);
        assert!(out.starts_with("HTTP/1.1 200 "), "{out}");
        assert_eq!(prompt_lines(&server.completions()[0]), 2);
    }
}
