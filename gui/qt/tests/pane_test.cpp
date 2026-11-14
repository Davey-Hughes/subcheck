// The video pane on real mpv, on an X display of the test's own, as the mpv of a player whose
// settle time is far longer than any wait here: only what mpv says, passed on by the pane, can end
// a move in time. A load and a seek end when playback starts again, a frame step when the position
// moves, and a load that fails when its file ends on an error. Linux only: Xvfb is X11's.
#include "desktop_session.h"
#include "mpv_item.h"
#include "overloaded.h"
#include "player_port.h"

#include <QDir>
#include <QFile>
#include <QGuiApplication>
#include <QQmlComponent>
#include <QQmlEngine>
#include <QQuickWindow>
#include <QTest>
#include <QtQml/QQmlExtensionPlugin>

#include <csignal>
#include <cstdlib>
#include <fcntl.h>
#include <memory>
#include <optional>
#include <string>
#include <sys/prctl.h>
#include <sys/wait.h>
#include <unistd.h>
#include <variant>
#include <vector>

Q_IMPORT_QML_PLUGIN(SubcheckPlugin)

using namespace subcheck_ffi;

namespace {

// A report as one line, to compare.
QString said(const PlayerReport &report) {
    const auto error = [](const std::optional<std::string> &e) {
        return e ? QStringLiteral(": ") + QString::fromStdString(*e) : QString();
    };
    return std::visit(
        Overloaded{
            [&](const PlayerReport::kOpened &r) {
                return QStringLiteral("opened") + error(r.error);
            },
            [](const PlayerReport::kPlayed &r) {
                return QStringLiteral("played %1 %2")
                    .arg(r.state.paused ? QStringLiteral("paused") : QStringLiteral("playing"))
                    .arg(r.state.at_ms);
            },
            [](const PlayerReport::kPlayFailed &r) {
                return QStringLiteral("play failed: ") + QString::fromStdString(r.error);
            },
            [&](const PlayerReport::kClosed &r) {
                return QStringLiteral("closed") + error(r.error);
            },
        },
        report.get_variant());
}

PlayerCommand playback(Playback p) { return PlayerCommand(PlayerCommand::kPlayback{std::move(p)}); }

} // namespace

class PaneTest : public QObject {
    Q_OBJECT

    // Far longer than the waits below, so that the settle timer cannot end a move for mpv.
    PlayerPort player{std::chrono::seconds(20)};
    QQmlEngine engine;
    std::unique_ptr<QObject> window;
    QStringList reports;
    std::string video = qEnvironmentVariable("SUBCHECK_TEST_VIDEO").toStdString();

private slots:
    void initTestCase() {
        QVERIFY2(!video.empty() && qEnvironmentVariableIsSet("XDG_CONFIG_HOME"),
                 "run through ctest, which names the video and gives mpv a config of its own");
        connect(&player, &PlayerPort::reported, this,
                [this](const PlayerReport &r) { reports << said(r); });
        QQmlComponent component(&engine);
        component.setData("import QtQuick; import Subcheck; Window { visible: true; width: 320; "
                          "height: 180; MpvItem { objectName: 'video'; anchors.fill: parent } }",
                          QUrl());
        window.reset(component.create());
        QVERIFY2(window, qPrintable(component.errorString()));
        auto *pane = window->findChild<MpvItem *>("video");
        QVERIFY(pane);
        pane->setPlayer(&player);
    }

    void cleanupTestCase() {
        player.release();
        window.reset();
    }

