//! `subcheck review`: accept, reject or reword each finding of a run, with a key that opens the
//! moment in mpv. This is its screen, the terminal's UI over review's core
//! (`subcheck_core::review`): `keymap` and `screen` turn the terminal's keys and clicks into the
//! core's actions, `view` draws it, and `run_effect` runs what the core asks for, a task through
//! the core's runner and a player command on mpv.

pub mod files;
pub mod keymap;
pub mod layout;
pub mod screen;
pub mod view;

use std::collections::VecDeque;
use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

use crate::mpv::{self, Player};
use model::Review;
use progress::Scan;
use runner::Runner;
use screen::{Mouse, Msg, Screen};
use subcheck_core::group;
use subcheck_core::player::PlayerCommand;
use subcheck_core::review::{jobs, model, progress, runner, update};
use subcheck_core::scan;
use subcheck_core::store::{Run, Store};
use update::{Effect, Input};

/// A terminal event as a `Msg`. Ctrl and Alt chords never reach the plain-key handlers, so Ctrl-C
/// quits instead of reading as `c`; with the mouse they are ignored.
pub fn map_event(ev: &Event) -> Option<Msg> {
    let k = match ev {
        Event::Key(k) => k,
        Event::Mouse(me) => {
            if me
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                return None;
            }
            let kind = match me.kind {
                MouseEventKind::Down(MouseButton::Left) => Mouse::Click,
                MouseEventKind::ScrollUp => Mouse::WheelUp,
                MouseEventKind::ScrollDown => Mouse::WheelDown,
                _ => return None,
            };
            return Some(Msg::Mouse(kind, me.column, me.row));
        }
        Event::Resize(w, h) => return Some(Msg::Resize(*w, *h)),
        _ => return None,
    };
    if k.kind != KeyEventKind::Press {
        return None;
    }
    if k.modifiers.contains(KeyModifiers::CONTROL) {
        return match k.code {
            KeyCode::Char('c' | 'd') => Some(Msg::Interrupt),
            KeyCode::Char('u') => Some(Msg::KillLine),
            _ => None,
        };
    }
    if k.modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    Some(Msg::Key(k.code))
}

/// The next event from `read` that is a `Msg`. With the mouse captured, every movement of the
/// pointer is an event too; reading past those here saves a redraw for each.
fn next_msg(mut read: impl FnMut() -> io::Result<Event>) -> io::Result<Msg> {
    loop {
        if let Some(msg) = map_event(&read()?) {
            return Ok(msg);
        }
    }
}

/// Read the terminal on a thread of its own, each key, click or resize sent as its `Msg`. A
/// terminal that cannot be read ends the screen (`Input::Fatal`), and so does a panic: with this
/// thread gone no key reaches the loop, `q` and Ctrl-C among them.
fn input(tx: Sender<Msg>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("input".into())
        .spawn(move || {
            let read = panic::catch_unwind(AssertUnwindSafe(|| {
                loop {
                    let msg = next_msg(event::read).unwrap_or_else(|e| {
                        Input::Fatal(format!("reading the terminal: {e}")).into()
                    });
                    let fatal = matches!(msg, Msg::Input(Input::Fatal(_)));
                    if tx.send(msg).is_err() || fatal {
                        return;
                    }
                }
            }));
            if let Err(panic) = read {
                let _ = tx.send(
                    Input::Fatal(format!(
                        "reading the terminal panicked: {}",
                        jobs::panic_text(panic.as_ref())
                    ))
                    .into(),
                );
            }
        })?;
    Ok(())
}

/// Run one effect and say how it went: a task through `runner`, a player command on mpv.
pub fn run_effect(effect: Effect, runner: &mut Runner<Msg>, player: &mut Player) -> Input {
    match effect {
        Effect::Task(task) => runner.run(task),
        Effect::Player(PlayerCommand::Open(moment)) => Input::Opened(player.open(&moment)),
        Effect::Player(PlayerCommand::Playback(p)) => Input::Played(player.playback(p)),
        Effect::Player(PlayerCommand::Close) => Input::Closed(player.close()),
    }
}

/// Restores the terminal on every exit path: normal return, `?`, or panic.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<TerminalGuard> {
        enable_raw_mode()?;
        if let Err(e) = execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture) {
            leave();
            return Err(e);
        }
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        leave();
    }
}

/// The terminal as it was: no raw mode, no mouse capture, the main screen.
fn leave() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
}

