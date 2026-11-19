// What the window remembers between runs: its size, the place of its frame where the platform lets
// a window place itself, and the widths of its sidebar and details panel. The list takes what is
// left. The frame's place is what is kept and what is restored, so that a window comes back to one
// place however many times it starts.
#pragma once

#include <QObject>
#include <QPoint>
#include <QRect>
#include <QRegion>
#include <QSettings>
#include <QSize>
#include <QTimer>
#include <QtQml/qqmlregistration.h>

#include <chrono>
#include <memory>
#include <optional>

class QQuickWindow;

class WindowSettings : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("main.cpp makes the one settings")
    // The columns' widths, as last left: the window starts with them.
    Q_PROPERTY(int sidebarWidth READ sidebarWidth NOTIFY columnsChanged)
    Q_PROPERTY(int detailsWidth READ detailsWidth NOTIFY columnsChanged)

public:
    // `store` is where they stick: the app's own settings, or a test's file. `platform` is the
    // one Qt runs on, as QGuiApplication names it: where it lets no window place itself, only the
    // size sticks. `settle` is how long the window must say nothing more before what it said is
    // kept, long enough for a platform's reports of one maximize to have all landed.
    WindowSettings(std::unique_ptr<QSettings> store, const QString &platform,
                   std::chrono::milliseconds settle = std::chrono::milliseconds(250),
                   QObject *parent = nullptr);
    // Keeps what the window last said, should it go before that settled.
    ~WindowSettings() override;

    // Whether a window can place itself on `platform`: not on Wayland, in any of its forms
    // (wayland, wayland-egl), where the compositor places each window.
    static bool places(const QString &platform);

    // Where a frame kept at `place` comes back to, for a window of `size`: `place` itself while the
    // window lies inside `free`, the free areas of the desktop's screens taken together; otherwise
    // moved no further than it takes to lie inside `room`, the free area of the screen it comes
    // back to. A window larger than `room` keeps the frame's top left corner, and so its title bar,
    // in view. `size` leaves the frame out, as its margins are not known before the window shows:
    // a window moved back against `room`'s right or bottom edge can reach past it by its frame.
    static QPoint placeBack(QPoint place, QSize size, const QRect &room, const QRegion &free);

    // Shows `window` at its size from last time, else at its default, no larger than the free area
    // of the screen it opens on unless its least size is; its frame where it was last time
    // (`placeBack`), where the platform lets it and a screen now holds that place or would show
    // part of the window there; maximized if it was. From then on it keeps them as they settle: the
    // size and frame's place a window has when it is neither maximized nor full screen, so that one
    // maximized last time comes back to its own size when it is restored.
    void show(QQuickWindow *window);

    // Keeps what the window last said, if it has said anything since this last ran.
    void keep();

    int sidebarWidth() const;
    int detailsWidth() const;
    // The columns' widths as the person left them.
    Q_INVOKABLE void keepColumns(int sidebar, int details);

signals:
    void columnsChanged();

private:
    // The window as it last said it was, read whole at each thing it says: a platform can report a
    // maximize's state and its size in either order, and only their last reading is the window's.
    struct Said {
        QSize size;
        // Its frame's top left corner, read again as the window settles.
        QPoint place;
        bool maximized;
        // Maximized or full screen: its size and place are then the screen's, not its own.
        bool whole;
    };

    std::unique_ptr<QSettings> store;
    bool placed;
    std::optional<Said> said;
    QTimer settled;
};
