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

// Where in its episode a row is: the time its line starts, or a timing finding's track, whose fix
// is the whole track's.
QString where(const EntryView &entry) {
    return entry.track ? track(*entry.track) : clock(entry.at_ms);
}

// Where a row is: its episode, then where in it.
QString place(const EntryView &entry) {
    return QStringLiteral("%1 · %2").arg(text(entry.label), where(entry));
}

// The same, as short as a Compact row's one line has room for: "E03 12:34".
QString shortPlace(const EntryView &entry) {
    return QStringLiteral("%1 %2").arg(text(entry.label), where(entry));
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

// Whether a row's shown members differ by their fixes: what the row holds, which the line of its
// first shown member alone would hide, so each line it draws says them instead.
bool fixesDiffer(const EntryView &entry) { return entry.fixes.size() > 1; }

RowLine lineOf(const EntryView &entry) {
    if (fixesDiffer(entry)) {
        const QString fixes = fixesLine(entry.fixes);
        return {fixes, fixes, QString()};
    }
    return rowLine(text(entry.original), entry.original_changed, text(entry.wording),
                   entry.wording_changed);
}

// A Compact row's change. A line review marks nothing in, as a timing finding's whose fix moves
// it and leaves its words, is itself: an arrow between nothing and nothing would say nothing.
QString changeOf(const EntryView &entry) {
    if (fixesDiffer(entry)) {
        return fixesLine(entry.fixes);
    }
    const auto empty = [](const subcheck_ffi::Span &span) { return span.end <= span.start; };
    if (empty(entry.original_changed) && empty(entry.wording_changed)) {
        return text(entry.original).toHtmlEscaped();
    }
    return changeLine(text(entry.original), entry.original_changed, text(entry.wording),
                      entry.wording_changed);
}

// A Rich row's tile draws the line as it is now, its change struck, on dark in every window.
QString tileLineOf(const EntryView &entry) {
    if (fixesDiffer(entry)) {
        return fixesLine(entry.fixes);
    }
    return marked(text(entry.original), entry.original_changed, true, Marking::InRow, true);
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
    case ShortPlaceRole:
        return shortPlace(entry);
    case ChangeRole:
        return changeOf(entry);
    case ReasonRole:
        return text(entry.reason);
    case TileLineRole:
        return tileLineOf(entry);
    case ExpandableRole:
        // Never a member, whose row is the one that opens; never a row of one.
        return !entry.is_member && entry.total > 1;
    case ExpandedRole:
        return entry.expanded;
    default:
        return {};
    }
}

QHash<int, QByteArray> FindingsModel::roleNames() const {
    return {{KindRole, "kind"},           {KindToneRole, "kindTone"},
            {SeveralRole, "several"},     {PlaceRole, "place"},
            {GlyphRole, "glyph"},         {StateWordRole, "stateWord"},
            {StateToneRole, "stateTone"}, {LineRole, "line"},
            {CutLineRole, "cutLine"},     {LineReachRole, "lineReach"},
            {MemberRole, "member"},       {ShortPlaceRole, "shortPlace"},
            {ChangeRole, "change"},       {ReasonRole, "reason"},
            {TileLineRole, "tileLine"},   {ExpandableRole, "expandable"},
            {ExpandedRole, "expanded"}};
}

bool FindingsModel::apply(const Splice &change) {
    return splice(*this, rows, change.at, change.removed, change.inserted);
}

void FindingsModel::recolour() {
    if (rows.empty()) {
        return;
    }
    emit dataChanged(index(0), index(rowCount() - 1),
                     {KindToneRole, StateToneRole, LineRole, CutLineRole, LineReachRole,
                      Qt::DisplayRole, ChangeRole, TileLineRole});
}

const EntryView *FindingsModel::at(int row) const {
    if (row < 0 || row >= rowCount()) {
        return nullptr;
    }
    return &rows[static_cast<size_t>(row)];
}

int FindingsModel::find(const QString &words, int from, bool back) const {
    const QString wanted = words.simplified();
    const int count = rowCount();
    if (wanted.isEmpty() || count == 0) {
        return -1;
    }
    const auto holds = [&wanted](const std::string &part) {
        return text(part).simplified().contains(wanted, Qt::CaseInsensitive);
    };
    // One step before the first row looked at, so that `from` comes last.
    const int start = from >= 0 && from < count ? from : (back ? count : -1);
    const int step = back ? -1 : 1;
    for (int i = 1; i <= count; ++i) {
        const int row = ((start + step * i) % count + count) % count;
        const EntryView &entry = rows[static_cast<size_t>(row)];
        if (holds(entry.original) || holds(entry.wording) || holds(entry.label) ||
            holds(entry.reason)) {
            return row;
        }
    }
    return -1;
}

std::optional<EntryKey> FindingsModel::keyAt(int row) const {
    const EntryView *entry = at(row);
    return entry ? std::optional<EntryKey>(entry->key) : std::nullopt;
}
