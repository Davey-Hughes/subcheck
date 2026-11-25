// The sidebar's three lists, with no session, each under Qt's own checker of a model's signals and
// indexes: the reviews as review sends them whole, the filters with their counts, and the
// episodes on their splices, one of them marked, in review's order or with those the check found
// nothing in last.
#include "sidebar_models.h"
#include "words.h"

#include <QAbstractItemModelTester>
#include <QSignalSpy>
#include <QTest>

#include <memory>
#include <string>
#include <tuple>
#include <utility>
#include <vector>

using namespace subcheck_ffi;

namespace {

FileView file(std::string name, ScanStage stage = ScanStage(ScanStage::kDone{2})) {
    return FileView{"/m/S1/" + name + ".mkv", name, std::move(stage), std::nullopt, 400, false};
}

FileView applying(std::string name, FileApply apply,
                  ScanStage stage = ScanStage(ScanStage::kDone{2})) {
    FileView f = file(std::move(name), std::move(stage));
    f.applying = std::move(apply);
    return f;
}

template <typename Model> QVariant role(const Model &model, int row, int role) {
    return model.data(model.index(row), role);
}

// The episodes `model` lists, by their labels, in its order.
QStringList labels(const QAbstractItemModel &model) {
    QStringList all;
    for (int row = 0; row < model.rowCount(); ++row) {
        all << model.data(model.index(row, 0), EpisodesModel::LabelRole).toString();
    }
    return all;
}

// Five episodes, E01 to E05, of which E01 and E03 have no findings, and E04 is being checked.
std::vector<FileView> fiveEpisodes() {
    return {
        file("E01", ScanStage(ScanStage::kDone{0})), file("E02", ScanStage(ScanStage::kDone{2})),
        file("E03", ScanStage(ScanStage::kDone{0})), file("E04", ScanStage(ScanStage::kReading{})),
        file("E05", ScanStage(ScanStage::kDone{1}))};
}

// Thirty reviews, /m/Show 0 to /m/Show 29, more than the sidebar shows at once, of which the one
// at `open` is open; none for -1.
std::vector<FolderView> manyReviews(int open) {
    std::vector<FolderView> folders;
    for (int i = 0; i < 30; ++i) {
        const std::string name = "Show " + std::to_string(i);
        folders.push_back(FolderView{"/m/" + name, name, 0, i == open});
    }
    return folders;
}

std::unique_ptr<QAbstractItemModelTester> tested(QAbstractItemModel *model) {
    return std::make_unique<QAbstractItemModelTester>(
        model, QAbstractItemModelTester::FailureReportingMode::QtTest);
}

Summary summary(Filter filter, uint64_t pending, uint64_t accepted, uint64_t rejected) {
    Summary s{};
    s.filter = filter;
    s.pending = pending;
    s.accepted = accepted;
    s.rejected = rejected;
    return s;
}

} // namespace

class SidebarModelsTest : public QObject {
    Q_OBJECT

private slots:
    // This program has no GUI application, so no palette: the window's colours are the light
    // ones, where reading a palette there would crash.
    void withNoGuiApplicationTheColoursAreTheLightOnes() {
        QVERIFY(!dark());
        QCOMPARE(stateLook(State::kPending, false).tone.name(), "#1f5fbf");
    }

    // Each role is its own field: values unlike each other.
    void aReviewIsItsNamePathCountAndWhetherItIsOpen() {
        FoldersModel model;
        const auto tester = tested(&model);
        model.set({{"/m/K-On!", "K-On!", 7, false}, {"/m/S1/Specials", "Specials", 0, true}});
        QCOMPARE(model.rowCount(), 2);
        QCOMPARE(role(model, 0, FoldersModel::NameRole).toString(), "K-On!");
        QCOMPARE(role(model, 0, FoldersModel::PathRole).toString(), "/m/K-On!");
        QCOMPARE(role(model, 0, FoldersModel::CountRole).toInt(), 7);
        QCOMPARE(role(model, 0, FoldersModel::OpenRole).toBool(), false);
        QCOMPARE(role(model, 1, FoldersModel::OpenRole).toBool(), true);
        QCOMPARE(role(model, 1, Qt::DisplayRole).toString(), "Specials");
        QCOMPARE(model.pathAt(1), std::optional<std::string>("/m/S1/Specials"));
        QVERIFY(!model.pathAt(2));
        QVERIFY(!model.pathAt(-1));
        QCOMPARE(model.roleNames().size(), 4);
    }

