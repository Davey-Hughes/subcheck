// The list's model on splices of every shape a session sends, each under Qt's own checker of a
// model's signals and indexes; and each entry as a Two Lines row draws it.
#include "findings_model.h"
#include "offscreen_main.h"
#include "words.h"

#include <QAbstractItemModelTester>
#include <QColor>
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
        .check = Check::kLlm,
        .original_changed = Span{16, 21},
        .wording_changed = Span{16, 22},
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

QVariant role(const FindingsModel &model, int row, FindingsModel::Role role) {
    return model.data(model.index(row), role);
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
        QCOMPARE(role(*model, 1, FindingsModel::StateWordRole).toString(), "Accepted");
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
        QCOMPARE(role(*model, 2, FindingsModel::StateWordRole).toString(), "Accepted");
        QCOMPARE(role(*model, 3, FindingsModel::StateWordRole).toString(), "To review");
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

    // Each role passes on its own part of the entry: values unlike each other, so that a role
    // that read another's field shows.
    void eachRoleIsItsOwnPartOfTheRow() {
        EntryView entry = group(false);
        entry.total = 3;
        entry.shown = 3;
        entry.label = "E07";
        entry.at_ms = 754'999;
        entry.state = State::kRejected;
        entry.original = "𝄞 We should leave befor it <rains>.";
        entry.wording = "𝄞 We should leave before it <rains>.";
        entry.original_changed = Span{19, 24};
        entry.wording_changed = Span{19, 25};
        QVERIFY(model->apply(splice(0, 0, {entry})));
        QCOMPARE(role(*model, 0, FindingsModel::KindRole).toString(), "Typo");
        QCOMPARE(role(*model, 0, FindingsModel::KindToneRole).value<QColor>(),
                 kindTone(Check::kLlm));
        QCOMPARE(role(*model, 0, FindingsModel::SeveralRole).toString(), "In 3 places");
        QCOMPARE(role(*model, 0, FindingsModel::PlaceRole).toString(), "E07 · 12:34");
        QCOMPARE(role(*model, 0, FindingsModel::GlyphRole).toString(), "✕");
        QCOMPARE(role(*model, 0, FindingsModel::StateWordRole).toString(), "Rejected");
        QCOMPARE(role(*model, 0, FindingsModel::StateToneRole).value<QColor>(),
                 stateLook(State::kRejected, false).tone);
        QCOMPARE(role(*model, 0, FindingsModel::LineRole).toString(),
                 rowLine(QString::fromStdString(entry.original), entry.original_changed,
                         QString::fromStdString(entry.wording), entry.wording_changed)
                     .whole);
        QVERIFY(role(*model, 0, FindingsModel::LineRole).toString().contains("&lt;rains&gt;"));
        QVERIFY(role(*model, 0, FindingsModel::LineRole).toString().contains(">befor<"));
        QCOMPARE(role(*model, 0, FindingsModel::MemberRole).toBool(), false);
        QCOMPARE(model->data(model->index(0), Qt::DisplayRole),
                 role(*model, 0, FindingsModel::LineRole));
        QVERIFY(!model->data(model->index(0), Qt::DecorationRole).isValid());
        QCOMPARE(model->roleNames().value(FindingsModel::LineRole), "line");
        QCOMPARE(model->roleNames().value(FindingsModel::CutLineRole), "cutLine");
        QCOMPARE(model->roleNames().value(FindingsModel::LineReachRole), "lineReach");
        QCOMPARE(model->roleNames().size(), 11);

        // A change far into its line: the three forms of it differ, and each role is its own.
        EntryView far = group(false);
        far.original = "One two three four five six seven eight nine befor it.";
        far.wording = "One two three four five six seven eight nine before it.";
        far.original_changed = Span{45, 50};
        far.wording_changed = Span{45, 51};
        QVERIFY(model->apply(splice(1, 0, {far})));
        const RowLine line = rowLine(QString::fromStdString(far.original), far.original_changed,
                                     QString::fromStdString(far.wording), far.wording_changed);
        QVERIFY(line.whole != line.cut && line.cut != line.reach && line.whole != line.reach);
        QCOMPARE(role(*model, 1, FindingsModel::LineRole).toString(), line.whole);
        QCOMPARE(role(*model, 1, FindingsModel::CutLineRole).toString(), line.cut);
        QCOMPARE(role(*model, 1, FindingsModel::LineReachRole).toString(), line.reach);
    }

    // A timing finding is placed by its track, which its fix moves whole, by its number too when
    // another track of its file has its name; a row of several says how many places the filter
    // shows when not all; a member says neither; a row whose decided members differ is mixed.
    void aRowIsPlacedAndCountedAsWhatItIs() {
        EntryView timing = row(7, std::nullopt);
        timing.check = Check::kTiming;
        timing.total = 3;
        timing.shown = 2;
        timing.track = ListTrack{4, "Signs", "jpn", true};
        EntryView unnamed = row(8);
        unnamed.track = ListTrack{5, std::nullopt, "ja", false};
        EntryView bare = row(9);
        bare.track = ListTrack{6, std::nullopt, std::nullopt, false};
        EntryView alike = row(12);
        alike.track = ListTrack{7, "Signs", "jpn", false};
        EntryView one = member(10);
        one.total = 2;
        one.mixed = true;
        QVERIFY(model->apply(splice(0, 0, {timing, unnamed, bare, one, alike})));
        QCOMPARE(role(*model, 0, FindingsModel::KindRole).toString(), "Timing");
        QCOMPARE(role(*model, 0, FindingsModel::PlaceRole).toString(), "E01 · Signs");
        QCOMPARE(role(*model, 0, FindingsModel::SeveralRole).toString(), "In 3 places, 2 shown");
        QCOMPARE(role(*model, 0, FindingsModel::StateWordRole).toString(), "Mixed");
        QCOMPARE(role(*model, 1, FindingsModel::PlaceRole).toString(), "E01 · Track 5 (ja)");
        QCOMPARE(role(*model, 2, FindingsModel::PlaceRole).toString(), "E01 · Track 6");
        QCOMPARE(role(*model, 2, FindingsModel::SeveralRole).toString(), "");
        QCOMPARE(role(*model, 3, FindingsModel::SeveralRole).toString(), "");
        QCOMPARE(role(*model, 3, FindingsModel::MemberRole).toBool(), true);
        QCOMPARE(role(*model, 3, FindingsModel::PlaceRole).toString(), "E02 · 0:01");
        QCOMPARE(role(*model, 3, FindingsModel::GlyphRole).toString(), "◐");
        QCOMPARE(role(*model, 3, FindingsModel::StateWordRole).toString(), "To review");
        // Another track of its file has its name: its number tells it apart.
        QCOMPARE(role(*model, 4, FindingsModel::PlaceRole).toString(), "E01 · Track 7: Signs");
    }

    // A timing row's line says the fixes its shown members differ by, each counted in the order
    // review sends them, the same in each form of the line, with nothing marked as changed.
    void aRowWhoseFixesDifferSaysThem() {
        struct Case {
            uint64_t shown;
            std::vector<FixCount> fixes;
            QString said;
            QString several;
        };
        const std::vector<Case> cases = {
            {12,
             {{Fix(Fix::kShift{2}), 8}, {Fix(Fix::kShift{1}), 4}},
             "2 frames late × 8 · 1 frame late × 4",
             "In 12 places"},
            {12,
             {{Fix(Fix::kShift{-3}), 7}, {Fix(Fix::kPartial{}), 4}, {Fix(Fix::kShift{1}), 1}},
             "3 frames early × 7 · partly off × 4 · 1 frame late × 1",
             "In 12 places"},
            // A filter showing some of the members: review counts only those.
            {3,
             {{Fix(Fix::kShift{2}), 2}, {Fix(Fix::kPartial{}), 1}},
             "2 frames late × 2 · partly off × 1",
             "In 12 places, 3 shown"},
        };
        for (const Case &c : cases) {
            EntryView timing = group(false);
            timing.check = Check::kTiming;
            timing.total = 12;
            timing.shown = c.shown;
            timing.fixes = c.fixes;
            QVERIFY(model->apply(splice(0, model->rowCount(), {timing})));
            QCOMPARE(role(*model, 0, FindingsModel::LineRole).toString(), c.said);
            QCOMPARE(role(*model, 0, FindingsModel::CutLineRole).toString(), c.said);
            QCOMPARE(role(*model, 0, FindingsModel::LineReachRole).toString(), "");
            QCOMPARE(role(*model, 0, FindingsModel::SeveralRole).toString(), c.several);
        }
    }

    // One fix, of all the members or of the only ones a filter shows, is the row's own line.
    void aRowOfOneFixKeepsItsLine() {
        for (const uint64_t shown : {uint64_t{12}, uint64_t{5}}) {
            EntryView timing = group(false);
            timing.check = Check::kTiming;
            timing.total = 12;
            timing.shown = shown;
            timing.fixes = {FixCount{Fix(Fix::kShift{2}), shown}};
            QVERIFY(model->apply(splice(0, model->rowCount(), {timing})));
            const RowLine line =
                rowLine(QString::fromStdString(timing.original), timing.original_changed,
                        QString::fromStdString(timing.wording), timing.wording_changed);
            QCOMPARE(role(*model, 0, FindingsModel::LineRole).toString(), line.whole);
            QCOMPARE(role(*model, 0, FindingsModel::CutLineRole).toString(), line.cut);
        }
    }
};

SUBCHECK_OFFSCREEN_TEST_MAIN(FindingsModelTest)
#include "findings_model_test.moc"
