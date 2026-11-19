#include "sidebar_models.h"

#include "overloaded.h"
#include "words.h"

#include <algorithm>
#include <variant>

using namespace subcheck_ffi;

namespace {

bool valid(const QAbstractListModel &model, const QModelIndex &index) {
    return model.checkIndex(index, QAbstractItemModel::CheckIndexOption::IndexIsValid |
                                       QAbstractItemModel::CheckIndexOption::ParentIsInvalid);
}

} // namespace

int FoldersModel::rowCount(const QModelIndex &parent) const {
    return parent.isValid() ? 0 : static_cast<int>(folders.size());
}

QVariant FoldersModel::data(const QModelIndex &index, int role) const {
    if (!valid(*this, index)) {
        return {};
    }
    const FolderView &folder = folders[static_cast<size_t>(index.row())];
    switch (role) {
    case Qt::DisplayRole:
    case NameRole:
        return text(folder.name);
    case PathRole:
        return text(folder.path);
    case CountRole:
        return static_cast<int>(folder.to_review);
    case OpenRole:
        return folder.open;
    default:
        return {};
    }
}

QHash<int, QByteArray> FoldersModel::roleNames() const {
    return {{NameRole, "name"}, {PathRole, "path"}, {CountRole, "count"}, {OpenRole, "open"}};
}

void FoldersModel::set(const std::vector<FolderView> &list) {
    const bool same =
        list.size() == folders.size() &&
        std::equal(list.begin(), list.end(), folders.begin(),
                   [](const FolderView &a, const FolderView &b) { return a.path == b.path; });
    if (!same) {
        beginResetModel();
        folders = list;
        endResetModel();
        return;
    }
    folders = list;
    if (!folders.empty()) {
        emit dataChanged(index(0), index(rowCount() - 1));
    }
}

std::optional<std::string> FoldersModel::pathAt(int row) const {
    if (row < 0 || row >= rowCount()) {
        return std::nullopt;
    }
    return folders[static_cast<size_t>(row)].path;
}

FiltersModel::FiltersModel(QObject *parent)
    : QAbstractListModel(parent), rows{{Filter::kAll, QStringLiteral("All"), 0},
                                       {Filter::kPending, QStringLiteral("To Review"), 0},
                                       {Filter::kAccepted, QStringLiteral("Accepted"), 0},
                                       {Filter::kRejected, QStringLiteral("Rejected"), 0}} {}

int FiltersModel::rowCount(const QModelIndex &parent) const {
    return parent.isValid() ? 0 : static_cast<int>(rows.size());
}

QVariant FiltersModel::data(const QModelIndex &index, int role) const {
    if (!valid(*this, index)) {
        return {};
    }
    const Shown &row = rows[static_cast<size_t>(index.row())];
    switch (role) {
    case Qt::DisplayRole:
    case NameRole:
        return row.name;
    case CountRole:
        return static_cast<int>(row.count);
    case CurrentRole:
        return row.filter == current;
    default:
        return {};
    }
}

QHash<int, QByteArray> FiltersModel::roleNames() const {
    return {{NameRole, "name"}, {CountRole, "count"}, {CurrentRole, "current"}};
}

void FiltersModel::show(const Summary &summary) {
    for (Shown &row : rows) {
        switch (row.filter) {
        case Filter::kAll:
            row.count = summary.pending + summary.accepted + summary.rejected;
            break;
        case Filter::kPending:
            row.count = summary.pending;
            break;
        case Filter::kAccepted:
            row.count = summary.accepted;
            break;
        case Filter::kRejected:
            row.count = summary.rejected;
            break;
        }
    }
    current = summary.filter;
    emit dataChanged(index(0), index(rowCount() - 1));
}

std::optional<Filter> FiltersModel::filterAt(int row) const {
    if (row < 0 || row >= rowCount()) {
        return std::nullopt;
    }
    return rows[static_cast<size_t>(row)].filter;
}

Filter FiltersModel::next() const {
    for (size_t i = 0; i < rows.size(); ++i) {
        if (rows[i].filter == current) {
            return rows[(i + 1) % rows.size()].filter;
        }
    }
    return rows.front().filter;
}

QString FiltersModel::heading() const {
    for (const Shown &row : rows) {
        if (row.filter == current) {
            const QString name =
                row.filter == Filter::kAll ? QStringLiteral("All Findings") : row.name;
            return QStringLiteral("%1 · %2").arg(name).arg(row.count);
        }
    }
    return {};
}

int EpisodesModel::rowCount(const QModelIndex &parent) const {
    return parent.isValid() ? 0 : static_cast<int>(files.size());
}

QVariant EpisodesModel::data(const QModelIndex &index, int role) const {
    if (!valid(*this, index)) {
        return {};
    }
    const FileView &file = files[static_cast<size_t>(index.row())];
    switch (role) {
    case Qt::DisplayRole:
    case LabelRole:
        return text(file.label);
    case PathRole:
        return text(file.path);
    case StageRole:
        return stage(file);
    case CurrentRole:
        return !followed.empty() && file.path == followed;
    default:
        return {};
    }
}

QHash<int, QByteArray> EpisodesModel::roleNames() const {
    return {
        {LabelRole, "label"}, {PathRole, "path"}, {StageRole, "stage"}, {CurrentRole, "current"}};
}

bool EpisodesModel::apply(const FilesSplice &change) {
    return splice(*this, files, change.at, change.removed, change.inserted);
}

void EpisodesModel::follow(const std::string &file) {
    if (file == followed) {
        return;
    }
    const int was = rowOf(followed);
    followed = file;
    const int now = rowOf(followed);
    for (int row : {was, now}) {
        if (row >= 0) {
            emit dataChanged(index(row), index(row), {CurrentRole});
        }
    }
}

std::optional<std::string> EpisodesModel::pathAt(int row) const {
    if (row < 0 || row >= rowCount()) {
        return std::nullopt;
    }
    return files[static_cast<size_t>(row)].path;
}

std::optional<std::string> EpisodesModel::labelAt(int row) const {
    if (row < 0 || row >= rowCount()) {
        return std::nullopt;
    }
    return files[static_cast<size_t>(row)].label;
}

int EpisodesModel::rowOf(const std::string &file) const {
    if (file.empty()) {
        return -1;
    }
    for (size_t i = 0; i < files.size(); ++i) {
        if (files[i].path == file) {
            return static_cast<int>(i);
        }
    }
    return -1;
}

QString EpisodesModel::stage(const FileView &file) {
    if (file.applying) {
        return std::visit(
            Overloaded{
                [](const FileApply::kQueued &) { return QStringLiteral("Waiting to apply"); },
                [](const FileApply::kAt &) { return QStringLiteral("Applying"); },
                [](const FileApply::kDone &d) {
                    return d.dry_run ? QStringLiteral("Previewed") : QStringLiteral("Applied");
                },
                [](const FileApply::kFailed &) { return QStringLiteral("Apply failed"); },
            },
            file.applying->get_variant());
    }
    return std::visit(Overloaded{
                          [](const ScanStage::kQueued &) { return QStringLiteral("Waiting"); },
                          [](const ScanStage::kQueuedAgain &) {
                              return QStringLiteral("Waiting to check again");
                          },
                          [](const ScanStage::kReading &) { return QStringLiteral("Checking"); },
                          [](const ScanStage::kAsking &) { return QStringLiteral("Checking"); },
                          [](const ScanStage::kDone &) { return QStringLiteral("Checked"); },
                          [](const ScanStage::kFailed &) { return QStringLiteral("Failed"); },
                      },
                      file.stage.get_variant());
}
