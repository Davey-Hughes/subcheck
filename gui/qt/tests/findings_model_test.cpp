// The list's model on splices of every shape a session sends, each under Qt's own checker of a
// model's signals and indexes; and each entry as one line.
#include "findings_model.h"

#include <QAbstractItemModelTester>
#include <QSignalSpy>
#include <QTest>

#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <vector>

using namespace subcheck_ffi;

namespace {

// A row of finding `id` at 0:00:01, pending, with the fixture's line and its correction.
EntryView row(uint64_t id, std::optional<State> state = State::kPending) {
    return EntryView{
        .key = EntryKey(EntryKey::kRow{{id}}),
        .state = state,
        .is_member = false,
        .expanded = false,
        .label = "E01",
        .shown = 1,
        .total = 1,
        .at_ms = 1000,
        .original = "We should leave befor it rains.",
        .wording = "We should leave before it rains.",
        .style = "Default",
        .marks = {},
        .track = std::nullopt,
        .fixes = {},
        .checking = false,
        .note = std::nullopt,
        .mixed = false,
    };
}

// A member of an expanded row: finding `id`, in E02.
EntryView member(uint64_t id) {
    EntryView entry = row(id);
    entry.key = EntryKey(EntryKey::kMember{id});
    entry.is_member = true;
    entry.label = "E02";
    return entry;
}

// The row of findings 1 and 2, and whether it is expanded.
EntryView group(bool expanded) {
    EntryView entry = row(1);
    entry.key = EntryKey(EntryKey::kRow{{1, 2}});
    entry.expanded = expanded;
    entry.shown = 2;
    entry.total = 2;
    return entry;
}

Splice splice(uint64_t at, uint64_t removed, std::vector<EntryView> inserted) {
    return Splice{at, removed, std::move(inserted)};
}

// The finding IDs of the key at `row`, a member's alone.
std::vector<uint64_t> ids(const FindingsModel &model, int row) {
    std::optional<EntryKey> key = model.keyAt(row);
    if (!key) {
        return {};
    }
    if (const auto *r = std::get_if<EntryKey::kRow>(&key->get_variant())) {
        return r->v1;
    }
    return {std::get<EntryKey::kMember>(key->get_variant()).v1};
}

QString line(const FindingsModel &model, int row) {
    return model.data(model.index(row), FindingsModel::LineRole).toString();
}

} // namespace

class FindingsModelTest : public QObject {
    Q_OBJECT

    std::unique_ptr<FindingsModel> model;
    std::unique_ptr<QAbstractItemModelTester> tester;

private slots:
    void init() {
        model = std::make_unique<FindingsModel>();
        tester = std::make_unique<QAbstractItemModelTester>(
            model.get(), QAbstractItemModelTester::FailureReportingMode::QtTest);
    }

    void cleanup() {
        tester.reset();
        model.reset();
    }

    void theFirstSpliceInsertsEveryRow() {
        QSignalSpy inserted(model.get(), &QAbstractItemModel::rowsInserted);
        QVERIFY(model->apply(splice(0, 0, {row(1), row(3), row(5)})));
        QCOMPARE(model->rowCount(), 3);
        QCOMPARE(inserted.count(), 1);
        QCOMPARE(inserted[0][1].toInt(), 0);
        QCOMPARE(inserted[0][2].toInt(), 2);
        QCOMPARE(ids(*model, 2), std::vector<uint64_t>{5});
    }

    void anEmptyRunInsertsNothing() {
        QSignalSpy inserted(model.get(), &QAbstractItemModel::rowsInserted);
        QVERIFY(model->apply(splice(0, 0, {})));
        QCOMPARE(model->rowCount(), 0);
        QCOMPARE(inserted.count(), 0);
    }

