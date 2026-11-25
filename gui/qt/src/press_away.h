// Says when a press of the pointer in a window lands anywhere but on one of its items, the keeper,
// while no menu has the window's keys: the window then takes its keys back from the Find box,
// whose words stay. One rule at the window, which hears of each press before any of its items
// does, rather than a call in each part a press can land on: most parts, a row, a label, the
// video, a button, a column handle, take no keys, and would leave them with the box, so that the
// next letter, an A or a J, would type into it; a call in each holds only for the parts that make
// one, and a part added later makes none. A part that takes the keys itself, as the edit's field
// does, still takes them: the press reaches it after. A press on the keeper, the box's field or
// its buttons, leaves the keys where they are, and so does a press while a menu has them by either
// of menu_keys.h's rules, an open menu of the bar or a popup with the keys: the press is the
// menu's, on its items or beside it, closing it.
#pragma once

#include <QObject>
#include <QPointer>
#include <QPointerEvent>
#include <QQuickItem>
#include <QtQml/qqmlregistration.h>

class MenuKeys;

class PressAway : public QObject {
    Q_OBJECT
    QML_ELEMENT
    Q_MOC_INCLUDE("menu_keys.h")
    // The window whose presses it hears.
    Q_PROPERTY(QObject *target READ target WRITE setTarget NOTIFY targetChanged)
    // The item a press on leaves the keys where they are; with none, every press is away.
    Q_PROPERTY(QQuickItem *keeper READ keeper WRITE setKeeper NOTIFY keeperChanged)
    // The MenuKeys that says whether an open menu of the window's bar has its keys; with none, a
    // menu of a bar holds no press.
    Q_PROPERTY(MenuKeys *menus READ menus WRITE setMenus NOTIFY menusChanged)

public:
    explicit PressAway(QObject *parent = nullptr);

    QObject *target() const;
    void setTarget(QObject *window);
    QQuickItem *keeper() const;
    void setKeeper(QQuickItem *item);
    MenuKeys *menus() const;
    void setMenus(MenuKeys *keys);

signals:
    void targetChanged();
    void keeperChanged();
    void menusChanged();
    // A press of the pointer landed in the window away from the keeper, while no menu had the keys.
    void pressedAway();

protected:
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    // Whether a point of `press`, in the window's coordinates, lies outside the keeper.
    bool away(const QPointerEvent &press) const;

    QPointer<QObject> window;
    QPointer<QQuickItem> kept;
    QPointer<MenuKeys> keys;
};
