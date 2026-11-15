// The player on a fake mpv, with no session and no window: which requests each command sends, in
// which order, what it waits for, and what it reports.
#include "fake_mpv.h"
#include "overloaded.h"
#include "player_port.h"

#include <QDir>
#include <QElapsedTimer>
#include <QFile>
#include <QTemporaryDir>
#include <QTest>

#include <chrono>
#include <filesystem>
#include <optional>
#include <string>
#include <tuple>
#include <variant>
#include <vector>

using namespace subcheck_ffi;

namespace {

const QString unavailable = QStringLiteral("property unavailable");
// mpv's text for a command it refuses, as a seek of a file it still loads.
const QString refused = QStringLiteral("error running command");

// A file of one byte in `dir`, for a test whose player stats what it opens; empty when it could
// not be written.
QString oneByteFile(const QTemporaryDir &dir) {
    const QString path = dir.filePath(QStringLiteral("a.mkv"));
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly) || file.write("a") != 1) {
        return {};
    }
    return path;
}

Moment moment(const std::string &file, std::int64_t at, std::optional<std::uint64_t> sid) {
    return Moment{file, at, sid};
}

PlayerCommand open(const Moment &m) { return PlayerCommand(PlayerCommand::kOpen{m}); }

PlayerCommand playback(Playback p) { return PlayerCommand(PlayerCommand::kPlayback{std::move(p)}); }

PlayerCommand seek(std::int32_t by) { return playback(Playback(Playback::kSeek{by})); }

PlayerCommand close() { return PlayerCommand(PlayerCommand::kClose{}); }

// What an Open asks of mpv first, before its move: where mpv is; whether it plays, with a file
// open only; then the pause every move starts with.
const QStringList firstWithNoFile = {"get path", "set pause yes"};
const QStringList firstWithAFile = {"get path", "get pause", "set pause yes"};

// Answers the Open `player` has under way as `mpv` with `path` open would (none when empty), paused
// or playing, as far as its move: the requests above, which it returns for the test to compare.
QStringList answerOpening(FakeMpv &mpv, PlayerPort &player, const QString &path = {},
                          bool paused = true) {
    QStringList asked{mpv.last()};
    if (path.isEmpty()) {
        mpv.answer(player, {}, unavailable);
    } else {
        mpv.answer(player, path);
        asked << mpv.last();
        mpv.answer(player, paused);
    }
    asked << mpv.last();
    mpv.answer(player);
    return asked;
}

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

// The player, its fake mpv, and what it has reported, each as one line.
struct Rig {
    PlayerPort player;
    FakeMpv mpv;
    QStringList reports;

    explicit Rig(std::chrono::milliseconds settle = std::chrono::seconds(10)) : player(settle) {
        QObject::connect(&player, &PlayerPort::reported,
                         [this](const PlayerReport &r) { reports << said(r); });
        player.attach(&mpv);
    }

    // Ready, and a file loaded and landed: what a Playback needs open.
    void loaded() {
        player.ready();
        player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        openWith();
        mpv.answer(player);
        player.restarted();
        reports.clear();
    }

    QStringList openWith(const QString &path = {}, bool paused = true) {
        return answerOpening(mpv, player, path, paused);
    }
};

} // namespace

