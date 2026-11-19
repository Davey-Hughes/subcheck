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
#include <QKeyEvent>
#include <QKeySequence>
#include <QQmlApplicationEngine>
#include <QQmlComponent>
#include <QQuickItem>
#include <QQuickWindow>
#include <QRegularExpression>
#include <QScopeGuard>
#include <QSignalSpy>
#include <QStandardPaths>
#include <QStyleHints>
#include <QTemporaryDir>
#include <QTest>
#include <QtMath>
#include <QtQml/QQmlExtensionPlugin>

#include <algorithm>
#include <chrono>
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

    // The window shown, as main.cpp shows it, the session started, and the window active, as a
    // person's would be.
    bool start() {
        if (!window) {
            return false;
        }
        settings.show(window);
        bridge.start();
        if (!QTest::qWaitForWindowExposed(window)) {
            return false;
        }
        window->requestActivate();
        return QTest::qWaitForWindowActive(window);
    }

    QObject *find(const char *name) const { return window->findChild<QObject *>(name); }

    // The item called `name` in what the window draws, a Repeater's among them, which are no
    // QObject's children.
    QQuickItem *item(const char *name) const { return item(window->contentItem(), name); }

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

    void click(QQuickItem *of) { QTest::mouseClick(window, Qt::LeftButton, {}, middle(of)); }

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
            {"&File", 2}, {"&Edit", 1}, {"&View", 2}, {"&Show", 4}, {"F&inding", 8}, {"Vi&deo", 9}};
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
        QCOMPARE(Shown::text(panel, "nowHeading"), "In the subtitles now");
        QCOMPARE(Shown::text(panel, "now"), detail->now());
        QCOMPARE(Shown::text(panel, "fixedHeading"), "With the fix");
        QCOMPARE(Shown::text(panel, "fixed"), detail->fixed());
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
        QTRY_COMPARE(w.window->width(), 772);
        QQuickItem *title = w.item("title");
        QQuickItem *undo = w.item("undoButton");
        QVERIFY(title && undo);
        auto *toolbar = w.window->property("header").value<QQuickItem *>();
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
        QTRY_COMPARE(w.window->width(), 772);
        QQuickItem *sidebar = w.item("sidebar");
        QQuickItem *list = w.item("findings");
        QQuickItem *details = w.item("details");
        QTRY_COMPARE(w.item("columns")->width(), 772);
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
            QCOMPARE(qRound(list->width()), 576);
            w.window->resize(1100, 700);
            QTRY_COMPARE(qRound(list->width()), 396);
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
            w.window->resize(800, 800);
            QTRY_COMPARE(qRound(list->width()), 260);
            QTRY_VERIFY(sidebar->width() < 220 && details->width() < 480);
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
