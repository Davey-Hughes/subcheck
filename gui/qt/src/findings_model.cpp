#include "findings_model.h"

#include "words.h"

#include <QStringList>

using subcheck_ffi::EntryKey;
using subcheck_ffi::EntryView;
using subcheck_ffi::Splice;

namespace {

// A timing finding's track as a row names it: by its own name when no other track of its file
// has it, else by its number too; with no name, by its number and language.
QString track(const subcheck_ffi::ListTrack &track) {
    if (track.name && track.apart) {
        return text(*track.name);
    }
    if (track.name) {
        return QStringLiteral("Track %1: %2").arg(track.id).arg(text(*track.name));
    }
    if (track.language) {
        return QStringLiteral("Track %1 (%2)").arg(track.id).arg(text(*track.language));
    }
    return QStringLiteral("Track %1").arg(track.id);
}

// Where a row is: its episode, then the time its line starts, or a timing finding's track, whose
// fix is the whole track's.
QString place(const EntryView &entry) {
    const QString where = entry.track ? track(*entry.track) : clock(entry.at_ms);
    return QStringLiteral("%1 · %2").arg(text(entry.label), where);
}

// A row of several: how many places it is in, and how many of them the filter shows when not all;
// empty for a row of one, and for a member. Places and not episodes: a row can hold more than one
// finding of a file, and review sends no count of the files.
QString several(const EntryView &entry) {
    if (entry.is_member || entry.total < 2) {
        return {};
    }
    if (entry.shown == entry.total) {
        return QStringLiteral("In %1 places").arg(entry.total);
    }
    return QStringLiteral("In %1 places, %2 shown").arg(entry.total).arg(entry.shown);
}

// The fixes a row's shown members differ by, each counted, the most common first as review sends
// them. As markup, as a row's line is.
QString fixesLine(const std::vector<subcheck_ffi::FixCount> &fixes) {
    QStringList each;
    for (const subcheck_ffi::FixCount &fix : fixes) {
        each << QStringLiteral("%1 × %2").arg(fixWords(fix.fix)).arg(fix.findings);
    }
    return each.join(QStringLiteral(" · ")).toHtmlEscaped();
}

RowLine lineOf(const EntryView &entry) {
    // What the row holds, which the line of its first shown member alone would hide.
    if (entry.fixes.size() > 1) {
        const QString fixes = fixesLine(entry.fixes);
        return {fixes, fixes, QString()};
    }
    return rowLine(text(entry.original), entry.original_changed, text(entry.wording),
                   entry.wording_changed);
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
    case KindRole:
        return kindWord(entry.check);
    case KindToneRole:
        return kindTone(entry.check);
    case SeveralRole:
        return several(entry);
    case PlaceRole:
        return place(entry);
    case GlyphRole:
        return stateLook(entry.state, entry.mixed).glyph;
    case StateWordRole:
        return stateLook(entry.state, entry.mixed).word;
    case StateToneRole:
        return stateLook(entry.state, entry.mixed).tone;
    case Qt::DisplayRole:
    case LineRole:
        return lineOf(entry).whole;
    case CutLineRole:
        return lineOf(entry).cut;
    case LineReachRole:
        return lineOf(entry).reach;
    case MemberRole:
        return entry.is_member;
    default:
        return {};
    }
}

QHash<int, QByteArray> FindingsModel::roleNames() const {
    return {{KindRole, "kind"},           {KindToneRole, "kindTone"}, {SeveralRole, "several"},
            {PlaceRole, "place"},         {GlyphRole, "glyph"},       {StateWordRole, "stateWord"},
            {StateToneRole, "stateTone"}, {LineRole, "line"},         {CutLineRole, "cutLine"},
            {LineReachRole, "lineReach"}, {MemberRole, "member"}};
}

bool FindingsModel::apply(const Splice &change) {
    return splice(*this, rows, change.at, change.removed, change.inserted);
}

void FindingsModel::recolour() {
    if (rows.empty()) {
        return;
    }
    emit dataChanged(
        index(0), index(rowCount() - 1),
        {KindToneRole, StateToneRole, LineRole, CutLineRole, LineReachRole, Qt::DisplayRole});
}

std::optional<EntryKey> FindingsModel::keyAt(int row) const {
    if (row < 0 || row >= rowCount()) {
        return std::nullopt;
    }
    return rows[static_cast<size_t>(row)].key;
}
