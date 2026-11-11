// The real window, its QML loaded from the module as the app loads it, offscreen, on real sessions
// over the run seeded for it: keys pressed and a row clicked as a person would, through the QML's
// handlers to review and back. Every test leaves the run as it found it.
#include "session_bridge.h"

#include <QQmlApplicationEngine>
#include <QQuickItem>
#include <QQuickWindow>
#include <QRegularExpression>
#include <QSignalSpy>
#include <QTest>
#include <QtQml/QQmlExtensionPlugin>

#include <chrono>
#include <sstream>
#include <string>
#include <vector>

Q_IMPORT_QML_PLUGIN(SubcheckPlugin)

namespace {

std::string media() { return qEnvironmentVariable("SUBCHECK_TEST_MEDIA").toStdString(); }

std::string clean() { return qEnvironmentVariable("SUBCHECK_TEST_CLEAN").toStdString(); }

// The window over a bridge on `selector`, made as main.cpp makes it but for a short wait after a
// quit, as in the bridge's tests: the store is the test's own, so no lock held elsewhere slows
// review's end, and a review that does not end fails its test sooner.
struct Shown {
    std::ostringstream out;
    std::ostringstream err;
    SessionBridge bridge;
    QQmlApplicationEngine engine;
    QQuickWindow *window = nullptr;

    explicit Shown(std::vector<std::string> selector)
        : bridge(std::move(selector), out, err, std::chrono::seconds(1)) {
        engine.setInitialProperties({{QStringLiteral("bridge"), QVariant::fromValue(&bridge)}});
        engine.loadFromModule("Subcheck", "Main");
        if (!engine.rootObjects().isEmpty()) {
            window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        }
    }

    // The session started, and the window shown and given the keys, as a person's would be.
    bool start() {
        bridge.start();
        if (!window || !QTest::qWaitForWindowExposed(window)) {
            return false;
        }
        window->requestActivate();
        return QTest::qWaitForWindowActive(window);
    }

    QObject *find(const char *name) const { return window->findChild<QObject *>(name); }

    int rows() { return bridge.findings()->rowCount(); }

    QString decision(int row) {
        FindingsModel *list = bridge.findings();
        return list->data(list->index(row), FindingsModel::StateRole).toString();
    }

    // The delegate showing `row`, wherever the list has put it.
    QQuickItem *delegate(int row) const {
        auto *content = find("findings")->property("contentItem").value<QQuickItem *>();
        const QList<QQuickItem *> items = content->childItems();
        for (QQuickItem *item : items) {
            if (item->property("index").toInt() == row) {
                return item;
            }
        }
        return nullptr;
    }

    void click(QQuickItem *item) {
        const QPointF middle(item->width() / 2, item->height() / 2);
        QTest::mouseClick(window, Qt::LeftButton, {}, item->mapToScene(middle).toPoint());
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
        QVERIFY(list && status);
        QObject *empty = w.find("empty");
        QVERIFY(empty && !empty->property("visible").toBool());
        QCOMPARE(status->property("text").toString(), "4 pending · 0 accepted · 0 rejected");

        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 1);
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 4);
        QTest::keyClick(w.window, Qt::Key_K);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 0);
        QTest::keyClick(w.window, Qt::Key_A);
        QTRY_COMPARE(w.decision(0), "accepted");
        QTRY_COMPARE(status->property("text").toString(), "accepted 2 findings");
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.decision(0), "pending");
        QTest::keyClick(w.window, Qt::Key_R);
        QTRY_COMPARE(w.decision(0), "rejected");
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.decision(0), "pending");

        // The last of the expanded rows' members.
        QQuickItem *member = w.delegate(3);
        QVERIFY(member);
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
        // Read once the j's answer is in, which lets the accept's message go, so the counts show;
        // checked after the undo, so that the run is left as found whatever it shows.
        QTest::keyClick(w.window, Qt::Key_A);
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(w.bridge.statusLine(), "2 pending · 2 accepted · 0 rejected");
        const int landed = list->property("currentIndex").toInt();
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.decision(0), "pending");
        QCOMPARE(landed, 2);
        QTest::keyClick(w.window, Qt::Key_J);
        QTest::keyClick(w.window, Qt::Key_J);
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 3);
        QTest::keyClick(w.window, Qt::Key_K);
        QTest::keyClick(w.window, Qt::Key_K);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 1);
    }

    // The window stays until review has ended, and the counts are printed.
    void closingTheWindowQuits() {
        Shown w({media()});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->close();
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 0);
        QVERIFY(w.window->isVisible());
        QCOMPARE(w.out.str(), "media: 0 accepted, 0 rejected, 4 pending\n");
    }

    void anEmptyRunSaysSo() {
        Shown w({clean()});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QVERIFY(w.start());
        QObject *empty = w.find("empty");
        QVERIFY(empty);
        QTRY_VERIFY(empty->property("visible").toBool());
        QTest::keyClick(w.window, Qt::Key_Q);
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(w.out.str(), "clean: 0 accepted, 0 rejected, 0 pending\n");
    }

    void aFatalEndShowsItsDialogAndClosingItEndsWithOne() {
        Shown w({"no-such-run"});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QVERIFY(w.start());
        QObject *dialog = w.find("fatal");
        QVERIFY(dialog);
        QTRY_VERIFY(dialog->property("opened").toBool());
        QCOMPARE(w.err.str(), "subcheck-qt: no run named no-such-run\n");
        QCOMPARE(finished.count(), 0);
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 1);
    }
};

QTEST_MAIN(WindowTest)
#include "window_test.moc"
