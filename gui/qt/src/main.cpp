// subcheck-qt [run | paths…]: a run's findings in a window, to decide them. The run or paths are
// `subcheck review`'s. At the end, as the terminal: the outro on stderr, then the counts on stdout
// or the error on stderr; but the counts name a view of paths as the window's title does, by the
// first path's last part and `+N` for the others, where the terminal names every path. The exit
// status is 1 after an error, or when review has not ended within the wait below after a quit,
// and 0 otherwise.
#include "player_port.h"
#include "session_bridge.h"

#include <QCoreApplication>
#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQuickWindow>
#include <QtQml/QQmlExtensionPlugin>

#include <chrono>
#include <iostream>
#include <string>
#include <vector>

using namespace std::chrono_literals;

// The QML module is a static library's, so its plugin is linked in and imported by name.
Q_IMPORT_QML_PLUGIN(SubcheckPlugin)

int main(int argc, char **argv) {
    // MpvQt draws mpv's frames with OpenGL, which Qt Quick must be asked for before it makes the
    // first window: Linux's default, but not macOS's, which is Metal.
    QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL);
    QGuiApplication app(argc, argv);
    // Through QString, as the session takes UTF-8 and toStdString always gives it.
    std::vector<std::string> selector;
    const QStringList arguments = QCoreApplication::arguments();
    for (qsizetype i = 1; i < arguments.size(); ++i) {
        selector.push_back(arguments[i].toStdString());
    }
    // A load or a seek has a second to land before the player goes on as if it had. Made before
    // the bridge, which stops it as it goes: the player must outlive the bridge, or the bridge's
    // last release runs on a player already destroyed.
    PlayerPort player(1s);
    // Before the engine, so that it outlives the window that shows it. The wait after a quit is
    // longer than the store waits for another process's write lock (core's BUSY_TIMEOUT), so that
    // review still ends on its own after one write that waits out a lock held elsewhere. It is
    // not long enough for several writes queued behind a lock held longer, each of which can wait
    // that long, or for an open that waits on the lock more than once: then the window gives up,
    // closing review, which can drop a decision still queued, and exits with 1. A session stalled
    // for good, on a hung mount say, still ends the window.
    SessionBridge bridge(std::move(selector), player, std::cout, std::cerr, 10s);
    QObject::connect(&bridge, &SessionBridge::finished, &app,
                     [](int status) { QCoreApplication::exit(status); });
    QQmlApplicationEngine engine;
    engine.setInitialProperties({{QStringLiteral("bridge"), QVariant::fromValue(&bridge)}});
    engine.loadFromModule("Subcheck", "Main");
    if (engine.rootObjects().isEmpty()) {
        return 1;
    }
    bridge.start();
    return QGuiApplication::exec();
}
