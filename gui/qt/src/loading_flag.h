// Whether the window shows that something loads: only once the load has lasted a moment, so that a
// quick one flashes nothing, and gone only once it has been over for a moment, so that one load
// that ends as the next starts is one animation, not two with a blink between.
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
