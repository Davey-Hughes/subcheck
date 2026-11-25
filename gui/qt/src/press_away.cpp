#include "press_away.h"

#include "menu_keys.h"

#include <QEvent>
#include <QEventPoint>

#include <algorithm>

PressAway::PressAway(QObject *parent) : QObject(parent) {}

QObject *PressAway::target() const { return window; }

void PressAway::setTarget(QObject *to) {
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
    emit targetChanged();
}

QQuickItem *PressAway::keeper() const { return kept; }

void PressAway::setKeeper(QQuickItem *item) {
    if (item == kept) {
        return;
    }
    kept = item;
    emit keeperChanged();
}

MenuKeys *PressAway::menus() const { return keys; }

void PressAway::setMenus(MenuKeys *to) {
    if (to == keys) {
        return;
    }
    keys = to;
    emit menusChanged();
}

bool PressAway::eventFilter(QObject *watched, QEvent *event) {
    const QEvent::Type type = event->type();
    if ((type == QEvent::MouseButtonPress || type == QEvent::TouchBegin ||
         type == QEvent::TabletPress) &&
        !(keys && keys->barHasTheKeys()) && !MenuKeys::popupHasTheKeys(window) &&
        away(*static_cast<QPointerEvent *>(event))) {
        emit pressedAway();
    }
    return QObject::eventFilter(watched, event);
}

bool PressAway::away(const QPointerEvent &press) const {
    const QList<QEventPoint> &points = press.points();
    return std::any_of(points.begin(), points.end(), [this](const QEventPoint &point) {
        return !kept || !kept->contains(kept->mapFromScene(point.scenePosition()));
    });
}