    // A decision's new count changes the list in place; another list, a folder more or one in
    // another place, is a new list.
    void theSameFoldersChangeInPlaceAndOthersMakeANewList() {
        FoldersModel model;
        const auto tester = tested(&model);
        model.set({{"/m/A", "A", 2, true}, {"/m/B", "B", 1, false}});
        QSignalSpy changed(&model, &QAbstractItemModel::dataChanged);
        QSignalSpy reset(&model, &QAbstractItemModel::modelReset);
        model.set({{"/m/A", "A", 1, true}, {"/m/B", "B", 1, false}});
        QCOMPARE(changed.count(), 1);
        QCOMPARE(reset.count(), 0);
        QCOMPARE(role(model, 0, FoldersModel::CountRole).toInt(), 1);
        model.set({{"/m/B", "B", 1, false}, {"/m/A", "A", 1, true}});
        QCOMPARE(reset.count(), 1);
        QCOMPARE(role(model, 0, FoldersModel::NameRole).toString(), "B");
        model.set({});
        QCOMPARE(reset.count(), 2);
        QCOMPARE(model.rowCount(), 0);
        model.set({});
        QCOMPARE(changed.count(), 1);
    }

    void eachFilterHasItsCountAndTheOneShownIsCurrent() {
        FiltersModel model;
        const auto tester = tested(&model);
        QCOMPARE(model.rowCount(), 4);
        QSignalSpy changed(&model, &QAbstractItemModel::dataChanged);
        model.show(summary(Filter::kAccepted, 4, 1, 2));
        // One decision moves the counts of several rows, and the sidebar draws what this says.
        QCOMPARE(changed.count(), 1);
        QCOMPARE(changed[0][0].toModelIndex().row(), 0);
        QCOMPARE(changed[0][1].toModelIndex().row(), 3);
        const QStringList names = {"All", "To Review", "Accepted", "Rejected"};
        const QList<int> counts = {7, 4, 1, 2};
        for (int row = 0; row < 4; ++row) {
            QCOMPARE(role(model, row, FiltersModel::NameRole).toString(), names[row]);
            QCOMPARE(role(model, row, FiltersModel::CountRole).toInt(), counts[row]);
            QCOMPARE(role(model, row, FiltersModel::CurrentRole).toBool(), row == 2);
        }
        QCOMPARE(model.heading(), "Accepted · 1");
        model.show(summary(Filter::kAll, 4, 1, 2));
        QCOMPARE(model.heading(), "All Findings · 7");
        QCOMPARE(model.filterAt(1), std::optional<Filter>(Filter::kPending));
        QVERIFY(!model.filterAt(4));
        QCOMPARE(model.roleNames().size(), 3);
    }

    // The filter after the one shown is the next in the sidebar's order, the first after the
    // last, so that stepping through them goes as the list reads.
    void theFilterAfterTheOneShownIsTheNextInTheList() {
        FiltersModel model;
        const std::vector<std::pair<Filter, Filter>> steps = {
            {Filter::kAll, Filter::kPending},
            {Filter::kPending, Filter::kAccepted},
            {Filter::kAccepted, Filter::kRejected},
            {Filter::kRejected, Filter::kAll},
        };
        for (const auto &[shown, after] : steps) {
            model.show(summary(shown, 0, 0, 0));
            QCOMPARE(model.next(), after);
        }
    }

    void theEpisodesFollowTheirSplices() {
        EpisodesModel model;
        const auto tester = tested(&model);
        QSignalSpy inserted(&model, &QAbstractItemModel::rowsInserted);
        QVERIFY(model.apply(FilesSplice{0, 0, {file("E01"), file("E02"), file("E03")}}));
        QCOMPARE(inserted.count(), 1);
        QSignalSpy changed(&model, &QAbstractItemModel::dataChanged);
        QVERIFY(model.apply(FilesSplice{1, 1, {file("E02", ScanStage(ScanStage::kReading{}))}}));
        QCOMPARE(changed.count(), 1);
        QCOMPARE(role(model, 1, EpisodesModel::StageRole).toString(), "Checking");
        QVERIFY(!model.apply(FilesSplice{4, 0, {file("E05")}}));
        QVERIFY(model.apply(FilesSplice{2, 1, {}}));
        QCOMPARE(model.rowCount(), 2);
        QCOMPARE(model.pathAt(1), std::optional<std::string>("/m/S1/E02.mkv"));
        QVERIFY(!model.pathAt(2));
    }

