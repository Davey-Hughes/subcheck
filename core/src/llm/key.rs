//! API keys: read from a variable or printed by a command, kept in the process, never shown.
//!
//! A command's key is kept until a server refuses it, so a passphrase prompt (`pass`, `gpg`)
//! appears once while the key is accepted and rechecks reuse the key. A key has worked once a
//! server has answered a chat sent with it; a list of models shows nothing, since some servers
//! list theirs without checking the key. The command runs again only after a refusal: a 401 for a
//! key that had worked runs it again once, and a key a 401 refuses for good is dropped, so the
//! next scan or recheck runs it. A 403 keeps the key for the scan under way, so that an answer
//! after it can still show that the key works and a 401 later still has it fetched again, and
//! marks it for the next scan or recheck to run the command: some servers answer 403 to a key
//! that has run out. The key the command then prints starts as one that has not worked. The
//! command has no terminal, since rechecks run while the screen owns it and the window has none:
//! its stdin is an empty pipe, it runs in a session of its own, so opening `/dev/tty` fails at
//! once, and `GPG_TTY` is unset. A prompt must have a window of its own.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use super::{run, without_terminal};
use crate::settings::KeySource;

/// A password manager's prompt, answered by hand, takes a while; a hung one must still end.
const KEY_TIMEOUT: Duration = Duration::from_mins(2);

/// Keys by the text of the command that printed them. The lock is held while a command runs, so two
/// fetches at once run it once.
static KEPT: Mutex<BTreeMap<String, Kept>> = Mutex::new(BTreeMap::new());

/// A command's key and whether it has worked: a server has answered a chat sent with it
/// ([`worked`]). A refused key that worked has probably been rotated, so the command is worth
/// running again; one that never worked is as likely the wrong key, which the command would only
/// print again.
struct Kept {
    key: Key,
    worked: bool,
    /// A server answered 403 to the key ([`forbidden`]): the next [`fetch`] runs the command
    /// again rather than give this key.
    run_again: bool,
}

impl Kept {
    /// A key the command has just printed, which no chat has been answered for since.
    fn new(key: &Key) -> Self {
        Self {
            key: key.clone(),
            worked: false,
            run_again: false,
        }
    }
}

/// An API key. Its `Debug` hides it, so a derived `Debug` on anything holding one cannot leak it.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Key(String);

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key(..)")
    }
}

impl Key {
    /// The `Authorization` header's value.
    pub(super) fn bearer(&self) -> String {
        format!("Bearer {}", self.0)
    }

    /// `text` with the key replaced, for text a server sent back that may echo it.
    pub(super) fn hide(&self, text: &str) -> String {
        text.replace(&self.0, "<key>")
    }
}

/// The key `source` gives: none for `KeySource::None`; a variable is read each time; a command runs
/// once while its key is kept. A kept key that a 403 marked ([`forbidden`]) is not given: the
/// command runs again, since the 403 may have meant the key has run out. The key it prints, the
/// same or another, is kept as one that has not worked: its next chat settles whether it works,
/// and a 401 before then must not open a second passphrase window for a key just printed.
///
/// `running(true)` is called just before the command runs and `running(false)` once it returns,
/// whether it printed a key or failed; neither is called when no command runs. It is called with
/// the kept keys' lock held, so it must not call back into this module: it would wait on that lock
/// forever.
pub(super) fn fetch(source: &KeySource, running: &dyn Fn(bool)) -> Result<Option<Key>, String> {
    match source {
        KeySource::None => Ok(None),
        KeySource::Env(var) => from_env(var, std::env::var_os(var)).map(Some),
        KeySource::Command(cmd) => {
            let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(entry) = kept.get(cmd).filter(|k| !k.run_again) {
                return Ok(Some(entry.key.clone()));
            }
            // Dropped before the command runs, so a failure leaves nothing kept, a key a 403
            // marked included, and the next `fetch` starts over.
            kept.remove(cmd);
            let key = run_command(cmd, running)?;
            kept.insert(cmd.clone(), Kept::new(&key));
            Ok(Some(key))
        }
    }
}

