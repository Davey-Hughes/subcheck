#include "status_words.h"

#include "overloaded.h"

#include <QStringList>

#include <algorithm>
#include <cstdint>
#include <variant>

using namespace subcheck_ffi;

// Worded as the terminal words each message (cli's `tui::status::words`), so that the window's
// status line words a message as the terminal's does. Nothing ties this copy to the terminal's: a
// change to the words there is made here too, until the window words its own. Text that is not
// review's (a label, an error) goes in with the last `arg`, or with the others in one, so that a
// `%1` in it is never taken for a place to fill.

namespace {

QString text(const std::string &s) { return QString::fromStdString(s); }

// As core's `text::timestamp_ms`: h:mm:ss.mmm, from 0 for a moment before the file's start.
QString timestamp(int64_t ms) {
    ms = std::max<int64_t>(ms, 0);
    return QStringLiteral("%1:%2:%3.%4")
        .arg(ms / 3600000)
        .arg(ms / 60000 % 60, 2, 10, QLatin1Char('0'))
        .arg(ms / 1000 % 60, 2, 10, QLatin1Char('0'))
        .arg(ms % 1000, 3, 10, QLatin1Char('0'));
}

// As core's `progress::findings_text`.
QString counted(uint64_t findings) {
    return findings == 1 ? QStringLiteral("1 finding")
                         : QStringLiteral("%1 findings").arg(findings);
}

QString refused(const Refusal &why) {
    return std::visit(
        Overloaded{
            [](const Refusal::kSelectionStays &) {
                return QStringLiteral("that finding changed; the selection stays");
            },
            [](const Refusal::kNotInView &r) {
                return QStringLiteral("%1 is not in this view").arg(text(r.label));
            },
            [](const Refusal::kTimingFix &) {
                return QStringLiteral("a timing fix is accepted or rejected, not edited");
            },
            [](const Refusal::kEmptyWording &) {
                return QStringLiteral("the wording cannot be empty (Esc cancels)");
            },
            [](const Refusal::kEditOutOfDate &) {
                return QStringLiteral("the edit is out of date; nothing was decided");
            },
            [](const Refusal::kAlready &r) {
                return QStringLiteral("already %1").arg(stateWord(r.state));
            },
            [](const Refusal::kNothingToClear &) { return QStringLiteral("nothing to clear"); },
            [](const Refusal::kNothingToUndo &) { return QStringLiteral("nothing to undo"); },
            [](const Refusal::kLastFile &) { return QStringLiteral("last file"); },
            [](const Refusal::kFirstFile &) { return QStringLiteral("first file"); },
            [](const Refusal::kFileMissing &r) {
                return QStringLiteral("file missing: %1").arg(text(r.path));
            },
            [](const Refusal::kNothingToOpen &) {
                return QStringLiteral("nothing to open: no line of this track moves");
            },
            [](const Refusal::kScanRunning &) {
                return QStringLiteral("the scan is still running");
            },
            [](const Refusal::kApplyRunning &) { return QStringLiteral("apply is still running"); },
            [](const Refusal::kAlreadyApplying &) { return QStringLiteral("already applying"); },
            [](const Refusal::kNoAcceptedFixes &) { return QStringLiteral("no accepted fixes"); },
            [](const Refusal::kBeingChecked &r) {
                return QStringLiteral("%1 is being checked now").arg(text(r.label));
            },
            [](const Refusal::kNoFindingsShown &r) {
                return QStringLiteral("no findings of %1 shown").arg(text(r.label));
            },
        },
        why.get_variant());
}

QString failed(const Failure &what, const std::string &error) {
    const QString e = text(error);
    return std::visit(
        Overloaded{
            [&](const Failure::kSave &) { return QStringLiteral("not saved: %1").arg(e); },
            [&](const Failure::kSwitch &) {
                return QStringLiteral("could not switch views: %1").arg(e);
            },
            [&](const Failure::kReadBack &f) {
                return QStringLiteral("could not read %1 back: %2").arg(text(f.label), e);
            },
            [&](const Failure::kRecheck &) { return e; },
            [&](const Failure::kMpv &) { return QStringLiteral("mpv: %1").arg(e); },
            [&](const Failure::kPlayer &) { return e; },
        },
        what.get_variant());
}

} // namespace

