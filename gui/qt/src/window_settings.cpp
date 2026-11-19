#include "window_settings.h"

#include <QGuiApplication>
#include <QList>
#include <QPoint>
#include <QPointer>
#include <QQuickWindow>
#include <QRect>
#include <QRegion>
#include <QScreen>

#include <algorithm>

namespace {

// The keys, and the defaults: a window with room for three columns on a laptop's screen.
const QString sizeKey = QStringLiteral("window/size");
const QString placeKey = QStringLiteral("window/place");
const QString maximizedKey = QStringLiteral("window/maximized");
const QString sidebarKey = QStringLiteral("columns/sidebar");
const QString detailsKey = QStringLiteral("columns/details");
const QSize defaultSize(1280, 800);
const int defaultSidebar = 220;
const int defaultDetails = 480;

// The screen a frame kept at `place` comes back to, for a window of `size`: the one holding that
// corner, else the one whose free area would show the most of the window, as a window flush with a
// screen's edge can keep a corner just off it. None if no free area would show any of the window.
const QScreen *screenFor(QPoint place, QSize size) {
    if (const QScreen *holding = QGuiApplication::screenAt(place)) {
        return holding;
    }
    const QScreen *most = nullptr;
    qint64 mostShown = 0;
    const QList<QScreen *> screens = QGuiApplication::screens();
    for (const QScreen *each : screens) {
        const QRect part = each->availableGeometry().intersected(QRect(place, size));
        const qint64 shown = qint64(part.width()) * part.height();
        if (shown > mostShown) {
            most = each;
            mostShown = shown;
        }
    }
    return most;
}

} // namespace

WindowSettings::WindowSettings(std::unique_ptr<QSettings> store, const QString &platform,
                               std::chrono::milliseconds settle, QObject *parent)
    : QObject(parent), store(std::move(store)), placed(places(platform)) {
    settled.setSingleShot(true);
    settled.setInterval(settle);
}

WindowSettings::~WindowSettings() { keep(); }

bool WindowSettings::places(const QString &platform) {
    return !platform.startsWith(QStringLiteral("wayland"));
}

QPoint WindowSettings::placeBack(QPoint place, QSize size, const QRect &room, const QRegion &free) {
    if (QRegion(QRect(place, size)).subtracted(free).isEmpty()) {
        return place;
    }
    // Not std::clamp, whose bounds are the wrong way round for a window larger than `room`.
    return {std::max(room.x(), std::min(place.x(), room.x() + room.width() - size.width())),
            std::max(room.y(), std::min(place.y(), room.y() + room.height() - size.height()))};
}

void WindowSettings::show(QQuickWindow *window) {
    std::optional<QPoint> place;
    if (placed && store->contains(placeKey)) {
        place = store->value(placeKey).toPoint();
    }
    // The screen the window comes back to bounds its size, as until it is placed the window is on
    // the primary one, which can be smaller or larger than that screen.
    QSize size = store->value(sizeKey, defaultSize).toSize();
    const QScreen *const there = place ? screenFor(*place, size) : nullptr;
    if (const QScreen *screen = there ? there : window->screen()) {
        size = size.boundedTo(screen->availableSize());
    }
    size = size.expandedTo(window->minimumSize());
    window->resize(size);
    if (there) {
        QRegion free;
        const QList<QScreen *> screens = there->virtualSiblings();
        for (const QScreen *each : screens) {
            free += each->availableGeometry();
        }
        window->setFramePosition(placeBack(*place, size, there->availableGeometry(), free));
    }
    // Shown at its own size first, which is then the size it is restored to.
    window->show();
    if (store->value(maximizedKey, false).toBool()) {
        window->showMaximized();
    }
    const auto read = [this, window] {
        const Qt::WindowStates states = window->windowStates();
        said = Said{window->size(), window->framePosition(), states.testFlag(Qt::WindowMaximized),
                    (states & (Qt::WindowMaximized | Qt::WindowFullScreen)) != 0};
        settled.start();
    };
    connect(window, &QWindow::widthChanged, this, read);
    connect(window, &QWindow::heightChanged, this, read);
    connect(window, &QWindow::xChanged, this, read);
    connect(window, &QWindow::yChanged, this, read);
    connect(window, &QWindow::windowStateChanged, this, read);
    // A window manager can report the frame's margins after the window's last move, and a change of
    // margins emits nothing, so the frame's place is read once more as the window settles. Only
    // while it shows: a closed window has no margins, and would report its contents' corner.
    connect(&settled, &QTimer::timeout, this, [this, shown = QPointer<QQuickWindow>(window)] {
        if (placed && said && shown && shown->isVisible()) {
            said->place = shown->framePosition();
        }
        keep();
    });
}

void WindowSettings::keep() {
    if (!said) {
        return;
    }
    store->setValue(maximizedKey, said->maximized);
    if (!said->whole) {
        store->setValue(sizeKey, said->size);
        if (placed) {
            store->setValue(placeKey, said->place);
        }
    }
    said.reset();
}

int WindowSettings::sidebarWidth() const {
    return store->value(sidebarKey, defaultSidebar).toInt();
}

int WindowSettings::detailsWidth() const {
    return store->value(detailsKey, defaultDetails).toInt();
}

void WindowSettings::keepColumns(int sidebar, int details) {
    store->setValue(sidebarKey, sidebar);
    store->setValue(detailsKey, details);
    emit columnsChanged();
}
