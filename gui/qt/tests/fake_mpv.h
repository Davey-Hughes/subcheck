// A stand-in for mpv, for the player's tests on both platforms: it keeps each request the player
// sends, for the test to read and to answer as mpv would. What mpv says unasked (that it can show
// video, that playback started again, that the position moved, that a file ended) is the test's
// to say too, through the player, in the order the test needs; but for the window's end, `serve`
// answers as mpv does once stopped, the file's end among it.
#pragma once

#include "player_port.h"

#include <QDeadlineTimer>
#include <QString>
#include <QStringList>
#include <QTest>
#include <QVariant>

#include <chrono>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <vector>

struct FakeMpv : Mpv {
    struct Request {
        std::uint64_t id;
        QString text;
    };
    std::vector<Request> requests;

    void command(std::uint64_t id, const QStringList &args) override {
        requests.push_back({id, args.join(QLatin1Char(' '))});
    }

    void get(std::uint64_t id, const QString &name) override {
        requests.push_back({id, QStringLiteral("get ") + name});
    }

    // The newest request, as "get path" or "seek 5 relative+exact": the player sends one at a
    // time, so it is the one the player waits on. Empty before the first.
    QString last() const { return requests.empty() ? QString() : requests.back().text; }

    // Answers the newest request, with its data or with mpv's error text.
    void answer(PlayerPort &player, const QVariant &data = {}, const QString &error = {}) {
        player.answered(requests.back().id, error, data);
    }

    // What mpv holds when review stops it, for the window's end, which `serve` answers as mpv
    // does: no file; a file loaded, which mpv ends straight after the requests sent with the stop;
    // or a file whose open waits on a share that does not answer, which mpv ends only once the
    // share answers (`shareAnswers`).
    enum class Holds { Nothing, Loaded, Stuck };
    Holds holds = Holds::Nothing;
    // The held file, as mpv's path reads.
    QString held = QStringLiteral("/m/a.mkv");

    // Answers every request sent since it last ran, in order, as mpv does once review's stop has
    // come: the path reads the held file's until mpv has ended it, and mpv's word for a property
    // with no value once it has, or with none held; any other request is answered with no data.
    // Then a loaded file ends, which mpv says after the answers to the requests sent with the stop.
    void serve(PlayerPort &player) {
        const QString unavailable = QStringLiteral("property unavailable");
        bool ends = false;
        // By index, and the request copied: an answer can make the player send the next.
        while (served < requests.size()) {
            const Request request = requests[served++];
            if (request.text == QStringLiteral("stop")) {
                stopped = true;
                ends = ends || holds == Holds::Loaded;
                player.answered(request.id, {}, {});
            } else if (request.text == QStringLiteral("get path") && holds == Holds::Nothing) {
                player.answered(request.id, unavailable, {});
            } else if (request.text == QStringLiteral("get path")) {
                player.answered(request.id, {}, held);
            } else {
                player.answered(request.id, {}, {});
            }
        }
        if (ends) {
            endHeld(player);
        }
    }

    // As serve, again each time the player sends more, until `done` holds or `limit` has
    // passed. Whether `done` came to hold.
    bool serveUntil(PlayerPort &player, const std::function<bool()> &done,
                    std::chrono::milliseconds limit) {
        return QTest::qWaitFor(
            [&] {
                serve(player);
                return done();
            },
            QDeadlineTimer(limit));
    }

    // The share answers: a stuck file that review has stopped ends now.
    void shareAnswers(PlayerPort &player) {
        if (holds == Holds::Stuck && stopped) {
            endHeld(player);
        }
    }

private:
    void endHeld(PlayerPort &player) {
        holds = Holds::Nothing;
        player.ended(PlayerPort::End::Stopped);
    }

    // Requests before this index are answered by serve.
    std::size_t served = 0;
    bool stopped = false;
};
