// The bridge on real sessions over the run seeded for it: two copies of typos.mkv, so two rows of
// two members each, four findings pending. The tests share the run in turn, so each leaves it as
// it found it.
#include "session_bridge.h"

#include <QElapsedTimer>
#include <QSignalSpy>
#include <QTest>

#include <chrono>
#include <sstream>
#include <string>
#include <vector>

using namespace subcheck_ffi;

namespace {

// The seeded run's folder: the session's selector.
std::string media() { return qEnvironmentVariable("SUBCHECK_TEST_MEDIA").toStdString(); }

// The folder of the run with no findings.
std::string clean() { return qEnvironmentVariable("SUBCHECK_TEST_CLEAN").toStdString(); }

const QString counts = "4 pending · 0 accepted · 0 rejected";

const QString firstRow = "· ×2 0:00:01.00 We should leave befor it rains. → We should leave "
                         "before it rains.";

QString line(SessionBridge &bridge, int row) {
    FindingsModel *list = bridge.findings();
    return list->data(list->index(row), FindingsModel::LineRole).toString();
}

QString decision(SessionBridge &bridge, int row) {
    FindingsModel *list = bridge.findings();
    return list->data(list->index(row), FindingsModel::StateRole).toString();
}

// A bridge on `selector`, its end's text in `out` and `err`. A short wait after a quit, so that
// the give-up's test runs in about a second: the store is the test's own, so no lock held
// elsewhere slows review's end in the others.
struct Opened {
    std::ostringstream out;
    std::ostringstream err;
    SessionBridge bridge;

    explicit Opened(std::vector<std::string> selector)
        : bridge(std::move(selector), out, err, std::chrono::seconds(1)) {}
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
        QCOMPARE(o.bridge.statusLine(), "opening…");
        QCOMPARE(o.bridge.title(), "subcheck");
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QCOMPARE(o.bridge.selected(), 0);
        QCOMPARE(line(o.bridge, 0), firstRow);
        QCOMPARE(o.bridge.statusLine(), counts);
        QCOMPARE(o.bridge.title(), "media");
        QVERIFY(!o.bridge.empty());
    }

