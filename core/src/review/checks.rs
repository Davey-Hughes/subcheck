//! Lanes for review's disk checks. A file on a share that stops answering blocks whatever reads
//! it, for as long as the share is gone, and no thread stuck there can be ended. So each kind of
//! check runs on a [`Lane`] of its own, off review's thread, and holds at most one thread however
//! often it is asked; [`SlowWord`] says when a check has gone too long unanswered. [`Checks`] is
//! review's three, as the runner hands them their tasks.

use std::marker::PhantomData;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use jiff::Timestamp;

use super::jobs::panic_text;
use super::update::{Input, Switch, Task};
use crate::player::Moment;
use crate::scan;
use crate::store::Run;

/// How a lane reads a file's size and modification time, `None` when it cannot. On a share that
/// stops answering this is the call that blocks, so tests put in one that blocks chosen paths.
pub type Stat = Arc<dyn Fn(&Path) -> Option<(u64, Timestamp)> + Send + Sync>;

/// The [`Stat`] outside tests: the stamp a scan reads, so that review compares like with like.
pub fn disk_stat() -> Stat {
    Arc::new(|path| scan::file_stamp(path).ok())
}

/// How a view switch reads the view it asks for ([`Task::Switch`]). It asks the disk too, of a
/// word of `OpenView`'s and of a path view's folders, so on a share that stops answering it blocks;
/// tests put in one that blocks on a word of theirs.
pub type ViewOf = Arc<dyn Fn(&Switch) -> anyhow::Result<Run> + Send + Sync>;

/// How long a check of `o`'s file goes unanswered before review says the share is not answering.
pub const SLOW_AFTER: Duration = Duration::from_secs(3);

/// Review's three kinds of disk check, each on a [`Lane`] of its own, so that one stuck on a dead
/// share holds neither review nor the other two: a view's file notes ([`Task::Note`]), `Play`'s
/// check of its file ([`Task::Stamp`]) and a view switch's read ([`Task::Switch`]). Each answer
/// comes as an [`Input`] on the channel review's inputs come in on, as a job's do, and so does a
/// check that panics, as `Input::Fatal`: its lane would otherwise be left marked running, and
/// every later check of its kind would wait for good. Dropping this never waits for a lane: a check
/// still stuck sends its answer, once it has one, to a channel no one may read any more, and ends.
pub struct Checks<T> {
    notes: Lane<(u64, Vec<String>)>,
    stamp: Lane<Moment>,
    switch: Lane<(u64, Switch)>,
    /// Told of each `Play` check as it is asked, by `take`, and as it answers, by the stamp lane.
    slow: Arc<SlowWord>,
    /// The lanes send `T`s, through the senders their runs hold.
    sends: PhantomData<fn(T)>,
}

