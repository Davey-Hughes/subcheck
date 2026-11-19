// What the window remembers between runs: its size, the place of its frame where the platform lets
// a window place itself, and the widths of its sidebar and details panel, the list taking what is
// left; and what the Settings window chooses, the palette and the size of each part's text. The
// frame's place is what is kept and what is restored, so that a window comes back to one place
// however many times it starts.
#pragma once

#include <QFont>
#include <QObject>
#include <QPoint>
#include <QRect>
#include <QRegion>
#include <QSettings>
#include <QSize>
#include <QString>
#include <QTimer>
#include <QVariantList>
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
    // The palette, by its kept name (words.h): a name no palette has reads as the default's. Each
    // palette's name and words, in the Settings window's order, as {key, label}.
    Q_PROPERTY(QString colours READ colours WRITE setColours NOTIFY coloursChanged)
    Q_PROPERTY(QVariantList colourChoices READ colourChoices CONSTANT)
    // The size of the text, in points, of each part of the window: the menus, toolbar, sidebar,
    // status line and dialogs; the findings list with its heading; and the details panel. Unset,
    // each is the platform's. Whole and half points only, from 6 to 24: a size kept outside that,
    // or not a number, reads as the platform's.
    Q_PROPERTY(
        qreal windowTextSize READ windowTextSize WRITE setWindowTextSize NOTIFY textSizesChanged)
    Q_PROPERTY(qreal listTextSize READ listTextSize WRITE setListTextSize NOTIFY textSizesChanged)
    Q_PROPERTY(
        qreal detailsTextSize READ detailsTextSize WRITE setDetailsTextSize NOTIFY textSizesChanged)
    // The least and most of those sizes, in half points, as the Settings window's boxes count them.
    Q_PROPERTY(int leastHalfPoints READ leastHalfPoints CONSTANT)
    Q_PROPERTY(int mostHalfPoints READ mostHalfPoints CONSTANT)

public:
    // `store` is where they stick: the app's own settings (appStore), or a test's file. `platform`
    // is the one Qt runs on, as QGuiApplication names it: where it lets no window place itself,
    // only the size sticks. `settle` is how long the window must say nothing more before what it
    // said is kept, long enough for a platform's reports of one maximize to have all landed. The
    // window draws in the kept palette from its first frame.
    WindowSettings(std::unique_ptr<QSettings> store, const QString &platform,
                   std::chrono::milliseconds settle = std::chrono::milliseconds(250),
                   QObject *parent = nullptr);
    // Keeps what the window last said, should it go before that settled.
    ~WindowSettings() override;

protected:
    // Says the text sizes changed when the application's font does.
    bool eventFilter(QObject *watched, QEvent *event) override;

public:
    // Whether a window can place itself on `platform`: not on Wayland, in any of its forms
    // (wayland, wayland-egl), where the compositor places each window.
    static bool places(const QString &platform);

    // The file the window's settings are kept in, as the store finds its own directory: under
    // `configHome`, XDG_CONFIG_HOME, when that is absolute, on every platform, macOS's among them,
    // where one that is set was set on purpose; otherwise under `home`'s .config, which is where
    // Qt keeps them on Linux, and none on macOS (`mac`), which then keeps them in its own
    // preferences. An empty or relative XDG_CONFIG_HOME is ignored, as the XDG spec says: a
    // relative one would resolve against the working directory.
    static std::optional<QString> fileFor(bool mac, const QString &configHome, const QString &home);
    // The app's settings: in fileFor's file for this platform, environment and home, else in the
    // platform's own place for them.
    static std::unique_ptr<QSettings> appStore();

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

    QString colours() const;
    // Kept, and drawn with at once; a name no palette has keeps the default.
    void setColours(const QString &key);
    QVariantList colourChoices() const;
    // The palette `by` places on from `key`'s in the Settings window's order, or its first or last:
    // the arrows step through them as through any desktop's radio buttons, and stop at the ends.
    Q_INVOKABLE static QString coloursBeside(const QString &key, int by);

    qreal windowTextSize() const;
    qreal listTextSize() const;
    qreal detailsTextSize() const;
    int leastHalfPoints() const;
    int mostHalfPoints() const;
    // Each kept at once, to the nearest half point from 6 to 24. The platform's size is kept as no
    // size, so that the part follows the platform's again.
    void setWindowTextSize(qreal points);
    void setListTextSize(qreal points);
    void setDetailsTextSize(qreal points);
    // Every part back to the platform's size.
    Q_INVOKABLE void useSystemTextSizes();
    // A size as the Settings window says it, given in half points: "10 pt", "10.5 pt".
    Q_INVOKABLE static QString sizeWords(int halfPoints);
    // The size, given in half points from `from` to `to`, whose words are widest in `font`: a
    // box as wide as one saying it says every size without growing. Each size is measured, as a
    // font's digits need not all be as wide as each other.
    Q_INVOKABLE static int widestSize(const QFont &font, int from, int to);

signals:
    void columnsChanged();
    void coloursChanged();
    void textSizesChanged();

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

    qreal textSize(const QString &key) const;
    void setTextSize(const QString &key, qreal points);

    std::unique_ptr<QSettings> store;
    bool placed;
    std::optional<Said> said;
    QTimer settled;
};
