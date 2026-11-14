// A desktop session's variables, which the tests on an X display of their own leave out of every
// process they start, so that none of those reaches the desktop: its display and the cookie to
// it; its sound servers and bus; its accessibility bus, on which Qt registers every window once the
// name is set, with no session bus needed; its theme, which Qt also picks from KDE_FULL_SESSION or
// DESKTOP_SESSION; its seat and terminal; and its session manager, with which Qt on xcb registers
// a process, which the desktop could then start again at the user's next login. A name that starts
// with KDE_ or XDG_SESSION_ goes by its prefix, so that one a desktop adds later goes too.
// gui/qt/CMakeLists.txt's DESKTOP_SESSION_NAMES, the twin of this list, unsets the same exact
// names for every test CTest runs, and of the prefixes, which CMake cannot match, the names Qt
// reads: the two change together.
#pragma once

#include <string_view>

inline bool isDesktopSession(std::string_view name) {
    for (std::string_view prefix : {"KDE_", "XDG_SESSION_"}) {
        if (name.starts_with(prefix)) {
            return true;
        }
    }
    for (std::string_view each :
         {"DISPLAY", "WAYLAND_DISPLAY", "PULSE_SERVER", "PIPEWIRE_REMOTE",
          "DBUS_SESSION_BUS_ADDRESS", "AT_SPI_BUS_ADDRESS", "XDG_CURRENT_DESKTOP",
          "QT_QPA_PLATFORMTHEME", "SESSION_MANAGER", "DESKTOP_SESSION", "ICEAUTHORITY",
          "XAUTHORITY", "XDG_SEAT", "XDG_VTNR"}) {
        if (name == each) {
            return true;
        }
    }
    return false;
}
