// The video pane: MpvQt's item, which draws mpv's video in the window, given to the player as the
// mpv it runs commands on, and passing on to it what mpv says.
#pragma once

#include "player_port.h"

#include <MpvAbstractItem>
#include <QtQml/qqmlregistration.h>

class MpvItem : public MpvAbstractItem, public Mpv {
    Q_OBJECT
    QML_ELEMENT
    // The player this pane is the mpv of, which the window binds once.
    Q_PROPERTY(PlayerPort *player READ player WRITE setPlayer NOTIFY playerChanged)

public:
    explicit MpvItem(QQuickItem *parent = nullptr);
    ~MpvItem() override;

    PlayerPort *player() const;
    void setPlayer(PlayerPort *player);

    void command(std::uint64_t id, const QStringList &args) override;
    void get(std::uint64_t id, const QString &name) override;

signals:
    void playerChanged();

private:
    PlayerPort *port = nullptr;
};
