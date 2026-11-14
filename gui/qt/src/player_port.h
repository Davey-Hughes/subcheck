// The window's player: review's player commands, run on mpv one at a time, and what came of each
// reported back to review, as the terminal's player does over mpv's socket (cli/src/mpv.rs).
// The mpv is the video item's, or a test's fake, behind `Mpv`.
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

public:
    // `settle` is how long a load or a seek is given to land before the port goes on as if it
    // had: mpv says when one has, and a test gives a short one.
    explicit PlayerPort(std::chrono::milliseconds settle, QObject *parent = nullptr);

    // The mpv to run commands on, which the video item gives once QML has made it, and takes back
    // (null) as it goes. Another mpv, or none, is not ready until it says so.
    void attach(Mpv *mpv);
    // Review's command. It runs once those before it have reported, and an `Open` once mpv can
    // show what it opens.
    void run(const subcheck_ffi::PlayerCommand &command);
    // Review has ended: what waits is dropped, playback stops, and nothing more is reported.
    void release();

    // What mpv says: that it can show video; a request's answer, with mpv's error text or none;
    // that playback has started again after a load or a seek; that the position has changed; and
    // that a file has ended, on an error or not. mpv's events come in its order, but it holds a
    // change of a property until its queue of events is empty, so a position can come after
    // events that came about later than it did.
    void ready();
    void answered(std::uint64_t id, const QString &error, const QVariant &data);
    void restarted();
    void moved();
    void ended(bool failed);

signals:
    void reported(const subcheck_ffi::PlayerReport &report);

private:
    using Answer = std::function<void(const QVariant &data)>;
    using Missing = std::function<void()>;
    using Refused = std::function<void(const QString &error)>;
    using Stamp = std::optional<std::pair<std::uintmax_t, std::int64_t>>;
    // What lands a move: playback starting again, after a load or a seek; or the position
    // changing, after a frame step, which mpv starts no playback again for.
    enum class Landing { OnRestart, OnPosition };

    void drop();
    void next();
    void open(const subcheck_ffi::Moment &moment);
    static std::deque<QStringList> pauseFirst(const subcheck_ffi::Moment &moment);
    void load(const subcheck_ffi::Moment &moment, Stamp now);
    void seekTo(const subcheck_ffi::Moment &moment, Stamp now);
    void opened(bool paused);
    void playback(subcheck_ffi::Playback playback);
    void close();
    std::uint64_t request(Answer then, Refused otherwise);
    void ask(const QStringList &command, Answer then, Refused otherwise = nullptr);
    void read(const QString &property, Answer then, Missing missing = nullptr);
    void askAll(std::deque<QStringList> commands, std::function<void()> then,
                Refused otherwise = nullptr);
    void land(const QStringList &move, Landing on, bool loading, std::function<void()> then,
              Refused otherwise = nullptr);
    void landed();
    void where();
    void fail(const QString &why);
    void finish(const subcheck_ffi::PlayerReport &report);

    Mpv *mpv = nullptr;
    bool isReady = false;
    // An Open that came before mpv could show video: only the newest is kept.
    std::optional<subcheck_ffi::Moment> waiting;
    std::deque<subcheck_ffi::PlayerCommand> queue;
    // The command under way, until it reports, and the request of it that mpv has yet to answer.
    std::optional<subcheck_ffi::PlayerCommand> busy;
    std::uint64_t asked = 0;
    std::uint64_t nextId = 1;
    Answer then;
    // What the request mpv has yet to answer does with mpv's error instead of failing the
    // command: a read of a property with no value now, or any request of a seek, which then loads.
    Refused refused;
    // After a move: until mpv says what lands it (`landing`), or the settle timer ends.
    QTimer settle;
    bool settling = false;
    Landing landing = Landing::OnRestart;
    bool loading = false;
    QString loadingName;
    std::function<void()> onLanded;
    // The file this port last loaded, as it was on disk then, so that the same file unchanged
    // is sought rather than loaded again.
    struct Loaded {
        std::string file;
        std::uintmax_t size;
        std::int64_t mtime;
    };
    std::optional<Loaded> loaded;
};