    void aDecisionChangesItsRowInPlace() {
        QVERIFY(model->apply(splice(0, 0, {row(1), row(3), row(5)})));
        QSignalSpy changed(model.get(), &QAbstractItemModel::dataChanged);
        QSignalSpy inserted(model.get(), &QAbstractItemModel::rowsInserted);
        QSignalSpy removed(model.get(), &QAbstractItemModel::rowsRemoved);
        QVERIFY(model->apply(splice(1, 1, {row(3, State::kAccepted)})));
        QCOMPARE(changed.count(), 1);
        QCOMPARE(changed[0][0].toModelIndex().row(), 1);
        QCOMPARE(changed[0][1].toModelIndex().row(), 1);
        QCOMPARE(inserted.count() + removed.count(), 0);
        QCOMPARE(model->data(model->index(1), FindingsModel::StateRole).toString(), "accepted");
    }

    void expandingChangesTheRowAndInsertsItsMembers() {
        QVERIFY(model->apply(splice(0, 0, {group(false), row(5)})));
        QSignalSpy changed(model.get(), &QAbstractItemModel::dataChanged);
        QSignalSpy inserted(model.get(), &QAbstractItemModel::rowsInserted);
        QVERIFY(model->apply(splice(0, 1, {group(true), member(1), member(2)})));
        QCOMPARE(model->rowCount(), 4);
        QCOMPARE(changed.count(), 1);
        QCOMPARE(changed[0][0].toModelIndex().row(), 0);
        QCOMPARE(inserted.count(), 1);
        QCOMPARE(inserted[0][1].toInt(), 1);
        QCOMPARE(inserted[0][2].toInt(), 2);
        QCOMPARE(ids(*model, 2), std::vector<uint64_t>{2});
        QCOMPARE(ids(*model, 3), std::vector<uint64_t>{5});
    }

    void expandingARowAfterTheFirstInsertsItsMembersAfterIt() {
        QVERIFY(model->apply(splice(0, 0, {row(5), group(false)})));
        QSignalSpy changed(model.get(), &QAbstractItemModel::dataChanged);
        QSignalSpy inserted(model.get(), &QAbstractItemModel::rowsInserted);
        QVERIFY(model->apply(splice(1, 1, {group(true), member(1), member(2)})));
        QCOMPARE(model->rowCount(), 4);
        QCOMPARE(changed.count(), 1);
        QCOMPARE(changed[0][0].toModelIndex().row(), 1);
        QCOMPARE(changed[0][1].toModelIndex().row(), 1);
        QCOMPARE(inserted.count(), 1);
        QCOMPARE(inserted[0][1].toInt(), 2);
        QCOMPARE(inserted[0][2].toInt(), 3);
        QCOMPARE(ids(*model, 3), std::vector<uint64_t>{2});
    }

    // A decision on an expanded row decides its members with it: one change over all three.
    void aDecisionOnAnExpandedRowChangesItAndItsMembersInPlace() {
        QVERIFY(model->apply(splice(0, 0, {group(true), member(1), member(2), row(5)})));
        std::vector<EntryView> accepted = {group(true), member(1), member(2)};
        for (EntryView &entry : accepted) {
            entry.state = State::kAccepted;
        }
        QSignalSpy changed(model.get(), &QAbstractItemModel::dataChanged);
        QSignalSpy inserted(model.get(), &QAbstractItemModel::rowsInserted);
        QSignalSpy removed(model.get(), &QAbstractItemModel::rowsRemoved);
        QVERIFY(model->apply(splice(0, 3, accepted)));
        QCOMPARE(model->rowCount(), 4);
        QCOMPARE(changed.count(), 1);
        QCOMPARE(changed[0][0].toModelIndex().row(), 0);
        QCOMPARE(changed[0][1].toModelIndex().row(), 2);
        QCOMPARE(inserted.count() + removed.count(), 0);
        QCOMPARE(model->data(model->index(2), FindingsModel::StateRole).toString(), "accepted");
        QCOMPARE(model->data(model->index(3), FindingsModel::StateRole).toString(), "pending");
    }

    void collapsingChangesTheRowAndRemovesItsMembers() {
        QVERIFY(model->apply(splice(0, 0, {group(true), member(1), member(2), row(5)})));
        QSignalSpy changed(model.get(), &QAbstractItemModel::dataChanged);
        QSignalSpy removed(model.get(), &QAbstractItemModel::rowsRemoved);
        QVERIFY(model->apply(splice(0, 3, {group(false)})));
        QCOMPARE(model->rowCount(), 2);
        QCOMPARE(changed.count(), 1);
        QCOMPARE(removed.count(), 1);
        QCOMPARE(removed[0][1].toInt(), 1);
        QCOMPARE(removed[0][2].toInt(), 2);
        QCOMPARE(ids(*model, 1), std::vector<uint64_t>{5});
    }

