// Showing a file in the file manager, with no file manager asked: the folder opened only once the
// file manager says it did not show the file, whenever it says so; and the D-Bus call, on a bus of
// the test's own that nothing listens on, never a desktop's.
#include "reveal.h"

#include <QElapsedTimer>
#include <QScopeGuard>
#include <QString>
#include <QStringList>
#include <QTemporaryDir>
#include <QTest>
#include <QTimer>

#include <chrono>
#include <functional>

class RevealTest : public QObject {
    Q_OBJECT

private slots:
    void aFileTheFileManagerShowsOpensNoFolder() {
        QStringList asked;
        QStringList opened;
        const Reveal how{[&asked](const QString &file, std::function<void(bool shown)> done) {
                             asked << file;
                             done(true);
                         },
                         [&opened](const QString &folder) { opened << folder; }};
        revealFile(QStringLiteral("/m/S1/E03.mkv"), how);
        QCOMPARE(asked, QStringList{"/m/S1/E03.mkv"});
        QCOMPARE(opened, QStringList());
    }

    // Said at once, and said later, once the Reveal it was asked through has gone, as the file
    // manager's answer comes after the click that asked: the file's folder opens, once.
    void aFileTheFileManagerCannotShowOpensItsFolder() {
        QStringList opened;
        {
            const Reveal how{
                [](const QString &, std::function<void(bool shown)> done) { done(false); },
                [&opened](const QString &folder) { opened << folder; }};
            revealFile(QStringLiteral("/m/S1/E03.mkv"), how);
        }
        QCOMPARE(opened, QStringList{"/m/S1"});

        opened.clear();
        {
            const Reveal how{[](const QString &, std::function<void(bool shown)> done) {
                                 QTimer::singleShot(std::chrono::milliseconds(10),
                                                    [done] { done(false); });
                             },
                             [&opened](const QString &folder) { opened << folder; }};
            revealFile(QStringLiteral("/m/S1/E03.mkv"), how);
            QCOMPARE(opened, QStringList());
        }
        QTRY_COMPARE(opened, QStringList{"/m/S1"});
        QTest::qWait(100);
        QCOMPARE(opened, QStringList{"/m/S1"});
    }

    // A path with a space, a '#' and letters beyond ASCII goes to the file manager percent-encoded,
    // as a URI holds them.
    void theFileManagerIsGivenAnEncodedUri() {
#ifdef Q_OS_MACOS
        QSKIP("the Finder is given a path by open -R, not a URI");
#else
        QCOMPARE(fileUri(QStringLiteral("/m/My Show #1/字幕.mkv")),
                 QStringLiteral("file:///m/My%20Show%20%231/%E5%AD%97%E5%B9%95.mkv"));
#endif
    }

    // On a bus that is not connected, as where no session bus runs: no file manager is asked, and
    // the answer is at once that the file was not shown, and never a second one, not even past the
    // wait an asked file manager has.
    void withNoBusTheFileManagerIsNotAsked() {
#ifdef Q_OS_MACOS
        QSKIP("the Finder is asked by open -R, not over D-Bus");
#else
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        const QString name = QStringLiteral("reveal-test");
        const auto letGo = qScopeGuard([&name] { QDBusConnection::disconnectFromBus(name); });
        const QDBusConnection bus = QDBusConnection::connectToBus(
            QStringLiteral("unix:path=") + dir.filePath(QStringLiteral("bus")), name);
        QVERIFY(!bus.isConnected());
        int answers = 0;
        bool shown = true;
        QElapsedTimer since;
        since.start();
        showItems(
            QStringLiteral("/m/S1/E03.mkv"),
            [&answers, &shown](bool said) {
                ++answers;
                shown = said;
            },
            bus);
        QTRY_COMPARE_WITH_TIMEOUT(answers, 1, 1000);
        QVERIFY2(since.elapsed() < 1000, qPrintable(QString::number(since.elapsed())));
        QVERIFY(!shown);
        QTest::qWait(6000);
        QCOMPARE(answers, 1);
#endif
    }
};

QTEST_GUILESS_MAIN(RevealTest)
#include "reveal_test.moc"
