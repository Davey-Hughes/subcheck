// The window's player: review's player commands, run on mpv one at a time, and what came of each
// reported back to review, as the terminal's player does over mpv's socket (cli/src/mpv.rs); and
// whether an Open is under way, which the window shows over the video while it waits. The mpv is
// the video item's, or a test's fake, behind `Mpv`.
#pragma once

#include "subcheck_ffi.hpp"

#include <QObject>
#include <QString>
#include <QStringList>
#include <QTimer>
#include <QVariant>
#include <QtQml/qqmlregistration.h>

#include <chrono>
#include <cstdint>
#include <deque>
#include <functional>
#include <optional>
#include <string>
#include <utility>

// What the port asks of mpv. Each request is answered later, through PlayerPort::answered with
// the request's id, among mpv's events and in their order; mpv may run requests sent together in
// any order, so the port sends one only once the one before is answered.
class Mpv {
public:
    virtual ~Mpv() = default;
    // One of mpv's commands, its arguments as mpv's input commands take them.
    virtual void command(std::uint64_t id, const QStringList &args) = 0;
    // A property's value, as the answer's data.
    virtual void get(std::uint64_t id, const QString &name) = 0;
};

class PlayerPort : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("main.cpp makes the one player")
    // Whether an Open is under way: queued, waiting for the pane, run, or late, a load that
    // reported opened as its settle time ended (`late`). From review's Open until the video shows
    // what it opened, cannot, or is closed.
    Q_PROPERTY(bool opening READ opening NOTIFY openingChanged)

public:
    // How mpv says a file ended: stopped, as a file replaced by a load or by a stop is; at the
    // file's own end; or on an error.
    enum class End { Stopped, Finished, Failed };

    // `settle` is how long a load or a seek is given to land before the port goes on as if it
    // had: mpv says when one has, and a test gives a short one. `answer` is how long mpv is given
    // to answer a request, as the terminal's socket gives it, and `ready` how long an Open waits
    // for the pane to show video, before each fails the command rather than wait for good.
    explicit PlayerPort(std::chrono::milliseconds settle,
                        std::chrono::milliseconds answer = std::chrono::seconds(2),
                        std::chrono::milliseconds ready = std::chrono::seconds(10),
                        QObject *parent = nullptr);

    // The mpv to run commands on, which the video item gives once QML has made it, and takes back
    // (null) as it goes. Another mpv, or none, is not ready until it says so.
    void attach(Mpv *mpv);
    // As attach(nullptr), but only while `mpv` is the one attached: an item that goes, or is given
    // another player, lets go of its own mpv and of no other.
    void detach(Mpv *mpv);
    // Review's command. It runs once those before it have reported, and an Open once mpv can
    // show what it opens.
    void run(const subcheck_ffi::PlayerCommand &command);
    // Review has ended: what waits is dropped, playback stops, and nothing more is reported.
    void release();
    // After release: `then(true)` once mpv has ended the file it had, or `then(false)` once
    // `within` has passed without, as when its open waits on a share that does not answer. mpv's
    // path, read once, says whether it has one: with none, or with no mpv, or one not ready, which
    // has loaded nothing, `then(true)` at once; with one, at that file's end. Not mpv's idle flag,
    // which says idle from the stop on, while mpv still waits on the file's open.
    void whenFileEnds(std::chrono::milliseconds within, std::function<void(bool ended)> then);

    // What mpv says: that it can show video; a request's answer, with mpv's error text or none;
    // that playback has started again after a load or a seek; that the position has changed; and
    // that a file has ended, and how. mpv's events come in its order, but it holds a change of a
    // property until its queue of events is empty, so a position can come after events that came
    // about later than it did.
    void ready();
    void answered(std::uint64_t id, const QString &error, const QVariant &data);
    void restarted();
    void moved();
    void ended(End end);

    bool opening() const;

signals:
    void reported(const subcheck_ffi::PlayerReport &report);
    void openingChanged();
    // mpv shows `file`, an Open's, as far as the port can tell: the Open's load has started to
    // play, or reached the file's end as it settled, or its seek in the file mpv had open has
    // landed. Not at the load's answer, nor at the end of its settle time with mpv having said
    // nothing, as on a slow share: until the new file plays, mpv still shows the file before it.
    // Said for that late load once it plays, and not before for a seek in its file.
    void shows(const std::string &file);