/// What review's screen opens on.
#[allow(clippy::large_enum_variant)] // one per process, moved once: a box would save nothing
pub enum Open {
    /// A run, as `subcheck review` names it, put together from the store.
    View(Run),
    /// A scan, as `subcheck scan` planned it: the screen runs it, on the run it records under. `paths`
    /// are the arguments it was given, for the command that finishes it if it is stopped;
    /// `warnings`, what planning it wrote to the terminal (see [`Tee`]), which the screen covers.
    Scan {
        plan: scan::Plan,
        opts: scan::Options,
        paths: Vec<PathBuf>,
        warnings: Vec<String>,
    },
}

/// A log passed on to `inner` as it is written, and kept line by line: what `scan` warns of before
/// its screen opens stays on the terminal, and the screen, which covers it, can say so too.
pub struct Tee<W> {
    inner: W,
    /// What is written after the last newline.
    partial: Vec<u8>,
    lines: Vec<String>,
}

impl<W: Write> Tee<W> {
    pub fn new(inner: W) -> Tee<W> {
        Tee {
            inner,
            partial: Vec::new(),
            lines: Vec::new(),
        }
    }

    /// Each whole line written, without its newline.
    pub fn into_lines(self) -> Vec<String> {
        self.lines
    }
}

impl<W: Write> Write for Tee<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Kept as far as `inner` took it, so the two never differ.
        let n = self.inner.write(buf)?;
        self.partial.extend_from_slice(&buf[..n]);
        while let Some(end) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            self.lines
                .push(String::from_utf8_lossy(&line[..end]).into_owned());
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// What the status line opens with: the first of what `scan` warned of before the screen covered
/// it, and how many warnings are more. They are printed above the screen too, so `+n more` points
/// at what the user can scroll up to. A message as others are, until the next key.
fn opening_status(warnings: &[String]) -> Option<String> {
    let (first, rest) = warnings.split_first()?;
    Some(match rest.len() {
        0 => first.clone(),
        n => format!("{first} · +{n} more"),
    })
}

/// How long the loop waits for a message while a job runs before the spinner turns.
const TICK: Duration = Duration::from_millis(250);

/// Open the screen on `open`, writing decisions through `store`. `state` is the state directory,
/// where mpv's socket goes when there is no runtime directory, and apply writes the fixed tracks.
/// `karaoke`: the screen's apply re-spaces per-letter karaoke. Exit status 1 when a scan or an
/// apply failed, or a file in one did.
pub fn run(open: Open, store: Store, state: &Path, karaoke: bool) -> anyhow::Result<ExitCode> {
    let (run, scanning, warnings) = match open {
        Open::View(run) => (run, None, Vec::new()),
        Open::Scan {
            plan,
            opts,
            paths,
            warnings,
        } => (
            runner::opening(&store, &plan).context("reading the run")?,
            Some((plan, opts, paths)),
            warnings,
        ),
    };
    if run.findings.is_empty() && scanning.is_none() {
        println!("No findings in {}.", run.called());
        return Ok(ExitCode::SUCCESS);
    }
    let notes = runner::file_notes(&run);
    let mut player = Player::new("mpv", mpv::socket_path(state));
    let mut model = Review::new(run, notes);
    model.karaoke = karaoke;
    model.status = opening_status(&warnings);

    // The terminal is restored should the screen's own thread panic. A job's panic comes to the
    // loop as `Input::Fatal`, which ends the screen in order.
    let screen_thread = std::thread::current().id();
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == screen_thread {
            leave();
        }
        prev_hook(info);
    }));
    let guard = TerminalGuard::enter().context("entering the alternate screen")?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let size = terminal.size()?;
    // With no scan, the status line opens on what the run's LLM calls cost (`Screen::show_cost`).
    let mut screen = Screen::default();
    screen.set_area(&model, Rect::new(0, 0, size.width, size.height));
    // The runner keeps a sender of its own, for the jobs `c`, `A` and `D` start: the channel
    // cannot close while the screen runs, and each job says why it ends before it does
    // (`Input::ScanEnded`, `Input::ApplyEnded`, `Input::Fatal`), as the input thread does.
    let (tx, rx) = mpsc::channel();
    if let Some((plan, opts, paths)) = scanning {
        // Running from the start, so a `q` before the scan's first word still asks.
        let name = model.run.name.clone();
        let resume = progress::resume_command(&name, &paths);
        model.progress.start(&name, resume, opts.into.is_some());
        let scans = store.reopen()?;
        jobs::scan(plan, opts, scans, tx.clone()).context("starting the scan")?;
    }
    let mut runner = Runner::new(store, state.to_path_buf(), tx.clone());
    // A scan may be running from here, so every way out goes through the teardown below, which
    // ends its LLM calls: an input thread that does not start, the loop's error, and a panic on
    // this thread, caught here (the hook has restored the terminal) and resumed once they are
    // ended. Not a drop guard: one made here would end the groups before `player`, made earlier,
    // let go of its own.
    let looped = match input(tx.clone()) {
        Ok(()) => panic::catch_unwind(AssertUnwindSafe(|| {
            screen_loop(
                &mut terminal,
                &mut model,
                &mut screen,
                &rx,
                &mut runner,
                &mut player,
            )
        })),
        Err(e) => Ok(Err(e).context("reading the terminal")),
    };
    drop(terminal);
    drop(guard);
    // The mpv this review started is closed, and its registration let go, before `group::end`: one
    // let go after it would wait forever.
    drop(player);
    if model.progress.busy() || model.fatal.is_some() {
        group::end();
    }
    let looped = looped.unwrap_or_else(|panic| panic::resume_unwind(panic));
    outcome(
        &mut model,
        looped,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    )
}

