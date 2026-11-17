#include "findings_model.h"

#include "overloaded.h"

#include <QStringList>

#include <algorithm>
#include <cstdint>
#include <cstdlib>
#include <variant>

using subcheck_ffi::EntryKey;
using subcheck_ffi::EntryView;
using subcheck_ffi::Fix;
using subcheck_ffi::ListTrack;
using subcheck_ffi::MarkKind;
using subcheck_ffi::Splice;
using subcheck_ffi::State;

namespace {

// As core's `text::timestamp`, which the terminal's list uses: h:mm:ss.cc, from 0 for a moment
// before the file's start.
QString timestamp(int64_t ms) {
    ms = std::max<int64_t>(ms, 0);
    return QStringLiteral("%1:%2:%3.%4")
        .arg(ms / 3600000)
        .arg(ms / 60000 % 60, 2, 10, QLatin1Char('0'))
        .arg(ms / 1000 % 60, 2, 10, QLatin1Char('0'))
        .arg(ms % 1000 / 10, 2, 10, QLatin1Char('0'));
}

QString text(const std::string &s) { return QString::fromStdString(s); }

// The rows' marks, fixes and track below are worded as the terminal's list words them in core
// (`MarkKind::text`, `marks_text`, `Fix::label`, `fixes_text`, `ListTrack::label`), so that the
// window's list words a row as the terminal's list does. Nothing ties these copies to core's: a
// change to the list's words there is made here too, until the window words its own.

// A mark as the terminal's list says it (core's `MarkKind::text`).
QString word(MarkKind kind) {
    switch (kind) {
    case MarkKind::kByHand:
        return QStringLiteral("by hand");
    case MarkKind::kNotFound:
        return QStringLiteral("not found");
    case MarkKind::kWouldApply:
        return QStringLiteral("would apply");
    case MarkKind::kAlreadyApplied:
        return QStringLiteral("already applied");
    case MarkKind::kWouldBeByHand:
        return QStringLiteral("would be by hand");
    case MarkKind::kWouldNotBeFound:
        return QStringLiteral("would not be found");
    case MarkKind::kFailed:
        return QStringLiteral("failed");
    case MarkKind::kWouldFail:
        return QStringLiteral("would fail");
    case MarkKind::kApplied:
        return QStringLiteral("applied");
    }
    return QStringLiteral("?");
}

// An entry's marks as the terminal's list says them (core's `marks_text`): the one alone when
// every finding it shows has it, else each with how many have it; empty with none.
QString marks(const EntryView &entry) {
    if (entry.marks.size() == 1 && entry.marks[0].findings == entry.shown) {
        return word(entry.marks[0].kind);
    }
    QStringList parts;
    for (const auto &mark : entry.marks) {
        parts << QStringLiteral("%1 %2").arg(word(mark.kind)).arg(mark.findings);
    }
    return parts.join(QStringLiteral(" · "));
}

// A whole track's shift in a word: −1 frame (its lines moved a frame earlier), +2 frames.
QString shift(int64_t frames) {
    const int64_t n = std::abs(frames);
    return QStringLiteral("%1%2 %3")
        .arg(frames > 0 ? QStringLiteral("−") : QStringLiteral("+"))
        .arg(n)
        .arg(n == 1 ? QStringLiteral("frame") : QStringLiteral("frames"));
}

// A timing fix in a word (core's `Fix::label`): a shift, or partial.
QString word(const Fix &fix) {
    return std::visit(Overloaded{[](const Fix::kShift &s) { return shift(s.frames); },
                                 [](const Fix::kPartial &) { return QStringLiteral("partial"); }},
                      fix.get_variant());
}

// An entry's fixes in short (core's `fixes_text`): one alone when they all agree, else each with
// how many have it, in the order given.
QString fixes(const EntryView &entry) {
    if (entry.fixes.size() == 1) {
        return word(entry.fixes[0].fix);
    }
    QStringList parts;
    for (const auto &fix : entry.fixes) {
        parts << QStringLiteral("%1 ×%2").arg(word(fix.fix)).arg(fix.findings);
    }
    return parts.join(QStringLiteral(" · "));
}

// A timing finding's track as the terminal's list names it (core's `ListTrack::label`).
QString name(const std::optional<ListTrack> &track) {
    if (!track) {
        return {};
    }
    if (track->name && track->apart) {
        return text(*track->name);
    }
    if (track->name) {
        return QStringLiteral("track %1 %2").arg(track->id).arg(text(*track->name));
    }
    if (track->language) {
        return QStringLiteral("track %1 (%2)").arg(track->id).arg(text(*track->language));
    }
    return QStringLiteral("track %1").arg(track->id);
}

// A decision's mark in the terminal's list, and its name for the list's colours; "mixed" for a row
// whose decided members differ.
struct Decision {
    QString mark;
    QString name;
};

Decision decision(const std::optional<State> &state) {
    if (!state) {
        return {QStringLiteral("±"), QStringLiteral("mixed")};
    }
    switch (*state) {
    case State::kPending:
        return {QStringLiteral("·"), QStringLiteral("pending")};
    case State::kAccepted:
        return {QStringLiteral("✓"), QStringLiteral("accepted")};
    case State::kRejected:
        return {QStringLiteral("✗"), QStringLiteral("rejected")};
    }
    return {QStringLiteral("?"), QStringLiteral("mixed")};
}

} // namespace