private:
    using Answer = std::function<void(const QVariant &data)>;
    using Missing = std::function<void()>;
    using Refused = std::function<void(const QString &error)>;
    // What lands a move: playback starting again, after a load or a seek; or the position
    // changing, after a frame step, which mpv starts no playback again for.
    enum class Landing { OnRestart, OnPosition };

    // run's work, without saying whether an Open is under way: attach runs again the commands it
    // took out of the queue, and says that once all have run, as midway an Open yet to run again
    // would count as none.
    void take(const subcheck_ffi::PlayerCommand &command);
    // Stops waiting for the late load and says at once whether an Open is still under way: for a
    // caller that has nothing to report of the late load itself. ended reports the late load's
    // failure between the two, so does both itself; load, attach and release clear `late` inside a
    // larger change that says it once at its end.
    void stopWaitingForLate();
    // Says whether an Open is under way, should that have changed since it last said.
    void noteOpening();
    void drop();
    void next();
    void open(const subcheck_ffi::PlayerCommand::kOpen &command);
    void load(const subcheck_ffi::PlayerCommand::kOpen &command, bool resume);
    void seekTo(const subcheck_ffi::PlayerCommand::kOpen &command, bool resume);
    void opened(bool resume);
    void playback(subcheck_ffi::Playback playback);
    void close();
    std::uint64_t request(Answer then, Refused otherwise);
    void ask(const QStringList &command, Answer then, Refused otherwise = nullptr);
    void read(const QString &property, Answer then, Missing missing = nullptr,
              Refused otherwise = nullptr);
    void askAll(std::deque<QStringList> commands, std::function<void()> then,
                Refused otherwise = nullptr);
    void land(const QStringList &move, Landing on, bool loading, std::function<void()> then,
              Refused otherwise = nullptr);
    void landed();
    void where();
    void noAnswer();
    void notReady();
    void saidEnded(bool ended);
    void fail(const subcheck_ffi::PlayerError &why);
    void finish(const subcheck_ffi::PlayerReport &report);

    Mpv *mpv = nullptr;
    bool isReady = false;
    // An Open that came before mpv could show video, until it can or `notShown` ends: only the
    // newest is kept.
    std::optional<subcheck_ffi::PlayerCommand> waiting;
    QTimer notShown;
    // A Space for the Open that waits, as there is nothing open to toggle: ready() hands it on
    // inside that Open, which then lands the other way from its play. A newer Open in its place
    // plays if it says play or if the one it replaces would have (its play turned by this Space),
    // and this clears, as the newer Open's play now holds it. A Space is for whichever video
    // arrives, as review keeps a held Space across a newer press, and an Open's play is such a
    // Space, one review held; and an Open that says play is one review's line says "will play"
    // for, which must not land paused. A Close, the end of `notShown` and the release forget it
    // with the Open it was for.
    bool flipPlay = false;
    std::deque<subcheck_ffi::PlayerCommand> queue;
    // The command under way, until it reports, and the request of it that mpv has yet to answer.
    std::optional<subcheck_ffi::PlayerCommand> busy;
    std::uint64_t asked = 0;
    std::uint64_t nextId = 1;
    // From each request until mpv answers it.
    QTimer unanswered;
    Answer then;
    // What the request mpv has yet to answer does with mpv's error instead of failing the
    // command, where the command takes a refusal as an answer: a read of the path, of whether mpv
    // plays, or of a property with no value now; the unpause after a seek; or any request of a
    // seek, which then loads.
    Refused refused;
    // After a move: until mpv says what lands it (`landing`), or the settle timer ends.
    QTimer settle;
    bool settling = false;
    Landing landing = Landing::OnRestart;
    bool loading = false;
    std::string loadingFile;
    std::function<void()> onLanded;
    // A load that reported opened when its settle time ended, mpv having said nothing: until it
    // plays, reaches its file's end, the port loads or stops, mpv ends it on an error, which is
    // reported then, another mpv takes its place, or review ends.
    std::optional<std::string> late;
    // What openingChanged last said.
    bool saidOpening = false;
    // whenFileEnds's: its read of the path, until mpv answers it; whether mpv then had a file, or
    // may have, whose end the wait is for; the wait's whole time; and what it calls.
    std::uint64_t pathAsked = 0;
    bool fileHeld = false;
    QTimer endLimit;
    std::function<void(bool ended)> onEnd;
};
