//! Review's jobs: the scan, the recheck and apply, each on a thread of its own, sending what
//! happens to the channel `update`'s inputs come in on (the terminal's loop's, or a session's), as
//! an `Input` or a message of that loop's own that one becomes. The runner starts them for
//! `update`'s tasks, and a UI that opens on a scan starts that one itself. Each runs to its end or
//! the process's: quitting with a job running ends its LLM calls or mkvmerge through `group::end`,
//! and its threads with the process.

use std::any::Any;
use std::ffi::OsStr;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use super::update::Input;
use crate::apply::{self, Mode};
use crate::scan::{self, Options, Plan};
use crate::store::{Run, Store};

/// Run `plan` on a thread of its own, saving through `store`, a connection of its own: each step
/// sent as `Input::Scan`, then how it ended as `Input::ScanEnded`. What `scan` would log goes
/// nowhere: the review has each file's lines from the scan's events. A panic is `Input::Fatal`.
pub fn scan<T: From<Input> + Send + 'static>(
    plan: Plan,
    opts: Options,
    store: Store,
    tx: Sender<T>,
) -> io::Result<()> {
    check(plan, opts, store, tx, false)
}

/// Run `plan` as [`scan()`] does, on a thread named `recheck`, readying `opts.backend` there first
/// ([`Backend::ready`](crate::llm::Backend::ready)): a server's key fetched and the server asked
/// what it serves; then sizing it ([`Backend::sized`](crate::llm::Backend::sized)), which has
/// Ollama load a model it has not. An error of either ends the job as `Input::ScanEnded(Err(..))`
/// with nothing checked. A warning of the server's context ([`Options::size_warning`]) is sent as
/// `Input::Warned` before the scan's first step: a recheck sizes its chunks by the entry and the
/// server as they are now, never by a `--chunk-lines` the run's own scan had, so that scan may not
/// have said it. Readying's own warnings are dropped, as the scan that made the run said
/// them; one it did not say (its entry has changed since) goes unseen.
pub fn recheck<T: From<Input> + Send + 'static>(
    plan: Plan,
    opts: Options,
    store: Store,
    tx: Sender<T>,
) -> io::Result<()> {
    check(plan, opts, store, tx, true)
}

/// [`scan()`], or with `ready_first` [`recheck()`].
fn check<T: From<Input> + Send + 'static>(
    plan: Plan,
    mut opts: Options,
    mut store: Store,
    tx: Sender<T>,
    ready_first: bool,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name(if ready_first { "recheck" } else { "scan" }.into())
        .spawn(move || {
            let report = |event: scan::Event| {
                let _ = tx.send(Input::Scan(event).into());
            };
            let ran = panic::catch_unwind(AssertUnwindSafe(|| {
                if ready_first {
                    opts.backend = opts.backend.ready()?.0.sized()?;
                    if let Some(w) = opts.size_warning() {
                        let _ = tx.send(Input::Warned(format!("warning: {w}")).into());
                    }
                }
                scan::execute(plan, &opts, &mut store, &mut io::sink(), &report)
                    .map_err(|e| format!("{e:#}"))
            }));
            let last = match ran {
                Ok(ended) => Input::ScanEnded(ended),
                Err(panic) => {
                    Input::Fatal(format!("the scan panicked: {}", panic_text(panic.as_ref())))
                }
            };
            let _ = tx.send(last.into());
        })?;
    Ok(())
}

/// Apply `run`'s accepted fixes on a thread of their own (with `dry_run`, say what would change
/// and write nothing), as a bare `subcheck apply` does: the original kept as `.orig`, the quick
/// check. Saving through `store`, a connection of its own; the fixed tracks written under `state`
/// before the remux. Each step is sent as `Input::Apply`, then how it ended as
/// `Input::ApplyEnded`. What `apply` prints goes nowhere: the review has each file's block from
/// the apply's events. A panic is `Input::Fatal`.
pub fn apply<T: From<Input> + Send + 'static>(
    run: Run,
    dry_run: bool,
    karaoke: bool,
    mut store: Store,
    state: PathBuf,
    tx: Sender<T>,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name("apply".into())
        .spawn(move || {
            let report = |event: apply::Event| {
                let _ = tx.send(Input::Apply(event).into());
            };
            let mode = if dry_run {
                Mode::DryRun
            } else {
                Mode::Apply {
                    backup: true,
                    full: false,
                }
            };
            let ran = panic::catch_unwind(AssertUnwindSafe(|| {
                apply::run(
                    &run,
                    &mut store,
                    &state,
                    &mode,
                    karaoke,
                    OsStr::new("mkvmerge"),
                    &mut io::sink(),
                    &report,
                )
            }));
            let last = match ran {
                Ok(ended) => Input::ApplyEnded(ended.map_err(|e| format!("{e:#}"))),
                Err(panic) => {
                    Input::Fatal(format!("apply panicked: {}", panic_text(panic.as_ref())))
                }
            };
            let _ = tx.send(last.into());
        })?;
    Ok(())
}

/// What a panic said, for a thread that ends on one with `Input::Fatal`.
pub fn panic_text(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".into())
}
