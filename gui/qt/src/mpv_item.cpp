#include "mpv_item.h"

#include <MpvController>

MpvItem::MpvItem(QQuickItem *parent) : MpvAbstractItem(parent) {
    // mpv's events come from the controller's own thread; each is passed on in this item's, the
    // window's, in mpv's order.
    MpvController *mpv = mpvController();
    connect(this, &MpvAbstractItem::ready, this, [this] {
        if (port) {
            port->ready();
        }
    });
    // Only the event's id and error are read, never its data: that points into mpv's memory, which
    // mpv has freed by the time this queued call runs. What the reply carries is `data`, which
    // MpvQt copied out on its own thread.
    connect(mpv, &MpvController::asyncReply, this, [this](const QVariant &data, mpv_event event) {
        if (port) {
            port->answered(event.reply_userdata,
                           event.error < 0 ? QString::fromUtf8(mpv_error_string(event.error))
                                           : QString(),
                           data);
        }
    });
    connect(mpv, &MpvController::playbackRestart, this, [this] {
        if (port) {
            port->restarted();
        }
    });
    connect(mpv, &MpvController::propertyChanged, this,
            [this](const QString &name, const QVariant &value) {
                // A position that is gone, as when a file ends, is no landing.
                if (port && name == QStringLiteral("time-pos") && value.isValid()) {
                    port->moved();
                }
            });
    connect(mpv, &MpvController::endFile, this, [this](const QString &reason) {
        if (port) {
            port->ended(reason == QStringLiteral("error"));
        }
    });
    observeProperty(QStringLiteral("time-pos"), MPV_FORMAT_DOUBLE);
}

// The player outlives the window that shows this pane, and must not send it requests after.
MpvItem::~MpvItem() {
    if (port) {
        port->attach(nullptr);
    }
}

PlayerPort *MpvItem::player() const { return port; }

// The window binds it as it makes the pane, before mpv can be ready.
void MpvItem::setPlayer(PlayerPort *player) {
    port = player;
    if (port) {
        port->attach(this);
    }
    emit playerChanged();
}

void MpvItem::command(std::uint64_t id, const QStringList &args) {
    commandAsync(args, static_cast<int>(id));
}

void MpvItem::get(std::uint64_t id, const QString &name) {
    getPropertyAsync(name, static_cast<int>(id));
}