    // Where a splice's old and new files overlap they change in place, and only the rest is
    // inserted or removed, after them, so that a view keeps the rows the two share.
    void aSpliceChangesTheRowsItOverlapsAndInsertsOrRemovesTheRest() {
        EpisodesModel model;
        const auto tester = tested(&model);
        QVERIFY(model.apply(FilesSplice{0, 0, {file("A"), file("B"), file("C")}}));
        QSignalSpy changed(&model, &QAbstractItemModel::dataChanged);
        QSignalSpy inserted(&model, &QAbstractItemModel::rowsInserted);
        QSignalSpy removed(&model, &QAbstractItemModel::rowsRemoved);

        QVERIFY(model.apply(FilesSplice{1, 1, {file("X"), file("Y"), file("Z")}}));
        QCOMPARE(labels(model), QStringList({"A", "X", "Y", "Z", "C"}));
        QCOMPARE(changed.count(), 1);
        QCOMPARE(changed[0][0].toModelIndex().row(), 1);
        QCOMPARE(changed[0][1].toModelIndex().row(), 1);
        QCOMPARE(inserted.count(), 1);
        QCOMPARE(inserted[0][1].toInt(), 2);
        QCOMPARE(inserted[0][2].toInt(), 3);
        QCOMPARE(removed.count(), 0);

        QVERIFY(model.apply(FilesSplice{1, 3, {file("W")}}));
        QCOMPARE(labels(model), QStringList({"A", "W", "C"}));
        QCOMPARE(changed.count(), 2);
        QCOMPARE(changed[1][0].toModelIndex().row(), 1);
        QCOMPARE(changed[1][1].toModelIndex().row(), 1);
        QCOMPARE(inserted.count(), 1);
        QCOMPARE(removed.count(), 1);
        QCOMPARE(removed[0][1].toInt(), 2);
        QCOMPARE(removed[0][2].toInt(), 3);
    }

    void anEpisodeIsItsLabelPathStateAndWhetherItIsTheFindings() {
        EpisodesModel model;
        const auto tester = tested(&model);
        QVERIFY(model.apply(FilesSplice{0, 0, {file("E01"), file("E02"), file("E03")}}));
        QCOMPARE(role(model, 0, EpisodesModel::LabelRole).toString(), "E01");
        QCOMPARE(role(model, 0, EpisodesModel::PathRole).toString(), "/m/S1/E01.mkv");
        QCOMPARE(role(model, 0, EpisodesModel::StageRole).toString(), "Checked");
        QCOMPARE(role(model, 0, Qt::DisplayRole).toString(), "E01");
        QSignalSpy changed(&model, &QAbstractItemModel::dataChanged);
        model.follow("/m/S1/E02.mkv");
        QCOMPARE(changed.count(), 1);
        QCOMPARE(changed[0][0].toModelIndex().row(), 1);
        model.follow("/m/S1/E02.mkv");
        QCOMPARE(changed.count(), 1);
        model.follow("/m/S1/E03.mkv");
        QCOMPARE(changed.count(), 3);
        for (int row = 0; row < 3; ++row) {
            QCOMPARE(role(model, row, EpisodesModel::CurrentRole).toBool(), row == 2);
        }
        // A file that is not in the list, or none: no episode is marked.
        model.follow("/m/S2/E01.mkv");
        model.follow("");
        for (int row = 0; row < 3; ++row) {
            QCOMPARE(role(model, row, EpisodesModel::CurrentRole).toBool(), false);
        }
        QCOMPARE(model.roleNames().size(), 5);
    }

