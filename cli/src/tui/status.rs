//! Review's status message as the terminal words it, naming the keys that answer it.

use subcheck_core::list::label;
use subcheck_core::review::progress::findings_text;
use subcheck_core::review::status::{Asked, Failure, Kept, Refusal, Status};
use subcheck_core::text::timestamp_ms;

/// `status` as the status line says it.
pub fn words(status: &Status) -> String {
    match status {
        Status::Decided {
            state,
            findings,
            replaced,
            kept,
        } => {
            let replacing = match replaced {
                0 => String::new(),
                n => format!(", replacing your wording on {n} (u undoes)"),
            };
            let kept = match kept {
                Some(Kept { findings, state }) => format!(", {findings} kept as {}", label(*state)),
                None => String::new(),
            };
            format!(
                "{} {}{replacing}{kept}",
                label(*state),
                findings_text(*findings)
            )
        }
        Status::Cleared { findings } => format!("cleared {}", findings_text(*findings)),
        Status::Undone { findings, labels } => {
            format!(
                "undone: {} · {}",
                findings_text(*findings),
                labels.join(", ")
            )
        }
        Status::Inherited {
            label: name,
            findings: 1,
            state,
        } => format!(
            "{name}: a new finding took its row's decision: {} (u undoes)",
            label(*state)
        ),
        Status::Inherited {
            label, findings, ..
        } => format!("{label}: {findings} new findings took their rows' decisions (u undoes)"),
        Status::Replaced { label, gone: true } => {
            format!("{label} was checked again; that finding is gone")
        }
        Status::Replaced { label, gone: false } => {
            format!("{label} was checked again; your decision went to its new finding")
        }
        Status::EditEnded { label } => format!("{label} was checked again; its finding changed"),
        Status::Refused(why) => refused(why),
        Status::QueuedAgain { label } => {
            format!("{label} will be checked again when the scan is done")
        }
        Status::NoSuchFile { label } => format!("{label}: no such file"),
        Status::OpeningView => "opening the view…".into(),
        Status::Opening { label, at_ms } => format!("opening {label} at {}", timestamp_ms(*at_ms)),
        Status::NotAnswering { label } => {
            format!("{label}: the share isn't answering, still waiting")
        }
        Status::StoppedWaiting { label } => format!("stopped waiting for {label}"),
        Status::Opened { paused: true } => "opened in mpv, paused".into(),
        Status::Opened { paused: false } => "opened in mpv, playing".into(),
        Status::Played(p) => format!(
            "{} {}",
            if p.paused { "⏸" } else { "▶" },
            timestamp_ms(p.at_ms)
        ),
        Status::Closed => "closed mpv".into(),
        Status::NotOpen {
            asked: Asked::Playback,
        } => "mpv is not open (o opens it)".into(),
        Status::NotOpen {
            asked: Asked::Open | Asked::Close,
        } => "mpv is not open".into(),
        Status::NoFile => "mpv has no file open (o opens one)".into(),
        Status::StillOpening => "mpv is still opening the file".into(),
        Status::CouldNotOpen { label } => format!("mpv could not open {label}"),
        Status::NoAnswer => "mpv did not answer".into(),
        Status::HungUp => "mpv closed the connection without answering".into(),
        Status::NoVideo => "the video pane could not show video".into(),
        Status::Failed { what, error } => match what {
            Failure::Save => format!("not saved: {error}"),
            Failure::Switch => format!("could not switch views: {error}"),
            Failure::ReadBack { label } => format!("could not read {label} back: {error}"),
            Failure::Mpv => format!("mpv: {error}"),
            Failure::Recheck | Failure::Player => error.clone(),
        },
        // The warnings are printed above the screen too, so `+n more` points at what the user can
        // scroll up to.
        Status::Warned { first, more: 0 } => first.clone(),
        Status::Warned { first, more } => format!("{first} · +{more} more"),
    }
}

