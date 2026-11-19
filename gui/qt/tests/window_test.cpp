// The real window, its QML loaded from the module as the app loads it, offscreen, on real sessions
// over the runs seeded for it: keys pressed, menu items chosen and rows, entries and buttons
// clicked as a person would, through the QML's handlers to review and back. Every test leaves the
// runs as it found them. Its video pane makes a real mpv, which offscreen can never show video:
// the player's own tests run it on a fake. Its settings are in a directory each window makes and
// removes, never the user's.
#include "menu_keys.h"
#include "mpv_item.h"
#include "offscreen_main.h"
#include "said.h"
#include "told.h"
#include "window_settings.h"
#include "words.h"

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QFont>
#include <QFontInfo>
#include <QKeyEvent>
#include <QKeySequence>
#include <QQmlApplicationEngine>
#include <QQmlComponent>
#include <QQuickItem>
#include <QQuickWindow>
#include <QRegularExpression>
#include <QScopeGuard>
#include <QScreen>
#include <QSignalSpy>
#include <QStandardPaths>
#include <QStyleHints>
#include <QTemporaryDir>
#include <QTest>
#include <QtMath>
#include <QtQml/QQmlExtensionPlugin>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <functional>
#include <memory>
#include <optional>
#include <sstream>
#include <string>
#include <tuple>
#include <utility>
#include <vector>

Q_IMPORT_QML_PLUGIN(SubcheckPlugin)

namespace {

std::string media() { return qEnvironmentVariable("SUBCHECK_TEST_MEDIA").toStdString(); }

std::string clean() { return qEnvironmentVariable("SUBCHECK_TEST_CLEAN").toStdString(); }

// The message handler quietOffscreen was installed over, which every message but the two it drops
// goes on to, and whether it was installed, so that the end of the run puts that handler back.
QtMessageHandler testsOwn = nullptr;
bool quieted = false;

// The offscreen platform can neither raise a window nor pass a window's least size on, and says so
// each time Settings is raised or its contents grow: nothing here rests on either. With no handler
// beneath it, a message goes to stderr, as Qt's own handler would write it.
void quietOffscreen(QtMsgType type, const QMessageLogContext &context, const QString &message) {
    if (message == QLatin1String("This plugin does not support raise()") ||
        message == QLatin1String("This plugin does not support propagateSizeHints()")) {
        return;
    }
    if (testsOwn) {
        testsOwn(type, context, message);
    } else {
        std::fprintf(stderr, "%s\n", qPrintable(qFormatLogMessage(type, context, message)));
    }
}

// More reviews than the sidebar has room for.
std::vector<subcheck_ffi::FolderView> manyReviews() {
    std::vector<subcheck_ffi::FolderView> folders;
    for (int i = 0; i < 30; ++i) {
        const std::string name = "Show " + std::to_string(i);
        folders.push_back(subcheck_ffi::FolderView{"/m/" + name, name, 0, false});
    }
    return folders;
}

// A finding with every part the details panel draws, each unlike the others.
subcheck_ffi::Detail everyPartOfAFinding() {
    using namespace subcheck_ffi;
    return Detail{
        .file = "/m/S1/E03.mkv",
        .label = "E03",
        .group = Group{{{"E03", std::nullopt}, {"E04", std::nullopt}}, 1},
        .at_ms = 754'450,
        .track = Track{3, 2, "Full", "en"},
        .more = 2,
        .note = FileNote::kChanged,
        .checking = Checking(Checking::kQueued{}),
        .truncated = Truncated{2'000'000, 1'000'000},
        .state = State::kRejected,
        .edited = false,
        .mark = Mark(Mark::kApplied{}),
        .original = "Your going",
        .wording = "You're going",
        .original_changed = Span{0, 4},
        .wording_changed = Span{0, 6},
        .reason = "your for you're",
        .kind = Kind(Kind::kLine{{"First before", "Second before"}, {"After"}}),
    };
}

// A row of 12 under `key`, 4 of them shown, with these fixes.
subcheck_ffi::EntryView rowOfTwelve(const subcheck_ffi::EntryKey &key,
                                    std::vector<subcheck_ffi::FixCount> fixes = {}) {
    return subcheck_ffi::EntryView{
        .key = key,
        .state = subcheck_ffi::State::kPending,
        .is_member = false,
        .expanded = false,
        .label = "E01",
        .shown = 4,
        .total = 12,
        .at_ms = 1000,
        .original = "befor",
        .wording = "before",
        .style = "Default",
        .marks = {},
        .track = std::nullopt,
        .fixes = std::move(fixes),
        .checking = false,
        .note = std::nullopt,
        .mixed = false,
        .check = subcheck_ffi::Check::kLlm,
        .original_changed = {0, 5},
        .wording_changed = {0, 6},
    };
}

// The window over a bridge on `selector`, made as main.cpp makes it but for a short wait after a
// quit, as in the bridge's tests: the store is the test's own, so no lock held elsewhere slows
// review's end, and a review that does not end fails its test sooner. Its player gives an Open a
// long wait for the pane, which offscreen never shows video, so that a Show in Video still waits
// there however slow the runner. Its settings are kept in `settings`, a directory given, or else
// its own.
struct Shown {
    std::ostringstream out;
    std::ostringstream err;
    PlayerPort player{std::chrono::seconds(1), std::chrono::seconds(2), std::chrono::minutes(1)};
    Telling bridge;
    QTemporaryDir own;
    WindowSettings settings;
    QQmlApplicationEngine engine;
    QQuickWindow *window = nullptr;

    explicit Shown(std::vector<std::string> selector, const QString &kept = {})
        : bridge(std::move(selector), player, out, err, std::chrono::seconds(1)),
          settings(std::make_unique<QSettings>((kept.isEmpty() ? own.path() : kept) +
                                                   "/subcheck-qt.conf",
                                               QSettings::IniFormat),
                   QGuiApplication::platformName()) {
        engine.setInitialProperties({{QStringLiteral("bridge"), QVariant::fromValue(&bridge)},
                                     {QStringLiteral("settings"), QVariant::fromValue(&settings)}});
        engine.loadFromModule("Subcheck", "Main");
        if (!engine.rootObjects().isEmpty()) {
            window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        }
    }

    // The window shown, as main.cpp shows it, and active, as a person's would be; the session not
    // started yet.
    bool show() {
        if (!window) {
            return false;
        }
        settings.show(window);
        return exposedAndActive();
    }

    // The window shown and the session started in main.cpp's order, review before the window's
    // first frame, so that what review says before then is covered too; then the window active.
    bool start() {
        if (!window) {
            return false;
        }
        settings.show(window);
        bridge.start();
        return exposedAndActive();
    }

    QObject *find(const char *name) const { return window->findChild<QObject *>(name); }

    // The item called `name` in what the window draws, a Repeater's among them, which are no
    // QObject's children.
    QQuickItem *item(const char *name) const { return item(window->contentItem(), name); }

    QQuickWindow *settingsWindow() const {
        return qobject_cast<QQuickWindow *>(find("settingsWindow"));
    }
    // The item called `name` in the Settings window, which is not in the main window's scene.
    QQuickItem *settingsItem(const char *name) const {
        QQuickWindow *settings = settingsWindow();
        return settings ? item(settings->contentItem(), name) : nullptr;
    }

    static QQuickItem *item(QQuickItem *in, const char *name) {
        if (in->objectName() == QLatin1String(name)) {
            return in;
        }
        const QList<QQuickItem *> children = in->childItems();
        for (QQuickItem *child : children) {
            if (QQuickItem *found = item(child, name)) {
                return found;
            }
        }
        return nullptr;
    }

    // The text the label called `name` in `in` draws.
    static QString text(QQuickItem *in, const char *name) {
        QQuickItem *label = item(in, name);
        return label ? label->property("text").toString() : QStringLiteral("(no %1)").arg(name);
    }

    // The lines a list of labels draws, in order: its Repeater, among its children, has no text.
    static QStringList lines(QQuickItem *of) {
        QStringList drawn;
        const QList<QQuickItem *> children = of->childItems();
        for (QQuickItem *child : children) {
            const QVariant line = child->property("text");
            if (line.isValid()) {
                drawn << line.toString();
            }
        }
        return drawn;
    }

    // The button in `in` that says `words`.
    static QQuickItem *button(QQuickItem *in, const QString &words) {
        if (in->inherits("QQuickAbstractButton") && in->property("text").toString() == words) {
            return in;
        }
        const QList<QQuickItem *> children = in->childItems();
        for (QQuickItem *child : children) {
            if (QQuickItem *found = button(child, words)) {
                return found;
            }
        }
        return nullptr;
    }

    // The menu of the bar, or under one of its menus, with the title `title`.
    QObject *menu(const char *title) const {
        QObject *bar = window->property("menuBar").value<QObject *>();
        for (QObject *each : bar->findChildren<QObject *>()) {
            if (each->inherits("QQuickMenu") && each->property("title").toString() == title) {
                return each;
            }
        }
        return nullptr;
    }

    // The entry of the bar that opens its menu at `index`.
    QQuickItem *barEntry(int index) const {
        QObject *bar = window->property("menuBar").value<QObject *>();
        QQuickItem *entry = nullptr;
        QMetaObject::invokeMethod(bar, "itemAt", Q_RETURN_ARG(QQuickItem *, entry),
                                  Q_ARG(int, index));
        return entry;
    }

    int rows() { return bridge.findings()->rowCount(); }

    QString decision(int row) {
        FindingsModel *list = bridge.findings();
        if (row >= list->rowCount()) {
            return {};
        }
        return list->data(list->index(row), FindingsModel::StateWordRole).toString();
    }

    // The delegate showing `row`, wherever the list has put it: never the highlight, which is the
    // list's child too, with no index of its own.
    QQuickItem *delegate(int row) const {
        auto *content = find("findings")->property("contentItem").value<QQuickItem *>();
        const QList<QQuickItem *> items = content->childItems();
        for (QQuickItem *each : items) {
            const QVariant index = each->property("index");
            if (index.isValid() && index.toInt() == row) {
                return each;
            }
        }
        return nullptr;
    }

    QPoint middle(QQuickItem *of) const {
        return of->mapToScene(QPointF(of->width() / 2, of->height() / 2)).toPoint();
    }

    void click(QQuickItem *of) { click(window, of); }
    void click(QQuickWindow *in, QQuickItem *of) {
        QTest::mouseClick(in, Qt::LeftButton, {}, middle(of));
    }

    // The handle between the column `left` ends and the one `right` starts, dragged `by` across.
    void drag(QQuickItem *left, QQuickItem *right, int by) {
        const qreal end = left->mapToScene(QPointF(left->width(), 0)).x();
        const qreal start = right->mapToScene(QPointF(0, 0)).x();
        const QPoint handle(qRound((end + start) / 2), window->height() / 2);
        QTest::mousePress(window, Qt::LeftButton, {}, handle);
        QTest::mouseMove(window, handle + QPoint(by / 3, 0));
        QTest::mouseMove(window, handle + QPoint(by, 0));
        QTest::mouseRelease(window, Qt::LeftButton, {}, handle + QPoint(by, 0));
    }

    // The actions sent since `from`, the count of those sent before; and the last, none before
    // the first.
    QStringList since(qsizetype from) const { return bridge.sent.mid(from); }
    QString last() const { return bridge.sent.isEmpty() ? QString() : bridge.sent.last(); }

private:
    // The window shown waited for until it is exposed, then made active, as a person's would be.
    bool exposedAndActive() {
        if (!QTest::qWaitForWindowExposed(window)) {
            return false;
        }
        window->requestActivate();
        return QTest::qWaitForWindowActive(window);
    }
};

} // namespace

