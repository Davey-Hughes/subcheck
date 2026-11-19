// What the window remembers, on a window of the test's own and a settings file in a directory the
// test makes and removes: never the user's, and never Qt's test mode, whose files go under the
// user's home. The app's own file is looked for under an XDG_CONFIG_HOME the test sets. CTest gives
// it two screens, a large one first and a small one beside it, so that the default size is the
// window's and not the offscreen screen's.
#include "offscreen_main.h"
#include "window_settings.h"
#include "words.h"

#include <QFile>
#include <QFont>
#include <QFontInfo>
#include <QFontMetricsF>
#include <QGuiApplication>
#include <QQuickWindow>
#include <QScopeGuard>
#include <QScreen>
#include <QSignalBlocker>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <QVariantMap>

#include <chrono>
#include <cmath>
#include <memory>
#include <optional>

namespace {

// How far the offscreen platform's frame reaches above and to the left of a window.
const QPoint frame(2, 2);

std::unique_ptr<QSettings> in(const QTemporaryDir &dir) {
    return std::make_unique<QSettings>(dir.filePath("subcheck-qt.conf"), QSettings::IniFormat);
}

// `window` at `size` and `place`, each change waited for until the window has said it, as the
// settings keep what it says.
bool move(QQuickWindow &window, QSize size, QPoint place) {
    QSignalSpy wide(&window, &QWindow::widthChanged);
    QSignalSpy high(&window, &QWindow::heightChanged);
    QSignalSpy across(&window, &QWindow::xChanged);
    QSignalSpy down(&window, &QWindow::yChanged);
    window.resize(size);
    window.setPosition(place);
    return QTest::qWaitFor([&] {
        return !wide.isEmpty() && wide.last()[0].toInt() == size.width() && !high.isEmpty() &&
               high.last()[0].toInt() == size.height() && !across.isEmpty() &&
               across.last()[0].toInt() == place.x() && !down.isEmpty() &&
               down.last()[0].toInt() == place.y();
    });
}

// Has `window` say where it is, as any move of it does, and leaves it where it was: a run keeps
// what its window last said.
bool sayWhereItIs(QQuickWindow &window) {
    const int x = window.x();
    QSignalSpy across(&window, &QWindow::xChanged);
    const auto goTo = [&](int to) {
        window.setX(to);
        return QTest::qWaitFor([&] { return !across.isEmpty() && across.last()[0].toInt() == to; });
    };
    return goTo(x + 1) && goTo(x);
}

// `settings` showing `window`, which has said nothing on the way: nothing is kept until the test
// has the window say something.
bool showUnheard(WindowSettings &settings, QQuickWindow &window) {
    const QSignalBlocker unheard(window);
    settings.show(&window);
    return QTest::qWaitForWindowExposed(&window);
}

} // namespace

class WindowSettingsTest : public QObject {
    Q_OBJECT

private slots:
    void initTestCase() {
        QVERIFY2(QGuiApplication::screens().size() == 2 &&
                     QGuiApplication::primaryScreen()->availableSize() == QSize(1920, 1080),
                 "run through ctest, which gives this test a large screen and a small one");
    }

    void eachPlatformButWaylandLetsAWindowPlaceItself() {
        QVERIFY(!WindowSettings::places("wayland"));
        QVERIFY(!WindowSettings::places("wayland-egl"));
        QVERIFY(WindowSettings::places("xcb"));
        QVERIFY(WindowSettings::places("cocoa"));
        QVERIFY(WindowSettings::places("offscreen"));
    }

