//! The login shell's environment, for a process macOS itself started.
//!
//! An app the Finder, the Dock or `open` starts is a child of launchd and gets launchd's
//! environment, not the one the user's shell builds: no Homebrew directories on `PATH`, and none
//! of the `XDG_*`, `SUBCHECK_*` or other variables the user's startup files set, so subcheck
//! would look for its store, its settings and its tools in other places than it does from a
//! terminal. Such a process runs the user's login shell once, as it starts, and takes the
//! environment the shell prints over launchd's (README, "The macOS app").
//!
//! The variables are set before any other thread exists: setting the environment is not safe while
//! another thread may read it, which is why Rust makes `env::set_var` unsafe. So the read starts
//! no thread of its own either, and reads the shell's output on the thread that calls it.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, IsTerminal, Read};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use rustix::process::{Pid, Signal, kill_process_group};

/// How long the shell has to print its environment: long enough for slow startup files, short
/// enough that one that never ends (a tmux attach, a question asked on the terminal) holds the
/// window back only that long, and `REAP_GRACE` more.
pub const READ_LIMIT: Duration = Duration::from_secs(5);

/// How long a shell that has printed its environment, or was killed at the limit, has to be reaped
/// before it is left unreaped. One that has printed ends at once unless what it runs as it exits
/// (a slow `.zlogout`, say) holds it, which is the user's to wait for, not the window's. A killed
/// process dies at once unless it is in an uninterruptible wait (a startup file reading a dead
/// network mount, say), and then only once that wait ends, which may be never: the window must not
/// wait with it.
const REAP_GRACE: Duration = Duration::from_millis(100);

/// The variables [`kept`] drops by name: those that describe the shell or the terminal it ran in
/// rather than the user's setup, and those that would point a library the app carries at code built
/// for another copy of that library (Homebrew's, say), whose version can differ from the bundled
/// one, so that a Finder launch would break the bundle: Qt's plugin and QML paths, glib's GIO
/// modules, LuaJIT's Lua paths and init code, and OpenSSL's modules, engines and config (a config
/// names modules and engines). Code a bundled library loads through an interface of its own,
/// independent of the library's version (a Vulkan driver or layer, a JVM, libaacs), is found as a
/// terminal launch finds it, so its variables are kept: dropping them would make the app differ
/// from the terminal's, and this list is not a sandbox for every plugin channel.
const DROPPED: &[&str] = &[
    "PWD",
    "OLDPWD",
    "SHLVL",
    "_",
    "TERM",
    "COLORTERM",
    "TERMINFO",
    "TMUX",
    "TMUX_PANE",
    "STY",
    "SSH_CLIENT",
    "SSH_CONNECTION",
    "SSH_TTY",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    "QT_PLUGIN_PATH",
    "QT_QPA_PLATFORM_PLUGIN_PATH",
    "QT_QPA_PLATFORM",
    "QML_IMPORT_PATH",
    "QML2_IMPORT_PATH",
    "QT_QUICK_CONTROLS_STYLE",
    "GIO_EXTRA_MODULES",
    "GIO_MODULE_DIR",
    "LUA_PATH",
    "LUA_CPATH",
    "LUA_INIT",
    "OPENSSL_MODULES",
    "OPENSSL_ENGINES",
    "OPENSSL_CONF",
];

/// And by prefix: terminals' own, those launchd sets to describe the process it started (`XPC_`,
/// `__CF`), and the dynamic loader's (`DYLD_`), which would load another copy of a library the
/// bundle carries.
const DROPPED_PREFIXES: &[&str] = &[
    "TERM_", "ITERM_", "GHOSTTY_", "KITTY_", "WEZTERM_", "XPC_", "__CF", "DYLD_",
];

/// Why the login shell's environment could not be read.
#[derive(Debug)]
pub struct Unread {
    pub shell: PathBuf,
    pub why: Why,
}

/// Which way the read failed.
#[derive(Debug)]
pub enum Why {
    /// The shell could not be started.
    Start(io::Error),
    /// The shell printed no environment within this limit, and was killed with its group.
    Limit(Duration),
    /// The shell ended without printing one, with this status, `None` when a signal ended it.
    Status(Option<i32>),
    /// The shell exited 0 without printing one.
    Nothing,
}

