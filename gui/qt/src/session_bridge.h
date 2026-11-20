// The window's review: one session, the thread that reads its events, and what QML shows of it
// and asks of it. Every event is applied on the GUI thread, in the order the session sent it.
#pragma once

#include "detail_view.h"
#include "findings_model.h"
#include "loading_flag.h"
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
#include <cstdint>
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
    // The lists QML shows: the findings, the sidebar's reviews, filters and episodes, the last in
    // the order the View menu chose; and the selected finding's details.
    Q_PROPERTY(FindingsModel *findings READ findings CONSTANT)
    Q_PROPERTY(FoldersModel *folders READ folders CONSTANT)
    Q_PROPERTY(FiltersModel *filters READ filters CONSTANT)
    Q_PROPERTY(EpisodeOrder *episodes READ episodes CONSTANT)
    Q_PROPERTY(DetailView *detail READ detail CONSTANT)
    // A finding made up for the Settings window, which draws it as the details panel does, so that
    // a palette is seen on a change before it is chosen.
    Q_PROPERTY(DetailView *sample READ sample CONSTANT)
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
    // check and apply said of it; empty but while it is the selected finding's or its row's, or one
    // the check found nothing in, clicked, until its view ends, as showingEpisode says.
    Q_PROPERTY(QString episodeName READ episodeName NOTIFY summaryChanged)
    Q_PROPERTY(QStringList episodeLog READ episodeLog NOTIFY summaryChanged)
    // Whether that episode is one the check found nothing in, clicked: the details panel shows its
    // log alone, and the keys that act on the finding review still has selected wait. The view
    // changes only with review's answer to the action that changes it, the first change or answer
    // alone (an `Event::Answered`) whose count reaches that action's: a click on a greyed episode
    // shows it, unless review was asking as it took the click; a select, a move, Next or Previous
    // Episode, an undo or a click on an episode with findings, at the answer to the last of its two
    // actions, ends it, each of which can leave review's selection where it is. Any other change
    // that selects another finding ends it too, unless it answers an action sent before the last
    // greyed click; so does one with another file selected while no answer that changes the view
    // is awaited.
    Q_PROPERTY(bool showingEpisode READ showingEpisode NOTIFY summaryChanged)
    Q_PROPERTY(QString fatal READ fatal NOTIFY fatalChanged)
    // The player, which the video pane is the mpv of; and whether the video shows that it
    // loads: once the player's Open has lasted a moment, until a moment after it ends.
    Q_PROPERTY(PlayerPort *player READ player CONSTANT)
    Q_PROPERTY(bool videoLoading READ videoLoading NOTIFY videoLoadingChanged)

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
    EpisodeOrder *episodes();
    DetailView *detail();
    DetailView *sample();
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
    bool showingEpisode() const;
    QString fatal() const;
    bool videoLoading() const;

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
    // The first finding of the episode after or before the selected finding's, as review steps
    // through them; each ends a greyed episode's view, as showingEpisode says.
    Q_INVOKABLE void nextEpisode();
    Q_INVOKABLE void previousEpisode();
    // Between a check's view and its folders'.
    Q_INVOKABLE void switchView();
    // The sidebar's review at `row`.
    Q_INVOKABLE void openFolder(int row);
    // The sidebar's episode at `row` of its list as shown, whichever order that is in: the file
    // selected, which review gives the logs of, then its first finding. Nothing is sent as the
    // selection later moves into another episode: a `SelectFile` then would let review's message,
    // or an open question, go. An episode the check found nothing in has no finding to go to, and
    // review would say so on the status bar, so its file alone is selected: what the check said of
    // it shows alone, from review's answer, while review keeps the finding selected, until its view
    // ends, as showingEpisode says. Under a question, as review's answer tells, review only lets
    // the question go, and the view stays as it was.
    Q_INVOKABLE void openEpisode(int row);
    void playback(subcheck_ffi::Playback playback);
    // Any of review's actions, counted: review's change or answer alone (an `Event::Answered`) that
    // carries the count answers it. After the session is let go of, it goes nowhere. Virtual, so
    // that a test can see each action the window sends.
    virtual void send(subcheck_ffi::Action action);
    // Asks review to end, stopping a job under way, and says so on the status bar until it has.
    // If it has not within the bridge's wait, the session is closed and the window ends anyway,
    // with status 1: what the last decisions came to is unknown.
    Q_INVOKABLE void quit();
    // The fatal end's dialog has closed.
    Q_INVOKABLE void dismiss();
    // Everything drawn in the palette's colours, drawn again in the one now chosen: the rows, the
    // details and the sample. Nothing else changes, so the selection and every scroll stay.
    Q_INVOKABLE void recolour();

signals:
    // An event the pump read, for the GUI thread.
    void eventArrived(const subcheck_ffi::Event &event);
    void summaryChanged();
    // Another entry of the list is selected, rather than the selected one refreshed in place.
    void selectionMoved();
    // What the check said of an episode with no findings shows, for a click on it: the details
    // panel scrolls to it, as the panel can be scrolled down the finding it takes the place of.
    void episodeShown();
    void fatalChanged();
    void videoLoadingChanged();
    // The window is done, and the process exits with `status`.
    void finished(int status);