    void aSpliceOfNoRowsRemovesThem() {
        QVERIFY(model->apply(splice(0, 0, {row(1), row(3), row(5)})));
        QSignalSpy changed(model.get(), &QAbstractItemModel::dataChanged);
        QSignalSpy removed(model.get(), &QAbstractItemModel::rowsRemoved);
        QVERIFY(model->apply(splice(1, 2, {})));
        QCOMPARE(model->rowCount(), 1);
        QCOMPARE(changed.count(), 0);
        QCOMPARE(removed.count(), 1);
        QCOMPARE(removed[0][1].toInt(), 1);
        QCOMPARE(removed[0][2].toInt(), 2);
        QCOMPARE(ids(*model, 0), std::vector<uint64_t>{1});
    }

    // A splice may start just past the last row: a pure insert there appends.
    void aSpliceAtTheEndAppends() {
        QVERIFY(model->apply(splice(0, 0, {row(1), row(3)})));
        QSignalSpy inserted(model.get(), &QAbstractItemModel::rowsInserted);
        QVERIFY(model->apply(splice(2, 0, {row(5)})));
        QCOMPARE(model->rowCount(), 3);
        QCOMPARE(inserted.count(), 1);
        QCOMPARE(inserted[0][1].toInt(), 2);
        QCOMPARE(inserted[0][2].toInt(), 2);
        QCOMPARE(ids(*model, 2), std::vector<uint64_t>{5});
    }

    void aSpliceThatDoesNotFitChangesNothing() {
        QVERIFY(model->apply(splice(0, 0, {row(1), row(3)})));
        QSignalSpy changed(model.get(), &QAbstractItemModel::dataChanged);
        QSignalSpy inserted(model.get(), &QAbstractItemModel::rowsInserted);
        QSignalSpy removed(model.get(), &QAbstractItemModel::rowsRemoved);
        QVERIFY(!model->apply(splice(3, 0, {row(5)})));
        QVERIFY(!model->apply(splice(1, 2, {})));
        QCOMPARE(changed.count(), 0);
        QCOMPARE(inserted.count(), 0);
        QCOMPARE(removed.count(), 0);
        QCOMPARE(model->rowCount(), 2);
        QCOMPARE(ids(*model, 1), std::vector<uint64_t>{3});
    }

    void aKeyIsTheEntrysOwnAndNoneOutsideTheList() {
        QVERIFY(model->apply(splice(0, 0, {group(true), member(2)})));
        QCOMPARE(ids(*model, 0), (std::vector<uint64_t>{1, 2}));
        QCOMPARE(ids(*model, 1), std::vector<uint64_t>{2});
        QVERIFY(!model->keyAt(-1));
        QVERIFY(!model->keyAt(2));
    }

