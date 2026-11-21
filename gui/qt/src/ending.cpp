#include "ending.h"

#include <QDeadlineTimer>

#include <utility>

void endWindow(PlayerPort &player, std::chrono::milliseconds settle,
               std::chrono::milliseconds within, int status, std::function<void()> keepSettings,
               std::function<void()> hide, std::function<void(int)> exitNormally,
               std::function<void(int)> exitHard) {
    const QDeadlineTimer limit(within, Qt::PreciseTimer);
    player.whenFileEnds(settle, [&player, limit, status, keepSettings = std::move(keepSettings),
                                 hide = std::move(hide), exitNormally = std::move(exitNormally),
                                 exitHard = std::move(exitHard)](bool ended) {
        if (ended) {
            exitNormally(status);
            return;
        }
        keepSettings();
        hide();
        // The rest of the time, rounded up, so that the process never ends hard before it; and a
        // wait of its own, which reads the path again, so that an end mpv said between the two,
        // which neither heard, still counts: the path then says there is no file.
        const auto left =
            std::chrono::ceil<std::chrono::milliseconds>(limit.remainingTimeAsDuration());
        player.whenFileEnds(left, [status, exitNormally, exitHard](bool ended) {
            if (ended) {
                exitNormally(status);
            } else {
                exitHard(status);
            }
        });
    });
}