QString stateWord(State state) {
    switch (state) {
    case State::kPending:
        return QStringLiteral("pending");
    case State::kAccepted:
        return QStringLiteral("accepted");
    case State::kRejected:
        return QStringLiteral("rejected");
    }
    return QStringLiteral("?");
}

QString statusWords(const Status &status) {
    return std::visit(
        Overloaded{
            [](const Status::kDecided &d) {
                QString said = QStringLiteral("%1 %2").arg(stateWord(d.state), counted(d.findings));
                if (d.replaced > 0) {
                    said +=
                        QStringLiteral(", replacing your wording on %1 (u undoes)").arg(d.replaced);
                }
                if (d.kept) {
                    said += QStringLiteral(", %1 kept as %2")
                                .arg(d.kept->findings)
                                .arg(stateWord(d.kept->state));
                }
                return said;
            },
            [](const Status::kCleared &c) {
                return QStringLiteral("cleared %1").arg(counted(c.findings));
            },
            [](const Status::kUndone &u) {
                QStringList labels;
                for (const std::string &label : u.labels) {
                    labels << text(label);
                }
                return QStringLiteral("undone: %1 · %2")
                    .arg(counted(u.findings), labels.join(QStringLiteral(", ")));
            },
            [](const Status::kInherited &i) {
                if (i.findings == 1) {
                    return QStringLiteral(
                               "%1: a new finding took its row's decision: %2 (u undoes)")
                        .arg(text(i.label), stateWord(i.state));
                }
                return QStringLiteral("%1: %2 new findings took their rows' decisions (u undoes)")
                    .arg(text(i.label), QString::number(i.findings));
            },
            [](const Status::kReplaced &r) {
                return QStringLiteral("%1 was checked again; %2")
                    .arg(text(r.label),
                         r.gone ? QStringLiteral("that finding is gone")
                                : QStringLiteral("your decision went to its new finding"));
            },
            [](const Status::kEditEnded &e) {
                return QStringLiteral("%1 was checked again; its finding changed")
                    .arg(text(e.label));
            },
            [](const Status::kRefused &r) { return refused(r.why); },
            [](const Status::kQueuedAgain &q) {
                return QStringLiteral("%1 will be checked again when the scan is done")
                    .arg(text(q.label));
            },
            [](const Status::kNoSuchFile &n) {
                return QStringLiteral("%1: no such file").arg(text(n.label));
            },
            [](const Status::kOpening &o) {
                return QStringLiteral("opening %1 at %2").arg(text(o.label), timestamp(o.at_ms));
            },
            [](const Status::kOpened &o) {
                return o.paused ? QStringLiteral("opened in mpv, paused")
                                : QStringLiteral("opened in mpv, playing");
            },
            [](const Status::kPlayed &p) {
                return QStringLiteral("%1 %2").arg(p.state.paused ? QStringLiteral("⏸")
                                                                  : QStringLiteral("▶"),
                                                   timestamp(p.state.at_ms));
            },
            [](const Status::kClosed &) { return QStringLiteral("closed mpv"); },
            [](const Status::kNotOpen &n) {
                return n.asked == Asked::kPlayback ? QStringLiteral("mpv is not open (o opens it)")
                                                   : QStringLiteral("mpv is not open");
            },
            [](const Status::kNoFile &) {
                return QStringLiteral("mpv has no file open (o opens one)");
            },
            [](const Status::kStillOpening &) {
                return QStringLiteral("mpv is still opening the file");
            },
            [](const Status::kCouldNotOpen &c) {
                return QStringLiteral("mpv could not open %1").arg(text(c.label));
            },
            [](const Status::kNoAnswer &) { return QStringLiteral("mpv did not answer"); },
            [](const Status::kHungUp &) {
                return QStringLiteral("mpv closed the connection without answering");
            },
            [](const Status::kNoVideo &) {
                return QStringLiteral("the video pane could not show video");
            },
            [](const Status::kFailed &f) { return failed(f.what, f.error); },
            [](const Status::kWarned &w) {
                if (w.more == 0) {
                    return text(w.first);
                }
                return QStringLiteral("%1 · +%2 more").arg(text(w.first), QString::number(w.more));
            },
        },
        status.get_variant());
}
