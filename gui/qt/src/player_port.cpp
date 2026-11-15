#include "player_port.h"

#include "overloaded.h"

#include <mpv/client.h>

#include <cmath>
#include <string>
#include <utility>
#include <variant>

using namespace subcheck_ffi;

namespace {

// mpv's own text for a property with no value now: the position, or the path, with no file open.
const QString unavailable = QStringLiteral("property unavailable");

PlayerError mpvSaid(const QString &error) {
    return PlayerError(PlayerError::kMpv{error.toStdString()});
}

PlayerError couldNotOpen(const std::string &file) {
    return PlayerError(PlayerError::kCouldNotOpen{file});
}

// `us` as the terminal gives mpv a time, H:MM:SS.uuuuuu, to the µs review gives it in. Review gives
// none before 0:00, which mpv would count back from the end.
QString mpvTime(std::int64_t us) {
    return QStringLiteral("%1:%2:%3.%4")
        .arg(us / 3'600'000'000)
        .arg(us / 60'000'000 % 60, 2, 10, QLatin1Char('0'))
        .arg(us / 1'000'000 % 60, 2, 10, QLatin1Char('0'))
        .arg(us % 1'000'000, 6, 10, QLatin1Char('0'));
}

// mpv's own pause, set before every move to a moment, or a playing mpv goes on past the frame; and
// unset once a seek has landed, if mpv played before the Open, not sooner, or it plays on from
// where it was meanwhile. A load sets its own, as the file's option (`load`).
QStringList setPause(bool paused) {
    return {QStringLiteral("set"), QStringLiteral("pause"),
            paused ? QStringLiteral("yes") : QStringLiteral("no")};
}

} // namespace

PlayerPort::PlayerPort(std::chrono::milliseconds settle, std::chrono::milliseconds answer,
                       std::chrono::milliseconds ready, QObject *parent)
    : QObject(parent) {
    // Each precise: a coarse timer may end up to 5% early. A move mpv says nothing of is given its
    // whole settle time, and its end reports the move as landed.
    for (auto [timer, after] : {std::pair{&this->settle, settle}, std::pair{&unanswered, answer},
                                std::pair{&notShown, ready}}) {
        timer->setSingleShot(true);
        timer->setInterval(after);
        timer->setTimerType(Qt::PreciseTimer);
    }
    // A load mpv has said nothing of when its time ends is opened as far as the port can tell, and
    // can still fail.
    connect(&this->settle, &QTimer::timeout, this, [this] {
        if (loading) {
            late = loadingFile;
        }
        landed();
    });
    connect(&unanswered, &QTimer::timeout, this, &PlayerPort::noAnswer);
    connect(&notShown, &QTimer::timeout, this, &PlayerPort::notReady);
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
    late.reset();
    this->mpv = mpv;
    isReady = false;
    for (const PlayerCommand &command : again) {
        run(command);
    }
}

void PlayerPort::detach(Mpv *mpv) {
    if (mpv == this->mpv) {
        attach(nullptr);
    }
}

void PlayerPort::run(const PlayerCommand &command) {
    if (!isReady) {
        // Nothing has opened yet, so nothing can play or need closing; an Open waits for mpv
        // to show what it opens, and a newer one replaces it, as review's status names only it,
        // each for the time the port gives it.
        std::visit(Overloaded{
                       [this, &command](const PlayerCommand::kOpen &) {
                           waiting = command;
                           notShown.start();
                       },
                       [this](const PlayerCommand::kPlayback &) {
                           emit reported(PlayerReport(PlayerReport::kPlayFailed{
                               waiting ? PlayerError(PlayerError::kStillOpening{})
                                       : PlayerError(PlayerError::kNoFile{})}));
                       },
                       [this](const PlayerCommand::kClose &) {
                           const bool closed = waiting.has_value();
                           waiting.reset();
                           notShown.stop();
                           emit reported(PlayerReport(PlayerReport::kClosed{
                               closed ? std::nullopt
                                      : std::optional(PlayerError(PlayerError::kNotOpen{}))}));
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
    notShown.stop();
    queue.clear();
    drop();
    late.reset();
    // Its answer goes nowhere: nothing more is reported.
    if (mpv && isReady) {
        mpv->command(nextId++, {QStringLiteral("stop")});
    }
}

void PlayerPort::ready() {
    isReady = true;
    notShown.stop();
    if (waiting) {
        queue.push_back(*std::exchange(waiting, std::nullopt));
    }
    next();
}

void PlayerPort::answered(std::uint64_t id, const QString &error, const QVariant &data) {
    // An answer to a request of a command that has since ended, or of the release's stop.
    if (id == 0 || id != asked) {
        return;
    }
    asked = 0;
    unanswered.stop();
    Answer answer = std::exchange(then, nullptr);
    Refused onError = std::exchange(refused, nullptr);
    if (error.isEmpty()) {
        answer(data);
    } else if (onError) {
        onError(error);
    } else {
        fail(mpvSaid(error));
    }
}

void PlayerPort::restarted() {
    // A file plays: a late load's has landed, or another move did after it.
    late.reset();
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
    // Only the load under way can fail the Open: a file replaced by it ends too, stopped. A late
    // load that ends on an error is reported as it is, its Open long since reported. An end with
    // no error leaves the late load waiting: it is most often the file the late load replaced,
    // whose end comes after its reads, which can outlast the settle time on a stalled mount.
    if (!failed) {
        return;
    }
    if (settling && loading) {
        settle.stop();
        settling = false;
        fail(couldNotOpen(loadingFile));
    } else if (late) {
        emit reported(
            PlayerReport(PlayerReport::kFailed{couldNotOpen(*std::exchange(late, std::nullopt))}));
    }
}

// The command under way gives up its request, whose answer is then no one's, and its wait for
// the move to land, whose end would otherwise ask mpv for where it landed.
void PlayerPort::drop() {
    busy.reset();
    asked = 0;
    unanswered.stop();
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
            [this](const PlayerCommand::kOpen &open) { this->open(open); },
            [this](const PlayerCommand::kPlayback &playback) { this->playback(playback.v1); },
            [this](const PlayerCommand::kClose &) { close(); },
        },
        command.get_variant());
}

// As the terminal's, in the steps core's PlayerCommand::Open names. Whether mpv plays is read
// first, as the pause that starts every move makes every mpv paused, and with a file open only:
// with none, nothing plays, whatever mpv's pause says. A read of the path that mpv refuses,
// whatever its words, counts as no file open, so the file is loaded. Whether mpv plays decides
// only whether it plays again once there, so a read that mpv fails, or that gives no flag, counts
// as paused, which leaves mpv on the frame. Then the pause, whose refusal fails the Open; then the
// seek, when review says seek and mpv still has the file open (a file opened in the pane since by
// other means is loaded again), else the load.
void PlayerPort::open(const PlayerCommand::kOpen &command) {
    const auto go = [this, command](bool seek, bool resume) {
        askAll({setPause(true)}, [this, command, seek, resume] {
            if (seek) {
                seekTo(command, resume);
            } else {
                load(command, resume);
            }
        });
    };
    read(
        QStringLiteral("path"),
        [this, command, go](const QVariant &path) {
            const bool seek = command.how == How::kSeek &&
                              path.toString() == QString::fromStdString(command.moment.file);
            read(
                QStringLiteral("pause"),
                [go, seek](const QVariant &paused) {
                    go(seek, paused.metaType() == QMetaType::fromType<bool>() && !paused.toBool());
                },
                nullptr, [go, seek](const QString &) { go(seek, false); });
        },
        nullptr, [go](const QString &) { go(false, false); });
}

// Once a seek has landed: if mpv played before the Open, it plays again, only now, so that it plays
// on from the frame, not from where it was, and the Open reports once mpv has answered that.
// Refused, mpv stays paused on the frame, which is still the moment opened, and the report says so.
void PlayerPort::opened(bool resume) {
    const auto done = [this](bool paused) { finish(PlayerReport(PlayerReport::kOpened{paused})); };
    if (!resume) {
        done(true);
        return;
    }
    ask(
        setPause(false), [done](const QVariant &) { done(false); },
        [done](const QString &) { done(true); });
}

// As the terminal's (cli/src/mpv.rs, `loadfile_command`): the moment's start and track, subtitles
// on, review's options for every load, then the pause as mpv is to end up. Every option is the
// file's alone, set as it starts, after the file before has put back its own, a pause among them,
// and a user's `reset-on-next-file` what it names, so that neither undoes this pause. The Open
// reports once the load lands.
void PlayerPort::load(const PlayerCommand::kOpen &command, bool resume) {
    const Moment &moment = command.moment;
    const QString file = QString::fromStdString(moment.file);
    QString options = QStringLiteral("start=") + mpvTime(moment.start_us);
    if (moment.sid) {
        options += QStringLiteral(",sid=") + QString::number(*moment.sid);
    }
    options += QStringLiteral(",sub-visibility=yes");
    for (const FileOption &option : command.options) {
        options += QLatin1Char(',') + QString::fromStdString(option.name) + QLatin1Char('=') +
                   QString::fromStdString(option.value);
    }
    options += QStringLiteral(",pause=") + (resume ? QStringLiteral("no") : QStringLiteral("yes"));
    loadingFile = moment.file;
    // Ends a late load's wait: this load replaces it, and from its answer on, an end on an error is
    // this load's.
    late.reset();
    // `replace` then -1, the index mpv 0.38 put before the options. mpv 0.38 came with client API
    // 2.3, so an older mpv's headers stop the build here, rather than every load at run time.
    static_assert(MPV_CLIENT_API_VERSION >= MPV_MAKE_VERSION(2, 3),
                  "loadfile's index argument needs mpv 0.38 or later");
    land({QStringLiteral("loadfile"), file, QStringLiteral("replace"), QStringLiteral("-1"),
          options},
         Landing::OnRestart, true,
         [this, resume] { finish(PlayerReport(PlayerReport::kOpened{!resume})); });
}

// The seek's steps in core's order, the subtitle track and subtitles on, then the seek itself: any
// of them refused loads the file instead, mpv paused already, as a file mpv still loads refuses the
// seek, the case it is for.
void PlayerPort::seekTo(const PlayerCommand::kOpen &command, bool resume) {
    const Moment &moment = command.moment;
    const Refused reload = [this, command, resume](const QString &) { load(command, resume); };
    std::deque<QStringList> settings;
    if (moment.sid) {
        settings.push_back(
            {QStringLiteral("set"), QStringLiteral("sid"), QString::number(*moment.sid)});
    }
    settings.push_back(
        {QStringLiteral("set"), QStringLiteral("sub-visibility"), QStringLiteral("yes")});
    askAll(
        settings,
        [this, moment, resume, reload] {
            land(
                {QStringLiteral("seek"), mpvTime(moment.start_us),
                 QStringLiteral("absolute+exact")},
                Landing::OnRestart, false, [this, resume] { opened(resume); }, reload);
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
                QStringLiteral("path"),
                [this](const QVariant &) { fail(PlayerError(PlayerError::kStillOpening{})); },
                [this] { fail(PlayerError(PlayerError::kNoFile{})); });
        });
}

// As the terminal's words: with nothing open, there was nothing to close, and a read of the path
// that mpv refuses, whatever its words, is nothing open, as it is for an Open. `stop` all the
// same, as mpv names no path for a load it has taken and not yet started, which would then play.
void PlayerPort::close() {
    const auto stop = [this](std::optional<PlayerError> said) {
        late.reset();
        ask({QStringLiteral("stop")},
            [this, said](const QVariant &) { finish(PlayerReport(PlayerReport::kClosed{said})); });
    };
    read(
        QStringLiteral("path"), [stop](const QVariant &) { stop(std::nullopt); }, nullptr,
        [stop](const QString &) { stop(PlayerError(PlayerError::kNotOpen{})); });
}

// The next request's id, now the one the port waits on: `then` takes its answer, and `otherwise`,
// if given, mpv's error in place of failing the command.
std::uint64_t PlayerPort::request(Answer then, Refused otherwise) {
    asked = nextId++;
    this->then = std::move(then);
    refused = std::move(otherwise);
    unanswered.start();
    return asked;
}

void PlayerPort::ask(const QStringList &command, Answer then, Refused otherwise) {
    mpv->command(request(std::move(then), std::move(otherwise)), command);
}

// `missing`, if given, takes the read when mpv has no value for the property, and `otherwise`,
// if given, any other error, or with no `missing` that one too; else an error fails the command.
void PlayerPort::read(const QString &property, Answer then, Missing missing, Refused otherwise) {
    mpv->get(request(std::move(then),
                     [this, missing = std::move(missing),
                      otherwise = std::move(otherwise)](const QString &error) {
                         if (missing && error == unavailable) {
                             missing();
                         } else if (otherwise) {
                             otherwise(error);
                         } else {
                             fail(mpvSaid(error));
                         }
                     }),
             property);
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

// mpv has not answered the request in its time: the request is no one's now, and the command fails.
// An answer that comes after is dropped, as an earlier command's is.
void PlayerPort::noAnswer() {
    asked = 0;
    then = nullptr;
    refused = nullptr;
    fail(PlayerError(PlayerError::kNoAnswer{}));
}

// The pane has not shown video in the time an Open waits for it: the Open fails, and the next one
// waits again.
void PlayerPort::notReady() {
    if (waiting) {
        waiting.reset();
        emit reported(
            PlayerReport(PlayerReport::kOpenFailed{PlayerError(PlayerError::kNoVideo{})}));
    }
}

void PlayerPort::fail(const PlayerError &why) {
    finish(std::visit(
        Overloaded{
            [&](const PlayerCommand::kOpen &) {
                return PlayerReport(PlayerReport::kOpenFailed{why});
            },
            [&](const PlayerCommand::kPlayback &) {
                return PlayerReport(PlayerReport::kPlayFailed{why});
            },
            [&](const PlayerCommand::kClose &) { return PlayerReport(PlayerReport::kClosed{why}); },
        },
        busy->get_variant()));
}

void PlayerPort::finish(const PlayerReport &report) {
    busy.reset();
    emit reported(report);
    next();
}
