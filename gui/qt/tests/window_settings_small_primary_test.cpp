// What the window remembers when the screen it comes back to is larger than the primary one, as a
// laptop's is with a larger monitor beside it. A program of its own, as CTest gives a program one
// set of screens and window_settings_test needs its large screen first.
#include "offscreen_main.h"
#include "window_settings.h"

#include <QGuiApplication>
#include <QQuickWindow>
#include <QScreen>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>

#include <chrono>
#include <memory>

namespace {

std::unique_ptr<QSettings> in(const QTemporaryDir &dir) {
    return std::make_unique<QSettings>(dir.filePath("subcheck-qt.conf"), QSettings::IniFormat);
}

} // namespace

class WindowSettingsSmallPrimaryTest : public QObject {
    Q_OBJECT

private slots:
    void initTestCase() {
        QVERIFY2(QGuiApplication::screens().size() == 2 &&
                     QGuiApplication::primaryScreen()->availableSize() == QSize(1024, 600),
                 "run through ctest, which gives this test a small screen and a large one");
    }

    // Bounded by the primary screen, the window would open small and keep that as its own size at
    // its next move, and the size the person gave it would be lost.
    void aWindowBackOnALargerScreenKeepsItsSize() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        in(dir)->setValue("window/size", QSize(1500, 900));
        in(dir)->setValue("window/place", QPoint(1100, 10));
        {
            WindowSettings settings(in(dir), "offscreen", std::chrono::hours(1));
            QQuickWindow window;
            settings.show(&window);
            QCOMPARE(window.screen()->name(), "b-large");
            QCOMPARE(window.size(), QSize(1500, 900));
            QSignalSpy across(&window, &QWindow::xChanged);
            window.setPosition(1110, 10);
            QTRY_VERIFY(!across.isEmpty() && across.last()[0].toInt() == 1110);
        }
        const auto kept = in(dir);
        QCOMPARE(kept->value("window/size").toSize(), QSize(1500, 900));
        // The frame's place: the offscreen platform's frame reaches 2 above and left of the window.
        QCOMPARE(kept->value("window/place").toPoint(), QPoint(1108, 8));
    }

    // The screen it comes back to is the one it moves inside, and not the primary one it starts on.
    void aWindowBackOnALargerScreenComesBackInsideIt() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        in(dir)->setValue("window/size", QSize(1500, 900));
        in(dir)->setValue("window/place", QPoint(2500, 500));
        WindowSettings settings(in(dir), "offscreen", std::chrono::hours(1));
        QQuickWindow window;
        settings.show(&window);
        QCOMPARE(window.screen()->name(), "b-large");
        QCOMPARE(window.size(), QSize(1500, 900));
        QCOMPARE(window.framePosition(), QPoint(1444, 180));
    }

    // Left across the two screens, it is inside the desktop, and stays where it was left.
    void aWindowAcrossBothScreensStaysWhereItWasLeft() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        in(dir)->setValue("window/size", QSize(600, 400));
        in(dir)->setValue("window/place", QPoint(800, 100));
        WindowSettings settings(in(dir), "offscreen", std::chrono::hours(1));
        QQuickWindow window;
        settings.show(&window);
        QCOMPARE(window.size(), QSize(600, 400));
        QCOMPARE(window.framePosition(), QPoint(800, 100));
    }
};

SUBCHECK_OFFSCREEN_TEST_MAIN(WindowSettingsSmallPrimaryTest)
#include "window_settings_small_primary_test.moc"
