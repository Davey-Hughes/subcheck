// The app end to end, as a person runs it: the real subcheck-qt on an X display of the test's own
// (Xvfb, drawn with Mesa's software OpenGL), real keys and clicks from that display's server
// (xdotool), and real mpv playing timing.mkv or apply.mkv, fixtures with video, which says where it
// is over the socket the test's mpvqt.conf gives it. Linux only: Xvfb and xdotool are X11's.
#include "desktop_session.h"

#include <QDir>
#include <QElapsedTimer>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLocalSocket>
#include <QProcess>
#include <QScopeGuard>
#include <QTest>

#include <cmath>
#include <csignal>
#include <sys/prctl.h>

namespace {

QString env(const char *name) { return qEnvironmentVariable(name); }

// mpv's answer to a read of the property `name`: its `data`, or its `error`; empty when the socket
// is not there yet, or gave no answer.
QJsonObject ask(const QString &socket, const QString &name) {
    QLocalSocket mpv;
    mpv.connectToServer(socket);
    if (!mpv.waitForConnected(1000)) {
        return {};
    }
    const QJsonObject request{{"command", QJsonArray{"get_property", name}}, {"request_id", 1}};
    mpv.write(QJsonDocument(request).toJson(QJsonDocument::Compact) + '\n');
    QElapsedTimer waited;
    waited.start();
    while (waited.elapsed() < 2000) {
        if (!mpv.canReadLine() && !mpv.waitForReadyRead(100)) {
            continue;
        }
        // An event's line may come before the answer.
        const QJsonObject reply = QJsonDocument::fromJson(mpv.readLine()).object();
        if (reply.value("request_id").toInt() == 1) {
            return reply;
        }
    }
    return {};
}

// What every process the test starts runs with: its own config, runtime and cache directories
// under `dir`, so that nothing of the user's mpv is read and nothing (Mesa's shader cache among
// it) is written into the user's; and none of the variables desktop_session.h names as a desktop
// session's, which the container the gui job runs in has none of either.
QProcessEnvironment isolated(const QString &dir) {
    QProcessEnvironment e = QProcessEnvironment::systemEnvironment();
    for (const QString &name : e.keys()) {
        if (isDesktopSession(name.toStdString())) {
            e.remove(name);
        }
    }
    e.insert("XDG_CONFIG_HOME", dir + "/config");
    e.insert("XDG_RUNTIME_DIR", dir + "/run");
    e.insert("XDG_CACHE_HOME", dir + "/cache");
    return e;
}

// A test that crashes, or that CTest's timeout kills, takes the processes it started with it.
void diesWithTheTest(QProcess &process) {
    process.setChildProcessModifier([] { prctl(PR_SET_PDEATHSIG, SIGTERM); });
}

// Once the test has failed, `process` ended and what it wrote on stderr printed: on a runner no one
// can step through, Qt's, MpvQt's and the X server's own words are the diagnosis. Ended first, so
// that all it wrote is read; terminated, not killed, so that Xvfb removes its socket and lock. A
// pass prints nothing.
void showStderrIfFailed(QProcess &process, const char *name) {
    if (!QTest::currentTestFailed()) {
        return;
    }
    process.terminate();
    process.waitForFinished(5000);
    qWarning().noquote().nospace() << name << " wrote on stderr:\n"
                                   << QString::fromUtf8(process.readAllStandardError());
}

} // namespace

class EndToEndTest : public QObject {
    Q_OBJECT

    QString dir = env("SUBCHECK_TEST_DIR");
    // Where mpv says where it is, as the test's mpvqt.conf tells it.
    QString socket = dir + "/mpv.sock";
    QProcess xvfb;
    QString display;

    // xdotool on the test's display, within 15 s; what it printed goes to `out`.
    bool xdotool(const QStringList &args, QString *out = nullptr) {
        QProcess tool;
        diesWithTheTest(tool);
        QProcessEnvironment e = isolated(dir);
        e.insert("DISPLAY", display);
        tool.setProcessEnvironment(e);
        tool.start(env("SUBCHECK_XDOTOOL"), args);
        const bool ran = tool.waitForFinished(15000) && tool.exitStatus() == QProcess::NormalExit &&
                         tool.exitCode() == 0;
        if (out) {
            *out = QString::fromUtf8(tool.readAllStandardOutput()).trimmed();
        }
        return ran;
    }

