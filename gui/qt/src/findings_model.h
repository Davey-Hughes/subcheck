// The findings list as the session last described it: the rows of every `Changed` splice applied
// in order, which is what makes `Summary.selected` an index into them; each as a Two Lines row
// draws it.
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
    // the fixes its shown members differ by; and whether it is a member of an expanded row.
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
    };

    using QAbstractListModel::QAbstractListModel;

    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    // The splice's rows in place of those it removes (spliced.h).
    bool apply(const subcheck_ffi::Splice &splice);

    // The key of the entry at `row`, for `Select`; none outside the list.
    std::optional<subcheck_ffi::EntryKey> keyAt(int row) const;

    // Every row drawn again in the palette now chosen: its badge, its state and its line.
    void recolour();

private:
    template <typename M, typename I>
    friend bool splice(M &, std::vector<I> &, uint64_t, uint64_t, const std::vector<I> &);

    std::vector<subcheck_ffi::EntryView> rows;
};
