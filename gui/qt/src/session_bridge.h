// The window's review: one session, the thread that reads its events, and what QML shows of it
// and asks of it. Every event is applied on the GUI thread, in the order the session sent it.
#pragma once

#include "detail_view.h"
#include "findings_model.h"
#include "player_port.h"
#include "sidebar_models.h"
#include "subcheck_ffi.hpp"

#include <QObject>
#include <QString>
#include <QStringList>
#include <QThread>
#include <QTimer>
#include <QtQml/qqmlregistration.h>

#include <chrono>
#include <memory>
#include <mutex>
#include <optional>
#include <ostream>
#include <string>
#include <vector>

class SessionBridge : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("main.cpp makes the one bridge, for the run it was given")
    // The lists QML shows: the findings, the sidebar's reviews, filters and episodes; and the
    // selected finding's details.
    Q_PROPERTY(FindingsModel *findings READ findings CONSTANT)
    Q_PROPERTY(FoldersModel *folders READ folders CONSTANT)
    Q_PROPERTY(FiltersModel *filters READ filters CONSTANT)
    Q_PROPERTY(EpisodesModel *episodes READ episodes CONSTANT)
    Q_PROPERTY(DetailView *detail READ detail CONSTANT)
    // The view's name; its counts; the list's heading (the filter's name and count); the words
    // of the View menu's item that switches between a check and its folders; the status bar
    // (that the window is quitting, else review's message, else nothing); the selected row (-1
    // for none); and what the list says when it shows no row, that the run has no findings or
    // that the filter hides them all, empty while it shows any.
    Q_PROPERTY(QString title READ title NOTIFY summaryChanged)
    Q_PROPERTY(QString counts READ counts NOTIFY summaryChanged)
    Q_PROPERTY(QString heading READ heading NOTIFY summaryChanged)
    Q_PROPERTY(QString switchLabel READ switchLabel NOTIFY summaryChanged)
    Q_PROPERTY(QString statusLine READ statusLine NOTIFY summaryChanged)
    Q_PROPERTY(int selected READ selected NOTIFY summaryChanged)
    Q_PROPERTY(QString emptyWords READ emptyWords NOTIFY summaryChanged)
    // The episode selected (the first as a view opens, then the one last clicked) and what the
    // check and apply said of it; empty but while it is the selected finding's or its row's.
    Q_PROPERTY(QString episodeName READ episodeName NOTIFY summaryChanged)
    Q_PROPERTY(QStringList episodeLog READ episodeLog NOTIFY summaryChanged)
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
    FoldersModel *folders();
    FiltersModel *filters();
    EpisodesModel *episodes();
    DetailView *detail();
    PlayerPort *player();
    QString title() const;
    QString counts() const;
    QString heading() const;
    QString switchLabel() const;
    QString statusLine() const;
    int selected() const;
    QString emptyWords() const;
    QString episodeName() const;
    QStringList episodeLog() const;
    QString fatal() const;

    // What the window's menus, keys, buttons and clicks ask of review, each one action but where
    // it says otherwise. The entry at `row`, as a click selects it.
    Q_INVOKABLE void select(int row);
    // The entry `by` places on from the selected one, as review holds it, or the list's first or
    // last: at an end, the entry already selected, which lets a message go.
    Q_INVOKABLE void move(int by);
    Q_INVOKABLE void toggle();
    Q_INVOKABLE void accept();
    Q_INVOKABLE void reject();
    Q_INVOKABLE void clear();
    Q_INVOKABLE void undo();
    // The selected entry's moment in the video pane; closing it; and the video's own.
    Q_INVOKABLE void play();
    Q_INVOKABLE void closePlayer();
    Q_INVOKABLE void pause();
    Q_INVOKABLE void seek(int seconds);
    Q_INVOKABLE void nextFrame();
    Q_INVOKABLE void previousFrame();
    // The filter at `row` of the sidebar's list; the one after the filter shown, the first after
    // the last, so that one key steps through them in the sidebar's order.
    Q_INVOKABLE void setFilter(int row);
    Q_INVOKABLE void cycleFilter();
    Q_INVOKABLE void nextEpisode();
    Q_INVOKABLE void previousEpisode();
    // Between a check's view and its folders'.
    Q_INVOKABLE void switchView();
    // The sidebar's review at `row`.
    Q_INVOKABLE void openFolder(int row);
    // The sidebar's episode at `row`: the file selected, which review gives the logs of, then its
    // first finding. Nothing is sent as the selection later moves into another episode: a
    // `SelectFile` then would let review's message, or an open question, go.
    Q_INVOKABLE void openEpisode(int row);
    void playback(subcheck_ffi::Playback playback);
    // Any of review's actions; after the session is let go of, it goes nowhere. Virtual, so that
    // a test can see each action the window sends.
    virtual void send(subcheck_ffi::Action action);
    // Asks review to end, stopping a job under way, and says so on the status bar until it has.
    // If it has not within the bridge's wait, the session is closed and the window ends anyway,
    // with status 1: what the last decisions came to is unknown.
    Q_INVOKABLE void quit();
    // The fatal end's dialog has closed.
    Q_INVOKABLE void dismiss();

signals:
    // An event the pump read, for the GUI thread.
    void eventArrived(const subcheck_ffi::Event &event);
    void summaryChanged();
    // Another entry of the list is selected, rather than the selected one refreshed in place.
    void selectionMoved();
    void fatalChanged();
    // The window is done, and the process exits with `status`.
    void finished(int status);

protected:
    // Closes the session: a call that throws only on a Rust panic, which no test can make a real
    // session's close do, so a test's bridge throws its own. The destructor's close is always
    // this one, as a destructor calls no override.
    virtual void closeSession(subcheck_ffi::Session &session);

private:
    // What the pump hands on, which it holds with the bridge: a pump let go of, after a close that
    // threw, reaches nothing of a bridge that has let go of it. `end` is the stream's last event,
    // when it is an end: close's own, or review's should it come first.
    struct Relay {
        std::mutex lock;
        SessionBridge *to = nullptr;
        std::optional<subcheck_ffi::Event::kEnded> end;
    };

    void apply(const subcheck_ffi::Event &event);
    void changed(const subcheck_ffi::Event::kChanged &change);
    void ended(const subcheck_ffi::Event::kEnded &end);
    template <typename Call> void guarded(Call call);
    void report(subcheck_ffi::PlayerReport report);
    void fail(const std::string &why);
    void giveUp();
    std::optional<subcheck_ffi::Event::kEnded> letGo();
    void say(const subcheck_ffi::Event::kEnded &end);
    void finish(int status);

    std::vector<std::string> selector;
    PlayerPort &port;
    std::ostream &out;
    std::ostream &err;
    FindingsModel list;
    FoldersModel reviews;
    FiltersModel shows;
    EpisodesModel files;
    DetailView details;
    std::optional<subcheck_ffi::FileLogs> logs;
    std::shared_ptr<subcheck_ffi::Session> session;
    std::shared_ptr<Relay> relay;
    std::unique_ptr<QThread> pump;
    QTimer quitWait;
    subcheck_ffi::Summary summary{};
    std::optional<subcheck_ffi::EntryKey> selectedEntry;
    bool opened = false;
    bool quitting = false;
    bool done = false;
    QString fatalText;
};
