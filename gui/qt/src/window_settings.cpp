#include "window_settings.h"

#include "words.h"

#include <QCoreApplication>
#include <QDir>
#include <QFontInfo>
#include <QFontMetricsF>
#include <QGuiApplication>
#include <QList>
#include <QPoint>
#include <QPointer>
#include <QQuickWindow>
#include <QRect>
#include <QRegion>
#include <QScreen>
#include <QVariantMap>

#include <algorithm>
#include <cmath>

namespace {

// The keys, and the defaults: a window with room for three columns on a laptop's screen.
const QString sizeKey = QStringLiteral("window/size");
const QString placeKey = QStringLiteral("window/place");
const QString maximizedKey = QStringLiteral("window/maximized");
const QString sidebarKey = QStringLiteral("columns/sidebar");
const QString detailsKey = QStringLiteral("columns/details");
const QString paletteKey = QStringLiteral("look/colours");
const QString windowTextKey = QStringLiteral("text/window");
const QString listTextKey = QStringLiteral("text/list");
const QString detailsTextKey = QStringLiteral("text/details");
const QString noFindingsLastKey = QStringLiteral("view/noFindingsLast");
const QString videoSizeKey = QStringLiteral("video/size");
const QString videoPlaceKey = QStringLiteral("video/place");
const QString rowStyleKey = QStringLiteral("rows/style");
const QString defaultRowStyle = QStringLiteral("twoLines");
const QSize defaultSize(1280, 800);
// The popped-out video's, until it has one of its own: the whole window, its control bar in it.
const QSize defaultVideoSize(960, 540);
const int defaultSidebar = 220;
const int defaultDetails = 480;

// The sizes the Settings window offers, in points. A size kept outside them was not set there, and
// reads as the platform's, so that a file edited by hand cannot make the window's text unreadable.
constexpr qreal leastSize = 6;
constexpr qreal mostSize = 24;

qreal halfPoint(qreal points) { return std::round(points * 2) / 2; }

// The row style `name` names, or Two Lines for a name no style has, so that a name edited into the
// file by hand still draws the list in a style it has.
QString rowStyleNamed(const QString &name) {
    for (const QLatin1String style :
         {QLatin1String("compact"), QLatin1String("twoLines"), QLatin1String("rich")}) {
        if (name == style) {
            return name;
        }
    }
    return defaultRowStyle;
}

// The platform's size of text, as the application's font resolves it: in points, though the
// platform may have set it in pixels.
qreal platformSize() { return halfPoint(QFontInfo(QGuiApplication::font()).pointSizeF()); }

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

WindowSettings::Watch::Watch(QString size, QString place, QString maximized)
    : sizeKey(std::move(size)), placeKey(std::move(place)), maximizedKey(std::move(maximized)) {}

WindowSettings::WindowSettings(std::unique_ptr<QSettings> store, const QString &platform,
                               std::chrono::milliseconds settle, QObject *parent)
    : QObject(parent), store(std::move(store)), placed(places(platform)),
      mainWatch(sizeKey, placeKey, maximizedKey), videoWatch(videoSizeKey, videoPlaceKey, {}) {
    for (Watch *each : {&mainWatch, &videoWatch}) {
        each->settled.setSingleShot(true);
        each->settled.setInterval(settle);
        // A window manager can report the frame's margins after the window's last move, and a
        // change of margins emits nothing, so the frame's place is read once more as the window
        // settles. Only while it shows, as a window of its own: a closed window has no margins, and
        // would report its contents' corner. Only this window's word is kept: another's can be
        // halfway through a maximize.
        connect(&each->settled, &QTimer::timeout, this, [this, each] {
            QQuickWindow *const shown = each->window;
            if (placed && each->said && shown && shown->isVisible() && !shown->parent()) {
                each->said->place = shown->framePosition();
            }
            keepSaid(*each);
            this->store->sync();
        });
    }
    useColours(coloursNamed(this->store->value(paletteKey).toString()));
    // An unset size is the platform's as it is now, so a change of the desktop's font moves it;
    // with no one to say so the window would keep the old size until a restart. The application's
    // fontChanged signal is deprecated, in favour of the event this watches for.
    if (qobject_cast<QGuiApplication *>(QCoreApplication::instance())) {
        QCoreApplication::instance()->installEventFilter(this);
    }
}

bool WindowSettings::eventFilter(QObject *watched, QEvent *event) {
    // The filter sees the events of every object, and Qt sends this one to each window as well as
    // to the application: only the application's own stands for the change, or it would say so
    // once for each window.
    if (event->type() == QEvent::ApplicationFontChange && watched == QCoreApplication::instance()) {
        emit textSizesChanged();
    }
    return QObject::eventFilter(watched, event);
}

WindowSettings::~WindowSettings() { keep(); }

bool WindowSettings::places(const QString &platform) {
    return !platform.startsWith(QStringLiteral("wayland"));
}

std::optional<QString> WindowSettings::fileFor(bool mac, const QString &configHome,
                                               const QString &home) {
    const QString file = QStringLiteral("subcheck/subcheck-qt.conf");
    if (QDir::isAbsolutePath(configHome)) {
        return QDir(configHome).filePath(file);
    }
    if (mac) {
        return std::nullopt;
    }
    return QDir(home).filePath(QStringLiteral(".config/") + file);
}

std::unique_ptr<QSettings> WindowSettings::appStore() {
#ifdef Q_OS_MACOS
    constexpr bool mac = true;
#else
    constexpr bool mac = false;
#endif
    const std::optional<QString> file =
        fileFor(mac, qEnvironmentVariable("XDG_CONFIG_HOME"), QDir::homePath());
    return file ? std::make_unique<QSettings>(*file, QSettings::IniFormat)
                : std::make_unique<QSettings>();
}

QPoint WindowSettings::placeBack(QPoint place, QSize size, const QRect &room, const QRegion &free) {
    if (QRegion(QRect(place, size)).subtracted(free).isEmpty()) {
        return place;
    }
    // Not std::clamp, whose bounds are the wrong way round for a window larger than `room`.
    return {std::max(room.x(), std::min(place.x(), room.x() + room.width() - size.width())),
            std::max(room.y(), std::min(place.y(), room.y() + room.height() - size.height()))};
}

std::pair<QSize, bool> WindowSettings::restore(const Watch &watch, QQuickWindow *window, QSize size,
                                               const QScreen *screen) {
    std::optional<QPoint> place;
    if (placed && store->contains(watch.placeKey)) {
        place = store->value(watch.placeKey).toPoint();
    }
    // The screen the window comes back to bounds its size, as until it is placed the window is on
    // the primary one, which can be smaller or larger than that screen.
    size = store->value(watch.sizeKey, size).toSize();
    const QScreen *const there = place ? screenFor(*place, size) : nullptr;
    if (const QScreen *bound = there ? there : screen) {
        size = size.boundedTo(bound->availableSize());
    }
    size = size.expandedTo(window->minimumSize());
    window->resize(size);
    if (!there) {
        return {size, false};
    }
    QRegion free;
    const QList<QScreen *> screens = there->virtualSiblings();
    for (const QScreen *each : screens) {
        free += each->availableGeometry();
    }
    window->setFramePosition(placeBack(*place, size, there->availableGeometry(), free));
    return {size, true};
}

void WindowSettings::watch(Watch &watch, QQuickWindow *window) {
    watch.window = window;
    const auto read = [&watch, window] {
        // Held by another window, it has the size and place its holder gives it, not its own.
        if (window->parent()) {
            return;
        }
        const Qt::WindowStates states = window->windowStates();
        watch.said =
            Said{window->size(), window->framePosition(), states.testFlag(Qt::WindowMaximized),
                 (states & (Qt::WindowMaximized | Qt::WindowFullScreen)) != 0};
        watch.settled.start();
    };
    connect(window, &QWindow::widthChanged, this, read);
    connect(window, &QWindow::heightChanged, this, read);
    connect(window, &QWindow::xChanged, this, read);
    connect(window, &QWindow::yChanged, this, read);
    connect(window, &QWindow::windowStateChanged, this, read);
}

void WindowSettings::show(QQuickWindow *window) {
    restore(mainWatch, window, defaultSize, window->screen());
    // Shown at its own size first, which is then the size it is restored to.
    window->show();
    if (store->value(maximizedKey, false).toBool()) {
        window->showMaximized();
    }
    if (mainWatch.window != window) {
        watch(mainWatch, window);
    }
}

void WindowSettings::showVideo(QQuickWindow *video, QQuickWindow *main) {
    const QScreen *const screen = main->screen();
    const auto [size, back] = restore(videoWatch, video, defaultVideoSize, screen);
    if (!back && placed && screen) {
        // Its top left corner in view, should its least size be larger than the free area.
        const QRect room = screen->availableGeometry();
        video->setFramePosition({room.x() + std::max(0, (room.width() - size.width()) / 2),
                                 room.y() + std::max(0, (room.height() - size.height()) / 2)});
    }
    video->show();
    if (videoWatch.window != video) {
        watch(videoWatch, video);
    }
}

void WindowSettings::keepSaid(Watch &watch) {
    if (!watch.said) {
        return;
    }
    if (!watch.maximizedKey.isEmpty()) {
        store->setValue(watch.maximizedKey, watch.said->maximized);
    }
    if (!watch.said->whole) {
        store->setValue(watch.sizeKey, watch.said->size);
        if (placed) {
            store->setValue(watch.placeKey, watch.said->place);
        }
    }
    watch.said.reset();
}

void WindowSettings::keep() {
    keepSaid(mainWatch);
    keepSaid(videoWatch);
    // Now, not as the store would, at the event loop's next pass or as it is destroyed: the
    // window's hard end (main.cpp) ends the process straight after this, which runs neither.
    store->sync();
}

void WindowSettings::keepVideo() { keepSaid(videoWatch); }

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

QString WindowSettings::colours() const {
    return coloursKey(coloursNamed(store->value(paletteKey).toString()));
}

void WindowSettings::setColours(const QString &key) {
    const Colours chosen = coloursNamed(key);
    if (coloursKey(chosen) == colours()) {
        return;
    }
    store->setValue(paletteKey, coloursKey(chosen));
    useColours(chosen);
    emit coloursChanged();
}

QVariantList WindowSettings::colourChoices() const {
    QVariantList each;
    for (const Colours colours : everyColours) {
        each << QVariantMap{{QStringLiteral("key"), coloursKey(colours)},
                            {QStringLiteral("label"), coloursLabel(colours)}};
    }
    return each;
}

QString WindowSettings::coloursBeside(const QString &key, int by) {
    const auto at = std::find(everyColours.begin(), everyColours.end(), coloursNamed(key)) -
                    everyColours.begin();
    const auto to = std::clamp<std::ptrdiff_t>(at + by, 0, std::ssize(everyColours) - 1);
    return coloursKey(everyColours[static_cast<size_t>(to)]);
}

qreal WindowSettings::textSize(const QString &key) const {
    bool number = false;
    const qreal kept = store->value(key).toReal(&number);
    // Written so that a kept NaN, which compares false either way, is out of range too.
    if (!number || !(kept >= leastSize && kept <= mostSize)) {
        return platformSize();
    }
    return halfPoint(kept);
}

void WindowSettings::setTextSize(const QString &key, qreal points) {
    const qreal was = textSize(key);
    const qreal size = halfPoint(std::clamp(points, leastSize, mostSize));
    if (size == platformSize()) {
        store->remove(key);
    } else {
        store->setValue(key, size);
    }
    if (textSize(key) != was) {
        emit textSizesChanged();
    }
}

qreal WindowSettings::windowTextSize() const { return textSize(windowTextKey); }

qreal WindowSettings::listTextSize() const { return textSize(listTextKey); }

qreal WindowSettings::detailsTextSize() const { return textSize(detailsTextKey); }

int WindowSettings::leastHalfPoints() const { return qRound(leastSize * 2); }

int WindowSettings::mostHalfPoints() const { return qRound(mostSize * 2); }

void WindowSettings::setWindowTextSize(qreal points) { setTextSize(windowTextKey, points); }

void WindowSettings::setListTextSize(qreal points) { setTextSize(listTextKey, points); }

void WindowSettings::setDetailsTextSize(qreal points) { setTextSize(detailsTextKey, points); }

void WindowSettings::useSystemTextSizes() {
    bool any = false;
    for (const QString &key : {windowTextKey, listTextKey, detailsTextKey}) {
        any = any || store->contains(key);
        store->remove(key);
    }
    if (any) {
        emit textSizesChanged();
    }
}

QString WindowSettings::sizeWords(int halfPoints) {
    return QStringLiteral("%1 pt").arg(QString::number(halfPoints / 2.0));
}

int WindowSettings::widestSize(const QFont &font, int from, int to) {
    const QFontMetricsF metrics(font);
    int widest = from;
    qreal most = metrics.horizontalAdvance(sizeWords(from));
    for (int halfPoints = from + 1; halfPoints <= to; ++halfPoints) {
        const qreal wide = metrics.horizontalAdvance(sizeWords(halfPoints));
        if (wide > most) {
            most = wide;
            widest = halfPoints;
        }
    }
    return widest;
}

bool WindowSettings::noFindingsLast() const {
    // Not QVariant's toBool, which reads as true any text but "", "0" and "false", and an INI file
    // gives every value back as text: a value set by hand to anything but true leaves it off.
    return store->value(noFindingsLastKey).toString() == QLatin1String("true");
}

void WindowSettings::setNoFindingsLast(bool on) {
    if (on == noFindingsLast()) {
        return;
    }
    store->setValue(noFindingsLastKey, on);
    emit noFindingsLastChanged();
}

QString WindowSettings::rowStyle() const {
    return rowStyleNamed(store->value(rowStyleKey).toString());
}

void WindowSettings::setRowStyle(const QString &name) {
    const QString style = rowStyleNamed(name);
    if (style == rowStyle()) {
        return;
    }
    store->setValue(rowStyleKey, style);
    emit rowStyleChanged();
}
