// When the busy animation over the video shows and goes, on moments of the test's own: only once a
// load has lasted its moment, and only once it has been over for its moment, so that neither a
// quick load nor a gap between two loads blinks.
#include "loading_flag.h"

#include <QElapsedTimer>
#include <QSignalSpy>
#include <QTest>

#include <chrono>

using namespace std::chrono_literals;

namespace {

// Far past either moment, for a wait in which nothing is to change.
constexpr int pastBoth = 300;

} // namespace

class LoadingFlagTest : public QObject {
    Q_OBJECT

private slots:
    // Over before its moment: nothing shows, then or after.
    void aShortLoadShowsNothing() {
        LoadingFlag flag(50ms, 30ms);
        QSignalSpy changed(&flag, &LoadingFlag::shownChanged);
        flag.set(true);
        flag.set(false);
        QTest::qWait(pastBoth);
        QVERIFY(!flag.shown());
        QCOMPARE(changed.count(), 0);
    }

    // Shown once the load has lasted its moment, and not before, however long a wait takes. The
    // clock starts before the load, so that the time it gives is never short of the flag's own.
    void aLongLoadShowsAfterItsMoment() {
        LoadingFlag flag(50ms, 30ms);
        QSignalSpy changed(&flag, &LoadingFlag::shownChanged);
        QElapsedTimer clock;
        qint64 shownAt = -1;
        connect(&flag, &LoadingFlag::shownChanged, this, [&] { shownAt = clock.elapsed(); });
        clock.start();
        flag.set(true);
        QVERIFY(!flag.shown());
        QTRY_VERIFY(flag.shown());
        QVERIFY2(shownAt >= 50, qPrintable(QString::number(shownAt)));
        QCOMPARE(changed.count(), 1);
        // Loading still: no more changes.
        flag.set(true);
        QTest::qWait(pastBoth);
        QVERIFY(flag.shown());
        QCOMPARE(changed.count(), 1);
    }

    // A load that stops before its moment is up ends that wait: one that starts after, however
    // soon, shows only once its own moment is up, not on the first's time. The moment is far longer
    // than the wait before the stop, so that nothing has shown by then; the stop's time is taken
    // before the stop, so that the time it gives is never short of the flag's own.
    void aStopBeforeItsMomentEndsTheWait() {
        LoadingFlag flag(1000ms, 30ms);
        QSignalSpy changed(&flag, &LoadingFlag::shownChanged);
        QElapsedTimer clock;
        qint64 shownAt = -1;
        connect(&flag, &LoadingFlag::shownChanged, this, [&] { shownAt = clock.elapsed(); });
        clock.start();
        flag.set(true);
        QTest::qWait(200);
        const qint64 stoppedAt = clock.elapsed();
        QVERIFY2(!flag.shown(), qPrintable(QString("shown at %1 ms").arg(stoppedAt)));
        flag.set(false);
        flag.set(true);
        QTRY_VERIFY(flag.shown());
        QVERIFY2(shownAt - stoppedAt >= 1000,
                 qPrintable(QString("shown %1 ms, stopped %2 ms").arg(shownAt).arg(stoppedAt)));
        QCOMPARE(changed.count(), 1);
    }

    // A load that stops before its moment, and one that starts after and stops before its own, show
    // nothing: the second is timed from its own start, not the first's, which its stop would have
    // had up by the time the second ends. The moment is far longer than each wait, and the second
    // load's end is past the first's start by more than the moment, so that only the first's time
    // carried over would show.
    void twoShortLoadsInARowShowNothing() {
        LoadingFlag flag(1000ms, 30ms);
        QSignalSpy changed(&flag, &LoadingFlag::shownChanged);
        flag.set(true);
        QTest::qWait(400);
        flag.set(false);
        QTest::qWait(400);
        flag.set(true);
        QTest::qWait(400);
        flag.set(false);
        QTest::qWait(1200);
        QVERIFY(!flag.shown());
        QCOMPARE(changed.count(), 0);
    }

    // Once shown, a load that ends and another that starts before the hide moment is up: one
    // animation, never hidden between.
    void aGapShorterThanTheHideMomentKeepsItShown() {
        LoadingFlag flag(50ms, 30ms);
        QSignalSpy changed(&flag, &LoadingFlag::shownChanged);
        flag.set(true);
        QTRY_VERIFY(flag.shown());
        flag.set(false);
        flag.set(true);
        QTest::qWait(pastBoth);
        QVERIFY(flag.shown());
        QCOMPARE(changed.count(), 1);
    }

    // Gone once the load has been over for its moment, and not before.
    void itGoesOnceTheLoadHasBeenOverForItsMoment() {
        LoadingFlag flag(50ms, 30ms);
        QSignalSpy changed(&flag, &LoadingFlag::shownChanged);
        flag.set(true);
        QTRY_VERIFY(flag.shown());
        QElapsedTimer clock;
        qint64 hiddenAt = -1;
        connect(&flag, &LoadingFlag::shownChanged, this, [&] { hiddenAt = clock.elapsed(); });
        clock.start();
        flag.set(false);
        QVERIFY(flag.shown());
        QTRY_VERIFY(!flag.shown());
        QVERIFY2(hiddenAt >= 30, qPrintable(QString::number(hiddenAt)));
        QCOMPARE(changed.count(), 2);
        // A load that starts after, as the first did, waits its own moment again.
        flag.set(true);
        QVERIFY(!flag.shown());
        QTRY_VERIFY(flag.shown());
        QCOMPARE(changed.count(), 3);
    }
};

QTEST_GUILESS_MAIN(LoadingFlagTest)
#include "loading_flag_test.moc"
