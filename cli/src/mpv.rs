//! Opening a moment in mpv. One window serves a whole review: the first `o` starts mpv with an IPC
//! socket, and every later one opens its moment in that window. The playback keys go to the
//! same socket, and `O` closes the window from there.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use subcheck_core::group;
use subcheck_core::player::{FileOption, How, Moment, Opened, PlayState, Playback, PlayerError};
use subcheck_core::text::timestamp_us;

/// mpv's error for a property with no value now: the position, or the path, with no file open.
const UNAVAILABLE: &str = "property unavailable";

/// How long a new mpv gets to open its socket.
const START_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a frame step gets to land. A step back seeks from the keyframe before it, and until it
/// has, mpv still reports the old position.
const FRAME_WAIT: Duration = Duration::from_secs(1);

/// How long the mpvs this review started get to exit once `O` asked the one on the socket to
/// quit, before they are killed.
const QUIT_WAIT: Duration = Duration::from_secs(1);

/// The app id of mpv's window on Wayland, so that a window rule can match it and no other mpv
/// (README, "Keeping focus in the terminal").
pub const APP_ID: &str = "subcheck-mpv";

/// The socket, from explicit inputs: in the runtime directory when `XDG_RUNTIME_DIR` is absolute,
/// else in the state directory (macOS has no runtime directory).
pub fn socket_path_from(runtime: Option<&Path>, state: &Path) -> PathBuf {
    match runtime.filter(|p| p.is_absolute()) {
        Some(dir) => dir.join("subcheck-mpv.sock"),
        None => state.join("mpv.sock"),
    }
}

/// The socket for this process.
pub fn socket_path(state: &Path) -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    socket_path_from(runtime.as_deref(), state)
}

/// The IPC command that loads `m` in place of whatever is playing, with review's options for the
/// file, playing there or paused. Named arguments, because mpv 0.38 put an `index` argument before
/// `options`; `options` as an object, so no value needs escaping. Every option is the file's alone,
/// set as it starts, after the file before has put back its own, a pause among them, and a user's
/// `reset-on-next-file` what it names: a pause set after the request could come before those, and
/// be undone by them.
pub fn loadfile_command(m: &Moment, review: &[FileOption], playing: bool) -> Value {
    let mut options = serde_json::Map::new();
    options.insert("start".into(), timestamp_us(m.start_us).into());
    if let Some(sid) = m.sid {
        options.insert("sid".into(), sid.to_string().into());
    }
    options.insert("sub-visibility".into(), "yes".into());
    for option in review {
        options.insert(option.name.clone(), option.value.clone().into());
    }
    options.insert("pause".into(), if playing { "no" } else { "yes" }.into());
    json!({"name": "loadfile", "url": m.file, "flags": "replace", "options": options})
}

/// What a playback key did: moved a video that plays, or, for a pause, changed how a file still
/// opening will land.
#[derive(Debug, PartialEq)]
pub enum Played {
    At(PlayState),
    Opening(Opened),
}

pub struct Player {
    program: OsString,
    socket: PathBuf,
    /// The mpvs this review started that are still up, newest last, each registered so that a
    /// signal that ends the review ends it too. More than one only when `o` found none answering
    /// while an earlier one was still up (slow to open its socket, say): it starts another, and
    /// either may end up with the socket.
    children: Vec<(Child, group::Registered)>,
    /// How many of [`focus_flags`] this mpv takes: all, until one that exited on them started with
    /// fewer.
    focus: usize,
}

impl Player {
    pub fn new(program: impl Into<OsString>, socket: PathBuf) -> Player {
        Player {
            program: program.into(),
            socket,
            children: Vec::new(),
            focus: focus_flags().len(),
        }
    }

    /// Show `m` in the mpv listening on the socket, or start one and show it there, on its frame,
    /// in the steps core's `PlayerCommand::Open` names: in the file mpv has open when `how` says
    /// seek, else loaded with `options`. Says whether mpv was left paused there. With `play` it
    /// plays there, whatever mpv's own state was: the user asked for it while the open waited.
    pub fn open(
        &mut self,
        m: &Moment,
        how: How,
        options: &[FileOption],
        play: bool,
    ) -> Result<Opened, PlayerError> {
        self.reap();
        // Whether mpv plays, read before the pause below makes every mpv paused: one started here
        // has no file to play, and one with no path has none either, whatever its pause says. It
        // decides only whether mpv plays again once there, so a read that mpv fails counts as
        // paused, which leaves mpv on the frame.
        let (mut conn, open, playing) = match Conn::connect(&self.socket) {
            // mpv refusing the read of the path, as with no file open, means no file open.
            Ok(mut conn) => {
                let open = unless_refused(conn.request(&json!(["get_property", "path"])))?;
                let playing = open.is_some()
                    && unless_refused(conn.request(&json!(["get_property", "pause"])))?
                        == Some(Value::Bool(false));
                (conn, open, playing)
            }
            Err(_) => (self.start()?, None, false),
        };
        let playing = playing || play;
        // Before mpv gets there, or a playing mpv goes on past the frame.
        conn.request(&json!(["set_property", "pause", true]))?;
        // mpv may no longer have the file review loaded (its window closed by hand, another file
        // opened in it), and refuses a seek in a file it still loads: loaded instead, whichever
        // step it refused.
        if how == How::Seek
            && open.as_ref().and_then(Value::as_str) == Some(m.file.as_str())
            && sought(&mut conn, m)?
        {
            // After the seek, so that mpv plays on from the frame, not from where it was. Refused,
            // mpv stays paused on the frame, which is still the moment opened.
            let resumed = playing
                && unless_refused(conn.request(&json!(["set_property", "pause", false])))?
                    .is_some();
            return Ok(Opened { paused: !resumed });
        }
        conn.request(&loadfile_command(m, options, playing))?;
        Ok(Opened { paused: !playing })
    }

