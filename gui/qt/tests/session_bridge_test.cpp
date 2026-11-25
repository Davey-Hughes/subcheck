// The bridge on real sessions over the run seeded for it: two copies of typos.mkv, so two rows of
// two members each, four findings pending. The tests share the run in turn, so each leaves it as
// it found it.
#include "fake_mpv.h"
#include "offscreen_main.h"
#include "session_bridge.h"
#include "told.h"
#include "words.h"

#include <QClipboard>
#include <QElapsedTimer>
#include <QFileInfo>
#include <QGuiApplication>
#include <QSignalSpy>
#include <QTest>

#include <chrono>
#include <functional>
#include <optional>
#include <sstream>
#include <stdexcept>
#include <string>
#include <tuple>
#include <utility>
#include <variant>
#include <vector>

using namespace subcheck_ffi;

namespace {

// The seeded run's folder: the session's selector.
std::string media() { return qEnvironmentVariable("SUBCHECK_TEST_MEDIA").toStdString(); }

// The folder of the run with no findings.
std::string clean() { return qEnvironmentVariable("SUBCHECK_TEST_CLEAN").toStdString(); }

const QString counts = "4 to review · 0 accepted · 0 rejected";

// The window's moments for the arc over the video, as the bridge's flag has them: shown once a wait
// has lasted the first, gone once it has been over for the second. What a busy machine may add to
// either before a test sees the change. A wait past the hide moment, in which the arc is to stay,
// short of the player's limit for an answer from mpv, past which an Open unanswered fails. When a
// test's check of the file answers: halfway through the first moment, so that the check not yet
// having answered when the arc shows and the arc's showing timed from the press (not from the
// answer) each get the same room.
constexpr int arcShows = 500;
constexpr int arcGoes = 100;
constexpr int slack = 1000;
constexpr int pastHiding = 300;
constexpr int checkAnswers = arcShows / 2;

// Where the row at `row` is, and whether a row of several, as its first line says them.
QString place(SessionBridge &bridge, int row) {
    FindingsModel *list = bridge.findings();
    if (row >= list->rowCount()) {
        return {};
    }
    const QString several = list->data(list->index(row), FindingsModel::SeveralRole).toString();
    const QString where = list->data(list->index(row), FindingsModel::PlaceRole).toString();
    return several.isEmpty() ? where : several + " " + where;
}

QString decision(SessionBridge &bridge, int row) {
    FindingsModel *list = bridge.findings();
    return list->data(list->index(row), FindingsModel::StateWordRole).toString();
}

// Which episodes the sidebar marks as the selected finding's.
QStringList current(SessionBridge &bridge) {
    EpisodeOrder *files = bridge.episodes();
    QStringList marked;
    for (int row = 0; row < files->rowCount(); ++row) {
        const QModelIndex at = files->index(row, 0);
        if (files->data(at, EpisodesModel::CurrentRole).toBool()) {
            marked << files->data(at, EpisodesModel::LabelRole).toString();
        }
    }
    return marked;
}

// A bridge on `selector`, its end's text in `out` and `err`, the actions it sent in `bridge.sent`,
// and its player, which has no mpv until a test gives it one. A short wait after a quit, so that
// the give-up's test does not wait long: the store is the test's own, so no lock held elsewhere
// slows review's end in the others.
struct Opened {
    std::ostringstream out;
    std::ostringstream err;
    PlayerPort player{std::chrono::milliseconds(100)};
    Telling bridge;

    explicit Opened(std::vector<std::string> selector)
        : bridge(std::move(selector), player, out, err, std::chrono::seconds(1)) {}
};

// A bridge whose session's close throws, as a Rust panic in close would, before closing it: the
// test closes it later, so that the pump, let go of, reads an end only once the bridge has let go.
class ThrowingClose : public SessionBridge {
public:
    using SessionBridge::SessionBridge;

    // The session the throwing close was given, which the pump still holds.
    Session *unclosed = nullptr;

protected:
    void closeSession(Session &session) override {
        unclosed = &session;
        throw std::runtime_error("close panicked");
    }
};

// A bridge on the run with no findings and the seeded one, whose review a test stands in for once
// it has gone quiet: each change stood in for is review's last with the count, the file and the
// selection the test gives it.
struct StandIn {
    // Review's first change, and its last before it was held, with the count of any answer alone
    // after it. Declared before the bridge, which writes them, so that they outlive it.
    std::optional<Event::kChanged> first;
    std::optional<Event::kChanged> last;
    std::ostringstream out;
    std::ostringstream err;
    PlayerPort player{std::chrono::milliseconds(100)};
    Holding bridge;

    StandIn() : bridge({clean(), media()}, player, out, err, std::chrono::seconds(1)) {
        QObject::connect(&bridge, &SessionBridge::eventArrived, &bridge, [this](const Event &e) {
            if (bridge.holding) {
                return;
            }
            if (const auto *answer = std::get_if<Event::kAnswered>(&e.get_variant());
                answer && last) {
                last->summary.answered = answer->answered;
            }
            const auto *change = std::get_if<Event::kChanged>(&e.get_variant());
            if (change) {
                if (!first) {
                    first = *change;
                }
                last = *change;
            }
        });
    }

    // Holds review from now on, once it has gone quiet, with no event of its for a while, so that
    // none lands among the changes stood in for.
    void holdOnceQuiet() {
        QSignalSpy arrived(&bridge, &SessionBridge::eventArrived);
        for (qsizetype before = -1; before != arrived.count();) {
            before = arrived.count();
            QTest::qWait(300);
        }
        bridge.holding = true;
    }

    // Review's change that answers the action counted `answered`, with the file and the entry
    // selected at those rows of the lists the bridge holds, and with `files`, `logs` and `detail`.
    void answer(uint64_t answered, uint64_t file, uint64_t selected,
                std::optional<FilesSplice> files = std::nullopt,
                std::optional<LogsNow> logs = std::nullopt,
                std::optional<DetailNow> detail = std::nullopt) {
        Summary summary = last->summary;
        summary.answered = answered;
        summary.file_selected = file;
        summary.selected = selected;
        emit bridge.eventArrived(
            Event(Event::kChanged{std::nullopt, summary, std::move(detail), std::move(files),
                                  std::move(logs), std::nullopt}));
    }

    // Review's answer alone to the action counted `answered`, as it says one that changed nothing
    // the window shows.
    void answerAlone(uint64_t answered) {
        emit bridge.eventArrived(Event(Event::kAnswered{answered}));
    }

    // Review's change as it last sent it, but for whether Show in Video waits for its file's check,
    // and whether the player's Open follows the change.
    void stamping(bool waiting, bool openFollows = false) {
        Summary summary = last->summary;
        summary.stamping = waiting;
        summary.open_follows = openFollows;
        emit bridge.eventArrived(Event(Event::kChanged{std::nullopt, summary, std::nullopt,
                                                       std::nullopt, std::nullopt, std::nullopt}));
    }

    // Review's Open of a made-up moment, which the player runs on the test's mpv.
    void open() {
        emit bridge.eventArrived(Event(Event::kPlayer{PlayerCommand(
            PlayerCommand::kOpen{Moment{"/m/a.mkv", 0, std::nullopt}, How::kLoad, {}, false})}));
    }
};

} // namespace

class SessionBridgeTest : public QObject {
    Q_OBJECT

private slots:
    void initTestCase() {
        // A test decides findings in whatever store it opens: without CTest's environment, that
        // would be the user's own.
        QVERIFY2(qEnvironmentVariableIsSet("SUBCHECK_DB") &&
                     qEnvironmentVariableIsSet("SUBCHECK_TEST_MEDIA"),
                 "run through ctest, which seeds a store of the test's own");
    }

    void theFirstEventFillsTheList() {
        Opened o({media()});
        QCOMPARE(o.bridge.statusLine(), "Opening…");
        QCOMPARE(o.bridge.title(), "subcheck");
        QCOMPARE(o.bridge.counts(), "");
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QCOMPARE(o.bridge.selected(), 0);
        QCOMPARE(place(o.bridge, 0), "In 2 places E01 · 0:01");
        QCOMPARE(o.bridge.statusLine(), "");
        QCOMPARE(o.bridge.counts(), counts);
        QCOMPARE(o.bridge.heading(), "All Findings · 4");
        QCOMPARE(o.bridge.title(), "media");
        QCOMPARE(o.bridge.emptyWords(), "");
    }