/// What the terminal gets as the screen ends, and the exit status. On an error (the loop's, or the
/// model's fatal one), the error, returned; a job it stopped says first how far it got and the
/// command that finishes it. Otherwise the outro on `err`, then the decisions' counts on `out`:
/// exit status 1 when any scan or apply the screen ran failed, or a file in it did.
fn outcome(
    model: &mut Review,
    looped: anyhow::Result<()>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<ExitCode> {
    let outro = model.outro();
    if let Some(e) = looped
        .err()
        .or_else(|| model.fatal.take().map(anyhow::Error::msg))
    {
        if model.progress.busy() {
            for line in outro {
                writeln!(err, "{line}")?;
            }
        }
        return Err(e);
    }
    for line in outro {
        writeln!(err, "{line}")?;
    }
    let (pending, accepted, rejected) = model.counts();
    writeln!(
        out,
        "{}: {accepted} accepted, {rejected} rejected, {pending} pending",
        model.run.called()
    )?;
    let failed = model.progress.earlier_failed
        || model.progress.failed > 0
        || matches!(model.progress.scan, Scan::Ended(Err(_)))
        || model.progress.apply_failed();
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Draw, take the next message (the terminal's, a job's, or a tick while one runs), update, and
/// run the effects, until the model quits.
fn screen_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    model: &mut Review,
    screen: &mut Screen,
    rx: &mpsc::Receiver<Msg>,
    runner: &mut Runner<Msg>,
    player: &mut Player,
) -> anyhow::Result<()> {
    while !model.should_quit {
        terminal.draw(|f| view::view(model, screen, f))?;
        let msg = receive(rx, model.progress.busy());
        let effects = screen::update(model, screen, msg, jiff::Timestamp::now());
        // Show what is about to happen (`opening E02 at …`) before an effect holds the loop:
        // starting mpv can take seconds.
        if !effects.is_empty() {
            terminal.draw(|f| view::view(model, screen, f))?;
        }
        let mut queue: VecDeque<Effect> = effects.into();
        while let Some(effect) = queue.pop_front() {
            let input = run_effect(effect, runner, player);
            queue.extend(screen::update(
                model,
                screen,
                input.into(),
                jiff::Timestamp::now(),
            ));
        }
    }
    Ok(())
}

/// The loop's next message: the terminal's or a job's, or while a job runs a tick should none come
/// for [`TICK`]. The runner holds a `Sender` of its own (for the jobs `c`, `A` and `D` start), so
/// the channel cannot close while the screen runs; a disconnect is fatal, a backstop: each job
/// (the input thread, each scan and apply) reports why it ends before it does (`Input::Fatal`,
/// `Input::ScanEnded`, `Input::ApplyEnded`), and with the input thread gone no key can quit.
fn receive(rx: &mpsc::Receiver<Msg>, busy: bool) -> Msg {
    let got = if busy {
        match rx.recv_timeout(TICK) {
            Err(mpsc::RecvTimeoutError::Timeout) => return Msg::Tick,
            got => got.ok(),
        }
    } else {
        rx.recv().ok()
    };
    got.unwrap_or_else(|| Input::Fatal("the screen's input stopped".into()).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, MouseEvent};
    use subcheck_core::apply;

    #[test]
    fn keys_map_to_msgs_and_chords_do_not_leak() {
        let key = |code, mods| Event::Key(KeyEvent::new(code, mods));
        assert_eq!(
            map_event(&key(KeyCode::Char('a'), KeyModifiers::NONE)),
            Some(Msg::Key(KeyCode::Char('a')))
        );
        assert_eq!(
            map_event(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Msg::Interrupt)
        );
        assert_eq!(
            map_event(&key(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Some(Msg::Interrupt)
        );
        assert_eq!(
            map_event(&key(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            Some(Msg::KillLine)
        );
        // Ctrl-A is not `a`: it must not accept.
        assert_eq!(
            map_event(&key(KeyCode::Char('a'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(map_event(&key(KeyCode::Char('a'), KeyModifiers::ALT)), None);
        let release = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        assert_eq!(map_event(&release), None);
        assert_eq!(map_event(&Event::Resize(80, 24)), Some(Msg::Resize(80, 24)));
        assert_eq!(map_event(&Event::FocusGained), None);
    }

    #[test]
    fn events_that_mean_nothing_are_read_past_without_a_redraw() {
        let moved = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 7,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });
        let j = Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        let mut events = vec![moved.clone(), moved, Event::FocusGained, j].into_iter();
        let mut reads = 0;
        let msg = next_msg(|| {
            reads += 1;
            Ok(events.next().unwrap())
        });
        assert_eq!(msg.unwrap(), Msg::Key(KeyCode::Char('j')));
        assert_eq!(reads, 4);
    }

    #[test]
    fn left_clicks_and_the_wheel_map_to_msgs() {
        let mouse = |kind, modifiers| {
            Event::Mouse(MouseEvent {
                kind,
                column: 7,
                row: 3,
                modifiers,
            })
        };
        let none = KeyModifiers::NONE;
        assert_eq!(
            map_event(&mouse(MouseEventKind::Down(MouseButton::Left), none)),
            Some(Msg::Mouse(Mouse::Click, 7, 3))
        );
        assert_eq!(
            map_event(&mouse(MouseEventKind::ScrollUp, none)),
            Some(Msg::Mouse(Mouse::WheelUp, 7, 3))
        );
        assert_eq!(
            map_event(&mouse(MouseEventKind::ScrollDown, none)),
            Some(Msg::Mouse(Mouse::WheelDown, 7, 3))
        );
        for kind in [
            MouseEventKind::Down(MouseButton::Right),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Moved,
        ] {
            assert_eq!(map_event(&mouse(kind, none)), None, "{kind:?}");
        }
        // Ctrl+wheel zooms the terminal; it is not a scroll.
        assert_eq!(
            map_event(&mouse(MouseEventKind::ScrollUp, KeyModifiers::CONTROL)),
            None
        );
    }

    #[test]
    fn a_scans_screen_opens_with_its_warnings_on_the_status_line() {
        let status = |m: &Review| m.status_text();
        let warnings = vec![
            "warning: /m/locked: skipped: Permission denied (os error 13)".to_string(),
            "warning: /m/lost: skipped: Input/output error (os error 5)".to_string(),
        ];
        let mut m = scanning();
        m.status = opening_status(&warnings);
        assert_eq!(
            status(&m).as_deref(),
            Some("warning: /m/locked: skipped: Permission denied (os error 13) · +1 more")
        );
        // A message as others are: the next key ends it, and the scan's progress shows.
        let mut s = Screen::default();
        screen::update(
            &mut m,
            &mut s,
            Msg::Key(KeyCode::Char('j')),
            jiff::Timestamp::now(),
        );
        assert_eq!(status(&m).as_deref(), Some("scan 0/2 · 0 findings"));
        // One warning alone, or none.
        assert_eq!(opening_status(&warnings[..1]), Some(warnings[0].clone()));
        assert_eq!(opening_status(&[]), None);
    }

    #[test]
    fn a_tee_passes_the_log_on_and_keeps_each_whole_line() {
        let mut tee = Tee::new(Vec::new());
        write!(tee, "warning: a").unwrap();
        writeln!(tee, " b").unwrap();
        tee.write_all(b"c\nd").unwrap();
        assert_eq!(tee.inner, b"warning: a b\nc\nd");
        assert_eq!(tee.into_lines(), ["warning: a b", "c"]);
    }

    #[test]
    fn a_channel_no_job_sends_on_any_more_ends_the_screen() {
        let (tx, rx) = mpsc::channel();
        tx.send(Msg::Key(KeyCode::Char('j'))).unwrap();
        drop(tx);
        assert_eq!(receive(&rx, true), Msg::Key(KeyCode::Char('j')));
        // Every job gone without a word: busy or not, the screen cannot go on.
        for running in [true, false] {
            assert_eq!(
                receive(&rx, running),
                Msg::Input(Input::Fatal("the screen's input stopped".into()))
            );
        }
        let (_tx, rx) = mpsc::channel();
        assert_eq!(receive(&rx, true), Msg::Tick);
    }

    #[test]
    fn each_player_command_runs_on_mpv_and_comes_back_as_its_input() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_at(&dir.path().join("subcheck.db")).unwrap();
        let mut player = Player::new(dir.path().join("no-mpv"), dir.path().join("mpv.sock"));
        let (tx, _rx) = mpsc::channel();
        let mut runner = Runner::new(store, dir.path().into(), tx);
        let mut ran = |command| {
            let effect = Effect::Player(command);
            run_effect(effect, &mut runner, &mut player)
        };
        let moment = subcheck_core::player::Moment {
            file: "/m/E01.mkv".into(),
            start_ms: 0,
            sid: None,
            paused: false,
        };
        // No mpv to start, and none on the socket: each says so as its own input.
        let opened = ran(PlayerCommand::Open(moment));
        let missing = "no-mpv not found: install mpv to open findings";
        assert!(
            matches!(&opened, Input::Opened(Err(e)) if e.ends_with(missing)),
            "{opened:?}"
        );
        assert_eq!(
            ran(PlayerCommand::Playback(
                subcheck_core::player::Playback::Pause
            )),
            Input::Played(Err("mpv is not open (o opens it)".into()))
        );
        assert_eq!(
            ran(PlayerCommand::Close),
            Input::Closed(Err("mpv is not open".into()))
        );
    }

    /// The sample run with a scan of two files under way, as the loop leaves it on a stop.
    fn scanning() -> Review {
        let mut m = model::tests_support::sample();
        m.progress.start(
            "2026-10-17T04-39-18",
            "subcheck scan --run r /m".into(),
            false,
        );
        m.progress.scan_event(&scan::Event::Queued(vec![
            "/m/E01.mkv".into(),
            "/m/E02.mkv".into(),
        ]));
        m
    }

    /// What `outcome` returns, and what it wrote to stdout and stderr.
    fn ended(
        m: &mut Review,
        looped: anyhow::Result<()>,
    ) -> (anyhow::Result<ExitCode>, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = outcome(m, looped, &mut out, &mut err);
        let text = |b: Vec<u8>| String::from_utf8(b).unwrap();
        (code, text(out), text(err))
    }

    #[test]
    fn an_error_that_stops_the_scan_says_first_how_to_finish_it() {
        let stopped = "run 2026-10-17T04-39-18: stopped at 0/2 files, 0 failed; 0 findings\n\
                       Finish it with: subcheck scan --run r /m\n";
        let mut m = scanning();
        m.fatal = Some("the scan panicked: boom".into());
        let (code, out, err) = ended(&mut m, Ok(()));
        assert_eq!(code.unwrap_err().to_string(), "the scan panicked: boom");
        assert_eq!((out.as_str(), err.as_str()), ("", stopped));
        // The loop's own error: the terminal could not be drawn on, say.
        let (code, out, err) = ended(&mut scanning(), Err(anyhow::anyhow!("os error 5")));
        assert_eq!(code.unwrap_err().to_string(), "os error 5");
        assert_eq!((out.as_str(), err.as_str()), ("", stopped));
        // With no scan running, the error alone.
        let mut m = model::tests_support::sample();
        m.fatal = Some("reading the terminal: gone".into());
        let (code, out, err) = ended(&mut m, Ok(()));
        assert_eq!(code.unwrap_err().to_string(), "reading the terminal: gone");
        assert_eq!((out.as_str(), err.as_str()), ("", ""));
    }

    #[test]
    fn quitting_with_files_queued_gives_the_command_that_checks_them() {
        let mut m = scanning();
        m.progress.queue_again("/m/E03.mkv", "2026-10-17T04-39-18");
        let (code, _, err) = ended(&mut m, Ok(()));
        assert_eq!(code.unwrap(), ExitCode::SUCCESS);
        assert_eq!(
            err,
            "run 2026-10-17T04-39-18: stopped at 0/2 files, 0 failed; 0 findings\n\
             Finish it with: subcheck scan --run r /m\n\
             Not checked again: subcheck scan --run 2026-10-17T04-39-18 --force '/m/E03.mkv'\n"
        );
    }

    #[test]
    fn a_stop_alone_exits_0_and_a_failure_1() {
        let (code, out, err) = ended(&mut scanning(), Ok(()));
        assert_eq!(code.unwrap(), ExitCode::SUCCESS);
        assert!(
            err.contains("Finish it with: subcheck scan --run r /m"),
            "{err}"
        );
        assert_eq!(
            out,
            "run 2026-10-17T04-39-18: 0 accepted, 0 rejected, 4 pending\n"
        );
        let mut m = scanning();
        m.progress.scan_event(&scan::Event::Finished {
            path: "/m/E01.mkv".into(),
            error: Some("LLM: boom".into()),
            findings: 0,
            lines: vec![],
        });
        assert_eq!(ended(&mut m, Ok(())).0.unwrap(), ExitCode::FAILURE);
        let mut m = scanning();
        m.progress.scan = Scan::Ended(Err("disk full".into()));
        assert_eq!(ended(&mut m, Ok(())).0.unwrap(), ExitCode::FAILURE);
        let mut m = model::tests_support::sample();
        let (code, out, err) = ended(&mut m, Ok(()));
        assert_eq!(code.unwrap(), ExitCode::SUCCESS);
        assert_eq!(err, "");
        assert!(out.ends_with("4 pending\n"), "{out}");
    }

    #[test]
    fn an_earlier_scans_failure_still_exits_1_after_a_clean_one() {
        let mut m = scanning();
        m.progress.scan_event(&scan::Event::Finished {
            path: "/m/E01.mkv".into(),
            error: Some("LLM: boom".into()),
            findings: 0,
            lines: vec![],
        });
        m.progress.scan = Scan::Ended(Ok(scan::Summary::default()));
        m.progress
            .start("r2", "subcheck scan --run r2 /m".into(), true);
        m.progress.scan = Scan::Ended(Ok(scan::Summary::default()));
        assert_eq!(m.progress.failed, 0);
        assert_eq!(ended(&mut m, Ok(())).0.unwrap(), ExitCode::FAILURE);
    }

    #[test]
    fn an_apply_that_failed_a_file_is_in_the_outro_and_exits_1() {
        let mut m = model::tests_support::sample();
        let e01 = model::tests_support::path(1);
        m.progress.start_apply(false, "subcheck apply r".into());
        m.progress
            .apply_event(&apply::Event::Queued(vec![e01.clone()]));
        m.progress.apply_event(&apply::Event::Failed {
            path: e01.clone(),
            error: "disk full".into(),
            text: String::new(),
            findings: vec![],
        });
        m.progress.apply_ended(Ok(apply::Summary {
            failed: 1,
            ..apply::Summary::default()
        }));
        let (code, _, err) = ended(&mut m, Ok(()));
        assert_eq!(code.unwrap(), ExitCode::FAILURE);
        assert_eq!(
            err,
            format!(
                "{e01}: failed: disk full\n\
                 applied 0, already applied 0, by hand 0, not found 0; 0 remuxed, 1 failed\n"
            )
        );
    }

    #[test]
    fn quitting_during_apply_gives_the_command_that_finishes_it() {
        let mut m = model::tests_support::sample();
        let (e01, e02) = (model::tests_support::path(1), model::tests_support::path(2));
        m.progress
            .start_apply(false, "subcheck apply 2026-10-17T04-39-18".into());
        m.progress
            .apply_event(&apply::Event::Queued(vec![e01.clone(), e02]));
        m.progress
            .apply_event(&apply::Event::Stage(e01, apply::Stage::Remux));
        let (code, _, err) = ended(&mut m, Ok(()));
        assert_eq!(code.unwrap(), ExitCode::SUCCESS);
        assert_eq!(
            err,
            "apply stopped on E01 (remux), 0/2 files done:\n\
             applied 0, already applied 0, by hand 0, not found 0; 0 remuxed, 0 failed\n\
             Finish it with: subcheck apply 2026-10-17T04-39-18\n"
        );
    }
}