impl<T: From<Input> + Send + 'static> Checks<T> {
    /// Lanes that read a file's size and mtime with `stat` and a view with `view_of`, and send
    /// their answers to `tx`; a `Play` check unanswered for `slow_after` is said to be
    /// ([`Input::StillChecking`]).
    pub fn new(stat: Stat, view_of: ViewOf, slow_after: Duration, tx: Sender<T>) -> Checks<T> {
        let said = tx.clone();
        let slow = Arc::new(SlowWord::new(slow_after, move |moment| {
            let _ = said.send(Input::StillChecking { moment }.into());
        }));
        let notes = {
            let (stat, tx) = (Arc::clone(&stat), tx.clone());
            Lane::new("file notes", move |(view, paths): (u64, Vec<String>)| {
                let noted = caught("a view's file check", || {
                    let stamps = paths
                        .into_iter()
                        .map(|path| {
                            let stamp = stat(Path::new(&path));
                            (path, stamp)
                        })
                        .collect();
                    Input::Noted { view, stamps }
                });
                let _ = tx.send(noted.into());
            })
        };
        let stamp = {
            let (slow, tx) = (Arc::clone(&slow), tx.clone());
            Lane::new("file stamp", move |moment: Moment| {
                let stamp = panic::catch_unwind(AssertUnwindSafe(|| stat(Path::new(&moment.file))));
                // Before the answer goes: once review has taken it, the press has been answered, and
                // must not be called slow after.
                slow.answered(&moment);
                let stamped = match stamp {
                    Ok(stamp) => Input::Stamped { moment, stamp },
                    Err(panic) => fatal("the video's file check", panic.as_ref()),
                };
                let _ = tx.send(stamped.into());
            })
        };
        let switch = Lane::new("view switch", move |(asked, to): (u64, Switch)| {
            let switched = caught("a view switch", || Input::Switched {
                asked,
                result: view_of(&to).map_err(|e| format!("{e:#}")),
            });
            let _ = tx.send(switched.into());
        });
        Checks {
            notes,
            stamp,
            switch,
            slow,
            sends: PhantomData,
        }
    }

    /// Hands a disk check to its lane and returns `None` at once, however long the check takes;
    /// its answer comes on the channel. Any other task comes back, for the caller to run.
    pub fn take(&self, task: Task) -> Option<Task> {
        match task {
            Task::Note { view, paths } => self.notes.ask((view, paths)),
            Task::Stamp(moment) => {
                // Counted from the press, whether its check starts now or waits behind a stuck one.
                self.slow.asked(moment.clone());
                self.stamp.ask(moment);
            }
            Task::Switch { asked, to } => self.switch.ask((asked, to)),
            other => return Some(other),
        }
        None
    }
}

/// What `check` answers, or `Input::Fatal` should it panic, naming `what` panicked.
fn caught(what: &str, check: impl FnOnce() -> Input) -> Input {
    panic::catch_unwind(AssertUnwindSafe(check)).unwrap_or_else(|panic| fatal(what, panic.as_ref()))
}

/// A check's panic, as review hears it: the end of review, as a job's panic is.
fn fatal(what: &str, panic: &(dyn std::any::Any + Send)) -> Input {
    Input::Fatal(format!("{what} panicked: {}", panic_text(panic)))
}

/// Runs one kind of disk check on a thread of its own, one request at a time. While one runs, only
/// the newest request waits, in place of any older one: a dead share must not pile up a stuck
/// thread per request, and a second look-up of a name on it would wait behind the first in the
/// kernel anyway. The thread ends when no request waits, so an idle lane holds none, and dropping a
/// lane never waits for its thread.
pub struct Lane<Q> {
    name: &'static str,
    run: Arc<dyn Fn(Q) + Send + Sync>,
    slot: Arc<Mutex<Slot<Q>>>,
}

/// What a lane's thread and its askers share.
struct Slot<Q> {
    /// A thread runs the lane's requests, and takes `next` when its run ends.
    running: bool,
    next: Option<Q>,
}

impl<Q: Send + 'static> Lane<Q> {
    /// A lane whose thread, named `name`, answers each request with `run`. `run` must not panic:
    /// its thread would end with the lane still marked running, and no later request would run.
    pub fn new(name: &'static str, run: impl Fn(Q) + Send + Sync + 'static) -> Lane<Q> {
        Lane {
            name,
            run: Arc::new(run),
            slot: Arc::new(Mutex::new(Slot {
                running: false,
                next: None,
            })),
        }
    }

    /// Hands `request` to the lane and returns at once, however long the running request takes:
    /// it waits, in place of any request still waiting, or starts the lane's thread when none
    /// runs. When no thread can be spawned, it runs here, as it would with no lane.
    pub fn ask(&self, request: Q) {
        {
            let mut slot = lock(&self.slot);
            if slot.running {
                slot.next = Some(request);
                return;
            }
            slot.running = true;
        }
        // The request goes over once the thread exists, so that a failed spawn still has it.
        let (hand, take) = mpsc::channel();
        let (slot, run) = (Arc::clone(&self.slot), Arc::clone(&self.run));
        let spawned = thread::Builder::new()
            .name(self.name.into())
            .spawn(move || {
                if let Ok(request) = take.recv() {
                    drain(request, &slot, &*run);
                }
            });
        match spawned {
            Ok(_) => {
                let _ = hand.send(request);
            }
            Err(_) => drain(request, &self.slot, &*self.run),
        }
    }
}