impl fmt::Display for Unread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not read the login shell's environment: {} ",
            self.shell.display()
        )?;
        match &self.why {
            Why::Start(e) => write!(f, "did not start: {e}"),
            Why::Limit(limit) => write!(f, "gave none within {} s", limit.as_secs_f64()),
            Why::Status(Some(code)) => write!(f, "exited with status {code}"),
            Why::Status(None) => f.write_str("ended by a signal"),
            Why::Nothing => f.write_str("printed none"),
        }
    }
}

/// Whether macOS itself started this process (the Finder, the Dock, `open`): its parent is
/// launchd, pid 1, and its stdin is not a terminal. A process a shell started keeps the
/// environment that shell gave it.
pub fn started_by_launchd(parent: u32, stdin_is_terminal: bool) -> bool {
    parent == 1 && !stdin_is_terminal
}

/// The shell to read: `$SHELL` when it is an absolute path, else zsh, macOS's default. A bare
/// name would be looked up on launchd's `PATH`, the one the read is there to replace.
pub fn login_shell(var: Option<&OsStr>) -> PathBuf {
    match var.map(Path::new) {
        Some(shell) if shell.is_absolute() => shell.to_owned(),
        _ => PathBuf::from("/bin/zsh"),
    }
}

/// Whether a variable the shell printed is taken over this process's: all but those `DROPPED`
/// and `DROPPED_PREFIXES` name.
pub fn kept(name: &OsStr) -> bool {
    let name = name.as_bytes();
    !DROPPED.iter().any(|dropped| name == dropped.as_bytes())
        && !DROPPED_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix.as_bytes()))
}

/// The variables `/usr/bin/env -0` printed between the first `marker` in `output` and the next
/// one: what a startup file printed around them is not among them. Each entry ends in a NUL, so a
/// value may hold a newline or an `=`; it is cut at its first `=`, and one with no `=` or an
/// empty name is dropped. `None` unless both markers are there, and for an empty `marker`, which
/// marks nothing.
pub fn parse(output: &[u8], marker: &[u8]) -> Option<Vec<(OsString, OsString)>> {
    if marker.is_empty() {
        return None;
    }
    let vars = between(output, marker)?
        .split(|&b| b == 0)
        .filter_map(|entry| {
            let eq = entry.iter().position(|&b| b == b'=')?;
            (eq > 0).then(|| {
                (
                    OsStr::from_bytes(&entry[..eq]).to_owned(),
                    OsStr::from_bytes(&entry[eq + 1..]).to_owned(),
                )
            })
        })
        .collect();
    Some(vars)
}

/// The environment `shell` builds as an interactive login shell, every variable it printed
/// ([`kept`] says which to take). Interactive, because a `PATH` is often set in `~/.zshrc`
/// rather than in `~/.zprofile`. Its stdin is /dev/null, so that a startup file that reads waits
/// for nothing; and it leads a process group of its own, so that what its startup files started
/// is killed with it should it print nothing within `limit`. Once it has printed, nothing is
/// killed and nothing more is waited for: what its startup files started, and what it runs as it
/// exits, is the user's.
pub fn read(shell: &Path, limit: Duration) -> Result<Vec<(OsString, OsString)>, Unread> {
    let unread = |why| Unread {
        shell: shell.to_owned(),
        why,
    };
    let deadline = Instant::now() + limit;
    let marker = marker();
    // Not group::spawn, which starts its signal thread: see the module's doc.
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &command(&marker)])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| unread(Why::Start(e)))?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let out = read_to_marker(stdout, marker.as_bytes(), deadline);
    if let Some(vars) = parse(&out, marker.as_bytes()) {
        let _ = exit_by(&mut child, Instant::now() + REAP_GRACE);
        return Ok(vars);
    }
    match exit_by(&mut child, deadline) {
        None => {
            // The group, for what the startup files started; the shell too, should it have left
            // the group.
            let _ = kill_process_group(Pid::from_child(&child), Signal::KILL);
            let _ = child.kill();
            let _ = exit_by(&mut child, Instant::now() + REAP_GRACE);
            Err(unread(Why::Limit(limit)))
        }
        Some(status) if status.success() => Err(unread(Why::Nothing)),
        Some(status) => Err(unread(Why::Status(status.code()))),
    }
}

