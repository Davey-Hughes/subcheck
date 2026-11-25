#include "menu_keys.h"

#include <QCoreApplication>
#include <QEvent>
#include <QGuiApplication>
#include <QQuickItem>
#include <QQuickWindow>

namespace {

// Whether `focus` is the window of a popup open over `window`, as Qt Quick makes one for a popup
// it draws in a window of its own. A hidden one, which can keep the application's keys on a
// platform with no window manager to take them back, holds none.
bool popupOver(const QWindow *focus, const QObject *window) {
    return focus && window && focus->type() == Qt::Popup && focus->isVisible() &&
           focus->transientParent() == window;
}

} // namespace

MenuKeys::MenuKeys(QObject *parent) : MenuKeys(QGuiApplication::platformName(), parent) {}

MenuKeys::MenuKeys(const QString &platform, QObject *parent)
    : QObject(parent), drawn(drawnBar(platform)) {
    connect(qGuiApp, &QGuiApplication::focusWindowChanged, this, &MenuKeys::follow);
}

bool MenuKeys::drawnBar(const QString &platform) {
    return platform != QLatin1String("cocoa") ||
           QCoreApplication::testAttribute(Qt::AA_DontUseNativeMenuBar);
}

bool MenuKeys::barHasTheKeys() const { return open && drawn; }

// Every Popup's items sit in its QQuickPopupItem, a class Qt keeps private, so it is known by its
// name.
bool MenuKeys::popupHasTheKeys(QObject *window) {
    if (popupOver(QGuiApplication::focusWindow(), window)) {
        return true;
    }
    const auto *quick = qobject_cast<QQuickWindow *>(window);
    for (QQuickItem *item = quick ? quick->activeFocusItem() : nullptr; item;
         item = item->parentItem()) {
        if (item->inherits("QQuickPopupItem")) {
            return true;
        }
    }
    return false;
}

QObject *MenuKeys::target() const { return window; }

void MenuKeys::setTarget(QObject *to) {
    if (to == window) {
        return;
    }
    if (window) {
        window->removeEventFilter(this);
    }
    window = to;
    if (window) {
        window->installEventFilter(this);
    }
    follow(QGuiApplication::focusWindow());
    emit targetChanged();
}

void MenuKeys::follow(QWindow *focus) {
    if (popup) {
        popup->removeEventFilter(this);
    }
    popup = popupOver(focus, window) ? focus : nullptr;
    if (popup) {
        popup->installEventFilter(this);
    }
}

bool MenuKeys::eventFilter(QObject *watched, QEvent *event) {
    // Accepted, a key is no shortcut: Qt then delivers it as a key press, to what is open.
    if (event->type() == QEvent::ShortcutOverride && (barHasTheKeys() || popupHasTheKeys(window))) {
        event->accept();
        return true;
    }
    return QObject::eventFilter(watched, event);
}