    /// Pause or seek the mpv on the socket, and say where it is now, or, for a pause while the file
    /// is still opening, how it will land. Never starts one.
    pub fn playback(&mut self, p: Playback) -> Result<Played, PlayerError> {
        self.reap();
        let mut conn = Conn::connect(&self.socket).map_err(|_| PlayerError::NotOpen)?;
        // No position: no file open, or one still opening (seconds, from a disk that has to spin
        // up), which has its path already and refuses seeks.
        let before = match conn.request(&json!(["get_property", "time-pos"])) {
            Err(PlayerError::Mpv(e)) if e == UNAVAILABLE => {
                return match conn.request(&json!(["get_property", "path"])) {
                    // mpv keeps a pause toggled while the file opens, and lands the file as that
                    // leaves it, so the Space is its to keep and the result is how it will land.
                    Ok(_) if p == Playback::Pause => {
                        conn.request(&json!(["cycle", "pause"]))?;
                        let paused = conn.request(&json!(["get_property", "pause"]))?;
                        Ok(Played::Opening(Opened {
                            paused: paused.as_bool().unwrap_or(false),
                        }))
                    }
                    Ok(_) => Err(PlayerError::StillOpening),
                    Err(PlayerError::Mpv(e)) if e == UNAVAILABLE => Err(PlayerError::NoFile),
                    Err(e) => Err(e),
                };
            }
            Err(e) => return Err(e),
            Ok(at) => at,
        };
        conn.request(&match p {
            Playback::Pause => json!(["cycle", "pause"]),
            // Exact: a plain relative seek lands on a keyframe, up to seconds away.
            Playback::Seek(s) => json!(["seek", s, "relative+exact"]),
            Playback::NextFrame => json!(["frame-step"]),
            Playback::PreviousFrame => json!(["frame-back-step"]),
        })?;
        let paused = conn.request(&json!(["get_property", "pause"]))?;
        // Read straight after an exact seek or a step on, this is already the new position; after
        // a step back, only once mpv has sought.
        let mut secs = conn.request(&json!(["get_property", "time-pos"]))?;
        if p == Playback::PreviousFrame {
            let deadline = Instant::now() + FRAME_WAIT;
            while secs == before && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
                secs = conn.request(&json!(["get_property", "time-pos"]))?;
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        let at_ms = (secs.as_f64().unwrap_or(0.0) * 1000.0).round() as i64;
        Ok(Played::At(PlayState {
            paused: paused.as_bool().unwrap_or(false),
            at_ms,
        }))
    }

    /// Close mpv (`O`), as its window takes no keys. The one on the socket is asked to quit,
    /// whether this review started it or found it open. Each one this review started that is still
    /// up is then killed: after [`QUIT_WAIT`] when one was asked, at once when none answered. That
    /// covers one the socket does not reach, and one that does not quit, stuck reading over NFS
    /// say. The next `o` starts another.
    pub fn close(&mut self) -> Result<(), PlayerError> {
        self.reap();
        // An mpv that hangs up before it answers `quit` has quit too.
        let quit =
            Conn::connect(&self.socket)
                .ok()
                .map(|mut conn| match conn.request(&json!(["quit"])) {
                    Err(PlayerError::HungUp) => Ok(()),
                    asked => asked.map(drop),
                });
        if quit.is_none() && self.children.is_empty() {
            return Err(PlayerError::NotOpen);
        }
        let wait = if quit.is_some() {
            QUIT_WAIT
        } else {
            Duration::ZERO
        };
        let deadline = Instant::now() + wait;
        while !self.children.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            self.reap();
        }
        for (mut child, _running) in self.children.drain(..) {
            group::kill(&child);
            let _ = child.wait();
        }
        quit.unwrap_or(Ok(()))
    }

    /// Forget the started mpvs that have exited (and collect their exit status).
    fn reap(&mut self) {
        self.children
            .retain_mut(|(child, _)| matches!(child.try_wait(), Ok(None)));
    }

    /// Start an idle mpv on the socket and connect to it. A stale socket file left by an mpv that
    /// died is no obstacle: mpv replaces it.
    fn start(&mut self) -> Result<Conn, PlayerError> {
        let mut focus = self.focus;
        loop {
            match self.launch(focus) {
                Ok(conn) => {
                    self.focus = focus;
                    return Ok(conn);
                }
                // mpv exits at once on an option it does not know, and an older one lacks
                // --focus-on, older still --wayland-app-id: one fewer, down to none. What worked
                // is kept for the next start.
                Err(Start::Exited(_)) if focus > 0 => focus -= 1,
                Err(Start::Exited(e) | Start::Failed(e)) => return Err(PlayerError::Failed(e)),
            }
        }
    }