    // An episode the check found nothing in is greyed, and says so in its word, so that colour is
    // not the only sign; any other is not: one the check has yet to finish or failed on, or one an
    // apply has.
    void anEpisodeWithNoFindingsIsGreyedAndSaysSo() {
        const FileApply applied(FileApply::kDone{false, true, {1, 0, 0, 0}});
        const std::vector<std::tuple<FileView, bool, QString>> said = {
            {file("a", ScanStage(ScanStage::kDone{0})), true, "No findings"},
            {file("b", ScanStage(ScanStage::kDone{3})), false, "Checked"},
            {applying("c", applied, ScanStage(ScanStage::kDone{0})), false, "Applied"},
            {applying("d", FileApply(FileApply::kQueued{}), ScanStage(ScanStage::kDone{0})), false,
             "Waiting to apply"},
            {file("e", ScanStage(ScanStage::kFailed{"bad EBML"})), false, "Failed"},
            {file("f", ScanStage(ScanStage::kQueued{})), false, "Waiting"},
            {file("g", ScanStage(ScanStage::kQueuedAgain{})), false, "Waiting to check again"},
            {file("h", ScanStage(ScanStage::kReading{})), false, "Checking"},
            {file("i", ScanStage(ScanStage::kAsking{1, 3})), false, "Checking"},
        };
        std::vector<FileView> files;
        for (const auto &entry : said) {
            files.push_back(std::get<0>(entry));
        }
        EpisodesModel model;
        const auto tester = tested(&model);
        QVERIFY(model.apply(FilesSplice{0, 0, files}));
        for (int row = 0; row < static_cast<int>(said.size()); ++row) {
            const auto &[f, none, words] = said[static_cast<size_t>(row)];
            QCOMPARE(EpisodesModel::hasNoFindings(f), none);
            QCOMPARE(role(model, row, EpisodesModel::NoFindingsRole).toBool(), none);
            QCOMPARE(role(model, row, EpisodesModel::StageRole).toString(), words);
            QCOMPARE(model.hasNoFindingsAt(row), none);
        }
        QVERIFY(!model.hasNoFindingsAt(-1));
        QVERIFY(!model.hasNoFindingsAt(model.rowCount()));
        QCOMPARE(model.roleNames().value(EpisodesModel::NoFindingsRole), "noFindings");
    }

    // With Episodes Without Findings Last on, the episodes the check found nothing in follow the
    // rest, each group in review's order; off, the list is review's own. Each row of the list
    // names the row of review's that it shows, and none outside the list.
    void episodesWithoutFindingsGoLastKeepingCoresOrderInEachGroup() {
        EpisodesModel model;
        QVERIFY(model.apply(FilesSplice{0, 0, fiveEpisodes()}));
        EpisodeOrder order(&model);
        const auto tester = tested(&order);
        const auto rows = [&order] {
            std::vector<std::optional<int>> each;
            for (int row = 0; row < order.rowCount(); ++row) {
                each.push_back(order.sourceRow(row));
            }
            return each;
        };
        const QStringList reviews = {"E01", "E02", "E03", "E04", "E05"};
        QVERIFY(!order.noFindingsLast());
        QCOMPARE(labels(order), reviews);
        QVERIFY(rows() == (std::vector<std::optional<int>>{0, 1, 2, 3, 4}));

        QSignalSpy changed(&order, &EpisodeOrder::noFindingsLastChanged);
        order.setNoFindingsLast(true);
        QCOMPARE(changed.count(), 1);
        QVERIFY(order.noFindingsLast());
        QCOMPARE(labels(order), QStringList({"E02", "E04", "E05", "E01", "E03"}));
        QVERIFY(rows() == (std::vector<std::optional<int>>{1, 3, 4, 0, 2}));
        QVERIFY(!order.sourceRow(-1));
        QVERIFY(!order.sourceRow(5));
        // The same again changes nothing.
        order.setNoFindingsLast(true);
        QCOMPARE(changed.count(), 1);

        order.setNoFindingsLast(false);
        QCOMPARE(changed.count(), 2);
        QCOMPARE(labels(order), reviews);
        QVERIFY(rows() == (std::vector<std::optional<int>>{0, 1, 2, 3, 4}));
    }