/// Take the login shell's environment over this process's when macOS itself started it, and
/// nothing otherwise, nor off macOS. A variable launchd set that the shell did not print keeps
/// launchd's value. `Some` says why the environment could not be read, which the window shows:
/// an app the Finder started has no stderr anyone reads.
///
/// # Safety
///
/// No other thread may exist: it sets environment variables, which another thread could be
/// reading.
pub unsafe fn import_login_environment() -> Option<String> {
    if !cfg!(target_os = "macos")
        || !started_by_launchd(
            std::os::unix::process::parent_id(),
            io::stdin().is_terminal(),
        )
    {
        return None;
    }
    match read(&login_shell(env::var_os("SHELL").as_deref()), READ_LIMIT) {
        Ok(vars) => {
            for (name, value) in vars.into_iter().filter(|(name, _)| kept(name)) {
                // SAFETY: the caller's, that no other thread exists.
                unsafe { env::set_var(name, value) };
            }
            None
        }
        Err(e) => Some(e.to_string()),
    }
}

/// A marker no startup file prints by chance: this process's id and the time.
fn marker() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("subcheck-env-{}-{nanos}", std::process::id())
}

/// What the shell runs: its environment between two `marker`s, the one command sh, bash, zsh
/// and fish 3 or later all run alike (fish 2 has no `&&`). `env -0` ends each entry in a NUL, as a
/// value may hold a newline. Joined with `&&`, so that an `env` that fails leaves no closing
/// marker and the read fails, instead of importing an empty environment.
fn command(marker: &str) -> String {
    format!("printf '%s' {marker} && /usr/bin/env -0 && printf '%s' {marker}")
}

/// The part of `output` between the first `marker` and the next one.
fn between<'a>(output: &'a [u8], marker: &[u8]) -> Option<&'a [u8]> {
    let start = find(output, marker)? + marker.len();
    let len = find(&output[start..], marker)?;
    Some(&output[start..start + len])
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// What the shell printed, read until both markers are in it, the pipe ends or `deadline`
/// passes. Not until the pipe ends once the markers are in: a startup file's background process
/// holds its copy of the pipe open for as long as it runs, which may be for good.
fn read_to_marker(mut stdout: ChildStdout, marker: &[u8], deadline: Instant) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chunk = vec![0; 1 << 16];
    while between(&out, marker).is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match readable(&stdout, left) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
        match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    out
}