    void anEntryIsOneLine() {
        EntryView timing = row(7, std::nullopt);
        timing.total = 3;
        timing.shown = 2;
        timing.track = ListTrack{4, "Signs", "jpn", true};
        timing.fixes = {FixCount{Fix(Fix::kShift{-3}), 2}};
        timing.marks = {MarkCount{MarkKind::kWouldApply, 2}};
        EntryView late = member(9);
        late.at_ms = -40;
        late.state = State::kRejected;
        EntryView fixed = member(8);
        fixed.fixes = {FixCount{Fix(Fix::kPartial{}), 1}};
        EntryView hours = member(10);
        hours.at_ms = 3723456;
        QVERIFY(model->apply(splice(
            0, 0, {row(1), group(true), timing, late, fixed, row(11, State::kAccepted), hours})));
        QCOMPARE(line(*model, 0),
                 "· E01 0:00:01.00 We should leave befor it rains. → We should leave before it "
                 "rains.");
        QCOMPARE(line(*model, 1),
                 "· ×2 0:00:01.00 We should leave befor it rains. → We should leave before it "
                 "rains.");
        QCOMPARE(line(*model, 2), "± ×2/3 Signs  +3 frames  would apply");
        QCOMPARE(line(*model, 3), "    ✗ E02 0:00:00.00");
        QCOMPARE(line(*model, 4), "    · E02 partial");
        QVERIFY(line(*model, 5).startsWith("✓ E01 0:00:01.00 "));
        QCOMPARE(line(*model, 6), "    · E02 1:02:03.45");
        QCOMPARE(model->data(model->index(0), Qt::DisplayRole).toString(), line(*model, 0));
        QCOMPARE(model->data(model->index(2), FindingsModel::StateRole).toString(), "mixed");
        QCOMPARE(model->data(model->index(3), FindingsModel::StateRole).toString(), "rejected");
        QCOMPARE(model->data(model->index(0), FindingsModel::StateRole).toString(), "pending");
        QCOMPARE(model->data(model->index(3), FindingsModel::MemberRole).toBool(), true);
        QCOMPARE(model->data(model->index(0), FindingsModel::MemberRole).toBool(), false);
        QVERIFY(!model->data(model->index(0), Qt::DecorationRole).isValid());
        QCOMPARE(model->roleNames().value(FindingsModel::LineRole), "line");
    }

    // An entry's marks, fixes and track, from the data the session sends, in the terminal's list's
    // words: a mark alone when every finding the entry shows has it, else each with its count; a
    // fix alone when all agree, else each with its count; a track by its own name, with its ID
    // when that is not its own, or by its ID alone.
    void itsMarksFixesAndTrackAreTheTerminalsWords() {
        EntryView entry = group(false);
        const auto said = [&entry](std::vector<MarkCount> marks) {
            entry.marks = std::move(marks);
            return FindingsModel::line(entry).section(QStringLiteral("rains.  "), 1);
        };
        QCOMPARE(said({{MarkKind::kByHand, 2}}), "by hand");
        QCOMPARE(said({{MarkKind::kByHand, 1}}), "by hand 1");
        QCOMPARE(said({{MarkKind::kApplied, 3}, {MarkKind::kNotFound, 1}}),
                 "applied 3 · not found 1");
        QCOMPARE(said({}), "");
        const std::vector<std::pair<MarkKind, QString>> words = {
            {MarkKind::kByHand, "by hand"},
            {MarkKind::kNotFound, "not found"},
            {MarkKind::kWouldApply, "would apply"},
            {MarkKind::kAlreadyApplied, "already applied"},
            {MarkKind::kWouldBeByHand, "would be by hand"},
            {MarkKind::kWouldNotBeFound, "would not be found"},
            {MarkKind::kFailed, "failed"},
            {MarkKind::kWouldFail, "would fail"},
            {MarkKind::kApplied, "applied"},
        };
        for (const auto &[kind, word] : words) {
            QCOMPARE(said({{kind, 2}}), word);
        }

        EntryView timing = row(7);
        timing.total = 9;
        timing.shown = 9;
        timing.track = ListTrack{4, "Signs", "jpn", true};
        timing.fixes = {{Fix(Fix::kShift{1}), 9}};
        QCOMPARE(FindingsModel::line(timing), "· ×9 Signs  −1 frame");
        timing.fixes = {{Fix(Fix::kShift{-2}), 5}, {Fix(Fix::kPartial{}), 4}};
        QCOMPARE(FindingsModel::line(timing), "· ×9 Signs  +2 frames ×5 · partial ×4");
        timing.fixes = {{Fix(Fix::kShift{0}), 9}};
        timing.track = ListTrack{4, "Signs", "jpn", false};
        QCOMPARE(FindingsModel::line(timing), "· ×9 track 4 Signs  +0 frames");
        timing.track = ListTrack{4, std::nullopt, "ja", false};
        QCOMPARE(FindingsModel::line(timing), "· ×9 track 4 (ja)  +0 frames");
        timing.track = ListTrack{4, std::nullopt, std::nullopt, false};
        QCOMPARE(FindingsModel::line(timing), "· ×9 track 4  +0 frames");
    }
};

QTEST_GUILESS_MAIN(FindingsModelTest)
#include "findings_model_test.moc"
