#include "status_words.h"

#include "overloaded.h"
#include "words.h"

#include <QStringList>

#include <variant>

using namespace subcheck_ffi;

// Text that is not the window's (a label, a path, an error) goes in with the last `arg`, or with
// the others in one, so that a `%1` in it is never taken for a place to fill.

namespace {

QString findings(uint64_t n) {
    return counted(n, QStringLiteral("finding"), QStringLiteral("findings"));
}

// A decision as a sentence's end says it: "to review", "accepted", "rejected".
QString left(State state) {
    return state == State::kPending ? QStringLiteral("to review") : stateName(state).toLower();
}

QString refused(const Refusal &why) {
    return std::visit(
        Overloaded{
            [](const Refusal::kSelectionStays &) {
                return QStringLiteral("That finding changed; the selection stays");
            },
            [](const Refusal::kNotInView &r) {
                return QStringLiteral("%1 is not in this view").arg(text(r.label));
            },
            [](const Refusal::kTimingFix &) {
                return QStringLiteral("A timing fix is accepted or rejected, not reworded");
            },
            [](const Refusal::kEmptyWording &) {
                return QStringLiteral("The wording cannot be empty");
            },
            [](const Refusal::kEditOutOfDate &) {
                return QStringLiteral("The edit is out of date; nothing was decided");
            },
            [](const Refusal::kAlready &r) {
                return QStringLiteral("Already %1").arg(left(r.state));
            },
            [](const Refusal::kNothingToClear &) {
                return QStringLiteral("Nothing to put back: none of it is decided");
            },
            [](const Refusal::kNothingToUndo &) { return QStringLiteral("Nothing to undo"); },
            [](const Refusal::kLastFile &) { return QStringLiteral("This is the last episode"); },
            [](const Refusal::kFirstFile &) { return QStringLiteral("This is the first episode"); },
            [](const Refusal::kFileMissing &r) {
                return QStringLiteral("The file is missing: %1").arg(text(r.path));
            },
            [](const Refusal::kNothingToOpen &) {
                return QStringLiteral("Nothing to show: no line of this track moves");
            },
            [](const Refusal::kScanRunning &) {
                return QStringLiteral("A check is still running");
            },
            [](const Refusal::kApplyRunning &) {
                return QStringLiteral("Fixes are still being applied");
            },
            [](const Refusal::kAlreadyApplying &) {
                return QStringLiteral("Fixes are being applied already");
            },
            [](const Refusal::kNoAcceptedFixes &) {
                return QStringLiteral("No accepted fixes to apply");
            },
            [](const Refusal::kBeingChecked &r) {
                return QStringLiteral("%1 is being checked now").arg(text(r.label));
            },
            [](const Refusal::kNoFindingsShown &r) {
                return QStringLiteral("No findings of %1 are shown").arg(text(r.label));
            },
        },
        why.get_variant());
}

QString failed(const Failure &what, const std::string &error) {
    const QString e = text(error);
    return std::visit(
        Overloaded{
            [&](const Failure::kSave &) { return QStringLiteral("Not saved: %1").arg(e); },
            [&](const Failure::kSwitch &) {
                return QStringLiteral("Could not open that view: %1").arg(e);
            },
            [&](const Failure::kReadBack &f) {
                return QStringLiteral("Could not read %1 back: %2").arg(text(f.label), e);
            },
            [&](const Failure::kRecheck &) {
                return QStringLiteral("Could not check again: %1").arg(e);
            },
            [&](const Failure::kMpv &) { return QStringLiteral("The video refused: %1").arg(e); },
            [&](const Failure::kPlayer &) { return QStringLiteral("The video failed: %1").arg(e); },
        },
        what.get_variant());
}

} // namespace

