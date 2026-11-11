// The findings list as the session last described it: the rows of every `Changed` splice applied
// in order, which is what makes `Summary.selected` an index into them.
#pragma once

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
    enum Role { LineRole = Qt::UserRole + 1, StateRole, MemberRole };

    using QAbstractListModel::QAbstractListModel;

    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    // The splice's rows in place of those it removes: where the two overlap, the rows change in
    // place, so the list keeps its delegates and its scroll position; the rest are inserted or
    // removed. False, and nothing changed, for a splice that does not fit the rows held: the list
    // would no longer be the session's.
    bool apply(const subcheck_ffi::Splice &splice);

    // The key of the entry at `row`, for `Select`; none outside the list.
    std::optional<subcheck_ffi::EntryKey> keyAt(int row) const;

    // The entry as one line of the list: the terminal's list line, but with the whole of both
    // lines where the terminal shows the words that changed, and no style.
    static QString line(const subcheck_ffi::EntryView &entry);

    // The entry's decision, for its colour: "pending", "accepted", "rejected", or "mixed" when its
    // decided members differ.
    static QString state(const subcheck_ffi::EntryView &entry);

private:
    std::vector<subcheck_ffi::EntryView> rows;
};
