// The player on a fake mpv, with no session and no window: which requests each command sends, in
// which order, what it waits for, and what it reports. And the pane's mapping of mpv's end names
// (MpvItem::endOf), which needs MpvQt's headers but no pane.
#include "fake_mpv.h"
#include "mpv_item.h"
#include "player_port.h"
#include "said.h"

#include <QElapsedTimer>
#include <QSignalSpy>
#include <QTest>

#include <chrono>
#include <functional>
#include <optional>
#include <string>
#include <tuple>
#include <vector>

using namespace subcheck_ffi;

namespace {

using End = PlayerPort::End;

const QString unavailable = QStringLiteral("property unavailable");
// mpv's text for a command it refuses, as a seek of a file it still loads.
const QString refused = QStringLiteral("error running command");

Moment moment(const std::string &file, std::int64_t at, std::optional<std::uint64_t> sid) {
    return Moment{file, at, sid};
}

// Options core never gives: the port hands mpv whatever list review gives it, and names none
// itself, which core's own pair would hide.
const std::vector<FileOption> given = {{"title", "t"}, {"osd-level", "1"}};

PlayerCommand open(const Moment &m, How how = How::kLoad, bool play = false) {
    return PlayerCommand(PlayerCommand::kOpen{m, how, given, play});
}

PlayerCommand playback(Playback p) { return PlayerCommand(PlayerCommand::kPlayback{std::move(p)}); }

PlayerCommand seek(std::int32_t by) { return playback(Playback(Playback::kSeek{by})); }

PlayerCommand close() { return PlayerCommand(PlayerCommand::kClose{}); }

PlayerCommand pause() { return playback(Playback(Playback::kPause{})); }

// What an Open asks of mpv first, before its move: where mpv is; whether it plays, with a file
// open only; then the pause every move starts with.
const QStringList firstWithNoFile = {"get path", "set pause yes"};
const QStringList firstWithAFile = {"get path", "get pause", "set pause yes"};

// The load of `file` at `start`, with its subtitle track if it has one and the options the tests
// give, paused or playing.
QString loadOf(const QString &file, const QString &start, const QString &sid = {},
               bool paused = true) {
    return QStringLiteral("loadfile %1 replace -1 start=%2%3,sub-visibility=yes,title=t,"
                          "osd-level=1,pause=%4")
        .arg(file, start, sid.isEmpty() ? QString() : QStringLiteral(",sid=") + sid,
             paused ? QStringLiteral("yes") : QStringLiteral("no"));
}

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

// What whenFileEnds says, each time, as a word in `said`.
std::function<void(bool)> saidInto(QStringList &said) {
    return [&said](bool ended) {
        said << (ended ? QStringLiteral("ended") : QStringLiteral("not ended"));
    };
}

// The player, its fake mpv, and what it has reported, each as one line. Its limits are far longer
// than any test waits, but where a test gives one of its own.
struct Rig {
    PlayerPort player;
    FakeMpv mpv;
    QStringList reports;

    explicit Rig(std::chrono::milliseconds settle = std::chrono::seconds(10),
                 std::chrono::milliseconds answer = std::chrono::seconds(10),
                 std::chrono::milliseconds ready = std::chrono::seconds(10))
        : player(settle, answer, ready) {
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
    // The newest moment only, once mpv can show it; loaded paused, as mpv had no file open to play.
    void anOpenWaitsUntilMpvCanShowVideo() {
        Rig r;
        r.player.run(open(moment("/m/a.mkv", 1'000'000, 1)));
        r.player.run(open(moment("/m/b.mkv", 2'004'500, 2)));
        QCOMPARE(r.mpv.requests.size(), 0u);
        r.player.ready();
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/b.mkv", "0:00:02.004500", "2"));
        QCOMPARE(r.reports, QStringList());
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened paused"});
        QCOMPARE(r.mpv.requests.size(), 3u);
    }

    // Nothing can be open before mpv can show video: a Playback says so, a Space among them
    // with no Open waiting, or a seek with one that the file is still opening; and a Close drops
    // the Open that waits, or with none says nothing is open.
    void beforeThenAPlaybackOrACloseIsAnsweredAtOnce() {
        Rig r;
        r.player.run(seek(5));
        r.player.run(pause());
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.player.run(seek(5));
        r.player.run(close());
        r.player.run(close());
        r.player.ready();
        QCOMPARE(r.reports,
                 (QStringList{"play failed: no file", "play failed: no file",
                              "play failed: still opening", "closed", "closed: not open"}));
        QCOMPARE(r.mpv.requests.size(), 0u);
    }

    // A Space before the pane can show video has nothing open to toggle, so it is the waiting
    // Open's, which then lands the other way from its play: nothing can play in a pane that is not
    // ready, so the Open's play is how it lands. Nothing is reported for the Space; the Open says
    // how it landed.
    void aSpaceBeforeThePaneIsReadyLandsTheOpenPlaying() {
        Rig r;
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.player.run(pause());
        QCOMPARE(r.reports, QStringList());
        QCOMPARE(r.mpv.requests.size(), 0u);
        r.player.ready();
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/a.mkv", "0:00:00.000000", {}, false));
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, QStringList());
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened playing"});
    }

