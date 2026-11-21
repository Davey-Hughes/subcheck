// How the window ends once review has: as any Qt window does, its objects destroyed, once mpv has
// ended the file it had; else with none destroyed, as MpvQt's teardown of the video pane waits for
// mpv to end its file, which mpv cannot while the file's open waits on a share that does not
// answer.
#pragma once

#include "player_port.h"

#include <chrono>
#include <functional>

// Once review has let `player` go, which stopped mpv: waits up to `within` for mpv to end its file,
// and ended, `exitNormally(status)`, else `exitHard(status)`, which ends the process with nothing
// destroyed. Not ended within `settle`, as a file mpv has loaded is within moments of the stop and
// none is at once: `keepSettings`, which the window's teardown would otherwise do, then `hide`,
// whichever way the wait then ends. Hidden then, as a display server shows a window until the
// process that drew it lets go of it, and a thread stuck on a FUSE share keeps the process from
// going; the event loop runs while the wait goes on, which takes the hide to the display server.
void endWindow(PlayerPort &player, std::chrono::milliseconds settle,
               std::chrono::milliseconds within, int status, std::function<void()> keepSettings,
               std::function<void()> hide, std::function<void(int)> exitNormally,
               std::function<void(int)> exitHard);
