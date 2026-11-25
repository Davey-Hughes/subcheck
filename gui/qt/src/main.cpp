// subcheck-qt [run | paths…]: a run's findings in a window, to decide them. The run or paths are
// `subcheck review`'s. At the end, as the terminal: the outro on stderr, then the counts on stdout
// or the error on stderr; but the counts name a view of paths as the window's title does, by the
// first path's last part and `+N` for the others, where the terminal names every path. The exit
// status is 1 after an error, or when review has not ended within the wait below after a quit,
// and 0 otherwise.
#include "ending.h"
#include "key_names.h"
#include "player_port.h"
#include "session_bridge.h"
#include "window_settings.h"

#include <QCoreApplication>
#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQuickWindow>
#include <QWindow>
#include <QtQml/QQmlExtensionPlugin>

#include <chrono>
#include <cstdlib>
#include <exception>
#include <iostream>
#include <memory>
#include <string>
#include <vector>

using namespace std::chrono_literals;

// The QML module is a static library's, so its plugin is linked in and imported by name.
Q_IMPORT_QML_PLUGIN(SubcheckPlugin)

namespace {

// As the terminal's main, once the window and its session are gone: while a signal is ending the
// process, its cleanup ends it, rather than this status before a job's groups are ended. The first
// call into the library when the window failed before opening a session, so a contract mismatch
// throws here, which the bridge would have caught at open. The process's status.
int finishing(int status) {
    try {
        subcheck_ffi::finish();
    } catch (const std::exception &e) {
        std::cerr << "subcheck-qt: " << e.what() << '\n';
        return 1;
    }
    return status;
}

// Ends the process at once, as main would end it but with no object destroyed: the video pane's
// teardown, MpvQt's, waits for mpv's load stuck on a share that does not answer, and so does its
// renderer's, on the render thread, which the window's teardown waits on. The kernel ends mpv's
// threads with the process. What main does once the window has gone is done here, as main will
// not run: the output written out, then finishing.
[[noreturn]] void exitHard(int status) {
    std::cout.flush();
    std::cerr.flush();
    std::_Exit(finishing(status));
}

// The window, from its start to its end: its status.
int run(int argc, char **argv) {
    // MpvQt draws mpv's frames with OpenGL, which Qt Quick must be asked for before it makes the
    // first window: Linux's default, but not macOS's, which is Metal.
    QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL);
    QGuiApplication app(argc, argv);
    // The window's names for keys, before its QML reads them (key_names.h).
    KeyNames keyNames;
    // The names the platform's own place for the window's settings goes by, which macOS uses when
    // XDG_CONFIG_HOME is not set (WindowSettings::appStore), and Qt's caches.
    QCoreApplication::setOrganizationName(QStringLiteral("subcheck"));
    QCoreApplication::setApplicationName(QStringLiteral("subcheck-qt"));
    // Through QString, as the session takes UTF-8 and toStdString always gives it.
    std::vector<std::string> selector;
    const QStringList arguments = QCoreApplication::arguments();
    for (qsizetype i = 1; i < arguments.size(); ++i) {
        selector.push_back(arguments[i].toStdString());
    }
    // A load or a seek has a second to land before the player goes on as if it had; mpv has two to
    // answer a request, as the terminal's socket gives it; and an Open waits ten for the pane to
    // show video, long past a first frame on a display that can draw, so that Show in Video on one
    // that cannot says so. Made before the bridge, which stops it as it goes: the player must
    // outlive the bridge, or the bridge's last release runs on a player already destroyed.
    PlayerPort player(1s, 2s, 10s);
    // Before the engine, so that it outlives the window that shows it. The wait after a quit is
    // longer than the store waits for another process's write lock (core's BUSY_TIMEOUT), so that
    // review still ends on its own after one write that waits out a lock held elsewhere. It is
    // not long enough for several writes queued behind a lock held longer, each of which can wait
    // that long, or for an open that waits on the lock more than once: then the window gives up,
    // closing review, which can drop a decision still queued, and exits with 1. A session stalled
    // for good, on a hung mount say, still ends the window.
    SessionBridge bridge(std::move(selector), player, std::cout, std::cerr, 10s);
    WindowSettings settings(WindowSettings::appStore(), QGuiApplication::platformName());
    QQmlApplicationEngine engine;
    engine.setInitialProperties({{QStringLiteral("bridge"), QVariant::fromValue(&bridge)},
                                 {QStringLiteral("settings"), QVariant::fromValue(&settings)}});
    engine.loadFromModule("Subcheck", "Main");
    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().value(0));
    if (!window) {
        return 1;
    }
    settings.show(window);
    // The settings kept now, as no teardown will keep them, and no window heard any more, the
    // popped-out video's among them: hidden next, a window can say it is no longer maximized, or
    // where it is as it goes, which the settings would keep once it settled.
    const auto keepSettings = [&settings] {
        settings.keep();
        const QWindowList windows = QGuiApplication::allWindows();
        for (QWindow *each : windows) {
            QObject::disconnect(each, nullptr, &settings, nullptr);
        }
    };
    const auto hideWindows = [] {
        const QWindowList windows = QGuiApplication::topLevelWindows();
        for (QWindow *each : windows) {
            each->hide();
        }
    };
    // Once review has ended, which stopped mpv: mpv has 2 s to end the file it had, and the window
    // then ends as any does. The windows go after a moment, should mpv not have ended it by then,
    // as it does with one loaded or none: a moment long past what that takes, and short enough that
    // the window still goes at once to the eye. Past the 2 s the process ends without the window's
    // teardown, from here, before the event loop returns and anything is destroyed.
    QObject::connect(&bridge, &SessionBridge::finished, &app,
                     [&player, keepSettings, hideWindows](int status) {
                         endWindow(
                             player, 200ms, 2s, status, keepSettings, hideWindows,
                             [](int code) { QCoreApplication::exit(code); }, exitHard);
                     });
    bridge.start();
    return QGuiApplication::exec();
}

} // namespace

int main(int argc, char **argv) { return finishing(run(argc, argv)); }
