//! A fake OpenAI-API server on 127.0.0.1, so no test needs a real model server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::fake_llm;

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
        let models = models_reply(models).after(models_delay);
        FakeServer::start(move |r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/v1/models") => models.clone(),
            ("POST", "/v1/chat/completions") => Reply::ok(complete(&last_content(r))),
            _ => Reply::status(404, "{}"),
        })
    }

    /// Every request so far, in the order they arrived.
    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }

    /// The POSTs to `/v1/chat/completions`, in the order they came.
    pub fn completions(&self) -> Vec<Request> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == "POST" && r.path == "/v1/chat/completions")
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
        assert_eq!(body["usage"]["prompt_tokens"], 1500);
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
}