int FindingsModel::rowCount(const QModelIndex &parent) const {
    return parent.isValid() ? 0 : static_cast<int>(rows.size());
}

QVariant FindingsModel::data(const QModelIndex &index, int role) const {
    if (!checkIndex(index, CheckIndexOption::IndexIsValid | CheckIndexOption::ParentIsInvalid)) {
        return {};
    }
    const EntryView &entry = rows[static_cast<size_t>(index.row())];
    switch (role) {
    case Qt::DisplayRole:
    case LineRole:
        return line(entry);
    case StateRole:
        return state(entry);
    case MemberRole:
        return entry.is_member;
    default:
        return {};
    }
}

QHash<int, QByteArray> FindingsModel::roleNames() const {
    return {{LineRole, "line"}, {StateRole, "decision"}, {MemberRole, "member"}};
}

bool FindingsModel::apply(const Splice &splice) {
    const size_t held = rows.size();
    if (splice.at > held || splice.removed > held - splice.at) {
        return false;
    }
    const int at = static_cast<int>(splice.at);
    const int removed = static_cast<int>(splice.removed);
    const int inserted = static_cast<int>(splice.inserted.size());
    const int changed = std::min(removed, inserted);
    for (int i = 0; i < changed; ++i) {
        rows[static_cast<size_t>(at + i)] = splice.inserted[static_cast<size_t>(i)];
    }
    if (changed > 0) {
        emit dataChanged(index(at), index(at + changed - 1));
    }
    if (inserted > changed) {
        beginInsertRows({}, at + changed, at + inserted - 1);
        rows.insert(rows.begin() + at + changed, splice.inserted.begin() + changed,
                    splice.inserted.end());
        endInsertRows();
    } else if (removed > changed) {
        beginRemoveRows({}, at + changed, at + removed - 1);
        rows.erase(rows.begin() + at + changed, rows.begin() + at + removed);
        endRemoveRows();
    }
    return true;
}

std::optional<EntryKey> FindingsModel::keyAt(int row) const {
    if (row < 0 || row >= rowCount()) {
        return std::nullopt;
    }
    return rows[static_cast<size_t>(row)].key;
}

QString FindingsModel::line(const EntryView &entry) {
    const QString said = marks(entry);
    const QString outcome = said.isEmpty() ? QString() : QStringLiteral("  ") + said;
    if (entry.is_member) {
        // A timing finding's members differ in their fix, not in a time.
        const QString what = entry.fixes.empty() ? timestamp(entry.at_ms) : fixes(entry);
        return QStringLiteral("    %1 %2 %3%4")
            .arg(decision(entry.state).mark, text(entry.label), what, outcome);
    }
    const QString who = entry.total == 1 ? text(entry.label)
                        : entry.shown == entry.total
                            ? QStringLiteral("×%1").arg(entry.total)
                            : QStringLiteral("×%1/%2").arg(entry.shown).arg(entry.total);
    if (!entry.fixes.empty()) {
        return QStringLiteral("%1 %2 %3  %4%5")
            .arg(decision(entry.state).mark, who, name(entry.track), fixes(entry), outcome);
    }
    return QStringLiteral("%1 %2 %3 %4 → %5%6")
        .arg(decision(entry.state).mark, who, timestamp(entry.at_ms), text(entry.original),
             text(entry.wording), outcome);
}

QString FindingsModel::state(const EntryView &entry) { return decision(entry.state).name; }