/// Whether `stdout` has bytes to read, or has ended, within `left`.
fn readable(stdout: &ChildStdout, left: Duration) -> io::Result<bool> {
    let mut fd = libc::pollfd {
        fd: stdout.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // Rounded up, so that less than a millisecond left waits rather than spins.
    let ms = libc::c_int::try_from(left.as_nanos().div_ceil(1_000_000)).unwrap_or(libc::c_int::MAX);
    // SAFETY: one pollfd, alive for the call.
    match unsafe { libc::poll(&raw mut fd, 1, ms) } {
        -1 => Err(io::Error::last_os_error()),
        ready => Ok(ready > 0),
    }
}

/// The shell's exit status, should it end by `deadline`; checked every few milliseconds, as
/// waiting with a limit takes a thread or a signal handler otherwise.
fn exit_by(child: &mut Child, deadline: Instant) -> Option<ExitStatus> {
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        thread::sleep(left.min(Duration::from_millis(5)));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_support::{script, tries};

    /// A fake shell in `dir`, a script that runs `body`; `read` gives it `-l -i -c CMD` as `$1`
    /// to `$4`.
    fn fake_shell(dir: &Path, body: &str) -> PathBuf {
        script(dir, "shell", body)
    }

    /// The state /proc gives the process `pid` (a pid file's text), `None` once it is gone.
    #[cfg(target_os = "linux")]
    fn state(pid: &str) -> Option<String> {
        let stat = fs::read_to_string(format!("/proc/{}/stat", pid.trim())).ok()?;
        let after_name = stat.rsplit(')').next()?;
        after_name.split_whitespace().next().map(str::to_owned)
    }

    #[test]
    fn started_by_launchd_only_for_pid_1_without_a_terminal() {
        assert!(started_by_launchd(1, false));
        assert!(!started_by_launchd(1, true));
        assert!(!started_by_launchd(4242, false));
    }

    #[test]
    fn login_shell_takes_an_absolute_shell_else_zsh() {
        assert_eq!(
            login_shell(Some(OsStr::new("/usr/local/bin/fish"))),
            Path::new("/usr/local/bin/fish")
        );
        for var in [None, Some(OsStr::new("")), Some(OsStr::new("fish"))] {
            assert_eq!(login_shell(var), Path::new("/bin/zsh"));
        }
    }

    #[test]
    fn parse_takes_what_is_between_the_markers() {
        let out = b"rc says hi\nMARKA=1\0B=x=y\0C=line\nnext\0MARK\nbye";
        assert_eq!(
            parse(out, b"MARK").unwrap(),
            vec![
                ("A".into(), "1".into()),
                ("B".into(), "x=y".into()),
                ("C".into(), "line\nnext".into())
            ]
        );
    }

    #[test]
    fn parse_skips_entries_without_a_name() {
        assert_eq!(
            parse(b"MARK=x\0noequals\0D=4\0MARK", b"MARK").unwrap(),
            vec![("D".into(), "4".into())]
        );
    }

    #[test]
    fn parse_without_both_markers_is_none() {
        assert!(parse(b"A=1\0", b"MARK").is_none());
        assert!(parse(b"MARKA=1\0", b"MARK").is_none());
    }

    #[test]
    fn parse_with_an_empty_marker_is_none() {
        assert!(parse(b"A=1\0", b"").is_none());
    }

    #[test]
    fn kept_drops_the_shells_own_and_library_code_variables() {
        for name in [
            "PWD",
            "OLDPWD",
            "SHLVL",
            "_",
            "TERM",
            "TERM_PROGRAM",
            "COLORTERM",
            "TERMINFO",
            "TMUX",
            "TMUX_PANE",
            "STY",
            "SSH_CLIENT",
            "SSH_CONNECTION",
            "SSH_TTY",
            "LC_TERMINAL",
            "ITERM_SESSION_ID",
            "GHOSTTY_RESOURCES_DIR",
            "KITTY_WINDOW_ID",
            "WEZTERM_PANE",
            "XPC_SERVICE_NAME",
            "__CFBundleIdentifier",
            "DYLD_LIBRARY_PATH",
            "DYLD_INSERT_LIBRARIES",
            "QT_PLUGIN_PATH",
            "QT_QPA_PLATFORM_PLUGIN_PATH",
            "QT_QPA_PLATFORM",
            "QML_IMPORT_PATH",
            "QML2_IMPORT_PATH",
            "QT_QUICK_CONTROLS_STYLE",
            "GIO_EXTRA_MODULES",
            "GIO_MODULE_DIR",
            "LUA_PATH",
            "LUA_CPATH",
            "LUA_INIT",
            "OPENSSL_MODULES",
            "OPENSSL_ENGINES",
            "OPENSSL_CONF",
        ] {
            assert!(!kept(OsStr::new(name)), "{name}");
        }
        for name in [
            "PATH",
            "XDG_STATE_HOME",
            "SUBCHECK_DB",
            "OLLAMA_HOST",
            "CLAUDE_CONFIG_DIR",
            "SSH_AUTH_SOCK",
            "TMUXCONFIG",
            "HOME",
            "LANG",
            "VK_ICD_FILENAMES",
            "VK_DRIVER_FILES",
            "VK_LAYER_PATH",
            "VK_INSTANCE_LAYERS",
            "JAVA_HOME",
            "LIBAACS_PATH",
        ] {
            assert!(kept(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn read_runs_an_interactive_login_shell() {
        let dir = tempfile::tempdir().unwrap();
        let args = dir.path().join("args");
        let shell = fake_shell(
            dir.path(),
            &format!(
                "printf '%s %s %s' \"$1\" \"$2\" \"$3\" > '{}'\nexport FROM_RC=yes\nexec /bin/sh -c \"$4\"\n",
                args.display()
            ),
        );
        let vars = read(&shell, Duration::from_secs(5)).unwrap();
        assert_eq!(fs::read_to_string(&args).unwrap(), "-l -i -c");
        assert!(vars.contains(&("FROM_RC".into(), "yes".into())));
    }

    // A try is spoiled when the machine stalls the read past 2 s, which the read that waits for the
    // pipe to end takes every time, as the background sleep holds it open for 3 s.
    #[test]
    fn read_ignores_rc_noise_and_stdin() {
        tries(|| {
            let dir = tempfile::tempdir().unwrap();
            let shell = fake_shell(
                dir.path(),
                "echo noise\nread line\nsleep 3 &\nexec /bin/sh -c \"$4\"\n",
            );
            let started = Instant::now();
            let vars = read(&shell, Duration::from_secs(5));
            if started.elapsed() >= Duration::from_secs(2) {
                return false;
            }
            assert!(vars.is_ok(), "{vars:?}");
            true
        });
    }

    // What the startup files started is the user's: a read that succeeds kills none of it.
    #[cfg(target_os = "linux")]
    #[test]
    fn read_leaves_the_shells_background_processes_alone() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let shell = fake_shell(
            dir.path(),
            &format!(
                "sleep 3 &\necho $! > '{}'\nexec /bin/sh -c \"$4\"\n",
                pidfile.display()
            ),
        );
        assert!(read(&shell, Duration::from_secs(5)).is_ok());
        let pid = fs::read_to_string(&pidfile).unwrap();
        let running = state(&pid);
        // Not left behind for the rest of the run, now that it has been seen.
        let raw = pid.trim().parse().unwrap();
        let _ = rustix::process::kill_process(Pid::from_raw(raw).unwrap(), Signal::KILL);
        assert!(
            !matches!(running.as_deref(), None | Some("Z" | "X")),
            "sleep not running: {running:?}"
        );
    }

    // A try is spoiled when the machine stalls the script so that the limit kills it before it
    // writes its pid file, or stalls the read past 2 s. A sleep left running fails any try that
    // has its pid.
    #[cfg(target_os = "linux")]
    #[test]
    fn read_stops_at_the_limit_and_kills_the_group() {
        tries(|| {
            let dir = tempfile::tempdir().unwrap();
            let pidfile = dir.path().join("pid");
            let shell = fake_shell(
                dir.path(),
                &format!("sleep 30 &\necho $! > '{}'\nwait\n", pidfile.display()),
            );
            let started = Instant::now();
            let err = read(&shell, Duration::from_millis(300)).unwrap_err();
            let took = started.elapsed();
            assert!(matches!(err.why, Why::Limit(_)), "{err:?}");
            assert_eq!(
                err.to_string(),
                format!(
                    "could not read the login shell's environment: {} gave none within 0.3 s",
                    shell.display()
                )
            );
            // Empty when the kill came between the file's making and the echo's write.
            let Some(pid) = fs::read_to_string(&pidfile)
                .ok()
                .filter(|pid| !pid.trim().is_empty())
            else {
                return false;
            };
            thread::sleep(Duration::from_millis(200));
            // Gone, or a zombie nobody has reaped yet (a container's PID 1 may not): either way no
            // longer running.
            let left = state(&pid);
            assert!(
                matches!(left.as_deref(), None | Some("Z" | "X")),
                "sleep still running: {left:?}"
            );
            took < Duration::from_secs(2)
        });
    }

    // What the shell does once it has printed, such as a slow logout file, is the user's: the read
    // neither waits for it nor kills it. A try is spoiled when the machine stalls the read past
    // 2 s, which the read that waits for the shell's exit takes every time, as the shell sleeps on
    // past the limit.
    #[test]
    fn read_returns_once_printed_and_leaves_a_shell_that_runs_on() {
        tries(|| {
            let dir = tempfile::tempdir().unwrap();
            let pidfile = dir.path().join("pid");
            let shell = fake_shell(
                dir.path(),
                &format!(
                    "echo $$ > '{}'\n/bin/sh -c \"$4\"\nsleep 30\n",
                    pidfile.display()
                ),
            );
            let started = Instant::now();
            let vars = read(&shell, Duration::from_secs(5));
            let took = started.elapsed();
            // The pid file is written before the shell prints, so a read that took the
            // environment finds it there.
            let Ok(pid) = fs::read_to_string(&pidfile) else {
                return false;
            };
            #[cfg(target_os = "linux")]
            let running = state(&pid);
            // Not left behind for the rest of the run, now that it has been seen: the shell leads
            // a group of its own, with its sleep in it.
            let raw = pid.trim().parse().unwrap();
            let _ = kill_process_group(Pid::from_raw(raw).unwrap(), Signal::KILL);
            if took >= Duration::from_secs(2) {
                return false;
            }
            assert!(vars.is_ok(), "{vars:?}");
            #[cfg(target_os = "linux")]
            assert!(
                !matches!(running.as_deref(), None | Some("Z" | "X")),
                "the shell was not left running: {running:?}"
            );
            true
        });
    }

    #[test]
    fn read_takes_an_environment_larger_than_a_pipe() {
        // More than a pipe holds (64 KiB), and less than the most Linux passes in one variable
        // (128 KiB): a longer one fails the exec.
        let big = 100_000;
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(
            dir.path(),
            &format!("export BIG={}\nexec /bin/sh -c \"$4\"\n", "x".repeat(big)),
        );
        let vars = read(&shell, Duration::from_secs(5)).unwrap();
        assert!(vars.iter().any(|(k, v)| k == "BIG" && v.len() == big));
    }

    #[test]
    fn read_of_a_shell_that_fails() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(dir.path(), "exit 3\n");
        let err = read(&shell, Duration::from_secs(5)).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "could not read the login shell's environment: {} exited with status 3",
                shell.display()
            )
        );
    }

    #[test]
    fn read_of_a_shell_a_signal_ends() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(dir.path(), "kill -KILL $$\n");
        let err = read(&shell, Duration::from_secs(5)).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "could not read the login shell's environment: {} ended by a signal",
                shell.display()
            )
        );
    }

    #[test]
    fn read_of_a_shell_that_prints_no_environment() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(dir.path(), "exit 0\n");
        let err = read(&shell, Duration::from_secs(5)).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "could not read the login shell's environment: {} printed none",
                shell.display()
            )
        );
    }

    #[test]
    fn read_of_a_shell_whose_env_fails_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        // Runs the command it is given with `/usr/bin/env -0` replaced by `false`.
        let shell = fake_shell(
            dir.path(),
            r#"eval "$(printf '%s' "$4" | sed 's|/usr/bin/env -0|false|')""#,
        );
        let err = read(&shell, Duration::from_secs(5)).unwrap_err();
        assert!(matches!(err.why, Why::Status(Some(1))), "{err}");
    }

    #[test]
    fn read_of_a_shell_that_is_not_there() {
        let err = read(Path::new("/nonexistent/zsh"), Duration::from_secs(5)).unwrap_err();
        assert!(
            err.to_string().starts_with(
                "could not read the login shell's environment: /nonexistent/zsh did not start: "
            ),
            "{err}"
        );
    }

    #[test]
    fn the_command_runs_in_sh_bash_zsh_and_fish() {
        let marker = marker();
        // Without -l and -i, so that the test machine's startup files stay out; zsh reads its
        // .zshenv and fish its config even for -c, so each is told not to.
        for argv in [
            &["/bin/sh"][..],
            &["bash"],
            &["zsh", "-f"],
            &["fish", "--no-config"],
        ] {
            let out = Command::new(argv[0])
                .args(&argv[1..])
                .args(["-c", &command(&marker)])
                .output();
            let out = match out {
                Err(e) if e.kind() == io::ErrorKind::NotFound && argv[0] != "/bin/sh" => {
                    eprintln!("skipped: {} not found", argv[0]);
                    continue;
                }
                out => out.unwrap(),
            };
            let vars = parse(&out.stdout, marker.as_bytes()).unwrap_or_else(|| {
                panic!(
                    "{}: no markers in {:?} (stderr {:?})",
                    argv[0],
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                )
            });
            assert!(vars.iter().any(|(name, _)| name == "PATH"), "{}", argv[0]);
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn import_login_environment_does_nothing_off_macos() {
        // SAFETY: off macOS it returns before it touches the environment.
        assert_eq!(unsafe { import_login_environment() }, None);
    }
}