    // While the episodes with no findings go last, one that a check takes up again goes back among
    // the rest, in review's place for it, and one the check leaves with none goes after them; an
    // episode review inserts goes in its own place. Each row of the list still opens the episode
    // it shows.
    void aRowThatGainsFindingsMovesBackAndRowsMapToTheirEpisodes() {
        EpisodesModel model;
        QVERIFY(model.apply(FilesSplice{0, 0, fiveEpisodes()}));
        EpisodeOrder order(&model);
        const auto tester = tested(&order);
        order.setNoFindingsLast(true);
        QVERIFY(model.apply(FilesSplice{0, 1, {file("E01", ScanStage(ScanStage::kReading{}))}}));
        QCOMPARE(labels(order), QStringList({"E01", "E02", "E04", "E05", "E03"}));
        QVERIFY(model.apply(FilesSplice{3, 1, {file("E04", ScanStage(ScanStage::kDone{0}))}}));
        QCOMPARE(labels(order), QStringList({"E01", "E02", "E05", "E03", "E04"}));
        QVERIFY(model.apply(FilesSplice{0, 0, {file("E00")}}));
        QCOMPARE(labels(order), QStringList({"E00", "E01", "E02", "E05", "E03", "E04"}));
        for (int row = 0; row < order.rowCount(); ++row) {
            const std::optional<int> at = order.sourceRow(row);
            QVERIFY(at);
            QCOMPARE(order.data(order.index(row, 0), EpisodesModel::PathRole).toString(),
                     QString::fromStdString(*model.pathAt(*at)));
        }
    }

    // The row of the list whose episode is marked, in the order the list is in, and none while
    // none is; and the list says it is in a new order once it is sorted again, each time its order
    // is changed and only then, so that the row read then is the new order's.
    void markedRowFollowsTheOrder() {
        EpisodesModel model;
        QVERIFY(model.apply(FilesSplice{0, 0, fiveEpisodes()}));
        EpisodeOrder order(&model);
        const auto tester = tested(&order);
        QCOMPARE(order.markedRow(), -1);
        std::vector<int> said;
        connect(&order, &EpisodeOrder::reordered, &order,
                [&said, &order] { said.push_back(order.markedRow()); });

        // E03, with no findings: third in review's order, and last with those without last.
        model.follow("/m/S1/E03.mkv");
        QCOMPARE(order.markedRow(), 2);
        order.setNoFindingsLast(true);
        QCOMPARE(order.markedRow(), 4);
        QVERIFY(said == std::vector<int>{4});
        order.setNoFindingsLast(true);
        QVERIFY(said == std::vector<int>{4});
        // E05, with findings: last in review's order, and third with those without last.
        model.follow("/m/S1/E05.mkv");
        QCOMPARE(order.markedRow(), 2);
        order.setNoFindingsLast(false);
        QCOMPARE(order.markedRow(), 4);
        QVERIFY((said == std::vector<int>{4, 4}));
        order.setNoFindingsLast(false);
        QCOMPARE(said.size(), size_t(2));

        model.follow("");
        QCOMPARE(order.markedRow(), -1);
    }

    // The mark moving onto a row of the list is said, through the order too, so that the sidebar
    // can scroll to it; and so is a splice that removes rows while the marked file stays listed,
    // as the list may have lost its place. Nothing else is: not the mark staying where it is or
    // going off the list, not a splice that changes rows in place or inserts them, as a check's
    // progress does at each step, and not one that removes the marked file.
    void theMarkMovingIsSaid() {
        EpisodesModel model;
        QVERIFY(model.apply(FilesSplice{0, 0, fiveEpisodes()}));
        EpisodeOrder order(&model);
        const auto tester = tested(&order);
        QSignalSpy followed(&model, &EpisodesModel::followed);
        QSignalSpy moved(&order, &EpisodeOrder::markMoved);

        model.follow("/m/S1/E02.mkv");
        QCOMPARE(followed.count(), 1);
        QCOMPARE(moved.count(), 1);
        model.follow("/m/S1/E02.mkv");
        QCOMPARE(followed.count(), 1);
        model.follow("/m/S2/E01.mkv");
        QCOMPARE(followed.count(), 1);
        QCOMPARE(moved.count(), 1);
        model.follow("/m/S1/E02.mkv");
        QCOMPARE(followed.count(), 2);
        QCOMPARE(moved.count(), 2);

        // E04 and E05 removed, after the marked E02.
        QVERIFY(model.apply(FilesSplice{3, 2, {}}));
        QCOMPARE(labels(model), QStringList({"E01", "E02", "E03"}));
        QCOMPARE(followed.count(), 3);
        QCOMPARE(moved.count(), 3);

        QVERIFY(model.apply(FilesSplice{0, 1, {file("E01", ScanStage(ScanStage::kReading{}))}}));
        QVERIFY(model.apply(FilesSplice{0, 0, {file("E00")}}));
        QCOMPARE(labels(model), QStringList({"E00", "E01", "E02", "E03"}));
        QCOMPARE(followed.count(), 3);
        QCOMPARE(moved.count(), 3);
        QVERIFY(model.apply(FilesSplice{1, 3, {}}));
        QCOMPARE(labels(model), QStringList({"E00"}));
        QCOMPARE(followed.count(), 3);
        QCOMPARE(moved.count(), 3);
    }