    // The run seeded afresh from core's fixture `fixture`, at the start of each test: each decides
    // findings in it, so that none rests on another's decisions, and a run of the test after
    // another, as `ctest --repeat` makes, opens the run as the scan made it. The seed starts `dir`
    // anew, so the directories the test's processes run with are made after it.
    void seed(const QString &fixture) {
        QProcess scan;
        diesWithTheTest(scan);
        scan.setProcessEnvironment(isolated(dir));
        scan.setProcessChannelMode(QProcess::MergedChannels);
        scan.start(env("SUBCHECK_CMAKE"), {"-D", "SUBCHECK=" + env("SUBCHECK_CLI"), "-D",
                                           "REPO_ROOT=" + env("SUBCHECK_REPO"), "-D", "DIR=" + dir,
                                           "-D", "FIXTURE=" + fixture, "-P", env("SUBCHECK_SEED")});
        QVERIFY2(scan.waitForFinished(30000) && scan.exitStatus() == QProcess::NormalExit &&
                     scan.exitCode() == 0,
                 qPrintable(QString::fromUtf8(scan.readAll())));
        QVERIFY(QDir().mkpath(dir + "/config/mpvqt") && QDir().mkpath(dir + "/run") &&
                QDir().mkpath(dir + "/cache"));
        QVERIFY(QFile::setPermissions(dir + "/run", QFileDevice::ReadOwner |
                                                        QFileDevice::WriteOwner |
                                                        QFileDevice::ExeOwner));
    }

    // The app on the test's display and nowhere else, its window found and given the keys. mpv's
    // config, in place of the user's: no sound, and a socket that says where it is; and what a
    // user's could hold that would land a load on a keyframe, but for the options review gives
    // each load. Qt's warnings go to the app's stderr, where the test reads them.
    void start(QProcess &app) {
        QFile config(dir + "/config/mpvqt/mpvqt.conf");
        QVERIFY(config.open(QIODevice::WriteOnly | QIODevice::Truncate));
        config.write("ao=null\nhr-seek=no\ninput-ipc-server=" + socket.toUtf8() + "\n");
        config.close();
        QProcessEnvironment e = isolated(dir);
        e.insert("DISPLAY", display);
        e.insert("QT_QPA_PLATFORM", "xcb");
        e.insert("QT_FORCE_STDERR_LOGGING", "1");
        diesWithTheTest(app);
        app.setProcessEnvironment(e);
        app.start(env("SUBCHECK_QT"), {env("SUBCHECK_TEST_MEDIA")});
        QVERIFY(app.waitForStarted());
        QString windows;
        QVERIFY(
            xdotool({"search", "--sync", "--onlyvisible", "--classname", "subcheck-qt"}, &windows));
        QVERIFY(xdotool({"windowfocus", "--sync", windows.section('\n', 0, 0)}));
    }

    // What the app and the X server wrote on stderr, once the test has failed. Made after the app,
    // so that it runs while the app is still there; and the app's first, while its display is
    // still there, so that its last words are not the display's loss.
    [[nodiscard]] auto stderrIfFailed(QProcess &app) {
        return qScopeGuard([this, &app] {
            showStderrIfFailed(app, "subcheck-qt");
            showStderrIfFailed(xvfb, "Xvfb");
        });
    }

    // Quit: review ends and the app exits 0, as main.cpp wires it, with no word of its QML's on
    // stderr.
    void quit(QProcess &app) {
        QVERIFY(xdotool({"key", "q"}));
        QVERIFY(app.waitForFinished(15000));
        QCOMPARE(app.exitStatus(), QProcess::NormalExit);
        QCOMPARE(app.exitCode(), 0);
        const QString err = QString::fromUtf8(app.readAllStandardError());
        QVERIFY2(!err.contains(".qml:"), qPrintable(err));
    }

private slots:
    void initTestCase() {
        // The app decides findings in whatever store it opens: without CTest's environment, that
        // would be the user's own. The seed deletes the test's directory and makes it anew, so
        // that directory may only be the one CMake made for the test in its build directory, whose
        // path the test is built with: any other, a folder of the user's say, fails the test here
        // with nothing deleted.
        QVERIFY2(dir == QString::fromUtf8(SUBCHECK_E2E_DIR) &&
                     qEnvironmentVariable("SUBCHECK_DB") == dir + "/subcheck.db",
                 "run through ctest, on the directory CMake made for the test, " SUBCHECK_E2E_DIR
                 ", and the store in it");
        // A display of the test's own, never the desktop's: Xvfb takes the first free number, and
        // says which once it is listening.
        diesWithTheTest(xvfb);
        xvfb.setProcessEnvironment(isolated(dir));
        xvfb.start(env("SUBCHECK_XVFB"),
                   {"-displayfd", "1", "-screen", "0", "1280x800x24", "-nolisten", "tcp"});
        const auto shown = qScopeGuard([this] { showStderrIfFailed(xvfb, "Xvfb"); });
        QVERIFY(xvfb.waitForReadyRead(10000));
        display = ":" + QString::fromUtf8(xvfb.readLine()).trimmed();
    }

