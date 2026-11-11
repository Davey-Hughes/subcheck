#include "findings_model.h"

#include <algorithm>
#include <cstdint>

using subcheck_ffi::EntryKey;
using subcheck_ffi::EntryView;
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
    const QString outcome = entry.mark ? QStringLiteral("  ") + text(*entry.mark) : QString();
    if (entry.is_member) {
        // A timing finding's members differ in their fix, not in a time.
        const QString what = entry.fix ? text(*entry.fix) : timestamp(entry.at_ms);
        return QStringLiteral("    %1 %2 %3%4")
            .arg(decision(entry.state).mark, text(entry.label), what, outcome);
    }
    const QString who = entry.total == 1 ? text(entry.label)
                        : entry.shown == entry.total
                            ? QStringLiteral("×%1").arg(entry.total)
                            : QStringLiteral("×%1/%2").arg(entry.shown).arg(entry.total);
    if (entry.fix) {
        return QStringLiteral("%1 %2 %3  %4%5")
            .arg(decision(entry.state).mark, who, text(entry.track), text(*entry.fix), outcome);
    }
    return QStringLiteral("%1 %2 %3 %4 → %5%6")
        .arg(decision(entry.state).mark, who, timestamp(entry.at_ms), text(entry.original),
             text(entry.wording), outcome);
}

QString FindingsModel::state(const EntryView &entry) { return decision(entry.state).name; }
