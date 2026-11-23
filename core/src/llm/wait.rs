//! What a call to an LLM waits on besides the LLM's answer, and the lines that say so.

use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

/// How long a wait lasts before [`WaitLines`] says it. Most waits are over well within it, and a
/// line for each of those would be noise about something nobody had to wait for.
pub const HOLD: Duration = Duration::from_secs(1);

/// What a call waits on before the LLM can answer. Each holds its line, worded where the wait
/// starts, so everything that shows a wait shows the same words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wait {
    /// An entry's key command running: `<entry>: waiting for its key command`.
    KeyCommand(String),
    /// A server asked what it serves: `<entry>: asking <entry url> what it serves`.
    Asking(String),
    /// Ollama loading a model: `loading <model> into Ollama at <root>`.
    Loading(String),
}

impl Wait {
    /// The line that names the wait.
    pub fn line(&self) -> &str {
        match self {
            Wait::KeyCommand(line) | Wait::Asking(line) | Wait::Loading(line) => line,
        }
    }

    /// Whether the wait is worth saying as it starts rather than once it has lasted [`HOLD`]:
    /// only a load, which always takes a while.
    pub fn at_once(&self) -> bool {
        matches!(self, Wait::Loading(_))
    }
}

/// Where a call reports its waits: `Some` as one starts, `None` once the wait under way ends.
/// There is one wait at a time: a `Some` while another is under way replaces it. `Sync`, since a
/// scan's workers all report through the one.
pub type Waits<'a> = &'a (dyn Fn(Option<Wait>) + Sync);

/// Says waits as lines on an output: a wait [`Wait::at_once`] as it starts, any other once it has
/// been the one under way for the hold, so one that is replaced or ends before then says nothing.
pub struct WaitLines {
    said: Arc<Mutex<Said>>,
    hold: Duration,
}

struct Said {
    out: Box<dyn Write + Send>,
    /// How many waits have started or ended. A held line is written only while this is still what
    /// its wait made it, which is while that wait is the one under way.
    changes: u64,
}

impl Said {
    /// Writes `line` and its line end in one write, so that nothing else written to the same
    /// output lands between them, and flushes it, since the wait it names is under way now. An
    /// error is ignored: a line that cannot be said must not fail the call that waits.
    fn write(&mut self, line: &str) {
        let _ = self.out.write_all(format!("{line}\n").as_bytes());
        let _ = self.out.flush();
    }
}

impl WaitLines {
    /// Lines on `out`. `hold` is [`HOLD`] but in tests, which cannot wait that long for each line.
    pub fn new(out: Box<dyn Write + Send>, hold: Duration) -> WaitLines {
        WaitLines {
            said: Arc::new(Mutex::new(Said { out, changes: 0 })),
            hold,
        }
    }