    void cleanupTestCase() {
        xvfb.terminate();
        xvfb.waitForFinished(5000);
    }

    void aRowIsAcceptedThenItsMomentPlays() {
        seed("timing.mkv");
        if (QTest::currentTestFailed()) {
            return;
        }
        QProcess app;
        const auto shown = stderrIfFailed(app);
        start(app);
        if (QTest::currentTestFailed()) {
            return;
        }

        // The first row accepted, which moves on to the second; back to the first, and played: a
        // timing finding's, on the frame that shows it, with its track's subtitles, and paused, as
        // mpv had no file open.
        QVERIFY(xdotool({"key", "a", "k", "o"}));
        QTRY_COMPARE_WITH_TIMEOUT(ask(socket, "time-pos").value("data").toDouble(), 2.5, 10000);
        QCOMPARE(ask(socket, "path").value("data").toString(),
                 env("SUBCHECK_TEST_MEDIA") + "/timing.mkv");
        QCOMPARE(ask(socket, "pause").value("data").toBool(), true);
        QCOMPARE(ask(socket, "sid").value("data").toInt(), 4);
        // mpv's video output is configured, which takes the pane's render context. Waited for, as
        // mpv configures it only once it has decoded the first frame, and the load's start option
        // can make time-pos read the moment's time before then.
        QTRY_COMPARE_WITH_TIMEOUT(ask(socket, "vo-configured").value("data").toBool(), true, 5000);
        // Its audio output is the test config's: none.
        QCOMPARE(ask(socket, "ao").value("data")[0]["name"].toString(), "null");
        // A frame on, at the fixture's 2 per second; then a second back, exactly.
        QVERIFY(xdotool({"key", "bracketright"}));
        QTRY_COMPARE_WITH_TIMEOUT(ask(socket, "time-pos").value("data").toDouble(), 3.0, 5000);
        QVERIFY(xdotool({"key", "comma"}));
        QTRY_COMPARE_WITH_TIMEOUT(ask(socket, "time-pos").value("data").toDouble(), 2.0, 5000);
        // Space plays. Paused still, as `o` opened it, through the frame step and the seek, so that
        // only Space can make the check after it pass.
        QCOMPARE(ask(socket, "pause").value("data").toBool(), true);
        QVERIFY(xdotool({"key", "space"}));
        QTRY_COMPARE_WITH_TIMEOUT(ask(socket, "pause").value("data").toBool(), false, 5000);
        // Closed: mpv answers, and has no file.
        QVERIFY(xdotool({"key", "O"}));
        QTRY_COMPARE_WITH_TIMEOUT(ask(socket, "path").value("error").toString(),
                                  "property unavailable", 5000);

        // Quit, and the app prints its counts.
        quit(app);
        if (QTest::currentTestFailed()) {
            return;
        }
        QCOMPARE(app.readAllStandardOutput(), "media: 1 accepted, 0 rejected, 1 pending\n");
    }

    // Out by View ▸ Pop Out Video, by its mnemonics, with a timing finding's moment showing: the
    // keys typed in the pop-out reach mpv as the main window's do, mpv's video output configured
    // still; Ctrl+W there puts the video back rather than quit, and the main window has the keys
    // again.
    void theVideoPopsOutAndBackAndItsKeysStillReachMpv() {
        seed("timing.mkv");
        if (QTest::currentTestFailed()) {
            return;
        }
        QProcess app;
        const auto shown = stderrIfFailed(app);
        start(app);
        if (QTest::currentTestFailed()) {
            return;
        }

        // Review selects the first row as the view opens, under All.
        QVERIFY(xdotool({"key", "o"}));
        QTRY_COMPARE_WITH_TIMEOUT(ask(socket, "time-pos").value("data").toDouble(), 2.5, 10000);
        QVERIFY(xdotool({"key", "alt+v", "p"}));
        // Top-level windows only: by its name alone, the search finds the video's window held too,
        // a child of the main window, which keeps the pop-out's title.
        const QStringList outside{"--onlyvisible", "--maxdepth", "1", "--name", " · subcheck$"};
        QString out;
        QVERIFY(xdotool(QStringList{"search", "--sync"} + outside, &out));
        QVERIFY(xdotool({"windowfocus", "--sync", out.section('\n', 0, 0)}));
        // A frame on, from the pop-out.
        QVERIFY(xdotool({"key", "bracketright"}));
        QTRY_COMPARE_WITH_TIMEOUT(ask(socket, "time-pos").value("data").toDouble(), 3.0, 5000);
        QCOMPARE(ask(socket, "vo-configured").value("data").toBool(), true);
        QVERIFY(xdotool({"key", "ctrl+w"}));
        QTRY_VERIFY_WITH_TIMEOUT(!xdotool(QStringList{"search"} + outside), 5000);
        QCOMPARE(ask(socket, "vo-configured").value("data").toBool(), true);
        // Pop In gave the main window back the keys: `,`, sent to the window that has them, seeks
        // a second back from there.
        QVERIFY(xdotool({"key", "comma"}));
        QTRY_COMPARE_WITH_TIMEOUT(ask(socket, "time-pos").value("data").toDouble(), 2.0, 5000);
        quit(app);
    }