class WindowTest : public QObject {
    Q_OBJECT

private slots:
    void initTestCase() {
        // A test decides findings in whatever store it opens: without CTest's environment, that
        // would be the user's own.
        QVERIFY2(qEnvironmentVariableIsSet("SUBCHECK_DB") &&
                     qEnvironmentVariableIsSet("SUBCHECK_TEST_MEDIA"),
                 "run through ctest, which seeds a store of the test's own");
        // Its mpv reads mpvqt/mpvqt.conf in Qt's config location, which CTest moves into the build
        // directory, beside this test: on Linux by XDG_CONFIG_HOME, on macOS by the home
        // CoreFoundation takes from CFFIXED_USER_HOME. Should either not take, every test stops
        // here, before any makes an mpv, rather than run on the user's own file. The location and
        // the file are checked apart, so that a failure on a runner no one can step through names
        // the half to fix.
#ifdef Q_OS_MACOS
        const QString config =
            QCoreApplication::applicationDirPath() + "/test-home/Library/Preferences";
#else
        const QString config = QCoreApplication::applicationDirPath() + "/test-config";
#endif
        const QString location = QStandardPaths::writableLocation(QStandardPaths::ConfigLocation);
        QVERIFY2(QDir::cleanPath(location) == QDir::cleanPath(config) ||
                     QFileInfo(location).canonicalFilePath() ==
                         QFileInfo(config).canonicalFilePath(),
                 qPrintable("Qt's config location is " + location + ", not the test's " + config));
        const QString conf = config + "/mpvqt/mpvqt.conf";
        QVERIFY2(
            QFile::exists(conf),
            qPrintable("the test's mpvqt.conf, which CMake writes as it configures, is not at " +
                       conf));
        // The fixed style CTest gives every test, so that sizes here are alike on every machine;
        // and a screen larger than the window's default, which the offscreen platform's is not.
        QCOMPARE(qEnvironmentVariable("QT_QUICK_CONTROLS_STYLE"), "Fusion");
        QVERIFY2(QGuiApplication::primaryScreen()->availableSize() == QSize(1920, 1080),
                 "run through ctest, which gives this test a large screen");
        // As main.cpp, before the first window.
        QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL);
        // Only where the two messages it drops are the platform's.
        if (QGuiApplication::platformName() == QLatin1String("offscreen")) {
            testsOwn = qInstallMessageHandler(quietOffscreen);
            quieted = true;
        }
    }

    void cleanupTestCase() {
        if (quieted) {
            qInstallMessageHandler(testsOwn);
            quieted = false;
        }
    }

    void init() {
        // A warning from the QML at run time, a binding loop say, which qmllint cannot see, fails
        // the test that met it.
        QTest::failOnWarning(QRegularExpression(QStringLiteral("\\.qml:\\d+")));
    }

    void theKeysAndAClickDriveReview() {
        Shown w({media()});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *list = w.find("findings");
        QObject *status = w.find("statusLine");
        QObject *counts = w.find("counts");
        QVERIFY(list && status && counts);
        QObject *empty = w.find("empty");
        QVERIFY(empty && !empty->property("visible").toBool());
        QCOMPARE(counts->property("text").toString(), "4 to review · 0 accepted · 0 rejected");
        QCOMPARE(status->property("text").toString(), "");

        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 1);
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 4);
        QTest::keyClick(w.window, Qt::Key_K);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 0);
        QTest::keyClick(w.window, Qt::Key_A);
        QTRY_COMPARE(w.decision(0), "Accepted");
        QTRY_COMPARE(status->property("text").toString(), "Accepted 2 findings");
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.decision(0), "To review");
        QTest::keyClick(w.window, Qt::Key_R);
        QTRY_COMPARE(w.decision(0), "Rejected");
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.decision(0), "To review");

        // The last of the expanded rows' members.
        QTRY_VERIFY(w.delegate(3));
        QQuickItem *member = w.delegate(3);
        w.click(member);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 3);
        // Collapsed from its member, the row is selected, and the list follows the splice.
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 2);
        QCOMPARE(list->property("currentIndex").toInt(), 1);

        QTest::keyClick(w.window, Qt::Key_Q);
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 0);
        QCOMPARE(w.out.str(), "media: 0 accepted, 0 rejected, 4 pending\n");
    }

    // Keys pressed without waiting for the window to catch up land where the terminal's would: two
    // moves add up, and a move after an accept starts where the accept moved on to.
    void keysPressedFasterThanTheWindowCatchesUpLandAsTheTerminalsDo() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *list = w.find("findings");
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 1);
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 4);
        QTest::keyClick(w.window, Qt::Key_K);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 0);
        // Read once the move's answer is in, which lets the accept's message go; checked after
        // the undo, so that the run is left as found whatever it shows.
        QTest::keyClick(w.window, Qt::Key_A);
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(w.bridge.counts(), "2 to review · 2 accepted · 0 rejected");
        QTRY_COMPARE(w.bridge.statusLine(), "");
        const int landed = list->property("currentIndex").toInt();
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.decision(0), "To review");
        QCOMPARE(landed, 2);
        QTest::keyClick(w.window, Qt::Key_J);
        QTest::keyClick(w.window, Qt::Key_J);
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 3);
        QTest::keyClick(w.window, Qt::Key_K);
        QTest::keyClick(w.window, Qt::Key_K);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 1);
    }

    // Every key the window has sends its action; review takes them in order, so only a key whose
    // action the window works out from what review last said waits for that, and the run is left
    // as found. The keys that quit are the next test's.
    void everyKeySendsItsAction() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *heading = w.find("heading");
        QVERIFY(heading);
        const std::vector<std::tuple<int, Qt::KeyboardModifiers, QString>> keys = {
            {Qt::Key_J, {}, "Move 1"},
            {Qt::Key_Down, {}, "Move 1"},
            {Qt::Key_K, {}, "Move -1"},
            {Qt::Key_Up, {}, "Move -1"},
            {Qt::Key_Return, {}, "Toggle"},
            {Qt::Key_Enter, Qt::KeypadModifier, "Toggle"},
            {Qt::Key_A, {}, "Accept"},
            {Qt::Key_U, {}, "Undo"},
            {Qt::Key_R, {}, "Reject"},
            {Qt::Key_Z, Qt::ControlModifier, "Undo"},
            {Qt::Key_X, {}, "Clear"},
            {Qt::Key_N, {}, "NextFile"},
            {Qt::Key_P, {}, "PreviousFile"},
            {Qt::Key_O, {}, "Play"},
            {Qt::Key_Space, {}, "Playback Pause"},
            {Qt::Key_Left, {}, "Playback Seek -5"},
            {Qt::Key_Right, {}, "Playback Seek 5"},
            {Qt::Key_Comma, {}, "Playback Seek -1"},
            {Qt::Key_Period, {}, "Playback Seek 1"},
            {Qt::Key_BracketLeft, {}, "Playback PreviousFrame"},
            {Qt::Key_BracketRight, {}, "Playback NextFrame"},
            {Qt::Key_O, Qt::ShiftModifier, "ClosePlayer"},
        };
        for (const auto &[key, modifiers, action] : keys) {
            const qsizetype from = w.bridge.sent.size();
            QTest::keyClick(w.window, Qt::Key(key), modifiers);
            QCOMPARE(w.since(from), QStringList{action});
        }
        QTRY_COMPARE(w.decision(0), "To review");
        // Each switch opens another view, from the one shown when review takes it.
        for (const char *now : {"Show Folders", "Show Newest Check"}) {
            QTest::keyClick(w.window, Qt::Key_V);
            QCOMPARE(w.last(), "SwitchView");
            QTRY_COMPARE(w.bridge.switchLabel(), now);
        }
        QCOMPARE(w.bridge.title(), "media");
        // The filter steps from the one shown, so each is waited for.
        for (const char *after :
             {"To Review · 4", "Accepted · 0", "Rejected · 0", "All Findings · 4"}) {
            QTest::keyClick(w.window, Qt::Key_F);
            QTRY_COMPARE(heading->property("text").toString(), after);
        }
        QCOMPARE(w.bridge.sent.mid(w.bridge.sent.size() - 4),
                 (QStringList{"SetFilter Pending", "SetFilter Accepted", "SetFilter Rejected",
                              "SetFilter All"}));
    }

    // The window's quit key, Close and Quit each quit: two Interrupts, the second stopping a job a
    // first asks about.
    void eachWayToQuitQuits() {
        const std::vector<std::pair<int, Qt::KeyboardModifiers>> keys = {
            {Qt::Key_Q, {}}, {Qt::Key_W, Qt::ControlModifier}, {Qt::Key_Q, Qt::ControlModifier}};
        for (const auto &[key, modifiers] : keys) {
            Shown w({media()});
            QSignalSpy finished(&w.bridge, &SessionBridge::finished);
            QVERIFY(w.start());
            QTRY_COMPARE(w.rows(), 2);
            QTest::keyClick(w.window, Qt::Key(key), modifiers);
            QVERIFY2(w.bridge.sent == (QStringList{"Interrupt", "Interrupt"}),
                     qPrintable(QKeySequence(key | modifiers).toString() + ": " +
                                w.bridge.sent.join(" | ")));
            QTRY_COMPARE(finished.count(), 1);
            QCOMPARE(finished[0][0].toInt(), 0);
        }
    }

    // The window's own close quits, as the others do; the window stays until review has ended,
    // and the counts are printed.
    void closingTheWindowQuits() {
        Shown w({media()});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->close();
        QCOMPARE(w.bridge.sent, (QStringList{"Interrupt", "Interrupt"}));
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 0);
        QVERIFY(w.window->isVisible());
        QCOMPARE(w.out.str(), "media: 0 accepted, 0 rejected, 4 pending\n");
    }

    // Each menu holds the items the window has now, each sending its action; the View menu's
    // words follow the kind of view shown, and its Show menu checks the filter shown.
    void everyMenuItemSendsItsAction() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        const std::vector<std::pair<const char *, QStringList>> items = {
            {"accept", {"Accept"}},
            {"undo", {"Undo"}},
            {"reject", {"Reject"}},
            {"undo", {"Undo"}},
            {"clear", {"Clear"}},
            {"nextEpisode", {"NextFile"}},
            {"previousEpisode", {"PreviousFile"}},
            {"play", {"Play"}},
            {"pause", {"Playback Pause"}},
            {"back5", {"Playback Seek -5"}},
            {"forward5", {"Playback Seek 5"}},
            {"back1", {"Playback Seek -1"}},
            {"forward1", {"Playback Seek 1"}},
            {"previousFrame", {"Playback PreviousFrame"}},
            {"nextFrame", {"Playback NextFrame"}},
            {"closeVideo", {"ClosePlayer"}},
            {"show2", {"SetFilter Accepted"}},
            {"show3", {"SetFilter Rejected"}},
            {"show1", {"SetFilter Pending"}},
            {"show0", {"SetFilter All"}},
        };
        for (const auto &[name, actions] : items) {
            QObject *item = w.find(name);
            QVERIFY2(item, name);
            const qsizetype from = w.bridge.sent.size();
            QVERIFY(QMetaObject::invokeMethod(item, "trigger"));
            QCOMPARE(w.since(from), actions);
        }
        QTRY_COMPARE(w.decision(0), "To review");
        QObject *show0 = w.find("show0");
        QObject *show2 = w.find("show2");
        QTRY_VERIFY(show0->property("checked").toBool());
        QVERIFY(!show2->property("checked").toBool());
        QCOMPARE(show2->property("text").toString(), "Accepted");

        QObject *switchView = w.find("switchView");
        QCOMPARE(switchView->property("text").toString(), "Show Newest Check");
        QVERIFY(QMetaObject::invokeMethod(switchView, "trigger"));
        QTRY_COMPARE(switchView->property("text").toString(), "Show Folders");
        QVERIFY(QMetaObject::invokeMethod(switchView, "trigger"));
        QTRY_COMPARE(switchView->property("text").toString(), "Show Newest Check");

        // What each menu holds: the items above, and only those.
        const std::vector<std::pair<const char *, int>> counts = {
            {"&File", 4}, {"&Edit", 1}, {"&View", 2}, {"&Show", 4}, {"F&inding", 8}, {"Vi&deo", 9}};
        for (const auto &[title, count] : counts) {
            QObject *each = w.menu(title);
            QVERIFY2(each, title);
            QCOMPARE(each->property("count").toInt(), count);
        }
        // One key, as StandardKey.Undo's several draw a warning on KDE's styles.
        QCOMPARE(w.find("undo")->property("shortcut").toString(), "Ctrl+Z");
    }

    // The Show menu checks the filter shown and no other, whichever that is, and choosing the one
    // already shown leaves it checked.
    void theShowMenuChecksTheFilterShown() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        FiltersModel *filters = w.bridge.filters();
        const auto checked = [&w] {
            QList<bool> each;
            for (const char *name : {"show0", "show1", "show2", "show3"}) {
                each << w.find(name)->property("checked").toBool();
            }
            return each;
        };
        for (const int shown : {2, 3, 1, 0}) {
            QList<bool> only{false, false, false, false};
            only[shown] = true;
            QObject *item = w.find(qPrintable(QStringLiteral("show%1").arg(shown)));
            QVERIFY(QMetaObject::invokeMethod(item, "trigger"));
            // Read once review has answered: until then the check is only the click's.
            QTRY_VERIFY(filters->data(filters->index(shown), FiltersModel::CurrentRole).toBool());
            QTRY_COMPARE(checked(), only);
            QTRY_COMPARE(w.bridge.statusLine(), "");
            const qsizetype from = w.bridge.sent.size();
            QVERIFY(QMetaObject::invokeMethod(item, "trigger"));
            QCOMPARE(w.since(from).size(), qsizetype(1));
            // Review takes its actions in order, so this answer comes after the filter's.
            w.bridge.undo();
            QTRY_COMPARE(w.bridge.statusLine(), "Nothing to undo");
            QCOMPARE(checked(), only);
        }
    }

    // While a menu of the bar is open the keys are the menu's: a letter chooses the item it
    // underlines there, whatever shortcut it is elsewhere; the arrows move along the bar; and a
    // letter no item underlines does nothing. With the menu closed, each is the window's again.
    void anOpenMenuHasTheKeys() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        const QList<const char *> titles = {"&File", "&Edit", "&View", "F&inding", "Vi&deo"};
        const auto noneOpen = [&] {
            return std::none_of(titles.begin(), titles.end(), [&w](const char *title) {
                return w.menu(title)->property("visible").toBool();
            });
        };
        struct Case {
            int menu;
            Qt::Key key;
            QStringList sends;
            int opens;
        };
        // Qt has no mnemonics on Apple platforms, so there a letter chooses no item: it still stays
        // with the menu, and no shortcut fires.
#ifdef Q_OS_MACOS
        const auto chosen = [](const char *) { return QStringList(); };
#else
        const auto chosen = [](const char *action) { return QStringList{action}; };