    /// Start mpv with the first `focus` of [`focus_flags`], and connect to it.
    fn launch(&mut self, focus: usize) -> Result<Conn, Start> {
        if let Some(dir) = self.socket.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| Start::Failed(format!("creating {}: {e}", dir.display())))?;
        }
        let mut server = OsString::from("--input-ipc-server=");
        server.push(&self.socket);
        // In a process group of its own: the terminal's signals stay with the review, which closes
        // the window itself.
        let (child, running) = group::spawn(
            Command::new(&self.program)
                .arg(server)
                .args(["--idle=yes", "--force-window=yes", "--no-terminal"])
                .args(&focus_flags()[..focus])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .map_err(|e| {
            let program = self.program.to_string_lossy();
            Start::Failed(if e.kind() == io::ErrorKind::NotFound {
                format!("{program} not found: install mpv to open findings")
            } else {
                format!("starting {program}: {e}")
            })
        })?;
        self.children.push((child, running));
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if let Ok(conn) = Conn::connect(&self.socket) {
                return Ok(conn);
            }
            if let Some((child, _)) = self.children.last_mut()
                && let Ok(Some(status)) = child.try_wait()
            {
                self.children.pop();
                let e = format!("mpv exited before opening its socket ({status})");
                // 1 is mpv's exit on an option it does not know; a launcher that exits 0 or a
                // crash is no reason to drop a flag.
                return Err(if status.code() == Some(1) {
                    Start::Exited(e)
                } else {
                    Start::Failed(e)
                });
            }
            if Instant::now() >= deadline {
                return Err(Start::Failed(format!(
                    "mpv did not open {} within {} s",
                    self.socket.display(),
                    START_TIMEOUT.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Every mpv this review started that is still up closes with it: the window takes no keys, so it
/// has no `q` of its own (review's `O` closes it sooner). Killed, as `group` does when a signal
/// ends the review (the terminal closed): mpv without a terminal has no handler for SIGTERM
/// either. One the review found open on the socket is left alone.
impl Drop for Player {
    fn drop(&mut self) {
        self.reap();
        for (child, _) in &self.children {
            group::kill(child);
        }
    }
}

/// A seek's steps, in core's order: the subtitle track and subtitles on, then the exact seek.
/// Whether mpv took each: a refusal is its word to load instead, where no answer fails the open.
fn sought(conn: &mut Conn, m: &Moment) -> Result<bool, PlayerError> {
    let mut steps = Vec::new();
    if let Some(sid) = m.sid {
        steps.push(json!(["set_property", "sid", sid]));
    }
    steps.push(json!(["set_property", "sub-visibility", true]));
    steps.push(json!(["seek", timestamp_us(m.start_us), "absolute+exact"]));
    for step in &steps {
        if unless_refused(conn.request(step))?.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// mpv's answer, or none when mpv itself refused the request, in its own words: a refusal is an
/// answer, which each caller reads as its step's rule says. No answer at all, a reply that names
/// no error, or mpv gone, fails the command.
fn unless_refused(reply: Result<Value, PlayerError>) -> Result<Option<Value>, PlayerError> {
    match reply {
        Ok(value) => Ok(Some(value)),
        Err(PlayerError::Mpv(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The flags that keep focus in the terminal, on macOS by themselves and on KDE with the README's
/// rule: the app id the rule matches, and no focus for a new window. Newest last (mpv 0.38 added
/// `--focus-on`, and had `--wayland-app-id` on every platform already), so an older mpv can be
/// given the first of them only.
fn focus_flags() -> [String; 2] {
    [
        format!("--wayland-app-id={APP_ID}"),
        "--focus-on=never".into(),
    ]
}

/// Why a started mpv gave no socket.
enum Start {
    /// It exited first with status 1, as mpv does on an option it does not know.
    Exited(String),
    Failed(String),
}

/// One connection to mpv: each command is answered before the next is sent.
struct Conn {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
}

impl Conn {
    fn connect(socket: &Path) -> io::Result<Conn> {
        let stream = UnixStream::connect(socket)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Conn {
            stream,
            reader,
            next_id: 1,
        })
    }

    /// Send `command` and wait for mpv's reply to it: its `data`, or its error. Event lines may
    /// come first. A reply that does not come within the socket's read timeout is no answer. Only
    /// an error mpv words is [`PlayerError::Mpv`], which a step may take as an answer; a reply
    /// that names none is a failure, as mpv refused nothing.
    fn request(&mut self, command: &Value) -> Result<Value, PlayerError> {
        let fail = |e: io::Error| match e.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => PlayerError::NoAnswer,
            _ => PlayerError::Failed(format!("talking to mpv: {e}")),
        };
        let id = self.next_id;
        self.next_id += 1;
        let mut line = json!({"command": command, "request_id": id}).to_string();
        line.push('\n');
        self.stream.write_all(line.as_bytes()).map_err(fail)?;
        let mut reply = String::new();
        loop {
            reply.clear();
            if self.reader.read_line(&mut reply).map_err(fail)? == 0 {
                return Err(PlayerError::HungUp);
            }
            let Ok(v) = serde_json::from_str::<Value>(&reply) else {
                continue;
            };
            if v.get("request_id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            return match v.get("error").and_then(Value::as_str) {
                Some("success") => Ok(v.get("data").cloned().unwrap_or(Value::Null)),
                Some(error) => Err(PlayerError::Mpv(error.into())),
                None => Err(PlayerError::Failed(format!(
                    "mpv: unexpected reply {reply}"
                ))),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread;
    use subcheck_core::test_support::script;

    fn moment() -> Moment {
        Moment {
            file: "/m/K-On! - E02.mkv".into(),
            start_us: 165_064_500,
            sid: Some(2),
        }
    }

    fn ok(data: impl Into<Value>) -> Value {
        let data: Value = data.into();
        json!({"data": data, "error": "success"})
    }

    fn error(e: &str) -> Value {
        json!({"error": e})
    }

    /// The words of a failure to start mpv, or to reach it.
    fn failed(opened: Result<Opened, PlayerError>) -> String {
        match opened {
            Err(PlayerError::Failed(e)) => e,
            other => panic!("expected a failure: {other:?}"),
        }
    }

    /// An mpv stand-in: on one connection to `listener`, answers each command with `answer`'s
    /// reply (after an event line, as mpv sends them unasked), and returns the commands it was
    /// sent once the connection closes.
    fn fake_mpv(
        listener: UnixListener,
        answer: impl Fn(&Value) -> Value + Send + 'static,
    ) -> thread::JoinHandle<Vec<Value>> {
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut sent = Vec::new();
            for line in BufReader::new(&stream).lines() {
                let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
                let mut reply = answer(&request["command"]);
                reply["request_id"] = request["request_id"].clone();
                let mut w = &stream;
                writeln!(w, "{{\"event\":\"idle\"}}\n{reply}").unwrap();
                sent.push(request["command"].clone());
            }
            sent
        })
    }

    /// An idle mpv: nothing open, every other command succeeds.
    fn idle(command: &Value) -> Value {
        if command[1] == "path" || command[1] == "time-pos" {
            error("property unavailable")
        } else {
            ok(Value::Null)
        }
    }

    /// A script standing in for the mpv program: it writes its arguments to `argv` and stays
    /// up, as mpv would, until the Player that started it is dropped.
    fn fake_program(dir: &Path) -> (PathBuf, PathBuf) {
        let argv = dir.join("argv");
        let program = script(
            dir,
            "fake-mpv",
            &format!(
                "printf '%s\\n' \"$@\" > '{0}.tmp'\nmv '{0}.tmp' '{0}'\nsleep 60\n",
                argv.display()
            ),
        );
        (program, argv)
    }

    fn pause() -> Value {
        json!(["set_property", "pause", true])
    }

    fn unpause() -> Value {
        json!(["set_property", "pause", false])
    }

    fn read(property: &str) -> Value {
        json!(["get_property", property])
    }

    #[test]
    fn the_socket_lives_in_the_runtime_dir_when_there_is_one() {
        let state = Path::new("/home/u/.local/state/subcheck");
        assert_eq!(
            socket_path_from(Some(Path::new("/run/user/1000")), state),
            Path::new("/run/user/1000/subcheck-mpv.sock")
        );
        assert_eq!(
            socket_path_from(Some(Path::new("relative")), state),
            state.join("mpv.sock")
        );
        assert_eq!(socket_path_from(None, state), state.join("mpv.sock"));
    }

    // Options core never gives: the player hands mpv whatever list review gives it, and names
    // none itself, which core's own pair would hide.
    fn given() -> Vec<FileOption> {
        [("title", "t"), ("osd-level", "1")]
            .into_iter()
            .map(|(name, value)| FileOption {
                name: name.into(),
                value: value.into(),
            })
            .collect()
    }

    // Every option is the file's alone, set as it starts: each of review's for the load, and the
    // pause as mpv is to end up.
    #[test]
    fn loadfile_names_its_arguments_and_starts_on_the_track() {
        assert_eq!(
            loadfile_command(&moment(), &given(), false),
            json!({
                "name": "loadfile",
                "url": "/m/K-On! - E02.mkv",
                "flags": "replace",
                "options": {
                    "start": "0:02:45.064500",
                    "sid": "2",
                    "sub-visibility": "yes",
                    "title": "t",
                    "osd-level": "1",
                    "pause": "yes"
                }
            })
        );
        assert_eq!(
            loadfile_command(&moment(), &given(), true)["options"]["pause"],
            "no"
        );
        let no_sid = Moment {
            sid: None,
            ..moment()
        };
        assert!(
            loadfile_command(&no_sid, &given(), false)["options"]
                .get("sid")
                .is_none()
        );
    }

    /// mpv left paused, and left playing.
    const PAUSED: Result<Opened, PlayerError> = Ok(Opened { paused: true });
    const PLAYING: Result<Opened, PlayerError> = Ok(Opened { paused: false });

    // mpv up with no file open: paused before the load, and loaded paused, as nothing played.
    // Told to seek, as when mpv was closed by hand since review loaded the file, it loads too. A
    // read of the path that mpv refuses in other words is no file open too, as in the window.
    #[test]
    fn with_no_file_open_a_moment_is_loaded_paused_to_the_microsecond() {
        for (how, refused) in [
            (How::Load, UNAVAILABLE),
            (How::Seek, UNAVAILABLE),
            (How::Seek, "unknown error"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("mpv.sock");
            let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), move |c| {
                if *c == read("path") {
                    error(refused)
                } else {
                    idle(c)
                }
            });
            let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
            assert_eq!(player.open(&moment(), how, &given(), false), PAUSED);
            assert_eq!(
                mpv.join().unwrap(),
                [
                    read("path"),
                    pause(),
                    loadfile_command(&moment(), &given(), false)
                ]
            );
        }
    }

    /// What `o` on `moment()` sends an mpv with its file open, told to seek, as far as the seek:
    /// whether mpv plays, read before anything is sent, then the pause, the subtitle track and
    /// subtitles on, and the seek.
    fn sought() -> Vec<Value> {
        vec![
            read("path"),
            read("pause"),
            pause(),
            json!(["set_property", "sid", 2]),
            json!(["set_property", "sub-visibility", true]),
            json!(["seek", "0:02:45.064500", "absolute+exact"]),
        ]
    }

    // An Open that says play lands playing, whatever mpv's own state was: by the load's option
    // when it loads, by an unpause after the seek when it seeks.
    #[test]
    fn an_open_that_says_play_lands_playing() {
        let run = |how: How, answer: Box<dyn Fn(&Value) -> Value + Send>| {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("mpv.sock");
            let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), answer);
            let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
            let opened = player.open(&moment(), how, &given(), true);
            (opened, mpv.join().unwrap())
        };
        let (opened, sent) = run(How::Load, Box::new(idle));
        assert_eq!(opened, PLAYING);
        assert_eq!(
            sent,
            [
                read("path"),
                pause(),
                loadfile_command(&moment(), &given(), true)
            ]
        );
        assert_eq!(sent[2]["options"]["pause"], "no");
        let (opened, sent) = run(
            How::Load,
            Box::new(has_open("/m/K-On! - E01.mkv", true, ok(Value::Null))),
        );
        assert_eq!(opened, PLAYING);
        assert_eq!(
            sent,
            [
                read("path"),
                read("pause"),
                pause(),
                loadfile_command(&moment(), &given(), true)
            ]
        );
        assert_eq!(sent[3]["options"]["pause"], "no");
        let (opened, sent) = run(
            How::Seek,
            Box::new(has_open(&moment().file, true, ok(Value::Null))),
        );
        assert_eq!(opened, PLAYING);
        let mut want = sought();
        want.push(unpause());
        assert_eq!(sent, want);
    }

    // `o` again, on the line mpv shows or another of the file, while mpv is paused: sought, and
    // left paused.
    #[test]
    fn a_paused_mpv_is_sought_in_the_file_it_has_open_and_left_paused() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let mpv = fake_mpv(
            UnixListener::bind(&socket).unwrap(),
            has_open(&moment().file, true, ok(Value::Null)),
        );
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        assert_eq!(player.open(&moment(), How::Seek, &given(), false), PAUSED);
        assert_eq!(mpv.join().unwrap(), sought());
    }

    // While mpv plays: paused before the seek, so that it does not go on past the frame meanwhile,
    // and playing again once it is there, as the last request.
    #[test]
    fn a_playing_mpv_is_sought_in_the_file_it_has_open_and_plays_on() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let mpv = fake_mpv(
            UnixListener::bind(&socket).unwrap(),
            has_open(&moment().file, false, ok(Value::Null)),
        );
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        assert_eq!(player.open(&moment(), How::Seek, &given(), false), PLAYING);
        let mut want = sought();
        want.push(unpause());
        assert_eq!(mpv.join().unwrap(), want);
    }

    // Another file while mpv plays: paused, then loaded playing, by the load's own option, and
    // nothing after it.
    #[test]
    fn a_playing_mpv_loads_another_file_and_plays_on() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let mpv = fake_mpv(
            UnixListener::bind(&socket).unwrap(),
            has_open("/m/K-On! - E01.mkv", false, ok(Value::Null)),
        );
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PLAYING);
        assert_eq!(
            mpv.join().unwrap(),
            [
                read("path"),
                read("pause"),
                pause(),
                loadfile_command(&moment(), &given(), true)
            ]
        );
    }

    // Told to seek, but mpv has another file open (opened in mpv's own window, say): loaded.
    #[test]
    fn a_seek_in_a_file_mpv_no_longer_has_open_is_a_load() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let mpv = fake_mpv(
            UnixListener::bind(&socket).unwrap(),
            has_open("/m/K-On! - E01.mkv", true, ok(Value::Null)),
        );
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        assert_eq!(player.open(&moment(), How::Seek, &given(), false), PAUSED);
        assert_eq!(
            mpv.join().unwrap(),
            [
                read("path"),
                read("pause"),
                pause(),
                loadfile_command(&moment(), &given(), false)
            ]
        );
    }

    // Told to load the file mpv has open, as review is once it changed on disk (a remux in place
    // keeps the path, and after a fix of one letter even the size): loaded again, as a seek would
    // stay in the old file and show its subtitles.
    #[test]
    fn a_file_mpv_has_open_is_loaded_again_when_review_says_load() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let mpv = fake_mpv(
            UnixListener::bind(&socket).unwrap(),
            has_open(&moment().file, true, ok(Value::Null)),
        );
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
        assert_eq!(
            mpv.join().unwrap(),
            [
                read("path"),
                read("pause"),
                pause(),
                loadfile_command(&moment(), &given(), false)
            ]
        );
    }

    // Whether mpv plays decides only whether it plays again once there: a read of it that mpv
    // fails counts as paused, and the moment opens, left paused on its frame.
    #[test]
    fn a_failed_read_of_pause_opens_the_moment_paused() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), |c| {
            match (c[0].as_str(), c[1].as_str()) {
                (Some("get_property"), Some("path")) => ok("/m/K-On! - E01.mkv"),
                (Some("get_property"), Some("pause")) => error("unknown error"),
                _ => ok(Value::Null),
            }
        });
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
        assert_eq!(
            mpv.join().unwrap(),
            [
                read("path"),
                read("pause"),
                pause(),
                loadfile_command(&moment(), &given(), false)
            ]
        );
    }

    // As a failed read does: a pause that is no flag says nothing of mpv playing.
    #[test]
    fn a_pause_that_is_no_flag_opens_the_moment_paused() {
        for paused in [Value::Null, json!("no")] {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("mpv.sock");
            let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), move |c| {
                match (c[0].as_str(), c[1].as_str()) {
                    (Some("get_property"), Some("path")) => ok("/m/K-On! - E01.mkv"),
                    (Some("get_property"), Some("pause")) => ok(paused.clone()),
                    _ => ok(Value::Null),
                }
            });
            let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
            assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
            assert_eq!(
                mpv.join().unwrap(),
                [
                    read("path"),
                    read("pause"),
                    pause(),
                    loadfile_command(&moment(), &given(), false)
                ]
            );
        }
    }

    // Once mpv has been sent to the moment, a refused unpause leaves it paused on the frame, which
    // is still the moment opened, and the open says it is paused.
    #[test]
    fn a_refused_unpause_still_opens_the_moment_paused() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let playing = has_open(&moment().file, false, ok(Value::Null));
        let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), move |c| {
            if *c == unpause() {
                error("unknown error")
            } else {
                playing(c)
            }
        });
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        assert_eq!(player.open(&moment(), How::Seek, &given(), false), PAUSED);
        let mut want = sought();
        want.push(unpause());
        assert_eq!(mpv.join().unwrap(), want);
    }

    // An mpv gone before it answers the unpause is no refusal, so the open fails, as it would on
    // any other request.
    #[test]
    fn an_mpv_gone_before_the_unpause_fails_the_open() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let playing = has_open(&moment().file, false, ok(Value::Null));
        let mpv = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut sent = Vec::new();
            for line in BufReader::new(&stream).lines() {
                let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
                sent.push(request["command"].clone());
                if request["command"] == unpause() {
                    break;
                }
                let mut reply = playing(&request["command"]);
                reply["request_id"] = request["request_id"].clone();
                writeln!(&stream, "{reply}").unwrap();
            }
            sent
        });
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        assert_eq!(
            player.open(&moment(), How::Seek, &given(), false),
            Err(PlayerError::HungUp)
        );
        assert_eq!(mpv.join().unwrap().last(), Some(&unpause()));
    }

    #[test]
    fn a_running_mpv_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), idle);
        // A program that cannot start: reusing the window must not try.
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
        assert_eq!(
            mpv.join().unwrap(),
            [
                read("path"),
                pause(),
                loadfile_command(&moment(), &given(), false)
            ]
        );
    }

    /// An mpv with `file` open, paused or playing; `seek` is its answer to a seek.
    fn has_open(
        file: &str,
        paused: bool,
        seek: Value,
    ) -> impl Fn(&Value) -> Value + Send + 'static + use<> {
        let file = file.to_string();
        move |c| match (c[0].as_str(), c[1].as_str()) {
            (Some("get_property"), Some("path")) => ok(file.clone()),
            (Some("get_property"), Some("pause")) => ok(paused),
            (Some("seek"), _) => seek.clone(),
            _ => ok(Value::Null),
        }
    }

    // A seek mpv refuses, as in a file it still loads, loads the file instead, which plays on as
    // mpv played.
    #[test]
    fn a_file_that_cannot_seek_yet_is_loaded_again_and_plays_on() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let mpv = fake_mpv(
            UnixListener::bind(&socket).unwrap(),
            has_open(&moment().file, false, error("error running command")),
        );
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        assert_eq!(player.open(&moment(), How::Seek, &given(), false), PLAYING);
        let mut want = sought();
        want.push(loadfile_command(&moment(), &given(), true));
        assert_eq!(mpv.join().unwrap(), want);
    }

    // Any step of the seek refused, the subtitle track as much as the seek, loads instead, as the
    // window's player does.
    #[test]
    fn a_refused_setting_of_the_seek_loads_the_file_instead() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let paused = has_open(&moment().file, true, ok(Value::Null));
        let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), move |c| {
            if c[1] == "sid" {
                error("property not found")
            } else {
                paused(c)
            }
        });
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        assert_eq!(player.open(&moment(), How::Seek, &given(), false), PAUSED);
        let mut want = sought()[..4].to_vec();
        want.push(loadfile_command(&moment(), &given(), false));
        assert_eq!(mpv.join().unwrap(), want);
    }

    // An mpv that takes a request and never answers it: the socket's read time, then the open
    // says so, rather than hold the terminal for good.
    #[test]
    fn an_mpv_that_does_not_answer_fails_the_open_in_time() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        // Each request taken, none answered, until the player hangs up.
        let mpv = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            BufReader::new(&stream).lines().count()
        });
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        let asked = Instant::now();
        assert_eq!(
            player.open(&moment(), How::Load, &given(), false),
            Err(PlayerError::NoAnswer)
        );
        assert!(asked.elapsed() >= Duration::from_secs(2));
        assert_eq!(mpv.join().unwrap(), 1, "the read of the path alone");
    }

    /// Stands in for the started mpv: an idle one on `socket`, once the program has written its
    /// `argv`.
    fn mpv_once_started(socket: &Path, argv: &Path) -> thread::JoinHandle<Vec<Value>> {
        let (socket, argv) = (socket.to_path_buf(), argv.to_path_buf());
        thread::spawn(move || {
            while !argv.exists() {
                thread::sleep(Duration::from_millis(10));
            }
            fake_mpv(UnixListener::bind(&socket).unwrap(), idle)
                .join()
                .unwrap()
        })
    }

    /// A stand-in for an older mpv: it counts its starts in `starts`, exits on an option that
    /// matches `unknown` (a `case` pattern) as mpv does on one it does not know, and else writes
    /// its arguments to `argv` and stays up for a while.
    fn older_mpv(dir: &Path, unknown: &str) -> (PathBuf, PathBuf, PathBuf) {
        let (argv, starts) = (dir.join("argv"), dir.join("starts"));
        let program = script(
            dir,
            "older-mpv",
            &format!(
                "echo >> '{starts}'\nfor a; do case \"$a\" in {unknown}) exit 1;; esac; done\n\
                 printf '%s\\n' \"$@\" > '{argv}.tmp'\nmv '{argv}.tmp' '{argv}'\nsleep 2\n",
                starts = starts.display(),
                argv = argv.display()
            ),
        );
        (program, argv, starts)
    }

    /// The arguments every mpv this review starts gets, before the focus flags.
    fn base_args(socket: &Path) -> String {
        format!(
            "--input-ipc-server={}\n--idle=yes\n--force-window=yes\n--no-terminal\n",
            socket.display()
        )
    }

    #[test]
    fn an_mpv_from_before_focus_on_is_started_again_with_the_app_id_alone() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let (program, argv, _) = older_mpv(dir.path(), "--focus-on=*");
        let server = mpv_once_started(&socket, &argv);
        let mut player = Player::new(&program, socket.clone());
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
        server.join().unwrap();
        // The app id stays, so the KWin rule still matches the window.
        assert_eq!(
            std::fs::read_to_string(&argv).unwrap(),
            format!("{}--wayland-app-id=subcheck-mpv\n", base_args(&socket))
        );
    }

    #[test]
    fn an_mpv_without_either_flag_is_started_with_neither_from_then_on() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let (program, argv, starts) = older_mpv(dir.path(), "--focus-on=*|--wayland-app-id=*");
        let server = mpv_once_started(&socket, &argv);
        let mut player = Player::new(&program, socket.clone());
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
        server.join().unwrap();
        assert_eq!(std::fs::read_to_string(&argv).unwrap(), base_args(&socket));
        let count = || std::fs::read_to_string(&starts).unwrap().lines().count();
        assert_eq!(count(), 3, "both flags, the app id, neither");
        // Its socket gone (as if that mpv had quit), the next o starts another straight away
        // without them.
        std::fs::remove_file(&socket).unwrap();
        std::fs::remove_file(&argv).unwrap();
        let server = mpv_once_started(&socket, &argv);
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
        server.join().unwrap();
        assert_eq!(count(), 4);
    }

    #[test]
    fn only_an_exit_on_an_option_starts_mpv_again() {
        let dir = tempfile::tempdir().unwrap();
        let starts = dir.path().join("starts");
        // A launcher that puts mpv in the background and exits 0: starting it again would only
        // start more of them.
        let program = script(
            dir.path(),
            "launcher",
            &format!("echo >> '{}'\nexit 0\n", starts.display()),
        );
        let mut player = Player::new(&program, dir.path().join("mpv.sock"));
        let err = failed(player.open(&moment(), How::Load, &given(), false));
        assert!(err.contains("exited before opening its socket"), "{err}");
        assert_eq!(std::fs::read_to_string(&starts).unwrap().lines().count(), 1);
    }

    #[test]
    fn mpv_installed_after_a_first_o_failed_still_gets_the_focus_flags() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let mut player = Player::new(dir.path().join("fake-mpv"), socket.clone());
        let err = failed(player.open(&moment(), How::Load, &given(), false));
        assert!(err.contains("not found"), "{err}");
        let (_, argv) = fake_program(dir.path());
        let server = mpv_once_started(&socket, &argv);
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
        server.join().unwrap();
        let argv = std::fs::read_to_string(&argv).unwrap();
        assert!(argv.ends_with("--focus-on=never\n"), "{argv}");
    }

    // An mpv started here has no file open: nothing is read from it first, and it stays paused.
    #[test]
    fn with_no_mpv_running_one_is_started_and_loads_paused() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("sock/mpv.sock");
        let (program, argv) = fake_program(dir.path());
        let server = mpv_once_started(&socket, &argv);
        let mut player = Player::new(&program, socket.clone());
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
        assert_eq!(
            server.join().unwrap(),
            [pause(), loadfile_command(&moment(), &given(), false)]
        );
        assert_eq!(
            std::fs::read_to_string(&argv).unwrap(),
            format!(
                "--input-ipc-server={}\n--idle=yes\n--force-window=yes\n--no-terminal\n\
                 --wayland-app-id=subcheck-mpv\n--focus-on=never\n",
                socket.display()
            )
        );
    }

    /// The pid of the newest mpv `player` started.
    fn newest(player: &Player) -> u32 {
        player.children.last().expect("an mpv started").0.id()
    }

    /// Whether the started mpv `pid` was killed, waiting up to 2 s for it to end. A dropped Player
    /// never waits for one it killed, so it is still there to wait for; one still up is killed
    /// here, so that no test leaves it behind.
    fn killed(pid: u32) -> bool {
        use rustix::io::Errno;
        use rustix::process::{
            Pid, Signal, WaitOptions, kill_process, kill_process_group, waitpid,
        };
        let pid = Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            match waitpid(Some(pid), WaitOptions::NOHANG) {
                Ok(Some((_, status))) => break status,
                Ok(None) | Err(Errno::INTR) => {}
                // Reaped already, or gone: no kill to tell of, and none to make (its pid may be
                // another process's by now).
                Err(_) => return false,
            }
            if Instant::now() >= deadline {
                // Its group, or it alone should it lead none; waited for only once killed, as a
                // failed kill would leave the wait hanging.
                let killed = kill_process_group(pid, Signal::KILL)
                    .or_else(|_| kill_process(pid, Signal::KILL));
                if killed.is_ok() {
                    let _ = waitpid(Some(pid), WaitOptions::empty());
                }
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        };
        status.terminating_signal() == Some(Signal::KILL.as_raw())
    }

    #[test]
    fn the_mpv_a_review_started_closes_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let (program, argv) = fake_program(dir.path());
        let server = mpv_once_started(&socket, &argv);
        let mut player = Player::new(&program, socket);
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
        server.join().unwrap();
        let pid = newest(&player);
        drop(player);
        assert!(killed(pid));
    }

    #[test]
    fn an_mpv_out_of_reach_closes_with_the_review_too() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let (program, argv) = fake_program(dir.path());
        let server = mpv_once_started(&socket, &argv);
        let mut player = Player::new(&program, socket.clone());
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
        server.join().unwrap();
        let first = newest(&player);
        // Its socket file gone, the next o cannot reach it and starts another.
        std::fs::remove_file(&socket).unwrap();
        std::fs::remove_file(&argv).unwrap();
        let server = mpv_once_started(&socket, &argv);
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
        server.join().unwrap();
        let second = newest(&player);
        assert_ne!(second, first);
        drop(player);
        assert!(killed(first));
        assert!(killed(second));
    }

    #[test]
    fn a_missing_program_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut player = Player::new(dir.path().join("no-such-mpv"), dir.path().join("mpv.sock"));
        let err = failed(player.open(&moment(), How::Load, &given(), false));
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn an_mpv_that_exits_without_a_socket_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "false-mpv", "exit 3\n");
        let mut player = Player::new(&program, dir.path().join("mpv.sock"));
        let err = failed(player.open(&moment(), How::Load, &given(), false));
        assert!(err.contains("exited before opening its socket"), "{err}");
    }

    #[test]
    fn an_mpv_error_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), |c| {
            if c["name"] == "loadfile" {
                error("invalid parameter")
            } else {
                idle(c)
            }
        });
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        assert_eq!(
            player.open(&moment(), How::Load, &given(), false),
            Err(PlayerError::Mpv("invalid parameter".into()))
        );
        mpv.join().unwrap();

        // A reply that names no error is no refusal of mpv's, which a read of the path would take
        // as no file open: the open fails there, and nothing is loaded.
        let socket = dir.path().join("mpv-2.sock");
        let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), |c| {
            if *c == read("path") {
                json!({"data": moment().file})
            } else {
                ok(Value::Null)
            }
        });
        let mut player = Player::new(dir.path().join("no-such-mpv"), socket);
        let err = failed(player.open(&moment(), How::Load, &given(), false));
        assert!(err.starts_with("mpv: unexpected reply"), "{err}");
        assert_eq!(mpv.join().unwrap(), [read("path")]);
    }

    #[test]
    fn playback_keys_send_their_command_and_read_back_where_mpv_is() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let playing = |c: &Value| match c[1].as_str() {
            Some("path") => ok(moment().file),
            Some("pause") if c[0] == "get_property" => ok(true),
            Some("time-pos") => ok(171.06),
            _ => ok(Value::Null),
        };
        let mut player = Player::new("mpv", socket.clone());
        let listener = UnixListener::bind(&socket).unwrap();
        let mpv = fake_mpv(listener.try_clone().unwrap(), playing);
        assert_eq!(
            player.playback(Playback::Pause),
            Ok(Played::At(PlayState {
                paused: true,
                at_ms: 171_060
            }))
        );
        assert_eq!(
            mpv.join().unwrap(),
            [
                json!(["get_property", "time-pos"]),
                json!(["cycle", "pause"]),
                json!(["get_property", "pause"]),
                json!(["get_property", "time-pos"])
            ]
        );
        let mpv = fake_mpv(listener, playing);
        player.playback(Playback::Seek(-5)).unwrap();
        assert_eq!(
            mpv.join().unwrap()[1],
            json!(["seek", -5, "relative+exact"])
        );
    }

    #[test]
    fn a_step_back_a_frame_reads_the_position_once_mpv_has_sought() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        // The old position twice more after the step, as mpv seeks from the keyframe before.
        let reads = std::sync::atomic::AtomicUsize::new(0);
        let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), move |c| {
            match c[1].as_str() {
                Some("time-pos") => {
                    let n = reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    ok(if n < 3 { 763.763 } else { 763.721 })
                }
                Some("pause") if c[0] == "get_property" => ok(true),
                _ => ok(Value::Null),
            }
        });
        let mut player = Player::new("mpv", socket);
        assert_eq!(
            player.playback(Playback::PreviousFrame),
            Ok(Played::At(PlayState {
                paused: true,
                at_ms: 763_721
            }))
        );
        let sent = mpv.join().unwrap();
        assert_eq!(sent[1], json!(["frame-back-step"]));
        assert_eq!(sent.len(), 6, "{sent:?}");
    }

    #[test]
    fn playback_never_starts_mpv_and_needs_a_file_open() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let (program, argv) = fake_program(dir.path());
        let mut player = Player::new(&program, socket.clone());
        assert_eq!(player.playback(Playback::Pause), Err(PlayerError::NotOpen));
        assert!(!argv.exists(), "no mpv started");
        let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), idle);
        assert_eq!(player.playback(Playback::Seek(5)), Err(PlayerError::NoFile));
        assert_eq!(
            mpv.join().unwrap(),
            [
                json!(["get_property", "time-pos"]),
                json!(["get_property", "path"])
            ]
        );
    }

    // A file from a disk that has to spin up: mpv names it but has no position yet. mpv keeps a
    // toggle made then, so a Space is how the file will land; a seek has nowhere to go.
    #[test]
    fn a_space_for_a_file_still_opening_goes_to_mpv_and_a_seek_still_waits() {
        for paused in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("mpv.sock");
            let opening = move |c: &Value| match (c[0].as_str(), c[1].as_str()) {
                (Some("get_property"), Some("path")) => ok(moment().file),
                (Some("get_property"), Some("time-pos")) => error("property unavailable"),
                (Some("get_property"), Some("pause")) => ok(paused),
                _ => ok(Value::Null),
            };
            let mut player = Player::new("mpv", socket.clone());
            let listener = UnixListener::bind(&socket).unwrap();
            let mpv = fake_mpv(listener.try_clone().unwrap(), opening);
            assert_eq!(
                player.playback(Playback::Pause),
                Ok(Played::Opening(Opened { paused }))
            );
            assert_eq!(
                mpv.join().unwrap(),
                [
                    read("time-pos"),
                    read("path"),
                    json!(["cycle", "pause"]),
                    read("pause")
                ]
            );
            let mpv = fake_mpv(listener, opening);
            assert_eq!(
                player.playback(Playback::Seek(5)),
                Err(PlayerError::StillOpening)
            );
            assert_eq!(mpv.join().unwrap(), [read("time-pos"), read("path")]);
        }
    }

    #[test]
    fn another_error_asking_for_the_file_is_reported_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), |c| {
            match c[1].as_str() {
                Some("time-pos") => error("property unavailable"),
                _ => error("invalid parameter"),
            }
        });
        let mut player = Player::new("mpv", socket);
        assert_eq!(
            player.playback(Playback::Pause),
            Err(PlayerError::Mpv("invalid parameter".into()))
        );
        mpv.join().unwrap();
    }

    /// Whether no process `pid` is left: killed, and waited for.
    fn gone(pid: u32) -> bool {
        let pid = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
        rustix::process::test_kill_process(pid).is_err()
    }

    #[test]
    fn close_asks_the_mpv_on_the_socket_to_quit() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        // One this review did not start: found open, and closed all the same.
        let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), idle);
        let mut player = Player::new("mpv", socket);
        assert_eq!(player.close(), Ok(()));
        assert_eq!(mpv.join().unwrap(), [json!(["quit"])]);
    }

    #[test]
    fn an_mpv_that_hangs_up_on_quit_is_closed() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let mpv = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            line
        });
        let mut player = Player::new("mpv", socket);
        assert_eq!(player.close(), Ok(()));
        assert!(mpv.join().unwrap().contains(r#"["quit"]"#));
    }

    #[test]
    fn close_with_no_mpv_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut player = Player::new("mpv", dir.path().join("mpv.sock"));
        assert_eq!(player.close(), Err(PlayerError::NotOpen));
    }

    #[test]
    fn an_mpv_the_review_started_that_does_not_quit_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let (program, argv) = fake_program(dir.path());
        let server = mpv_once_started(&socket, &argv);
        let mut player = Player::new(&program, socket.clone());
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
        server.join().unwrap();
        let pid = newest(&player);
        // The socket answers `quit`, but the program (a `sleep`) stays up.
        std::fs::remove_file(&socket).unwrap();
        let mpv = fake_mpv(UnixListener::bind(&socket).unwrap(), idle);
        let asked = Instant::now();
        assert_eq!(player.close(), Ok(()));
        assert!(asked.elapsed() >= QUIT_WAIT);
        assert_eq!(mpv.join().unwrap(), [json!(["quit"])]);
        assert!(gone(pid));
        assert!(player.children.is_empty());
    }

    #[test]
    fn an_mpv_out_of_the_sockets_reach_is_killed_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let (program, argv) = fake_program(dir.path());
        let server = mpv_once_started(&socket, &argv);
        let mut player = Player::new(&program, socket.clone());
        assert_eq!(player.open(&moment(), How::Load, &given(), false), PAUSED);
        server.join().unwrap();
        let pid = newest(&player);
        std::fs::remove_file(&socket).unwrap();
        let asked = Instant::now();
        assert_eq!(player.close(), Ok(()));
        assert!(asked.elapsed() < QUIT_WAIT);
        assert!(gone(pid));
        assert!(player.children.is_empty());
    }
}
