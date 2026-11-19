#include "menu_keys.h"

#include <QCoreApplication>
#include <QEvent>
#include <QGuiApplication>

MenuKeys::MenuKeys(QObject *parent) : MenuKeys(QGuiApplication::platformName(), parent) {}

MenuKeys::MenuKeys(const QString &platform, QObject *parent)
    : QObject(parent), drawn(drawnBar(platform)) {}

bool MenuKeys::drawnBar(const QString &platform) {
    return platform != QLatin1String("cocoa") ||
           QCoreApplication::testAttribute(Qt::AA_DontUseNativeMenuBar);
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
    if (window && drawn) {
        window->installEventFilter(this);
    }
    emit targetChanged();
}

bool MenuKeys::eventFilter(QObject *watched, QEvent *event) {
    // Accepted, a key is no shortcut: Qt then delivers it as a key press, to the open menu.
    if (open && event->type() == QEvent::ShortcutOverride) {
        event->accept();
        return true;
    }
    return QObject::eventFilter(watched, event);
}
