// The video pane: MpvQt's item, which draws mpv's video in the window, given to the player as the
// mpv it runs commands on, and passing on to it what mpv says.
#pragma once

#include "player_port.h"

#include <MpvAbstractItem>
#include <QPointer>
#include <QtQml/qqmlregistration.h>

class MpvItem : public MpvAbstractItem, public Mpv {
    Q_OBJECT
    QML_ELEMENT
    // The player this pane is the mpv of, which the window binds once.
    Q_PROPERTY(PlayerPort *player READ player WRITE setPlayer NOTIFY playerChanged)
    // As mpv says: whether it is paused, and whether it has a video output, which a file open has.
    Q_PROPERTY(bool paused READ paused NOTIFY pausedChanged)
    Q_PROPERTY(bool showing READ showing NOTIFY showingChanged)

public:
    explicit MpvItem(QQuickItem *parent = nullptr);
    ~MpvItem() override;

    PlayerPort *player() const;
    void setPlayer(PlayerPort *player);
    bool paused() const { return isPaused; }
    bool showing() const { return isShowing; }

    void command(std::uint64_t id, const QStringList &args) override;
    void get(std::uint64_t id, const QString &name) override;

    // What mpv says of a property it is observed on, as the controller's signal carries it; public
    // so that a test can say it for mpv.
    void heard(const QString &name, const QVariant &value);

signals:
    void playerChanged();
    void pausedChanged();
    void showingChanged();

private:
    // Null once the player has gone, whichever of the two goes first.
    QPointer<PlayerPort> port;
    bool isPaused = false;
    bool isShowing = false;
};
