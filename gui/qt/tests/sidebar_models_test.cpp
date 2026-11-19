// The sidebar's three lists, with no session, each under Qt's own checker of a model's signals and
// indexes: the reviews as review sends them whole, the filters with their counts, and the
// episodes on their splices, one of them marked.
#include "sidebar_models.h"
#include "words.h"

#include <QAbstractItemModelTester>
#include <QSignalSpy>
#include <QTest>

#include <memory>
#include <string>
#include <utility>
#include <vector>

using namespace subcheck_ffi;

namespace {

FileView file(std::string name, ScanStage stage = ScanStage(ScanStage::kDone{2})) {
    return FileView{"/m/S1/" + name + ".mkv", name, std::move(stage), std::nullopt, 400, false};
}

FileView applying(std::string name, FileApply apply) {
    FileView f = file(std::move(name));
    f.applying = std::move(apply);
    return f;
}

template <typename Model> QVariant role(const Model &model, int row, int role) {
    return model.data(model.index(row), role);
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
        const auto labels = [](const EpisodesModel &model) {
            QStringList all;
            for (int row = 0; row < model.rowCount(); ++row) {
                all << role(model, row, EpisodesModel::LabelRole).toString();
            }
            return all;
        };
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
        QCOMPARE(model.roleNames().size(), 4);
    }

    // Where a file is in a word: apply's stage while an apply has it, else the check's.
    void eachStageOfACheckOrAnApplyHasItsWord() {
        const std::vector<std::pair<FileView, QString>> said = {
            {file("a", ScanStage(ScanStage::kQueued{})), "Waiting"},
            {file("b", ScanStage(ScanStage::kQueuedAgain{})), "Waiting to check again"},
            {file("c", ScanStage(ScanStage::kReading{})), "Checking"},
            {file("d", ScanStage(ScanStage::kAsking{1, 3})), "Checking"},
            {file("e", ScanStage(ScanStage::kDone{0})), "Checked"},
            {file("f", ScanStage(ScanStage::kFailed{"bad EBML"})), "Failed"},
            {applying("g", FileApply(FileApply::kQueued{})), "Waiting to apply"},
            {applying("h", FileApply(FileApply::kAt{ApplyStage::kRemux})), "Applying"},
            {applying("i", FileApply(FileApply::kDone{false, true, {1, 0, 0, 0}})), "Applied"},
            {applying("j", FileApply(FileApply::kDone{true, false, {1, 0, 0, 0}})), "Previewed"},
            {applying("k", FileApply(FileApply::kFailed{"disk full"})), "Apply failed"},
        };
        for (const auto &[f, words] : said) {
            QCOMPARE(EpisodesModel::stage(f), words);
        }
    }
};

QTEST_GUILESS_MAIN(SidebarModelsTest)
#include "sidebar_models_test.moc"