    // A move at either end selects the row it is on, which lets a message go. A click sends a
    // Select of its row's key, an EntryKey made in C++ for Rust.
    void aMoveStopsAtEitherEndAndAClickSelectsItsRow() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QVERIFY(o.bridge.key(Qt::Key_J, "j"));
        QTRY_COMPARE(o.bridge.selected(), 1);
        QVERIFY(o.bridge.key(Qt::Key_U, "u"));
        QTRY_COMPARE(o.bridge.statusLine(), "nothing to undo");
        QVERIFY(o.bridge.key(Qt::Key_Down, ""));
        QTRY_COMPARE(o.bridge.statusLine(), counts);
        QCOMPARE(o.bridge.selected(), 1);
        QVERIFY(o.bridge.key(Qt::Key_K, "k"));
        QTRY_COMPARE(o.bridge.selected(), 0);
        QVERIFY(o.bridge.key(Qt::Key_U, "u"));
        QTRY_COMPARE(o.bridge.statusLine(), "nothing to undo");
        QVERIFY(o.bridge.key(Qt::Key_Up, ""));
        QTRY_COMPARE(o.bridge.statusLine(), counts);
        QCOMPARE(o.bridge.selected(), 0);
        // A click: on a row, it selects it; outside the list there is no key to send, so a message
        // stays until a move lets it go.
        o.bridge.select(1);
        QTRY_COMPARE(o.bridge.selected(), 1);
        QVERIFY(o.bridge.key(Qt::Key_U, "u"));
        QTRY_COMPARE(o.bridge.statusLine(), "nothing to undo");
        o.bridge.select(2);
        o.bridge.select(-1);
        QTest::qWait(100);
        QCOMPARE(o.bridge.statusLine(), "nothing to undo");
        o.bridge.move(0);
        QTRY_COMPARE(o.bridge.statusLine(), counts);
        QCOMPARE(o.bridge.selected(), 1);
        QVERIFY(!o.bridge.key(Qt::Key_X, "x"));
    }

    // Keys sent before review has answered the one before: each moves on from where review's
    // selection is by then, as the terminal's do, not from the summary the bridge last applied.
    void movesSentFasterThanReviewAnswersAddUp() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QVERIFY(o.bridge.key(Qt::Key_Return, "\r"));
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 4);
        QVERIFY(o.bridge.key(Qt::Key_J, "j"));
        QTRY_COMPARE(o.bridge.selected(), 1);
        QVERIFY(o.bridge.key(Qt::Key_J, "j"));
        QVERIFY(o.bridge.key(Qt::Key_J, "j"));
        QTRY_COMPARE(o.bridge.selected(), 3);
    }

    // An accept moves on to the next pending entry; a move sent before review has answered it
    // starts from there, so that the next key acts where the terminal's would. The selection is
    // read once the j's answer is in, which lets the accept's message go, so the counts show; and
    // the accept is undone before it is checked, so that the run is left as found.
    void aMoveSentBeforeAnAcceptIsAnsweredStartsWhereItMovedOn() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QVERIFY(o.bridge.key(Qt::Key_J, "j"));
        QTRY_COMPARE(o.bridge.selected(), 1);
        QVERIFY(o.bridge.key(Qt::Key_Return, "\r"));
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 4);
        QVERIFY(o.bridge.key(Qt::Key_K, "k"));
        QTRY_COMPARE(o.bridge.selected(), 0);
        QVERIFY(o.bridge.key(Qt::Key_A, "a"));
        QVERIFY(o.bridge.key(Qt::Key_J, "j"));
        QTRY_COMPARE(o.bridge.statusLine(), "2 pending · 2 accepted · 0 rejected");
        const int landed = o.bridge.selected();
        QVERIFY(o.bridge.key(Qt::Key_U, "u"));
        QTRY_COMPARE(decision(o.bridge, 0), "pending");
        QCOMPARE(landed, 2);
    }

    void enterExpandsARowAndCollapsesIt() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QVERIFY(o.bridge.key(Qt::Key_Return, "\r"));
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 4);
        QCOMPARE(line(o.bridge, 1), "    · E01 0:00:01.00");
        QCOMPARE(line(o.bridge, 2), "    · E02 0:00:01.00");
        o.bridge.move(2);
        QTRY_COMPARE(o.bridge.selected(), 2);
        // From a member, it collapses its row, which it selects.
        QVERIFY(o.bridge.key(Qt::Key_Enter, ""));
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QCOMPARE(o.bridge.selected(), 0);
        QCOMPARE(line(o.bridge, 0), firstRow);
    }

    void aDecisionShowsAndItsUndoPutsItBack() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QVERIFY(o.bridge.key(Qt::Key_A, "a"));
        QTRY_COMPARE(decision(o.bridge, 0), "accepted");
        QCOMPARE(o.bridge.statusLine(), "accepted 2 findings");
        QCOMPARE(o.bridge.selected(), 1);
        QVERIFY(o.bridge.key(Qt::Key_R, "r"));
        QTRY_COMPARE(decision(o.bridge, 1), "rejected");
        QVERIFY(o.bridge.key(Qt::Key_U, "u"));
        QTRY_COMPARE(decision(o.bridge, 1), "pending");
        QVERIFY(o.bridge.key(Qt::Key_U, "u"));
        QTRY_COMPARE(decision(o.bridge, 0), "pending");
        // Any action lets a message go, and the counts show again.
        o.bridge.move(0);
        QTRY_COMPARE(o.bridge.statusLine(), counts);
    }

    void quittingPrintsTheCountsAndEndsWithZero() {
        Opened o({media()});
        QSignalSpy finished(&o.bridge, &SessionBridge::finished);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        QVERIFY(o.bridge.key(Qt::Key_Q, "q"));
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 0);
        QCOMPARE(o.out.str(), "media: 0 accepted, 0 rejected, 4 pending\n");
        QCOMPARE(o.err.str(), "");
        // Once let go, what the pump read is dropped, and a key does nothing.
        emit o.bridge.eventArrived(Event(Event::kChanged{Splice{0, 2, {}}, Summary{}}));
        QVERIFY(o.bridge.key(Qt::Key_A, "a"));
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
        QTRY_VERIFY(o.bridge.empty());
        QCOMPARE(o.bridge.findings()->rowCount(), 0);
        QCOMPARE(o.bridge.selected(), -1);
        QCOMPARE(o.bridge.statusLine(), "0 pending · 0 accepted · 0 rejected");
        QCOMPARE(o.bridge.title(), "clean");
        QVERIFY(o.bridge.key(Qt::Key_J, "j"));
        QVERIFY(o.bridge.key(Qt::Key_Q, "q"));
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 0);
        QCOMPARE(o.out.str(), "clean: 0 accepted, 0 rejected, 0 pending\n");
    }

    // The window starts no job, so a real end has no outro: this one is made up, to show where
    // it goes.
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
        QCOMPARE(o.bridge.statusLine(), "quitting: waiting for review to end");
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

    // Nothing this window sends asks for the player; a command that comes anyway is told there is
    // none, which the status line says.
    void aPlayerCommandIsToldThereIsNoPlayer() {
        Opened o({media()});
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        const std::vector<PlayerCommand> commands = {
            PlayerCommand(PlayerCommand::kOpen{Moment{"typos.mkv", 0, std::nullopt, false}}),
            PlayerCommand(PlayerCommand::kPlayback{Playback(Playback::kPause{})}),
            PlayerCommand(PlayerCommand::kClose{}),
        };
        for (const PlayerCommand &command : commands) {
            emit o.bridge.eventArrived(Event(Event::kPlayer{command}));
            QTRY_COMPARE(o.bridge.statusLine(), "this window has no player yet");
            // Any action lets a message go.
            o.bridge.move(0);
            QTRY_COMPARE(o.bridge.statusLine(), counts);
        }
    }

    void aSpliceThatDoesNotFitIsAFatalEnd() {
        Opened o({media()});
        QSignalSpy fatal(&o.bridge, &SessionBridge::fatalChanged);
        o.bridge.start();
        QTRY_COMPARE(o.bridge.findings()->rowCount(), 2);
        emit o.bridge.eventArrived(Event(Event::kChanged{Splice{3, 1, {}}, Summary{}}));
        QTRY_COMPARE(fatal.count(), 1);
        QCOMPARE(o.bridge.fatal(), "the list is out of step with review");
        QCOMPARE(o.bridge.findings()->rowCount(), 2);
    }
};

QTEST_GUILESS_MAIN(SessionBridgeTest)
#include "session_bridge_test.moc"
