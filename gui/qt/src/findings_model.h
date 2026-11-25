// The findings list as the session last described it: the rows of every `Changed` splice applied
// in order, which is what makes `Summary.selected` an index into them; each with what every row
// style draws of it, Compact, Two Lines and Rich, so that a change of style asks review nothing.
#pragma once

#include "spliced.h"
#include "subcheck_ffi.hpp"

#include <QAbstractListModel>
#include <QString>
#include <QtQml/qqmlregistration.h>

#include <optional>
#include <vector>

class FindingsModel : public QAbstractListModel {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("the session's bridge makes the one list")

public:
    // A row's badge (what found it) and its colour; for a row of several, how many places it is
    // in; where it is (its episode and time, or a timing finding's track); its decision as a
    // glyph, a word and their colour; its line with the change marked, cut before the change,
    // and as far as a row measures to choose between the two (words.h), or in place of that line
    // the fixes its shown members differ by; and whether it is a member of an expanded row. Then,
    // for Compact, where it is in short, "E03 12:34", and its change alone, "old → new"; for
    // Rich, why it was found, in plain text, and its line as its tile draws it, dark in every
    // window; for the triangle, whether it opens into its members, and whether it is open. The
    // fixes its shown members differ by stand in for its change and its tile's line as for its
    // line.
    enum Role {
        KindRole = Qt::UserRole + 1,
        KindToneRole,
        SeveralRole,
        PlaceRole,
        GlyphRole,
        StateWordRole,
        StateToneRole,
        LineRole,
        CutLineRole,
        LineReachRole,
        MemberRole,
        ShortPlaceRole,
        ChangeRole,
        ReasonRole,
        TileLineRole,
        ExpandableRole,
        ExpandedRole,
    };

    using QAbstractListModel::QAbstractListModel;

    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    // The splice's rows in place of those it removes (spliced.h).
    bool apply(const subcheck_ffi::Splice &splice);

    // The entry at `row`, as review last sent it; none outside the list. Held until the next
    // splice, which can replace it.
    const subcheck_ffi::EntryView *at(int row) const;

    // The key of the entry at `row`, for `Select`; none outside the list.
    std::optional<subcheck_ffi::EntryKey> keyAt(int row) const;

    // The first row after `from`, or before it with `back`, going round past either end, whose
    // line as it is now, its fix's wording, its episode or why it was found holds `words`, case
    // aside, and a line break or a run of spaces in either as one space; `from` itself last. From
    // -1, the first row, or the last with `back`. -1 for none, and for no words. Only the rows the
    // list shows: the members of a closed row are not rows of the list, and Find selects through
    // the list, as a click does.
    Q_INVOKABLE int find(const QString &words, int from, bool back) const;

    // Every row drawn again in the palette now chosen: its badge, its state, its line, its change
    // and its tile's line.
    void recolour();

private:
    template <typename M, typename I>
    friend bool splice(M &, std::vector<I> &, uint64_t, uint64_t, const std::vector<I> &);

    std::vector<subcheck_ffi::EntryView> rows;
};
