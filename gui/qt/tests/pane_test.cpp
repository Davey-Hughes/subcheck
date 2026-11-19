// The video pane on real mpv, on an X display of the test's own, as the mpv of a player whose
// settle time is far longer than any wait here: only what mpv says, passed on by the pane, can end
// a move in time. A load and a seek end when playback starts again, a frame step when the position
// moves, and a load that fails when its file ends on an error. A line opens on its first frame,
// paused or playing as mpv was, and the pane draws it. Once closed, the pane is black again. mpv's
// config is one a user could have: `hr-seek=no`, which would land a load on a keyframe but for the
// option review gives each load, and `reset-on-next-file=pause`, under which the play state still
// holds. Linux only: Xvfb is X11's.
#include "desktop_session.h"
#include "mpv_item.h"
#include "player_port.h"
#include "said.h"

#include <QDir>
#include <QFile>
#include <QGuiApplication>
#include <QQmlComponent>
#include <QQmlEngine>
#include <QQuickWindow>
#include <QScopeGuard>
#include <QTest>
#include <QtQml/QQmlExtensionPlugin>

#include <cmath>
#include <csignal>
#include <cstdlib>
#include <fcntl.h>
#include <memory>
#include <optional>
#include <string>
#include <sys/prctl.h>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

Q_IMPORT_QML_PLUGIN(SubcheckPlugin)

using namespace subcheck_ffi;

namespace {

// With the options review gives every load (core's `player::load_options`).
PlayerCommand open(const std::string &file, std::int64_t at, std::optional<std::uint64_t> sid,
                   How how = How::kLoad) {
    return PlayerCommand(PlayerCommand::kOpen{
        Moment{file, at, sid}, how, {{"hr-seek", "yes"}, {"rebase-start-time", "no"}}});
}

PlayerCommand playback(Playback p) { return PlayerCommand(PlayerCommand::kPlayback{std::move(p)}); }

} // namespace

class PaneTest : public QObject {
    Q_OBJECT

    // Far longer than the waits below, so that no timer of the player's can end a command for mpv.
    PlayerPort player{std::chrono::seconds(20), std::chrono::seconds(20), std::chrono::seconds(20)};
    QQmlEngine engine;
    std::unique_ptr<QObject> window;
    MpvItem *pane = nullptr;
    QStringList reports;
    std::string video = qEnvironmentVariable("SUBCHECK_TEST_VIDEO").toStdString();
    // Cleaned, as mpv gives a path back without the `//` that CMake's REPO_ROOT, ending in `/`,
    // leaves in it: the player seeks only in the file mpv says it has open.
    std::string lines = QDir::cleanPath(qEnvironmentVariable("SUBCHECK_TEST_LINES")).toStdString();

    // The pane's pixels that are bright, in what the window shows now: the fixture's video is all
    // but black, so these are a subtitle's. -1 when the grab holds none of the pane, so that a
    // failed grab is never taken for a black pane.
    int bright() const {
        const QImage shot = qobject_cast<QQuickWindow *>(window.get())->grabWindow();
        const qreal scale = shot.devicePixelRatio();
        const QRectF area = pane->mapRectToScene(pane->boundingRect());
        const QRect pixels =
            QRectF(area.topLeft() * scale, area.size() * scale).toRect() & shot.rect();
        if (pixels.isEmpty()) {
            return -1;
        }
        int count = 0;
        for (int y = pixels.top(); y <= pixels.bottom(); ++y) {
            for (int x = pixels.left(); x <= pixels.right(); ++x) {
                if (qGray(shot.pixel(x, y)) > 127) {
                    ++count;
                }
            }
        }
        return count;
    }

private slots:
    void initTestCase() {
        QVERIFY2(!video.empty() && !lines.empty() && qEnvironmentVariableIsSet("XDG_CONFIG_HOME"),
                 "run through ctest, which names the videos and gives mpv a config of its own");
        connect(&player, &PlayerPort::reported, this,
                [this](const PlayerReport &r) { reports << said(r); });
        QQmlComponent component(&engine);
        component.setData("import QtQuick; import Subcheck; Window { visible: true; width: 320; "
                          "height: 180; MpvItem { objectName: 'video'; anchors.fill: parent } }",
                          QUrl());
        window.reset(component.create());
        QVERIFY2(window, qPrintable(component.errorString()));
        pane = window->findChild<MpvItem *>("video");
        QVERIFY(pane);
        pane->setPlayer(&player);
    }