protected:
    // Closes the session: a call that throws only on a Rust panic, which no test can make a real
    // session's close do, so a test's bridge throws its own. The destructor's close is always
    // this one, as a destructor calls no override.
    virtual void closeSession(subcheck_ffi::Session &session);

    // The actions sent to review, which review's count reaches once it has taken them all. Here, so
    // that a test that holds actions back from review, to stand in for its answers, counts them.
    uint64_t asked = 0;

private:
    // What the pump hands on, which it holds with the bridge: a pump let go of, after a close that
    // threw, reaches nothing of a bridge that has let go of it. `end` is the stream's last event,
    // when it is an end: close's own, or review's should it come first.
    struct Relay {
        std::mutex lock;
        SessionBridge *to = nullptr;
        std::optional<subcheck_ffi::Event::kEnded> end;
    };

    // What an action does to a greyed episode's view once review has taken it: at the first change
    // or answer alone (an `Event::Answered`) whose count reaches `at`, the episode at `path`
    // shows, or with none the view ends.
    struct ViewChange {
        uint64_t at;
        std::optional<std::string> path;
    };

    // What a greyed click puts aside until review answers it: the count of the click whose view
    // shows, and the change then awaited, both put back should review take the click while it
    // asks; and whether a change meanwhile that answers an action sent after that view's click
    // selected another finding, which would have ended the view but for the click.
    struct BeforeClick {
        uint64_t lastClick;
        std::optional<ViewChange> viewChange;
        bool moved = false;
    };

    // A greyed episode's view: the episode, and what the check and apply said of it as review last
    // sent them for its file. Held, so that the view still shows them in a change whose file is
    // already another's, as review's answer to the first of a click's two actions has it.
    struct QuietView {
        std::string path;
        subcheck_ffi::FileLogs logs;
    };

    // What review's answer did to a greyed episode's view: whether it changed what the window
    // shows, and whether a click's episode now shows, to be scrolled to.
    struct Settled {
        bool changed;
        bool shown;
    };

    void apply(const subcheck_ffi::Event &event);
    void changed(const subcheck_ffi::Event::kChanged &change);
    // Review's answer alone, to an action that changed nothing it shows: nothing is said or drawn
    // again, but for what the answer does to a greyed episode's view.
    void answered(uint64_t count);
    // Brings a greyed episode's view up to review's summary, whose count says which actions review
    // has taken: `moved`, whether its change selected another entry; `asking`, whether review
    // asked as it took the action the change answers.
    Settled settleView(bool moved, bool asking);
    // Shows the episode at `path`, or with none ends the view, as review's answer to the action
    // that names it has it: the episode only while review has `file`, its file, selected. Whether a
    // click's episode now shows.
    bool showView(const std::optional<std::string> &path, const std::optional<std::string> &file);
    // Marks in the sidebar the episode the details panel shows.
    void markEpisode();
    // Whether a greyed episode's view shows: one is held, its episode is still in the list, and
    // review has that episode's file selected, or any file while an answer that changes the view is
    // awaited, as between the two answers to a click on an episode with findings. Then the details
    // panel shows what the check said of it.
    bool showsQuietEpisode() const;
    // The path of review's selected file; none while it has none.
    std::optional<std::string> selectedFile() const;
    // Ends what a click on an episode with no findings shows, with review's answer to the action
    // just sent, for each ask showingEpisode names: said before it, the finding review still had
    // selected would show first, for as long as review took to answer. It takes the place of a
    // click still to be answered, which the ask came after. Whatever the bridge last heard of a
    // question: under one, review only lets it go and keeps the finding selected, which then shows.
    void leaveQuietEpisode();
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
    // Review's episodes, in its order, which every index it sends counts in; and the sidebar's list
    // of them.
    EpisodesModel files;
    EpisodeOrder order{&files};
    DetailView details;
    DetailView example;
    std::optional<subcheck_ffi::FileLogs> logs;
    std::shared_ptr<subcheck_ffi::Session> session;
    std::shared_ptr<Relay> relay;
    std::unique_ptr<QThread> pump;
    QTimer quitWait;
    // An Open quicker than the first moment shows no animation; two back to back, the next started
    // before the hide moment after the first is up, show as one.
    LoadingFlag loading{std::chrono::milliseconds(150), std::chrono::milliseconds(100)};
    subcheck_ffi::Summary summary{};
    std::optional<subcheck_ffi::EntryKey> selectedEntry;
    // The greyed episode's view that shows, until it ends, as showingEpisode says.
    std::optional<QuietView> quiet;
    // The last action's change to that view, until review has taken it: one at a time, as the last
    // action that changes the view decides it.
    std::optional<ViewChange> viewChange;
    // The count at the last click on an episode with no findings: a change that answers an action
    // sent before it can select another finding without ending that click's view.
    uint64_t lastClick = 0;
    // What the last greyed click put aside, until review answers it.
    std::optional<BeforeClick> beforeClick;
    bool opened = false;
    bool quitting = false;
    bool done = false;
    QString fatalText;
};