    // A toggle: a second Space undoes the first.
    void twoSpacesBeforeThePaneIsReadyLandItPaused() {
        Rig r;
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.player.run(pause());
        r.player.run(pause());
        r.player.ready();
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/a.mkv", "0:00:00.000000"));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened paused"});
    }

    // The Space was for the video about to show, whichever it is, as review keeps one across a
    // newer press: a newer Open in the waiting one's place plays, as the waiting one would have.
    void aNewerOpenKeepsASpaceMadeBeforeThePaneIsReady() {
        Rig r;
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.player.run(pause());
        r.player.run(open(moment("/m/b.mkv", 0, std::nullopt)));
        r.player.ready();
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/b.mkv", "0:00:00.000000", {}, false));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened playing"});
    }

    // A toggle, not "always play": an Open that says play, as review held a Space for it, lands
    // paused after a Space in the port.
    void aSpaceOnAWaitingOpenThatSaysPlayLandsItPaused() {
        Rig r;
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt), How::kLoad, true));
        r.player.run(pause());
        r.player.ready();
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/a.mkv", "0:00:00.000000"));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened paused"});
    }

    // An Open that says play, sent for a Space review held, keeps that Space for the newer Open in
    // its place, as a Space in the port is kept.
    void aNewerOpenKeepsAnOlderOpensPlay() {
        Rig r;
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt), How::kLoad, true));
        r.player.run(open(moment("/m/b.mkv", 0, std::nullopt)));
        r.player.ready();
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/b.mkv", "0:00:00.000000", {}, false));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened playing"});
    }

    // A newer Open that says play lands playing, as review's line says "will play" for it, whatever
    // a Space made of the Open it replaces.
    void aNewerOpenThatSaysPlayStillPlaysAfterAFlip() {
        Rig r;
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.player.run(pause());
        r.player.run(open(moment("/m/c.mkv", 0, std::nullopt), How::kLoad, true));
        r.player.ready();
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/c.mkv", "0:00:00.000000", {}, false));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened playing"});
    }

    // Two Spaces undo each other, the newer Open in the waiting one's place too: it lands as it
    // says.
    void aNewerOpenAfterTwoSpacesLandsAsItSays() {
        Rig r;
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.player.run(pause());
        r.player.run(pause());
        r.player.run(open(moment("/m/b.mkv", 0, std::nullopt)));
        r.player.ready();
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/b.mkv", "0:00:00.000000"));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened paused"});
    }

    // A Space belongs to the wait it came in: a Close drops the waiting Open and the Space with
    // it, as does the end of the Open's time for the pane, and the release, so the next Open lands
    // as it says.
    void aCloseOrTheWaitsEndForgetsASpace() {
        Rig r;
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.player.run(pause());
        r.player.run(close());
        QCOMPARE(r.reports, QStringList{"closed"});
        r.player.run(open(moment("/m/b.mkv", 0, std::nullopt)));
        r.player.ready();
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/b.mkv", "0:00:00.000000"));

        Rig slow(std::chrono::seconds(10), std::chrono::seconds(10),
                 std::chrono::milliseconds(300));
        slow.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        slow.player.run(pause());
        QTRY_COMPARE_WITH_TIMEOUT(slow.reports, QStringList{"open failed: no video"}, 2000);
        slow.player.run(open(moment("/m/b.mkv", 0, std::nullopt)));
        slow.player.ready();
        QCOMPARE(slow.openWith(), firstWithNoFile);
        QCOMPARE(slow.mpv.last(), loadOf("/m/b.mkv", "0:00:00.000000"));

        Rig released;
        released.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        released.player.run(pause());
        released.player.release();
        released.player.run(open(moment("/m/b.mkv", 0, std::nullopt)));
        released.player.ready();
        QCOMPARE(released.openWith(), firstWithNoFile);
        QCOMPARE(released.mpv.last(), loadOf("/m/b.mkv", "0:00:00.000000"));
        QCOMPARE(released.reports, QStringList());
    }

    // Paused before mpv gets there, so that a playing mpv does not go on past the frame, and
    // opened once playback starts there, loaded paused as mpv had no file open, then one paused:
    // a position is no landing for a load, as mpv can hold one from before it until now. Whether
    // mpv plays is read only with a file open, and before anything is sent. The subtitle track
    // only when review names one; the time in µs, as review gives it.
    void anotherFileIsLoaded() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 62'504'500, 2)));
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/a.mkv", "0:01:02.504500", "2"));
        r.mpv.answer(r.player);
        r.player.moved();
        QCOMPARE(r.reports, QStringList());
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened paused"});
        QCOMPARE(r.mpv.requests.size(), 3u);

        r.player.run(open(moment("/m/b.mkv", 0, std::nullopt)));
        QCOMPARE(r.openWith("/m/a.mkv"), firstWithAFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/b.mkv", "0:00:00.000000"));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.mpv.requests.size(), 7u);
        QCOMPARE(r.reports, (QStringList{"opened paused", "opened paused"}));
    }

    // Whether mpv plays decides only whether it plays again once there: a read of it that mpv
    // fails, or that gives no flag, counts as paused, and the Open goes on, loading paused.
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
        QCOMPARE(r.mpv.last(), loadOf("/m/b.mkv", "0:00:00.000000"));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened paused"});

        r.player.run(open(moment("/m/c.mkv", 0, std::nullopt)));
        r.mpv.answer(r.player, QStringLiteral("/m/b.mkv"));
        QCOMPARE(r.mpv.last(), "get pause");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "set pause yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), loadOf("/m/c.mkv", "0:00:00.000000"));
        r.mpv.answer(r.player);
        const auto sent = r.mpv.requests.size();
        r.player.restarted();
        QCOMPARE(r.reports, (QStringList{"opened paused", "opened paused"}));
        QCOMPARE(r.mpv.requests.size(), sent);
    }

    // As the terminal's: mpv refusing the read of the path, whatever its words, counts as no file
    // open, so even a seek loads, paused, with no read of whether mpv plays. No answer at all is
    // no refusal, and fails the Open.
    void aRefusedReadOfThePathLoads() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt), How::kSeek));
        QCOMPARE(r.mpv.last(), "get path");
        r.mpv.answer(r.player, {}, QStringLiteral("unknown error"));
        QCOMPARE(r.mpv.last(), "set pause yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), loadOf("/m/a.mkv", "0:00:00.000000"));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened paused"});

        Rig silent(std::chrono::seconds(10), std::chrono::milliseconds(300));
        silent.player.ready();
        silent.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        QTRY_COMPARE_WITH_TIMEOUT(silent.reports, QStringList{"open failed: no answer"}, 2000);
        QCOMPARE(silent.mpv.requests.size(), 1u);
    }

    // Another file while mpv plays one: paused, then loaded playing, by the load's own option, so
    // that a user's reset-on-next-file cannot undo it; nothing after it, and the Open reports once
    // the load has landed.
    void aPlayingMpvLoadsAnotherFileAndPlaysOn() {
        Rig r;
        r.loaded();
        r.player.run(open(moment("/m/b.mkv", 2'004'500, 2)));
        QCOMPARE(r.openWith("/m/a.mkv", false), firstWithAFile);
        const QString load = loadOf("/m/b.mkv", "0:00:02.004500", "2", false);
        QCOMPARE(r.mpv.last(), load);
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, QStringList());
        r.player.restarted();
        QCOMPARE(r.mpv.last(), load);
        QCOMPARE(r.reports, QStringList{"opened playing"});
    }

    // An Open that says play lands playing, whatever mpv's own state, as the user pressed Space
    // while it waited: by the load's option, with no file open and with one open and paused; by
    // the unpause once the seek has landed, as a playing mpv's seek does.
    void anOpenThatSaysPlayLandsPlaying() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt), How::kLoad, true));
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/a.mkv", "0:00:00.000000", {}, false));
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, QStringList());
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened playing"});

        r.player.run(open(moment("/m/b.mkv", 0, std::nullopt), How::kLoad, true));
        QCOMPARE(r.openWith("/m/a.mkv"), firstWithAFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/b.mkv", "0:00:00.000000", {}, false));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, (QStringList{"opened playing", "opened playing"}));

        r.player.run(open(moment("/m/b.mkv", 5'000'000, std::nullopt), How::kSeek, true));
        QCOMPARE(r.openWith("/m/b.mkv"), firstWithAFile);
        QCOMPARE(r.mpv.last(), "set sub-visibility yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 0:00:05.000000 absolute+exact");
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.mpv.last(), "set pause no");
        QCOMPARE(r.reports.size(), 2);
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, (QStringList{"opened playing", "opened playing", "opened playing"}));
    }

    // Review says seek when it had the player load the file, unchanged since: sought, if mpv still
    // has it open, its settings first, in core's order, and left paused with nothing after it.
    // Told to seek with another file open, or none, the port loads; told to load, it loads, the
    // file it has open or not (review's word: the file changed on disk).
    void aSeekIsSoughtInTheFileMpvHasOpenAndAnythingElseLoaded() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 5'000'000, 3), How::kSeek));
        QCOMPARE(r.openWith("/m/a.mkv"), firstWithAFile);
        QCOMPARE(r.mpv.last(), "set sid 3");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "set sub-visibility yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 0:00:05.000000 absolute+exact");
        r.mpv.answer(r.player);
        auto sent = r.mpv.requests.size();
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened paused"});
        QCOMPARE(r.mpv.requests.size(), sent);

        const QString load = loadOf("/m/a.mkv", "0:00:05.000000", "3");
        for (const auto &[how, path] : {std::pair{How::kSeek, QStringLiteral("/m/b.mkv")},
                                        std::pair{How::kLoad, QStringLiteral("/m/a.mkv")}}) {
            r.player.run(open(moment("/m/a.mkv", 5'000'000, 3), how));
            QCOMPARE(r.openWith(path), firstWithAFile);
            QCOMPARE(r.mpv.last(), load);
            r.mpv.answer(r.player);
            r.player.restarted();
        }
        r.player.run(open(moment("/m/a.mkv", 5'000'000, 3), How::kSeek));
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(), load);
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports.size(), 4);
    }

    // While mpv plays the file: sought paused, and playing again only once the seek has landed, so
    // that it plays on from the frame; the Open reports once mpv has answered that.
    void aPlayingMpvIsSoughtAndPlaysOn() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 5'000'000, std::nullopt), How::kSeek));
        QCOMPARE(r.openWith("/m/a.mkv", false), firstWithAFile);
        QCOMPARE(r.mpv.last(), "set sub-visibility yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 0:00:05.000000 absolute+exact");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 0:00:05.000000 absolute+exact");
        r.player.restarted();
        QCOMPARE(r.mpv.last(), "set pause no");
        QCOMPARE(r.reports, QStringList());
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, QStringList{"opened playing"});
    }

    // As the terminal's: a seek mpv refuses, whatever the error, loads the file instead, and the
    // Open goes on as that load, mpv paused already: the loadfile, which lands when playback
    // starts, playing if mpv played before the Open, and fails when its file ends on an error. A
    // file mpv still loads is the case it is for.
    void aRefusedSeekLoadsTheFileAgain() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 5'000'000, 3), How::kSeek));
        QCOMPARE(r.openWith("/m/a.mkv", false), firstWithAFile);
        QCOMPARE(r.mpv.last(), "set sid 3");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "set sub-visibility yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 0:00:05.000000 absolute+exact");
        r.mpv.answer(r.player, {}, refused);
        QCOMPARE(r.mpv.last(), loadOf("/m/a.mkv", "0:00:05.000000", "3", false));
        r.mpv.answer(r.player);
        r.player.ended(End::Stopped);
        QCOMPARE(r.reports, QStringList());
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened playing"});

        r.reports.clear();
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt), How::kSeek));
        QCOMPARE(r.openWith("/m/a.mkv"), firstWithAFile);
        QCOMPARE(r.mpv.last(), "set sub-visibility yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 0:00:00.000000 absolute+exact");
        r.mpv.answer(r.player, {}, refused);
        QCOMPARE(r.mpv.last(), loadOf("/m/a.mkv", "0:00:00.000000"));
        r.mpv.answer(r.player);
        r.player.ended(End::Failed);
        QCOMPARE(r.reports, QStringList{"open failed: could not open /m/a.mkv"});
    }

    // A setting the seek needs, refused as the seek can be, loads the file instead too, the
    // terminal's rule as much as the window's.
    void aRefusedSettingOfTheSeekLoadsTheFileAgain() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 5'000'000, 3), How::kSeek));
        QCOMPARE(r.openWith("/m/a.mkv", false), firstWithAFile);
        QCOMPARE(r.mpv.last(), "set sid 3");
        r.mpv.answer(r.player, {}, refused);
        QCOMPARE(r.mpv.last(), loadOf("/m/a.mkv", "0:00:05.000000", "3", false));
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened playing"});
    }

    // Only a pause is no step of the seek: mpv refuses none while it answers at all, so a refusal
    // fails the Open rather than load.
    void aRefusedPauseFailsTheOpen() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt), How::kSeek));
        r.mpv.answer(r.player, QStringLiteral("/m/a.mkv"));
        r.mpv.answer(r.player, true);
        QCOMPARE(r.mpv.last(), "set pause yes");
        r.mpv.answer(r.player, {}, QStringLiteral("unknown error"));
        QCOMPARE(r.reports, QStringList{"open failed: mpv: unknown error"});
        QCOMPARE(r.mpv.requests.size(), 3u);
    }

    // Why a seek meets a file still loading: an Open that went on, its settle time up, while mpv
    // said nothing. The reload then lands on a restart after its own answer only: one before it is
    // the earlier load's, late.
    void aReloadLandsOnlyOnARestartAfterItsAnswer() {
        Rig r(std::chrono::milliseconds(300));
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        QCOMPARE(r.openWith(), firstWithNoFile);
        const QString load = loadOf("/m/a.mkv", "0:00:00.000000");
        QCOMPARE(r.mpv.last(), load);
        r.mpv.answer(r.player);
        QTRY_COMPARE_WITH_TIMEOUT(r.reports, QStringList{"opened paused"}, 2000);

        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt), How::kSeek));
        QCOMPARE(r.openWith("/m/a.mkv"), firstWithAFile);
        QCOMPARE(r.mpv.last(), "set sub-visibility yes");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "seek 0:00:00.000000 absolute+exact");
        r.mpv.answer(r.player, {}, refused);
        QCOMPARE(r.mpv.last(), load);
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened paused"});
        QCOMPARE(r.mpv.last(), load);
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.reports, (QStringList{"opened paused", "opened paused"}));
    }

    // Only the load's own end fails it: the file it replaces ends too, stopped. Its file is
    // named as review gave it.
    void aLoadThatFailsSaysSo() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/dir/b.mkv", 0, std::nullopt)));
        QCOMPARE(r.openWith(), firstWithNoFile);
        r.mpv.answer(r.player);
        r.player.ended(End::Stopped);
        QCOMPARE(r.reports, QStringList());
        r.player.ended(End::Failed);
        QCOMPARE(r.reports, QStringList{"open failed: could not open /m/dir/b.mkv"});
        // Once the Open has reported, a later end is no one's.
        r.player.ended(End::Failed);
        QCOMPARE(r.reports.size(), 1);
    }

    // A load that outlasts its settle time reports opened. Should mpv then end that file on an
    // error, before it ever played, that is a report of its own, after the stop of the file it
    // replaced too. Once it plays, or the port loads or stops, an end on an error is no longer its.
    void aLoadThatFailsAfterItReportedOpenedSaysSoLate() {
        Rig r(std::chrono::milliseconds(100));
        r.player.ready();
        const auto loadLate = [&r](const std::string &file) {
            r.reports.clear();
            r.player.run(open(moment(file, 0, std::nullopt)));
            r.openWith();
            r.mpv.answer(r.player);
            QTRY_COMPARE_WITH_TIMEOUT(r.reports, QStringList{"opened paused"}, 2000);
        };
        loadLate("/m/a.mkv");
        r.player.ended(End::Failed);
        QCOMPARE(r.reports, (QStringList{"opened paused", "failed: could not open /m/a.mkv"}));
        r.player.ended(End::Failed);
        QCOMPARE(r.reports.size(), 2);
        loadLate("/m/b.mkv");
        r.player.restarted();
        r.player.ended(End::Failed);
        QCOMPARE(r.reports, QStringList{"opened paused"});
        loadLate("/m/c.mkv");
        r.player.ended(End::Stopped);
        r.player.ended(End::Failed);
        QCOMPARE(r.reports, (QStringList{"opened paused", "failed: could not open /m/c.mkv"}));
        // A new Open's load sent, the late load is given up: an end on an error before its answer
        // is the late file's, which the user has moved on from.
        loadLate("/m/d.mkv");
        r.player.run(open(moment("/m/e.mkv", 0, std::nullopt)));
        r.openWith("/m/d.mkv");
        QVERIFY(r.mpv.last().startsWith("loadfile /m/e.mkv"));
        r.player.ended(End::Failed);
        QCOMPARE(r.reports, QStringList{"opened paused"});
        // A stop sent, whatever ended was stopped.
        r.mpv.answer(r.player);
        r.player.restarted();
        loadLate("/m/f.mkv");
        r.player.run(close());
        r.mpv.answer(r.player, QStringLiteral("/m/f.mkv"));
        QCOMPARE(r.mpv.last(), "stop");
        r.player.ended(End::Failed);
        QCOMPARE(r.reports, QStringList{"opened paused"});
    }

    // A late load's file still opens, and mpv keeps a pause toggled meanwhile, landing the file as
    // the toggle leaves it: so a Space goes to mpv, and is reported as how the open will land. The
    // load is still awaited. A seek still waits for the file.
    void aLateLoadsSpaceGoesToMpvAndSaysHowItWillLand() {
        Rig r(std::chrono::milliseconds(100));
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.openWith();
        r.mpv.answer(r.player);
        QTRY_COMPARE_WITH_TIMEOUT(r.reports, QStringList{"opened paused"}, 2000);
        r.reports.clear();
        r.player.run(pause());
        QCOMPARE(r.mpv.last(), "get time-pos");
        r.mpv.answer(r.player, {}, unavailable);
        QCOMPARE(r.mpv.last(), "get path");
        r.mpv.answer(r.player, QStringLiteral("/m/a.mkv"));
        QCOMPARE(r.mpv.last(), "cycle pause");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "get pause");
        QCOMPARE(r.reports, QStringList());
        r.mpv.answer(r.player, false);
        QCOMPARE(r.reports, QStringList{"opened playing"});
        QVERIFY(r.player.opening());

        r.player.run(seek(5));
        QCOMPARE(r.mpv.last(), "get time-pos");
        r.mpv.answer(r.player, {}, unavailable);
        QCOMPARE(r.mpv.last(), "get path");
        r.mpv.answer(r.player, QStringLiteral("/m/a.mkv"));
        QCOMPARE(r.reports, (QStringList{"opened playing", "play failed: still opening"}));
    }

    // The kind the pane gives the player for each end MpvQt names. A name MpvQt does not give is a
    // stop, which ends no wait early.
    void mpvsEndNamesMapToTheirKinds() {
        QCOMPARE(MpvItem::endOf(QStringLiteral("eof")), End::Finished);
        QCOMPARE(MpvItem::endOf(QStringLiteral("stop")), End::Stopped);
        QCOMPARE(MpvItem::endOf(QStringLiteral("error")), End::Failed);
        QCOMPARE(MpvItem::endOf(QStringLiteral("redirect")), End::Stopped);
    }

    // A late load that reaches its file's end will not play now: it is no longer awaited, and
    // nothing is reported, its Open reported already. The file it replaced ends stopped, and leaves
    // it awaited.
    void aLateLoadThatReachesItsFilesEndIsNoLongerOpening() {
        Rig r(std::chrono::milliseconds(100));
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.openWith();
        r.mpv.answer(r.player);
        QTRY_COMPARE_WITH_TIMEOUT(r.reports, QStringList{"opened paused"}, 2000);
        QSignalSpy changed(&r.player, &PlayerPort::openingChanged);
        r.player.ended(End::Stopped);
        QVERIFY(r.player.opening());
        QCOMPARE(changed.count(), 0);
        r.player.ended(End::Finished);
        QVERIFY(!r.player.opening());
        QCOMPARE(changed.count(), 1);
        // A later end is no one's.
        r.player.ended(End::Failed);
        r.player.ended(End::Finished);
        QCOMPARE(r.reports, QStringList{"opened paused"});
        QCOMPARE(changed.count(), 1);
    }

    // An end at the file's end while the load still settles is the load's own: the Open reports
    // opened there, as the end of the settle time would, but the load is not late, as no playback
    // starts again after it. A stop, the replaced file's, changes nothing.
    void anEndAtTheFilesEndWhileTheLoadSettlesEndsTheOpen() {
        Rig r;
        r.player.ready();
        QSignalSpy changed(&r.player, &PlayerPort::openingChanged);
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.openWith();
        r.mpv.answer(r.player);
        r.player.ended(End::Stopped);
        QCOMPARE(r.reports, QStringList());
        QVERIFY(r.player.opening());
        r.player.ended(End::Finished);
        QCOMPARE(r.reports, QStringList{"opened paused"});
        QVERIFY(!r.player.opening());
        QCOMPARE(changed.count(), 2);
        const auto sent = r.mpv.requests.size();
        r.player.restarted();
        r.player.ended(End::Failed);
        QCOMPARE(r.reports, QStringList{"opened paused"});
        QVERIFY(!r.player.opening());
        QCOMPARE(changed.count(), 2);
        QCOMPARE(r.mpv.requests.size(), sent);
    }

    // Whether an Open is under way, for the window's busy animation: from review's Open, its wait
    // for the pane among it, until it reports, and on through a load that reported opened when its
    // settle time ended, mpv having said nothing, until that plays, reaches its file's end, or mpv
    // ends it on an error. A Close ends it, and so does the release. Each change is said once.
    void openingLastsFromAnOpenToItsReportAndThroughALateLoad() {
        Rig r(std::chrono::milliseconds(100));
        QSignalSpy changed(&r.player, &PlayerPort::openingChanged);
        QVERIFY(!r.player.opening());
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        QVERIFY(r.player.opening());
        r.player.run(close());
        QVERIFY(!r.player.opening());
        QCOMPARE(changed.count(), 2);
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.player.ready();
        r.openWith();
        r.mpv.answer(r.player);
        QVERIFY(r.player.opening());
        r.player.restarted();
        QCOMPARE(r.reports, (QStringList{"closed", "opened paused"}));
        QVERIFY(!r.player.opening());
        QCOMPARE(changed.count(), 4);

        // Queued behind a Playback, then under way once that has reported.
        r.reports.clear();
        r.player.run(seek(5));
        QVERIFY(!r.player.opening());
        r.player.run(open(moment("/m/b.mkv", 0, std::nullopt)));
        QVERIFY(r.player.opening());
        r.mpv.answer(r.player, 1.0);
        r.mpv.answer(r.player);
        r.player.restarted();
        r.mpv.answer(r.player, true);
        r.mpv.answer(r.player, 5.0);
        QCOMPARE(r.reports, QStringList{"played paused 5000"});
        QCOMPARE(r.mpv.last(), "get path");
        QVERIFY(r.player.opening());
        QCOMPARE(changed.count(), 5);

        // Late: opening still, past its report, while the file it replaced ends, until it plays.
        const auto late = [&r] {
            r.openWith(QStringLiteral("/m/a.mkv"));
            r.mpv.answer(r.player);
            QTRY_COMPARE_WITH_TIMEOUT(r.reports, QStringList{"opened paused"}, 2000);
        };
        r.reports.clear();
        late();
        QVERIFY(r.player.opening());
        r.player.ended(End::Stopped);
        QVERIFY(r.player.opening());
        r.player.restarted();
        QVERIFY(!r.player.opening());
        QCOMPARE(changed.count(), 6);

        // Late, until mpv ends it on an error, reported with the load no longer waited for.
        r.reports.clear();
        r.player.run(open(moment("/m/c.mkv", 0, std::nullopt)));
        late();
        std::optional<bool> openingAsReported;
        {
            QObject watching;
            connect(&r.player, &PlayerPort::reported, &watching,
                    [&] { openingAsReported = r.player.opening(); });
            r.player.ended(End::Failed);
        }
        QCOMPARE(r.reports, (QStringList{"opened paused", "failed: could not open /m/c.mkv"}));
        QVERIFY(openingAsReported.has_value());
        QVERIFY(!*openingAsReported);
        QVERIFY(!r.player.opening());
        QCOMPARE(changed.count(), 8);

        // Late, until a Close stops it.
        r.reports.clear();
        r.player.run(open(moment("/m/d.mkv", 0, std::nullopt)));
        late();
        r.player.run(close());
        r.mpv.answer(r.player, QStringLiteral("/m/d.mkv"));
        QCOMPARE(r.mpv.last(), "stop");
        QVERIFY(!r.player.opening());
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, (QStringList{"opened paused", "closed"}));
        QCOMPARE(changed.count(), 10);

        r.player.run(open(moment("/m/e.mkv", 0, std::nullopt)));
        QVERIFY(r.player.opening());
        r.player.release();
        QVERIFY(!r.player.opening());
        QCOMPARE(changed.count(), 12);
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
        QCOMPARE(r.reports, (QStringList{"play failed: no file", "play failed: still opening"}));
    }

    // As the terminal: with nothing open, there was nothing to close; stopped all the same, as a
    // load mpv has taken and not started names no path. A read of the path that mpv refuses in
    // other words is nothing open too, as it is for an Open.
    void aCloseWithNothingOpenSaysSo() {
        Rig r;
        r.player.ready();
        r.player.run(close());
        QCOMPARE(r.mpv.last(), "get path");
        r.mpv.answer(r.player, {}, unavailable);
        QCOMPARE(r.mpv.last(), "stop");
        r.mpv.answer(r.player);
        r.player.run(close());
        r.mpv.answer(r.player, QStringLiteral("/m/a.mkv"));
        QCOMPARE(r.mpv.last(), "stop");
        r.mpv.answer(r.player);
        r.player.run(close());
        r.mpv.answer(r.player, {}, QStringLiteral("unknown error"));
        QCOMPARE(r.mpv.last(), "stop");
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, (QStringList{"closed: not open", "closed", "closed: not open"}));
    }

    // Any other error ends the command with mpv's words, as the terminal's do, the Open's pause
    // among them, but a refused unpause after a seek has landed: mpv is still paused on the frame,
    // the moment it opened, and the Open says it is paused.
    void mpvsErrorsEndTheirCommand() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.mpv.answer(r.player, {}, unavailable);
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
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt), How::kSeek));
        r.openWith("/m/a.mkv", false);
        r.mpv.answer(r.player);
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(r.mpv.last(), "set pause no");
        r.mpv.answer(r.player, {}, QStringLiteral("unknown error"));
        r.player.run(close());
        r.mpv.answer(r.player, QStringLiteral("/m/a.mkv"));
        r.mpv.answer(r.player, {}, QStringLiteral("unknown error"));
        QCOMPARE(r.reports,
                 (QStringList{"open failed: mpv: unknown error", "open failed: mpv: loading failed",
                              "play failed: mpv: unknown error", "play failed: mpv: unknown error",
                              "play failed: mpv: error running command",
                              "play failed: mpv: property unavailable",
                              "play failed: mpv: property unavailable", "opened paused",
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
        QCOMPARE(r.mpv.last(), "get path");
        r.mpv.answer(r.player, QStringLiteral("/m/a.mkv"));
        QCOMPARE(r.mpv.last(), "stop");
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, (QStringList{"played playing 6000", "closed"}));
    }

    // With the pane ready, a Space during an Open under way waits behind it, as every command
    // waits for the one before: the Open loads as review said, and the Space toggles the video
    // once it has landed.
    void aSpaceDuringAnOpenUnderWayRunsOnceItHasLanded() {
        Rig r;
        r.player.ready();
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        r.player.run(pause());
        QCOMPARE(r.openWith(), firstWithNoFile);
        QCOMPARE(r.mpv.last(), loadOf("/m/a.mkv", "0:00:00.000000"));
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, QStringList());
        r.player.restarted();
        QCOMPARE(r.reports, QStringList{"opened paused"});
        QCOMPARE(r.mpv.last(), "get time-pos");
        r.mpv.answer(r.player, 0.0);
        QCOMPARE(r.mpv.last(), "cycle pause");
        r.mpv.answer(r.player);
        QCOMPARE(r.mpv.last(), "get pause");
        r.mpv.answer(r.player, false);
        r.mpv.answer(r.player, 0.0);
        QCOMPARE(r.reports, (QStringList{"opened paused", "played playing 0"}));
    }

    // A request mpv does not answer in its time fails its command, as the terminal's do, and the
    // next runs: an answer that comes after is no one's. An answer ends its request's time, so a
    // command that has reported fails no later.
    void aRequestMpvDoesNotAnswerInTimeFailsItsCommand() {
        Rig r(std::chrono::seconds(10), std::chrono::milliseconds(300));
        r.loaded();
        QElapsedTimer waited;
        waited.start();
        r.player.run(seek(5));
        const std::uint64_t unanswered = r.mpv.requests.back().id;
        r.player.run(close());
        QTRY_COMPARE_WITH_TIMEOUT(r.reports, QStringList{"play failed: no answer"}, 2000);
        QVERIFY2(waited.elapsed() >= 300, qPrintable(QString::number(waited.elapsed())));
        QCOMPARE(r.mpv.last(), "get path");
        r.player.answered(unanswered, {}, 1.0);
        QCOMPARE(r.mpv.last(), "get path");
        r.mpv.answer(r.player, QStringLiteral("/m/a.mkv"));
        r.mpv.answer(r.player);
        QCOMPARE(r.reports, (QStringList{"play failed: no answer", "closed"}));
        QTest::qWait(500);
        QCOMPARE(r.reports.size(), 2);
    }

    // An Open that waits for a pane that never shows video fails in its time, rather than wait
    // for good; a newer one waits its own time, and the next mpv that is ready has none to run.
    void anOpenThatWaitsTooLongForThePaneFails() {
        Rig r(std::chrono::seconds(10), std::chrono::seconds(10), std::chrono::seconds(1));
        QElapsedTimer waited;
        waited.start();
        r.player.run(open(moment("/m/a.mkv", 0, std::nullopt)));
        QTest::qWait(300);
        // A runner so slow that the first Open's time ended in the wait fails here, not on the
        // port's rule below.
        QCOMPARE(r.reports, QStringList());
        r.player.run(open(moment("/m/b.mkv", 0, std::nullopt)));
        QTRY_COMPARE_WITH_TIMEOUT(r.reports, QStringList{"open failed: no video"}, 5000);
        QVERIFY2(waited.elapsed() >= 1300, qPrintable(QString::number(waited.elapsed())));
        QVERIFY(!r.player.opening());
        r.player.ready();
        QCOMPARE(r.mpv.requests.size(), 0u);
        QCOMPARE(r.reports.size(), 1);
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

    // Once review has let go: mpv's path read once, behind release's stop. With no file, which
    // mpv's word for a property with no value says, ended at once, with nothing more asked.
    void whenFileEndsAtOnceWithNoFile() {
        Rig r;
        r.player.ready();
        r.player.release();
        QStringList said;
        r.player.whenFileEnds(std::chrono::seconds(10), saidInto(said));
        QCOMPARE(r.mpv.last(), "get path");
        r.mpv.serve(r.player);
        QCOMPARE(said, QStringList{"ended"});
        QTest::qWait(100);
        QCOMPARE(said, QStringList{"ended"});
        QCOMPARE(r.mpv.requests.size(), 2u);
    }

    // A file mpv has loaded, whose path the read gives: ended at that file's end, which mpv says
    // after the read's answer. An end mpv says before the answer is an earlier file's, and once the
    // wait has said so, a later end says nothing more.
    void whenFileEndsAtTheEndOfTheFileMpvHas() {
        Rig r;
        r.mpv.holds = FakeMpv::Holds::Loaded;
        r.player.ready();
        r.player.release();
        QStringList said;
        r.player.whenFileEnds(std::chrono::seconds(10), saidInto(said));
        r.mpv.serve(r.player);
        QCOMPARE(said, QStringList{"ended"});

        Rig h;
        h.player.ready();
        h.player.release();
        QStringList heard;
        h.player.whenFileEnds(std::chrono::seconds(10), saidInto(heard));
        h.player.ended(End::Stopped);
        h.mpv.answer(h.player, QStringLiteral("/m/a.mkv"));
        QCOMPARE(heard, QStringList());
        h.player.ended(End::Stopped);
        QCOMPARE(heard, QStringList{"ended"});
        h.player.ended(End::Failed);
        QCOMPARE(heard, QStringList{"ended"});
        QCOMPARE(h.reports, QStringList());
    }

    // A file whose open waits on a share that does not answer, which mpv ends only once the share
    // answers, though its idle flag says idle from the stop on: not ended once the wait's time from
    // the call has passed, said once, with nothing more asked, and the file's end then heard by no
    // wait dropped. A wait after it reads the path again, so the file's end counts there, whether
    // mpv says it during that wait or said it between the two, when the path reads no file.
    void whenFileEndsGivesUpOnAStuckFileAndAWaitAfterReadsAgain() {
        Rig r;
        r.mpv.holds = FakeMpv::Holds::Stuck;
        r.player.ready();
        r.player.release();
        QStringList said;
        QElapsedTimer waited;
        waited.start();
        r.player.whenFileEnds(std::chrono::milliseconds(300), saidInto(said));
        QVERIFY(r.mpv.serveUntil(
            r.player, [&said] { return !said.isEmpty(); }, std::chrono::seconds(2)));
        QVERIFY2(waited.elapsed() >= 300, qPrintable(QString::number(waited.elapsed())));
        QCOMPARE(said, QStringList{"not ended"});
        QCOMPARE(r.mpv.requests.size(), 2u);
        r.player.whenFileEnds(std::chrono::seconds(10), saidInto(said));
        QCOMPARE(r.mpv.last(), "get path");
        r.mpv.serve(r.player);
        QCOMPARE(said, QStringList{"not ended"});
        r.mpv.shareAnswers(r.player);
        QCOMPARE(said, (QStringList{"not ended", "ended"}));
        QCOMPARE(r.reports, QStringList());

        Rig between;
        between.mpv.holds = FakeMpv::Holds::Stuck;
        between.player.ready();
        between.player.release();
        QStringList heard;
        between.player.whenFileEnds(std::chrono::milliseconds(300), saidInto(heard));
        QVERIFY(between.mpv.serveUntil(
            between.player, [&heard] { return !heard.isEmpty(); }, std::chrono::seconds(2)));
        between.mpv.shareAnswers(between.player);
        QCOMPARE(heard, QStringList{"not ended"});
        between.player.whenFileEnds(std::chrono::seconds(10), saidInto(heard));
        between.mpv.serve(between.player);
        QCOMPARE(heard, (QStringList{"not ended", "ended"}));
    }

    // A read mpv fails with any other error says nothing of a file, so the wait goes on: to an end
    // mpv says, or to its time.
    void whenFileEndsWaitsOnAReadThatSaysNothing() {
        const QString failed = QStringLiteral("error accessing property");
        Rig r;
        r.player.ready();
        r.player.release();
        QStringList said;
        r.player.whenFileEnds(std::chrono::seconds(10), saidInto(said));
        r.mpv.answer(r.player, {}, failed);
        QCOMPARE(said, QStringList());
        r.player.ended(End::Stopped);
        QCOMPARE(said, QStringList{"ended"});

        Rig quiet;
        quiet.player.ready();
        quiet.player.release();
        QStringList heard;
        QElapsedTimer waited;
        waited.start();
        quiet.player.whenFileEnds(std::chrono::milliseconds(300), saidInto(heard));
        quiet.mpv.answer(quiet.player, {}, failed);
        QTRY_COMPARE_WITH_TIMEOUT(heard, QStringList{"not ended"}, 2000);
        QVERIFY2(waited.elapsed() >= 300, qPrintable(QString::number(waited.elapsed())));
    }

    // A read mpv answers only once the wait's time has passed is no one's: the wait has said its
    // word.
    void whenFileEndsDropsAReadAnsweredAfterItsTime() {
        Rig r;
        r.player.ready();
        r.player.release();
        QStringList said;
        r.player.whenFileEnds(std::chrono::milliseconds(300), saidInto(said));
        QTRY_COMPARE_WITH_TIMEOUT(said, QStringList{"not ended"}, 2000);
        r.mpv.answer(r.player, {}, unavailable);
        QCOMPARE(said, QStringList{"not ended"});
        QCOMPARE(r.mpv.requests.size(), 2u);
    }

    // With no mpv, or one not ready, which has loaded nothing, there is nothing to wait for: ended
    // at once, with nothing asked.
    void whenFileEndsWithNoMpvEndsAtOnce() {
        PlayerPort alone(std::chrono::seconds(10));
        QStringList said;
        alone.whenFileEnds(std::chrono::seconds(10), saidInto(said));
        QCOMPARE(said, QStringList{"ended"});

        Rig r;
        r.player.release();
        r.player.whenFileEnds(std::chrono::seconds(10), saidInto(said));
        QCOMPARE(said, (QStringList{"ended", "ended"}));
        QCOMPARE(r.mpv.requests.size(), 0u);
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
    // runs on the new mpv once it can show video. The same mpv given again changes nothing. A
    // Space queued behind an Open under way was pressed against the old mpv's play state, so it is
    // answered, not kept for the Open; one pressed after the new mpv is attached is kept.
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
        QSignalSpy changed(&r.player, &PlayerPort::openingChanged);
        FakeMpv other;
        r.player.attach(&other);
        QCOMPARE(r.reports, QStringList{"play failed: no file"});
        // The Open waits for the new mpv now: under way throughout, with no change said between.
        QVERIFY(r.player.opening());
        QCOMPARE(changed.count(), 0);
        r.mpv.answer(r.player);
        r.player.restarted();
        QCOMPARE(other.requests.size(), 0u);
        r.player.ready();
        QCOMPARE(answerOpening(other, r.player), firstWithNoFile);
        QCOMPARE(other.last(), loadOf("/m/b.mkv", "0:00:00.000000"));
        QCOMPARE(r.mpv.requests.size(), sent);
        QCOMPARE(r.reports.size(), 1);

        Rig spaced;
        spaced.loaded();
        spaced.player.run(open(moment("/m/b.mkv", 0, std::nullopt)));
        spaced.player.run(pause());
        QCOMPARE(spaced.mpv.last(), "get path");
        FakeMpv another;
        spaced.player.attach(&another);
        QCOMPARE(spaced.reports, QStringList{"play failed: still opening"});
        QVERIFY(spaced.player.opening());
        spaced.player.ready();
        QCOMPARE(answerOpening(another, spaced.player), firstWithNoFile);
        QCOMPARE(another.last(), loadOf("/m/b.mkv", "0:00:00.000000", {}, true));
        another.answer(spaced.player);
        spaced.player.restarted();
        QCOMPARE(spaced.reports, (QStringList{"play failed: still opening", "opened paused"}));

        Rig after;
        after.loaded();
        after.player.run(open(moment("/m/b.mkv", 0, std::nullopt)));
        FakeMpv third;
        after.player.attach(&third);
        QCOMPARE(after.reports, QStringList());
        after.player.run(pause());
        QCOMPARE(after.reports, QStringList());
        after.player.ready();
        QCOMPARE(answerOpening(third, after.player), firstWithNoFile);
        QCOMPARE(third.last(), loadOf("/m/b.mkv", "0:00:00.000000", {}, false));
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
        QCOMPARE(r.reports, QStringList{"play failed: no file"});
    }

    // A pane lets go of its own mpv only: an mpv the player has since been given stays. Letting go
    // gives up the request under way, and its time with it.
    void detachLetsGoOnlyOfItsOwnMpv() {
        Rig r(std::chrono::seconds(10), std::chrono::milliseconds(300));
        r.loaded();
        FakeMpv other;
        r.player.detach(&other);
        r.player.run(seek(5));
        QCOMPARE(r.mpv.last(), "get time-pos");
        r.player.detach(&r.mpv);
        QCOMPARE(r.reports, QStringList{"play failed: no file"});
        QTest::qWait(500);
        QCOMPARE(r.reports, QStringList{"play failed: no file"});
    }
};

QTEST_GUILESS_MAIN(PlayerPortTest)
#include "player_port_test.moc"
