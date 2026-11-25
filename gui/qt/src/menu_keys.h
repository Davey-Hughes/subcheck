// Who has the keys while a menu is open over the window. The window's shortcuts are asked first
// about every key, so that without this a letter underlined in an open menu would run another
// item's shortcut, and the arrows would seek the video instead of moving through the menu. Two
// rules give the keys to what is open:
// - A menu of the window's bar, while it is open, where the window draws the bar (drawnBar).
// - Any popup the window draws that has the keys: a menu off the bar, as the list's Rows menu, a
//   text field's own menu, a sheet or a dialog that takes them. Qt Quick draws a popup in the
//   window, or in a window of its own over it (as Fusion's text field menu is but on Wayland),
//   wherever the platform has no native one, and the window's shortcuts still fire behind it: Qt
//   matches them for a key typed into a window transient for the window too. So this rule holds on
//   every platform, for a key that reaches the window or the popup's own window, and needs no
//   wiring in QML; a native popup has the keys before the window hears of them, so the rule never
//   meets one.
// Either keeps the window's Close and Quit keys too: a dialog that has the keys still ends by its
// own Esc and buttons.
#pragma once

#include <QObject>
#include <QPointer>
#include <QString>
#include <QWindow>
#include <QtQml/qqmlregistration.h>

class MenuKeys : public QObject {
    Q_OBJECT
    QML_ELEMENT
    // The window whose shortcuts give way.
    Q_PROPERTY(QObject *target READ target WRITE setTarget NOTIFY targetChanged)
    // Whether a menu of its bar is open, as QML sees it.
    Q_PROPERTY(bool menuOpen MEMBER open NOTIFY menuOpenChanged)

public:
    explicit MenuKeys(QObject *parent = nullptr);
    // On `platform`, as QGuiApplication names it, rather than the one Qt runs on.
    explicit MenuKeys(const QString &platform, QObject *parent = nullptr);

    // Whether the window draws the menu bar on `platform`. On macOS, unless the app has asked Qt
    // not to, the bar is the system's, whose open menu has the keys before the window hears of
    // them: there is nothing to give way to, so the bar's rule keeps no key there.
    static bool drawnBar(const QString &platform);

    // Whether a menu of the bar has the keys, as the first rule above says.
    bool barHasTheKeys() const;

    // Whether a popup the window draws has its keys, as the second rule above says: the
    // application's, in the popup's own window over it, or the window's own, in an item in a popup
    // in it.
    static bool popupHasTheKeys(QObject *window);

    QObject *target() const;
    void setTarget(QObject *window);

signals:
    void targetChanged();
    void menuOpenChanged();

protected:
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    // Watches `focus`, the window with the application's keys, while it is a popup's own window
    // over the target, and none other: a key typed there is the popup's.
    void follow(QWindow *focus);

    bool drawn;
    bool open = false;
    QPointer<QObject> window;
    QPointer<QWindow> popup;
};