    void cleanupTestCase() {
        player.release();
        window.reset();
    }

    // Once mpv can show it, as the pane says, on playback starting there. The longest wait, for the
    // most work, mpv's first video output and a cold shader cache among it; still half the settle
    // time, so that only mpv's word can end it. mpv runs on the config above, which the load's
    // own options override for its file alone. Each of review's is read back: mpv takes a file's
    // options without checking them, so only its value shows that one was handed on.
    void aLoadEndsOnPlaybackStarting() {
        QCOMPARE(pane->getProperty("hr-seek"), QVariant(false));
        QCOMPARE(pane->getProperty("reset-on-next-file").toStringList(), QStringList{"pause"});
        player.run(open(video, 2'500'000, 4));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"opened paused"}, 10000);
        QCOMPARE(pane->getProperty("hr-seek"), QVariant(true));
        QCOMPARE(pane->getProperty("rebase-start-time"), QVariant(false));
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

    // A line that starts a millisecond after a frame: ties.mkv's at 41.25 s, after the frame stored
    // at 41.249 s, which shows none of it. Opened where review opens a line, 4.5 ms past its start
    // (core/src/player.rs), mpv stays on the next frame, the first with the line on it. Line 20 is
    // up on both frames and the video is black, so the pane has drawn the line under test when it
    // draws more on that frame than on the one before. An open can report before the pane has
    // drawn its frame, so the first bright count is taken for the opened frame's, which holds on
    // the first open only: the test before leaves timing.mkv paused at 2 s, before its track's
    // first line, so nothing bright shows before it; a load over a paused file, it stays paused.
    // Opened again while it plays, past the line, told to seek, as review would tell it after the
    // load, it is sought, and it plays on from the line. The frames it plays through can carry
    // both lines too, so the second open is checked in mpv's properties, not in the pane's pixels.
    // A load would pass every other check: only a seek keeps mpv's playlist entry as it was.
    void aLineOpensOnItsFirstFramePausedOrPlayingAsMpvWas() {
        const QString underTest = QStringLiteral("Starts on a frame stored early.");
        // Put back however the test ends, so that no test after it plays slowly.
        const auto speed = qScopeGuard([this] { pane->setPropertyBlocking("speed", 1.0); });
        QVERIFY2(pane->getProperty("path").toString().endsWith("/timing.mkv"),
                 "the test before leaves timing.mkv open");
        QCOMPARE(pane->getProperty("pause").toBool(), true);
        QVariant loaded;
        for (int opening = 0; opening < 2; ++opening) {
            reports.clear();
            player.run(open(lines, 41'254'500, 1, opening == 0 ? How::kLoad : How::kSeek));
            QTRY_COMPARE_WITH_TIMEOUT(
                reports, QStringList{opening == 0 ? "opened paused" : "opened playing"}, 10000);
            QCOMPARE(pane->getProperty("path").toString(), QString::fromStdString(lines));
            const QVariant entry = pane->getProperty("playlist/0/id");
            // An id, not the empty value MpvQt documents for a property mpv lacks: two of those
            // compare equal, and would pass the check on the second open.
            QCOMPARE(entry.metaType(), QMetaType::fromType<qlonglong>());
            if (opening == 0) {
                loaded = entry;
                QCOMPARE(pane->getProperty("pause").toBool(), true);
                QCOMPARE(std::lround(pane->getProperty("time-pos").toDouble() * 1000), 41291);
                // With line 20, up from 40.29 s to 42.29 s.
                QCOMPARE(pane->getProperty("sub-text").toString(),
                         "Line 20 on the cut.\n" + underTest);
                // The counts the checks took, not a fresh grab's, which can differ or be -1.
                int both = 0;
                QTRY_VERIFY2_WITH_TIMEOUT(
                    (both = bright()) > 0,
                    qPrintable(
                        QStringLiteral("%1 bright: with no font, mpv draws no subtitle").arg(both)),
                    5000);
                reports.clear();
                player.run(playback(Playback(Playback::kPreviousFrame{})));
                QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"played paused 41249"}, 5000);
                int alone = 0;
                QTRY_VERIFY2_WITH_TIMEOUT(
                    (alone = bright()) > 0 && alone < both,
                    qPrintable(QStringLiteral("%1 bright with both lines, %2 with line 20 alone")
                                   .arg(both)
                                   .arg(alone)),
                    5000);
                reports.clear();
                player.run(playback(Playback(Playback::kPause{})));
                QTRY_COMPARE_WITH_TIMEOUT(reports.size(), 1, 5000);
                QVERIFY2(reports[0].startsWith("played playing "), qPrintable(reports[0]));
                // Playing, not only unpaused, so that the second open comes while the file plays;
                // and past the line, its end at 41.5 s, so that only a seek back puts it there.
                QTRY_COMPARE_WITH_TIMEOUT(pane->getProperty("core-idle").toBool(), false, 5000);
                QTRY_VERIFY_WITH_TIMEOUT(
                    std::lround(pane->getProperty("time-pos").toDouble() * 1000) >= 41500, 5000);
                // At a tenth of its speed, the line, which ends a fifth of a second of the video
                // after the frame the open lands on, stays up ten times as long in the test's
                // time, so that the second open's checks of it cannot come after it has gone.
                QCOMPARE(pane->setPropertyBlocking("speed", 0.1), 0);
            } else {
                QCOMPARE(entry, loaded);
                QCOMPARE(pane->getProperty("pause").toBool(), false);
                // The line is read first: a position still on the line after it says the line was
                // up when it was read. At or past the frame the open landed on, the position says
                // mpv went back to the line, as it was past it before; short of 41.4 s, well short
                // of the line's end, that it went back to the line's first frames. At a tenth of
                // its speed, mpv is still on them long after these reads, which come as soon as
                // the Open has reported.
                const QString text = pane->getProperty("sub-text").toString();
                const long at = std::lround(pane->getProperty("time-pos").toDouble() * 1000);
                QVERIFY2(at >= 41291 && at < 41400, qPrintable(QString::number(at)));
                QVERIFY2(text.contains(underTest), qPrintable(text));
                // Playing, not only unpaused.
                QTRY_COMPARE_WITH_TIMEOUT(pane->getProperty("core-idle").toBool(), false, 5000);
            }
        }
    }

    // Another file over the one open, a copy of the video in the test's own directory, so that its
    // path differs and it is loaded: the file it replaces ends too, stopped, and only the load's
    // own end can fail it. It plays on, as the file it replaces played, by the load's own pause
    // option, as nothing unpauses after a load, and the config's reset-on-next-file=pause leaves
    // that be. The test closes what the test before left and opens that file itself, so that it
    // opens paused whatever was left, and the player's Pause sets it playing.
    void anotherFileLoadsOverTheOneOpen() {
        reports.clear();
        player.run(PlayerCommand(PlayerCommand::kClose{}));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"closed"}, 5000);
        reports.clear();
        // As long as the first open's wait: mpv makes its video output again, which Close took.
        player.run(open(video, 2'500'000, 4));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"opened paused"}, 10000);
        QCOMPARE(pane->getProperty("pause").toBool(), true);
        reports.clear();
        player.run(playback(Playback(Playback::kPause{})));
        QTRY_COMPARE_WITH_TIMEOUT(reports.size(), 1, 5000);
        QVERIFY2(reports[0].startsWith("played playing "), qPrintable(reports[0]));
        // Playing: mpv can be unpaused with nothing playing, which `pause` would not tell.
        QTRY_COMPARE_WITH_TIMEOUT(pane->getProperty("core-idle").toBool(), false, 5000);
        reports.clear();
        const QString copy = qEnvironmentVariable("SUBCHECK_TEST_DIR") + "/pane_test-copy.mkv";
        // An earlier run's, which a copy does not overwrite.
        QFile::remove(copy);
        QVERIFY(QFile::copy(QString::fromStdString(video), copy));
        player.run(open(copy.toStdString(), 2'500'000, 4));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"opened playing"}, 5000);
        QCOMPARE(pane->getProperty("pause").toBool(), false);
        QTRY_COMPARE_WITH_TIMEOUT(pane->getProperty("core-idle").toBool(), false, 5000);
        // Paused, and opened there again, sought, it stays paused: the test after finds the sign
        // up however long this one took, where played on, it is gone 2 s after the open.
        reports.clear();
        player.run(playback(Playback(Playback::kPause{})));
        QTRY_COMPARE_WITH_TIMEOUT(reports.size(), 1, 5000);
        QVERIFY2(reports[0].startsWith("played paused "), qPrintable(reports[0]));
        reports.clear();
        player.run(open(copy.toStdString(), 2'500'000, 4, How::kSeek));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"opened paused"}, 5000);
        QCOMPARE(pane->getProperty("pause").toBool(), true);
    }

    // A file that is not there, as one gone between review's stat and the load would be: loaded, as
    // the Open says load, and its end on an error says so. The frame of the file it replaced, a
    // subtitle up, goes with that file: the pane is black.
    void aLoadThatFailsEndsOnItsFileEnding() {
        QTRY_VERIFY2_WITH_TIMEOUT(bright() > 0, "no subtitle: mpv draws none without a font", 5000);
        reports.clear();
        const std::string gone = QDir::currentPath().toStdString() + "/pane_test-gone.mkv";
        player.run(open(gone, 0, std::nullopt));
        QTRY_COMPARE_WITH_TIMEOUT(
            reports, QStringList{"open failed: could not open " + QString::fromStdString(gone)},
            5000);
        QTRY_COMPARE_WITH_TIMEOUT(bright(), 0, 5000);
    }

    // The load that failed left mpv nothing open: nothing to close, as the terminal says.
    void aCloseWithNothingOpenSaysSo() {
        reports.clear();
        player.run(PlayerCommand(PlayerCommand::kClose{}));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"closed: not open"}, 5000);
    }

    // A frame left in the pane would show a file that is no longer open. The pane draws when mpv
    // asks it to, and mpv need not ask as its video output goes. An open of its own first, at a
    // moment with a subtitle up, so that there is a frame to lose, as the load that failed left
    // none. It waits as long as the first: mpv makes its video output again, which Close took.
    void closeBlanksThePane() {
        reports.clear();
        player.run(open(video, 2'500'000, 4));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"opened paused"}, 10000);
        QTRY_VERIFY2_WITH_TIMEOUT(bright() > 0, "no subtitle: mpv draws none without a font", 5000);
        reports.clear();
        player.run(PlayerCommand(PlayerCommand::kClose{}));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"closed"}, 5000);
        QTRY_COMPARE_WITH_TIMEOUT(bright(), 0, 5000);
    }

    // What the window's indicator draws from: a video showing once one is open, paused or playing
    // as mpv is, and none once it is closed.
    void thePaneSaysWhetherAVideoShowsAndWhetherItPlays() {
        reports.clear();
        player.run(open(video, 2'500'000, 4));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"opened paused"}, 10000);
        QTRY_VERIFY_WITH_TIMEOUT(pane->showing(), 5000);
        QTRY_VERIFY_WITH_TIMEOUT(pane->paused(), 5000);
        player.run(playback(Playback(Playback::kPause{})));
        QTRY_VERIFY_WITH_TIMEOUT(!pane->paused(), 5000);
        player.run(playback(Playback(Playback::kPause{})));
        QTRY_VERIFY_WITH_TIMEOUT(pane->paused(), 5000);
        reports.clear();
        player.run(PlayerCommand(PlayerCommand::kClose{}));
        QTRY_COMPARE_WITH_TIMEOUT(reports, QStringList{"closed"}, 5000);
        QTRY_VERIFY_WITH_TIMEOUT(!pane->showing(), 5000);
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
