// A stand-in for mpv, for the player's tests on both platforms: it keeps each request the player
// sends, for the test to read and to answer as mpv would. What mpv says unasked (that it can show
// video, that playback started again, that the position moved, that a file ended) is the test's
// to say too, through the player, in the order the test needs.
#pragma once

#include "player_port.h"

#include <QString>
#include <QStringList>
#include <QVariant>

#include <cstdint>
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
};