QString statusWords(const Status &status) {
    return std::visit(
        Overloaded{
            [](const Status::kDecided &d) {
                QString said =
                    QStringLiteral("%1 %2").arg(stateName(d.state), findings(d.findings));
                if (d.replaced > 0) {
                    said += QStringLiteral(", replacing your wording on %1 (Undo puts it back)")
                                .arg(d.replaced);
                }
                if (d.kept) {
                    said += QStringLiteral(", leaving %1 %2")
                                .arg(d.kept->findings)
                                .arg(left(d.kept->state));
                }
                return said;
            },
            [](const Status::kCleared &c) {
                return QStringLiteral("%1 back to undecided").arg(findings(c.findings));
            },
            [](const Status::kUndone &u) {
                QStringList labels;
                for (const std::string &label : u.labels) {
                    labels << text(label);
                }
                return QStringLiteral("Undone: %1 in %2")
                    .arg(findings(u.findings), labels.join(QStringLiteral(", ")));
            },
            [](const Status::kInherited &i) {
                if (i.findings == 1) {
                    return QStringLiteral("%1: a new finding took its row's decision, %2 (Undo "
                                          "puts it back)")
                        .arg(text(i.label), left(i.state));
                }
                return QStringLiteral(
                           "%1: %2 new findings took their rows' decisions (Undo puts them back)")
                    .arg(text(i.label), QString::number(i.findings));
            },
            [](const Status::kReplaced &r) {
                return QStringLiteral("%1 was checked again; %2")
                    .arg(text(r.label),
                         r.gone ? QStringLiteral("that finding is gone")
                                : QStringLiteral("your decision went to its new finding"));
            },
            [](const Status::kEditEnded &e) {
                return QStringLiteral("%1 was checked again; its finding changed, so the edit "
                                      "ended")
                    .arg(text(e.label));
            },
            [](const Status::kRefused &r) { return refused(r.why); },
            [](const Status::kQueuedAgain &q) {
                return QStringLiteral("%1 will be checked again when the check under way ends")
                    .arg(text(q.label));
            },
            [](const Status::kNoSuchFile &n) {
                return QStringLiteral("%1 is not on disk, so it was not checked again")
                    .arg(text(n.label));
            },
            [](const Status::kOpeningView &) { return QStringLiteral("Opening the view…"); },
            [](const Status::kOpening &o) {
                const QString words =
                    QStringLiteral("Opening %1 at %2").arg(text(o.label), exactClock(o.at_ms));
                return o.play ? words + QStringLiteral(", will play") : words;
            },
            [](const Status::kNotAnswering &n) {
                return QStringLiteral("%1: the share isn't answering, still waiting")
                    .arg(text(n.label));
            },
            [](const Status::kStoppedWaiting &s) {
                return QStringLiteral("Stopped waiting for %1").arg(text(s.label));
            },
            [](const Status::kOpened &o) {
                return o.paused ? QStringLiteral("Showing it, paused")
                                : QStringLiteral("Showing it, playing");
            },
            [](const Status::kPlayed &p) {
                return QStringLiteral("%1 at %2")
                    .arg(p.state.paused ? QStringLiteral("Paused") : QStringLiteral("Playing"),
                         exactClock(p.state.at_ms));
            },
            [](const Status::kClosed &) { return QStringLiteral("Video closed"); },
            [](const Status::kNotOpen &n) {
                return n.asked == Asked::kPlayback
                           ? QStringLiteral("No video is open; Show in Video opens one")
                           : QStringLiteral("No video is open");
            },
            [](const Status::kNoFile &) {
                return QStringLiteral("The video has no file open; Show in Video opens one");
            },
            [](const Status::kStillOpening &) {
                return QStringLiteral("The video is still opening the file");
            },
            [](const Status::kCouldNotOpen &c) {
                return QStringLiteral("The video could not open %1").arg(text(c.label));
            },
            [](const Status::kNoAnswer &) { return QStringLiteral("The video did not answer"); },
            [](const Status::kHungUp &) {
                return QStringLiteral("The video stopped without answering");
            },
            [](const Status::kNoVideo &) {
                return QStringLiteral("The video pane could not show video");
            },
            [](const Status::kFailed &f) { return failed(f.what, f.error); },
            [](const Status::kWarned &w) {
                if (w.more == 0) {
                    return text(w.first);
                }
                return QStringLiteral("%1 (and %2 more)")
                    .arg(text(w.first), QString::number(w.more));
            },
        },
        status.get_variant());
}
