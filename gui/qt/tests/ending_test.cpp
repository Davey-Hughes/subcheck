// How the window ends once review has, on a fake mpv that answers as mpv does, with no window: by
// the event loop's exit once mpv has ended its file, as it does at once with one loaded or none;
// with the settings kept and the windows hidden first when that takes longer than the settle time;
// and with the process ended hard when it does not happen in the whole time.
#include "ending.h"
#include "fake_mpv.h"
#include "player_port.h"

#include <QElapsedTimer>
#include <QList>
#include <QString>
#include <QStringList>
#include <QTest>

#include <chrono>

namespace {

// A player on a fake mpv that can show video and holds `holds`, let go as review's end leaves it;
// what endWindow had done, in order, each as a line, and when, in ms from the call.
struct Rig {
    PlayerPort player{std::chrono::seconds(10)};
    FakeMpv mpv;
    QStringList did;
    QList<qint64> at;
    QElapsedTimer since;

    explicit Rig(FakeMpv::Holds holds) {
        mpv.holds = holds;
        player.attach(&mpv);
        player.ready();
        player.release();
    }

    void end(std::chrono::milliseconds settle, std::chrono::milliseconds within) {
        since.start();
        endWindow(
            player, settle, within, 3, [this] { note(QStringLiteral("keep settings")); },
            [this] { note(QStringLiteral("hide")); },
            [this](int status) { note(QStringLiteral("exit %1").arg(status)); },
            [this](int status) { note(QStringLiteral("hard exit %1").arg(status)); });
    }

    void note(const QString &what) {
        did << what;
        at << since.elapsed();
    }
};

} // namespace

class EndingTest : public QObject {
    Q_OBJECT

private slots:
    // No file: the window ends at once, by the event loop's exit, whose teardown keeps the settings
    // itself, with nothing hidden first.
    void withNoFileTheEventLoopExitsAtOnce() {
        Rig r(FakeMpv::Holds::Nothing);
        r.end(std::chrono::seconds(10), std::chrono::seconds(20));
        QCOMPARE(r.mpv.last(), "get path");
        QCOMPARE(r.did, QStringList());
        r.mpv.serve(r.player);
        QCOMPARE(r.did, QStringList{"exit 3"});
    }

    // A file mpv ends within the settle time, as it does one loaded, straight after the stop: the
    // window ends by the event loop's exit, with nothing hidden first.
    void aFileEndedWithinTheSettleTimeExitsTheEventLoop() {
        Rig r(FakeMpv::Holds::Loaded);
        r.end(std::chrono::seconds(10), std::chrono::seconds(20));
        r.mpv.serve(r.player);
        QCOMPARE(r.did, QStringList{"exit 3"});
    }

    // A file mpv ends after the settle time but within the whole time, as one whose teardown is
    // slow or whose share answers late: once the settle time has passed, the settings are kept and
    // the windows hidden, and once mpv has ended the file, the window ends by the event loop's
    // exit, with no hard exit, then or at the whole time.
    void aFileEndedAfterTheSettleTimeHidesThenExitsTheEventLoop() {
        Rig r(FakeMpv::Holds::Stuck);
        r.end(std::chrono::milliseconds(100), std::chrono::milliseconds(1000));
        QVERIFY(
            r.mpv.serveUntil(r.player, [&r] { return !r.did.isEmpty(); }, std::chrono::seconds(2)));
        QCOMPARE(r.did, (QStringList{"keep settings", "hide"}));
        QVERIFY2(r.at.first() >= 100, qPrintable(QString::number(r.at.first())));
        r.mpv.serve(r.player);
        QCOMPARE(r.mpv.last(), "get path");
        QTest::qWait(100);
        QCOMPARE(r.did.size(), 2);
        r.mpv.shareAnswers(r.player);
        QCOMPARE(r.did, (QStringList{"keep settings", "hide", "exit 3"}));
        QVERIFY2(r.at.last() < 1000, qPrintable(QString::number(r.at.last())));
        QTest::qWait(1100);
        QCOMPARE(r.did.size(), 3);
    }

    // A file whose open waits on a share that does not answer for the whole time, though mpv's idle
    // flag says idle: once the settle time has passed, the settings are kept first, as no teardown
    // will keep them, then the windows are hidden, and once the whole time has passed since the
    // call, the process ends, with review's status. The file's late end changes nothing.
    void aFileNeverEndedKeepsHidesThenEndsTheProcess() {
        Rig r(FakeMpv::Holds::Stuck);
        r.end(std::chrono::milliseconds(100), std::chrono::milliseconds(500));
        QVERIFY(
            r.mpv.serveUntil(r.player, [&r] { return !r.did.isEmpty(); }, std::chrono::seconds(2)));
        QCOMPARE(r.did, (QStringList{"keep settings", "hide"}));
        QVERIFY2(r.at.first() >= 100, qPrintable(QString::number(r.at.first())));
        QVERIFY(r.mpv.serveUntil(
            r.player, [&r] { return r.did.size() == 3; }, std::chrono::seconds(2)));
        QCOMPARE(r.did, (QStringList{"keep settings", "hide", "hard exit 3"}));
        QVERIFY2(r.at.last() >= 500, qPrintable(QString::number(r.at.last())));
        r.mpv.shareAnswers(r.player);
        QTest::qWait(100);
        QCOMPARE(r.did.size(), 3);
    }
};

QTEST_GUILESS_MAIN(EndingTest)
#include "ending_test.moc"