/// Runs `request`, then each request that waited for the one before, until none waits.
fn drain<Q>(mut request: Q, slot: &Mutex<Slot<Q>>, run: &dyn Fn(Q)) {
    loop {
        run(request);
        let mut shared = lock(slot);
        let Some(next) = shared.next.take() else {
            // Under the lock that found nothing waiting: a request asked between the two would wait
            // for a thread that has ended.
            shared.running = false;
            return;
        };
        request = next;
    }
}

/// Says, once, that a check has gone `after` unanswered: for the newest moment asked about only,
/// and never for one answered in time. One thread counts for every check, so a stuck check costs
/// no thread of its own here: the first `asked` starts it, and dropping the `SlowWord` ends it.
pub struct SlowWord {
    after: Duration,
    timer: Mutex<Timer>,
}

/// What `asked` and `answered` reach the timer's thread through.
struct Timer {
    /// Kept until the first `asked` starts the thread, which takes it.
    say: Option<Box<dyn Fn(Moment) + Send>>,
    /// To the thread: `None` until it starts, and for good if it could not be spawned.
    words: Option<Sender<Word>>,
}

/// What the timer's thread hears.
enum Word {
    /// Count to the instant for this moment, in place of any other.
    Asked(Moment, Instant),
    /// Stop, if the count is this moment's.
    Answered(Moment),
}

impl SlowWord {
    /// `say` is called on the timer's thread, with the moment whose check went `after` unanswered.
    pub fn new(after: Duration, say: impl Fn(Moment) + Send + 'static) -> SlowWord {
        SlowWord {
            after,
            timer: Mutex::new(Timer {
                say: Some(Box::new(say)),
                words: None,
            }),
        }
    }

    /// Starts the count for `moment`, in place of any other's: only the newest request waits on a
    /// lane, so only it can be slow. It counts from here, whether its check has started or waits
    /// behind a stuck one.
    pub fn asked(&self, moment: Moment) {
        let due = Instant::now() + self.after;
        let mut timer = lock(&self.timer);
        if let Some(say) = timer.say.take() {
            timer.words = start(say);
        }
        if let Some(words) = &timer.words {
            let _ = words.send(Word::Asked(moment, due));
        }
    }

    /// Stops the count if it is `moment`'s: an older moment's answer leaves a newer one counting.
    pub fn answered(&self, moment: &Moment) {
        if let Some(words) = &lock(&self.timer).words {
            let _ = words.send(Word::Answered(moment.clone()));
        }
    }
}

/// The timer's thread, or `None` when it cannot be spawned: then no check is ever called slow,
/// and the checks themselves go on.
fn start(say: Box<dyn Fn(Moment) + Send>) -> Option<Sender<Word>> {
    let (words, heard) = mpsc::channel();
    thread::Builder::new()
        .name("slow word".into())
        .spawn(move || count(&heard, &*say))
        .ok()?;
    Some(words)
}