#endif
        const std::vector<Case> cases = {
            {4, Qt::Key_X, chosen("Playback NextFrame"), -1},
            {4, Qt::Key_P, chosen("Playback Pause"), -1},
            {4, Qt::Key_V, chosen("Playback PreviousFrame"), -1},
            {3, Qt::Key_V, chosen("Play"), -1},
            {0, Qt::Key_Right, {}, 1},
            {4, Qt::Key_Left, {}, 3},
            {0, Qt::Key_A, {}, -1},
        };
        for (const Case &each : cases) {
            QTRY_VERIFY(noneOpen());
            QQuickItem *entry = w.barEntry(each.menu);
            QVERIFY(entry);
            w.click(entry);
            QTRY_VERIFY(w.menu(titles[each.menu])->property("opened").toBool());
            const qsizetype from = w.bridge.sent.size();
            QTest::keyClick(w.window, each.key);
            if (each.opens >= 0) {
                QTRY_VERIFY(w.menu(titles[each.opens])->property("opened").toBool());
            }
            QTRY_COMPARE(w.since(from), each.sends);
            if (each.sends.isEmpty()) {
                QTest::keyClick(w.window, Qt::Key_Escape);
            }
            QTRY_VERIFY(noneOpen());
            QCOMPARE(w.since(from), each.sends);
        }
        QTest::keyClick(w.window, Qt::Key_X);
        QCOMPARE(w.last(), "Clear");
    }

    // A menu's keys are kept from the window's shortcuts only while it is open, and only for a bar
    // the window draws: the system's own bar has the keys of its open menu before the window does.
    void theKeysAreKeptForAnOpenMenuOfABarTheWindowDraws() {
        const auto kept = [](const QString &platform, bool open, QEvent::Type type) {
            MenuKeys keys(platform);
            QObject window;
            keys.setTarget(&window);
            keys.setProperty("menuOpen", open);
            QKeyEvent event(type, Qt::Key_X, Qt::NoModifier);
            event.ignore();
            QCoreApplication::sendEvent(&window, &event);
            return event.isAccepted();
        };
        for (const char *platform : {"xcb", "wayland", "wayland-egl", "offscreen", "windows"}) {
            QVERIFY2(kept(platform, true, QEvent::ShortcutOverride), platform);
            QVERIFY2(!kept(platform, false, QEvent::ShortcutOverride), platform);
            QVERIFY2(!kept(platform, true, QEvent::KeyPress), platform);
        }
        QVERIFY(!kept("cocoa", true, QEvent::ShortcutOverride));
        // An app can ask Qt to draw the bar on macOS too.
        QCoreApplication::setAttribute(Qt::AA_DontUseNativeMenuBar, true);
        const bool drawnWhenAsked = kept("cocoa", true, QEvent::ShortcutOverride);
        QCoreApplication::setAttribute(Qt::AA_DontUseNativeMenuBar, false);
        QVERIFY(drawnWhenAsked);

        // A window it no longer watches keeps its shortcuts, and a new one gives way.
        MenuKeys keys(QStringLiteral("xcb"));
        QObject first;
        QObject second;
        keys.setProperty("menuOpen", true);
        keys.setTarget(&first);
        QCOMPARE(keys.target(), &first);
        keys.setTarget(&first);
        keys.setTarget(&second);
        QKeyEvent toFirst(QEvent::ShortcutOverride, Qt::Key_X, Qt::NoModifier);
        toFirst.ignore();
        QCoreApplication::sendEvent(&first, &toFirst);
        QVERIFY(!toFirst.isAccepted());
        QKeyEvent toSecond(QEvent::ShortcutOverride, Qt::Key_X, Qt::NoModifier);
        toSecond.ignore();
        QCoreApplication::sendEvent(&second, &toSecond);
        QVERIFY(toSecond.isAccepted());
    }

    // The window's Close and Quit, from their menu.
    void theFileMenusItemsQuit() {
        for (const char *name : {"close", "quit"}) {
            Shown w({media()});
            QSignalSpy finished(&w.bridge, &SessionBridge::finished);
            QVERIFY(w.start());
            QTRY_COMPARE(w.rows(), 2);
            QVERIFY(QMetaObject::invokeMethod(w.find(name), "trigger"));
            QCOMPARE(w.bridge.sent, (QStringList{"Interrupt", "Interrupt"}));
            QTRY_COMPARE(finished.count(), 1);
        }
    }

    // The sidebar: a review opens its folder, a filter is set, an episode selected and then its
    // first finding; Check a Folder… is drawn, and does nothing. A click takes no keys from the
    // window, so a key after it still sends its action.
    void theSidebarsEntriesAskReviewAndLeaveTheKeysWorking() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        const QString folder = QString::fromStdString(media());
        const QString clean = QFileInfo(folder).path() + "/clean";
        // A list makes its entries when it next lays itself out, which can come after the rows.
        QTRY_VERIFY(w.item("review0") && w.item("episode1"));
        QQuickItem *review = w.item("review0");
        QQuickItem *filter = w.item("filter2");
        QQuickItem *episode = w.item("episode1");
        QQuickItem *check = w.item("checkAFolder");
        QVERIFY(review && filter && episode && check);
        QCOMPARE(review->property("text").toString(), "clean");
        QVERIFY(!check->isEnabled());
        w.click(check);
        QCOMPARE(w.bridge.sent, QStringList());

        w.click(episode);
        QCOMPARE(w.bridge.sent,
                 (QStringList{"SelectFile " + folder + "/typos - E02.mkv", "ShowFileFindings"}));
        QTRY_COMPARE(w.bridge.episodeName(), "E02");
        QTest::keyClick(w.window, Qt::Key_J);
        QCOMPARE(w.last(), "Move 1");

        w.click(filter);
        QCOMPARE(w.last(), "SetFilter Accepted");
        QTRY_COMPARE(w.rows(), 0);
        QTest::keyClick(w.window, Qt::Key_F);
        QCOMPARE(w.last(), "SetFilter Rejected");

        w.click(review);
        QCOMPARE(w.last(), "OpenFolder " + clean);
        QTRY_COMPARE(w.bridge.title(), "clean");
        QTRY_VERIFY(w.item("review0") && w.item("review0")->property("highlighted").toBool());
        QTest::keyClick(w.window, Qt::Key_K);
        QCOMPARE(w.last(), "Move -1");
    }

    // Each entry of each of the sidebar's lists asks for its own row, says what review says of
    // that row, and is marked while it is the one shown.
    void eachSidebarEntryIsItsOwnRow() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("review1") && w.item("episode1"));
        const auto entry = [&w](const char *list, int row) {
            return w.item(qPrintable(QStringLiteral("%1%2").arg(list).arg(row)));
        };
        const auto marked = [&entry](const char *list, int row) {
            return entry(list, row)->property("highlighted").toBool();
        };

        FiltersModel *filters = w.bridge.filters();
        const QStringList names = {"All", "Pending", "Accepted", "Rejected"};
        QCOMPARE(qsizetype(filters->rowCount()), names.size());
        for (const int row : {1, 2, 3, 0}) {
            const QVariant count = filters->data(filters->index(row), FiltersModel::CountRole);
            QCOMPARE(Shown::text(entry("filter", row), "entrySide"), count.toString());
            w.click(entry("filter", row));
            QCOMPARE(w.last(), "SetFilter " + names[row]);
            QTRY_VERIFY(marked("filter", row));
            QVERIFY(!marked("filter", (row + 1) % 4));
        }
        QCOMPARE(Shown::text(entry("filter", 0), "entrySide"), "4");
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(entry("episode", 1));

        EpisodesModel *files = w.bridge.episodes();
        QCOMPARE(files->rowCount(), 2);
        QTRY_VERIFY(marked("episode", 0));
        QVERIFY(!marked("episode", 1));
        for (const int row : {1, 0}) {
            const QModelIndex at = files->index(row);
            const QString stage = files->data(at, EpisodesModel::StageRole).toString();
            QVERIFY(!stage.isEmpty());
            QCOMPARE(Shown::text(entry("episode", row), "entrySide"), stage);
            const QString path = files->data(at, EpisodesModel::PathRole).toString();
            const qsizetype from = w.bridge.sent.size();
            w.click(entry("episode", row));
            QCOMPARE(w.since(from), (QStringList{"SelectFile " + path, "ShowFileFindings"}));
            QTRY_COMPARE(w.bridge.episodeName(),
                         files->data(at, EpisodesModel::LabelRole).toString());
            QTRY_VERIFY(marked("episode", row));
            QVERIFY(!marked("episode", 1 - row));
        }

        FoldersModel *folders = w.bridge.folders();
        QCOMPARE(folders->rowCount(), 2);
        QTRY_VERIFY(marked("review", 1));
        QVERIFY(!marked("review", 0));
        for (const int row : {0, 1}) {
            const QModelIndex at = folders->index(row);
            const QString path = folders->data(at, FoldersModel::PathRole).toString();
            const int count = folders->data(at, FoldersModel::CountRole).toInt();
            QCOMPARE(Shown::text(entry("review", row), "entrySide"),
                     count > 0 ? QString::number(count) : QString());
            w.click(entry("review", row));
            QCOMPARE(w.last(), "OpenFolder " + path);
            QTRY_COMPARE(w.bridge.title(), folders->data(at, FoldersModel::NameRole).toString());
            QTRY_VERIFY(entry("review", row) && marked("review", row));
            QVERIFY(!marked("review", 1 - row));
        }
    }

    // Each review's path shows as its tooltip while the pointer is on it. Whether controls follow
    // the pointer is the platform's to say, and the offscreen one says not, so this test asks.
    void aReviewsPathIsItsTooltip() {
        QStyleHints *hints = QGuiApplication::styleHints();
        const auto restore = qScopeGuard(
            [hints, was = hints->useHoverEffects()] { hints->setUseHoverEffects(was); });
        hints->setUseHoverEffects(true);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("review1"));
        FoldersModel *folders = w.bridge.folders();
        for (const int row : {0, 1}) {
            const QString name = QStringLiteral("review%1").arg(row);
            QQuickItem *review = w.item(qPrintable(name));
            QVERIFY(review);
            auto *tip = review->findChild<QObject *>("reviewPath");
            QVERIFY(tip);
            QVERIFY(!tip->property("visible").toBool());
            QTest::mouseMove(w.window, w.middle(review));
            // Long past the tooltip's own delay, however busy the machine.
            QTRY_VERIFY_WITH_TIMEOUT(tip->property("visible").toBool(), 15000);
            const QString path =
                folders->data(folders->index(row), FoldersModel::PathRole).toString();
            QCOMPARE(tip->property("text").toString(), path);
            auto *drawn = tip->property("contentItem").value<QQuickItem *>();
            QVERIFY(drawn);
            QCOMPARE(drawn->property("text").toString(), path);
        }
    }

    // A row and the details panel each draw what review says of the finding, every part in its
    // own place.
    void aRowAndTheDetailsDrawWhatReviewSays() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        FindingsModel *list = w.bridge.findings();
        for (const int row : {0, 1}) {
            QTRY_VERIFY(w.delegate(row));
            QQuickItem *drawn = w.delegate(row);
            const auto says = [list, row](int role) {
                return list->data(list->index(row), role).toString();
            };
            QVERIFY(!says(FindingsModel::SeveralRole).isEmpty());
            QCOMPARE(Shown::text(drawn, "rowKind"), says(FindingsModel::KindRole));
            QCOMPARE(Shown::text(drawn, "rowSeveral"), says(FindingsModel::SeveralRole));
            QCOMPARE(Shown::text(drawn, "rowPlace"), says(FindingsModel::PlaceRole));
            QCOMPARE(Shown::text(drawn, "rowState"),
                     says(FindingsModel::GlyphRole) + " " + says(FindingsModel::StateWordRole));
            const auto colour = [list, row](int role) {
                return list->data(list->index(row), role).value<QColor>();
            };
            QCOMPARE(Shown::item(drawn, "rowState")->property("color").value<QColor>(),
                     colour(FindingsModel::StateToneRole));
            QCOMPARE(Shown::item(drawn, "badge")->property("color").value<QColor>(),
                     colour(FindingsModel::KindToneRole));
        }
        QTRY_COMPARE(w.window->title(), w.bridge.title());
        QCOMPARE(w.window->title(), "media");

        // The selected row: the style's selection colour as a tint, outlined at full strength.
        auto *selection = w.find("findings")->property("highlightItem").value<QQuickItem *>();
        QVERIFY(selection);
        const QColor highlight =
            w.window->property("palette").value<QObject *>()->property("highlight").value<QColor>();
        const QColor tint = selection->property("color").value<QColor>();
        QCOMPARE(tint.rgb(), highlight.rgb());
        QVERIFY(qAbs(tint.alphaF() - 0.3) < 0.01);
        auto *outline = selection->property("border").value<QObject *>();
        QVERIFY(outline);
        QCOMPARE(outline->property("color").value<QColor>(), highlight);
        QCOMPARE(outline->property("width").toInt(), 1);

        // A member sits in under its row.
        const auto sitsAt = [&w](int row) -> qreal {
            QQuickItem *drawn = w.delegate(row);
            QQuickItem *line = drawn ? Shown::item(drawn, "rowLine") : nullptr;
            return line ? line->mapToItem(drawn, QPointF(0, 0)).x() : -1;
        };
        QTRY_COMPARE(sitsAt(0), 8.0);
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 4);
        QTRY_COMPARE(sitsAt(1), 28.0);
        QTRY_COMPARE(sitsAt(0), 8.0);
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 2);

        DetailView *detail = w.bridge.detail();
        QTRY_VERIFY(detail->shown());
        QQuickItem *panel = w.item("details");
        QVERIFY(panel);
        // Parts unlike each other, so that one drawn in another's place shows.
        QVERIFY(detail->now() != detail->fixed());
        QVERIFY(!detail->reason().isEmpty());
        QVERIFY(detail->reason() != detail->mark());
        QCOMPARE(Shown::text(panel, "detailKind"), detail->kind());
        QCOMPARE(Shown::text(panel, "place"), detail->place());
        QCOMPARE(Shown::text(panel, "state"), detail->glyph() + " " + detail->stateWord());
        QQuickItem *now = Shown::item(panel, "nowBlock");
        QQuickItem *fixed = Shown::item(panel, "fixedBlock");
        QVERIFY(now && fixed);
        QCOMPARE(Shown::text(now, "blockLabel"), "− In the subtitles now");
        QCOMPARE(Shown::text(now, "blockLine"), detail->now());
        QCOMPARE(Shown::text(fixed, "blockLabel"), "+ With the fix");
        QCOMPARE(Shown::text(fixed, "blockLine"), detail->fixed());
        QCOMPARE(Shown::text(panel, "reason"), detail->reason());
        QCOMPARE(Shown::text(panel, "mark"), detail->mark());
    }

    // Every part of a finding is drawn in the details panel, from what review says of it: the
    // lines around it with the line itself between them, what sets its file apart, its row's
    // episodes, a timing finding's stretches, and what the check said of its episode.
    void everyPartOfAFindingIsDrawnInTheDetails() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        DetailView *detail = w.bridge.detail();
        QTRY_VERIFY(detail->shown());
        QQuickItem *panel = w.item("details");
        QVERIFY(panel);
        const auto drawn = [panel](const char *name) {
            return Shown::lines(Shown::item(panel, name));
        };

        QTRY_VERIFY(!w.bridge.episodeLog().isEmpty());
        QQuickItem *heading = Shown::item(panel, "episodeLogHeading");
        QVERIFY(heading);
        QTRY_VERIFY(heading->isVisible());
        QCOMPARE(heading->property("text").toString(),
                 "What the check said of " + w.bridge.episodeName());
        QTRY_COMPARE(drawn("episodeLog"), w.bridge.episodeLog());

        subcheck_ffi::Detail finding = everyPartOfAFinding();
        detail->show(finding);
        QVERIFY(!detail->before().isEmpty() && !detail->after().isEmpty());
        QVERIFY(detail->before() != detail->after());
        QVERIFY(!detail->banners().isEmpty());
        QVERIFY(!detail->episodes().isEmpty());
        QTRY_COMPARE(drawn("before"), detail->before());
        QTRY_COMPARE(drawn("after"), detail->after());
        QTRY_COMPARE(drawn("banners"), detail->banners());
        QCOMPARE(Shown::text(panel, "between"), detail->now());
        QCOMPARE(Shown::text(panel, "episodes"), detail->episodes());
        QCOMPARE(Shown::item(panel, "state")->property("color").value<QColor>(),
                 detail->stateTone());

        finding.kind = subcheck_ffi::Kind(subcheck_ffi::Kind::kTiming{
            {{2, std::nullopt, 60'000}}, {{1'000, 2'000}}, std::nullopt});
        detail->show(finding);
        QVERIFY(!detail->timing().isEmpty());
        QTRY_COMPARE(drawn("timing"), detail->timing());
    }

    // Another finding opens at the top of the details panel, however far the last was scrolled;
    // the same finding, refreshed in place by a decision on it, stays where it was.
    void anotherFindingOpensAtTheTopOfTheDetails() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->height(), 480);
        auto *scroll = w.item("detailsScroll")->property("contentItem").value<QQuickItem *>();
        QVERIFY(scroll);
        const qreal down = 40;
        QTRY_VERIFY(scroll->property("contentHeight").toReal() - scroll->height() > down);

        // The last row: a decision on it has no row to move on to, so it refreshes it in place.
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(w.bridge.selected(), 1);
        scroll->setProperty("contentY", down);
        QCOMPARE(scroll->property("contentY").toReal(), down);
        QTest::keyClick(w.window, Qt::Key_A);
        QTRY_COMPARE(w.bridge.detail()->stateWord(), "Accepted");
        QCOMPARE(w.bridge.selected(), 1);
        QCOMPARE(scroll->property("contentY").toReal(), down);
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.bridge.detail()->stateWord(), "To review");
        QCOMPARE(w.bridge.selected(), 1);
        QCOMPARE(scroll->property("contentY").toReal(), down);

        QTest::keyClick(w.window, Qt::Key_K);
        QTRY_COMPARE(w.bridge.selected(), 0);
        QTRY_COMPARE(scroll->property("contentY").toReal(), 0.0);
    }

    // However long a row's count of places, at the list's least width its state stays inside
    // the row: a state is never shown by colour alone, so its glyph and word must be in view.
    void aNarrowRowKeepsItsStateInView() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(qRound(w.item("findings")->width()), 260);
        FindingsModel *findings = w.bridge.findings();
        const std::optional<subcheck_ffi::EntryKey> key = findings->keyAt(0);
        QVERIFY(key);
        QVERIFY(findings->apply(subcheck_ffi::Splice{0, 1, {rowOfTwelve(*key)}}));
        const QString several = "In 12 places, 4 shown";
        QTRY_VERIFY(w.delegate(0) && Shown::text(w.delegate(0), "rowSeveral") == several);
        QQuickItem *row = w.delegate(0);
        QQuickItem *count = Shown::item(row, "rowSeveral");
        QQuickItem *state = Shown::item(row, "rowState");
        // The row lays itself out in more than one pass, so what is waited for is the end of it:
        // the count cut short, and the state inside the row's margin.
        const auto stateEnds = [row, state] {
            return state->mapToItem(row, QPointF(state->width(), 0)).x();
        };
        QTRY_VERIFY2(count->property("truncated").toBool() && stateEnds() <= row->width() - 8 + 0.5,
                     qPrintable(QStringLiteral("the state ends at %1").arg(stateEnds())));

        // With room to spare, the count is drawn whole and takes no more than that.
        w.window->resize(1280, 800);
        QTRY_VERIFY(row->width() > 400);
        QTRY_VERIFY(!count->property("truncated").toBool());
        QTRY_COMPARE(qRound(count->width()), qCeil(count->implicitWidth()));
    }

    // A row's fixes are one line like any other: too many for the row, they end at its margin.
    void aLongListOfFixesElidesAtTheRowsEdge() {
        using subcheck_ffi::Fix;
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(qRound(w.item("findings")->width()), 260);
        FindingsModel *findings = w.bridge.findings();
        const std::optional<subcheck_ffi::EntryKey> key = findings->keyAt(0);
        QVERIFY(key);
        QVERIFY(findings->apply(
            subcheck_ffi::Splice{0,
                                 1,
                                 {rowOfTwelve(*key, {{Fix(Fix::kShift{4}), 5},
                                                     {Fix(Fix::kShift{3}), 3},
                                                     {Fix(Fix::kShift{2}), 2},
                                                     {Fix(Fix::kShift{1}), 1},
                                                     {Fix(Fix::kPartial{}), 1}})}}));
        const QString said = "4 frames late × 5 · 3 frames late × 3 · 2 frames late × 2 · "
                             "1 frame late × 1 · partly off × 1";
        QTRY_VERIFY(w.delegate(0) && Shown::text(w.delegate(0), "rowLine") == said);
        QQuickItem *row = w.delegate(0);
        QQuickItem *line = Shown::item(row, "rowLine");
        QTRY_VERIFY(line->property("truncated").toBool());
        QVERIFY(line->mapToItem(row, QPointF(line->width(), 0)).x() <= row->width() - 8 + 0.5);
    }

    // A click held past a long press is still a click on its row.
    void aClickHeldLongStillSelectsItsRow() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.delegate(1));
        const QPoint at = w.middle(w.delegate(1));
        QTest::mousePress(w.window, Qt::LeftButton, {}, at);
        QTest::qWait(QGuiApplication::styleHints()->mousePressAndHoldInterval() + 200);
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, at);
        QTRY_COMPARE(w.find("findings")->property("currentIndex").toInt(), 1);
        QVERIFY2(w.last().startsWith("Select row "), qPrintable(w.last()));
    }

    // Whatever the finding's lines, the details panel wraps them at its width and never scrolls
    // sideways: its technical details hold the longest, a file's whole path.
    void theDetailsPanelScrollsOnlyUpAndDown() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *scroll = w.item("detailsScroll");
        QQuickItem *technical = w.item("technical");
        QVERIFY(scroll && technical);
        w.click(technical);
        QTRY_VERIFY(w.item("technicalLines")->isVisible());
        QTRY_COMPARE(scroll->property("contentWidth").toReal(),
                     scroll->property("availableWidth").toReal());
    }

    // A title too long for the toolbar is cut, and Undo stays where it is.
    void aLongTitleLeavesUndoInTheWindow() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->width(), 762);
        QQuickItem *title = w.item("title");
        QQuickItem *undo = w.item("undoButton");
        QVERIFY(title && undo);
        QQuickItem *toolbar = w.item("toolbar");
        QVERIFY(toolbar);
        const auto undoEndsInPlace = [toolbar, undo] {
            const qreal end = undo->mapToItem(toolbar, QPointF(undo->width(), 0)).x();
            return qAbs(end - (toolbar->width() - toolbar->property("rightPadding").toReal() - 8)) <
                   1;
        };
        QTRY_VERIFY(undoEndsInPlace());
        title->setProperty("text", QStringLiteral("A very long name for a show ").repeated(10));
        QTRY_VERIFY(title->property("truncated").toBool());
        QTRY_VERIFY(undoEndsInPlace());
    }

    // The details panel's buttons, the toolbar's Undo and a double-click on a row; and the
    // finding's particulars under their disclosure.
    void theButtonsAndADoubleClickAskReview() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *accept = w.item("acceptButton");
        QQuickItem *reject = w.item("rejectButton");
        QQuickItem *undo = w.item("undoButton");
        QQuickItem *technical = w.item("technical");
        QVERIFY(accept && reject && undo && technical);
        w.click(accept);
        QTRY_COMPARE(w.decision(0), "Accepted");
        w.click(undo);
        QTRY_COMPARE(w.decision(0), "To review");
        w.click(reject);
        QTRY_COMPARE(w.decision(0), "Rejected");
        w.click(undo);
        QTRY_COMPARE(w.decision(0), "To review");
        QCOMPARE(w.bridge.sent, (QStringList{"Accept", "Undo", "Reject", "Undo"}));

        QQuickItem *particulars = w.item("technicalLines");
        QVERIFY(particulars);
        QVERIFY(!technical->property("checked").toBool());
        QVERIFY(!particulars->isVisible());
        w.click(technical);
        QVERIFY(technical->property("checked").toBool());
        QTRY_VERIFY(particulars->isVisible());
        QVERIFY(!w.bridge.detail()->technical().isEmpty());
        QTRY_COMPARE(Shown::lines(particulars), w.bridge.detail()->technical());

        QTRY_VERIFY(w.delegate(1));
        QQuickItem *second = w.delegate(1);
        const qsizetype from = w.bridge.sent.size();
        QTest::mouseDClick(w.window, Qt::LeftButton, {}, w.middle(second));
        const QStringList sent = w.since(from);
        QVERIFY2(sent.size() >= 2 && sent.last() == "Play" &&
                     sent.first().startsWith("Select row "),
                 qPrintable(sent.join(" | ")));
        QTRY_COMPARE(w.bridge.statusLine(), "Opening E01 at 0:00:20.000");
    }

    // The three columns side by side, the list between; the video at the top of the details
    // panel, 16:9 at its width, as the mpv of the window's player; a key of the video's reaches
    // it through review, whose answer is the status bar. Offscreen, mpv never becomes ready, so
    // Show in Video waits there.
    void theColumnsAndTheVideoAreWhereTheyBelong() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *sidebar = w.item("sidebar");
        QQuickItem *list = w.item("findings");
        QQuickItem *details = w.item("details");
        QQuickItem *video = w.item("video");
        QVERIFY(sidebar && list && details && video);
        const auto left = [](QQuickItem *of) { return of->mapToScene(QPointF(0, 0)).x(); };
        QCOMPARE(left(sidebar), 0.0);
        QVERIFY(left(list) >= sidebar->width());
        QVERIFY(left(details) >= left(list) + list->width());
        QVERIFY(qAbs(left(details) + details->width() - w.window->width()) < 1);
        QCOMPARE(video->property("player").value<PlayerPort *>(), &w.player);
        QCOMPARE(left(video), left(details));
        QCOMPARE(video->width(), details->width());
        QVERIFY(qAbs(video->height() - video->width() * 9 / 16) < 1);
        QCOMPARE(video->mapToItem(details, QPointF(0, 0)).y(), 0.0);

        QObject *status = w.find("statusLine");
        QTest::keyClick(w.window, Qt::Key_Space);
        QTRY_COMPARE(status->property("text").toString(),
                     "The video has no file open; Show in Video opens one");
        QTest::keyClick(w.window, Qt::Key_O);
        QTRY_COMPARE(status->property("text").toString(), "Opening E01 at 0:00:01.000");
        QTest::keyClick(w.window, Qt::Key_Right);
        QTRY_COMPARE(status->property("text").toString(), "The video is still opening the file");
        QTest::keyClick(w.window, Qt::Key_O, Qt::ShiftModifier);
        QTRY_COMPARE(status->property("text").toString(), "Video closed");
    }

    // However many reviews the store holds, they take a third of the sidebar at most, and Show and
    // the episodes stay in view below them.
    void aLongListOfReviewsLeavesShowAndTheEpisodesInView() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("episode1"));
        QQuickItem *sidebar = w.item("sidebar");
        QQuickItem *reviews = w.item("reviews");
        QVERIFY(sidebar && reviews);
        // Shorter than its third, the list takes only the height of its entries.
        QTRY_VERIFY(reviews->property("contentHeight").toReal() > 0);
        QTRY_COMPARE(reviews->height(), reviews->property("contentHeight").toReal());
        QVERIFY(reviews->height() < sidebar->height() / 3);
        w.bridge.folders()->set(manyReviews());
        // Taller than the third it has, so that the list fills that third and no more.
        QTRY_VERIFY(reviews->property("contentHeight").toReal() > sidebar->height() / 3);
        QTRY_VERIFY(qAbs(reviews->height() - sidebar->height() / 3) < 1);
        const auto inView = [sidebar](QQuickItem *of) {
            return of && of->mapToItem(sidebar, QPointF(0, of->height())).y() <= sidebar->height();
        };
        QTRY_VERIFY(inView(w.item("filter3")));
        QTRY_VERIFY(inView(w.item("episode1")));
    }

    // At the window's least height the episodes keep room for three entries, which the reviews
    // give up, and everything under the reviews stays inside the sidebar.
    void aShortWindowKeepsRoomForTheEpisodes() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("episode1"));
        w.bridge.folders()->set(manyReviews());
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->height(), 480);
        QQuickItem *sidebar = w.item("sidebar");
        QQuickItem *reviews = w.item("reviews");
        QQuickItem *entry = w.item("checkAFolder");
        QVERIFY(sidebar && reviews && entry);
        QQuickItem *episodes = Shown::item(sidebar, "episodes");
        QVERIFY(episodes);
        // Read once the sidebar has laid itself out at the new height, with the long list in it:
        // the episodes then end where the sidebar does.
        QTRY_VERIFY(sidebar->height() < w.window->height());
        QTRY_VERIFY(reviews->property("contentHeight").toReal() > sidebar->height() / 3);
        const auto end = [sidebar, episodes] {
            return episodes->mapToItem(sidebar, QPointF(0, episodes->height())).y();
        };
        QTRY_VERIFY(qAbs(end() - sidebar->height()) < 1);
        QVERIFY(episodes->height() >= 3 * entry->height());
        QVERIFY(reviews->height() < sidebar->height() / 3);
        QVERIFY(reviews->height() > 0);
    }

    // A row draws its whole line while all of its change ends inside the row, and the line cut
    // before its change once it would not, following the list's width as a column is dragged: the
    // change is in view at each width. The line is made up, in place of the first row's, with one
    // long word first so that its cut line fits the list's least width.
    void aRowsLineIsCutOnlyWhenTheWholeLineWouldHideItsChange() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        const QString words = QString("x").repeated(35) + " bb ";
        const auto at = static_cast<uint64_t>(words.size());
        const QString original = words + "befor it";
        const QString wording = words + "before it";
        const subcheck_ffi::Span was{at, at + 5};
        const subcheck_ffi::Span now{at, at + 6};
        FindingsModel *findings = w.bridge.findings();
        const std::optional<subcheck_ffi::EntryKey> key = findings->keyAt(0);
        QVERIFY(key);
        const auto firstRowSays = [&](const QString &said, const QString &fixed) {
            return findings->apply(subcheck_ffi::Splice{0,
                                                        1,
                                                        {subcheck_ffi::EntryView{
                                                            .key = *key,
                                                            .state = subcheck_ffi::State::kPending,
                                                            .is_member = false,
                                                            .expanded = false,
                                                            .label = "E01",
                                                            .shown = 1,
                                                            .total = 1,
                                                            .at_ms = 1000,
                                                            .original = said.toStdString(),
                                                            .wording = fixed.toStdString(),
                                                            .style = "Default",
                                                            .marks = {},
                                                            .track = std::nullopt,
                                                            .fixes = {},
                                                            .checking = false,
                                                            .note = std::nullopt,
                                                            .mixed = false,
                                                            .check = subcheck_ffi::Check::kLlm,
                                                            .original_changed = was,
                                                            .wording_changed = now,
                                                        }}});
        };
        QVERIFY(firstRowSays(original, wording));
        const RowLine forms = rowLine(original, was, wording, now);
        QTRY_VERIFY(w.delegate(0));
        QQuickItem *line = Shown::item(w.delegate(0), "rowLine");
        QQuickItem *reach = Shown::item(w.delegate(0), "rowReach");
        QQuickItem *list = w.item("findings");
        QQuickItem *details = w.item("details");
        QVERIFY(line && reach && list && details);
        QVERIFY(!reach->isVisible());

        QTRY_COMPARE(line->property("text").toString(), forms.whole);
        QVERIFY(reach->implicitWidth() > 0 && reach->implicitWidth() <= line->width());

        w.drag(list, details, -340);
        QTRY_COMPARE(line->property("text").toString(), forms.cut);
        QVERIFY(reach->implicitWidth() > line->width());
        QVERIFY(!line->property("truncated").toBool());

        w.drag(list, details, 340);
        QTRY_COMPARE(line->property("text").toString(), forms.whole);
        QVERIFY(reach->implicitWidth() <= line->width());

        // A line too long for its row stays whole while its change is in view: a row measures its
        // change, not its line, and only the row's end cuts the words after it.
        const QString tail = QString(" and the words go on").repeated(10);
        QVERIFY(firstRowSays(original + tail, wording + tail));
        const QString wholeLine = rowLine(original + tail, was, wording + tail, now).whole;
        const auto lineNow = [&w]() -> QQuickItem * {
            QQuickItem *row = w.delegate(0);
            return row ? Shown::item(row, "rowLine") : nullptr;
        };
        QTRY_VERIFY(lineNow() && lineNow()->property("text").toString() == wholeLine);
        QTRY_VERIFY(lineNow() && lineNow()->property("truncated").toBool());
    }

    // What review names is drawn as it is, so that a word in angle brackets is a word: the heading
    // over an episode's log, and a review's path in its tooltip.
    void whatReviewNamesIsDrawnAsPlainText() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_VERIFY(w.item("review0"));
        QQuickItem *heading = w.item("episodeLogHeading");
        QVERIFY(heading);
        QCOMPARE(heading->property("textFormat").toInt(), int(Qt::PlainText));
        QObject *tip = w.item("review0")->findChild<QObject *>("reviewPath");
        QVERIFY(tip);
        auto *content = tip->property("contentItem").value<QQuickItem *>();
        QVERIFY(content);
        QCOMPARE(content->property("textFormat").toInt(), int(Qt::PlainText));
    }

    // At its least width the window keeps each column's least: the list's gives way before the
    // details panel's.
    void aNarrowWindowKeepsEachColumnsLeastWidth() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->width(), 762);
        QQuickItem *sidebar = w.item("sidebar");
        QQuickItem *list = w.item("findings");
        QQuickItem *details = w.item("details");
        QTRY_COMPARE(w.item("columns")->width(), 762);
        QTRY_VERIFY(sidebar->width() <= 160);
        QVERIFY(list->width() <= 280);
        QVERIFY(list->width() >= 260);
        QVERIFY(details->width() >= 340);
        QVERIFY(sidebar->width() >= 160);
        const auto right = [](QQuickItem *of) {
            return of->mapToScene(QPointF(of->width(), 0)).x();
        };
        QVERIFY(right(details) <= w.window->width() + 0.5);
    }

    // The window opens at its default size, the sidebar and the details panel at theirs and the
    // list taking what is left; the size and the columns' widths a person leaves are those the next
    // window starts with, from the same settings.
    void theSizeAndTheColumnsStickAcrossARestart() {
        QTemporaryDir kept;
        QVERIFY(kept.isValid());
        {
            Shown w({media()}, kept.path());
            QVERIFY(w.start());
            QTRY_COMPARE(w.rows(), 2);
            QQuickItem *sidebar = w.item("sidebar");
            QQuickItem *list = w.item("findings");
            QQuickItem *details = w.item("details");
            QVERIFY(sidebar && list && details);
            QCOMPARE(w.window->size(), QSize(1280, 800));
            QTRY_COMPARE(qRound(sidebar->width()), 220);
            QCOMPARE(qRound(details->width()), 480);
            QCOMPARE(qRound(list->width()), 578);
            w.window->resize(1100, 700);
            QTRY_COMPARE(qRound(list->width()), 398);
            w.drag(sidebar, list, 30);
            QTRY_COMPARE(qRound(sidebar->width()), 250);
            w.drag(list, details, 40);
            QTRY_COMPARE(qRound(details->width()), 440);
            QCOMPARE(w.settings.sidebarWidth(), 250);
            QCOMPARE(w.settings.detailsWidth(), 440);
        }
        Shown w({media()}, kept.path());
        QVERIFY(w.start());
        QCOMPARE(w.window->size(), QSize(1100, 700));
        QTRY_COMPARE(qRound(w.item("sidebar")->width()), 250);
        QCOMPARE(qRound(w.item("details")->width()), 440);
    }

    // A drag of one column's edge at a narrow window keeps that column's width and leaves the
    // other's as the person last chose it, though the window squeezes it for now: widened again,
    // each column has its own.
    void aDragAtANarrowWindowLeavesTheOtherColumnsKeptWidth() {
        for (const bool bySidebar : {true, false}) {
            Shown w({media()});
            QVERIFY(w.start());
            QTRY_COMPARE(w.rows(), 2);
            QQuickItem *sidebar = w.item("sidebar");
            QQuickItem *list = w.item("findings");
            QQuickItem *details = w.item("details");
            QVERIFY(sidebar && list && details);
            QTRY_COMPARE(qRound(details->width()), 480);
            // The list gives way first, then the details panel, then the sidebar: at 800 the
            // sidebar is squeezed only once the details panel is at its least width, which leaves
            // no room for the details drag. At 830 the details panel is squeezed but has room to
            // give.
            w.window->resize(bySidebar ? 800 : 830, 800);
            QTRY_COMPARE(qRound(list->width()), 260);
            QTRY_VERIFY(bySidebar ? sidebar->width() < 220 : qRound(sidebar->width()) == 220);
            QTRY_VERIFY(details->width() < 480);
            if (bySidebar) {
                const int dragged = qRound(sidebar->width()) - 10;
                w.drag(sidebar, list, -10);
                QTRY_COMPARE(qRound(sidebar->width()), dragged);
                QCOMPARE(w.settings.sidebarWidth(), dragged);
                QCOMPARE(w.settings.detailsWidth(), 480);
                w.window->resize(1280, 800);
                QTRY_COMPARE(qRound(details->width()), 480);
                QCOMPARE(qRound(sidebar->width()), dragged);
            } else {
                const int dragged = qRound(details->width()) - 5;
                w.drag(list, details, 5);
                QTRY_COMPARE(qRound(details->width()), dragged);
                QCOMPARE(w.settings.detailsWidth(), dragged);
                QCOMPARE(w.settings.sidebarWidth(), 220);
                w.window->resize(1280, 800);
                QTRY_COMPARE(qRound(sidebar->width()), 220);
                QCOMPARE(qRound(details->width()), dragged);
            }
        }
    }

    // The pane holds its player weakly: the same player given again keeps the pane, its command
    // under way still waiting; a player given in another's place takes the pane from it, and the
    // one before lets go, its command under way run again with no mpv, as if it came now; a
    // player that goes first leaves the pane none. Offscreen, the pane's mpv never says it is
    // ready, so the test says so for it, and the pane's mpv has not answered when the player goes.
    void aPaneLetsGoOfAnEarlierPlayerAndOutlivesItsOwn() {
        QQmlEngine engine;
        QQmlComponent component(&engine);
        component.setData("import Subcheck; MpvItem {}", QUrl());
        std::unique_ptr<QObject> made(component.create());
        auto *pane = qobject_cast<MpvItem *>(made.get());
        QVERIFY2(pane, qPrintable(component.errorString()));
        PlayerPort first(std::chrono::seconds(10));
        auto second = std::make_unique<PlayerPort>(std::chrono::seconds(10));
        QStringList reports;
        connect(&first, &PlayerPort::reported, this,
                [&reports](const subcheck_ffi::PlayerReport &r) { reports << said(r); });
        pane->setPlayer(&first);
        first.ready();
        using subcheck_ffi::Playback;
        using subcheck_ffi::PlayerCommand;
        first.run(PlayerCommand(PlayerCommand::kPlayback{Playback(Playback::kSeek{5})}));
        QCOMPARE(reports, QStringList());
        pane->setPlayer(&first);
        QCOMPARE(reports, QStringList());
        pane->setPlayer(second.get());
        QCOMPARE(reports, QStringList{"play failed: no file"});
        QCOMPARE(pane->player(), second.get());
        second.reset();
        QCOMPARE(pane->player(), nullptr);
    }

    void anEmptyRunSaysSo() {
        Shown w({clean()});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QVERIFY(w.start());
        QObject *empty = w.find("empty");
        QVERIFY(empty);
        QTRY_VERIFY(empty->property("visible").toBool());
        QCOMPARE(empty->property("text").toString(), "No findings in this run");
        QVERIFY(!w.bridge.detail()->shown());
        QTest::keyClick(w.window, Qt::Key_Q);
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(w.out.str(), "clean: 0 accepted, 0 rejected, 0 pending\n");
    }

    // A filter that hides every finding is told from a run with none, and the words go with the
    // filter that hid them.
    void aFilterThatHidesEveryFindingSaysSo() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *empty = w.find("empty");
        QVERIFY(empty);
        QVERIFY(!empty->property("visible").toBool());
        w.bridge.setFilter(3);
        QTRY_VERIFY(empty->property("visible").toBool());
        QCOMPARE(empty->property("text").toString(), "No findings under this filter");
        QCOMPARE(w.rows(), 0);
        w.bridge.setFilter(0);
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(!empty->property("visible").toBool());
    }

    // While a video shows, its corner says whether it plays or is paused, as mpv says; it takes no
    // click. The pane's first word from mpv is waited for, by a change of mpv's own, so that it
    // cannot land on what the test says for mpv after it.
    void theIndicatorSaysWhetherTheVideoPlaysWhileOneShows() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        auto *video = qobject_cast<MpvItem *>(w.item("video"));
        QQuickItem *indicator = w.item("playIndicator");
        QVERIFY(video && indicator);
        QVERIFY(!indicator->isVisible());
        video->MpvAbstractItem::setProperty(QStringLiteral("pause"), true);
        QTRY_VERIFY(video->paused());

        video->heard(QStringLiteral("vo-configured"), true);
        QVERIFY(video->showing());
        QVERIFY(indicator->isVisible());
        QCOMPARE(indicator->mapToItem(video, QPointF(indicator->width(), 0)),
                 QPointF(video->width() - 8, 8));
        for (const bool paused : {true, false, true}) {
            video->heard(QStringLiteral("pause"), paused);
            QCOMPARE(video->paused(), paused);
            QCOMPARE(Shown::item(indicator, "pausedBars")->isVisible(), paused);
            QCOMPARE(Shown::item(indicator, "playingTriangle")->isVisible(), !paused);
            QCOMPARE(indicator->property("said").toString(), paused ? "Paused" : "Playing");
        }
        QCOMPARE(indicator->acceptedMouseButtons(), Qt::NoButton);
        const qsizetype from = w.bridge.sent.size();
        w.click(indicator);
        QCOMPARE(w.since(from), QStringList());

        video->heard(QStringLiteral("vo-configured"), false);
        QVERIFY(!video->showing());
        QVERIFY(!indicator->isVisible());
        // As mpv makes its video output again for the next file.
        video->heard(QStringLiteral("vo-configured"), true);
        QVERIFY(indicator->isVisible());
    }

    // A row of another height takes the highlight's height at once, as a move takes its place.
    void theHighlightTakesARowsSizeAtOnce() {
        Shown w({media()});
        QVERIFY(w.start());
        QCOMPARE(w.find("findings")->property("highlightResizeDuration").toInt(), 0);
        QCOMPARE(w.find("findings")->property("highlightMoveDuration").toInt(), 0);
    }

    // Each column handle is taken within the columns' margins beside its line, over whatever
    // lies beside it, with the resize cursor, and a press there drags the column and clicks
    // nothing under it.
    void aColumnHandleIsTakenWithinTheMarginsBesideItsLine() {
        QStyleHints *hints = QGuiApplication::styleHints();
        const auto restore = qScopeGuard(
            [hints, was = hints->useHoverEffects()] { hints->setUseHoverEffects(was); });
        hints->setUseHoverEffects(true);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.delegate(0));
        QTRY_VERIFY(w.item("episode0"));
        QList<QQuickItem *> handles;
        const QList<QQuickItem *> parts = w.item("columns")->childItems();
        for (QQuickItem *part : parts) {
            if (part->objectName() == QLatin1String("columnHandle")) {
                handles << part;
            }
        }
        QCOMPARE(handles.size(), 2);
        std::sort(handles.begin(), handles.end(),
                  [](QQuickItem *a, QQuickItem *b) { return a->x() < b->x(); });
        const auto lineOf = [](QQuickItem *handle) {
            return qRound(handle->mapToScene(QPointF(handle->width() / 2, 0)).x());
        };
        const int entry = w.middle(w.item("episode0")).y();
        const int row = w.middle(w.delegate(0)).y();
        const int video = w.middle(w.item("video")).y();
        // The grip stays within the columns' 8 px margins, but for 3 px over the sidebar's
        // scroll bars on the handle beside the sidebar. Offsets are from lineOf, which rounds the
        // line's centre up to the pixel after the handle's x (L = x + 1): the grip spans
        // [x - reach, x + 9), so the last pixel in is L + 7 on the right and L - reach - 1 on
        // the left, and the first pixel out is one past each.
        struct Place {
            QQuickItem *handle;
            int y;
            int in[2];
            int out[2];
        };
        const std::vector<Place> places{{handles[0], entry, {-4, 7}, {-5, 8}},
                                        {handles[0], row, {-4, 7}, {-5, 8}},
                                        {handles[1], row, {-9, 7}, {-10, 8}},
                                        {handles[1], video, {-9, 7}, {-10, 8}}};
        for (const Place &p : places) {
            for (const int off : p.in) {
                QTest::mouseMove(w.window, QPoint(lineOf(p.handle) + off, p.y));
                QTRY_VERIFY(p.handle->property("active").toBool());
                QTRY_COMPARE(w.window->cursor().shape(), Qt::SplitHCursor);
            }
            for (const int off : p.out) {
                QTest::mouseMove(w.window, QPoint(lineOf(p.handle) + off, p.y));
                QTRY_VERIFY(!p.handle->property("active").toBool());
            }
        }

        QQuickItem *sidebar = w.item("sidebar");
        const qreal before = sidebar->width();
        const qsizetype from = w.bridge.sent.size();
        const QPoint at(lineOf(handles[0]) - 2, entry);
        QTest::mouseMove(w.window, at);
        QTest::mousePress(w.window, Qt::LeftButton, {}, at);
        QTest::mouseMove(w.window, at + QPoint(15, 0));
        QTest::mouseMove(w.window, at + QPoint(45, 0));
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, at + QPoint(45, 0));
        QTRY_COMPARE(sidebar->width(), before + 45);
        QCOMPARE(w.since(from), QStringList());

        // Over the video too, on the handle's other side.
        QQuickItem *details = w.item("details");
        const qreal wide = details->width();
        const QPoint onVideo(lineOf(handles[1]) + 7, video);
        QTest::mouseMove(w.window, onVideo);
        QTest::mousePress(w.window, Qt::LeftButton, {}, onVideo);
        QTest::mouseMove(w.window, onVideo + QPoint(15, 0));
        QTest::mouseMove(w.window, onVideo + QPoint(45, 0));
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, onVideo + QPoint(45, 0));
        QTRY_COMPARE(details->width(), wide - 45);
        QCOMPARE(w.since(from), QStringList());

        // And over a finding row, on the first handle's right side.
        const qreal grown = sidebar->width();
        const QPoint onRow(lineOf(handles[0]) + 7, row);
        QTest::mouseMove(w.window, onRow);
        QTest::mousePress(w.window, Qt::LeftButton, {}, onRow);
        QTest::mouseMove(w.window, onRow + QPoint(15, 0));
        QTest::mouseMove(w.window, onRow + QPoint(45, 0));
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, onRow + QPoint(45, 0));
        QTRY_COMPARE(sidebar->width(), grown + 45);
        QCOMPARE(w.since(from), QStringList());
    }

    // The grip leaves the sidebar's scroll bars to them: dragging a list's own bar scrolls it and
    // does not move the column.
    void aSidebarScrollBarIsNotTakenByTheColumnHandle() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.bridge.folders()->set(manyReviews());
        QQuickItem *sidebar = w.item("sidebar");
        QQuickItem *reviews = w.item("reviews");
        QVERIFY(sidebar && reviews);
        QTRY_VERIFY(reviews->property("contentHeight").toReal() > reviews->height() + 1);
        QTRY_VERIFY(qAbs(reviews->height() - sidebar->height() / 3) < 1);
        const qreal before = sidebar->width();
        QCOMPARE(reviews->property("contentY").toReal(), 0.0);
        // The bar's thumb, at the top of the right edge of the list.
        const qreal thumb =
            reviews->height() * reviews->height() / reviews->property("contentHeight").toReal();
        const QPointF top = reviews->mapToScene(QPointF(reviews->width() - 4, thumb / 2));
        const QPoint at = top.toPoint();
        QTest::mouseMove(w.window, at);
        QTest::mousePress(w.window, Qt::LeftButton, {}, at);
        QTest::mouseMove(w.window, at + QPoint(0, 10));
        QTest::mouseMove(w.window, at + QPoint(0, 30));
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, at + QPoint(0, 30));
        QTRY_VERIFY(reviews->property("contentY").toReal() > 0);
        QCOMPARE(sidebar->width(), before);
    }

    // A line across the window, under the menu bar and above the toolbar's words.
    void aLineSitsBetweenTheMenuBarAndTheToolbar() {
        Shown w({media()});
        QVERIFY(w.start());
        QQuickItem *line = w.item("menuLine");
        auto *bar = w.window->property("menuBar").value<QQuickItem *>();
        QVERIFY(line && bar);
        QVERIFY(line->isVisible());
        QCOMPARE(line->height(), 1.0);
        QCOMPARE(line->width(), qreal(w.window->width()));
        QCOMPARE(line->mapToScene(QPointF(0, 0)).y(), bar->height());
        QVERIFY(w.item("title")->mapToScene(QPointF(0, 0)).y() >= bar->height() + 1);
        // The line follows the bar: with none drawn there is no line.
        bar->setVisible(false);
        QVERIFY(!line->isVisible());
        bar->setVisible(true);
        QVERIFY(line->isVisible());
    }

    // The details panel stacks the line as it is now above the line with the fix, each a block: a
    // bar 3 px wide down its left in its side's tone, its right corners rounded, on the style's
    // alternate background; a label in bold capitals in the same tone, 2 pt below the panel's text;
    // and the line, 2 pt above it. An edited finding's block says the wording is the person's.
    void theDetailsStackTheOldLineAboveTheNewAsBlocks() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        DetailView *detail = w.bridge.detail();
        QTRY_VERIFY(detail->shown());
        QQuickItem *panel = w.item("details");
        QQuickItem *now = Shown::item(panel, "nowBlock");
        QQuickItem *fixed = Shown::item(panel, "fixedBlock");
        QVERIFY(now && fixed);
        const auto top = [](QQuickItem *of) { return of->mapToScene(QPointF(0, 0)).y(); };
        QTRY_VERIFY(now->height() > 0 && top(now) + now->height() < top(fixed));
        QCOMPARE(now->mapToScene(QPointF(0, 0)).x(), fixed->mapToScene(QPointF(0, 0)).x());
        QCOMPARE(now->width(), fixed->width());
        const qreal size = panel->property("font").value<QFont>().pointSizeF();
        const QColor alternate = panel->property("palette")
                                     .value<QObject *>()
                                     ->property("alternateBase")
                                     .value<QColor>();
        const std::vector<std::tuple<QQuickItem *, QColor, QString>> blocks = {
            {now, detail->oldTone(), "− In the subtitles now"},
            {fixed, detail->newTone(), "+ With the fix"}};
        QVERIFY(detail->oldTone() != detail->newTone());
        for (const auto &[block, tone, words] : blocks) {
            QQuickItem *bar = Shown::item(block, "blockBar");
            QQuickItem *label = Shown::item(block, "blockLabel");
            QQuickItem *line = Shown::item(block, "blockLine");
            QVERIFY(bar && label && line);
            QCOMPARE(bar->width(), 3.0);
            QCOMPARE(bar->height(), block->height());
            QCOMPARE(bar->mapToItem(block, QPointF(0, 0)), QPointF(0, 0));
            QCOMPARE(bar->property("color").value<QColor>(), tone);
            QVERIFY(label->mapToItem(block, QPointF(0, 0)).x() > bar->width());
            QCOMPARE(label->property("text").toString(), words);
            QCOMPARE(label->property("color").value<QColor>(), tone);
            const QFont labelFont = label->property("font").value<QFont>();
            QVERIFY(labelFont.bold());
            QCOMPARE(labelFont.capitalization(), QFont::AllUppercase);
            QCOMPARE(labelFont.pointSizeF(), size - 2);
            QCOMPARE(line->property("font").value<QFont>().pointSizeF(), size + 2);
            QVERIFY(top(line) > top(label));
            auto *back = block->property("background").value<QQuickItem *>();
            QVERIFY(back);
            QCOMPARE(back->property("color").value<QColor>(), alternate);
            QCOMPARE(back->property("topRightRadius").toReal(), 4.0);
            QCOMPARE(back->property("bottomRightRadius").toReal(), 4.0);
            QCOMPARE(back->property("topLeftRadius").toReal(), 0.0);
            QCOMPARE(back->property("bottomLeftRadius").toReal(), 0.0);
        }
        QCOMPARE(Shown::text(now, "blockLine"), detail->now());
        QCOMPARE(Shown::text(fixed, "blockLine"), detail->fixed());

        subcheck_ffi::Detail edited = everyPartOfAFinding();
        edited.edited = true;
        detail->show(edited);
        QCOMPARE(Shown::text(fixed, "blockLabel"), "+ With your wording");
        QCOMPARE(Shown::text(now, "blockLabel"), "− In the subtitles now");
    }

    // A palette chosen redraws the rows' states, badges and lines, the details' state, badge,
    // blocks and marks, at once, in the palette's tones; and moves nothing: the selected row, the
    // list's scroll and the panel's stay, and review is asked nothing.
    void aPaletteChosenRedrawsTheRowsAndTheDetailsInPlace() {
        const auto restore = qScopeGuard([] { useColours(Colours::Default); });
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->height(), 480);
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(w.bridge.selected(), 1);
        // Rows made up after review's two, enough for the list to scroll.
        FindingsModel *findings = w.bridge.findings();
        std::vector<subcheck_ffi::EntryView> more;
        for (uint64_t id = 100; id < 120; ++id) {
            more.push_back(rowOfTwelve(subcheck_ffi::EntryKey(subcheck_ffi::EntryKey::kRow{{id}})));
        }
        QVERIFY(findings->apply(subcheck_ffi::Splice{2, 0, more}));
        auto *list = qobject_cast<QQuickItem *>(w.find("findings"));
        QTRY_VERIFY(list->property("contentHeight").toReal() > list->height() + 100);
        list->setProperty("contentY", 60);
        auto *scroll = w.item("detailsScroll")->property("contentItem").value<QQuickItem *>();
        QTRY_VERIFY(scroll->property("contentHeight").toReal() - scroll->height() > 40);
        scroll->setProperty("contentY", 40);
        QQuickItem *panel = w.item("details");
        DetailView *detail = w.bridge.detail();
        const qsizetype from = w.bridge.sent.size();

        for (const Colours each : {Colours::Deuteran, Colours::Protan, Colours::Tritan,
                                   Colours::Monochrome, Colours::Default}) {
            w.settings.setColours(coloursKey(each));
            const Tones t = tones(each, dark());
            const QString name = coloursKey(each);
            // Every row the list draws now.
            auto *content = list->property("contentItem").value<QQuickItem *>();
            int drawn = 0;
            const QList<QQuickItem *> rows = content->childItems();
            for (QQuickItem *row : rows) {
                if (!row->property("index").isValid() || !row->isVisible()) {
                    continue;
                }
                ++drawn;
                QCOMPARE(Shown::item(row, "rowState")->property("color").value<QColor>(),
                         t.pending);
                QCOMPARE(Shown::item(row, "badge")->property("color").value<QColor>(), t.typoBadge);
                const QString line = Shown::text(row, "rowLine");
                QVERIFY2(line.contains(t.old.name()) && line.contains(t.added.name()),
                         qPrintable(name + ": " + line));
            }
            QVERIFY(drawn > 2);
            QCOMPARE(Shown::item(panel, "state")->property("color").value<QColor>(), t.pending);
            QCOMPARE(
                Shown::item(panel, "detailKind")->parentItem()->property("color").value<QColor>(),
                t.typoBadge);
            QQuickItem *now = Shown::item(panel, "nowBlock");
            QQuickItem *fixed = Shown::item(panel, "fixedBlock");
            QCOMPARE(Shown::item(now, "blockBar")->property("color").value<QColor>(), t.old);
            QCOMPARE(Shown::item(fixed, "blockBar")->property("color").value<QColor>(), t.added);
            QCOMPARE(Shown::item(now, "blockLabel")->property("color").value<QColor>(), t.old);
            QCOMPARE(Shown::item(fixed, "blockLabel")->property("color").value<QColor>(), t.added);
            QVERIFY2(Shown::text(now, "blockLine").contains(t.oldTint.name()), qPrintable(name));
            QVERIFY2(Shown::text(fixed, "blockLine").contains(t.addedTint.name()),
                     qPrintable(name));
            QCOMPARE(Shown::text(now, "blockLine"), detail->now());
            QCOMPARE(w.bridge.sample()->oldTone(), t.old);
            QVERIFY(w.bridge.sample()->now().contains(t.oldTint.name()));

            QCOMPARE(w.bridge.selected(), 1);
            QCOMPARE(list->property("currentIndex").toInt(), 1);
            QCOMPARE(list->property("contentY").toReal(), 60.0);
            QCOMPARE(scroll->property("contentY").toReal(), 40.0);
        }
        QCOMPARE(w.since(from), QStringList());
    }

    // Each part's text is the platform's size until it is set: the window's, its menus and dialogs
    // among it, and the Settings window, which takes no font from the window it opens over; the
    // list's, with its heading; and the details panel's. Each set at once, and only its own part
    // changes.
    void eachPartsTextIsThePlatformsSizeUntilSetAndEachIsItsOwn() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.delegate(0));
        QTRY_VERIFY(w.item("episode0"));
        const qreal platform = std::round(QFontInfo(QGuiApplication::font()).pointSizeF() * 2) / 2;
        const auto size = [](QObject *of) {
            return of->property("font").value<QFont>().pointSizeF();
        };
        QQuickItem *panel = w.item("details");
        const QList<QObject *> windowParts = {w.item("title"), w.find("statusLine"),
                                              w.item("episode0"), w.menu("&File"),
                                              w.settingsWindow()};
        const QList<QObject *> listParts = {w.item("heading"),
                                            Shown::item(w.delegate(0), "rowLine"),
                                            Shown::item(w.delegate(0), "rowState")};
        const QList<QObject *> detailsParts = {Shown::item(panel, "place"),
                                               Shown::item(panel, "rejectButton"),
                                               Shown::item(panel, "nowBlock")};
        // The parts not at their size, each as its name and the size it is.
        const auto offSize = [&](qreal inWindow, qreal inList, qreal inDetails) {
            QStringList off;
            const std::vector<std::pair<const QList<QObject *> *, qreal>> each = {
                {&windowParts, inWindow}, {&listParts, inList}, {&detailsParts, inDetails}};
            for (const auto &[parts, wanted] : each) {
                for (QObject *part : *parts) {
                    if (!part || size(part) != wanted) {
                        off << QStringLiteral("%1 %2")
                                   .arg(part ? part->objectName() : "none")
                                   .arg(part ? size(part) : 0);
                    }
                }
            }
            return off;
        };
        QCOMPARE(offSize(platform, platform, platform), QStringList());
        w.settings.setWindowTextSize(platform + 3);
        QCOMPARE(offSize(platform + 3, platform, platform), QStringList());
        w.settings.setListTextSize(platform + 1);
        QCOMPARE(offSize(platform + 3, platform + 1, platform), QStringList());
        w.settings.setDetailsTextSize(platform + 2.5);
        QCOMPARE(offSize(platform + 3, platform + 1, platform + 2.5), QStringList());
        w.settings.useSystemTextSizes();
        QCOMPARE(offSize(platform, platform, platform), QStringList());
    }

    // A platform font set in pixels still gives each part its size, in points.
    void aPlatformFontSetInPixelsStillSizesEachPart() {
        const auto restore =
            qScopeGuard([was = QGuiApplication::font()] { QGuiApplication::setFont(was); });
        QFont pixels = QGuiApplication::font();
        pixels.setPixelSize(16);
        QGuiApplication::setFont(pixels);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.delegate(0));
        const auto size = [](QObject *of) {
            return of->property("font").value<QFont>().pointSizeF();
        };
        // 16 pixels at the tests' 96 dots an inch.
        QCOMPARE(size(w.item("title")), 12.0);
        QCOMPARE(size(Shown::item(w.delegate(0), "rowLine")), 12.0);
        QCOMPARE(size(w.item("place")), 12.0);
    }

    // With the list's and the details panel's text at 14 pt, the narrowest window still has room
    // for each column's least width, and each row's state in view.
    void aNarrowWindowWithLargerTextKeepsEveryColumnAndEachRowsState() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.settings.setListTextSize(14);
        w.settings.setDetailsTextSize(14);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->width(), 762);
        QQuickItem *sidebar = w.item("sidebar");
        QQuickItem *list = w.item("findings");
        QQuickItem *details = w.item("details");
        QTRY_COMPARE(w.item("columns")->width(), 762);
        QTRY_VERIFY(list->width() >= 260 && list->width() <= 280);
        QVERIFY(sidebar->width() >= 160);
        QVERIFY(details->width() >= 340);
        QVERIFY(details->mapToScene(QPointF(details->width(), 0)).x() <= w.window->width() + 0.5);
        for (const int at : {0, 1}) {
            QTRY_VERIFY(w.delegate(at));
            QQuickItem *row = w.delegate(at);
            QQuickItem *state = Shown::item(row, "rowState");
            QCOMPARE(state->property("font").value<QFont>().pointSizeF(), 14.0);
            const auto stateEnds = [row, state] {
                return state->mapToItem(row, QPointF(state->width(), 0)).x();
            };
            QTRY_VERIFY2(stateEnds() <= row->width() - 8 + 0.5 &&
                             qRound(state->width()) >= qFloor(state->implicitWidth()),
                         qPrintable(QStringLiteral("the state ends at %1").arg(stateEnds())));
        }
        for (const char *name : {"nowBlock", "fixedBlock"}) {
            QQuickItem *block = Shown::item(details, name);
            QVERIFY(block->mapToItem(details, QPointF(block->width(), 0)).x() <= details->width());
        }
    }

    // The Controls that carry the list's and the details panel's sizes leave hover on, so that
    // what is in them still follows the pointer where the platform lets it.
    void theListAndDetailsControlsKeepHoverOn() {
        QStyleHints *hints = QGuiApplication::styleHints();
        const auto restore = qScopeGuard(
            [hints, was = hints->useHoverEffects()] { hints->setUseHoverEffects(was); });
        hints->setUseHoverEffects(true);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        for (const char *name : {"acceptButton", "rejectButton", "technical"}) {
            QQuickItem *item = w.item(name);
            QVERIFY2(item, name);
            QVERIFY2(item->property("hoverEnabled").toBool(), name);
        }
    }

    // File ▸ Settings… opens Settings, above Close: a dialog's window of its own, titled, modal and
    // transient for the window, which opens with the keys at its contents' size and can grow, and
    // which a click in the window beside it leaves open. Chosen again while it is open, as a menu
    // bar that still offers it could, it stays the one Settings window, with the keys. A choice in
    // it applies at once, so it has no button to confirm or cancel one, and closes as a window
    // does: Escape, Ctrl+W and the title bar's close button each close it and give review its keys
    // back, and none asks review anything; closed, it leaves Ctrl+W to File ▸ Close.
    void settingsOpensFromTheFileMenuAndClosesAsAWindowDoes() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *file = w.menu("&File");
        QVERIFY(file);
        const auto entry = [file](int index) {
            QQuickItem *at = nullptr;
            QMetaObject::invokeMethod(file, "itemAt", Q_RETURN_ARG(QQuickItem *, at),
                                      Q_ARG(int, index));
            return at;
        };
        QVERIFY(entry(0) && entry(1) && entry(2));
        QCOMPARE(entry(0)->property("action").value<QObject *>(), w.find("settings"));
        QVERIFY(entry(1)->inherits("QQuickMenuSeparator"));
        QCOMPARE(entry(2)->property("action").value<QObject *>(), w.find("close"));
        QObject *item = w.find("settings");
        QCOMPARE(item->property("text").toString(), "&Settings…");
        QCOMPARE(item->property("shortcut").toString(), "Ctrl+,");
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QCOMPARE(sw->title(), "Settings");
        QCOMPARE(sw->flags() & Qt::WindowType_Mask, Qt::Dialog);
        QCOMPARE(sw->transientParent(), w.window);
        QCOMPARE(sw->modality(), Qt::ApplicationModal);
        QVERIFY(!sw->isVisible());
        // No footer, and no button but the palettes' and Use the System Size.
        QVERIFY(!sw->property("footer").value<QQuickItem *>());
        QStringList buttons;
        const std::function<void(QQuickItem *)> gather = [&buttons, &gather](QQuickItem *in) {
            if (in->inherits("QQuickAbstractButton") && !in->inherits("QQuickRadioButton")) {
                buttons << in->objectName();
            }
            const QList<QQuickItem *> children = in->childItems();
            for (QQuickItem *child : children) {
                gather(child);
            }
        };
        gather(sw->contentItem());
        QCOMPARE(buttons, QStringList{"systemSize"});
        const qsizetype from = w.bridge.sent.size();
        bool first = true;
        for (const QString way : {"Escape", "Ctrl+W", "title bar"}) {
            QVERIFY(QMetaObject::invokeMethod(item, "trigger"));
            QTRY_VERIFY2(sw->isVisible(), qPrintable(way));
            QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
            QCOMPARE(sw->width(), sw->minimumWidth());
            QCOMPARE(sw->height(), sw->minimumHeight());
            QVERIFY2(sw->maximumWidth() > sw->minimumWidth(), qPrintable(way));
            QTest::mouseClick(w.window, Qt::LeftButton, {}, QPoint(4, 4));
            QVERIFY2(sw->isVisible(), qPrintable(way));
            if (first) {
                first = false;
                QVERIFY(QMetaObject::invokeMethod(item, "trigger"));
                QVERIFY(sw->isVisible());
                QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
                QWindowList others;
                const QWindowList windows = QGuiApplication::topLevelWindows();
                for (QWindow *each : windows) {
                    if (each->isVisible() && each != w.window) {
                        others << each;
                    }
                }
                QCOMPARE(others.size(), 1);
                QCOMPARE(others.first(), sw);
            }
            if (way == "Escape") {
                QTest::keyClick(sw, Qt::Key_Escape);
            } else if (way == "Ctrl+W") {
                QTest::keyClick(sw, Qt::Key_W, Qt::ControlModifier);
            } else {
                // What the title bar's close button asks.
                sw->close();
            }
            QTRY_VERIFY2(!sw->isVisible(), qPrintable(way));
            QVERIFY2(w.window->property("keysToReview").toBool(), qPrintable(way));
            QCOMPARE(w.since(from), QStringList());
        }
        // The window manager gives the keys back to the window, which offscreen has none to do.
        // Closed, Settings leaves Ctrl+W to File ▸ Close, which quits.
        w.window->requestActivate();
        QVERIFY(QTest::qWaitForWindowActive(w.window));
        QTest::keyClick(w.window, Qt::Key_W, Qt::ControlModifier);
        QCOMPARE(w.since(from), (QStringList{"Interrupt", "Interrupt"}));
    }

    // While Settings is open nothing of the window's reaches review, the quit keys among them, and
    // nothing reaches the video: no key typed into Settings, where a person's keys go and where Qt
    // still matches the window's shortcuts, nor given to the window, and no menu item of review's,
    // chosen as a menu bar outside the window could choose it. Settings' own keys work, Down
    // choosing the next palette at once, and Escape closing it. Closed, the keys are review's
    // again; opened again on another palette, its button has them, so that Down moves on from it.
    void settingsHoldsBackEveryKeyOfReviewsWhileItIsOpen() {
        const auto restore = qScopeGuard([] { useColours(Colours::Default); });
        Shown w({media()});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QSignalSpy played(&w.player, &PlayerPort::reported);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.delegate(0));
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        // Every key of every menu item and shortcut the window has, but Settings' own: its menu
        // item's, and those of its window, which is the window's child.
        QList<QKeySequence> keys;
        const QList<QObject *> own = sw->findChildren<QObject *>();
        const QList<QObject *> all = w.window->findChildren<QObject *>();
        for (QObject *each : all) {
            if (own.contains(each)) {
                continue;
            }
            if (each->inherits("QQuickAction") && each->objectName() != "settings") {
                const QKeySequence key(each->property("shortcut").toString());
                if (!key.isEmpty()) {
                    keys << key;
                }
            } else if (each->inherits("QQuickShortcut")) {
                const QVariantList sequences = each->property("sequences").toList();
                for (const QVariant &sequence : sequences) {
                    keys << QKeySequence(sequence.toString());
                }
                const QKeySequence one(each->property("sequence").toString());
                if (!one.isEmpty()) {
                    keys << one;
                }
            }
        }
        for (const QKeySequence &key :
             {QKeySequence("A"), QKeySequence("Space"), QKeySequence("J"), QKeySequence("Q"),
              QKeySequence("Ctrl+W"), QKeySequence("Ctrl+Q"), QKeySequence("Down")}) {
            QVERIFY2(keys.contains(key), qPrintable(key.toString()));
        }

        QTest::keyClick(w.window, Qt::Key_Comma, Qt::ControlModifier);
        QTRY_VERIFY(sw->isVisible());
        // Settings has the keys, and in it the palette chosen.
        QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
        QTRY_COMPARE(sw->activeFocusItem(), w.settingsItem("colours0"));
        const qsizetype from = w.bridge.sent.size();
        const QString status = w.bridge.statusLine();
        for (const QKeySequence &key : std::as_const(keys)) {
            // The arrows choose palettes while Settings is open, and Ctrl+W closes it; the presses
            // after the loop prove that none of them reaches a review action either.
            if (key == QKeySequence("Down") || key == QKeySequence("Up") ||
                key == QKeySequence("Left") || key == QKeySequence("Right") ||
                key == QKeySequence("Ctrl+W")) {
                continue;
            }
            for (QQuickWindow *to : {sw, w.window}) {
                const QString sent =
                    key.toString() + (to == sw ? " in Settings" : " in the window");
                QTest::keyClick(to, key[0].key(), key[0].keyboardModifiers());
                QVERIFY2(w.since(from).isEmpty(),
                         qPrintable(sent + ": " + w.since(from).join(" | ")));
                QVERIFY2(sw->isVisible(), qPrintable(sent));
            }
        }
        // Each of review's menu items, chosen as a menu bar outside the window could choose it.
        for (QObject *each : all) {
            if (each->inherits("QQuickAction") && each->objectName() != "settings" &&
                !own.contains(each)) {
                QVERIFY(QMetaObject::invokeMethod(each, "trigger"));
                QVERIFY2(w.since(from).isEmpty(),
                         qPrintable(each->objectName() + ": " + w.since(from).join(" | ")));
            }
        }
        QCOMPARE(w.settings.colours(), "default");
        QTest::qWait(100);
        QCOMPARE(w.since(from), QStringList());
        QCOMPARE(played.count(), 0);
        QCOMPARE(finished.count(), 0);
        QCOMPARE(w.bridge.statusLine(), status);
        // Given to the window, Ctrl+W is matched, as every key is, against the shortcuts of the
        // windows Qt counts active, whichever window it is given to: Settings, which has the keys,
        // and the window it is over. Close, disabled, sends nothing, and Settings' own Ctrl+W
        // closes it.
        QTest::keyClick(w.window, Qt::Key_W, Qt::ControlModifier);
        QCOMPARE(w.since(from), QStringList());
        QTRY_VERIFY(!sw->isVisible());
        QVERIFY(QMetaObject::invokeMethod(w.find("settings"), "trigger"));
        QTRY_VERIFY(sw->isVisible());
        QTRY_COMPARE(sw->activeFocusItem(), w.settingsItem("colours0"));

        QTest::keyClick(sw, Qt::Key_Down);
        QCOMPARE(w.settings.colours(), "deuteran");
        const Tones deuteran = tones(Colours::Deuteran, dark());
        QCOMPARE(Shown::item(w.delegate(0), "rowState")->property("color").value<QColor>(),
                 deuteran.pending);
        QQuickItem *sample = w.settingsItem("sampleNow");
        QVERIFY(sample);
        QCOMPARE(Shown::item(sample, "blockBar")->property("color").value<QColor>(), deuteran.old);
        QCOMPARE(Shown::text(sample, "blockLine"), w.bridge.sample()->now());
        QCOMPARE(sw->activeFocusItem(), w.settingsItem("colours1"));
        QTest::keyClick(sw, Qt::Key_Up);
        QCOMPARE(w.settings.colours(), "default");
        QCOMPARE(sw->activeFocusItem(), w.settingsItem("colours0"));
        QTest::keyClick(sw, Qt::Key_Right);
        QCOMPARE(w.settings.colours(), "deuteran");
        QCOMPARE(sw->activeFocusItem(), w.settingsItem("colours1"));
        QTest::keyClick(sw, Qt::Key_Left);
        QCOMPARE(w.settings.colours(), "default");
        QCOMPARE(sw->activeFocusItem(), w.settingsItem("colours0"));
        QCOMPARE(w.since(from), QStringList());

        QTest::keyClick(sw, Qt::Key_Escape);
        QTRY_VERIFY(!sw->isVisible());
        // The window manager gives the keys back to the window Settings was over, which offscreen
        // has none to do.
        w.window->requestActivate();
        QVERIFY(QTest::qWaitForWindowActive(w.window));
        QTest::keyClick(w.window, Qt::Key_J);
        QCOMPARE(w.since(from), QStringList{"Move 1"});
        w.settings.setColours(QStringLiteral("protan"));
        QTest::keyClick(w.window, Qt::Key_Comma, Qt::ControlModifier);
        QTRY_VERIFY(sw->isVisible());
        QTRY_COMPARE(sw->activeFocusItem(), w.settingsItem("colours2"));
        QTest::keyClick(sw, Qt::Key_Down);
        QCOMPARE(w.settings.colours(), "tritan");
        QTest::keyClick(sw, Qt::Key_Escape);
        QTRY_VERIFY(!sw->isVisible());
    }

    // In Settings each part's size steps by half a point and says so in points; a step sets that
    // part's size at once, and the system size puts all three back.
    void settingsSetsEachPartsSizeInHalfPoints() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QTest::keyClick(w.window, Qt::Key_Comma, Qt::ControlModifier);
        QTRY_VERIFY(sw->isVisible());
        // The keys typed below go where a person's do, to Settings once it has them.
        QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
        const qreal platform = w.settings.windowTextSize();
        const std::vector<std::pair<const char *, std::function<qreal()>>> parts = {
            {"windowSize", [&w] { return w.settings.windowTextSize(); }},
            {"listSize", [&w] { return w.settings.listTextSize(); }},
            {"detailsSize", [&w] { return w.settings.detailsTextSize(); }}};
        for (const auto &[name, kept] : parts) {
            QQuickItem *box = w.settingsItem(name);
            QVERIFY2(box, name);
            QCOMPARE(box->property("value").toInt(), qRound(platform * 2));
            QCOMPARE(box->property("from").toInt(), 12);
            QCOMPARE(box->property("to").toInt(), 48);
            QCOMPARE(box->property("displayText").toString(),
                     WindowSettings::sizeWords(qRound(platform * 2)));
            QVERIFY(QMetaObject::invokeMethod(box, "forceActiveFocus"));
            QTest::keyClick(sw, Qt::Key_Up);
            QCOMPARE(kept(), platform + 0.5);
            QCOMPARE(box->property("displayText").toString(),
                     WindowSettings::sizeWords(qRound(platform * 2) + 1));
        }
        QCOMPARE(w.item("title")->property("font").value<QFont>().pointSizeF(), platform + 0.5);
        w.click(sw, w.settingsItem("systemSize"));
        QCOMPARE(w.settings.windowTextSize(), platform);
        QCOMPARE(w.settings.listTextSize(), platform);
        QCOMPARE(w.settingsItem("detailsSize")->property("value").toInt(), qRound(platform * 2));
    }

    // Each size box is as wide as the style makes a box saying the widest size, and 2 px more. A
    // step of the list's or the details panel's box leaves every width as it is: at every size from
    // the first to the last, with the window's text at the platform's size and at 24 pt, both
    // keep one width, the three boxes share it, and no size shown is cut. A step of the window's
    // own box changes the text all three are drawn in, and so all three widths alike: at each of
    // its sizes the three share one width and none is cut. Stepped by a person's keys from a whole
    // point to a half and back, a box keeps its width; and typed into, it takes no size.
    void aSizeBoxKeepsOneWidthAtEverySize() {
        Shown w({media()});
        const auto restore = qScopeGuard([&w] { w.settings.useSystemTextSizes(); });
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QVERIFY(QMetaObject::invokeMethod(w.find("settings"), "trigger"));
        QTRY_VERIFY(sw->isVisible());
        QQuickItem *window = w.settingsItem("windowSize");
        QQuickItem *list = w.settingsItem("listSize");
        QQuickItem *details = w.settingsItem("detailsSize");
        QQuickItem *widest = w.settingsItem("widestSize");
        QVERIFY(window && list && details && widest);
        const QList<QQuickItem *> boxes = {window, list, details};
        const int from = window->property("from").toInt();
        const int to = window->property("to").toInt();
        QCOMPARE(from, w.settings.leastHalfPoints());
        QCOMPARE(to, w.settings.mostHalfPoints());
        QCOMPARE(to - from + 1, 37);
        for (QQuickItem *box : boxes) {
            QVERIFY2(!box->property("editable").toBool(), qPrintable(box->objectName()));
        }
        // Whether `box` says `halfPoints`.
        const auto says = [](QQuickItem *box, int halfPoints) {
            auto *shown = box->property("contentItem").value<QQuickItem *>();
            return shown->property("text").toString() == WindowSettings::sizeWords(halfPoints);
        };
        // Whether the layout has given every box the width it asks for.
        const auto settled = [&boxes] {
            return std::all_of(boxes.begin(), boxes.end(), [](QQuickItem *box) {
                return box->width() == box->implicitWidth();
            });
        };
        // The boxes not `wide`, asked or given, not 2 px wider than the box saying the widest
        // size, or whose size is cut, each by its name, its width and what it says.
        const auto off = [&boxes, widest](qreal wide) {
            QStringList odd;
            for (QQuickItem *box : boxes) {
                auto *shown = box->property("contentItem").value<QQuickItem *>();
                if (box->width() != wide || box->implicitWidth() != wide ||
                    box->implicitWidth() != widest->implicitWidth() + 2 ||
                    shown->implicitWidth() > shown->width() + 0.5) {
                    odd << QStringLiteral("%1 %2 %3")
                               .arg(box->objectName())
                               .arg(box->width())
                               .arg(shown->property("text").toString());
                }
            }
            return odd;
        };

        for (const std::optional<qreal> windowText :
             {std::optional<qreal>(), std::optional<qreal>(24.0)}) {
            if (windowText) {
                w.settings.setWindowTextSize(*windowText);
            }
            QTRY_VERIFY(settled());
            const qreal wide = window->width();
            for (int halfPoints = from; halfPoints <= to; ++halfPoints) {
                w.settings.setListTextSize(halfPoints / 2.0);
                w.settings.setDetailsTextSize(halfPoints / 2.0);
                QTRY_VERIFY(says(list, halfPoints) && says(details, halfPoints));
                QTRY_VERIFY(settled());
                QCOMPARE(off(wide), QStringList());
            }
        }
        for (int halfPoints = from; halfPoints <= to; ++halfPoints) {
            w.settings.setWindowTextSize(halfPoints / 2.0);
            QTRY_VERIFY(says(window, halfPoints));
            QTRY_VERIFY(settled());
            QCOMPARE(off(window->width()), QStringList());
        }

        QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
        w.settings.setListTextSize(20);
        QTRY_VERIFY(says(list, 40));
        QTRY_VERIFY(settled());
        const qreal wide = list->width();
        QVERIFY(QMetaObject::invokeMethod(list, "forceActiveFocus"));
        QTRY_VERIFY(list->hasActiveFocus());
        for (const auto &[key, halfPoints] :
             {std::pair(Qt::Key_Up, 41), std::pair(Qt::Key_Down, 40)}) {
            QTest::keyClick(sw, key);
            QTRY_VERIFY(says(list, halfPoints));
            QCOMPARE(w.settings.listTextSize(), halfPoints / 2.0);
            QTRY_VERIFY(settled());
            QCOMPARE(off(wide), QStringList());
        }
        // Its words chosen whole, so that digits typed would take their place were it editable.
        QTest::keyClick(sw, Qt::Key_A, Qt::ControlModifier);
        for (const Qt::Key key : {Qt::Key_1, Qt::Key_2, Qt::Key_Return}) {
            QTest::keyClick(sw, key);
        }
        QTest::qWait(50);
        QCOMPARE(list->property("value").toInt(), 40);
        QCOMPARE(w.settings.listTextSize(), 20.0);
        QVERIFY(says(list, 40));
    }

    // Every setting is in reach at the largest sizes, on a screen shorter than Settings at 24 pt.
    // Raised to 24 pt while Settings is open, its window grows at once, so that no palette's name,
    // size or button is cut short or reaches past its edge, but grows no taller than four fifths
    // of the screen: what that has no room for scrolls into view, Use the System Size the last of
    // it. Closed and opened again, it opens at that size. At each size all of it shows, or its
    // window is as tall as it is allowed.
    void everySettingIsInReachAtTheLargestSizes() {
        Shown w({media()});
        const auto restore = qScopeGuard([&w] { w.settings.useSystemTextSizes(); });
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *open = w.find("settings");
        QVERIFY(open);
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QQuickItem *scroll = w.settingsItem("settingsScroll");
        QVERIFY(scroll);
        // The window at its least size on the shorter of the tests' two screens, and Settings
        // with it, where a window manager would place it, over the window: offscreen has none, and
        // leaves a window where it is put.
        const QList<QScreen *> screens = QGuiApplication::screens();
        QScreen *shorter = *std::min_element(screens.begin(), screens.end(), [](auto *a, auto *b) {
            return a->geometry().height() < b->geometry().height();
        });
        w.window->resize(w.window->minimumSize());
        w.window->setPosition(shorter->availableGeometry().topLeft());
        sw->setScreen(shorter);
        sw->setPosition(shorter->availableGeometry().topLeft());
        QTRY_COMPARE(w.window->screen(), shorter);
        const int allowed = qFloor(0.8 * shorter->geometry().height());
        // The settings cut short or reaching past the window's right edge, each by its name.
        const auto cut = [&w, sw] {
            QStringList off;
            for (const char *name : {"colours0", "colours1", "colours2", "colours3", "colours4",
                                     "windowSize", "listSize", "detailsSize", "systemSize"}) {
                QQuickItem *setting = w.settingsItem(name);
                if (!setting) {
                    off << QStringLiteral("no %1").arg(name);
                } else if (setting->width() < setting->implicitWidth() - 0.5 ||
                           setting->mapToScene(QPointF(setting->width(), 0)).x() >
                               sw->width() + 0.5) {
                    off << QString::fromLatin1(name);
                }
            }
            return off;
        };
        const auto allOrAsTallAsAllowed = [sw, scroll, allowed] {
            return scroll->property("contentHeight").toReal() <= scroll->height() + 0.5 ||
                   sw->height() >= allowed;
        };
        // Whether all of `of` is inside the scroll view, as the scene draws them.
        const auto inView = [scroll](QQuickItem *of) {
            const QRectF at = of->mapRectToScene(QRectF(0, 0, of->width(), of->height()));
            const QRectF in =
                scroll->mapRectToScene(QRectF(0, 0, scroll->width(), scroll->height()));
            return at.left() >= in.left() - 0.5 && at.top() >= in.top() - 0.5 &&
                   at.right() <= in.right() + 0.5 && at.bottom() <= in.bottom() + 0.5;
        };

        QVERIFY(QMetaObject::invokeMethod(open, "trigger"));
        QTRY_VERIFY(sw->isVisible());
        QCOMPARE(sw->screen(), shorter);
        QTRY_COMPARE(cut(), QStringList());
        QVERIFY(allOrAsTallAsAllowed());

        w.settings.setWindowTextSize(24);
        w.settings.setListTextSize(24);
        w.settings.setDetailsTextSize(24);
        QTRY_COMPARE(cut(), QStringList());
        QVERIFY2(sw->height() <= allowed,
                 qPrintable(QStringLiteral("%1 tall, %2 allowed").arg(sw->height()).arg(allowed)));
        QVERIFY(allOrAsTallAsAllowed());
        const qreal contentHeight = scroll->property("contentHeight").toReal();
        QVERIFY2(
            contentHeight > scroll->height(),
            qPrintable(
                QStringLiteral("%1 of content in %2").arg(contentHeight).arg(scroll->height())));
        auto *view = scroll->property("contentItem").value<QQuickItem *>();
        QVERIFY(view);
        view->setProperty("contentY", contentHeight - scroll->height());
        QTRY_VERIFY(inView(w.settingsItem("systemSize")));
        QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
        QTest::keyClick(sw, Qt::Key_Escape);
        QTRY_VERIFY(!sw->isVisible());

        QVERIFY(QMetaObject::invokeMethod(open, "trigger"));
        QTRY_VERIFY(sw->isVisible());
        QCOMPARE(sw->screen(), shorter);
        QTRY_COMPARE(cut(), QStringList());
        QVERIFY(allOrAsTallAsAllowed());
    }

    // Made larger and closed, Settings opens again at its contents' size.
    void settingsOpensAgainAtItsContentsSize() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *open = w.find("settings");
        QVERIFY(open);
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QVERIFY(QMetaObject::invokeMethod(open, "trigger"));
        QTRY_VERIFY(sw->isVisible());
        const QSize least = sw->minimumSize();
        QCOMPARE(sw->size(), least);
        sw->resize(least + QSize(120, 80));
        // Once the window has taken the size, as it takes a person's drag.
        QTRY_COMPARE(sw->contentItem()->size(), QSizeF(least + QSize(120, 80)));
        sw->close();
        QTRY_VERIFY(!sw->isVisible());
        QVERIFY(QMetaObject::invokeMethod(open, "trigger"));
        QTRY_VERIFY(sw->isVisible());
        QCOMPARE(sw->minimumSize(), least);
        QCOMPARE(sw->size(), least);
    }

    // Settings' labels sit level with the first line of what they name, whatever the sizes of the
    // two: Palette with the first palette's name, Sample with the sample's first line.
    void settingsLabelsAreLevelWithWhatTheyName() {
        Shown w({media()});
        const auto restore = qScopeGuard([&w] { w.settings.useSystemTextSizes(); });
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QVERIFY(QMetaObject::invokeMethod(w.find("settings"), "trigger"));
        QTRY_VERIFY(sw->isVisible());
        const auto baseline = [](QQuickItem *of) {
            return of->mapToScene(QPointF(0, of->baselineOffset())).y();
        };
        // How far each label's baseline is from that of the first line of what it names, by the
        // label's name: the first palette's, and the sample's label, the first line in it.
        const auto off = [&w, &baseline] {
            QStringList apart;
            QQuickItem *sample = w.settingsItem("sampleNow");
            const std::vector<std::pair<const char *, QQuickItem *>> pairs = {
                {"paletteLabel", w.settingsItem("colours0")},
                {"sampleLabel", sample ? Shown::item(sample, "blockLabel") : nullptr}};
            for (const auto &[label, named] : pairs) {
                QQuickItem *beside = w.settingsItem(label);
                if (!beside || !named) {
                    apart << QStringLiteral("%1 or what it names missing").arg(label);
                } else if (qAbs(baseline(beside) - baseline(named)) > 1) {
                    apart << QStringLiteral("%1 %2").arg(label).arg(baseline(beside) -
                                                                    baseline(named));
                }
            }
            return apart;
        };
        QTRY_COMPARE(off(), QStringList());
        w.settings.setDetailsTextSize(24);
        QTRY_COMPARE(off(), QStringList());
        w.settings.setWindowTextSize(24);
        w.settings.setDetailsTextSize(6);
        QTRY_COMPARE(off(), QStringList());
    }

    // Review ending in an error while Settings is open closes Settings, which would otherwise
    // hold the keys from the window the error's dialog is in, and Settings cannot open again over
    // the dialog: the dialog opens, and Escape in the window closes it, which ends the window
    // with 1.
    void aFatalEndClosesSettingsForItsDialog() {
        Shown w({"no-such-run"});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        // Review not started until Settings is open.
        QVERIFY(w.show());
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QObject *settings = w.find("settings");
        QVERIFY(settings);
        QVERIFY(settings->property("enabled").toBool());
        QVERIFY(QMetaObject::invokeMethod(settings, "trigger"));
        QTRY_VERIFY(sw->isVisible());
        QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
        QObject *fatal = w.find("fatal");
        QVERIFY(fatal);
        QVERIFY(!fatal->property("visible").toBool());
        w.bridge.start();
        QTRY_VERIFY(!sw->isVisible());
        QTRY_VERIFY(fatal->property("opened").toBool());
        QCOMPARE(finished.count(), 0);
        // Chosen now, as a menu bar that still offers it could, Settings stays closed.
        QVERIFY(!settings->property("enabled").toBool());
        QVERIFY(QMetaObject::invokeMethod(settings, "trigger"));
        QVERIFY(!sw->isVisible());
        // The window manager gives the keys back to the window Settings was over, which offscreen
        // has none to do.
        w.window->requestActivate();
        QVERIFY(QTest::qWaitForWindowActive(w.window));
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 1);
    }

    // What Settings chooses is there when the next window over the same settings starts: its rows
    // are drawn in the kept palette and at the kept list size.
    void whatSettingsChoosesIsThereAtTheNextStart() {
        const auto restore = qScopeGuard([] { useColours(Colours::Default); });
        QTemporaryDir kept;
        QVERIFY(kept.isValid());
        {
            Shown w({media()}, kept.path());
            QVERIFY(w.start());
            QTRY_COMPARE(w.rows(), 2);
            w.settings.setColours(QStringLiteral("protan"));
            w.settings.setListTextSize(14);
        }
        // The palette drawn with is the process's, and outlives the window: put back to the
        // default, only the kept file can bring Protan back.
        useColours(Colours::Default);
        Shown w({media()}, kept.path());
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QCOMPARE(w.settings.colours(), "protan");
        QTRY_VERIFY(w.delegate(0));
        QQuickItem *row = w.delegate(0);
        QCOMPARE(Shown::item(row, "rowState")->property("color").value<QColor>(),
                 tones(Colours::Protan, dark()).pending);
        QCOMPARE(Shown::item(row, "rowLine")->property("font").value<QFont>().pointSizeF(), 14.0);
    }

    // Unset, a part's size is the platform's, which follows the desktop's font as it changes.
    void theWindowFollowsThePlatformsFontWhileItsSizeIsUnset() {
        const auto restore =
            qScopeGuard([was = QGuiApplication::font()] { QGuiApplication::setFont(was); });
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QFont larger = QGuiApplication::font();
        const qreal platform = w.settings.windowTextSize();
        larger.setPointSizeF(larger.pointSizeF() + 3);
        QSignalSpy changed(&w.settings, &WindowSettings::textSizesChanged);
        QGuiApplication::setFont(larger);
        QTRY_COMPARE(w.item("title")->property("font").value<QFont>().pointSizeF(), platform + 3);
        // Qt may send the event to each window too, which must not say it again.
        QTest::qWait(50);
        QCOMPARE(changed.count(), 1);
    }

    // Esc and OK each close the dialog, which ends the window with 1; a click beside it does not.
    void aFatalEndShowsItsDialogAndClosingItEndsWithOne() {
        for (const bool byButton : {false, true}) {
            Shown w({"no-such-run"});
            QSignalSpy finished(&w.bridge, &SessionBridge::finished);
            QVERIFY(w.start());
            QObject *dialog = w.find("fatal");
            QVERIFY(dialog);
            QTRY_VERIFY(dialog->property("opened").toBool());
            QCOMPARE(w.err.str(), "subcheck-qt: no run named no-such-run\n");
            QCOMPARE(finished.count(), 0);
            QTest::mouseClick(w.window, Qt::LeftButton, {}, QPoint(4, 4));
            QVERIFY(dialog->property("opened").toBool());
            QCOMPARE(finished.count(), 0);
            if (byButton) {
                auto *footer = dialog->property("footer").value<QQuickItem *>();
                QVERIFY(footer);
                QQuickItem *ok = Shown::button(footer, QStringLiteral("OK"));
                QVERIFY(ok);
                w.click(ok);
            } else {
                QTest::keyClick(w.window, Qt::Key_Escape);
            }
            QTRY_COMPARE(finished.count(), 1);
            QCOMPARE(finished[0][0].toInt(), 1);
        }
    }
};

SUBCHECK_OFFSCREEN_TEST_MAIN(WindowTest)
#include "window_test.moc"
