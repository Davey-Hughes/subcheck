// A list model's items as review's splices describe them, each applied in order, which is what
// makes an index review sends (the selected row, the selected file) an index into them.
#pragma once

#include <QAbstractListModel>

#include <algorithm>
#include <cstdint>
#include <vector>

// `inserted` in place of the `removed` items at `at`: where the two overlap, the items change in
// place, so a view keeps its delegates and its scroll position; the rest are inserted or removed.
// False, and nothing changed, for a splice that does not fit the items held: the list would no
// longer be review's. A friend of each model, as it calls the model's protected beginInsertRows,
// endInsertRows, beginRemoveRows and endRemoveRows.
template <typename Model, typename Item>
bool splice(Model &model, std::vector<Item> &items, uint64_t at, uint64_t removed,
            const std::vector<Item> &inserted) {
    const size_t held = items.size();
    if (at > held || removed > held - at) {
        return false;
    }
    const int from = static_cast<int>(at);
    const int gone = static_cast<int>(removed);
    const int added = static_cast<int>(inserted.size());
    const int changed = std::min(gone, added);
    for (int i = 0; i < changed; ++i) {
        items[static_cast<size_t>(from + i)] = inserted[static_cast<size_t>(i)];
    }
    if (changed > 0) {
        emit model.dataChanged(model.index(from), model.index(from + changed - 1));
    }
    if (added > changed) {
        model.beginInsertRows({}, from + changed, from + added - 1);
        items.insert(items.begin() + from + changed, inserted.begin() + changed, inserted.end());
        model.endInsertRows();
    } else if (gone > changed) {
        model.beginRemoveRows({}, from + changed, from + gone - 1);
        items.erase(items.begin() + from + changed, items.begin() + from + gone);
        model.endRemoveRows();
    }
    return true;
}
