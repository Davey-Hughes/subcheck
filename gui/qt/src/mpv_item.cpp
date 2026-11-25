#include "mpv_item.h"

#include "words.h"

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
    connect(mpv, &MpvController::propertyChanged, this, &MpvItem::heard);
    connect(mpv, &MpvController::endFile, this, [this](const QString &reason) {
        if (port) {
            port->ended(endOf(reason));
        }
    });
    observeProperty(QStringLiteral("time-pos"), MPV_FORMAT_DOUBLE);
    observeProperty(QStringLiteral("vo-configured"), MPV_FORMAT_FLAG);
    // After vo-configured: a change of pause is then a sign that mpv's first word on vo-configured
    // has been heard, which a test waits on.
    observeProperty(QStringLiteral("pause"), MPV_FORMAT_FLAG);
    // mpv's render call otherwise waits out each frame's display time, and the window's frame,
    // which waits on that call, with it: the whole window is held to the video's frame rate while
    // it plays. At 0 the call draws the frame due and returns. A global option, it holds for every
    // file after.
    MpvAbstractItem::setProperty(QStringLiteral("video-timing-offset"), 0.0);
}

// The player must not send this pane requests once it has gone.
MpvItem::~MpvItem() {
    if (port) {
        port->detach(this);
    }
}

PlayerPort *MpvItem::player() const { return port; }

// The window binds it as it makes the pane, before mpv can be ready. A player this pane had before
// lets go of it, as it will hear nothing more from it.
void MpvItem::setPlayer(PlayerPort *player) {
    if (player == port) {
        return;
    }
    if (port) {
        port->detach(this);
    }
    port = player;
    if (port) {
        port->attach(this);
    }
    emit playerChanged();
}

void MpvItem::heard(const QString &name, const QVariant &value) {
    if (name == QStringLiteral("time-pos")) {
        // A position that is gone, as when a file ends, is no landing.
        if (port && value.isValid()) {
            port->moved();
        }
        const QString now =
            value.isValid() ? exactClock(qRound64(value.toDouble() * 1000)) : QString();
        if (now != saidClock) {
            saidClock = now;
            emit clockChanged();
        }
    }
    if (name == QStringLiteral("pause") && isPaused != value.toBool()) {
        isPaused = value.toBool();
        emit pausedChanged();
    }
    if (name == QStringLiteral("vo-configured")) {
        if (isShowing != value.toBool()) {
            isShowing = value.toBool();
            emit showingChanged();
        }
        // The pane shows what it last drew until it draws again, and MpvQt draws only when
        // mpv asks; mpv need not ask as its video output goes, when a file is closed or
        // plays to its end. Drawn then, as if mpv had asked, with no video, it is black.
        if (!isShowing) {
            requestUpdateFromRenderer();
        }
    }
}

// Once the stretch ends, the frame is drawn again at the pane's size: MpvQt draws only when mpv
// asks, which mpv does not while paused, and the frame would stay stretched.
void MpvItem::setStretching(bool stretching) {
    if (stretching == isStretching) {
        return;
    }
    isStretching = stretching;
    setTextureFollowsItemSize(!stretching);
    if (!stretching) {
        update();
    }
    emit stretchingChanged();
}

void MpvItem::command(std::uint64_t id, const QStringList &args) {
    commandAsync(args, static_cast<int>(id));
}

void MpvItem::get(std::uint64_t id, const QString &name) {
    getPropertyAsync(name, static_cast<int>(id));
}

// MpvQt 1.2.1 names mpv's ends "eof", "stop" and "error", and passes on no other. Any other name is
// taken for a stop, which ends no wait, so that a name that is no end of the load cannot end its
// wait early. The cost falls on a real error under a new name: while the load settles it goes
// unseen, the Open reports opened when the settle time ends, and the load stays late, the animation
// up, as any late load does, until something ends that wait (`late` in player_port.h).
PlayerPort::End MpvItem::endOf(const QString &reason) {
    if (reason == QLatin1String("error")) {
        return PlayerPort::End::Failed;
    }
    if (reason == QLatin1String("eof")) {
        return PlayerPort::End::Finished;
    }
    return PlayerPort::End::Stopped;
}
