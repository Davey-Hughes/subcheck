// The main of a Qt test that makes a GUI application: it refuses to start, before it makes one,
// anywhere but Qt's offscreen platform with no display to open (offscreen still opens DISPLAY's
// for OpenGL). CTest gives every test that; a run by hand from a desktop's shell does not, and
// would otherwise put the test's windows on the desktop. Of a desktop's variables it reads only
// the two that name a display: CTest unsets the names Qt reads and leaves the rest of a session's
// (XDG_SESSION_ID, KDE_SESSION_UID), so refusing on those would refuse every run from a desktop.
#pragma once

#include <QGuiApplication>
#include <QTest>

#include <cstdio>

inline bool offscreenOnly() {
    if (qgetenv("QT_QPA_PLATFORM").startsWith("offscreen") &&
        !qEnvironmentVariableIsSet("DISPLAY") && !qEnvironmentVariableIsSet("WAYLAND_DISPLAY")) {
        return true;
    }
    std::fputs("run through ctest, which runs this test offscreen with no display\n", stderr);
    return false;
}

#define SUBCHECK_OFFSCREEN_TEST_MAIN(TestObject)                                                   \
    int main(int argc, char **argv) {                                                              \
        if (!offscreenOnly()) {                                                                    \
            return 2;                                                                              \
        }                                                                                          \
        QGuiApplication app(argc, argv);                                                           \
        app.setAttribute(Qt::AA_Use96Dpi, true);                                                   \
        TestObject test;                                                                           \
        return QTest::qExec(&test, argc, argv);                                                    \
    }
