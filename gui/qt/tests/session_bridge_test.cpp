// The bridge on real sessions over the run seeded for it: two copies of typos.mkv, so two rows of
// two members each, four findings pending. The tests share the run in turn, so each leaves it as
// it found it.
#include "fake_mpv.h"
#include "offscreen_main.h"
#include "session_bridge.h"
#include "told.h"
#include "words.h"

#include <QElapsedTimer>
#include <QFileInfo>
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

// A bridge that, once `holding`, keeps each action from review as it tells it, counted as sent: a
// test then stands in for review's answers, each at the moment it chooses, with the count review
// would give it.
class Holding : public Telling {
public:
    using Telling::Telling;

    bool holding = false;

    void send(Action action) override {
        if (holding) {
            sent << told(action);
            ++asked;
            return;
        }
        Telling::send(std::move(action));
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