    // The sidebar's three lists and the details panel come with the first change: the store's
    // reviews, newest first, the one shown open; the filters' counts, all shown; the view's
    // episodes, the selected finding's marked; and that finding's details.
    void theFirstEventFillsTheSidebarAndTheDetails() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        FoldersModel *reviews = o.bridge.folders();
        QCOMPARE(reviews->rowCount(), 2);
        QCOMPARE(reviews->data(reviews->index(0), FoldersModel::NameRole).toString(), "clean");
        QCOMPARE(reviews->data(reviews->index(1), FoldersModel::NameRole).toString(), "media");
        QCOMPARE(reviews->data(reviews->index(1), FoldersModel::CountRole).toInt(), 4);
        QCOMPARE(reviews->data(reviews->index(1), FoldersModel::OpenRole).toBool(), true);
        QCOMPARE(reviews->data(reviews->index(0), FoldersModel::OpenRole).toBool(), false);
        FiltersModel *filters = o.bridge.filters();
        QCOMPARE(filters->data(filters->index(1), FiltersModel::CountRole).toInt(), 4);
        QCOMPARE(filters->data(filters->index(0), FiltersModel::CurrentRole).toBool(), true);
        QCOMPARE(o.bridge.episodes()->rowCount(), 2);
        QCOMPARE(current(o.bridge), QStringList{"E01"});
        QVERIFY(o.bridge.detail()->shown());
        QCOMPARE(o.bridge.detail()->place(), "E01 · 0:01");
        QCOMPARE(o.bridge.detail()->episodes(), "In 2 places: E01, E02");
        // A folder's view is of paths: the View menu's item opens the newest check.
        QCOMPARE(o.bridge.switchLabel(), "Show Newest Check");
        QCOMPARE(o.bridge.sent, QStringList());
    }

    // A move at either end selects the row it is on, which lets a message go. A click sends a
    // Select of its row's key, an EntryKey made in C++ for Rust.
    void aMoveStopsAtEitherEndAndAClickSelectsItsRow() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        o.bridge.undo();
        QTRY_COMPARE(o.bridge.statusLine(), "Nothing to undo");
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.statusLine(), "");
        QCOMPARE(o.bridge.selected(), 1);
        o.bridge.move(-1);
        QTRY_COMPARE(o.bridge.selected(), 0);
        o.bridge.undo();
        QTRY_COMPARE(o.bridge.statusLine(), "Nothing to undo");
        o.bridge.move(-1);
        QTRY_COMPARE(o.bridge.statusLine(), "");
        QCOMPARE(o.bridge.selected(), 0);
        // A click: on a row, it selects it; outside the list there is no key to send, so a message
        // stays until a move lets it go.
        o.bridge.select(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        o.bridge.undo();
        QTRY_COMPARE(o.bridge.statusLine(), "Nothing to undo");
        o.bridge.select(2);
        o.bridge.select(-1);
        QTest::qWait(100);
        QCOMPARE(o.bridge.statusLine(), "Nothing to undo");
        o.bridge.move(0);
        QTRY_COMPARE(o.bridge.statusLine(), "");
        QCOMPARE(o.bridge.selected(), 1);
    }

    // Moves sent before review has answered the one before: each moves on from where review's
    // selection is by then, as the terminal's do, not from the summary the bridge last applied.
    void movesSentFasterThanReviewAnswersAddUp() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.toggle();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 4);
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        o.bridge.move(1);
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 3);
    }

    // An accept moves on to the next pending entry; a move sent before review has answered it
    // starts from there, so that the next action acts where the terminal's would. The selection
    // is read once the move's answer is in, which lets the accept's message go; and the accept is
    // undone before it is checked, so that the run is left as found.
    void aMoveSentBeforeAnAcceptIsAnsweredStartsWhereItMovedOn() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        o.bridge.toggle();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 4);
        o.bridge.move(-1);
        QTRY_COMPARE(o.bridge.selected(), 0);
        o.bridge.accept();
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.counts(), "2 to review · 2 accepted · 0 rejected");
        QTRY_COMPARE(o.bridge.statusLine(), "");
        const int landed = o.bridge.selected();
        o.bridge.undo();
        QTRY_COMPARE(decision(o.bridge, 0), "To review");
        QCOMPARE(landed, 2);
    }

    void aToggleExpandsARowAndCollapsesIt() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.toggle();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 4);
        QCOMPARE(place(o.bridge, 1), "E01 · 0:01");
        QCOMPARE(place(o.bridge, 2), "E02 · 0:01");
        o.bridge.move(2);
        QTRY_COMPARE(o.bridge.selected(), 2);
        // From a member, it collapses its row, which it selects.
        o.bridge.toggle();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QCOMPARE(o.bridge.selected(), 0);
        QCOMPARE(place(o.bridge, 0), "In 2 places E01 · 0:01");
    }

    // What Finding ▸ Show Each Episode says of the selected entry: whether it opens and closes, a
    // row of several or one of its members, and whether it is open, which a member always is. A
    // row's triangle selects its row, then toggles it; outside the list there is no row to toggle.
    void theSelectedRowSaysWhetherItOpensAndIsOpen() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QVERIFY(o.bridge.canExpand());
        QVERIFY(!o.bridge.expanded());
        o.bridge.toggle();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 4);
        QVERIFY(o.bridge.canExpand());
        QVERIFY(o.bridge.expanded());
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        QCOMPARE(place(o.bridge, 1), "E01 · 0:01");
        QVERIFY(o.bridge.canExpand());
        QVERIFY(o.bridge.expanded());
        o.bridge.toggle();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QCOMPARE(o.bridge.selected(), 0);
        QVERIFY(o.bridge.canExpand());
        QVERIFY(!o.bridge.expanded());

        qsizetype from = o.bridge.sent.size();
        o.bridge.toggleRow(1);
        const QStringList sent = o.bridge.sent.mid(from);
        QVERIFY2(sent.size() == 2 && sent[0].startsWith("Select row ") && sent[1] == "Toggle",
                 qPrintable(sent.join(" | ")));
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 4);
        QCOMPARE(o.bridge.selected(), 1);
        QVERIFY(o.bridge.expanded());
        o.bridge.toggleRow(1);
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QVERIFY(!o.bridge.expanded());
        from = o.bridge.sent.size();
        o.bridge.toggleRow(-1);
        o.bridge.toggleRow(2);
        QCOMPARE(o.bridge.sent.mid(from), QStringList());
    }

    // An edit is review's: whether one is open and the wording it opened with are what review's
    // last change says, and it opens once, at the change that first has it, so that the window sets
    // the field's text once per edit. A change while it is open opens nothing: review's message, or
    // the same edit with its entry renamed, as a re-check renames it. Moving off the edit ends it,
    // as review ends it then. The decision the last edit makes is undone, so that the run is left
    // as found.
    void theEditAndItsWordingComeFromReviewAndOpenOncePerEdit() {
        Opened o({media()});
        std::optional<Event::kChanged> last;
        QObject::connect(
            &o.bridge, &SessionBridge::eventArrived, &o.bridge, [&last](const Event &e) {
                if (const auto *change = std::get_if<Event::kChanged>(&e.get_variant())) {
                    last = *change;
                }
            });
        QSignalSpy opened(&o.bridge, &SessionBridge::editOpened);
        const auto wordingAt = [&o](int row) {
            const EntryView *entry = o.bridge.findings()->at(row);
            return entry ? QString::fromStdString(entry->wording) : QStringLiteral("(no row)");
        };
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QVERIFY(!o.bridge.editing());
        QCOMPARE(o.bridge.editWording(), "");
        QVERIFY(o.bridge.canEdit());

        o.bridge.edit();
        QTRY_VERIFY(o.bridge.editing());
        QVERIFY(!o.bridge.editWording().isEmpty());
        QCOMPARE(o.bridge.editWording(), wordingAt(0));
        QCOMPARE(opened.count(), 1);
        o.bridge.undo();
        QTRY_COMPARE(o.bridge.statusLine(), "Nothing to undo");
        QVERIFY(o.bridge.editing());
        QCOMPARE(opened.count(), 1);
        o.bridge.cancelEdit();
        QTRY_VERIFY(!o.bridge.editing());
        QCOMPARE(o.bridge.editWording(), "");

        o.bridge.edit();
        QTRY_COMPARE(opened.count(), 2);
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        QVERIFY(!o.bridge.editing());
        o.bridge.edit();
        QTRY_COMPARE(opened.count(), 3);
        QVERIFY(o.bridge.editing());
        QCOMPARE(o.bridge.editWording(), wordingAt(1));

        // Review's last change again, its edit's entry under other IDs, the same wording.
        QVERIFY(last && last->summary.edit);
        Summary renamed = last->summary;
        renamed.edit->entry = EntryKey(EntryKey::kRow{{9001, 9002}});
        QSignalSpy changed(&o.bridge, &SessionBridge::summaryChanged);
        emit o.bridge.eventArrived(Event(Event::kChanged{
            std::nullopt, renamed, std::nullopt, std::nullopt, std::nullopt, std::nullopt}));
        QTRY_VERIFY(changed.count() > 0);
        QVERIFY(o.bridge.editing());
        QCOMPARE(o.bridge.editWording(), wordingAt(1));
        QCOMPARE(opened.count(), 3);

        o.bridge.reword("Fixed.");
        QTRY_VERIFY(!o.bridge.editing());
        QTRY_COMPARE(decision(o.bridge, 1), "Accepted");
        QCOMPARE(opened.count(), 3);
        o.bridge.undo();
        QTRY_COMPARE(decision(o.bridge, 1), "To review");
    }

    // Review answers an Edit at the first change or answer alone whose count reaches it, after the
    // edit that change opens, and whether or not it opens one: under a question, which the Edit
    // only lets go, none opens. A job's change before then answers nothing. Review is held back,
    // and its changes stood in for, once it asks.
    void anEditIsAnsweredAtTheCountThatReachesIt() {
        StandIn s;
        QStringList heard;
        QObject::connect(&s.bridge, &SessionBridge::editOpened, &s.bridge,
                         [&heard] { heard << "opened"; });
        QObject::connect(&s.bridge, &SessionBridge::editAnswered, &s.bridge,
                         [&heard] { heard << "answered"; });
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.bridge.edit();
        QTRY_COMPARE(heard, (QStringList{"opened", "answered"}));
        s.bridge.cancelEdit();
        QTRY_VERIFY(!s.bridge.editing());
        // Recheck asks. The window has no way to ask it yet, so the test sends it.
        s.bridge.send(Action(Action::kRecheck{}));
        QTRY_VERIFY(s.last && s.last->summary.question);
        s.holdOnceQuiet();
        QCOMPARE(heard, (QStringList{"opened", "answered"}));
        heard.clear();
        QSignalSpy changed(&s.bridge, &SessionBridge::summaryChanged);
        const auto count = static_cast<uint64_t>(s.bridge.sent.size());
        s.bridge.edit();
        // A job's change: no action since the question.
        s.answer(count, 0, 0);
        QTRY_COMPARE(changed.count(), 1);
        QCOMPARE(heard, QStringList());
        // The Edit's answer: review only let the question go.
        s.last->summary.question.reset();
        s.answer(count + 1, 0, 0);
        QTRY_COMPARE(heard, QStringList{"answered"});
        QVERIFY(!s.bridge.editing());
        // An Edit answered alone, as review answers an action that changes nothing it shows.
        s.bridge.edit();
        s.answerAlone(count + 2);
        QTRY_COMPARE(heard, (QStringList{"answered", "answered"}));
        QVERIFY(!s.bridge.editing());
    }

    // Copy Subtitle Line puts the selected row's line on the clipboard, as its file has it, and
    // asks review nothing. A timing fix's row, a track's timing rather than a line, has no line to
    // copy, nor has a row outside the list: the clipboard keeps what it held.
    void copyLinePutsTheRowsLineOnTheClipboard() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QClipboard *clipboard = QGuiApplication::clipboard();
        clipboard->clear();
        const int at = o.bridge.selected();
        QVERIFY(o.bridge.canCopy());
        QVERIFY(o.bridge.canCopyAt(at));
        const QString line = text(o.bridge.findings()->at(at)->original);
        QVERIFY(!line.isEmpty());
        o.bridge.copyLine(at);
        QCOMPARE(clipboard->text(), line);

        // A timing row made up in the selected row's place, with words of its own, drawn as
        // review's next change would.
        EntryView timing = *o.bridge.findings()->at(at);
        timing.check = Check::kTiming;
        timing.original = "a timing row's own words";
        QVERIFY(o.bridge.findings()->apply(Splice{static_cast<uint64_t>(at), 1, {timing}}));
        emit o.bridge.summaryChanged();
        QVERIFY(!o.bridge.canCopy());
        QVERIFY(!o.bridge.canCopyAt(at));
        o.bridge.copyLine(at);
        QCOMPARE(clipboard->text(), line);
        QVERIFY(!o.bridge.canCopyAt(-1));
        QVERIFY(!o.bridge.canCopyAt(o.bridge.findings()->rowCount()));
        o.bridge.copyLine(-1);
        o.bridge.copyLine(o.bridge.findings()->rowCount());
        QCOMPARE(clipboard->text(), line);
        QCOMPARE(o.bridge.sent, QStringList());
    }

    // Copy Subtitle Line and Show File in Folder chosen from the right-click menu of another row,
    // before review has answered the click's Select: each takes the row clicked, not the one review
    // still has selected. The second row is opened, so that its member in the second copy is a
    // row whose line and file are both unlike the first row's.
    void copyAndShowInFolderReadTheRowGivenNotTheSelection() {
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.bridge.toggleRow(1);
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 4);
        s.bridge.select(0);
        QTRY_COMPARE(s.bridge.selected(), 0);
        s.holdOnceQuiet();
        const EntryView *selected = s.bridge.findings()->at(0);
        const EntryView *clicked = s.bridge.findings()->at(3);
        QVERIFY(selected && clicked);
        QVERIFY(clicked->original != selected->original);
        QVERIFY(clicked->file != selected->file);

        s.bridge.select(3);
        QCOMPARE(s.bridge.selected(), 0);
        s.bridge.copyLine(3);
        QCOMPARE(QGuiApplication::clipboard()->text(), text(clicked->original));
        s.bridge.showInFolder(3);
        QCOMPARE(s.bridge.revealed, QStringList{text(clicked->file)});
    }

    // Show File in Folder shows the file of the row it is given, as review names it, and asks
    // review nothing; outside the list there is no file, and nothing is shown.
    void showInFolderRevealsTheRowsOwnFile() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        const QString file = text(o.bridge.findings()->at(1)->file);
        QVERIFY2(QFileInfo::exists(file), qPrintable(file));
        o.bridge.showInFolder(1);
        QCOMPARE(o.bridge.revealed, QStringList{file});
        o.bridge.showInFolder(-1);
        o.bridge.showInFolder(o.bridge.findings()->rowCount());
        QCOMPARE(o.bridge.revealed, QStringList{file});
        QCOMPARE(o.bridge.sent, QStringList());
    }

    // Find selects the row after the selected one that holds the words, as a click selects it, or
    // the one before it going back; words no row holds select nothing, and say so. The seeded run's
    // two rows are both of the first episode, so each holds the other's label.
    void findNextSelectsTheMatchAndSaysWhenThereIsNone() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QCOMPARE(o.bridge.selected(), 0);
        const QString label = text(o.bridge.findings()->at(1)->label);
        const QString second = told(Action(Action::kSelect{o.bridge.findings()->at(1)->key}));
        QVERIFY(o.bridge.findNext(label, false));
        QCOMPARE(o.bridge.sent, QStringList{second});
        QTRY_COMPARE(o.bridge.selected(), 1);
        const QString first = told(Action(Action::kSelect{o.bridge.findings()->at(0)->key}));
        QVERIFY(o.bridge.findNext(label, true));
        QCOMPARE(o.bridge.sent, (QStringList{second, first}));
        QTRY_COMPARE(o.bridge.selected(), 0);
        QVERIFY(!o.bridge.findNext("no such words", false));
        QVERIFY(!o.bridge.findNext("no such words", true));
        QCOMPARE(o.bridge.sent, (QStringList{second, first}));
    }

    // Finds sent faster than review answers them go on each from the row the one before it found,
    // not from review's selection, which is still the row before them all. Any other action lets
    // that row go, as where review's selection then lands is review's to say: a find after a click
    // goes on from review's selection, here the row clicked, which review still has selected; one
    // after a move, from where review's answer to the move lands; and one after review's answer to
    // the last find's Select, from where that answer left review's selection, as a question would,
    // which a Select only lets go. The first row is opened, so that its line is in three rows of
    // four, the row and its members; the finds start from the last row, which lacks it.
    void findsSentFasterThanReviewAnswersGoOnFromTheLastMatch() {
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.bridge.toggleRow(0);
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 4);
        s.bridge.select(3);
        QTRY_COMPARE(s.bridge.selected(), 3);
        s.holdOnceQuiet();
        const qsizetype from = s.bridge.sent.size();
        const auto count = static_cast<uint64_t>(from);
        QVERIFY(s.last && s.last->summary.answered == count && s.last->summary.file_selected);
        FindingsModel *list = s.bridge.findings();
        const QString words = "befor";
        QCOMPARE(list->find(words, 3, false), 0);
        QCOMPARE(list->find(words, 0, false), 1);
        QCOMPARE(list->find(words, 1, false), 2);
        QCOMPARE(list->find(words, 2, false), 0);
        QStringList selects;
        for (int row = 0; row < list->rowCount(); ++row) {
            selects << told(Action(Action::kSelect{list->at(row)->key}));
        }

        QVERIFY(s.bridge.findNext(words, false));
        QVERIFY(s.bridge.findNext(words, false));
        QCOMPARE(s.bridge.sent.mid(from), (QStringList{selects[0], selects[1]}));
        QCOMPARE(s.bridge.selected(), 3);
        s.bridge.select(3);
        QVERIFY(s.bridge.findNext(words, false));
        QCOMPARE(s.bridge.sent.mid(from),
                 (QStringList{selects[0], selects[1], selects[3], selects[0]}));

        s.bridge.move(1);
        const uint64_t file = *s.last->summary.file_selected;
        const std::vector<uint64_t> lands = {0, 1, 3, 0, 1};
        for (size_t i = 0; i < lands.size(); ++i) {
            s.answer(count + 1 + i, file, lands[i]);
        }
        QTRY_COMPARE(s.bridge.selected(), 1);
        QVERIFY(s.bridge.findNext(words, false));
        QCOMPARE(s.bridge.sent.mid(from), (QStringList{selects[0], selects[1], selects[3],
                                                       selects[0], "Move 1", selects[2]}));

        QSignalSpy changed(&s.bridge, &SessionBridge::summaryChanged);
        s.answer(count + 6, file, 1);
        QTRY_COMPARE(changed.count(), 1);
        QVERIFY(s.bridge.findNext(words, false));
        QCOMPARE(s.bridge.sent.size(), from + 7);
        QCOMPARE(s.bridge.sent.last(), selects[2]);
    }

    // What a dry run says of a finding shows in its details. The accept is undone before the words
    // are checked, so that the run is left as found; the dry run has ended by then, so that the
    // bridge's close ends no job's groups, which would end them for every test after this.
    void aDryRunsMarkShowsInTheDetails() {
        Opened o({media()});
        std::optional<Job> job;
        QObject::connect(
            &o.bridge, &SessionBridge::eventArrived, &o.bridge, [&job](const Event &e) {
                if (const auto *change = std::get_if<Event::kChanged>(&e.get_variant())) {
                    job = change->summary.job;
                }
            });
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.toggle();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 4);
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        o.bridge.accept();
        QTRY_COMPARE(decision(o.bridge, 1), "Accepted");
        o.bridge.send(Action(Action::kApply{true}));
        QTRY_VERIFY(job && std::holds_alternative<Job::kApplyDone>(job->get_variant()));
        // The accept moved on: back to the finding it decided.
        o.bridge.move(-1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        const QString mark = o.bridge.detail()->mark();
        o.bridge.undo();
        QTRY_COMPARE(decision(o.bridge, 1), "To review");
        QCOMPARE(mark, "The preview would apply it");
    }

    void aDecisionShowsAndItsUndoPutsItBack() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.accept();
        QTRY_COMPARE(decision(o.bridge, 0), "Accepted");
        QCOMPARE(o.bridge.statusLine(), "Accepted 2 findings");
        QCOMPARE(o.bridge.selected(), 1);
        o.bridge.reject();
        QTRY_COMPARE(decision(o.bridge, 1), "Rejected");
        o.bridge.undo();
        QTRY_COMPARE(decision(o.bridge, 1), "To review");
        o.bridge.undo();
        QTRY_COMPARE(decision(o.bridge, 0), "To review");
        // Any action lets a message go, and the counts show again.
        o.bridge.move(0);
        QTRY_COMPARE(o.bridge.statusLine(), "");
    }

    void quittingPrintsTheCountsAndEndsWithZero() {
        Opened o({media()});
        QSignalSpy finished(&o.bridge, &SessionBridge::finished);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.quit();
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 0);
        QCOMPARE(o.out.str(), "media: 0 accepted, 0 rejected, 4 pending\n");
        QCOMPARE(o.err.str(), "");
        // Once let go, what the pump read is dropped, and an action goes nowhere.
        emit o.bridge.eventArrived(
            Event(Event::kChanged{Splice{0, 2, {}}, Summary{}, {}, {}, {}, {}}));
        o.bridge.accept();
        QCoreApplication::sendPostedEvents();
        QCOMPARE(o.bridge.findings()->rowCount(), 2);
        // The end stopped the give-up's timer: past its second, nothing more is said.
        QTest::qWait(1200);
        QCOMPARE(finished.count(), 1);
        QCOMPARE(o.err.str(), "");
    }

    // A run with no findings opens to an empty list, which the window says, and nothing to move to.
    void anEmptyRunSaysSo() {
        Opened o({clean()});
        QSignalSpy finished(&o.bridge, &SessionBridge::finished);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.emptyWords(), "No findings in this run");
        QCOMPARE(o.bridge.findings()->rowCount(), 0);
        QCOMPARE(o.bridge.selected(), -1);
        QCOMPARE(o.bridge.counts(), "0 to review · 0 accepted · 0 rejected");
        QCOMPARE(o.bridge.title(), "clean");
        o.bridge.move(1);
        o.bridge.quit();
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 0);
        QCOMPARE(o.out.str(), "clean: 0 accepted, 0 rejected, 0 pending\n");
    }

    // A filter that hides every finding says so, as the run still has them, and the way back to a
    // filter that shows them lets the words go.
    void aFilterThatHidesEveryFindingIsNotAnEmptyRun() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QCOMPARE(o.bridge.emptyWords(), "");
        o.bridge.setFilter(3);
        QTRY_COMPARE(o.bridge.heading(), "Rejected · 0");
        QCOMPARE(o.bridge.findings()->rowCount(), 0);
        QCOMPARE(o.bridge.emptyWords(), "No findings under this filter");
        o.bridge.setFilter(0);
        QTRY_COMPARE(o.bridge.heading(), "All Findings · 4");
        QCOMPARE(o.bridge.findings()->rowCount(), 2);
        QCOMPARE(o.bridge.emptyWords(), "");
    }

    // Made up, to show where each part goes: an end that stops a job, whose outro says how to
    // finish it, ends the process's groups, so session_bridge_job_test has its own.
    void anEndsOutroGoesToStderrAndTheCountsToStdout() {
        Opened o({media()});
        QSignalSpy finished(&o.bridge, &SessionBridge::finished);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        emit o.bridge.eventArrived(Event(Event::kEnded{std::nullopt, {"one", "two"}}));
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 0);
        QCOMPARE(o.err.str(), "one\ntwo\n");
        QCOMPARE(o.out.str(), "media: 0 accepted, 0 rejected, 4 pending\n");
    }

    // No selector, as the app given no arguments has: review opens the newest run's folders, the
    // run with no findings, which the seed made last.
    void noSelectorOpensTheNewestRunsFolders() {
        Opened o({});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.title(), "clean");
        QCOMPARE(o.bridge.emptyWords(), "No findings in this run");
    }

    void aRunThatIsNotThereIsAFatalEnd() {
        Opened o({"no-such-run"});
        QSignalSpy finished(&o.bridge, &SessionBridge::finished);
        QSignalSpy fatal(&o.bridge, &SessionBridge::fatalChanged);
        o.bridge.start();
        QTRY_COMPARE(fatal.count(), 1);
        QCOMPARE(o.bridge.fatal(), "no run named no-such-run");
        QCOMPARE(o.err.str(), "subcheck-qt: no run named no-such-run\n");
        QCOMPARE(o.out.str(), "");
        // The dialog's to close: the window stays until it is.
        QCOMPARE(finished.count(), 0);
        o.bridge.quit();
        QCOMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 1);
        o.bridge.dismiss();
        QCOMPARE(finished.count(), 1);
    }

    // An error that gives no reason still has words for its dialog, which is the way out: closing
    // the window closes it, and ends with 1. The error is made up, as no real one is empty.
    void aFatalEndWithNoReasonStillShowsItsDialog() {
        Opened o({media()});
        QSignalSpy finished(&o.bridge, &SessionBridge::finished);
        QSignalSpy fatal(&o.bridge, &SessionBridge::fatalChanged);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        emit o.bridge.eventArrived(Event(Event::kEnded{std::string(), {}}));
        QTRY_COMPARE(fatal.count(), 1);
        QVERIFY(!o.bridge.fatal().isEmpty());
        QCOMPARE(o.err.str(), "subcheck-qt: " + o.bridge.fatal().toStdString() + "\n");
        QCOMPARE(finished.count(), 0);
        o.bridge.quit();
        QCOMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 1);
    }

    // The session's events kept from the bridge, so that review's end never reaches it, as it
    // would not from a stalled core thread: the bridge waits the second it was given, not much
    // more, then closes the session and ends the window anyway.
    void anEndThatDoesNotComeWithinASecondIsGivenUp() {
        Opened o({media()});
        QSignalSpy finished(&o.bridge, &SessionBridge::finished);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QVERIFY(disconnect(&o.bridge, &SessionBridge::eventArrived, &o.bridge, nullptr));
        QElapsedTimer waited;
        qint64 endedAt = -1;
        connect(&o.bridge, &SessionBridge::finished, this, [&] { endedAt = waited.elapsed(); });
        QSignalSpy shown(&o.bridge, &SessionBridge::summaryChanged);
        waited.start();
        o.bridge.quit();
        // The window says why it stays while it waits.
        QCOMPARE(shown.count(), 1);
        QCOMPARE(o.bridge.statusLine(), "Quitting: waiting for review to end");
        // A second quit, while the first waits, does not put the give-up off.
        QTest::qWait(600);
        o.bridge.quit();
        QTRY_COMPARE_WITH_TIMEOUT(finished.count(), 1, 3000);
        QVERIFY2(endedAt >= 1000 && endedAt < 1500, qPrintable(QString::number(endedAt)));
        QCOMPARE(finished[0][0].toInt(), 1);
        QCOMPARE(o.err.str(),
                 "subcheck-qt: review had not ended 1 s after quitting, so it was closed\n");
        QCOMPARE(o.out.str(), "");
    }

    // Show in Video opens the selected line's moment through review and the player to mpv, on its
    // first frame (core's target, 4.5 ms past the line's start at 1 s), with its subtitle track,
    // loaded paused, as mpv has no file open; each of the video's own asks mpv its command; Close
    // Video closes. What the player reports is review's status message.
    void theVideosAsksReachMpvAndItsReportsTheStatusLine() {
        // Before the bridge, which stops it as it goes.
        FakeMpv mpv;
        Opened o({media()});
        o.player.attach(&mpv);
        o.player.ready();
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.play();
        QTRY_COMPARE(mpv.last(), "get path");
        mpv.answer(o.player, {}, "property unavailable");
        QCOMPARE(mpv.last(), "set pause yes");
        mpv.answer(o.player);
        QCOMPARE(mpv.last(), QString("loadfile %1/typos - E01.mkv replace -1 "
                                     "start=0:00:01.004500,sid=1,sub-visibility=yes,hr-seek=yes,"
                                     "rebase-start-time=no,pause=yes")
                                 .arg(QString::fromStdString(media())));
        mpv.answer(o.player);
        o.player.restarted();
        QTRY_COMPARE(o.bridge.statusLine(), "Showing it, paused");

        const std::vector<std::pair<std::function<void()>, QString>> asks = {
            {[&] { o.bridge.pause(); }, "cycle pause"},
            {[&] { o.bridge.seek(-5); }, "seek -5 relative+exact"},
            {[&] { o.bridge.seek(5); }, "seek 5 relative+exact"},
            {[&] { o.bridge.seek(-1); }, "seek -1 relative+exact"},
            {[&] { o.bridge.seek(1); }, "seek 1 relative+exact"},
            {[&] { o.bridge.previousFrame(); }, "frame-back-step"},
            {[&] { o.bridge.nextFrame(); }, "frame-step"},
        };
        for (const auto &[ask, command] : asks) {
            const std::size_t before = mpv.requests.size();
            ask();
            QTRY_VERIFY(mpv.requests.size() > before);
            QCOMPARE(mpv.last(), "get time-pos");
            mpv.answer(o.player, 2.0);
            QCOMPARE(mpv.last(), command);
            mpv.answer(o.player);
            // Both are said, as one or the other lands each of these moves (a frame step, which
            // starts no playback again, on the position alone) and this test does not care which;
            // what lands the move ends its wait, and a pause is read at once.
            o.player.restarted();
            o.player.moved();
            QCOMPARE(mpv.last(), "get pause");
            mpv.answer(o.player, true);
            mpv.answer(o.player, 2.5);
            QTRY_COMPARE(o.bridge.statusLine(), "Paused at 0:00:02.500");
            // Any action lets a message go.
            o.bridge.move(0);
            QTRY_COMPARE(o.bridge.statusLine(), "");
        }

        o.bridge.closePlayer();
        QTRY_COMPARE(mpv.last(), "get path");
        mpv.answer(o.player, QString::fromStdString(media()) + "/typos - E01.mkv");
        QCOMPARE(mpv.last(), "stop");
        mpv.answer(o.player);
        QTRY_COMPARE(o.bridge.statusLine(), "Video closed");
    }

    // Review's end stops the video, as the terminal's quit closes its mpv; what the player says
    // after goes nowhere.
    void theEndStopsThePlayer() {
        // Before the bridge, which stops it as it goes.
        FakeMpv mpv;
        Opened o({media()});
        o.player.attach(&mpv);
        o.player.ready();
        QSignalSpy finished(&o.bridge, &SessionBridge::finished);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.quit();
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(mpv.last(), "stop");
        emit o.player.reported(PlayerReport(PlayerReport::kClosed{std::nullopt}));
        QCOMPARE(o.err.str(), "");
    }

    // While review waits for Show in Video's check of its file, before the player has heard of an
    // Open, the video shows that it loads: once the wait has lasted the window's moment, and not
    // before; and it stops a moment after review's change that ends the wait, with no Open under
    // way. The clock starts before each change, so that the time it gives is never short of the
    // flag's own. Review is held back, and its changes stood in for, once it has gone quiet.
    void theArcShowsWhileReviewWaitsForTheFileCheck() {
        // Declared before the stand-in, whose bridge the handler below writes them from, so that
        // they outlive it.
        QElapsedTimer clock;
        qint64 changedAt = -1;
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.holdOnceQuiet();
        QSignalSpy changed(&s.bridge, &SessionBridge::videoLoadingChanged);
        connect(&s.bridge, &SessionBridge::videoLoadingChanged, this,
                [&] { changedAt = clock.elapsed(); });

        clock.start();
        s.stamping(true);
        QTRY_VERIFY_WITH_TIMEOUT(s.bridge.videoLoading(), arcShows + slack);
        QVERIFY2(changedAt >= arcShows, qPrintable(QString::number(changedAt)));
        QVERIFY(!s.player.opening());
        clock.start();
        s.stamping(false);
        QTRY_VERIFY_WITH_TIMEOUT(!s.bridge.videoLoading(), arcGoes + slack);
        QVERIFY2(changedAt >= arcGoes, qPrintable(QString::number(changedAt)));
        QCOMPARE(changed.count(), 2);
    }

    // Review's end ends the arc, though its last change had a check of the file waiting: nothing is
    // left to answer the check, and no arc spins under the fatal end's dialog. The end is made up,
    // with review held back once it has gone quiet.
    void theArcEndsWithReview() {
        StandIn s;
        QSignalSpy fatal(&s.bridge, &SessionBridge::fatalChanged);
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.holdOnceQuiet();
        s.stamping(true);
        QTRY_VERIFY(s.bridge.videoLoading());
        emit s.bridge.eventArrived(Event(Event::kEnded{std::string("the store went away"), {}}));
        QTRY_COMPARE(fatal.count(), 1);
        QVERIFY(!s.player.opening());
        QTRY_VERIFY_WITH_TIMEOUT(!s.bridge.videoLoading(), arcGoes + slack);
    }

    // From review's check of the file into the player's Open, the arc shows as one, never gone
    // between. The change that ends the check's wait says that the Open follows; and once an Open
    // runs, a change with no check waiting and none to follow leaves the arc up, so the change
    // heard after the Open does too. Review is held back, and its changes stood in for, once it has
    // gone quiet.
    void theArcStaysUpFromTheCheckIntoTheLoad() {
        for (const bool openFirst : {false, true}) {
            const char *order =
                openFirst ? "the Open, then the change" : "the change, then the Open";
            // Before the stand-in, whose bridge stops it as it goes.
            FakeMpv mpv;
            StandIn s;
            s.player.attach(&mpv);
            s.player.ready();
            s.bridge.start();
            QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
            s.holdOnceQuiet();
            QSignalSpy changed(&s.bridge, &SessionBridge::videoLoadingChanged);
            s.stamping(true);
            QTRY_VERIFY2(s.bridge.videoLoading(), order);
            if (openFirst) {
                s.open();
                s.stamping(false);
            } else {
                s.stamping(false, true);
                s.open();
            }
            QTRY_COMPARE(mpv.last(), "get path");
            QTest::qWait(pastHiding);
            QVERIFY2(s.player.opening(), order);
            QVERIFY2(s.bridge.videoLoading(), order);
            QVERIFY2(changed.count() == 1, order);
        }
    }

    // Review's check of the file answers before the arc has shown, with the change that says the
    // Open follows, and the Open stays under way: the arc shows a moment after the press, as for
    // one wait, not a moment after the Open. The clock starts before the press, and the answer's
    // time is taken before its change is sent, so that neither time is short. Review is held back,
    // and its changes stood in for, once it has gone quiet.
    void theArcShowsAMomentAfterThePressThoughTheCheckAnswersFirst() {
        // Declared before the stand-in, whose bridge the handler below writes them from, so that
        // they outlive it; the mpv before the stand-in too, whose bridge stops it as it goes.
        QElapsedTimer clock;
        qint64 shownAt = -1;
        FakeMpv mpv;
        StandIn s;
        s.player.attach(&mpv);
        s.player.ready();
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.holdOnceQuiet();
        QSignalSpy changed(&s.bridge, &SessionBridge::videoLoadingChanged);
        connect(&s.bridge, &SessionBridge::videoLoadingChanged, this,
                [&] { shownAt = clock.elapsed(); });

        clock.start();
        s.stamping(true);
        QTest::qWait(checkAnswers);
        const qint64 answeredAt = clock.elapsed();
        QVERIFY2(!s.bridge.videoLoading(), qPrintable(QString("loading at %1 ms").arg(answeredAt)));
        s.stamping(false, true);
        s.open();
        QTRY_VERIFY_WITH_TIMEOUT(s.bridge.videoLoading(), arcShows + slack);
        QVERIFY(s.player.opening());
        QVERIFY2(shownAt >= arcShows, qPrintable(QString::number(shownAt)));
        QVERIFY2(shownAt - answeredAt < arcShows,
                 qPrintable(QString("shown %1 ms, answered %2 ms").arg(shownAt).arg(answeredAt)));
        QCOMPARE(changed.count(), 1);
    }

    // The same, but the change that ends the check's wait does not say that an Open follows: the
    // wait ends there, and the Open after it waits a moment of its own, the arc shown a moment
    // after the Open and not after the press. Review's word that the Open follows is what carries
    // the press's wait into it. The answer's time is taken before its change is sent, so that the
    // time from it is never short of the flag's own.
    void anOpenNotSaidToFollowWaitsAMomentOfItsOwn() {
        // Declared before the stand-in, as above.
        QElapsedTimer clock;
        qint64 shownAt = -1;
        FakeMpv mpv;
        StandIn s;
        s.player.attach(&mpv);
        s.player.ready();
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.holdOnceQuiet();
        QSignalSpy changed(&s.bridge, &SessionBridge::videoLoadingChanged);
        connect(&s.bridge, &SessionBridge::videoLoadingChanged, this,
                [&] { shownAt = clock.elapsed(); });

        clock.start();
        s.stamping(true);
        QTest::qWait(checkAnswers);
        const qint64 answeredAt = clock.elapsed();
        QVERIFY2(!s.bridge.videoLoading(), qPrintable(QString("loading at %1 ms").arg(answeredAt)));
        s.stamping(false);
        s.open();
        QTRY_VERIFY_WITH_TIMEOUT(s.bridge.videoLoading(), arcShows + slack);
        QVERIFY(s.player.opening());
        QVERIFY2(shownAt - answeredAt >= arcShows,
                 qPrintable(QString("shown %1 ms, answered %2 ms").arg(shownAt).arg(answeredAt)));
        QCOMPARE(changed.count(), 1);
    }

    // The change that said an Open follows is used up by the Open: once the player has it and
    // finishes it, with no change of review's after, the arc goes, though the change's flag is
    // still the last the bridge heard. Review is held back, and its changes stood in for, once it
    // has gone quiet.
    void theArcGoesWhenTheOpenItWasToldOfFinishes() {
        FakeMpv mpv;
        StandIn s;
        s.player.attach(&mpv);
        s.player.ready();
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.holdOnceQuiet();
        s.stamping(true);
        QTRY_VERIFY_WITH_TIMEOUT(s.bridge.videoLoading(), arcShows + slack);
        // The player's report would make real review answer with a change of its own, which ends
        // the flag's use; the case is that none comes.
        QObject::disconnect(&s.player, &PlayerPort::reported, &s.bridge, nullptr);
        s.stamping(false, true);
        s.open();
        QTRY_COMPARE(mpv.last(), "get path");
        QVERIFY(s.bridge.videoLoading());
        mpv.answer(s.player, {}, "property unavailable");
        mpv.answer(s.player);
        s.player.restarted();
        QTRY_VERIFY(!s.player.opening());
        QTRY_VERIFY_WITH_TIMEOUT(!s.bridge.videoLoading(), arcGoes + slack);
    }

    // A quit under a question, here `Recheck`'s: review ends at once, as no job runs, where a
    // `Quit` would only let the question go and the window would wait out the give-up.
    void quittingUnderAQuestionEndsReviewAtOnce() {
        Opened o({media()});
        QSignalSpy finished(&o.bridge, &SessionBridge::finished);
        bool asked = false;
        connect(&o.bridge, &SessionBridge::eventArrived, &o.bridge, [&asked](const Event &e) {
            if (const auto *change = std::get_if<Event::kChanged>(&e.get_variant())) {
                asked = change->summary.question.has_value();
            }
        });
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.send(Action(Action::kShowPane{Pane::kFiles}));
        o.bridge.send(Action(Action::kSelectFile{media() + "/typos - E01.mkv"}));
        o.bridge.send(Action(Action::kRecheck{}));
        QTRY_VERIFY(asked);
        o.bridge.quit();
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 0);
        QCOMPARE(o.out.str(), "media: 0 accepted, 0 rejected, 4 pending\n");
        QCOMPARE(o.err.str(), "");
    }

    // A close that throws: the pump is let go of, not joined, and what it reads after, close's
    // end here, reaches nothing of the bridge, which could be gone by then.
    void whatThePumpReadsAfterACloseThatThrewReachesNothing() {
        std::ostringstream out;
        std::ostringstream err;
        PlayerPort player{std::chrono::milliseconds(100)};
        ThrowingClose bridge({media()}, player, out, err, std::chrono::seconds(1));
        QSignalSpy fatal(&bridge, &SessionBridge::fatalChanged);
        bridge.start();
        QTRY_COMPARE(bridge.findings()->rowCount(), 2);
        QSignalSpy arrived(&bridge, &SessionBridge::eventArrived);
        // Out of step: review is let go of, as a failure lets it go.
        emit bridge.eventArrived(
            Event(Event::kChanged{Splice{3, 1, {}}, Summary{}, {}, {}, {}, {}}));
        QTRY_COMPARE(fatal.count(), 1);
        QCOMPARE(bridge.fatal(), "the list is out of step with review");
        // The stream ended now: given the time its end takes to come, it does not, and the made-up
        // change is all that arrived.
        QVERIFY(bridge.unclosed);
        bridge.unclosed->close();
        QTest::qWait(500);
        QCOMPARE(arrived.count(), 1);
    }

    // The episode marked in the sidebar is the selected finding's, as the selection moves from
    // one episode's finding to another's; and a move sends no file selection of its own, which
    // would let a decision's message go as the selection crossed into the next episode.
    void theEpisodeMarkedFollowsTheFindingAndAMoveSelectsNoFile() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.toggle();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 4);
        QCOMPARE(current(o.bridge), QStringList{"E01"});
        o.bridge.move(2);
        QTRY_COMPARE(o.bridge.selected(), 2);
        QCOMPARE(current(o.bridge), QStringList{"E02"});
        o.bridge.move(-1);
        QTRY_COMPARE(current(o.bridge), QStringList{"E01"});
        // The accept moves on into E02: its message stays, and E02 is marked.
        o.bridge.accept();
        QTRY_COMPARE(o.bridge.selected(), 2);
        QCOMPARE(o.bridge.statusLine(), "Accepted 1 finding");
        QCOMPARE(current(o.bridge), QStringList{"E02"});
        o.bridge.undo();
        QTRY_COMPARE(decision(o.bridge, 1), "To review");
        QCOMPARE(o.bridge.sent, (QStringList{"Toggle", "Move 2", "Move -1", "Accept", "Undo"}));
    }

    // A click on an episode selects its file, then its first finding, whose row of several here
    // is both episodes': the episode clicked is marked, and what the check said of it shows,
    // beside that row and not beside a finding of the other episode alone, where that one is
    // marked. The first episode is the one selected as the view opens, beside the first row.
    void aClickOnAnEpisodeSelectsItThenItsFirstFinding() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QCOMPARE(o.bridge.episodeName(), "E01");
        o.bridge.toggle();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 4);
        o.bridge.move(3);
        QTRY_COMPARE(o.bridge.selected(), 3);
        QCOMPARE(current(o.bridge), QStringList{"E01"});
        o.bridge.openEpisode(1);
        QTRY_COMPARE(o.bridge.selected(), 0);
        QCOMPARE(o.bridge.episodeName(), "E02");
        QCOMPARE(o.bridge.episodeLog(), QStringList{"4 lines, 2 findings"} +
                                            QStringList{"    track 0 English Full: cues"});
        // The row is E01's and E02's: the episode clicked stays marked.
        QCOMPARE(current(o.bridge), QStringList{"E02"});
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        QCOMPARE(o.bridge.episodeName(), "");
        QCOMPARE(o.bridge.episodeLog(), QStringList());
        QCOMPARE(current(o.bridge), QStringList{"E01"});
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.episodeName(), "E02");
        QCOMPARE(current(o.bridge), QStringList{"E02"});
        o.bridge.openEpisode(2);
        o.bridge.openEpisode(-1);
        QCOMPARE(o.bridge.sent,
                 (QStringList{"Toggle", "Move 3",
                              "SelectFile " + QString::fromStdString(media()) + "/typos - E02.mkv",
                              "ShowFileFindings", "Move 1", "Move 1"}));
    }

    // As a view opens, review's file is its first episode's. A click on that episode when the check
    // found nothing in it names that file again, which changes nothing of review's but its count:
    // what the check said of it shows with review's answer, the episode marked, and is scrolled to
    // again with a second click's. It goes once another finding is selected.
    void aClickOnTheSelectedEpisodeWithNoFindingsShowsItsLogWithReviewsAnswer() {
        Opened o({clean(), media()});
        QSignalSpy shown(&o.bridge, &SessionBridge::episodeShown);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        EpisodeOrder *files = o.bridge.episodes();
        const QString label = files->data(files->index(0, 0), EpisodesModel::LabelRole).toString();
        QVERIFY(files->data(files->index(0, 0), EpisodesModel::NoFindingsRole).toBool());
        QCOMPARE(o.bridge.episodeName(), QString());
        QCOMPARE(current(o.bridge), QStringList{"E01"});

        o.bridge.openEpisode(0);
        const QString file = QString::fromStdString(clean()) + "/typos.mkv";
        QCOMPARE(o.bridge.sent, QStringList{"SelectFile " + file});
        QCOMPARE(o.bridge.episodeName(), QString());
        QTRY_COMPARE(o.bridge.episodeName(), label);
        QVERIFY(!o.bridge.episodeLog().isEmpty());
        QCOMPARE(current(o.bridge), QStringList{label});
        QCOMPARE(shown.count(), 1);
        o.bridge.openEpisode(0);
        QTRY_COMPARE(shown.count(), 2);
        QCOMPARE(o.bridge.episodeName(), label);

        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        QCOMPARE(o.bridge.episodeName(), QString());
        QCOMPARE(o.bridge.episodeLog(), QStringList());
        QCOMPARE(current(o.bridge), QStringList{"E01"});
        QCOMPARE(shown.count(), 2);
        QCOMPARE(o.bridge.sent,
                 (QStringList{"SelectFile " + file, "SelectFile " + file, "Move 1"}));
    }

    // A click on an episode the check found nothing in, here from another episode's file, shows its
    // view with review's answer to the click; a move ends it with its own.
    void showingEpisodeFollowsAClickOnAGreyedEpisode() {
        Opened o({clean(), media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QVERIFY(!o.bridge.showingEpisode());
        o.bridge.openEpisode(1);
        QTRY_COMPARE(o.bridge.episodeName(), "E01");
        QVERIFY(!o.bridge.showingEpisode());
        o.bridge.openEpisode(0);
        QVERIFY(!o.bridge.showingEpisode());
        QTRY_VERIFY(o.bridge.showingEpisode());
        QVERIFY(o.bridge.property("showingEpisode").toBool());
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        QVERIFY(!o.bridge.showingEpisode());
    }

    // A click on another row while a greyed episode's view shows ends the view with review's
    // answer to its select, the change that carries the select's count, and nothing is said before
    // it: the panel goes from the log straight to the finding clicked, with no moment of the
    // finding review still had selected.
    void theViewEndsAtTheAnswerToTheActionThatEndsIt() {
        // Declared before the bridge, which the handler below writes it from, so that it outlives
        // it.
        QList<std::tuple<uint64_t, bool, int, qsizetype>> seen;
        Opened o({clean(), media()});
        QSignalSpy changed(&o.bridge, &SessionBridge::summaryChanged);
        // Each change once applied: its count, what the bridge then shows, and how often the bridge
        // has said so.
        connect(&o.bridge, &SessionBridge::eventArrived, &o.bridge, [&](const Event &e) {
            if (const auto *change = std::get_if<Event::kChanged>(&e.get_variant())) {
                seen << std::tuple{change->summary.answered, o.bridge.showingEpisode(),
                                   o.bridge.selected(), changed.count()};
            }
        });
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.openEpisode(0);
        QTRY_VERIFY(o.bridge.showingEpisode());
        QCOMPARE(o.bridge.selected(), 0);

        seen.clear();
        changed.clear();
        o.bridge.select(1);
        QVERIFY2(o.bridge.sent.last().startsWith("Select row "), qPrintable(o.bridge.sent.last()));
        // Each action the bridge sent counts, the select last.
        const auto select = static_cast<uint64_t>(o.bridge.sent.size());
        QVERIFY(o.bridge.showingEpisode());
        QCOMPARE(changed.count(), 0);
        QTRY_VERIFY(!seen.isEmpty());
        QCOMPARE(seen.first(), (std::tuple<uint64_t, bool, int, qsizetype>{select, false, 1, 1}));
        QCOMPARE(o.bridge.episodeName(), QString());
        QCOMPARE(current(o.bridge), QStringList{"E01"});
    }

    // A move on at the last finding moves nothing in review, and still ends the view, with review's
    // answer, which comes alone as nothing else changed: the finding is back, and its episode
    // marked again.
    void aNoOpActionStillEndsTheView() {
        Opened o({clean(), media()});
        QList<uint64_t> alone;
        connect(&o.bridge, &SessionBridge::eventArrived, &o.bridge, [&alone](const Event &e) {
            if (const auto *answer = std::get_if<Event::kAnswered>(&e.get_variant())) {
                alone << answer->answered;
            }
        });
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        o.bridge.openEpisode(0);
        QTRY_VERIFY(o.bridge.showingEpisode());
        alone.clear();
        o.bridge.move(1);
        QVERIFY(o.bridge.showingEpisode());
        QTRY_VERIFY(!o.bridge.showingEpisode());
        QCOMPARE(alone, QList<uint64_t>{static_cast<uint64_t>(o.bridge.sent.size())});
        QCOMPARE(o.bridge.selected(), 1);
        QCOMPARE(o.bridge.episodeName(), QString());
        QCOMPARE(current(o.bridge), QStringList{"E01"});
    }

    // A select of the entry already selected moves nothing in review, and still ends the view, with
    // review's answer, which changes nothing else: the finding's episode is marked again, and no
    // log shows, as the file review has selected is still the one clicked.
    void selectingTheRowAlreadySelectedEndsTheEpisodesView() {
        Opened o({clean(), media()});
        QSignalSpy changed(&o.bridge, &SessionBridge::summaryChanged);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.openEpisode(1);
        QTRY_COMPARE(o.bridge.episodeName(), "E01");
        o.bridge.openEpisode(0);
        QTRY_VERIFY(o.bridge.showingEpisode());
        const QString label = o.bridge.episodeName();
        QCOMPARE(current(o.bridge), QStringList{label});

        const qsizetype from = o.bridge.sent.size();
        changed.clear();
        o.bridge.select(o.bridge.selected());
        const QStringList sent = o.bridge.sent.mid(from);
        QVERIFY2(sent.size() == 1 && sent.first().startsWith("Select row "),
                 qPrintable(sent.join(" | ")));
        QCOMPARE(changed.count(), 0);
        QVERIFY(o.bridge.showingEpisode());
        QTRY_COMPARE(changed.count(), 1);
        QVERIFY(!o.bridge.showingEpisode());
        QCOMPARE(o.bridge.episodeName(), QString());
        QCOMPARE(o.bridge.episodeLog(), QStringList());
        QCOMPARE(current(o.bridge), QStringList{"E01"});
        QCOMPARE(o.bridge.selected(), 0);
    }

    // An undo can leave the selection where it is, and with no decision to take back review only
    // says so: the view ends with that answer.
    void undoEndsTheEpisodesView() {
        Opened o({clean(), media()});
        QSignalSpy changed(&o.bridge, &SessionBridge::summaryChanged);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.openEpisode(1);
        QTRY_COMPARE(o.bridge.episodeName(), "E01");
        o.bridge.openEpisode(0);
        QTRY_VERIFY(o.bridge.showingEpisode());

        const qsizetype from = o.bridge.sent.size();
        changed.clear();
        o.bridge.undo();
        QCOMPARE(o.bridge.sent.mid(from), QStringList{"Undo"});
        QCOMPARE(changed.count(), 0);
        QVERIFY(o.bridge.showingEpisode());
        QTRY_COMPARE(o.bridge.statusLine(), "Nothing to undo");
        QVERIFY(!o.bridge.showingEpisode());
        QCOMPARE(o.bridge.episodeName(), QString());
        QCOMPARE(current(o.bridge), QStringList{"E01"});
        QCOMPARE(o.bridge.selected(), 0);
    }

    // A move, or a step to another episode, that review cannot make changes nothing there but its
    // count, and still ends a greyed episode's view, with review's answer: the move on at the last
    // finding; the step to the next episode, which goes nowhere either; and here the step back,
    // which finds no other episode before it. Review answers the last two with a refusal.
    void everyMoveEndsTheEpisodesView() {
        Opened o({clean(), media()});
        QSignalSpy changed(&o.bridge, &SessionBridge::summaryChanged);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        const std::vector<std::tuple<QString, std::function<void()>, QString>> moves = {
            {"Move 1", [&o] { o.bridge.move(1); }, QString()},
            {"NextFile", [&o] { o.bridge.nextEpisode(); }, "This is the last episode"},
            {"PreviousFile", [&o] { o.bridge.previousEpisode(); }, "This is the first episode"},
        };
        for (const auto &[action, call, said] : moves) {
            // Review's file is still the one with no findings: the view shows with the click's
            // answer, which lets the last refusal go.
            o.bridge.openEpisode(0);
            QTRY_VERIFY2(o.bridge.showingEpisode(), qPrintable(action));
            QCOMPARE(o.bridge.statusLine(), QString());
            const qsizetype from = o.bridge.sent.size();
            changed.clear();
            call();
            QCOMPARE(o.bridge.sent.mid(from), QStringList{action});
            QCOMPARE(changed.count(), 0);
            QVERIFY2(o.bridge.showingEpisode(), qPrintable(action));
            QTRY_VERIFY2(!o.bridge.showingEpisode(), qPrintable(action));
            QCOMPARE(changed.count(), 1);
            QCOMPARE(o.bridge.statusLine(), said);
            QCOMPARE(o.bridge.episodeName(), QString());
            QCOMPARE(current(o.bridge), QStringList{"E01"});
            QCOMPARE(o.bridge.selected(), 1);
        }
    }

    // A click on a greyed episode while review has yet to answer an ask that ends the view takes
    // that end's place: the view stays through the ask's answer, though it selects another
    // finding, and shows again with the click's, scrolled to.
    void aGreyedClickCancelsAPendingEnd() {
        // Declared before the bridge, which the handler below writes it from, so that it outlives
        // it.
        QList<bool> seen;
        Opened o({clean(), media()});
        QSignalSpy shown(&o.bridge, &SessionBridge::episodeShown);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.openEpisode(0);
        QTRY_COMPARE(shown.count(), 1);
        QVERIFY(o.bridge.showingEpisode());
        connect(&o.bridge, &SessionBridge::summaryChanged, &o.bridge,
                [&] { seen << o.bridge.showingEpisode(); });

        o.bridge.move(1);
        o.bridge.openEpisode(0);
        QVERIFY(o.bridge.showingEpisode());
        QTRY_COMPARE(shown.count(), 2);
        QCOMPARE(o.bridge.selected(), 1);
        QCOMPARE(seen, (QList<bool>{true, true}));
        QVERIFY(o.bridge.showingEpisode());
    }

    // A click on a greyed episode that review has yet to take: a change answering an action sent
    // before it, which selects another finding, leaves it be, and its episode shows once review
    // answers the click's `SelectFile`. Review is held back, and its changes stood in for, once
    // E01's file is selected, so that the click changes review's file.
    void aStaleChangeDoesNotEndANewerClicksView() {
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.bridge.openEpisode(1);
        QTRY_COMPARE(s.bridge.episodeName(), "E01");
        s.holdOnceQuiet();
        const auto count = static_cast<uint64_t>(s.bridge.sent.size());
        QVERIFY(s.last && s.last->summary.answered == count);
        EpisodeOrder *files = s.bridge.episodes();
        const QString label = files->data(files->index(0, 0), EpisodesModel::LabelRole).toString();

        s.bridge.cycleFilter();
        s.bridge.openEpisode(0);
        QCOMPARE(s.bridge.sent.size(), qsizetype(count + 2));
        // The filter's answer, which selects the second row.
        s.answer(count + 1, 1, 1);
        QTRY_COMPARE(s.bridge.selected(), 1);
        QVERIFY(!s.bridge.showingEpisode());
        // The click's.
        s.answer(count + 2, 0, 1, std::nullopt, LogsNow{FileLogs{{"Its log"}, {}}});
        QTRY_VERIFY(s.bridge.showingEpisode());
        QCOMPARE(s.bridge.episodeName(), label);
        QCOMPARE(s.bridge.episodeLog(), QStringList{"Its log"});
        QCOMPARE(current(s.bridge), QStringList{label});
        QCOMPARE(s.bridge.selected(), 1);
    }

    // A click's answer that leaves another file selected shows no log, then or with a later change
    // of review's that has the clicked file selected after all: the view shows what review's answer
    // to the click has. That answer comes alone, as nothing else changed, so nothing is said for
    // it: the later change, said once, is taken after it.
    void aClickAnsweredWithAnotherFileShowsNothing() {
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.bridge.openEpisode(1);
        QTRY_COMPARE(s.bridge.episodeName(), "E01");
        s.holdOnceQuiet();
        const auto count = static_cast<uint64_t>(s.bridge.sent.size());
        QSignalSpy changed(&s.bridge, &SessionBridge::summaryChanged);
        QSignalSpy shown(&s.bridge, &SessionBridge::episodeShown);

        s.bridge.openEpisode(0);
        s.answerAlone(count + 1);
        s.answer(count + 1, 0, 0, std::nullopt, LogsNow{FileLogs{{"Its log"}, {}}});
        QTRY_COMPARE(changed.count(), 1);
        QVERIFY(!s.bridge.showingEpisode());
        QCOMPARE(s.bridge.episodeLog(), QStringList());
        QCOMPARE(shown.count(), 0);
    }

    // A click on an episode with findings while a greyed episode's view shows sends two actions,
    // its file and then its first finding. The view holds through review's answer to the first,
    // which has the new file selected and the finding selected before, and ends with the answer to
    // the second, on that episode's first finding. Review is held back, and its changes stood in
    // for, so that the first answer comes alone.
    void aClickOnAnEpisodeWithFindingsEndsTheViewAtItsLastAnswer() {
        // Declared before the stand-in, whose bridge the handler below writes it from, so that it
        // outlives it.
        QList<std::pair<bool, QString>> seen;
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.bridge.move(1);
        QTRY_COMPARE(s.bridge.selected(), 1);
        s.holdOnceQuiet();
        QVERIFY(s.first && s.first->detail && s.first->files);
        QCOMPARE(s.first->files->inserted.size(), std::size_t(3));
        const QString label = QString::fromStdString(s.first->files->inserted[0].label);
        const QString e02 = QString::fromStdString(s.first->files->inserted[2].path);
        const auto count = static_cast<uint64_t>(s.bridge.sent.size());
        s.bridge.openEpisode(0);
        s.answer(count + 1, 0, 1);
        QTRY_VERIFY(s.bridge.showingEpisode());
        const QStringList log = s.bridge.episodeLog();
        QVERIFY(!log.isEmpty());
        connect(&s.bridge, &SessionBridge::summaryChanged, &s.bridge,
                [&] { seen << std::pair{s.bridge.showingEpisode(), s.bridge.episodeName()}; });

        s.bridge.openEpisode(2);
        QCOMPARE(s.bridge.sent.mid(s.bridge.sent.size() - 2),
                 (QStringList{"SelectFile " + e02, "ShowFileFindings"}));
        // `SelectFile`'s answer: E02's file and log, and the finding selected before.
        s.answer(count + 2, 2, 1, std::nullopt, LogsNow{FileLogs{{"E02's log"}, {}}});
        QTRY_COMPARE(seen.size(), 1);
        QCOMPARE(seen.last(), (std::pair<bool, QString>{true, label}));
        QCOMPARE(s.bridge.episodeLog(), log);
        QCOMPARE(current(s.bridge), QStringList{label});
        // `ShowFileFindings`': the first row, E01's and E02's, with its detail.
        s.answer(count + 3, 2, 0, std::nullopt, std::nullopt, s.first->detail);
        QTRY_COMPARE(seen.size(), 2);
        QCOMPARE(seen.last(), (std::pair<bool, QString>{false, "E02"}));
        QCOMPARE(s.bridge.selected(), 0);
        QCOMPARE(s.bridge.detail()->place(), "E01 · 0:01");
        QCOMPARE(s.bridge.episodeLog(), QStringList{"E02's log"});
        QCOMPARE(current(s.bridge), QStringList{"E02"});
    }

    // A change of review's own after a greyed click's answer, as a job's is, carries the click's
    // count: one that selects another finding ends the view.
    void aJobsMoveAfterTheClicksAnswerEndsTheView() {
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.holdOnceQuiet();
        const auto count = static_cast<uint64_t>(s.bridge.sent.size());
        s.bridge.openEpisode(0);
        s.answer(count + 1, 0, 0);
        QTRY_VERIFY(s.bridge.showingEpisode());
        // No action of the window's since the click, and another finding selected.
        s.answer(count + 1, 0, 1);
        QTRY_COMPARE(s.bridge.selected(), 1);
        QTRY_VERIFY(!s.bridge.showingEpisode());
        QCOMPARE(s.bridge.episodeName(), QString());
    }

    // Review's answer to a later ask, here Switch View's, that has another file selected and the
    // same finding ends a greyed episode's view: the log shown is no longer review's file's.
    void anotherFileSelectedEndsTheView() {
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.holdOnceQuiet();
        const auto count = static_cast<uint64_t>(s.bridge.sent.size());
        s.bridge.openEpisode(0);
        s.answer(count + 1, 0, 0);
        QTRY_VERIFY(s.bridge.showingEpisode());
        s.bridge.switchView();
        s.answer(count + 2, 1, 0);
        QTRY_VERIFY(!s.bridge.showingEpisode());
        QCOMPARE(s.bridge.selected(), 0);
        QCOMPARE(current(s.bridge), QStringList{"E01"});
    }

    // A greyed click the bridge sends while it still holds a question review has let go, at a key
    // whose answer has yet to come: review takes the click as any other, and its answer, with the
    // file clicked selected, shows the episode.
    void aGreyedClickAfterAQuestionWasLetGoShowsAtItsAnswer() {
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.bridge.openEpisode(1);
        QTRY_COMPARE(s.bridge.episodeName(), "E01");
        // Recheck asks first. The window has no way to ask it yet, so the test sends it.
        s.bridge.send(Action(Action::kRecheck{}));
        QTRY_VERIFY(s.last && s.last->summary.question);
        s.holdOnceQuiet();
        const auto count = static_cast<uint64_t>(s.bridge.sent.size());
        s.bridge.cycleFilter();
        s.bridge.openEpisode(0);
        s.last->summary.question.reset();
        // The filter key's answer, which let the question go; then the click's.
        s.answer(count + 1, 1, 0);
        s.answer(count + 2, 0, 0, std::nullopt, LogsNow{FileLogs{{"Its log"}, {}}});
        QTRY_VERIFY(s.bridge.showingEpisode());
        QCOMPARE(s.bridge.episodeLog(), QStringList{"Its log"});
    }

    // A greyed click review takes while it asks only lets the question go, and shows nothing, even
    // with the file clicked already review's, as it is as the view opens.
    void aGreyedClickReviewTakesWhileAskingShowsNothing() {
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.bridge.send(Action(Action::kRecheck{}));
        QTRY_VERIFY(s.last && s.last->summary.question);
        s.holdOnceQuiet();
        const auto count = static_cast<uint64_t>(s.bridge.sent.size());
        QSignalSpy changed(&s.bridge, &SessionBridge::summaryChanged);
        s.bridge.openEpisode(0);
        s.last->summary.question.reset();
        s.answer(count + 1, 0, 0);
        QTRY_COMPARE(changed.count(), 1);
        QVERIFY(!s.bridge.showingEpisode());
        QCOMPARE(s.bridge.episodeName(), QString());
    }

    // A greyed click review takes while it asks only lets the question go, and the view stays as it
    // was before the click: a job's change meanwhile, which answers an action sent after the shown
    // view's click and selects another finding, ends that view once the click's answer says review
    // was asking. Review is held back, and its changes stood in for, once it asks; the second
    // greyed episode is made up.
    void aClickReviewTakesWhileAskingLeavesAJobsMoveToEndTheView() {
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        // Review's file as the view opens is the greyed episode's: its view shows with the answer.
        s.bridge.openEpisode(0);
        QTRY_VERIFY(s.bridge.showingEpisode());
        // Recheck asks. The window has no way to ask it yet, so the test sends it.
        s.bridge.send(Action(Action::kRecheck{}));
        QTRY_VERIFY(s.last && s.last->summary.question);
        s.holdOnceQuiet();
        QVERIFY(s.bridge.showingEpisode());
        QVERIFY(s.first && s.first->files && !s.first->files->inserted.empty());
        FileView other = s.first->files->inserted[0];
        other.path = clean() + "/other.mkv";
        other.label = "Other";
        const auto count = static_cast<uint64_t>(s.bridge.sent.size());
        s.answer(count, 0, 0, FilesSplice{1, 0, {other}});
        QTRY_COMPARE(s.bridge.episodes()->rowCount(), 4);

        s.bridge.openEpisode(1);
        // A job's change: no action since the question, another finding selected. The view holds
        // while the click's answer is awaited.
        s.answer(count, 0, 1);
        QTRY_COMPARE(s.bridge.selected(), 1);
        QVERIFY(s.bridge.showingEpisode());
        // The click's answer: review only let the question go, its file still the first's.
        s.last->summary.question.reset();
        s.answer(count + 1, 0, 1);
        QTRY_VERIFY(!s.bridge.showingEpisode());
        QCOMPARE(s.bridge.episodeName(), QString());
    }

    // Two greyed clicks, the second taken while review asks: review answered the first before the
    // question came, so once the second's answer says review was asking, the first's view, put
    // back, shows. Review is held back, and its changes stood in for; the second greyed episode is
    // made up.
    void aClickReviewTakesWhileAskingPutsBackTheClickBefore() {
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.holdOnceQuiet();
        QVERIFY(s.first && s.first->files && !s.first->files->inserted.empty());
        FileView other = s.first->files->inserted[0];
        other.path = clean() + "/other.mkv";
        other.label = "Other";
        const auto count = static_cast<uint64_t>(s.bridge.sent.size());
        s.answer(count, 0, 0, FilesSplice{1, 0, {other}});
        QTRY_COMPARE(s.bridge.episodes()->rowCount(), 4);
        QSignalSpy shown(&s.bridge, &SessionBridge::episodeShown);

        s.bridge.openEpisode(1);
        s.bridge.openEpisode(0);
        // The first click's answer, with its file and log; then a job's change that asks.
        s.answer(count + 1, 1, 0, std::nullopt, LogsNow{FileLogs{{"Other's log"}, {}}});
        s.last->summary.question = Question(Question::kRecheck{"Other", "claude"});
        s.answer(count + 1, 1, 0);
        // The second click's answer: review only let the question go.
        s.last->summary.question.reset();
        s.answer(count + 2, 1, 0);
        QTRY_VERIFY(s.bridge.showingEpisode());
        QCOMPARE(s.bridge.episodeName(), "Other");
        QCOMPARE(s.bridge.episodeLog(), QStringList{"Other's log"});
        QCOMPARE(shown.count(), 1);
    }

    // A playback key's answer changes nothing the window shows, and comes alone: nothing is said
    // for it and nothing drawn again. The move after it is said, once, after the answer was taken.
    void aPlaybackKeysAnswerAloneSaysNothing() {
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.holdOnceQuiet();
        const auto count = static_cast<uint64_t>(s.bridge.sent.size());
        QSignalSpy changed(&s.bridge, &SessionBridge::summaryChanged);
        QSignalSpy filters(s.bridge.filters(), &QAbstractItemModel::dataChanged);
        s.bridge.pause();
        s.answerAlone(count + 1);
        s.bridge.move(1);
        s.answer(count + 2, 0, 1);
        QTRY_COMPARE(changed.count(), 1);
        QCOMPARE(s.bridge.selected(), 1);
        QCOMPARE(filters.count(), 1);
        QCOMPARE(s.bridge.sent.mid(s.bridge.sent.size() - 2),
                 (QStringList{"Playback Pause", "Move 1"}));
    }

    // A click on a second episode with no findings, while the first's view shows, keeps the first
    // shown until review answers it: a change answering an earlier action, which still has the
    // first's file selected, changes nothing shown; the second's log shows with the click's
    // answer, and is scrolled to. Review is held back, and its changes stood in for, as the seeded
    // run has one episode with no findings: the second is made up.
    void aSecondGreyedClickKeepsTheFirstShownUntilItsAnswer() {
        // Declared before the stand-in, whose bridge the handler below writes it from, so that it
        // outlives it.
        QList<std::pair<bool, QString>> seen;
        StandIn s;
        s.bridge.start();
        QTRY_COMPARE(s.bridge.findings()->rowCount(), 2);
        s.holdOnceQuiet();
        QVERIFY(s.first && s.first->files && !s.first->files->inserted.empty());
        QVERIFY(s.last->summary.file_selected == std::optional<uint64_t>(0));
        const QString label = QString::fromStdString(s.first->files->inserted[0].label);
        const QString path = QString::fromStdString(s.first->files->inserted[0].path);
        FileView other = s.first->files->inserted[0];
        other.path = clean() + "/other.mkv";
        other.label = "Other";
        const auto count = static_cast<uint64_t>(s.bridge.sent.size());
        // The made-up episode, after the first, in a change that answers no action of its own.
        s.answer(count, 0, 0, FilesSplice{1, 0, {other}});
        QTRY_COMPARE(s.bridge.episodes()->rowCount(), 4);
        QVERIFY(s.bridge.episodes()
                    ->data(s.bridge.episodes()->index(1, 0), EpisodesModel::NoFindingsRole)
                    .toBool());
        QSignalSpy shown(&s.bridge, &SessionBridge::episodeShown);
        connect(&s.bridge, &SessionBridge::summaryChanged, &s.bridge,
                [&] { seen << std::pair{s.bridge.showingEpisode(), s.bridge.episodeName()}; });

        s.bridge.openEpisode(0);
        QCOMPARE(seen.size(), 0);
        s.answer(count + 1, 0, 0);
        QTRY_COMPARE(seen.size(), 1);
        QCOMPARE(seen.last(), (std::pair<bool, QString>{true, label}));
        QCOMPARE(shown.count(), 1);
        QCOMPARE(current(s.bridge), QStringList{label});

        s.bridge.openEpisode(1);
        QCOMPARE(s.bridge.sent.mid(s.bridge.sent.size() - 2),
                 (QStringList{"SelectFile " + path,
                              "SelectFile " + QString::fromStdString(other.path)}));
        QCOMPARE(seen.size(), 1);
        // A change answering the first click, as a job's after it would, here one that adds to the
        // first's log: the first stays shown, its log as review now has it.
        s.answer(count + 1, 0, 0, std::nullopt, LogsNow{FileLogs{{"More of its log"}, {}}});
        QTRY_COMPARE(seen.size(), 2);
        QCOMPARE(seen.last(), (std::pair<bool, QString>{true, label}));
        QCOMPARE(s.bridge.episodeLog(), QStringList{"More of its log"});
        QCOMPARE(current(s.bridge), QStringList{label});
        QCOMPARE(shown.count(), 1);

        s.answer(count + 2, 1, 0, std::nullopt, LogsNow{FileLogs{{"Other's log"}, {}}});
        QTRY_COMPARE(seen.size(), 3);
        QCOMPARE(seen.last(), (std::pair<bool, QString>{true, "Other"}));
        QCOMPARE(s.bridge.episodeLog(), QStringList{"Other's log"});
        QCOMPARE(current(s.bridge), QStringList{"Other"});
        QCOMPARE(shown.count(), 2);
    }

    // Under a question, review only lets it go, whatever else is asked, and keeps the finding
    // selected. A select, a move, a step to another episode and an undo each still end a greyed
    // episode's view, with that answer, whatever the bridge last heard of a question: the finding
    // review keeps then shows, which is where the view would end without the question.
    void underAQuestionAMoveOrUndoStillEndsTheEpisodesView() {
        Opened o({clean(), media()});
        bool asked = false;
        connect(&o.bridge, &SessionBridge::eventArrived, &o.bridge, [&asked](const Event &e) {
            if (const auto *change = std::get_if<Event::kChanged>(&e.get_variant())) {
                asked = change->summary.question.has_value();
            }
        });
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        const std::vector<std::pair<QString, std::function<void()>>> asks = {
            {"Select", [&o] { o.bridge.select(o.bridge.selected()); }},
            {"Undo", [&o] { o.bridge.undo(); }},
            {"Move 1", [&o] { o.bridge.move(1); }},
            {"NextFile", [&o] { o.bridge.nextEpisode(); }},
            {"PreviousFile", [&o] { o.bridge.previousEpisode(); }},
        };
        for (const auto &[action, call] : asks) {
            // Review's file is still the one with no findings: the view shows with the click's
            // answer.
            o.bridge.openEpisode(0);
            QTRY_VERIFY2(o.bridge.showingEpisode(), qPrintable(action));
            // Recheck asks first. The window has no way to ask it yet, so the test sends it.
            o.bridge.send(Action(Action::kRecheck{}));
            QTRY_VERIFY2(asked, qPrintable(action));
            call();
            QVERIFY2(o.bridge.sent.last().startsWith(action), qPrintable(o.bridge.sent.last()));
            QVERIFY2(o.bridge.showingEpisode(), qPrintable(action));
            QTRY_VERIFY2(!asked, qPrintable(action));
            QVERIFY2(!o.bridge.showingEpisode(), qPrintable(action));
            QCOMPARE(o.bridge.episodeName(), QString());
            QCOMPARE(current(o.bridge), QStringList{"E01"});
            QCOMPARE(o.bridge.selected(), 0);
        }
    }

    // Another entry selected is said, a row or a member of one; a row expanded where it is, still
    // the entry selected, is not.
    void anotherEntrySelectedIsSaidAndAnExpandedRowIsNot() {
        Opened o({media()});
        QSignalSpy moved(&o.bridge, &SessionBridge::selectionMoved);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QTRY_COMPARE(o.bridge.selected(), 0);
        QCOMPARE(moved.count(), 1);
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        QCOMPARE(moved.count(), 2);
        o.bridge.toggle();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 4);
        QCOMPARE(moved.count(), 2);
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 2);
        QCOMPARE(moved.count(), 3);
        o.bridge.move(1);
        QTRY_COMPARE(o.bridge.selected(), 3);
        QCOMPARE(moved.count(), 4);
    }

    // Each filter from the sidebar's list, by its row; the next steps through them in the list's
    // order, the first after the last; the list's heading names the one shown.
    void theFiltersAreSetByTheirRowAndTheNextGoesInTheirOrder() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.setFilter(2);
        QTRY_COMPARE(o.bridge.heading(), "Accepted · 0");
        QCOMPARE(o.bridge.findings()->rowCount(), 0);
        o.bridge.cycleFilter();
        QTRY_COMPARE(o.bridge.heading(), "Rejected · 0");
        o.bridge.cycleFilter();
        QTRY_COMPARE(o.bridge.heading(), "All Findings · 4");
        o.bridge.cycleFilter();
        QTRY_COMPARE(o.bridge.heading(), "To Review · 4");
        o.bridge.setFilter(4);
        o.bridge.setFilter(0);
        QTRY_COMPARE(o.bridge.heading(), "All Findings · 4");
        QCOMPARE(o.bridge.sent,
                 (QStringList{"SetFilter Accepted", "SetFilter Rejected", "SetFilter All",
                              "SetFilter Pending", "SetFilter All"}));
    }

    // A review from the sidebar opens in place of the one shown, which the list then marks; the
    // View menu's words follow the kind of view shown.
    void aReviewOpensFromTheSidebarAndTheViewSwitchesBetweenACheckAndItsFolders() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        FoldersModel *reviews = o.bridge.folders();
        o.bridge.openFolder(0);
        QTRY_COMPARE(o.bridge.title(), "clean");
        QTRY_VERIFY(reviews->data(reviews->index(0), FoldersModel::OpenRole).toBool());
        QVERIFY(!reviews->data(reviews->index(1), FoldersModel::OpenRole).toBool());
        QCOMPARE(o.bridge.emptyWords(), "No findings in this run");
        o.bridge.openFolder(1);
        QTRY_COMPARE(o.bridge.title(), "media");
        QCOMPARE(o.bridge.switchLabel(), "Show Newest Check");
        o.bridge.switchView();
        QTRY_COMPARE(o.bridge.switchLabel(), "Show Folders");
        QVERIFY(!reviews->data(reviews->index(1), FoldersModel::OpenRole).toBool());
        o.bridge.switchView();
        QTRY_COMPARE(o.bridge.switchLabel(), "Show Newest Check");
        o.bridge.openFolder(2);
        const QString media = QString::fromStdString(::media());
        QCOMPARE(o.bridge.sent, (QStringList{"OpenFolder " + QFileInfo(media).path() + "/clean",
                                             "OpenFolder " + media, "SwitchView", "SwitchView"}));
    }

    // Every row here holds both episodes, so from the first row there is none before and none
    // after; a clear with nothing decided says so.
    void theEpisodesStepAndAClearPutsBackWhatIsDecided() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        o.bridge.previousEpisode();
        QTRY_COMPARE(o.bridge.statusLine(), "This is the first episode");
        o.bridge.nextEpisode();
        QTRY_COMPARE(o.bridge.statusLine(), "This is the last episode");
        o.bridge.clear();
        QTRY_COMPARE(o.bridge.statusLine(), "Nothing to put back: none of it is decided");
        o.bridge.reject();
        QTRY_COMPARE(decision(o.bridge, 0), "Rejected");
        o.bridge.move(-1);
        QTRY_COMPARE(o.bridge.selected(), 0);
        o.bridge.clear();
        QTRY_COMPARE(decision(o.bridge, 0), "To review");
        QCOMPARE(o.bridge.statusLine(), "2 findings back to undecided");
        QCOMPARE(o.bridge.sent,
                 (QStringList{"PreviousFile", "NextFile", "Clear", "Reject", "Move -1", "Clear"}));
    }

    void anEpisodesSpliceThatDoesNotFitIsAFatalEnd() {
        Opened o({media()});
        QSignalSpy fatal(&o.bridge, &SessionBridge::fatalChanged);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        emit o.bridge.eventArrived(
            Event(Event::kChanged{std::nullopt, Summary{}, {}, FilesSplice{5, 1, {}}, {}, {}}));
        QTRY_COMPARE(fatal.count(), 1);
        QCOMPARE(o.bridge.fatal(), "the episodes are out of step with review");
        QCOMPARE(o.bridge.episodes()->rowCount(), 2);
    }

    void aSpliceThatDoesNotFitIsAFatalEnd() {
        Opened o({media()});
        QSignalSpy fatal(&o.bridge, &SessionBridge::fatalChanged);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        emit o.bridge.eventArrived(
            Event(Event::kChanged{Splice{3, 1, {}}, Summary{}, {}, {}, {}, {}}));
        QTRY_COMPARE(fatal.count(), 1);
        QCOMPARE(o.bridge.fatal(), "the list is out of step with review");
        QCOMPARE(o.bridge.findings()->rowCount(), 2);
    }

    // The Settings window's sample is there before any session: a word taken out and a longer one
    // put in, each marked as the details panel marks a change.
    void theSampleIsAChangeOfOneWordForAnother() {
        Opened o({media()});
        DetailView *sample = o.bridge.sample();
        QVERIFY(sample->shown());
        QVERIFY(sample != o.bridge.detail());
        QCOMPARE(sample->now(), marked("We should of left before it got dark.", Span{10, 12}, true,
                                       Marking::InPanel));
        QCOMPARE(sample->fixed(), marked("We should have left before it got dark.", Span{10, 14},
                                         false, Marking::InPanel));
        QVERIFY(sample->now().contains("<s>of</s>"));
        QVERIFY(sample->fixed().contains("<b>have</b>"));
    }

    // A recolour redraws, in place, everything drawn in the palette's colours: the list's rows, the
    // details and the sample; and moves nothing, so the selection stays.
    void aRecolourRedrawsTheListTheDetailsAndTheSampleInPlace() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QSignalSpy rows(o.bridge.findings(), &QAbstractItemModel::dataChanged);
        QSignalSpy detail(o.bridge.detail(), &DetailView::changed);
        QSignalSpy sample(o.bridge.sample(), &DetailView::changed);
        QSignalSpy moved(&o.bridge, &SessionBridge::selectionMoved);
        o.bridge.recolour();
        QCOMPARE(rows.count(), 1);
        QCOMPARE(detail.count(), 1);
        QCOMPARE(sample.count(), 1);
        QCOMPARE(moved.count(), 0);
        QCOMPARE(o.bridge.selected(), 0);
        QCOMPARE(o.bridge.sent, QStringList());
    }
};

SUBCHECK_OFFSCREEN_TEST_MAIN(SessionBridgeTest)
#include "session_bridge_test.moc"
