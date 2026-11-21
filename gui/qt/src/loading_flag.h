// Whether the window shows that something loads: only once the load has lasted a moment, so that a
// quick one flashes nothing, and gone only once it has been over for a moment, so that one load
// that ends as the next starts is one animation, not two with a blink between. Before the show, a
// stop ends the wait, and a start after it, however soon, waits a moment of its own, so that a
// quick load shows nothing whatever came before it. A wait handed on, as Show in Video's is from
// review's check of the file to the player's Open, comes to the flag as one, with no stop between:
// review's change that ends the check says that the Open follows.
#pragma once

#include <QObject>
#include <QTimer>

#include <chrono>

class LoadingFlag : public QObject {
    Q_OBJECT

public:
    LoadingFlag(std::chrono::milliseconds showAfter, std::chrono::milliseconds hideAfter,
                QObject *parent = nullptr);

    // Whether something loads now; the same value again changes nothing.
    void set(bool loading);
    bool shown() const;

signals:
    void shownChanged();

private:
    std::chrono::milliseconds showAfter;
    std::chrono::milliseconds hideAfter;
    // Runs while what loads differs from what is shown, for the moment that change waits.
    QTimer moment;
    bool loading = false;
    bool isShown = false;
};
