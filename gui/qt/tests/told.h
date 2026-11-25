// What the window asked of review, each action as one line, and a bridge that keeps each line as
// it sends the action on, or holds the action back from review: so that a test of a key, a menu
// item or a click checks the action it sends, not only what review then did.
#pragma once

#include "overloaded.h"
#include "session_bridge.h"

#include <QString>
#include <QStringList>

#include <utility>
#include <variant>

inline QString told(const subcheck_ffi::Filter filter) {
    using subcheck_ffi::Filter;
    switch (filter) {
    case Filter::kAll:
        return QStringLiteral("All");
    case Filter::kPending:
        return QStringLiteral("Pending");
    case Filter::kAccepted:
        return QStringLiteral("Accepted");
    case Filter::kRejected:
        return QStringLiteral("Rejected");
    }
    return QStringLiteral("?");
}

inline QString told(const subcheck_ffi::Action &action) {
    using namespace subcheck_ffi;
    const auto q = [](const std::string &s) { return QString::fromStdString(s); };
    return std::visit(
        Overloaded{
            [](const Action::kQuit &) { return QStringLiteral("Quit"); },
            [](const Action::kInterrupt &) { return QStringLiteral("Interrupt"); },
            [](const Action::kSelect &a) {
                if (const auto *row = std::get_if<EntryKey::kRow>(&a.key.get_variant())) {
                    QStringList ids;
                    for (uint64_t id : row->v1) {
                        ids << QString::number(id);
                    }
                    return QStringLiteral("Select row %1").arg(ids.join(QLatin1Char(',')));
                }
                return QStringLiteral("Select member %1")
                    .arg(std::get<EntryKey::kMember>(a.key.get_variant()).v1);
            },
            [](const Action::kMove &a) { return QStringLiteral("Move %1").arg(a.by); },
            [](const Action::kToggle &) { return QStringLiteral("Toggle"); },
            [](const Action::kSetFilter &a) {
                return QStringLiteral("SetFilter %1").arg(told(a.filter));
            },
            [](const Action::kNextFile &) { return QStringLiteral("NextFile"); },
            [](const Action::kPreviousFile &) { return QStringLiteral("PreviousFile"); },
            [](const Action::kAccept &) { return QStringLiteral("Accept"); },
            [](const Action::kReject &) { return QStringLiteral("Reject"); },
            [](const Action::kClear &) { return QStringLiteral("Clear"); },
            [](const Action::kUndo &) { return QStringLiteral("Undo"); },
            [](const Action::kEdit &) { return QStringLiteral("Edit"); },
            [&](const Action::kReword &a) { return QStringLiteral("Reword ") + q(a.wording); },
            [](const Action::kCancelEdit &) { return QStringLiteral("CancelEdit"); },
            [](const Action::kShowPane &a) {
                return a.pane == Pane::kFiles ? QStringLiteral("ShowPane Files")
                                              : QStringLiteral("ShowPane Findings");
            },
            [](const Action::kSwitchView &) { return QStringLiteral("SwitchView"); },
            [&](const Action::kOpenView &a) {
                QStringList words;
                for (const std::string &word : a.selector) {
                    words << q(word);
                }
                return QStringLiteral("OpenView ") + words.join(QLatin1Char(' '));
            },
            [&](const Action::kOpenFolder &a) {
                return QStringLiteral("OpenFolder ") + q(a.folder);
            },
            [](const Action::kApply &a) {
                return a.dry_run ? QStringLiteral("Apply dry") : QStringLiteral("Apply");
            },
            [](const Action::kPlay &) { return QStringLiteral("Play"); },
            [](const Action::kClosePlayer &) { return QStringLiteral("ClosePlayer"); },
            [](const Action::kPlayback &a) {
                return std::visit(
                    Overloaded{
                        [](const Playback::kPause &) { return QStringLiteral("Playback Pause"); },
                        [](const Playback::kSeek &s) {
                            return QStringLiteral("Playback Seek %1").arg(s.v1);
                        },
                        [](const Playback::kNextFrame &) {
                            return QStringLiteral("Playback NextFrame");
                        },
                        [](const Playback::kPreviousFrame &) {
                            return QStringLiteral("Playback PreviousFrame");
                        },
                    },
                    a.playback.get_variant());
            },
            [](const Action::kAnswer &a) {
                return a.yes ? QStringLiteral("Answer yes") : QStringLiteral("Answer no");
            },
            [&](const Action::kSelectFile &a) { return QStringLiteral("SelectFile ") + q(a.path); },
            [](const Action::kShowFileFindings &) { return QStringLiteral("ShowFileFindings"); },
            [](const Action::kRecheck &) { return QStringLiteral("Recheck"); },
            [](const Action::kDismiss &) { return QStringLiteral("Dismiss"); },
        },
        action.get_variant());
}

// A bridge that keeps, in `sent`, each action it sends to review; and, in `revealed`, each file it
// is asked to show in the file manager, which no test asks a real one to show.
class Telling : public SessionBridge {
public:
    using SessionBridge::SessionBridge;

    QStringList sent;
    QStringList revealed;

    void send(subcheck_ffi::Action action) override {
        sent << told(action);
        SessionBridge::send(std::move(action));
    }

protected:
    void reveal(const QString &file) override { revealed << file; }
};

// A bridge that, once `holding`, keeps each action from review as it tells it, counted as sent: a
// test then stands in for review's answers, each at the moment it chooses, with the count review
// would give it, or leaves the actions unanswered, as a review busy with a job would.
class Holding : public Telling {
public:
    using Telling::Telling;

    bool holding = false;

    void send(subcheck_ffi::Action action) override {
        if (holding) {
            sent << told(action);
            ++asked;
            return;
        }
        Telling::send(std::move(action));
    }
};