/// Why an action did nothing, as the status line says it.
fn refused(why: &Refusal) -> String {
    match why {
        Refusal::SelectionStays => "that finding changed; the selection stays".into(),
        Refusal::NotInView { label } => format!("{label} is not in this view"),
        Refusal::TimingFix => "a timing fix is accepted or rejected, not reworded".into(),
        Refusal::EmptyWording => "the wording cannot be empty (Esc cancels)".into(),
        Refusal::EditOutOfDate => "the edit is out of date; nothing was decided".into(),
        Refusal::Already { state } => format!("already {}", label(*state)),
        Refusal::NothingToClear => "nothing to clear".into(),
        Refusal::NothingToUndo => "nothing to undo".into(),
        Refusal::LastFile => "last file".into(),
        Refusal::FirstFile => "first file".into(),
        Refusal::FileMissing { path } => format!("file missing: {path}"),
        Refusal::NothingToOpen => "nothing to open: no line of this track moves".into(),
        Refusal::ScanRunning => "the scan is still running".into(),
        Refusal::ApplyRunning => "apply is still running".into(),
        Refusal::AlreadyApplying => "already applying".into(),
        Refusal::NoAcceptedFixes => "no accepted fixes".into(),
        Refusal::BeingChecked { label } => format!("{label} is being checked now"),
        Refusal::NoFindingsShown { label } => format!("no findings of {label} shown"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use subcheck_core::player::PlayState;
    use subcheck_core::store::State;

    // Every form of every message, with values made up and unlike each other, so that a word
    // taken from the wrong field shows; one row each, so that a change to any wording fails here.
    #[test]
    #[allow(clippy::too_many_lines)] // one row per form
    fn every_form_of_every_message_has_its_words() {
        let label = |s: &str| s.to_string();
        let kept = |findings, state| Some(Kept { findings, state });
        let failed = |what, error: &str| Status::Failed {
            what,
            error: error.into(),
        };
        let refused = Status::Refused;
        for (status, said) in [
            (
                Status::Decided {
                    state: State::Accepted,
                    findings: 3,
                    replaced: 0,
                    kept: None,
                },
                "accepted 3 findings",
            ),
            (
                Status::Decided {
                    state: State::Rejected,
                    findings: 1,
                    replaced: 0,
                    kept: None,
                },
                "rejected 1 finding",
            ),
            (
                Status::Decided {
                    state: State::Accepted,
                    findings: 1,
                    replaced: 1,
                    kept: None,
                },
                "accepted 1 finding, replacing your wording on 1 (u undoes)",
            ),
            (
                Status::Decided {
                    state: State::Accepted,
                    findings: 2,
                    replaced: 0,
                    kept: kept(1, State::Rejected),
                },
                "accepted 2 findings, 1 kept as rejected",
            ),
            (
                Status::Decided {
                    state: State::Accepted,
                    findings: 4,
                    replaced: 2,
                    kept: kept(3, State::Pending),
                },
                "accepted 4 findings, replacing your wording on 2 (u undoes), 3 kept as pending",
            ),
            (Status::Cleared { findings: 3 }, "cleared 3 findings"),
            (Status::Cleared { findings: 1 }, "cleared 1 finding"),
            (
                Status::Undone {
                    findings: 1,
                    labels: vec![label("E04")],
                },
                "undone: 1 finding · E04",
            ),
            (
                Status::Undone {
                    findings: 2,
                    labels: vec![label("E05"), label("E04")],
                },
                "undone: 2 findings · E05, E04",
            ),
            (
                Status::Inherited {
                    label: label("E04"),
                    findings: 1,
                    state: State::Accepted,
                },
                "E04: a new finding took its row's decision: accepted (u undoes)",
            ),
            (
                Status::Inherited {
                    label: label("E06"),
                    findings: 2,
                    state: State::Rejected,
                },
                "E06: 2 new findings took their rows' decisions (u undoes)",
            ),
            (
                Status::Replaced {
                    label: label("E02"),
                    gone: false,
                },
                "E02 was checked again; your decision went to its new finding",
            ),
            (
                Status::Replaced {
                    label: label("E03"),
                    gone: true,
                },
                "E03 was checked again; that finding is gone",
            ),
            (
                Status::EditEnded {
                    label: label("E07"),
                },
                "E07 was checked again; its finding changed",
            ),
            (
                refused(Refusal::SelectionStays),
                "that finding changed; the selection stays",
            ),
            (
                refused(Refusal::NotInView {
                    label: label("E08"),
                }),
                "E08 is not in this view",
            ),
            (
                refused(Refusal::TimingFix),
                "a timing fix is accepted or rejected, not reworded",
            ),
            (
                refused(Refusal::EmptyWording),
                "the wording cannot be empty (Esc cancels)",
            ),
            (
                refused(Refusal::EditOutOfDate),
                "the edit is out of date; nothing was decided",
            ),
            (
                refused(Refusal::Already {
                    state: State::Rejected,
                }),
                "already rejected",
            ),
            (
                refused(Refusal::Already {
                    state: State::Accepted,
                }),
                "already accepted",
            ),
            (refused(Refusal::NothingToClear), "nothing to clear"),
            (refused(Refusal::NothingToUndo), "nothing to undo"),
            (refused(Refusal::LastFile), "last file"),
            (refused(Refusal::FirstFile), "first file"),
            (
                refused(Refusal::FileMissing {
                    path: label("/m/s1/E09.mkv"),
                }),
                "file missing: /m/s1/E09.mkv",
            ),
            (
                refused(Refusal::NothingToOpen),
                "nothing to open: no line of this track moves",
            ),
            (refused(Refusal::ScanRunning), "the scan is still running"),
            (refused(Refusal::ApplyRunning), "apply is still running"),
            (refused(Refusal::AlreadyApplying), "already applying"),
            (refused(Refusal::NoAcceptedFixes), "no accepted fixes"),
            (
                refused(Refusal::BeingChecked {
                    label: label("E10"),
                }),
                "E10 is being checked now",
            ),
            (
                refused(Refusal::NoFindingsShown {
                    label: label("E11"),
                }),
                "no findings of E11 shown",
            ),
            (
                Status::QueuedAgain {
                    label: label("E12"),
                },
                "E12 will be checked again when the scan is done",
            ),
            (
                Status::NoSuchFile {
                    label: label("E13"),
                },
                "E13: no such file",
            ),
            (Status::OpeningView, "opening the view…"),
            (
                Status::Opening {
                    label: label("E14"),
                    at_ms: 165_060,
                },
                "opening E14 at 0:02:45.060",
            ),
            (
                Status::Opening {
                    label: label("E17"),
                    at_ms: -1_500,
                },
                "opening E17 at 0:00:00.000",
            ),
            (
                Status::NotAnswering {
                    label: label("E18"),
                },
                "E18: the share isn't answering, still waiting",
            ),
            (
                Status::StoppedWaiting {
                    label: label("E19"),
                },
                "stopped waiting for E19",
            ),
            (Status::Opened { paused: true }, "opened in mpv, paused"),
            (Status::Opened { paused: false }, "opened in mpv, playing"),
            (
                Status::Played(PlayState {
                    paused: true,
                    at_ms: 171_060,
                }),
                "⏸ 0:02:51.060",
            ),
            (
                Status::Played(PlayState {
                    paused: false,
                    at_ms: 1_000,
                }),
                "▶ 0:00:01.000",
            ),
            (
                Status::Played(PlayState {
                    paused: false,
                    at_ms: 3_723_004,
                }),
                "▶ 1:02:03.004",
            ),
            (Status::Closed, "closed mpv"),
            (
                Status::NotOpen {
                    asked: Asked::Playback,
                },
                "mpv is not open (o opens it)",
            ),
            (Status::NotOpen { asked: Asked::Open }, "mpv is not open"),
            (
                Status::NotOpen {
                    asked: Asked::Close,
                },
                "mpv is not open",
            ),
            (Status::NoFile, "mpv has no file open (o opens one)"),
            (Status::StillOpening, "mpv is still opening the file"),
            (
                Status::CouldNotOpen {
                    label: label("E15"),
                },
                "mpv could not open E15",
            ),
            (Status::NoAnswer, "mpv did not answer"),
            (
                Status::HungUp,
                "mpv closed the connection without answering",
            ),
            (Status::NoVideo, "the video pane could not show video"),
            (failed(Failure::Save, "disk full"), "not saved: disk full"),
            (
                failed(Failure::Switch, "no run named nope"),
                "could not switch views: no run named nope",
            ),
            (
                failed(
                    Failure::ReadBack {
                        label: label("E16"),
                    },
                    "database is locked",
                ),
                "could not read E16 back: database is locked",
            ),
            (
                failed(Failure::Recheck, "run r: no LLM is called gpt"),
                "run r: no LLM is called gpt",
            ),
            (
                failed(Failure::Mpv, "invalid parameter"),
                "mpv: invalid parameter",
            ),
            (
                failed(
                    Failure::Player,
                    "mpv not found: install mpv to open findings",
                ),
                "mpv not found: install mpv to open findings",
            ),
            (
                Status::Warned {
                    first: label("warning: /m/locked: skipped: Permission denied (os error 13)"),
                    more: 3,
                },
                "warning: /m/locked: skipped: Permission denied (os error 13) · +3 more",
            ),
            (
                Status::Warned {
                    first: label("warning: /m/lost: skipped: Input/output error (os error 5)"),
                    more: 0,
                },
                "warning: /m/lost: skipped: Input/output error (os error 5)",
            ),
        ] {
            assert_eq!(words(&status), said, "{status:?}");
        }
    }
}
