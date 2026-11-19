// Who has the keys while a menu of the window's bar is open. The window's shortcuts are asked first
// about every key, so that without this a letter underlined in the open menu would run another
// item's shortcut, and the arrows would seek the video instead of moving along the bar.
#pragma once

#include <QObject>
#include <QPointer>
#include <QString>
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
    // them: there is nothing to give way to, so the window's shortcuts are left alone.
    static bool drawnBar(const QString &platform);

    QObject *target() const;
    void setTarget(QObject *window);

signals:
    void targetChanged();
    void menuOpenChanged();

protected:
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    bool drawn;
    bool open = false;
    QPointer<QObject> window;
};