    // The open review's row, which the sidebar keeps in view, and each time it may have left the
    // view: another row open, or a new list, which a view shows from its top, with one open. The
    // same list with new counts changes in place, and keeps the view where it was.
    void theOpenReviewIsFoundAndItsMoveSaid() {
        FoldersModel model;
        const auto tester = tested(&model);
        QCOMPARE(model.openRow(), -1);
        QSignalSpy moved(&model, &FoldersModel::openMoved);
        QSignalSpy reset(&model, &QAbstractItemModel::modelReset);
        model.set(manyReviews(25));
        QCOMPARE(model.openRow(), 25);
        QCOMPARE(moved.count(), 1);

        std::vector<FolderView> counted = manyReviews(25);
        for (FolderView &each : counted) {
            each.to_review = 4;
        }
        model.set(counted);
        QCOMPARE(role(model, 25, FoldersModel::CountRole).toInt(), 4);
        QCOMPARE(model.openRow(), 25);
        QCOMPARE(moved.count(), 1);

        model.set(manyReviews(3));
        QCOMPARE(model.openRow(), 3);
        QCOMPARE(moved.count(), 2);
        QCOMPARE(reset.count(), 1);

        // Another list, the last folder gone, open at the same row.
        std::vector<FolderView> fewer = manyReviews(3);
        fewer.pop_back();
        model.set(fewer);
        QCOMPARE(reset.count(), 2);
        QCOMPARE(model.openRow(), 3);
        QCOMPARE(moved.count(), 3);

        // None open: the open row is another, though there is none to show.
        fewer[3].open = false;
        model.set(fewer);
        QCOMPARE(reset.count(), 2);
        QCOMPARE(model.openRow(), -1);
        QCOMPARE(moved.count(), 4);
    }

    // Where a file is in a word: apply's stage while an apply has it, else the check's.
    void eachStageOfACheckOrAnApplyHasItsWord() {
        const std::vector<std::pair<FileView, QString>> said = {
            {file("a", ScanStage(ScanStage::kQueued{})), "Waiting"},
            {file("b", ScanStage(ScanStage::kQueuedAgain{})), "Waiting to check again"},
            {file("c", ScanStage(ScanStage::kReading{})), "Checking"},
            {file("d", ScanStage(ScanStage::kAsking{1, 3})), "Checking"},
            {file("e", ScanStage(ScanStage::kDone{1})), "Checked"},
            {file("f", ScanStage(ScanStage::kDone{0})), "No findings"},
            {file("g", ScanStage(ScanStage::kFailed{"bad EBML"})), "Failed"},
            {applying("h", FileApply(FileApply::kQueued{})), "Waiting to apply"},
            {applying("i", FileApply(FileApply::kAt{ApplyStage::kRemux})), "Applying"},
            {applying("j", FileApply(FileApply::kDone{false, true, {1, 0, 0, 0}})), "Applied"},
            {applying("k", FileApply(FileApply::kDone{true, false, {1, 0, 0, 0}})), "Previewed"},
            {applying("l", FileApply(FileApply::kFailed{"disk full"})), "Apply failed"},
        };
        for (const auto &[f, words] : said) {
            QCOMPARE(EpisodesModel::stage(f), words);
        }
    }
};

QTEST_GUILESS_MAIN(SidebarModelsTest)
#include "sidebar_models_test.moc"