class PlayerPortTest : public QObject {
    Q_OBJECT

private slots:
    // The newest moment only, once mpv can show it; paused, with nothing after the load to unpause
    // it, as mpv had no file open to play.
    void anOpenWaitsUntilMpvCanShowVideo() {
        Rig r;
        r.player.run(open(moment("/m/a.mkv", 1'000'000, 1)));
        r.player.run(open(moment("/m/b.mkv", 2'004'500, 2)));
        QCOMPARE(r.mpv.requests.size(), 0u);
        r.player.ready();
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(),
                 "loadfile /m/b.mkv replace -1 start=0:00:02.004500,sid=2,sub-visibility=yes");
        QCOMPARE(r.reports, QStringList());
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened"});
        QCOMPARE(r.mpv.requests.size(), 3u);
    }

    // Nothing can be open before mpv can show video: a Playback says so, or that the file is
    // still opening, and a Close drops the Open that waits.
    void beforeThenAPlaybackOrACloseIsAnsweredAtOnce() {
        Rig r;
        r.player.run(seek(5));
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.player.run(seek(5));
        r.player.run(close());
        r.player.ready();
        QCOMPARE(r.reports, (QStringList{"play failed: mpv has no file open (o opens one)",
                                         "play failed: mpv is still opening the file", "closed"}));
        QCOMPARE(r.mpv.requests.size(), 0u);
    }

    // Paused before mpv gets there, so that a playing mpv does not go on past the frame, and opened
    // once playback starts there, with nothing after it to unpause, as mpv had no file open, then
    // one paused: a position is no landing for a load, as mpv can hold one from before it until
    // now. Whether mpv plays is read only with a file open, and before anything is sent. The
    // subtitle track only when review names one; the time in µs, as review gives it; never a time
    // before 0:00, which mpv counts from the end.
    void anotherFileIsLoaded() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 62'504'500, 2)));
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(),
                 "loadfile /m/a.mkv replace -1 start=0:01:02.504500,sid=2,sub-visibility=yes");
        r.mpv.answer(r.player);
        r.player.moved();
        QCOMPARE(r.reports, QStringList());
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened"});
        QCOMPARE(r.mpv.requests.size(), 3u);

        r.player.run(open(moment("/m/b.mkv", -40'000, std::nullopt)));
        QCOMPARE(r.openWith("/m/a.mkv"), firstWithAFile);
        QCOMPARE(r.mpv.last(),
                 "loadfile /m/b.mkv replace -1 start=0:00:00.000000,sub-visibility=yes");
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.mpv.requests.size(), 7u);
        QCOMPARE(r.reports, (QStringList{"opened", "opened"}));
    }

    // Whether mpv plays decides only whether it plays again once there: a read of it that mpv
    // fails, or that gives no flag, counts as paused, and the Open goes on, left paused.
    void aFailedReadOfPauseOpensPaused() {
        Rig r;
        r.loaded();
        r.player.run(open(moment("/m/b.mkv", 0, std::nullopt)));
        QCOMPARE(r.mpv.last(), "get path");
        r.mpv.answer(r.player, QStringLiteral("/m/a.mkv"));
        QCOMPARE(r.mpv.last(), "get pause");
        r.mpv.answer(r.player, {}, QStringLiteral("unknown error"));
        QCOMPARE(r.mpv.last(), "set pause yes");
        r.mpv.answer(r.player);
        QVERIFY(r.mpv.last().startsWith("loadfile "));
        r.mpv.answer(r.player);
        auto sent = r.mpv.requests.size();
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened"});
        QCOMPARE(r.mpv.requests.size(), sent);

        r.player.run(open(moment("/m/c.mkv", 0, std::nullopt)));
        r.mpv.answer(r.player, QStringLiteral("/m/b.mkv"));
        QCOMPARE(r.mpv.last(), "get pause");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "set pause yes");
        r.mpv.answer(r.player);
        QVERIFY(r.mpv.last().startsWith("loadfile "));
        r.mpv.answer(r.player);
        sent = r.mpv.requests.size();
        r.player.restarted();
        QCOMPARE(r.reports, (QStringList{"opened", "opened"}));
        QCOMPARE(r.mpv.requests.size(), sent);
    }

    // Another file while mpv plays one: loaded paused, as any is, and playing again only once the
    // load has landed, so that mpv plays on from the frame, not from where it was; the Open reports
    // once mpv has answered that.
    void aPlayingMpvLoadsAnotherFileAndPlaysOn() {
        Rig r;
        r.loaded();
        r.player.run(open(moment("/m/b.mkv", 2'004'500, 2)));
        QCOMPARE(r.openWith("/m/a.mkv", false), firstWithAFile);
        const QString load = QStringLiteral(
            "loadfile /m/b.mkv replace -1 start=0:00:02.004500,sid=2,sub-visibility=yes");
        QCOMPARE(r.mpv.last(), load);
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), load);
        r.player.restarted();
        QCOMPARE(r.mpv.last(), "set pause no");
        QCOMPARE(r.reports, QStringList());
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, QStringList{"opened"});
    }

    // As the terminal's: sought if this player loaded it and it is unchanged on disk since, in
    // its size and in its modification time; loaded otherwise, also when mpv has it open but
    // this player did not load it, and when it cannot be read now.
    void theSameFileUnchangedIsSought() {
        QTemporaryDir dir(QDir::currentPath() + QStringLiteral("/player_port_test-XXXXXX"));
        QVERIFY(dir.isValid());
        const QString path = dir.filePath(QStringLiteral("a.mkv"));
        const std::string file = path.toStdString();
        QFile written(path);
        QVERIFY(written.open(QIODevice::WriteOnly));
        written.write("a");
        written.close();
        Rig r;
        r.player.ready();
        // An Open of the file mpv has open, paused: the move it makes once its settings are made.
        const auto move = [&] {
            r.player.run(open(moment(file, 5'000'000, 3)));
            r.openWith(path);
            while (r.mpv.last().startsWith("set ")) {
                r.mpv.answer(r.player);
            }
            const QString made = r.mpv.last();
            r.mpv.answer(r.player);
            r.player.restarted();
            return made;
        };
        QVERIFY(move().startsWith("loadfile "));
        r.player.run(open(moment(file, 5'000'000, 3)));
        QCOMPARE(r.openWith(path), firstWithAFile);
        QCOMPARE(r.mpv.last(), "set sid 3");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "set sub-visibility yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 0:00:05.000000 absolute+exact");
        r.mpv.answer(r.player);
        const auto sent = r.mpv.requests.size();
        r.player.restarted();
        QCOMPARE(r.reports, (QStringList{"opened", "opened"}));
        // Paused, it stays so: nothing after the seek.
        QCOMPARE(r.mpv.requests.size(), sent);

        // A fix of one letter in place: the same size, a later modification time.
        QVERIFY(written.open(QIODevice::ReadWrite));
        written.write("b");
        written.close();
        const auto later = std::filesystem::last_write_time(file) + std::chrono::seconds(1);
        std::filesystem::last_write_time(file, later);
        QVERIFY(move().startsWith("loadfile "));
        QCOMPARE(move(), "seek 0:00:05.000000 absolute+exact");
        // A letter more, the modification time put back: only the size has changed.
        QVERIFY(written.open(QIODevice::Append));
        written.write("c");
        written.close();
        std::filesystem::last_write_time(file, later);
        QVERIFY(move().startsWith("loadfile "));
        // Gone from disk since this player loaded it: loaded again, not sought, so that mpv says
        // whether it still opens.
        QVERIFY(QFile::remove(path));
        QVERIFY(move().startsWith("loadfile "));

        // Never loaded by this player: loaded on the name alone.
        const std::string gone = dir.filePath(QStringLiteral("gone.mkv")).toStdString();
        r.player.run(open(moment(gone, 0, std::nullopt)));
        QCOMPARE(r.openWith(QString::fromStdString(gone)), firstWithAFile);
        QVERIFY(r.mpv.last().startsWith("loadfile "));
    }

    // While mpv plays the file this player loaded, unchanged: sought paused, and playing again
    // only once the seek has landed, so that it plays on from the frame.
    void aPlayingMpvIsSoughtAndPlaysOn() {
        QTemporaryDir dir(QDir::currentPath() + QStringLiteral("/player_port_test-XXXXXX"));
        QVERIFY(dir.isValid());
        const QString path = oneByteFile(dir);
        QVERIFY(!path.isEmpty());
        const std::string file = path.toStdString();
        Rig r;
        r.player.ready();
        r.player.run(open(moment(file, 5'000'000, 3)));
        QCOMPARE(r.openWith(), firstWithNoFile);
        QVERIFY(r.mpv.last().startsWith("loadfile "));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened"});

        r.player.run(open(moment(file, 5'000'000, 3)));
        QCOMPARE(r.openWith(path, false), firstWithAFile);
        QCOMPARE(r.mpv.last(), "set sid 3");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "set sub-visibility yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 0:00:05.000000 absolute+exact");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 0:00:05.000000 absolute+exact");
        r.player.restarted();
        QCOMPARE(r.mpv.last(), "set pause no");
        QCOMPARE(r.reports, QStringList{"opened"});
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, (QStringList{"opened", "opened"}));
    }

    // As the terminal's: a seek mpv refuses, whatever the error, loads the file instead, and the
    // Open goes on as that load, the whole of it: paused first, then the loadfile, which lands when
    // playback starts and fails when its file ends on an error, then playing again if mpv played
    // before the Open. A file mpv still loads is the case it is for.
    void aRefusedSeekLoadsTheFileAgain() {
        QTemporaryDir dir(QDir::currentPath() + QStringLiteral("/player_port_test-XXXXXX"));
        QVERIFY(dir.isValid());
        const QString path = oneByteFile(dir);
        QVERIFY(!path.isEmpty());
        const std::string file = path.toStdString();
        Rig r;
        r.player.ready();
        r.player.run(open(moment(file, 5'000'000, 3)));
        QCOMPARE(r.openWith(), firstWithNoFile);
        QVERIFY(r.mpv.last().startsWith("loadfile "));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened"});

        r.player.run(open(moment(file, 5'000'000, 3)));
        QCOMPARE(r.openWith(path, false), firstWithAFile);
        QCOMPARE(r.mpv.last(), "set sid 3");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "set sub-visibility yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 0:00:05.000000 absolute+exact");
        r.mpv.answer(r.player, {}, refused);
        QCOMPARE(r.mpv.last(), "set pause yes");
        r.mpv.answer(r.player);
        QCOMPARE(
            r.mpv.last(),
            QStringLiteral("loadfile %1 replace -1 start=0:00:05.000000,sid=3,sub-visibility=yes")
                .arg(path));
        r.mpv.answer(r.player);
        r.player.ended(false);
        QCOMPARE(r.reports, QStringList{"opened"});
        r.player.restarted();
        QCOMPARE(r.mpv.last(), "set pause no");
        QCOMPARE(r.reports, QStringList{"opened"});
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, (QStringList{"opened", "opened"}));

        r.reports.clear();
        r.player.run(open(moment(file, 0, std::nullopt)));
        QCOMPARE(r.openWith(path), firstWithAFile);
        QCOMPARE(r.mpv.last(), "set sub-visibility yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 0:00:00.000000 absolute+exact");
        r.mpv.answer(r.player, {}, refused);
        QCOMPARE(r.mpv.last(), "set pause yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(),
                 QStringLiteral("loadfile %1 replace -1 start=0:00:00.000000,sub-visibility=yes")
                     .arg(path));
        r.mpv.answer(r.player);
        r.player.ended(true);
        QCOMPARE(r.reports, QStringList{"opened: mpv could not open a.mkv"});
    }

    // A setting the seek needs, refused as the seek can be, loads the file instead too, and mpv,
    // which played before the Open, plays again once the load has landed.
    void aRefusedSettingOfTheSeekLoadsTheFileAgain() {
        QTemporaryDir dir(QDir::currentPath() + QStringLiteral("/player_port_test-XXXXXX"));
        QVERIFY(dir.isValid());
        const QString path = oneByteFile(dir);
        QVERIFY(!path.isEmpty());
        const std::string file = path.toStdString();
        Rig r;
        r.player.ready();
        r.player.run(open(moment(file, 5'000'000, 3)));
        QCOMPARE(r.openWith(), firstWithNoFile);
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened"});

        r.player.run(open(moment(file, 5'000'000, 3)));
        QCOMPARE(r.openWith(path, false), firstWithAFile);
        QCOMPARE(r.mpv.last(), "set sid 3");
        r.mpv.answer(r.player, {}, refused);
        QCOMPARE(r.mpv.last(), "set pause yes");
        r.mpv.answer(r.player);
        QCOMPARE(
            r.mpv.last(),
            QStringLiteral("loadfile %1 replace -1 start=0:00:05.000000,sid=3,sub-visibility=yes")
                .arg(path));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.mpv.last(), "set pause no");
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, (QStringList{"opened", "opened"}));
    }

    // Why a seek meets a file still loading: an Open that went on, its settle time up, while mpv
    // said nothing. The reload then lands on a restart after its own answer only: one before it is
    // the earlier load's, late.
    void aReloadLandsOnlyOnARestartAfterItsAnswer() {
        QTemporaryDir dir(QDir::currentPath() + QStringLiteral("/player_port_test-XXXXXX"));
        QVERIFY(dir.isValid());
        const QString path = oneByteFile(dir);
        QVERIFY(!path.isEmpty());
        const std::string file = path.toStdString();
        Rig r(std::chrono::milliseconds(300));
        r.player.ready();
        r.player.run(open(moment(file, 0, std::nullopt)));
        QCOMPARE(r.openWith(), firstWithNoFile);
        QVERIFY(r.mpv.last().startsWith("loadfile "));
        r.mpv.answer(r.player);
        QTRY_COMPARE_WITH_TIMEOUT(r.reports, QStringList{"opened"}, 2000);

        r.player.run(open(moment(file, 0, std::nullopt)));
        QCOMPARE(r.openWith(path), firstWithAFile);
        QCOMPARE(r.mpv.last(), "set sub-visibility yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 0:00:00.000000 absolute+exact");
        r.mpv.answer(r.player, {}, refused);
        QCOMPARE(r.mpv.last(), "set pause yes");
        r.mpv.answer(r.player);
        const QString reload =
            QStringLiteral("loadfile %1 replace -1 start=0:00:00.000000,sub-visibility=yes")
                .arg(path);
        QCOMPARE(r.mpv.last(), reload);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened"});
        QCOMPARE(r.mpv.last(), reload);
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, (QStringList{"opened", "opened"}));
    }

    // Only the load's own end fails it: the file it replaces ends too, stopped.
    void aLoadThatFailsSaysSo() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/dir/b.mkv", 0, std::nullopt)));
        QCOMPARE(r.openWith(), firstWithNoFile);
        r.mpv.answer(r.player);
        r.player.ended(false);
        QCOMPARE(r.reports, QStringList());
        r.player.ended(true);
        QCOMPARE(r.reports, QStringList{"opened: mpv could not open b.mkv"});
        // Once the Open has reported, a later end is no one's.
        r.player.ended(true);
        QCOMPARE(r.reports.size(), 1);
    }

    // The position is read only once playback starts again after mpv's answer to the seek: a
    // restart before the answer was an earlier request's, and a position, before or after, can
    // be older than the seek, as mpv holds one until its queue of events is empty.
    void aSeekIsReadOnlyOnceItHasLanded() {
        Rig r;
        r.loaded();
        r.player.run(seek(5));
        QCOMPARE(r.mpv.last(), "get time-pos");
        r.mpv.answer(r.player, 10.0);
        QCOMPARE(r.mpv.last(), "seek 5 relative+exact");
        r.player.restarted();
        r.player.moved();
        QCOMPARE(r.mpv.last(), "seek 5 relative+exact");
        r.mpv.answer(r.player);
        r.player.moved();
        QCOMPARE(r.mpv.last(), "seek 5 relative+exact");
        r.player.restarted();
        QCOMPARE(r.mpv.last(), "get pause");
        r.mpv.answer(r.player, true);
        QCOMPARE(r.mpv.last(), "get time-pos");
        r.mpv.answer(r.player, 15.0004);
        QCOMPARE(r.reports, QStringList{"played paused 15000"});
    }

    // Each playback key's command, and what lands it: playback starting again after a seek, a
    // step back among them, and the position changing after a step on, which starts none. A pause
    // is no move, so where it is is read at once.
    void eachPlaybackHasItsCommand() {
        Rig r;
        r.loaded();
        const std::vector<std::tuple<Playback, QString, bool>> keys = {
            {Playback(Playback::kSeek{-1}), "seek -1 relative+exact", true},
            {Playback(Playback::kNextFrame{}), "frame-step", false},
            {Playback(Playback::kPreviousFrame{}), "frame-back-step", true},
        };
        for (const auto &[p, command, onRestart] : keys) {
            r.player.run(playback(p));
            r.mpv.answer(r.player, 1.0);
            QCOMPARE(r.mpv.last(), command);
            r.mpv.answer(r.player);
            if (onRestart) {
                r.player.moved();
                QCOMPARE(r.mpv.last(), command);
                r.player.restarted();
            } else {
                r.player.restarted();
                QCOMPARE(r.mpv.last(), command);
                r.player.moved();
            }
            QCOMPARE(r.mpv.last(), "get pause");
            r.mpv.answer(r.player, false);
            r.mpv.answer(r.player, 2.5);
        }
        r.player.run(playback(Playback(Playback::kPause{})));
        r.mpv.answer(r.player, 1.0);
        QCOMPARE(r.mpv.last(), "cycle pause");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "get pause");
        r.mpv.answer(r.player, true);
        r.mpv.answer(r.player, 2.5);
        QCOMPARE(r.reports, (QStringList{"played playing 2500", "played playing 2500",
                                         "played playing 2500", "played paused 2500"}));
    }

    // A seek mpv says nothing more of is read once the settle time is up, not before.
    void aMoveMpvSaysNothingOfLandsWhenTheSettleTimeEnds() {
        Rig r(std::chrono::milliseconds(300));
        r.loaded();
        r.player.run(seek(5));
        r.mpv.answer(r.player, 1.0);
        QElapsedTimer waited;
        waited.start();
        r.mpv.answer(r.player);
        QTRY_COMPARE_WITH_TIMEOUT(r.mpv.last(), "get pause", 2000);
        QVERIFY2(waited.elapsed() >= 300, qPrintable(QString::number(waited.elapsed())));
        r.mpv.answer(r.player, false);
        r.mpv.answer(r.player, 6.0);
        QCOMPARE(r.reports, QStringList{"played playing 6000"});
    }

    // As the terminal: no position is no file open, or one still opening.
    void aPlaybackWithNothingOpenSaysSo() {
        Rig r;
        r.player.ready();
        r.player.run(seek(5));
        r.mpv.answer(r.player, {}, unavailable);
        QCOMPARE(r.mpv.last(), "get path");
        r.mpv.answer(r.player, {}, unavailable);
        r.player.run(seek(5));
        r.mpv.answer(r.player, {}, unavailable);
        r.mpv.answer(r.player, QStringLiteral("/m/a.mkv"));
        QCOMPARE(r.reports, (QStringList{"play failed: mpv has no file open (o opens one)",
                                         "play failed: mpv is still opening the file"}));
    }

    // Any other error ends the command with mpv's words, as the terminal's do, but a refused
    // unpause after an Open has landed: mpv is still paused on the frame, the moment it opened.
    void mpvsErrorsEndTheirCommand() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.mpv.answer(r.player, {}, QStringLiteral("unknown error"));
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.openWith();
        r.mpv.answer(r.player, {}, QStringLiteral("loading failed"));
        r.player.run(seek(5));
        r.mpv.answer(r.player, {}, QStringLiteral("unknown error"));
        r.player.run(seek(5));
        r.mpv.answer(r.player, {}, unavailable);
        r.mpv.answer(r.player, {}, QStringLiteral("unknown error"));
        r.player.run(seek(5));
        r.mpv.answer(r.player, 1.0);
        r.mpv.answer(r.player, {}, QStringLiteral("error running command"));
        r.player.run(seek(5));
        r.mpv.answer(r.player, 1.0);
        r.mpv.answer(r.player);
        r.player.restarted();
        r.mpv.answer(r.player, {}, unavailable);
        r.player.run(seek(5));
        r.mpv.answer(r.player, 1.0);
        r.mpv.answer(r.player);
        r.player.restarted();
        r.mpv.answer(r.player, true);
        r.mpv.answer(r.player, {}, unavailable);
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.openWith("/m/b.mkv", false);
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.mpv.last(), "set pause no");
        r.mpv.answer(r.player, {}, QStringLiteral("unknown error"));
        r.player.run(close());
        r.mpv.answer(r.player, {}, QStringLiteral("unknown error"));
        QCOMPARE(r.reports,
                 (QStringList{"opened: mpv: unknown error", "opened: mpv: loading failed",
                              "play failed: mpv: unknown error", "play failed: mpv: unknown error",
                              "play failed: mpv: error running command",
                              "play failed: mpv: property unavailable",
                              "play failed: mpv: property unavailable", "opened",
                              "closed: mpv: unknown error"}));
    }

    // mpv may run requests sent together in any order: one command's waits for the one before.
    void commandsRunOneAtATime() {
        Rig r;
        r.loaded();
        r.player.run(seek(5));
        r.player.run(close());
        r.mpv.answer(r.player, 1.0);
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 5 relative+exact");
        r.player.restarted();
        r.mpv.answer(r.player, false);
        r.mpv.answer(r.player, 6.0);
        QCOMPARE(r.mpv.last(), "stop");
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, (QStringList{"played playing 6000", "closed"}));
    }

    // Review has ended: playback stops, and what was under way or waiting reports nothing.
    void releaseStopsAndReportsNothingMore() {
        Rig r(std::chrono::milliseconds(50));
        r.loaded();
        r.player.run(seek(5));
        r.mpv.answer(r.player, 1.0);
        const std::uint64_t seekId = r.mpv.requests.back().id;
        r.player.run(close());
        r.player.release();
        QCOMPARE(r.mpv.last(), "stop");
        r.mpv.answer(r.player);
        r.player.answered(seekId, {}, {});
        r.player.restarted();
        QTest::qWait(100);
        QCOMPARE(r.mpv.last(), "stop");
        QCOMPARE(r.reports, QStringList());
    }

    // Without its mpv (the window that showed it has gone), the player can show nothing, and
    // asks nothing of it.
    void withoutItsMpvAnOpenWaits() {
        Rig r;
        r.player.ready();
        r.player.attach(nullptr);
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.player.release();
        QCOMPARE(r.mpv.requests.size(), 0u);
        QCOMPARE(r.reports, QStringList());
    }

    // Another mpv in place of the one a request waits on, which will not answer it now: the
    // command under way is run again as if it came now, nothing can be open yet, and the next
    // runs on the new mpv once it can show video. The same mpv given again changes nothing.
    void aCommandUnderWayWhenItsMpvGoesIsRunAgain() {
        Rig r;
        r.loaded();
        r.player.run(seek(5));
        r.player.run(open(moment("/m/b.mkv", 0, std::nullopt)));
        r.mpv.answer(r.player, 1.0);
        QCOMPARE(r.mpv.last(), "seek 5 relative+exact");
        r.player.attach(&r.mpv);
        QCOMPARE(r.reports, QStringList());
        const auto sent = r.mpv.requests.size();
        FakeMpv other;
        r.player.attach(&other);
        QCOMPARE(r.reports, QStringList{"play failed: mpv has no file open (o opens one)"});
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(other.requests.size(), 0u);
        r.player.ready();
        QCOMPARE(answerOpening(other, r.player), firstWithNoFile);
        QCOMPARE(other.last(),
                 "loadfile /m/b.mkv replace -1 start=0:00:00.000000,sub-visibility=yes");
        QCOMPARE(r.mpv.requests.size(), sent);
        QCOMPARE(r.reports.size(), 1);
    }

    // The mpv gone while a move settles: the wait goes with it, so that its end asks nothing of
    // an mpv that is no more, and the move is answered as if it came now.
    void aMoveSettlingWhenItsMpvGoesAsksNothingMore() {
        Rig r(std::chrono::milliseconds(300));
        r.loaded();
        r.player.run(seek(5));
        r.mpv.answer(r.player, 1.0);
        r.mpv.answer(r.player);
        const auto sent = r.mpv.requests.size();
        r.player.attach(nullptr);
        QTest::qWait(400);
        r.player.restarted();
        r.player.moved();
        QCOMPARE(r.mpv.requests.size(), sent);
        QCOMPARE(r.reports, QStringList{"play failed: mpv has no file open (o opens one)"});
    }
};

QTEST_GUILESS_MAIN(PlayerPortTest)
#include "player_port_test.moc"