    /// Takes what a [`Waits`] reports: the wait that has started, or `None` for the end of the one
    /// under way.
    pub fn say(&self, wait: Option<Wait>) {
        let mut said = self.said.lock().unwrap_or_else(PoisonError::into_inner);
        said.changes += 1;
        let Some(wait) = wait else { return };
        if wait.at_once() {
            said.write(wait.line());
            return;
        }
        let mine = said.changes;
        drop(said);
        let said = Arc::clone(&self.said);
        let hold = self.hold;
        // A thread the OS cannot start drops the line: a wait that cannot be said must not fail the
        // call that waits.
        let _ = thread::Builder::new().spawn(move || {
            thread::sleep(hold);
            // Checked and written under the one lock, which `say` takes to end a wait: the line
            // is never written after its wait has ended.
            let mut said = said.lock().unwrap_or_else(PoisonError::into_inner);
            if said.changes == mine {
                said.write(wait.line());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::time::Instant;

    use super::*;
    use crate::test_support::tries;

    /// Shorter than [`HOLD`], so the tests are quick; they wait three of it for a line that must
    /// not come.
    const SHORT: Duration = Duration::from_millis(100);

    /// What reached the lines' output and was flushed: bytes written wait here until a flush, so a
    /// line never flushed is a line the tests never see.
    #[derive(Default)]
    struct Out {
        written: Vec<u8>,
        flushed: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for Out {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushed.lock().unwrap().append(&mut self.written);
            Ok(())
        }
    }

    fn lines() -> (WaitLines, Arc<Mutex<Vec<u8>>>) {
        let out = Out::default();
        let flushed = Arc::clone(&out.flushed);
        (WaitLines::new(Box::new(out), SHORT), flushed)
    }

    fn said(flushed: &Mutex<Vec<u8>>) -> String {
        String::from_utf8(flushed.lock().unwrap().clone()).unwrap()
    }

    /// What has been said once `enough` holds of it, or after 5 seconds whatever has been.
    fn said_when(flushed: &Mutex<Vec<u8>>, enough: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text = said(flushed);
            if enough(&text) || Instant::now() >= deadline {
                return text;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn key_command() -> Wait {
        Wait::KeyCommand("srv: waiting for its key command".into())
    }

    fn a_load() -> Wait {
        Wait::Loading("loading m into Ollama at http://h".into())
    }

    #[test]
    fn a_wait_gives_its_line_and_only_a_load_is_said_at_once() {
        let asking = Wait::Asking("srv: asking http://h/v1 what it serves".into());
        assert_eq!(key_command().line(), "srv: waiting for its key command");
        assert_eq!(asking.line(), "srv: asking http://h/v1 what it serves");
        assert_eq!(a_load().line(), "loading m into Ollama at http://h");
        assert!(!key_command().at_once());
        assert!(!asking.at_once());
        assert!(a_load().at_once());
    }

    #[test]
    fn a_load_is_said_at_once() {
        let (lines, flushed) = lines();
        lines.say(Some(a_load()));
        assert_eq!(said(&flushed), "loading m into Ollama at http://h\n");
    }

    #[test]
    fn a_wait_that_lasts_the_hold_is_said_once() {
        let (lines, flushed) = lines();
        let started = Instant::now();
        lines.say(Some(key_command()));
        let first = said_when(&flushed, |text| !text.is_empty());
        assert!(started.elapsed() >= SHORT, "said before the hold was over");
        assert_eq!(first, "srv: waiting for its key command\n");
        thread::sleep(3 * SHORT);
        assert_eq!(said(&flushed), "srv: waiting for its key command\n");
    }

    #[test]
    fn a_wait_over_within_the_hold_says_nothing() {
        // The wait must end before its hold is up, or its line would be right.
        tries(|| {
            let (lines, flushed) = lines();
            let started = Instant::now();
            lines.say(Some(key_command()));
            lines.say(None);
            if started.elapsed() >= SHORT {
                return false;
            }
            thread::sleep(3 * SHORT);
            assert_eq!(said(&flushed), "");
            true
        });
    }

    #[test]
    fn a_wait_that_replaces_another_within_the_hold_is_the_only_one_said() {
        // The second wait must start before the first's hold is up, or the first's line would be
        // right.
        tries(|| {
            let (lines, flushed) = lines();
            let started = Instant::now();
            lines.say(Some(key_command()));
            lines.say(Some(Wait::Asking(
                "srv: asking http://h/v1 what it serves".into(),
            )));
            if started.elapsed() >= SHORT {
                return false;
            }
            said_when(&flushed, |text| !text.is_empty());
            thread::sleep(3 * SHORT);
            assert_eq!(said(&flushed), "srv: asking http://h/v1 what it serves\n");
            true
        });
    }

    #[test]
    fn a_load_that_replaces_a_held_wait_is_the_only_one_said() {
        // The load must start before the held wait's hold is up, or the held wait's line would be
        // right.
        tries(|| {
            let (lines, flushed) = lines();
            let started = Instant::now();
            lines.say(Some(key_command()));
            lines.say(Some(a_load()));
            if started.elapsed() >= SHORT {
                return false;
            }
            thread::sleep(3 * SHORT);
            assert_eq!(said(&flushed), "loading m into Ollama at http://h\n");
            true
        });
    }

    #[test]
    fn a_wait_said_again_after_it_ended_is_said_again() {
        let (lines, flushed) = lines();
        let line = "srv: waiting for its key command\n";
        lines.say(Some(key_command()));
        assert_eq!(said_when(&flushed, |text| !text.is_empty()), line);
        lines.say(None);
        lines.say(Some(key_command()));
        let twice = line.repeat(2);
        assert_eq!(said_when(&flushed, |text| text == twice), twice);
    }

    #[test]
    fn waits_are_said_from_any_thread_through_one_set_of_lines() {
        let (lines, flushed) = lines();
        let waits: Waits = &|wait| lines.say(wait);
        thread::scope(|scope| {
            scope.spawn(|| waits(Some(a_load())));
        });
        assert_eq!(said(&flushed), "loading m into Ollama at http://h\n");
    }
}
