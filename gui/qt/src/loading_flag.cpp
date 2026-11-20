#include "loading_flag.h"

LoadingFlag::LoadingFlag(std::chrono::milliseconds showAfter, std::chrono::milliseconds hideAfter,
                         QObject *parent)
    : QObject(parent), showAfter(showAfter), hideAfter(hideAfter) {
    // Precise: a coarse timer may end up to 5% early, and show a load before it has lasted its
    // moment.
    moment.setSingleShot(true);
    moment.setTimerType(Qt::PreciseTimer);
    connect(&moment, &QTimer::timeout, this, [this] {
        isShown = loading;
        emit shownChanged();
    });
}

void LoadingFlag::set(bool loading) {
    if (loading == this->loading) {
        return;
    }
    this->loading = loading;
    // Back to what is shown before the moment was up: there is no change left to show.
    if (loading == isShown) {
        moment.stop();
    } else {
        moment.start(loading ? showAfter : hideAfter);
    }
}

bool LoadingFlag::shown() const { return isShown; }
