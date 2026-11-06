//! Review's jobs: the scan and apply, each on a thread of its own, sending what happens to the
//! UI's channel as an `Input` or the UI's own message that one becomes. The runner starts them for
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
    mut store: Store,
    tx: Sender<T>,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name("scan".into())
        .spawn(move || {
            let report = |event: scan::Event| {
                let _ = tx.send(Input::Scan(event).into());
            };
            let ran = panic::catch_unwind(AssertUnwindSafe(|| {
                scan::execute(plan, &opts, &mut store, &mut io::sink(), &report)
            }));
            let last = match ran {
                Ok(ended) => Input::ScanEnded(ended.map_err(|e| format!("{e:#}"))),
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
