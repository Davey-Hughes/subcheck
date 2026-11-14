// The app end to end, as a person runs it: the real subcheck-qt on an X display of the test's own
// (Xvfb, drawn with Mesa's software OpenGL), real keys from that display's server (xdotool), and
// real mpv playing timing.mkv, the fixture with video, which says where it is over the socket the
// test's mpvqt.conf gives it. Linux only: Xvfb and xdotool are X11's.
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

private slots:
    void initTestCase() {
        // The app decides findings in whatever store it opens: without CTest's environment, that
        // would be the user's own.
        QVERIFY2(qEnvironmentVariableIsSet("SUBCHECK_DB") && !dir.isEmpty(),
                 "run through ctest, which seeds a store of the test's own");
        QVERIFY(QDir().mkpath(dir + "/config/mpvqt") && QDir().mkpath(dir + "/run") &&
                QDir().mkpath(dir + "/cache"));
        QVERIFY(QFile::setPermissions(dir + "/run", QFileDevice::ReadOwner |
                                                        QFileDevice::WriteOwner |
                                                        QFileDevice::ExeOwner));
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
        // mpv's config, in place of the user's: no sound, and a socket that says where it is.
        const QString socket = dir + "/mpv.sock";
        QFile config(dir + "/config/mpvqt/mpvqt.conf");
        QVERIFY(config.open(QIODevice::WriteOnly | QIODevice::Truncate));
        config.write("ao=null\ninput-ipc-server=" + socket.toUtf8() + "\n");
        config.close();

        // The app on that display and nowhere else; Qt's warnings on its stderr, where the test
        // reads them.
        QProcessEnvironment e = isolated(dir);
        e.insert("DISPLAY", display);
        e.insert("QT_QPA_PLATFORM", "xcb");
        e.insert("QT_FORCE_STDERR_LOGGING", "1");
        QProcess app;
        diesWithTheTest(app);
        // Made after the app, so that it runs while the app is still there; and the app's first,
        // while its display is still there, so that its last words are not the display's loss.
        const auto shown = qScopeGuard([&] {
            showStderrIfFailed(app, "subcheck-qt");
            showStderrIfFailed(xvfb, "Xvfb");
        });
        app.setProcessEnvironment(e);
        app.start(env("SUBCHECK_QT"), {env("SUBCHECK_TEST_MEDIA")});
        QVERIFY(app.waitForStarted());
        QString windows;
        QVERIFY(
            xdotool({"search", "--sync", "--onlyvisible", "--classname", "subcheck-qt"}, &windows));
        QVERIFY(xdotool({"windowfocus", "--sync", windows.section('\n', 0, 0)}));

        // The first row accepted, which moves on to the second; back to the first, and played: a
        // timing finding's, paused on the frame that shows it, with its track's subtitles.
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
        // Closed: mpv answers, and has no file.
        QVERIFY(xdotool({"key", "O"}));
        QTRY_COMPARE_WITH_TIMEOUT(ask(socket, "path").value("error").toString(),
                                  "property unavailable", 5000);

        // Quit: review ends, the app prints its counts and exits 0, as main.cpp wires it.
        QVERIFY(xdotool({"key", "q"}));
        QVERIFY(app.waitForFinished(15000));
        QCOMPARE(app.exitStatus(), QProcess::NormalExit);
        QCOMPARE(app.exitCode(), 0);
        QCOMPARE(app.readAllStandardOutput(), "media: 1 accepted, 0 rejected, 1 pending\n");
        const QString err = QString::fromUtf8(app.readAllStandardError());
        QVERIFY2(!err.contains(".qml:"), qPrintable(err));
    }
};

QTEST_GUILESS_MAIN(EndToEndTest)
#include "end_to_end_test.moc"