/// Marks a command's kept key as one that has worked, while it is still `sent`, the one a server
/// answered a chat for; a key fetched since has yet to be tried.
pub(super) fn worked(source: &KeySource, sent: &Key) {
    if let KeySource::Command(cmd) = source {
        let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = kept.get_mut(cmd).filter(|k| k.key == *sent) {
            entry.worked = true;
        }
    }
}

/// The key to send in place of `refused`, which a server answered 401; `Ok(None)` when there is
/// none, so `refused` is refused for good. A command runs again only for a kept key that had
/// worked, and a key it prints is kept as one that has not, so the calls refused with one kept key
/// have it fetched again at most once between them. A key a 403 marked ([`forbidden`]) is treated
/// as any other: what a 403 said of it changes nothing of what a 401 says.
///
/// `running` is called as for [`fetch`], and only when the command runs, with the kept keys' lock
/// held: it must not call back into this module, or it would wait on that lock forever.
pub(super) fn refetch(
    source: &KeySource,
    refused: &Key,
    running: &dyn Fn(bool),
) -> Result<Option<Key>, String> {
    let KeySource::Command(cmd) = source else {
        return Ok(None);
    };
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(entry) = kept.get(cmd) else {
        return Ok(None);
    };
    // Another call refused with the same key has fetched again already.
    if entry.key != *refused {
        return Ok(Some(entry.key.clone()));
    }
    // Dropped before the command runs, so a failure or the same key again leaves nothing kept and
    // the next `fetch` starts over.
    let had_worked = kept.remove(cmd).is_some_and(|k| k.worked);
    if !had_worked {
        return Ok(None);
    }
    let key = run_command(cmd, running)?;
    if key == *refused {
        return Ok(None);
    }
    kept.insert(cmd.clone(), Kept::new(&key));
    Ok(Some(key))
}

/// What a 403 does to a command's kept key: the next [`fetch`] runs the command again, when the
/// kept key is still `sent`, the one the server refused; a key fetched since is left as it is.
/// The key stays kept, marked for that run: a 403 can be about one request, and dropping the key
/// would lose whether it has worked, which an answer after the 403 can yet show ([`worked`]) and
/// which [`refetch`] needs to run the command when a 401 refuses the key later in the same scan.
pub(super) fn forbidden(source: &KeySource, sent: &Key) {
    if let KeySource::Command(cmd) = source {
        let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = kept.get_mut(cmd).filter(|k| k.key == *sent) {
            entry.run_again = true;
        }
    }
}

fn from_env(var: &str, value: Option<OsString>) -> Result<Key, String> {
    match value.map(OsString::into_string) {
        // A value read from a `.env` file can keep its line's CR or newline, which no header can
        // hold. The message never says the value, which is the key.
        Some(Ok(v)) if v.contains(char::is_control) => Err(format!(
            "api_key_env names {var}, whose value holds a control character"
        )),
        Some(Ok(v)) if !v.is_empty() => Ok(Key(v)),
        Some(Err(_)) => Err(format!("api_key_env names {var}, which is not valid UTF-8")),
        _ => Err(format!("api_key_env names {var}, which is unset or empty")),
    }
}

/// Runs the command with `running` told before and after, so the end is reported on a failure too.
fn run_command(cmd: &str, running: &dyn Fn(bool)) -> Result<Key, String> {
    running(true);
    let key = from_command(cmd);
    running(false);
    key
}