    // With nothing kept yet: the default size, and the columns' default widths.
    void aFirstRunShowsTheDefaults() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        WindowSettings settings(in(dir), "offscreen");
        QQuickWindow window;
        settings.show(&window);
        QVERIFY(QTest::qWaitForWindowExposed(&window));
        QCOMPARE(window.size(), QSize(1280, 800));
        QCOMPARE(settings.sidebarWidth(), 220);
        QCOMPARE(settings.detailsWidth(), 480);
    }

    // A window whose least size allows it is no larger than its screen's free area.
    void aWindowIsNoLargerThanItsScreen() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        WindowSettings settings(in(dir), "offscreen");
        QQuickWindow window;
        QScreen *small = QGuiApplication::screens().value(1);
        QVERIFY(small);
        window.setScreen(small);
        settings.show(&window);
        QCOMPARE(window.size(), QSize(1024, 600));
    }

    // The screen a window comes back to bounds it, and not the primary one it starts on.
    void aWindowBackOnASmallerScreenIsNoLargerThanIt() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        in(dir)->setValue("window/size", QSize(1900, 1000));
        in(dir)->setValue("window/place", QPoint(1950, 10));
        WindowSettings settings(in(dir), "offscreen");
        QQuickWindow window;
        settings.show(&window);
        QCOMPARE(window.screen()->name(), "small");
        QCOMPARE(window.size(), QSize(1024, 600));
    }

    // The place kept is the frame's. A window part of which would be off the desktop moves inside
    // the screen holding its corner, or with no screen holding that, the screen showing the most
    // of it, no further than it has to; one the screen cannot hold keeps its frame's top left
    // corner in view. One inside the desktop stays, across two screens too.
    void aWindowComesBackInsideItsScreen_data() {
        QTest::addColumn<QSize>("kept");
        QTest::addColumn<QPoint>("place");
        QTest::addColumn<QSize>("least");
        QTest::addColumn<QString>("screen");
        QTest::addColumn<QSize>("size");
        QTest::addColumn<QPoint>("at");
        QTest::newRow("past the large screen's right and bottom")
            << QSize(900, 800) << QPoint(1500, 700) << QSize() << "large" << QSize(900, 800)
            << QPoint(1020, 280);
        QTest::newRow("past the small screen's right and bottom")
            << QSize(900, 500) << QPoint(2500, 300) << QSize() << "small" << QSize(900, 500)
            << QPoint(2044, 100);
        QTest::newRow("past the small screen's right alone")
            << QSize(900, 500) << QPoint(2500, 50) << QSize() << "small" << QSize(900, 500)
            << QPoint(2044, 50);
        QTest::newRow("a size the screen bounds")
            << QSize(1900, 400) << QPoint(1950, 100) << QSize() << "small" << QSize(1024, 400)
            << QPoint(1920, 100);
        QTest::newRow("a least size larger than the screen")
            << QSize(900, 500) << QPoint(2500, 300) << QSize(1100, 700) << "small"
            << QSize(1100, 700) << QPoint(1920, 0);
        QTest::newRow("across both screens and below the small one")
            << QSize(900, 400) << QPoint(1300, 400) << QSize() << "large" << QSize(900, 400)
            << QPoint(1020, 400);
        QTest::newRow("a corner just left of the desktop")
            << QSize(700, 500) << QPoint(-4, 200) << QSize() << "large" << QSize(700, 500)
            << QPoint(0, 200);
        QTest::newRow("a corner just above the small screen")
            << QSize(700, 500) << QPoint(2000, -4) << QSize() << "small" << QSize(700, 500)
            << QPoint(2000, 0);
        QTest::newRow("a corner off the desktop, the size bounded by the screen it comes back to")
            << QSize(1900, 400) << QPoint(2000, -4) << QSize() << "small" << QSize(1024, 400)
            << QPoint(1920, 0);
        QTest::newRow("a corner off the desktop, most of the window over the small screen")
            << QSize(100, 300) << QPoint(1900, -4) << QSize() << "small" << QSize(100, 300)
            << QPoint(1920, 0);
        QTest::newRow("a corner off the desktop, most of the window over the large screen")
            << QSize(200, 300) << QPoint(1800, -4) << QSize() << "large" << QSize(200, 300)
            << QPoint(1720, 0);
        QTest::newRow("inside its screen") << QSize(700, 500) << QPoint(2000, 50) << QSize()
                                           << "small" << QSize(700, 500) << QPoint(2000, 50);
        QTest::newRow("across both screens") << QSize(900, 400) << QPoint(1300, 100) << QSize()
                                             << "large" << QSize(900, 400) << QPoint(1300, 100);
    }

    void aWindowComesBackInsideItsScreen() {
        QFETCH(QSize, kept);
        QFETCH(QPoint, place);
        QFETCH(QSize, least);
        QFETCH(QString, screen);
        QFETCH(QSize, size);
        QFETCH(QPoint, at);
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        in(dir)->setValue("window/size", kept);
        in(dir)->setValue("window/place", place);
        WindowSettings settings(in(dir), "offscreen");
        QQuickWindow window;
        if (least.isValid()) {
            window.setMinimumSize(least);
        }
        settings.show(&window);
        QCOMPARE(window.screen()->name(), screen);
        QCOMPARE(window.size(), size);
        QCOMPARE(window.framePosition(), at);
    }

    // A frame kept under a panel along the top of the screen moves down to the free area, and stays
    // once it is there. Made up, as the offscreen platform's screens are free to their edges.
    void aFrameAboveTheFreeAreaMovesInsideIt() {
        const QRect room(0, 30, 1920, 1050);
        const QRegion free(room);
        QCOMPARE(WindowSettings::placeBack(QPoint(100, 10), QSize(800, 600), room, free),
                 QPoint(100, 30));
        QCOMPARE(WindowSettings::placeBack(QPoint(100, 30), QSize(800, 600), room, free),
                 QPoint(100, 30));
        // Taller than the free area, it keeps its top in view.
        QCOMPARE(WindowSettings::placeBack(QPoint(100, 10), QSize(800, 1200), room, free),
                 QPoint(100, 30));
    }

    // Each run starts from the place the run before it kept, and the frame comes back to one place
    // however many times the window starts.
    void aWindowsFrameComesBackToOnePlaceAtEveryStart_data() {
        QTest::addColumn<QSize>("kept");
        QTest::addColumn<QPoint>("place");
        QTest::addColumn<QPoint>("at");
        QTest::newRow("a window that stands")
            << QSize(700, 500) << QPoint(2000, 50) << QPoint(2000, 50);
        QTest::newRow("a window moved back onto its screen")
            << QSize(900, 500) << QPoint(2500, 50) << QPoint(2044, 50);
        QTest::newRow("a window across two screens")
            << QSize(900, 400) << QPoint(1300, 100) << QPoint(1300, 100);
        QTest::newRow("a window whose corner was just off the desktop")
            << QSize(700, 500) << QPoint(-4, 200) << QPoint(0, 200);
    }

    void aWindowsFrameComesBackToOnePlaceAtEveryStart() {
        QFETCH(QSize, kept);
        QFETCH(QPoint, place);
        QFETCH(QPoint, at);
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        in(dir)->setValue("window/size", kept);
        in(dir)->setValue("window/place", place);
        std::optional<QPoint> first;
        for (int start = 0; start < 4; ++start) {
            WindowSettings settings(in(dir), "offscreen", std::chrono::hours(1));
            QQuickWindow window;
            settings.show(&window);
            const QPoint now = window.framePosition();
            QCOMPARE(now, first.value_or(now));
            first = now;
            QVERIFY(sayWhereItIs(window));
        }
        QCOMPARE(*first, at);
    }

    // What the window's size and place come to, and the columns' widths, the next run starts
    // with; one under the window's least size is raised to it.
    void theSizePlaceAndColumnsStickAcrossARestart() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        {
            WindowSettings settings(in(dir), "offscreen");
            QQuickWindow window;
            settings.show(&window);
            QVERIFY(move(window, QSize(700, 500), QPoint(40, 30)));
            QSignalSpy columns(&settings, &WindowSettings::columnsChanged);
            settings.keepColumns(180, 410);
            QCOMPARE(columns.count(), 1);
        }
        WindowSettings settings(in(dir), "offscreen");
        QQuickWindow window;
        settings.show(&window);
        QCOMPARE(window.size(), QSize(700, 500));
        QCOMPARE(window.position(), QPoint(40, 30));
        QCOMPARE(settings.sidebarWidth(), 180);
        QCOMPARE(settings.detailsWidth(), 410);

        WindowSettings again(in(dir), "offscreen");
        QQuickWindow small;
        small.setMinimumSize(QSize(750, 520));
        again.show(&small);
        QCOMPARE(small.size(), QSize(750, 520));
    }

    // A window moved and not resized has its frame's place kept where it was moved to.
    void aWindowMovedAloneKeepsItsPlace() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        {
            WindowSettings settings(in(dir), "offscreen");
            QQuickWindow window;
            settings.show(&window);
            QVERIFY(move(window, QSize(700, 500), QPoint(40, 30)));
            settings.keep();
            QSignalSpy across(&window, &QWindow::xChanged);
            QSignalSpy down(&window, &QWindow::yChanged);
            window.setPosition(90, 80);
            QTRY_VERIFY(!across.isEmpty() && across.last()[0].toInt() == 90 && !down.isEmpty() &&
                        down.last()[0].toInt() == 80);
        }
        QCOMPARE(in(dir)->value("window/place").toPoint(), QPoint(90, 80) - frame);
    }

    // Where the platform does not let a window place itself, the place is neither kept nor used.
    void onWaylandOnlyTheSizeSticks() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        {
            WindowSettings placed(in(dir), "offscreen");
            QQuickWindow window;
            placed.show(&window);
            QVERIFY(move(window, QSize(600, 500), QPoint(40, 30)));
        }
        {
            WindowSettings unplaced(in(dir), "wayland-egl");
            QQuickWindow window;
            window.setPosition(5, 6);
            unplaced.show(&window);
            QCOMPARE(window.position(), QPoint(5, 6));
            QVERIFY(move(window, QSize(640, 480), QPoint(90, 80)));
        }
        WindowSettings placed(in(dir), "offscreen");
        QQuickWindow window;
        placed.show(&window);
        QCOMPARE(window.size(), QSize(640, 480));
        QCOMPARE(window.position(), QPoint(40, 30));
    }

    // A place where no screen would show any of the window, as on a monitor since unplugged, is
    // not used.
    void aPlaceOffEveryScreenIsNotUsed_data() {
        QTest::addColumn<QPoint>("place");
        QTest::newRow("far off the desktop") << QPoint(-30000, -30000);
        QTest::newRow("against the large screen's left edge") << QPoint(-700, 200);
        QTest::newRow("in the gap below the small screen") << QPoint(2000, 700);
    }

    void aPlaceOffEveryScreenIsNotUsed() {
        QFETCH(QPoint, place);
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        in(dir)->setValue("window/size", QSize(700, 500));
        in(dir)->setValue("window/place", place);
        WindowSettings settings(in(dir), "offscreen");
        QQuickWindow window;
        window.setPosition(7, 8);
        settings.show(&window);
        QCOMPARE(window.position(), QPoint(7, 8));
    }

    // A window left maximized comes back maximized, and restored it has the size it had before.
    void aMaximizedWindowComesBackMaximizedAndRestoresToItsOwnSize() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        {
            WindowSettings settings(in(dir), "offscreen");
            QQuickWindow window;
            settings.show(&window);
            QVERIFY(move(window, QSize(700, 500), QPoint(40, 30)));
            settings.keep();
            window.showMaximized();
            // The screen, less what the platform keeps for a frame.
            QTRY_VERIFY(window.width() > 1900 && window.height() > 1000);
            QCOMPARE(window.visibility(), QWindow::Maximized);
        }
        WindowSettings settings(in(dir), "offscreen");
        QQuickWindow window;
        settings.show(&window);
        QTRY_COMPARE(window.visibility(), QWindow::Maximized);
        window.showNormal();
        QTRY_COMPARE(window.size(), QSize(700, 500));
    }

    // Full screen is neither the window's own size nor maximized: a window left that way comes
    // back as it was before, at the size and place it had.
    void aFullScreenWindowKeepsItsOwnSizeAndComesBackWindowed() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        {
            WindowSettings settings(in(dir), "offscreen", std::chrono::hours(1));
            QQuickWindow window;
            settings.show(&window);
            QVERIFY(move(window, QSize(700, 500), QPoint(40, 30)));
            settings.keep();
            window.showFullScreen();
            QTRY_COMPARE(window.size(), QSize(1920, 1080));
            QCOMPARE(window.visibility(), QWindow::FullScreen);
        }
        {
            const auto kept = in(dir);
            QCOMPARE(kept->value("window/size").toSize(), QSize(700, 500));
            QCOMPARE(kept->value("window/place").toPoint(), QPoint(40, 30) - frame);
            QCOMPARE(kept->value("window/maximized").toBool(), false);
        }
        WindowSettings settings(in(dir), "offscreen");
        QQuickWindow window;
        settings.show(&window);
        QVERIFY(QTest::qWaitForWindowExposed(&window));
        QCOMPARE(window.visibility(), QWindow::Windowed);
        QCOMPARE(window.size(), QSize(700, 500));
    }

    // Whichever a platform says first as it maximizes a window, the state or the new size, the
    // size kept is the one the window had before. The settling is the test's to end, so that a slow
    // machine cannot end it between the two.
    void aMaximizedWindowsOwnSizeIsKeptWhicheverThePlatformSaysFirst_data() {
        QTest::addColumn<bool>("sizeFirst");
        QTest::newRow("the size first") << true;
        QTest::newRow("the state first") << false;
    }

    void aMaximizedWindowsOwnSizeIsKeptWhicheverThePlatformSaysFirst() {
        QFETCH(bool, sizeFirst);
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        {
            WindowSettings settings(in(dir), "offscreen", std::chrono::hours(1));
            QQuickWindow window;
            settings.show(&window);
            QVERIFY(move(window, QSize(700, 500), QPoint(40, 30)));
            settings.keep();
            if (sizeFirst) {
                QVERIFY(move(window, QSize(1900, 1000), QPoint(10, 10)));
                QVERIFY(!(window.windowStates() & Qt::WindowMaximized));
            }
            window.setWindowStates(Qt::WindowMaximized);
            // The screen, less what the platform keeps for a frame.
            QTRY_VERIFY(window.width() > 1900 && window.height() > 1000);
            settings.keep();
        }
        const auto kept = in(dir);
        QCOMPARE(kept->value("window/size").toSize(), QSize(700, 500));
        QCOMPARE(kept->value("window/place").toPoint(), QPoint(40, 30) - frame);
        QVERIFY(kept->value("window/maximized").toBool());
    }

    // What the window says reaches the settings once it has settled, with the window still open.
    void whatTheWindowSaysIsKeptOnceItSettles() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        WindowSettings settings(in(dir), "offscreen", std::chrono::milliseconds(1));
        QQuickWindow window;
        settings.show(&window);
        QVERIFY(move(window, QSize(700, 500), QPoint(40, 30)));
        QTRY_COMPARE(in(dir)->value("window/size").toSize(), QSize(700, 500));
    }

    // A window manager can report the frame's margins after the window's last move, and a change of
    // margins says nothing. The offscreen platform's margins do not change alone, so a move the
    // settings cannot hear stands in for them: the window reports a new place and says nothing.
    // No event runs between what the window says and that move, so the settling ends after both.
    void thePlaceKeptIsTheOneTheWindowReportsWhenItSettles() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        WindowSettings settings(in(dir), "offscreen", std::chrono::milliseconds(1));
        QQuickWindow window;
        QVERIFY(showUnheard(settings, window));
        emit window.xChanged(window.x());
        const QSignalBlocker unheard(window);
        window.setPosition(90, 80);
        QTRY_VERIFY(in(dir)->contains("window/place"));
        QCOMPARE(in(dir)->value("window/place").toPoint(), QPoint(90, 80) - frame);
    }

    // A closed window has no frame left to report, only the corner of what was inside it: the place
    // kept is the frame's, from when the window last said where it was.
    void aWindowClosedBeforeItSettlesKeepsItsFramesPlace() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        WindowSettings settings(in(dir), "offscreen", std::chrono::milliseconds(1));
        QQuickWindow window;
        QVERIFY(showUnheard(settings, window));
        const QPoint place = window.framePosition();
        emit window.xChanged(window.x());
        window.close();
        QCOMPARE_NE(window.framePosition(), place);
        QTRY_VERIFY(in(dir)->contains("window/place"));
        QCOMPARE(in(dir)->value("window/place").toPoint(), place);
    }

    // The settling can outlast the window: what it last said is still kept.
    void aWindowGoneBeforeItSettlesKeepsItsFramesPlace() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        WindowSettings settings(in(dir), "offscreen", std::chrono::milliseconds(1));
        auto window = std::make_unique<QQuickWindow>();
        QVERIFY(showUnheard(settings, *window));
        const QPoint place = window->framePosition();
        emit window->xChanged(window->x());
        window.reset();
        QTRY_VERIFY(in(dir)->contains("window/place"));
        QCOMPARE(in(dir)->value("window/place").toPoint(), place);
    }

    // The palette is kept by its name and drawn with from the start: the settings use the kept one
    // as they are made, a choice at once, and a name no palette has as the default.
    void thePaletteIsKeptByNameAndDrawnWithFromTheStart() {
        const auto restore = qScopeGuard([] { useColours(Colours::Default); });
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        in(dir)->setValue("look/colours", "protan");
        WindowSettings settings(in(dir), "offscreen");
        QCOMPARE(settings.colours(), "protan");
        QCOMPARE(colours(), Colours::Protan);
        QSignalSpy changed(&settings, &WindowSettings::coloursChanged);
        settings.setColours("tritan");
        QCOMPARE(changed.count(), 1);
        QCOMPARE(colours(), Colours::Tritan);
        QCOMPARE(in(dir)->value("look/colours").toString(), "tritan");
        // The same choice again changes nothing.
        settings.setColours("tritan");
        QCOMPARE(changed.count(), 1);
        settings.setColours("sepia");
        QCOMPARE(changed.count(), 2);
        QCOMPARE(settings.colours(), "default");
        QCOMPARE(colours(), Colours::Default);
        QCOMPARE(in(dir)->value("look/colours").toString(), "default");

        in(dir)->setValue("look/colours", "sepia");
        useColours(Colours::Monochrome);
        WindowSettings unknown(in(dir), "offscreen");
        QCOMPARE(unknown.colours(), "default");
        QCOMPARE(colours(), Colours::Default);
    }

    // The Settings window's radio buttons, one for each palette in its order; the arrows step
    // through them and stop at either end.
    void eachPaletteIsAChoiceInOrderAndTheArrowsStopAtTheEnds() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        WindowSettings settings(in(dir), "offscreen");
        const QVariantList choices = settings.colourChoices();
        QCOMPARE(choices.size(), qsizetype(everyColours.size()));
        for (size_t i = 0; i < everyColours.size(); ++i) {
            const QVariantMap choice = choices[static_cast<qsizetype>(i)].toMap();
            QCOMPARE(choice.value("key").toString(), coloursKey(everyColours[i]));
            QCOMPARE(choice.value("label").toString(), coloursLabel(everyColours[i]));
        }
        QCOMPARE(WindowSettings::coloursBeside("default", 1), "deuteran");
        QCOMPARE(WindowSettings::coloursBeside("protan", -1), "deuteran");
        QCOMPARE(WindowSettings::coloursBeside("protan", 2), "monochrome");
        QCOMPARE(WindowSettings::coloursBeside("monochrome", 1), "monochrome");
        QCOMPARE(WindowSettings::coloursBeside("default", -1), "default");
        QCOMPARE(WindowSettings::coloursBeside("sepia", 1), "deuteran");
    }

    // Each part's text is the platform's size until it is set, each on its own, kept to the half
    // point from 6 to 24; set to the platform's, it is kept as no size, and follows the platform
    // again; and all three go back to the platform's at once.
    void eachPartsTextSizeIsThePlatformsUntilSetAndEachIsItsOwn() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        WindowSettings settings(in(dir), "offscreen");
        const qreal platform = std::round(QFontInfo(QGuiApplication::font()).pointSizeF() * 2) / 2;
        QCOMPARE(settings.windowTextSize(), platform);
        QCOMPARE(settings.listTextSize(), platform);
        QCOMPARE(settings.detailsTextSize(), platform);
        QSignalSpy changed(&settings, &WindowSettings::textSizesChanged);

        settings.setWindowTextSize(14);
        QCOMPARE(changed.count(), 1);
        QCOMPARE(settings.windowTextSize(), 14.0);
        QCOMPARE(settings.listTextSize(), platform);
        QCOMPARE(settings.detailsTextSize(), platform);
        QCOMPARE(in(dir)->value("text/window").toReal(), 14.0);
        settings.setListTextSize(12.3);
        QCOMPARE(settings.listTextSize(), 12.5);
        settings.setDetailsTextSize(30);
        QCOMPARE(settings.detailsTextSize(), 24.0);
        settings.setDetailsTextSize(2);
        QCOMPARE(settings.detailsTextSize(), 6.0);
        QCOMPARE(changed.count(), 4);
        // The same size again changes nothing.
        settings.setDetailsTextSize(6);
        QCOMPARE(changed.count(), 4);

        settings.setWindowTextSize(platform);
        QCOMPARE(settings.windowTextSize(), platform);
        QVERIFY(!in(dir)->contains("text/window"));
        QCOMPARE(changed.count(), 5);
        settings.useSystemTextSizes();
        QCOMPARE(changed.count(), 6);
        QCOMPARE(settings.listTextSize(), platform);
        QCOMPARE(settings.detailsTextSize(), platform);
        for (const char *key : {"text/window", "text/list", "text/details"}) {
            QVERIFY2(!in(dir)->contains(key), key);
        }
        settings.useSystemTextSizes();
        QCOMPARE(changed.count(), 6);
    }

    // A size kept outside 6 to 24, or that is not a number, was not set by the Settings window:
    // the part's text is the platform's. One inside is kept to the half point.
    void aKeptSizeOutsideTheRangeOrNotANumberIsThePlatforms() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        in(dir)->setValue("text/window", "big");
        in(dir)->setValue("text/list", 3);
        in(dir)->setValue("text/details", 40);
        WindowSettings settings(in(dir), "offscreen");
        const qreal platform = std::round(QFontInfo(QGuiApplication::font()).pointSizeF() * 2) / 2;
        QCOMPARE(settings.windowTextSize(), platform);
        QCOMPARE(settings.listTextSize(), platform);
        QCOMPARE(settings.detailsTextSize(), platform);
        in(dir)->setValue("text/window", "nan");
        QCOMPARE(settings.windowTextSize(), platform);
        in(dir)->setValue("text/details", 13.3);
        QCOMPARE(settings.detailsTextSize(), 13.5);
    }

    // An unset size is the platform's as it is now: a change of the desktop's font reaches it, and
    // a size that was set stays as set.
    void anUnsetSizeFollowsThePlatformsFontAsItChanges() {
        const auto restore =
            qScopeGuard([was = QGuiApplication::font()] { QGuiApplication::setFont(was); });
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        WindowSettings settings(in(dir), "offscreen");
        settings.setListTextSize(14);
        QSignalSpy changed(&settings, &WindowSettings::textSizesChanged);
        QFont larger = QGuiApplication::font();
        const qreal platform = std::round((larger.pointSizeF() + 3) * 2) / 2;
        larger.setPointSizeF(larger.pointSizeF() + 3);
        QGuiApplication::setFont(larger);
        QCOMPARE(changed.count(), 1);
        QCOMPARE(settings.windowTextSize(), platform);
        QCOMPARE(settings.detailsTextSize(), platform);
        QCOMPARE(settings.listTextSize(), 14.0);
    }

    // A platform font set in pixels has a size in points all the same, which the parts take.
    void aPlatformFontSetInPixelsGivesItsSizeInPoints() {
        const auto restore =
            qScopeGuard([was = QGuiApplication::font()] { QGuiApplication::setFont(was); });
        QFont pixels = QGuiApplication::font();
        pixels.setPixelSize(16);
        QGuiApplication::setFont(pixels);
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        WindowSettings settings(in(dir), "offscreen");
        // 16 pixels at the tests' 96 dots an inch.
        QCOMPARE(settings.windowTextSize(), 12.0);
    }

    // A size in the Settings window's words, from half points.
    void aSizeIsSaidInPoints() {
        QCOMPARE(WindowSettings::sizeWords(20), "10 pt");
        QCOMPARE(WindowSettings::sizeWords(21), "10.5 pt");
        QCOMPARE(WindowSettings::sizeWords(48), "24 pt");
    }

    // The boxes count the sizes from 6 to 24 pt in half points. The size returned is one of those
    // given whose words are widest: in the same font, at a small size and a large one, no size's
    // words from the first to the last are wider.
    void theWidestSizeIsOneOfThoseGivenWithTheWidestWords() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        WindowSettings settings(in(dir), "offscreen");
        QCOMPARE(settings.leastHalfPoints(), 12);
        QCOMPARE(settings.mostHalfPoints(), 48);
        for (const qreal points : {6.0, 24.0}) {
            QFont font = QGuiApplication::font();
            font.setPointSizeF(points);
            const QFontMetricsF metrics(font);
            const int widest = WindowSettings::widestSize(font, 12, 48);
            QVERIFY2(widest >= 12 && widest <= 48, qPrintable(QString::number(widest)));
            const qreal most = metrics.horizontalAdvance(WindowSettings::sizeWords(widest));
            for (int halfPoints = 12; halfPoints <= 48; ++halfPoints) {
                const QString words = WindowSettings::sizeWords(halfPoints);
                QVERIFY2(metrics.horizontalAdvance(words) <= most, qPrintable(words));
            }
            QCOMPARE(WindowSettings::widestSize(font, 20, 20), 20);
            QCOMPARE(WindowSettings::widestSize(font, 20, 21), 21);
        }
    }

    // The settings' file is under XDG_CONFIG_HOME when that is absolute, on Linux and macOS alike;
    // otherwise under the home's .config on Linux, and macOS's own preferences there.
    void theSettingsFileIsUnderAnAbsoluteXdgConfigHome() {
        const QString file = QStringLiteral("subcheck/subcheck-qt.conf");
        for (const bool mac : {false, true}) {
            QCOMPARE(WindowSettings::fileFor(mac, "/c", "/h"),
                     std::optional<QString>("/c/" + file));
            QCOMPARE(WindowSettings::fileFor(mac, "/c/", "/h"),
                     std::optional<QString>("/c/" + file));
        }
        for (const char *unset : {"", "relative", "./c"}) {
            QCOMPARE(WindowSettings::fileFor(false, unset, "/h"),
                     std::optional<QString>("/h/.config/" + file));
            QVERIFY(!WindowSettings::fileFor(true, unset, "/h"));
        }
    }

    // The app's settings, under a set XDG_CONFIG_HOME, are written to its file there: on Linux the
    // one Qt's own settings for the app's names use.
    void theAppsSettingsAreWrittenUnderASetXdgConfigHome() {
        const QByteArray was = qgetenv("XDG_CONFIG_HOME");
        const bool wasSet = qEnvironmentVariableIsSet("XDG_CONFIG_HOME");
        const auto restore = qScopeGuard([&] {
            if (wasSet) {
                qputenv("XDG_CONFIG_HOME", was);
            } else {
                qunsetenv("XDG_CONFIG_HOME");
            }
        });
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        qputenv("XDG_CONFIG_HOME", dir.path().toUtf8());
        const QString file = dir.filePath("subcheck/subcheck-qt.conf");
        {
            const std::unique_ptr<QSettings> store = WindowSettings::appStore();
            QCOMPARE(store->fileName(), file);
            QCOMPARE(store->format(), QSettings::IniFormat);
            store->setValue("look/colours", "protan");
        }
        QVERIFY(QFile::exists(file));
        QCOMPARE(QSettings(file, QSettings::IniFormat).value("look/colours").toString(), "protan");
#ifndef Q_OS_MACOS
        // Qt's own format there is INI, in a .conf file.
        QCOMPARE(QSettings(QSettings::NativeFormat, QSettings::UserScope, "subcheck", "subcheck-qt")
                     .fileName(),
                 file);
#endif
    }
};

SUBCHECK_OFFSCREEN_TEST_MAIN(WindowSettingsTest)
#include "window_settings_test.moc"