    // Once mpv can show it, as the pane says, on playback starting there. The longest wait, for the
    // most work, mpv's first video output and a cold shader cache among it; still half the settle
    // time, so that only mpv's word can end it.
    void aLoadEndsOnPlaybackStarting() {
        player.run(PlayerCommand(PlayerCommand::kOpen{Moment{video, 2500, 4, true}}));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"opened"}, 10000);
    }

    // A frame on, at the fixture's 2 per second, on the position moving.
    void aFrameStepEndsOnThePositionMoving() {
        reports.clear();
        player.run(playback(Playback(Playback::kNextFrame{})));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"played paused 3000"}, 5000);
    }

    // A second back, exactly, on playback starting again.
    void aSeekEndsOnPlaybackStarting() {
        reports.clear();
        player.run(playback(Playback(Playback::kSeek{-1})));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"played paused 2000"}, 5000);
    }

    // Another file over the one open, a copy of the video in the test's own directory, so that its
    // path differs and it is loaded: the file it replaces ends too, stopped, and only the load's
    // own end can fail it.
    void anotherFileLoadsOverTheOneOpen() {
        reports.clear();
        const QString copy = qEnvironmentVariable("SUBCHECK_TEST_DIR") + "/pane_test-copy.mkv";
        // An earlier run's, which a copy does not overwrite.
        QFile::remove(copy);
        QVERIFY(QFile::copy(QString::fromStdString(video), copy));
        player.run(PlayerCommand(PlayerCommand::kOpen{Moment{copy.toStdString(), 2500, 4, true}}));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"opened"}, 5000);
    }

    // A file that is not there: loaded, as it cannot be read, and its end on an error says so.
    void aLoadThatFailsEndsOnItsFileEnding() {
        reports.clear();
        const std::string gone = QDir::currentPath().toStdString() + "/pane_test-gone.mkv";
        player.run(PlayerCommand(PlayerCommand::kOpen{Moment{gone, 0, std::nullopt, false}}));
        QTRY_COMPARE_WITH_TIMEOUT(
            reports, QStringList{"opened: mpv could not open pane_test-gone.mkv"}, 5000);
    }

    void closeStops() {
        reports.clear();
        player.run(PlayerCommand(PlayerCommand::kClose{}));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"closed"}, 5000);
    }
};

// An X server of the test's own, before Qt picks its platform; and none of the variables
// desktop_session.h names as a desktop session's in the test's environment, which the server
// inherits. Xvfb takes the first free display number and writes it once it listens. Its cache,
// Mesa's shaders among it, and the test's own go to the directory CTest names, never the user's.
int main(int argc, char **argv) {
    const QByteArray dir = qgetenv("SUBCHECK_TEST_DIR");
    if (dir.isEmpty()) {
        qCritical("run through ctest, which gives the test a directory of its own");
        return 2;
    }
    // Collected first: unsetting a name while walking the environment moves the ones after it.
    std::vector<std::string> session;
    for (char **entry = environ; *entry; ++entry) {
        const std::string_view variable(*entry);
        const std::string_view name = variable.substr(0, variable.find('='));
        if (isDesktopSession(name)) {
            session.emplace_back(name);
        }
    }
    for (const std::string &name : session) {
        unsetenv(name.c_str());
    }
    QDir().mkpath(QString::fromLocal8Bit(dir + "/cache"));
    QDir().mkpath(QString::fromLocal8Bit(dir + "/run"));
    QFile::setPermissions(QString::fromLocal8Bit(dir + "/run"),
                          QFileDevice::ReadOwner | QFileDevice::WriteOwner | QFileDevice::ExeOwner);
    setenv("XDG_CACHE_HOME", (dir + "/cache").constData(), 1);
    setenv("XDG_RUNTIME_DIR", (dir + "/run").constData(), 1);
    int number[2];
    if (pipe(number) != 0) {
        return 2;
    }
    const pid_t xvfb = fork();
    // Stops here: the kill below, given fork's -1, would signal every process of the user's.
    if (xvfb == -1) {
        qCritical("fork failed: no Xvfb to run the test on");
        return 2;
    }
    if (xvfb == 0) {
        // A test that crashes, or that CTest's timeout kills, takes its X server with it.
        prctl(PR_SET_PDEATHSIG, SIGTERM);
        dup2(number[1], 1);
        dup2(open("/dev/null", O_WRONLY), 2);
        close(number[0]);
        execl(qgetenv("SUBCHECK_XVFB").constData(), "Xvfb", "-displayfd", "1", "-screen", "0",
              "1280x800x24", "-nolisten", "tcp", static_cast<char *>(nullptr));
        _exit(127);
    }
    close(number[1]);
    std::string display = ":";
    char c = 0;
    while (read(number[0], &c, 1) == 1 && c != '\n') {
        display += c;
    }
    close(number[0]);
    if (display == ":") {
        qCritical("Xvfb named no display");
        return 2;
    }
    setenv("DISPLAY", display.c_str(), 1);
    setenv("QT_QPA_PLATFORM", "xcb", 1);
    // As main.cpp, before the first window.
    QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL);
    int status = 0;
    {
        QGuiApplication app(argc, argv);
        PaneTest test;
        status = QTest::qExec(&test, argc, argv);
    }
    kill(xvfb, SIGTERM);
    waitpid(xvfb, nullptr, 0);
    return status;
}

#include "pane_test.moc"
