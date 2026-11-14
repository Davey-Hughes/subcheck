// The window's review: one session, the thread that reads its events, and what QML shows of it
// and asks of it. Every event is applied on the GUI thread, in the order the session sent it.
#pragma once

#include "findings_model.h"
#include "player_port.h"
#include "subcheck_ffi.hpp"

#include <QObject>
#include <QString>
#include <QThread>
#include <QTimer>
#include <QtQml/qqmlregistration.h>

#include <chrono>
#include <memory>
#include <ostream>
#include <string>
#include <vector>

class SessionBridge : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("main.cpp makes the one bridge, for the run it was given")
    // What QML shows: the view's name, the status line (that the window is quitting, else review's
    // message, else the counts), the selected row (-1 for none), whether the run opened with no
    // findings, and a fatal end's text.
    Q_PROPERTY(FindingsModel *findings READ findings CONSTANT)
    Q_PROPERTY(QString title READ title NOTIFY summaryChanged)
    Q_PROPERTY(QString statusLine READ statusLine NOTIFY summaryChanged)
    Q_PROPERTY(int selected READ selected NOTIFY summaryChanged)
    Q_PROPERTY(bool empty READ empty NOTIFY summaryChanged)
    Q_PROPERTY(QString fatal READ fatal NOTIFY fatalChanged)
    // The player, which the video pane is the mpv of.
    Q_PROPERTY(PlayerPort *player READ player CONSTANT)

public:
    // `selector` is `subcheck review`'s: a run, or paths. `player` runs review's player commands,
    // and stops when review ends. The end's text goes to `out` and `err`, as the terminal's does
    // to stdout and stderr. `wait` is how long a quit gives review to end before the session is
    // closed, and the window ends anyway.
    SessionBridge(std::vector<std::string> selector, PlayerPort &player, std::ostream &out,
                  std::ostream &err, std::chrono::milliseconds wait, QObject *parent = nullptr);
    // Lets go of the session as every way out does, should the bridge go first.
    ~SessionBridge() override;

    // Opens the session and starts reading its events. Apart from the constructor, so that what
    // shows the first event is connected before it can come.
    void start();

    FindingsModel *findings();
    PlayerPort *player();
    QString title() const;
    QString statusLine() const;
    int selected() const;
    bool empty() const;
    QString fatal() const;

    // A key pressed in the list: the terminal's keys for what this window does. False for any
    // other key, which is left to Qt.
    Q_INVOKABLE bool key(int key, const QString &text);
    // The entry at `row`, as a click selects it.
    Q_INVOKABLE void select(int row);
    // The entry `by` places on from the selected one, as review holds it, or the list's first or
    // last: at an end, the entry already selected, which lets a message go.
    void move(int by);
    void toggle();
    void accept();
    void reject();
    void undo();
    // The selected entry's moment in the video pane; closing it; and the playback keys'.
    void play();
    void closePlayer();
    void playback(subcheck_ffi::Playback playback);
    // Asks review to end, and says so on the status line until it has. If it has not within the
    // bridge's wait, the session is closed and the window ends anyway, with status 1: what the
    // last decisions came to is unknown.
    Q_INVOKABLE void quit();
    // The fatal end's dialog has closed.
    Q_INVOKABLE void dismiss();

signals:
    // An event the pump read, for the GUI thread.
    void eventArrived(const subcheck_ffi::Event &event);
    void summaryChanged();
    void fatalChanged();
    // The window is done, and the process exits with `status`.
    void finished(int status);

private:
    void apply(const subcheck_ffi::Event &event);
    void changed(const subcheck_ffi::Event::kChanged &change);
    void ended(const subcheck_ffi::Event::kEnded &end);
    template <typename Call> void guarded(Call call);
    void send(subcheck_ffi::Action action);
    void report(subcheck_ffi::PlayerReport report);
    void fail(const std::string &why);
    void giveUp();
    void letGo();
    void finish(int status);

    std::vector<std::string> selector;
    PlayerPort &port;
    std::ostream &out;
    std::ostream &err;
    FindingsModel list;
    std::shared_ptr<subcheck_ffi::Session> session;
    std::unique_ptr<QThread> pump;
    QTimer quitWait;
    subcheck_ffi::Summary summary{};
    bool opened = false;
    bool quitting = false;
    bool done = false;
    QString fatalText;
};
