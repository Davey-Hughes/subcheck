// What the player reported, each as one line, for the player's tests on the fake mpv and on real
// mpv to compare: the same words in both.
#pragma once

#include "overloaded.h"
#include "subcheck_ffi.hpp"

#include <QString>

#include <variant>

// What went wrong, in short.
inline QString named(const subcheck_ffi::PlayerError &error) {
    using subcheck_ffi::PlayerError;
    return std::visit(
        Overloaded{
            [](const PlayerError::kNotOpen &) { return QStringLiteral("not open"); },
            [](const PlayerError::kNoFile &) { return QStringLiteral("no file"); },
            [](const PlayerError::kStillOpening &) { return QStringLiteral("still opening"); },
            [](const PlayerError::kCouldNotOpen &e) {
                return QStringLiteral("could not open ") + QString::fromStdString(e.file);
            },
            [](const PlayerError::kNoAnswer &) { return QStringLiteral("no answer"); },
            [](const PlayerError::kHungUp &) { return QStringLiteral("hung up"); },
            [](const PlayerError::kNoVideo &) { return QStringLiteral("no video"); },
            [](const PlayerError::kMpv &e) {
                return QStringLiteral("mpv: ") + QString::fromStdString(e.v1);
            },
            [](const PlayerError::kFailed &e) { return QString::fromStdString(e.v1); },
        },
        error.get_variant());
}

inline QString said(const subcheck_ffi::PlayerReport &report) {
    using subcheck_ffi::PlayerReport;
    return std::visit(
        Overloaded{
            [](const PlayerReport::kOpened &r) {
                return QStringLiteral("opened ") +
                       (r.paused ? QStringLiteral("paused") : QStringLiteral("playing"));
            },
            [](const PlayerReport::kOpenFailed &r) {
                return QStringLiteral("open failed: ") + named(r.error);
            },
            [](const PlayerReport::kPlayed &r) {
                return QStringLiteral("played %1 %2")
                    .arg(r.state.paused ? QStringLiteral("paused") : QStringLiteral("playing"))
                    .arg(r.state.at_ms);
            },
            [](const PlayerReport::kPlayFailed &r) {
                return QStringLiteral("play failed: ") + named(r.error);
            },
            [](const PlayerReport::kClosed &r) {
                return r.error ? QStringLiteral("closed: ") + named(*r.error)
                               : QStringLiteral("closed");
            },
            [](const PlayerReport::kFailed &r) {
                return QStringLiteral("failed: ") + named(r.error);
            },
        },
        report.get_variant());
}