/// The timer: counts to the instant asked last, says its moment when that comes first, and ends
/// when the `SlowWord` is dropped. A word already sent is heard before an instant that has passed,
/// so an answer sent in time never loses to its own count.
fn count(heard: &Receiver<Word>, say: &dyn Fn(Moment)) {
    let mut counting: Option<(Moment, Instant)> = None;
    loop {
        let word = match &counting {
            Some((_, due)) => heard.recv_timeout(due.saturating_duration_since(Instant::now())),
            None => heard.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match word {
            Ok(Word::Asked(moment, due)) => counting = Some((moment, due)),
            Ok(Word::Answered(moment)) => {
                if counting.as_ref().is_some_and(|(asked, _)| *asked == moment) {
                    counting = None;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some((moment, _)) = counting.take() {
                    say(moment);
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// `mutex`, locked, poisoned or not: what each lock here guards is whole between statements.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::TryRecvError;

    use super::*;

    /// How long a test waits for what should come at once: a lane that hangs fails its test, and
    /// does not hang the run.
    const LIMIT: Duration = Duration::from_secs(30);

    /// A lane of named requests, and what its run said: each request as it starts and as it ends,
    /// and the most runs ever in flight at once. A run of a request named in `held` ends only on a
    /// word on `release`, or when the probe is dropped, so a failed test leaves no thread stuck.
    struct Probe {
        lane: Arc<Lane<&'static str>>,
        started: Receiver<&'static str>,
        ended: Receiver<&'static str>,
        release: Sender<()>,
        most: Arc<AtomicUsize>,
    }

    fn probe(held: &'static [&'static str]) -> Probe {
        let (start, started) = mpsc::channel();
        let (end, ended) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let released = Mutex::new(released);
        let in_flight = AtomicUsize::new(0);
        let most = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&most);
        let lane = Lane::new("probe", move |request: &'static str| {
            let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            seen.fetch_max(now, Ordering::SeqCst);
            let _ = start.send(request);
            if held.contains(&request) {
                let _ = lock(&released).recv();
            }
            in_flight.fetch_sub(1, Ordering::SeqCst);
            let _ = end.send(request);
        });
        Probe {
            lane: Arc::new(lane),
            started,
            ended,
            release,
            most,
        }
    }

    /// Waits, within [`LIMIT`], until no thread runs `lane`'s requests. A lane tells nobody when
    /// it goes idle, so this reads its slot.
    fn wait_idle<Q>(lane: &Lane<Q>) {
        let since = Instant::now();
        while lock(&lane.slot).running {
            assert!(since.elapsed() < LIMIT, "the lane never went idle");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_lane_runs_one_request_at_a_time_and_only_the_newest_waits() {
        let p = probe(&["A"]);
        p.lane.ask("A");
        p.lane.ask("B");
        p.lane.ask("C");
        assert_eq!(p.started.recv_timeout(LIMIT), Ok("A"));
        p.release.send(()).unwrap();
        assert_eq!(p.ended.recv_timeout(LIMIT), Ok("A"));
        assert_eq!(p.started.recv_timeout(LIMIT), Ok("C"));
        assert_eq!(p.ended.recv_timeout(LIMIT), Ok("C"));
        wait_idle(&p.lane);
        assert_eq!(p.started.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(p.most.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn asking_a_stuck_lane_returns_at_once() {
        let p = probe(&["A"]);
        p.lane.ask("A");
        assert_eq!(p.started.recv_timeout(LIMIT), Ok("A"));
        let (back, returned) = mpsc::channel();
        let lane = Arc::clone(&p.lane);
        thread::spawn(move || {
            lane.ask("B");
            let _ = back.send(());
        });
        returned
            .recv_timeout(Duration::from_secs(1))
            .expect("asking waited for the stuck request");
        p.release.send(()).unwrap();
        assert_eq!(p.started.recv_timeout(LIMIT), Ok("B"));
    }

    #[test]
    fn a_lane_starts_again_after_it_went_idle() {
        let p = probe(&[]);
        p.lane.ask("A");
        assert_eq!(p.started.recv_timeout(LIMIT), Ok("A"));
        assert_eq!(p.ended.recv_timeout(LIMIT), Ok("A"));
        wait_idle(&p.lane);
        p.lane.ask("B");
        assert_eq!(p.started.recv_timeout(LIMIT), Ok("B"));
        assert_eq!(p.ended.recv_timeout(LIMIT), Ok("B"));
    }

    fn moment(file: &str) -> Moment {
        Moment {
            file: file.into(),
            start_us: 0,
            sid: None,
        }
    }

    /// A test's `say`: it sends each moment, with when it was said, on `said`. `_held` drops
    /// before `said` (a struct's fields drop in order), so once the test sees `said` close, the
    /// timer thread has let go of `_held` too.
    struct Ear {
        _held: Arc<()>,
        said: Sender<(Moment, Instant)>,
    }

    impl Ear {
        fn hear(&self, moment: Moment) {
            let _ = self.said.send((moment, Instant::now()));
        }
    }

    /// A slow word counting to `after`; what it says; and an `Arc` its `say` holds a clone of.
    fn slow_word(after: Duration) -> (SlowWord, Receiver<(Moment, Instant)>, Arc<()>) {
        let held = Arc::new(());
        let (said, heard) = mpsc::channel();
        let ear = Ear {
            _held: Arc::clone(&held),
            said,
        };
        // A method call, not a field's: the closure must take the whole `Ear`, `_held` with it.
        let slow = SlowWord::new(after, move |moment| ear.hear(moment));
        (slow, heard, held)
    }

    #[test]
    fn the_slow_word_comes_for_the_newest_moment_only() {
        let after = Duration::from_millis(50);
        // B must be asked before A's count is up, or A's word would be right. A machine too busy to
        // ask it in time spoils the try, and the case runs again.
        for _ in 0..5 {
            let (slow, said, _) = slow_word(after);
            let a_asked = Instant::now();
            slow.asked(moment("A"));
            thread::sleep(Duration::from_millis(20));
            let b_asked = Instant::now();
            slow.asked(moment("B"));
            if a_asked.elapsed() >= after {
                continue;
            }
            let (word, at) = said.recv_timeout(LIMIT).expect("no word came");
            assert_eq!(word, moment("B"));
            assert!(
                at.duration_since(b_asked) >= after,
                "the word came {:?} after B was asked",
                at.duration_since(b_asked)
            );
            assert_eq!(
                said.recv_timeout(Duration::from_millis(200)),
                Err(RecvTimeoutError::Timeout)
            );
            return;
        }
        panic!("no try asked B before A's count was up");
    }

    #[test]
    fn the_slow_word_never_comes_for_an_answered_moment() {
        let (slow, said, _) = slow_word(Duration::from_millis(100));
        slow.asked(moment("A"));
        slow.answered(&moment("A"));
        assert_eq!(
            said.recv_timeout(Duration::from_millis(200)),
            Err(RecvTimeoutError::Timeout)
        );
    }

    #[test]
    fn an_older_moments_answer_leaves_the_newest_counting() {
        let (slow, said, _) = slow_word(Duration::from_millis(50));
        slow.asked(moment("A"));
        slow.asked(moment("B"));
        slow.answered(&moment("A"));
        assert_eq!(
            said.recv_timeout(LIMIT).map(|(word, _)| word),
            Ok(moment("B"))
        );
    }

    #[test]
    fn a_dropped_slow_word_ends_its_thread() {
        let (slow, said, held) = slow_word(LIMIT);
        slow.asked(moment("A"));
        drop(slow);
        assert_eq!(
            said.recv_timeout(Duration::from_secs(1)),
            Err(RecvTimeoutError::Disconnected)
        );
        assert_eq!(Arc::strong_count(&held), 1);
    }

    /// Checks over a test's `stat` and `view_of`, and the channel they answer on.
    fn checks(
        stat: Stat,
        view_of: ViewOf,
        slow_after: Duration,
    ) -> (Checks<Input>, Receiver<Input>) {
        let (tx, rx) = mpsc::channel();
        (Checks::new(stat, view_of, slow_after, tx), rx)
    }

    /// A view switch's read that finds a folder's view by its path, and nothing else.
    fn folders_only() -> ViewOf {
        Arc::new(|to: &Switch| -> anyhow::Result<Run> {
            match to {
                Switch::Folder(folder) => Ok(Run {
                    name: folder.clone(),
                    ..Run::default()
                }),
                other => anyhow::bail!("not a folder: {other:?}"),
            }
        })
    }

    #[test]
    fn the_three_disk_checks_go_to_their_lanes_and_answer_on_the_channel() {
        let at = |s: i64| (1_u64, Timestamp::from_second(s).unwrap());
        let stat: Stat =
            Arc::new(move |path: &Path| (path == Path::new("/m/E01.mkv")).then(|| at(7)));
        let (checks, rx) = checks(stat, folders_only(), LIMIT);
        assert_eq!(checks.take(Task::Stamp(moment("/m/E01.mkv"))), None);
        assert_eq!(
            rx.recv_timeout(LIMIT),
            Ok(Input::Stamped {
                moment: moment("/m/E01.mkv"),
                stamp: Some(at(7)),
            })
        );
        let paths = vec!["/m/E01.mkv".to_string(), "/m/E02.mkv".to_string()];
        assert_eq!(checks.take(Task::Note { view: 3, paths }), None);
        assert_eq!(
            rx.recv_timeout(LIMIT),
            Ok(Input::Noted {
                view: 3,
                stamps: vec![
                    ("/m/E01.mkv".into(), Some(at(7))),
                    ("/m/E02.mkv".into(), None)
                ],
            })
        );
        let to = Switch::Folder("/m".into());
        assert_eq!(checks.take(Task::Switch { asked: 2, to }), None);
        let folder = Run {
            name: "/m".into(),
            ..Run::default()
        };
        assert_eq!(
            rx.recv_timeout(LIMIT),
            Ok(Input::Switched {
                asked: 2,
                result: Ok(folder),
            })
        );
        let to = Switch::RunNaming("/m/E01.mkv".into());
        assert_eq!(checks.take(Task::Switch { asked: 3, to }), None);
        assert_eq!(
            rx.recv_timeout(LIMIT),
            Ok(Input::Switched {
                asked: 3,
                result: Err("not a folder: RunNaming(\"/m/E01.mkv\")".into()),
            })
        );
        // Any other task is the caller's to run.
        let other = Task::ReadBack("/m/E01.mkv".into());
        assert_eq!(checks.take(other.clone()), Some(other));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    }

    // A check that panics ends review, as a job's panic does, rather than wedge its lane: the lane
    // still takes the next check of its kind.
    #[test]
    fn a_check_that_panics_is_fatal_and_its_lane_goes_on() {
        let stat: Stat = Arc::new(|path: &Path| {
            assert!(path != Path::new("/m/boom"), "boom");
            None
        });
        let view_of: ViewOf = Arc::new(|_: &Switch| -> anyhow::Result<Run> { panic!("bang") });
        let (checks, rx) = checks(stat, view_of, LIMIT);
        let fatal = |what: &str, text: &str| Ok(Input::Fatal(format!("{what} panicked: {text}")));
        checks.take(Task::Stamp(moment("/m/boom")));
        assert_eq!(
            rx.recv_timeout(LIMIT),
            fatal("the video's file check", "boom")
        );
        let paths = vec!["/m/boom".to_string()];
        checks.take(Task::Note { view: 0, paths });
        assert_eq!(rx.recv_timeout(LIMIT), fatal("a view's file check", "boom"));
        let to = Switch::Folder("/m".into());
        checks.take(Task::Switch { asked: 1, to });
        assert_eq!(rx.recv_timeout(LIMIT), fatal("a view switch", "bang"));
        checks.take(Task::Stamp(moment("/m/E01.mkv")));
        assert_eq!(
            rx.recv_timeout(LIMIT),
            Ok(Input::Stamped {
                moment: moment("/m/E01.mkv"),
                stamp: None,
            })
        );
    }

    // `Play`'s check, held past the count from its press, is said to be slow on the channel, once;
    // its answer comes after all the same.
    #[test]
    fn a_play_check_gone_unanswered_is_said_slow_and_still_answers() {
        let (let_go, held) = mpsc::channel::<()>();
        let held = Mutex::new(held);
        let stat: Stat = Arc::new(move |_: &Path| {
            let _ = lock(&held).recv_timeout(LIMIT);
            None
        });
        let (checks, rx) = checks(stat, folders_only(), Duration::from_millis(50));
        checks.take(Task::Stamp(moment("/m/E01.mkv")));
        assert_eq!(
            rx.recv_timeout(LIMIT),
            Ok(Input::StillChecking {
                moment: moment("/m/E01.mkv"),
            })
        );
        drop(let_go);
        assert_eq!(
            rx.recv_timeout(LIMIT),
            Ok(Input::Stamped {
                moment: moment("/m/E01.mkv"),
                stamp: None,
            })
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(200)),
            Err(RecvTimeoutError::Timeout)
        );
    }
}
