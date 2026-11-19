// The sidebar's three lists: the reviews (the store's show folders), what the list shows (the
// filter, with each one's count) and the episodes (the view's files). Each as review last
// described it; what a click on one asks of review is the bridge's.
#pragma once

#include "spliced.h"
#include "subcheck_ffi.hpp"

#include <QAbstractListModel>
#include <QString>
#include <QtQml/qqmlregistration.h>

#include <optional>
#include <string>
#include <vector>

// The reviews: the store's show folders, newest first, as review last sent them whole.
class FoldersModel : public QAbstractListModel {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("the session's bridge makes the one list")

public:
    // What its view is called, its path (two can share a name), how many of its findings are
    // still to review, and whether the view shown is of it.
    enum Role { NameRole = Qt::UserRole + 1, PathRole, CountRole, OpenRole };

    using QAbstractListModel::QAbstractListModel;

    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    // The list in place of the one held. The same folders in the same order change in place, as
    // a decision's new count does, so that a view keeps its delegates; any other list is new.
    void set(const std::vector<subcheck_ffi::FolderView> &folders);

    // The path of the folder at `row`, for `OpenFolder`; none outside the list.
    std::optional<std::string> pathAt(int row) const;

private:
    std::vector<subcheck_ffi::FolderView> folders;
};

// What the list can show, in the order the sidebar lists them and the filter steps through
// them: all the findings, then those to review, accepted and rejected, each with its count.
class FiltersModel : public QAbstractListModel {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("the session's bridge makes the one list")

public:
    // Its name, how many findings it shows, and whether the list shows it now.
    enum Role { NameRole = Qt::UserRole + 1, CountRole, CurrentRole };

    explicit FiltersModel(QObject *parent = nullptr);

    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    // The counts and the filter of `summary`, which every change carries.
    void show(const subcheck_ffi::Summary &summary);

    // The filter at `row`; none outside the list.
    std::optional<subcheck_ffi::Filter> filterAt(int row) const;
    // The filter after the one shown now, the first after the last.
    subcheck_ffi::Filter next() const;
    // The name of the filter shown now and its count, as the list's heading says them.
    QString heading() const;

private:
    struct Shown {
        subcheck_ffi::Filter filter;
        QString name;
        uint64_t count = 0;
    };

    std::vector<Shown> rows;
    subcheck_ffi::Filter current = subcheck_ffi::Filter::kAll;
};

// The episodes: the view's files, as review's splices of them left them, and which one is marked
// as the current episode.
class EpisodesModel : public QAbstractListModel {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("the session's bridge makes the one list")

public:
    // The name the list gives it, its path, where the check or apply is with it in a word, and
    // whether it is the one marked.
    enum Role { LabelRole = Qt::UserRole + 1, PathRole, StageRole, CurrentRole };

    using QAbstractListModel::QAbstractListModel;

    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    // The splice's files in place of those it removes (spliced.h).
    bool apply(const subcheck_ffi::FilesSplice &splice);
    // The file to mark as the current episode; empty, or one not in the list, marks none.
    void follow(const std::string &file);

    // The path of the file at `row`, for `SelectFile`, and the name the list gives it; none
    // outside the list.
    std::optional<std::string> pathAt(int row) const;
    std::optional<std::string> labelAt(int row) const;

    // Where a file is in a word: apply's stage while an apply has it, else the check's.
    static QString stage(const subcheck_ffi::FileView &file);

private:
    template <typename M, typename I>
    friend bool splice(M &, std::vector<I> &, uint64_t, uint64_t, const std::vector<I> &);

    int rowOf(const std::string &file) const;

    std::vector<subcheck_ffi::FileView> files;
    std::string followed;
};