    // A click on the control bar's Play/Pause while an edit is open reaches the button, and leaves
    // the keys with the edit's field: the video's window, held, takes no focus, so that the next
    // letter goes on into the wording rather than run one of the window's keys, q's quit among
    // them. The one test that sees the platform's own focus follow a real click. On apply.mkv, the
    // fixture with video and a planted typo, as timing.mkv and ties.mkv have none.
    void aClickOnTheBarLeavesAnOpenEditItsKeys() {
        seed("apply.mkv");
        if (QTest::currentTestFailed()) {
            return;
        }
        QProcess app;
        const auto shown = stderrIfFailed(app);
        start(app);
        if (QTest::currentTestFailed()) {
            return;
        }

        // The typo's row, selected by the Find box, which Escape leaves.
        QVERIFY(xdotool({"key", "ctrl+f"}));
        QVERIFY(xdotool({"type", "befor"}));
        QVERIFY(xdotool({"key", "Return", "Escape"}));
        QVERIFY(xdotool({"key", "o"}));
        QTRY_COMPARE_WITH_TIMEOUT(ask(socket, "vo-configured").value("data").toBool(), true, 10000);
        QVERIFY(xdotool({"key", "e"}));
        // The video's window held, a child of the main window, which a search by name finds at any
        // depth; and where it is on the screen. Play/Pause is first in the bar, at its top left,
        // and the bar is under the video, 16:9 at the window's width.
        QString held;
        QVERIFY(xdotool({"search", "--sync", "--onlyvisible", "--name", " · subcheck$"}, &held));
        QString shell;
        QVERIFY(xdotool({"getwindowgeometry", "--shell", held.section('\n', 0, 0)}, &shell));
        const auto value = [&shell](const QString &name) {
            for (const QString &line : shell.split('\n')) {
                if (line.startsWith(name + '=')) {
                    return line.section('=', 1).toInt();
                }
            }
            return -1;
        };
        const int x = value("X");
        const int y = value("Y");
        const int width = value("WIDTH");
        QVERIFY2(x >= 0 && y >= 0 && width > 0, qPrintable(shell));
        QVERIFY(xdotool({"mousemove", "--sync", QString::number(x + 24),
                         QString::number(y + std::lround(width * 9.0 / 16) + 16), "click", "1"}));
        // The click reached the button.
        QTRY_COMPARE_WITH_TIMEOUT(ask(socket, "pause").value("data").toBool(), false, 5000);
        // And the field kept the keys: "!q" goes into the wording, and the app runs on, where the
        // window, given the keys by the click, would quit on the q. Typed once mpv has answered the
        // click, not straight after E: the edit opens on review's word, which reaches the window
        // through its event loop after the key, and nothing on the display says when it has.
        QVERIFY(xdotool({"type", "!q"}));
        QVERIFY(!app.waitForFinished(1000));
        // Saved and accepted.
        QVERIFY(xdotool({"key", "Return"}));
        quit(app);
        if (QTest::currentTestFailed()) {
            return;
        }
        // The wording as it was typed, accepted, as the store kept it.
        QProcess list;
        diesWithTheTest(list);
        list.setProcessEnvironment(isolated(dir));
        list.start(env("SUBCHECK_CLI"), {"list", "--accepted", env("SUBCHECK_TEST_MEDIA")});
        QVERIFY(list.waitForFinished(15000) && list.exitStatus() == QProcess::NormalExit &&
                list.exitCode() == 0);
        const QString listed = QString::fromUtf8(list.readAllStandardOutput());
        QVERIFY2(listed.split('\n').contains("      + We should leave before it rains.!q"),
                 qPrintable(listed));
    }
};

QTEST_GUILESS_MAIN(EndToEndTest)
#include "end_to_end_test.moc"
