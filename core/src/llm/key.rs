//! API keys: read from a variable or printed by a command, kept for the process, never shown.
//!
//! A command's key is kept for the rest of the process so a passphrase prompt (`pass`, `gpg`)
//! appears at most once a session and rechecks reuse the key. The command has no terminal, since
//! rechecks run while the screen owns it and the window has none: its stdin is an empty pipe, it
//! runs in a session of its own, so opening `/dev/tty` fails at once, and `GPG_TTY` is unset. A
//! prompt must have a window of its own.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::process::{Child, Command};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use super::run;
use crate::group::{self, Registered};
use crate::settings::KeySource;

/// A password manager's prompt, answered by hand, takes a while; a hung one must still end.
const KEY_TIMEOUT: Duration = Duration::from_mins(2);

/// Keys by the text of the command that printed them. The lock is held while a command runs, so two
/// fetches at once run it once.
static KEPT: Mutex<BTreeMap<String, Key>> = Mutex::new(BTreeMap::new());

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
/// once while its key is kept.
pub(super) fn fetch(source: &KeySource) -> Result<Option<Key>, String> {
    match source {
        KeySource::None => Ok(None),
        KeySource::Env(var) => from_env(var, std::env::var_os(var)).map(Some),
        KeySource::Command(cmd) => {
            let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(key) = kept.get(cmd) {
                return Ok(Some(key.clone()));
            }
            let key = from_command(cmd)?;
            kept.insert(cmd.clone(), key.clone());
            Ok(Some(key))
        }
    }
}

/// Drops a command's kept key while it is still `sent`, the one refused, so the next `fetch` runs
/// the command again; a key fetched since is kept.
pub(super) fn forget(source: &KeySource, sent: &Key) {
    if let KeySource::Command(cmd) = source {
        let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
        if kept.get(cmd) == Some(sent) {
            kept.remove(cmd);
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

fn from_command(cmd: &str) -> Result<Key, String> {
    let argv = ["sh".to_string(), "-c".to_string(), cmd.to_string()];
    // Only the failure's message is shown: a command that fails after printing part of a key must
    // not put that part in an error. Nor is sh's own for a program it cannot find (127), which
    // names the program: a key written in place of the command is that program.
    let out = run(&argv, "", KEY_TIMEOUT, spawn).map_err(|e| match e.code {
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

/// How a key command starts: with no terminal, and without `GPG_TTY`, which would have gpg-agent
/// put pinentry-curses on subcheck's terminal; the agent can still use the terminal it started
/// from.
fn spawn(cmd: &mut Command) -> io::Result<(Child, Registered)> {
    group::spawn_without_terminal(cmd.env_remove("GPG_TTY"))
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt;
    use std::thread;

    use super::*;

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
        assert_eq!(fetch(&KeySource::Env("HOME".into())), Ok(Some(Key(home))));
        assert_eq!(fetch(&KeySource::None), Ok(None));
    }

    #[test]
    fn a_key_command_runs_once_a_process_until_its_key_is_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let cmd = KeySource::Command(format!("echo x >> '{}'; printf ' sk-2\n'", count.display()));
        let runs = || std::fs::read_to_string(&count).unwrap().lines().count();
        assert_eq!(fetch(&cmd), Ok(Some(Key("sk-2".into()))));
        assert_eq!(fetch(&cmd), Ok(Some(Key("sk-2".into()))));
        assert_eq!(runs(), 1);
        // A refused key fetched before the kept one leaves it kept.
        forget(&cmd, &Key("sk-1".into()));
        fetch(&cmd).unwrap();
        assert_eq!(runs(), 1);
        forget(&cmd, &Key("sk-2".into()));
        fetch(&cmd).unwrap();
        assert_eq!(runs(), 2);
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
            let both = [s.spawn(|| fetch(&cmd)), s.spawn(|| fetch(&cmd))];
            both.map(|t| t.join().unwrap()).into()
        });
        assert_eq!(fetched, vec![Ok(Some(Key("sk-6".into()))); 2]);
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 1);
    }

    #[test]
    fn a_key_command_that_fails_says_its_stderr_and_never_its_stdout() {
        let failed =
            KeySource::Command("echo sk-partial; echo 'gpg: decryption failed' >&2; exit 2".into());
        assert_eq!(
            fetch(&failed),
            Err("api_key_cmd failed: sh exited with exit status: 2: gpg: decryption failed".into())
        );
        assert_eq!(
            fetch(&KeySource::Command("true".into())),
            Err("api_key_cmd printed no key".into())
        );
        let many = fetch(&KeySource::Command("printf 'sk-3\\nuser: me\\n'".into())).unwrap_err();
        assert_eq!(
            many,
            "api_key_cmd printed more than one line: make it print the key alone"
        );
        for control in ["printf 'sk\\t-7'", "printf '\\033[1msk-7\\033[0m'"] {
            assert_eq!(
                fetch(&KeySource::Command(control.into())),
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
                fetch(&KeySource::Command(pasted.into())),
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
        let k = fetch(&KeySource::Command(
            "if [ -t 0 ]; then echo tty; else cat; echo sk-4; fi".into(),
        ));
        assert_eq!(k, Ok(Some(Key("sk-4".into()))));
    }

    #[test]
    fn a_key_command_has_no_terminal_and_no_gpg_tty() {
        // Its own session, which a run with no terminal (CI) can tell apart.
        let (pid, session) = crate::test_support::a_childs_session(spawn);
        assert_eq!(session, Ok(pid));
        // In subcheck's session the terminal opens, and a read of it stops the command.
        let k = fetch(&KeySource::Command(
            "if (: </dev/tty) 2>/dev/null; then echo tty; else echo sk-8; fi".into(),
        ));
        assert_eq!(k, Ok(Some(Key("sk-8".into()))));
        let mut sh = Command::new("sh");
        sh.args(["-c", "echo \"${GPG_TTY:-unset}\""])
            .env("GPG_TTY", "/dev/pts/9")
            .stdout(std::process::Stdio::piped());
        let (child, _running) = spawn(&mut sh).unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "unset\n");
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
