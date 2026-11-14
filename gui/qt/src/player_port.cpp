#include "player_port.h"

#include "overloaded.h"

#include <QFileInfo>

#include <mpv/client.h>

#include <algorithm>
#include <cmath>
#include <filesystem>
#include <string>
#include <system_error>
#include <utility>
#include <variant>

using namespace subcheck_ffi;

namespace {

// mpv's own text for a property with no value now: the position, or the path, with no file open.
const QString unavailable = QStringLiteral("property unavailable");

// A Playback's answer with nothing to play, and while a file opens: the terminal's words for the
// same states of its mpv (cli/src/mpv.rs).
const char *const nothingOpen = "mpv has no file open (o opens one)";
const char *const stillOpening = "mpv is still opening the file";

// `ms` as the terminal gives mpv a time, H:MM:SS.mmm; never before 0:00, as mpv counts a negative
// time back from the end.
QString mpvTime(std::int64_t ms) {
    ms = std::max<std::int64_t>(ms, 0);
    return QStringLiteral("%1:%2:%3.%4")
        .arg(ms / 3'600'000)
        .arg(ms / 60'000 % 60, 2, 10, QLatin1Char('0'))
        .arg(ms / 1000 % 60, 2, 10, QLatin1Char('0'))
        .arg(ms % 1000, 3, 10, QLatin1Char('0'));
}

// The file's size and modification time, to the nanosecond as the terminal's are, or none when
// they cannot be read.
std::optional<std::pair<std::uintmax_t, std::int64_t>> stamp(const std::string &file) {
    std::error_code error;
    const std::filesystem::path path(file);
    const std::uintmax_t size = std::filesystem::file_size(path, error);
    if (error) {
        return std::nullopt;
    }
    const auto mtime = std::filesystem::last_write_time(path, error);
    if (error) {
        return std::nullopt;
    }
    return std::pair{size, static_cast<std::int64_t>(mtime.time_since_epoch().count())};
}

} // namespace

PlayerPort::PlayerPort(std::chrono::milliseconds settle, QObject *parent) : QObject(parent) {
    this->settle.setSingleShot(true);
    this->settle.setInterval(settle);
    // A coarse timer may end up to 5% early, and the wait's end reports the move as landed: a
    // move mpv says nothing of is given its whole settle time.
    this->settle.setTimerType(Qt::PreciseTimer);
    connect(&this->settle, &QTimer::timeout, this, &PlayerPort::landed);
}

// The mpv gone will answer nothing more and land nothing, and a new one has shown nothing and is
// not ready until it says so. So what review sent that has not reported (the command under way,
// then those queued behind it) is given up there and run again as if it came now: an Open waits
// for an mpv to be ready, as before the first was, and a Playback or a Close is answered at once,
// as nothing can be open. The same mpv given again is still ready, if it was, and still answers.
void PlayerPort::attach(Mpv *mpv) {
    if (mpv == this->mpv) {
        return;
    }
    std::deque<PlayerCommand> again = std::exchange(queue, {});
    if (busy) {
        again.push_front(*busy);
    }
    drop();
    this->mpv = mpv;
    isReady = false;
    for (const PlayerCommand &command : again) {
        run(command);
    }
}

void PlayerPort::run(const PlayerCommand &command) {
    if (!isReady) {
        // Nothing has opened yet, so nothing can play or need closing; an Open waits for mpv
        // to show what it opens, and a newer one replaces it, as review's status names only it.
        std::visit(Overloaded{
                       [this](const PlayerCommand::kOpen &open) { waiting = open.v1; },
                       [this](const PlayerCommand::kPlayback &) {
                           emit reported(PlayerReport(
                               PlayerReport::kPlayFailed{waiting ? stillOpening : nothingOpen}));
                       },
                       [this](const PlayerCommand::kClose &) {
                           waiting.reset();
                           emit reported(PlayerReport(PlayerReport::kClosed{std::nullopt}));
                       },
                   },
                   command.get_variant());
        return;
    }
    queue.push_back(command);
    next();
}

void PlayerPort::release() {
    waiting.reset();
    queue.clear();
    drop();
    // Its answer goes nowhere: nothing more is reported.
    if (mpv && isReady) {
        mpv->command(nextId++, {QStringLiteral("stop")});
    }
}

void PlayerPort::ready() {
    isReady = true;
    if (waiting) {
        queue.push_back(PlayerCommand(PlayerCommand::kOpen{*waiting}));
        waiting.reset();
    }
    next();
}

void PlayerPort::answered(std::uint64_t id, const QString &error, const QVariant &data) {
    // An answer to a request of a command that has since ended, or of the release's stop.
    if (id == 0 || id != asked) {
        return;
    }
    asked = 0;
    Answer answer = std::exchange(then, nullptr);
    Refused onError = std::exchange(refused, nullptr);
    if (error.isEmpty()) {
        answer(data);
    } else if (onError) {
        onError(error);
    } else {
        fail(QStringLiteral("mpv: ") + error);
    }
}

void PlayerPort::restarted() {
    if (landing == Landing::OnRestart) {
        landed();
    }
}

void PlayerPort::moved() {
    if (landing == Landing::OnPosition) {
        landed();
    }
}

void PlayerPort::ended(bool failed) {
    // Only the load under way can fail the Open: a file replaced by it ends too, stopped.
    if (settling && loading && failed) {
        settle.stop();
        settling = false;
        fail(QStringLiteral("mpv could not open ") + loadingName);
    }
}

// The command under way gives up its request, whose answer is then no one's, and its wait for
// the move to land, whose end would otherwise ask mpv for where it landed.
void PlayerPort::drop() {
    busy.reset();
    asked = 0;
    then = nullptr;
    refused = nullptr;
    onLanded = nullptr;
    settle.stop();
    settling = false;
}

void PlayerPort::next() {
    if (busy || queue.empty() || !mpv) {
        return;
    }
    const PlayerCommand command = std::move(queue.front());
    queue.pop_front();
    busy = command;
    std::visit(
        Overloaded{
            [this](const PlayerCommand::kOpen &open) { this->open(open.v1); },
            [this](const PlayerCommand::kPlayback &playback) { this->playback(playback.v1); },
            [this](const PlayerCommand::kClose &) { close(); },
        },
        command.get_variant());
}

// As the terminal's: the file mpv has open is sought, not loaded again (slow for a big file over
// NFS), if this port loaded it and it is unchanged on disk since, as a remux in place keeps the
// path, and after a fix of one letter even the size. Whatever error mpv refuses the seek with, the
// file is loaded instead, as the terminal's is: a file mpv still loads, as after an Open that went
// on past its settle time, is the case it is for.
void PlayerPort::open(const Moment &moment) {
    read(
        QStringLiteral("path"),
        [this, moment](const QVariant &path) {
            const auto now = stamp(moment.file);
            const bool same = path.toString() == QString::fromStdString(moment.file) && loaded &&
                              now && loaded->file == moment.file && loaded->size == now->first &&
                              loaded->mtime == now->second;
            if (same) {
                seekTo(moment, now);
            } else {
                load(moment, now);
            }
        },
        [this, moment] { load(moment, stamp(moment.file)); });
}

// Paused before mpv gets there, for a frame to stay on, or a playing mpv goes on past it; a moment
// to play is unpaused only once mpv is there (opened), so that what it showed before does not play
// on meanwhile. As the terminal's: not as a per-file option of the load, which mpv puts back when
// its file ends, and that can be after the next was loaded.
std::deque<QStringList> PlayerPort::pauseFirst(const Moment &moment) {
    if (!moment.paused) {
        return {};
    }
    return {{QStringLiteral("set"), QStringLiteral("pause"), QStringLiteral("yes")}};
}

void PlayerPort::opened(bool paused) {
    if (paused) {
        finish(PlayerReport(PlayerReport::kOpened{}));
        return;
    }
    ask({QStringLiteral("set"), QStringLiteral("pause"), QStringLiteral("no")},
        [this](const QVariant &) { finish(PlayerReport(PlayerReport::kOpened{})); });
}

void PlayerPort::load(const Moment &moment, Stamp now) {
    loaded.reset();
    if (now) {
        loaded = Loaded{moment.file, now->first, now->second};
    }
    const QString file = QString::fromStdString(moment.file);
    QString options = QStringLiteral("start=") + mpvTime(moment.start_ms);
    if (moment.sid) {
        options += QStringLiteral(",sid=") + QString::number(*moment.sid);
    }
    options += QStringLiteral(",sub-visibility=yes");
    loadingName = QFileInfo(file).fileName();
    // `replace` then -1, the index mpv 0.38 put before the options. mpv 0.38 came with client API
    // 2.3, so an older mpv's headers stop the build here, rather than every load at run time.
    static_assert(MPV_CLIENT_API_VERSION >= MPV_MAKE_VERSION(2, 3),
                  "loadfile's index argument needs mpv 0.38 or later");
    askAll(pauseFirst(moment), [this, file, options, paused = moment.paused] {
        land({QStringLiteral("loadfile"), file, QStringLiteral("replace"), QStringLiteral("-1"),
              options},
             Landing::OnRestart, true, [this, paused] { opened(paused); });
    });
}

// Any of the seek's requests refused, the setting or the seek itself, loads the file instead: the
// whole load, its pause first, so that a moment to stay on is paused whichever was refused.
void PlayerPort::seekTo(const Moment &moment, Stamp now) {
    const Refused reload = [this, moment, now](const QString &) { load(moment, now); };
    std::deque<QStringList> settings = pauseFirst(moment);
    if (moment.sid) {
        settings.push_back(
            {QStringLiteral("set"), QStringLiteral("sid"), QString::number(*moment.sid)});
    }
    settings.push_back(
        {QStringLiteral("set"), QStringLiteral("sub-visibility"), QStringLiteral("yes")});
    askAll(
        settings,
        [this, moment, reload] {
            land(
                {QStringLiteral("seek"), mpvTime(moment.start_ms),
                 QStringLiteral("absolute+exact")},
                Landing::OnRestart, false, [this, paused = moment.paused] { opened(paused); },
                reload);
        },
        reload);
}

void PlayerPort::playback(Playback playback) {
    // As the terminal: no position means no file open, or one still opening.
    read(
        QStringLiteral("time-pos"),
        [this, playback](const QVariant &) {
            std::visit(Overloaded{
                           // Not a seek: where it is can be read at once.
                           [this](const Playback::kPause &) {
                               ask({QStringLiteral("cycle"), QStringLiteral("pause")},
                                   [this](const QVariant &) { where(); });
                           },
                           // Exact: a plain relative seek lands on a keyframe, up to seconds away.
                           [this](const Playback::kSeek &seek) {
                               land({QStringLiteral("seek"), QString::number(seek.v1),
                                     QStringLiteral("relative+exact")},
                                    Landing::OnRestart, false, [this] { where(); });
                           },
                           [this](const Playback::kNextFrame &) {
                               land({QStringLiteral("frame-step")}, Landing::OnPosition, false,
                                    [this] { where(); });
                           },
                           [this](const Playback::kPreviousFrame &) {
                               // A step back is a seek, and starts playback again.
                               land({QStringLiteral("frame-back-step")}, Landing::OnRestart, false,
                                    [this] { where(); });
                           },
                       },
                       playback.get_variant());
        },
        [this] {
            read(
                QStringLiteral("path"), [this](const QVariant &) { fail(stillOpening); },
                [this] { fail(nothingOpen); });
        });
}

void PlayerPort::close() {
    ask({QStringLiteral("stop")},
        [this](const QVariant &) { finish(PlayerReport(PlayerReport::kClosed{std::nullopt})); });
}

// The next request's id, now the one the port waits on: `then` takes its answer, and `otherwise`,
// if given, mpv's error in place of failing the command.
std::uint64_t PlayerPort::request(Answer then, Refused otherwise) {
    asked = nextId++;
    this->then = std::move(then);
    refused = std::move(otherwise);
    return asked;
}

void PlayerPort::ask(const QStringList &command, Answer then, Refused otherwise) {
    mpv->command(request(std::move(then), std::move(otherwise)), command);
}

// `missing`, if given, takes the read when mpv has no value for the property; any other error,
// or that one with no `missing`, fails the command.
void PlayerPort::read(const QString &property, Answer then, Missing missing) {
    Refused otherwise;
    if (missing) {
        otherwise = [this, missing = std::move(missing)](const QString &error) {
            if (error == unavailable) {
                missing();
            } else {
                fail(QStringLiteral("mpv: ") + error);
            }
        };
    }
    mpv->get(request(std::move(then), std::move(otherwise)), property);
}

// One after another, each once the one before is answered; `otherwise`, if given, takes the error
// of any of them.
void PlayerPort::askAll(std::deque<QStringList> commands, std::function<void()> then,
                        Refused otherwise) {
    if (commands.empty()) {
        then();
        return;
    }
    const QStringList first = commands.front();
    commands.pop_front();
    ask(
        first,
        [this, rest = std::move(commands), then = std::move(then), otherwise](const QVariant &) {
            askAll(rest, then, otherwise);
        },
        otherwise);
}

// Once mpv has answered the move, a load or a seek lands when playback starts again: mpv keeps its
// events in order, so that comes after the answer, and one before it was an earlier request's (one
// after it can be an earlier move's only if that one outlasted its settle time). A frame step,
// which starts no playback again, lands when the position changes; mpv holds a change of a property
// until its queue of events is empty, so while it plays, the change that ends the wait can be older
// than the step, and the position read then the frame before it. If mpv says neither, the move
// lands when the settle timer ends. The position is never read before one of those: straight after
// a seek, mpv can still give the old one.
void PlayerPort::land(const QStringList &move, Landing on, bool loading, std::function<void()> then,
                      Refused otherwise) {
    ask(
        move,
        [this, on, loading, then = std::move(then)](const QVariant &) {
            landing = on;
            this->loading = loading;
            onLanded = then;
            settling = true;
            settle.start();
        },
        std::move(otherwise));
}

void PlayerPort::landed() {
    if (!settling) {
        return;
    }
    settle.stop();
    settling = false;
    std::exchange(onLanded, nullptr)();
}

// Where playback is now, for review's status line.
void PlayerPort::where() {
    read(QStringLiteral("pause"), [this](const QVariant &paused) {
        read(QStringLiteral("time-pos"), [this, isPaused = paused.toBool()](const QVariant &at) {
            const auto ms = static_cast<std::int64_t>(std::llround(at.toDouble() * 1000));
            finish(PlayerReport(PlayerReport::kPlayed{PlayState{isPaused, ms}}));
        });
    });
}

void PlayerPort::fail(const QString &why) {
    const std::string text = why.toStdString();
    finish(std::visit(
        Overloaded{
            [&](const PlayerCommand::kOpen &) { return PlayerReport(PlayerReport::kOpened{text}); },
            [&](const PlayerCommand::kPlayback &) {
                return PlayerReport(PlayerReport::kPlayFailed{text});
            },
            [&](const PlayerCommand::kClose &) {
                return PlayerReport(PlayerReport::kClosed{text});
            },
        },
        busy->get_variant()));
}

void PlayerPort::finish(const PlayerReport &report) {
    busy.reset();
    emit reported(report);
    next();
}
