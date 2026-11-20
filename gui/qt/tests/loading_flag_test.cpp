// When the busy animation over the video shows and goes, on moments of the test's own, shorter
// than the window's: only once a load has lasted its moment, and only once it has been over for
// its moment, so that neither a quick load nor a gap between two loads blinks.
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

    // A load that ends and another that starts before the hide moment is up: one animation, never
    // hidden between.
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