fn from_command(cmd: &str) -> Result<Key, String> {
    let argv = ["sh".to_string(), "-c".to_string(), cmd.to_string()];
    // Only the failure's message is shown: a command that fails after printing part of a key must
    // not put that part in an error. Nor is sh's own for a program it cannot find (127), which
    // names the program: a key written in place of the command is that program.
    let out = run(&argv, "", KEY_TIMEOUT, without_terminal).map_err(|e| match e.code {
        Some(127) => "api_key_cmd failed: a program it runs was not found (exit status 127); \
                      api_key_cmd is a command that prints the key, not the key"
            .to_string(),
        _ => format!("api_key_cmd failed: {}", e.message),
    })?;
    let key = out.trim();
    if key.is_empty() {
        return Err("api_key_cmd printed no key".into());
    }
    if key.contains('\n') {
        return Err("api_key_cmd printed more than one line: make it print the key alone".into());
    }
    // No header can hold one, and the error would then blame the server, not the command.
    if key.contains(char::is_control) {
        return Err("api_key_cmd printed a key holding a control character".into());
    }
    Ok(Key(key.to_string()))
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt;
    use std::path::{Path, PathBuf};
    use std::thread;

    use super::*;

    /// `running`'s calls, in order.
    #[derive(Default)]
    struct Said(Mutex<Vec<bool>>);

    impl Said {
        fn say(&self) -> impl Fn(bool) + '_ {
            |on| self.0.lock().unwrap().push(on)
        }
        fn take(&self) -> Vec<bool> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    #[test]
    fn fetch_says_when_its_command_runs_and_when_it_ends() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = KeySource::Command(format!("echo sk-10 # {}", dir.path().display()));
        let said = Said::default();
        fetch(&cmd, &said.say()).unwrap();
        assert_eq!(said.take(), [true, false]);
        fetch(&cmd, &said.say()).unwrap();
        assert_eq!(said.take(), Vec::<bool>::new());
        fetch(&KeySource::Env("HOME".into()), &said.say()).unwrap();
        fetch(&KeySource::None, &said.say()).unwrap();
        assert_eq!(said.take(), Vec::<bool>::new());
        let failing = KeySource::Command(format!("exit 3 # {}", dir.path().display()));
        assert!(fetch(&failing, &said.say()).is_err());
        assert_eq!(said.take(), [true, false]);
    }

    #[test]
    fn refetch_says_when_its_command_runs() {
        let dir = tempfile::tempdir().unwrap();
        let (never, key, _) = printing(&dir.path().join("a"));
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(&key, "sk-11\n").unwrap();
        let said = Said::default();
        let k = fetch(&never, &|_| {}).unwrap().unwrap();
        assert_eq!(refetch(&never, &k, &said.say()), Ok(None));
        assert_eq!(said.take(), Vec::<bool>::new());

        std::fs::create_dir_all(dir.path().join("b")).unwrap();
        let (ran, key, _) = printing(&dir.path().join("b"));
        std::fs::write(&key, "sk-12\n").unwrap();
        let k1 = fetch(&ran, &|_| {}).unwrap().unwrap();
        worked(&ran, &k1);
        std::fs::write(&key, "sk-13\n").unwrap();
        assert!(refetch(&ran, &k1, &said.say()).unwrap().is_some());
        assert_eq!(said.take(), [true, false]);
        // Another call's key is already fetched, so no command runs.
        assert!(refetch(&ran, &k1, &said.say()).unwrap().is_some());
        assert_eq!(said.take(), Vec::<bool>::new());
    }

    #[test]
    fn a_key_from_a_variable_is_its_value_read_each_time() {
        assert_eq!(from_env("K", Some("sk-1".into())), Ok(Key("sk-1".into())));
        for unset in [None, Some(OsString::new())] {
            assert_eq!(
                from_env("K", unset),
                Err("api_key_env names K, which is unset or empty".into())
            );
        }
        let bad = OsString::from_vec(vec![0xff]);
        assert_eq!(
            from_env("K", Some(bad)),
            Err("api_key_env names K, which is not valid UTF-8".into())
        );
        // As a value read from a `.env` file with Windows line ends can be.
        for control in ["sk-1\r", "sk-1\n", "sk\t-1"] {
            assert_eq!(
                from_env("K", Some(control.into())),
                Err("api_key_env names K, whose value holds a control character".into()),
                "{control:?}"
            );
        }
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            fetch(&KeySource::Env("HOME".into()), &|_| {}),
            Ok(Some(Key(home)))
        );
        assert_eq!(fetch(&KeySource::None, &|_| {}), Ok(None));
    }

    #[test]
    fn a_key_command_runs_once_a_process_until_a_403_marks_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let cmd = KeySource::Command(format!("echo x >> '{}'; printf ' sk-2\n'", count.display()));
        let runs = || std::fs::read_to_string(&count).unwrap().lines().count();
        assert_eq!(fetch(&cmd, &|_| {}), Ok(Some(Key("sk-2".into()))));
        assert_eq!(fetch(&cmd, &|_| {}), Ok(Some(Key("sk-2".into()))));
        assert_eq!(runs(), 1);
        // A refused key fetched before the kept one leaves it kept.
        forbidden(&cmd, &Key("sk-1".into()));
        fetch(&cmd, &|_| {}).unwrap();
        assert_eq!(runs(), 1);
        forbidden(&cmd, &Key("sk-2".into()));
        fetch(&cmd, &|_| {}).unwrap();
        assert_eq!(runs(), 2);
    }

    #[test]
    fn a_key_that_worked_is_kept_through_a_403_and_a_401_still_fetches_it_again() {
        let dir = tempfile::tempdir().unwrap();
        let (cmd, key, runs) = printing(dir.path());
        std::fs::write(&key, "sk-1\n").unwrap();
        let k1 = fetch(&cmd, &|_| {}).unwrap().unwrap();
        worked(&cmd, &k1);
        forbidden(&cmd, &k1);
        // It is still known to have worked, so a 401 runs the command, once.
        std::fs::write(&key, "sk-2\n").unwrap();
        let k2 = Key("sk-2".into());
        assert_eq!(refetch(&cmd, &k1, &|_| {}), Ok(Some(k2.clone())));
        assert_eq!(refetch(&cmd, &k1, &|_| {}), Ok(Some(k2.clone())));
        assert_eq!(runs(), 2);
        // The key kept anew carries no 403's mark, so the next fetch gives it.
        assert_eq!(fetch(&cmd, &|_| {}), Ok(Some(k2)));
        assert_eq!(runs(), 2);
    }

    #[test]
    fn after_a_403_a_key_that_worked_is_fetched_by_a_new_run_once() {
        let dir = tempfile::tempdir().unwrap();
        let (cmd, key, runs) = printing(dir.path());
        std::fs::write(&key, "sk-1\n").unwrap();
        let k1 = fetch(&cmd, &|_| {}).unwrap().unwrap();
        worked(&cmd, &k1);
        forbidden(&cmd, &k1);
        std::fs::write(&key, "sk-2\n").unwrap();
        let k2 = Key("sk-2".into());
        assert_eq!(fetch(&cmd, &|_| {}), Ok(Some(k2.clone())));
        assert_eq!(runs(), 2);
        assert_eq!(fetch(&cmd, &|_| {}), Ok(Some(k2.clone())));
        assert_eq!(runs(), 2);
        // The new key has not worked: the old one's acceptance says nothing of it.
        assert_eq!(refetch(&cmd, &k2, &|_| {}), Ok(None));
        assert_eq!(runs(), 2);
    }

    #[test]
    fn a_403_shows_nothing_of_whether_a_key_works() {
        let dir = tempfile::tempdir().unwrap();
        let (cmd, key, runs) = printing(dir.path());
        std::fs::write(&key, "sk-1\n").unwrap();
        let k1 = fetch(&cmd, &|_| {}).unwrap().unwrap();
        forbidden(&cmd, &k1);
        assert_eq!(refetch(&cmd, &k1, &|_| {}), Ok(None));
        assert_eq!(runs(), 1);
    }

    #[test]
    fn a_403_before_a_key_has_worked_keeps_it_for_the_answer_that_shows_it_works() {
        let dir = tempfile::tempdir().unwrap();
        let (cmd, key, runs) = printing(dir.path());
        std::fs::write(&key, "sk-1\n").unwrap();
        let k1 = fetch(&cmd, &|_| {}).unwrap().unwrap();
        // As a scan's first answer can be: one chunk refused before any chat is answered.
        forbidden(&cmd, &k1);
        worked(&cmd, &k1);
        std::fs::write(&key, "sk-2\n").unwrap();
        assert_eq!(refetch(&cmd, &k1, &|_| {}), Ok(Some(Key("sk-2".into()))));
        assert_eq!(runs(), 2);
    }

    #[test]
    fn a_key_printed_again_after_a_403_is_kept_as_one_that_has_not_worked() {
        let dir = tempfile::tempdir().unwrap();
        let (cmd, key, runs) = printing(dir.path());
        std::fs::write(&key, "sk-1\n").unwrap();
        let k1 = fetch(&cmd, &|_| {}).unwrap().unwrap();
        worked(&cmd, &k1);
        forbidden(&cmd, &k1);
        assert_eq!(fetch(&cmd, &|_| {}), Ok(Some(k1.clone())));
        assert_eq!(runs(), 2);
        assert_eq!(fetch(&cmd, &|_| {}), Ok(Some(k1.clone())));
        assert_eq!(runs(), 2);
        // A 401 for the key just printed runs the command no third time.
        assert_eq!(refetch(&cmd, &k1, &|_| {}), Ok(None));
        assert_eq!(runs(), 2);
    }

    #[test]
    fn a_key_a_403_marked_whose_command_then_fails_is_dropped_and_the_fetch_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let cmd = KeySource::Command(format!(
            "echo x >> '{0}'; if [ $(wc -l < '{0}') -gt 1 ]; then echo locked >&2; exit 2; fi; \
             echo sk-1",
            count.display()
        ));
        let runs = || std::fs::read_to_string(&count).unwrap().lines().count();
        let locked = Err("api_key_cmd failed: sh exited with exit status: 2: locked".to_string());
        let k = fetch(&cmd, &|_| {}).unwrap().unwrap();
        worked(&cmd, &k);
        forbidden(&cmd, &k);
        assert_eq!(fetch(&cmd, &|_| {}), locked);
        assert_eq!(runs(), 2);
        // The old key is not kept behind the failure.
        assert_eq!(fetch(&cmd, &|_| {}), locked);
        assert_eq!(runs(), 3);
    }

    #[test]
    fn two_fetches_at_once_run_the_key_command_once() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let cmd = KeySource::Command(format!(
            "echo x >> '{}'; sleep 0.3; echo sk-6",
            count.display()
        ));
        let fetched: Vec<_> = thread::scope(|s| {
            let both = [
                s.spawn(|| fetch(&cmd, &|_| {})),
                s.spawn(|| fetch(&cmd, &|_| {})),
            ];
            both.map(|t| t.join().unwrap()).into()
        });
        assert_eq!(fetched, vec![Ok(Some(Key("sk-6".into()))); 2]);
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 1);
    }

    /// A key command that prints `key`'s contents, a line added to `count` each run.
    fn printing(dir: &Path) -> (KeySource, PathBuf, impl Fn() -> usize) {
        let (key, count) = (dir.join("key"), dir.join("count"));
        let cmd = format!("echo x >> '{}'; cat '{}'", count.display(), key.display());
        let runs = move || std::fs::read_to_string(&count).unwrap().lines().count();
        (KeySource::Command(cmd), key, runs)
    }

    #[test]
    fn a_refused_key_that_worked_is_fetched_again_once() {
        let dir = tempfile::tempdir().unwrap();
        let (cmd, key, runs) = printing(dir.path());
        std::fs::write(&key, "sk-1\n").unwrap();
        let k1 = fetch(&cmd, &|_| {}).unwrap().unwrap();
        assert_eq!(k1, Key("sk-1".into()));
        worked(&cmd, &k1);
        std::fs::write(&key, "sk-2\n").unwrap();
        assert_eq!(refetch(&cmd, &k1, &|_| {}), Ok(Some(Key("sk-2".into()))));
        assert_eq!(runs(), 2);
        // A second call refused with the same key finds the one fetched since.
        assert_eq!(refetch(&cmd, &k1, &|_| {}), Ok(Some(Key("sk-2".into()))));
        assert_eq!(runs(), 2);
        assert_eq!(fetch(&cmd, &|_| {}), Ok(Some(Key("sk-2".into()))));
        assert_eq!(runs(), 2);
    }

    #[test]
    fn a_refused_key_that_never_worked_is_not_fetched_again_and_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (cmd, key, runs) = printing(dir.path());
        std::fs::write(&key, "sk-1\n").unwrap();
        let k = fetch(&cmd, &|_| {}).unwrap().unwrap();
        assert_eq!(refetch(&cmd, &k, &|_| {}), Ok(None));
        assert_eq!(runs(), 1);
        assert_eq!(fetch(&cmd, &|_| {}), Ok(Some(k.clone())));
        assert_eq!(runs(), 2);
        // Another key working says nothing of the kept one.
        worked(&cmd, &Key("other".into()));
        assert_eq!(refetch(&cmd, &k, &|_| {}), Ok(None));
        assert_eq!(runs(), 2);
        assert_eq!(fetch(&cmd, &|_| {}), Ok(Some(k)));
        assert_eq!(runs(), 3);
    }

    #[test]
    fn a_command_that_prints_the_refused_key_again_refuses_it_and_keeps_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (cmd, key, runs) = printing(dir.path());
        std::fs::write(&key, "sk-1\n").unwrap();
        let k = fetch(&cmd, &|_| {}).unwrap().unwrap();
        worked(&cmd, &k);
        assert_eq!(refetch(&cmd, &k, &|_| {}), Ok(None));
        assert_eq!(runs(), 2);
        assert_eq!(refetch(&cmd, &k, &|_| {}), Ok(None));
        assert_eq!(runs(), 2);
        assert_eq!(fetch(&cmd, &|_| {}), Ok(Some(k)));
        assert_eq!(runs(), 3);
    }

    #[test]
    fn a_key_fetched_again_has_not_worked_until_it_is_said_to() {
        let dir = tempfile::tempdir().unwrap();
        let (cmd, key, runs) = printing(dir.path());
        std::fs::write(&key, "sk-1\n").unwrap();
        let k1 = fetch(&cmd, &|_| {}).unwrap().unwrap();
        worked(&cmd, &k1);
        std::fs::write(&key, "sk-2\n").unwrap();
        let k2 = refetch(&cmd, &k1, &|_| {}).unwrap().unwrap();
        assert_eq!(k2, Key("sk-2".into()));
        assert_eq!(runs(), 2);
        assert_eq!(refetch(&cmd, &k2, &|_| {}), Ok(None));
        assert_eq!(runs(), 2);
    }

    #[test]
    fn a_command_that_fails_when_run_again_says_why_and_keeps_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let cmd = KeySource::Command(format!(
            "echo x >> '{0}'; if [ $(wc -l < '{0}') -gt 1 ]; then echo locked >&2; exit 2; fi; \
             echo sk-1",
            count.display()
        ));
        let runs = || std::fs::read_to_string(&count).unwrap().lines().count();
        let locked = Err("api_key_cmd failed: sh exited with exit status: 2: locked".to_string());
        let k = fetch(&cmd, &|_| {}).unwrap().unwrap();
        worked(&cmd, &k);
        assert_eq!(refetch(&cmd, &k, &|_| {}), locked);
        assert_eq!(runs(), 2);
        assert_eq!(fetch(&cmd, &|_| {}), locked);
        assert_eq!(runs(), 3);
    }

    #[test]
    fn a_key_from_a_variable_or_no_key_is_never_fetched_again() {
        let k = Key("sk-1".into());
        for source in [KeySource::Env("HOME".into()), KeySource::None] {
            worked(&source, &k);
            assert_eq!(refetch(&source, &k, &|_| {}), Ok(None), "{source:?}");
        }
    }

    #[test]
    fn two_refetches_at_once_run_the_key_command_once() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let cmd = KeySource::Command(format!(
            "echo x >> '{}'; sleep 0.3; echo sk-9",
            count.display()
        ));
        let k = fetch(&cmd, &|_| {}).unwrap().unwrap();
        worked(&cmd, &k);
        let said = Said::default();
        let refetched: Vec<_> = thread::scope(|s| {
            let both = [
                s.spawn(|| refetch(&cmd, &k, &said.say())),
                s.spawn(|| refetch(&cmd, &k, &said.say())),
            ];
            both.map(|t| t.join().unwrap()).into()
        });
        assert_eq!(refetched, vec![Ok(None); 2]);
        // The second finds nothing kept, so only one thread ran the command and said so.
        let mut calls = said.take();
        calls.sort_unstable();
        assert_eq!(calls, [false, true]);
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2);
    }

    #[test]
    fn two_refetches_at_once_share_the_new_key() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        // Each run prints a key of its own: its number, read through `$(( ))` since a `wc` can
        // pad its count.
        let cmd = KeySource::Command(format!(
            "echo x >> '{0}'; sleep 0.3; echo sk-$(($(wc -l < '{0}')))",
            count.display()
        ));
        let k = fetch(&cmd, &|_| {}).unwrap().unwrap();
        assert_eq!(k, Key("sk-1".into()));
        worked(&cmd, &k);
        // The second waits for the lock through the first's run, and so finds its key; let go
        // while the command ran, the lock would show it nothing kept.
        let refetched: Vec<_> = thread::scope(|s| {
            let both = [
                s.spawn(|| refetch(&cmd, &k, &|_| {})),
                s.spawn(|| refetch(&cmd, &k, &|_| {})),
            ];
            both.map(|t| t.join().unwrap()).into()
        });
        assert_eq!(refetched, vec![Ok(Some(Key("sk-2".into()))); 2]);
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2);
    }

    #[test]
    fn a_key_command_that_fails_says_its_stderr_and_never_its_stdout() {
        let failed =
            KeySource::Command("echo sk-partial; echo 'gpg: decryption failed' >&2; exit 2".into());
        assert_eq!(
            fetch(&failed, &|_| {}),
            Err("api_key_cmd failed: sh exited with exit status: 2: gpg: decryption failed".into())
        );
        assert_eq!(
            fetch(&KeySource::Command("true".into()), &|_| {}),
            Err("api_key_cmd printed no key".into())
        );
        let many = fetch(
            &KeySource::Command("printf 'sk-3\\nuser: me\\n'".into()),
            &|_| {},
        )
        .unwrap_err();
        assert_eq!(
            many,
            "api_key_cmd printed more than one line: make it print the key alone"
        );
        for control in ["printf 'sk\\t-7'", "printf '\\033[1msk-7\\033[0m'"] {
            assert_eq!(
                fetch(&KeySource::Command(control.into()), &|_| {}),
                Err("api_key_cmd printed a key holding a control character".into()),
                "{control}"
            );
        }
    }

    #[test]
    fn a_key_written_in_place_of_the_command_is_not_shown() {
        // As written, quoted, and with a `$` that sh expands: each names a program sh cannot find.
        for pasted in ["sk-or-v1-0a1b", "'sk-or-v1-0a1b'", "sk-or-v1-0a$1b"] {
            assert_eq!(
                fetch(&KeySource::Command(pasted.into()), &|_| {}),
                Err(
                    "api_key_cmd failed: a program it runs was not found (exit status 127); \
                     api_key_cmd is a command that prints the key, not the key"
                        .into()
                ),
                "{pasted}"
            );
        }
    }

    #[test]
    fn a_key_command_reads_an_empty_pipe_not_the_terminal() {
        let k = fetch(
            &KeySource::Command("if [ -t 0 ]; then echo tty; else cat; echo sk-4; fi".into()),
            &|_| {},
        );
        assert_eq!(k, Ok(Some(Key("sk-4".into()))));
    }

    #[test]
    fn a_key_command_has_no_terminal() {
        // In subcheck's session the terminal opens, and a read of it stops the command.
        let tty = "if (: </dev/tty) 2>/dev/null; then echo tty; else echo sk-8; fi";
        let (pid, session, k) = crate::test_support::a_commands_session(tty, |cmd| {
            fetch(&KeySource::Command(cmd.into()), &|_| {})
        });
        // Its own session, which a run with no terminal (CI) can tell apart.
        assert_eq!(session, Ok(pid));
        assert_eq!(k, Ok(Some(Key("sk-8".into()))));
    }

    #[test]
    fn a_key_is_never_shown() {
        let k = Key("sk-5".into());
        assert_eq!(format!("{k:?} {:?}", Some(&k)), "Key(..) Some(Key(..))");
        assert_eq!(k.bearer(), "Bearer sk-5");
        assert_eq!(
            k.hide("bad key sk-5, try again (sk-5)"),
            "bad key <key>, try again (<key>)"
        );
    }
}
