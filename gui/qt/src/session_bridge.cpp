#include "session_bridge.h"

#include "overloaded.h"
#include "status_words.h"

#include <chrono>
#include <exception>
#include <utility>
#include <variant>

using namespace subcheck_ffi;

namespace {

// What an error that gives no reason says. A failure is known by its text alone: the dialog, the
// one way out of a failed review, shows only with text, and quit and dismiss look for it.
const char *const noReason = "review ended on an error that gave no reason";

// Whether two keys name one entry of the list: the same row, or the same member of a row.
bool sameEntry(const std::optional<EntryKey> &a, const std::optional<EntryKey> &b) {
    if (!a || !b) {
        return !a && !b;
    }
    const auto *rowA = std::get_if<EntryKey::kRow>(&a->get_variant());
    const auto *rowB = std::get_if<EntryKey::kRow>(&b->get_variant());
    if (rowA || rowB) {
        return rowA && rowB && rowA->v1 == rowB->v1;
    }
    return std::get<EntryKey::kMember>(a->get_variant()).v1 ==
           std::get<EntryKey::kMember>(b->get_variant()).v1;
}

// The Settings window's sample: a word taken out and a longer one put in, as a typo's fix is.
Detail sampleFinding() {
    return Detail{
        .file = {},
        .label = {},
        .group = std::nullopt,
        .at_ms = std::nullopt,
        .track = std::nullopt,
        .more = 0,
        .note = std::nullopt,
        .checking = std::nullopt,
        .truncated = std::nullopt,
        .state = State::kPending,
        .edited = false,
        .mark = std::nullopt,
        .original = "We should of left before it got dark.",
        .wording = "We should have left before it got dark.",
        .original_changed = Span{10, 12},
        .wording_changed = Span{10, 14},
        .reason = {},
        .kind = Kind(Kind::kLine{{}, {}}),
    };
}

} // namespace

SessionBridge::SessionBridge(std::vector<std::string> selector, PlayerPort &player,
                             std::ostream &out, std::ostream &err, std::chrono::milliseconds wait,
                             QObject *parent)
    : QObject(parent), selector(std::move(selector)), port(player), out(out), err(err) {
    connect(&port, &PlayerPort::reported, this, &SessionBridge::report);
    // The end's release ends any Open, and with it the animation.
    connect(&port, &PlayerPort::openingChanged, this, [this] { loading.set(port.opening()); });
    connect(&loading, &LoadingFlag::shownChanged, this, &SessionBridge::videoLoadingChanged);
    // Queued: each event the pump reads is applied on this object's thread, in the pump's order.
    connect(this, &SessionBridge::eventArrived, this, &SessionBridge::apply, Qt::QueuedConnection);
    // Precise, as a coarse timer may fire early, before review has had the whole wait.
    quitWait.setTimerType(Qt::PreciseTimer);
    quitWait.setSingleShot(true);
    quitWait.setInterval(wait);
    connect(&quitWait, &QTimer::timeout, this, &SessionBridge::giveUp);
    example.show(sampleFinding());
}

SessionBridge::~SessionBridge() {
    if (std::optional<Event::kEnded> end = letGo()) {
        say(*end);
    }
}

// Every call into the session can throw: a contract mismatch at the first, a Rust panic, or text
// that is not UTF-8. Nothing may throw through Qt's event loop, so a throw ends review as a fatal
// end does.
template <typename Call> void SessionBridge::guarded(Call call) {
    try {
        call();
    } catch (const std::exception &e) {
        fail(e.what());
    }
}

void SessionBridge::start() {
    // The window offers no apply yet, nor a way to ask for karaoke's re-spacing.
    guarded([this] { session = Session::open(selector, false); });
    if (!session) {
        return;
    }
    relay = std::make_shared<Relay>();
    relay->to = this;
    // The pump reads a session it holds itself, never the bridge's, which letGo lets go of, and
    // hands what it reads on through the relay. A throw is a Rust panic, which ends review as a
    // fatal end does.
    pump.reset(QThread::create([relay = relay, session = session] {
        const auto pass = [&relay](const Event &e) {
            const std::lock_guard<std::mutex> held(relay->lock);
            if (const auto *end = std::get_if<Event::kEnded>(&e.get_variant())) {
                relay->end = *end;
            }
            if (relay->to) {
                emit relay->to->eventArrived(e);
            }
        };
        try {
            while (std::optional<Event> e = session->next_event()) {
                pass(*e);
            }
        } catch (const std::exception &e) {
            pass(Event(Event::kEnded{std::string(e.what()), {}}));
        }
    }));
    pump->start();
}

FindingsModel *SessionBridge::findings() { return &list; }

FoldersModel *SessionBridge::folders() { return &reviews; }

FiltersModel *SessionBridge::filters() { return &shows; }

EpisodeOrder *SessionBridge::episodes() { return &order; }

DetailView *SessionBridge::detail() { return &details; }

DetailView *SessionBridge::sample() { return &example; }

PlayerPort *SessionBridge::player() { return &port; }

QString SessionBridge::title() const {
    return opened ? QString::fromStdString(summary.title) : QStringLiteral("subcheck");
}

QString SessionBridge::counts() const {
    if (!opened) {
        return {};
    }
    return QStringLiteral("%1 to review · %2 accepted · %3 rejected")
        .arg(summary.pending)
        .arg(summary.accepted)
        .arg(summary.rejected);
}

QString SessionBridge::heading() const { return opened ? shows.heading() : QString(); }

QString SessionBridge::switchLabel() const {
    return summary.of_paths ? QStringLiteral("Show Newest Check") : QStringLiteral("Show Folders");
}

QString SessionBridge::statusLine() const {
    // Until the end comes, which can take the whole wait, so that the window says why it stays.
    if (quitting) {
        return QStringLiteral("Quitting: waiting for review to end");
    }
    if (!opened) {
        return QStringLiteral("Opening…");
    }
    return summary.status ? statusWords(*summary.status) : QString();
}

int SessionBridge::selected() const {
    return summary.selected ? static_cast<int>(*summary.selected) : -1;
}

QString SessionBridge::emptyWords() const {
    if (!opened || list.rowCount() > 0) {
        return {};
    }
    // Review's counts are the whole view's, whatever the filter shows.
    return summary.pending + summary.accepted + summary.rejected == 0
               ? QStringLiteral("No findings in this run")
               : QStringLiteral("No findings under this filter");
}

QString SessionBridge::episodeName() const {
    // An episode with no findings, clicked, has no finding to be beside: its view shows what the
    // click asked for, until it ends as the header's showingEpisode says. Else review sends the
    // logs of the file selected, which stays where a click on an episode left it while the finding
    // selected moves on: beside a finding of another episode, they are not its. A row of several
    // is each of its episodes', and its members name them as the list does.
    if (showsQuietEpisode()) {
        return QString::fromStdString(*files.labelOf(quiet->path));
    }
    if (!logs || !summary.file_selected) {
        return {};
    }
    const std::optional<std::string> label =
        files.labelAt(static_cast<int>(*summary.file_selected));
    return label && details.isOf(*label) ? QString::fromStdString(*label) : QString();
}

QStringList SessionBridge::episodeLog() const {
    if (episodeName().isEmpty()) {
        return {};
    }
    const FileLogs &of = showsQuietEpisode() ? quiet->logs : *logs;
    QStringList said;
    for (const std::string &line : of.log) {
        said << QString::fromStdString(line);
    }
    if (!of.apply_log.empty()) {
        said << QString();
        for (const std::string &line : of.apply_log) {
            said << QString::fromStdString(line);
        }
    }
    return said;
}

bool SessionBridge::showingEpisode() const { return showsQuietEpisode(); }

QString SessionBridge::fatal() const { return fatalText; }

bool SessionBridge::videoLoading() const { return loading.shown(); }

void SessionBridge::select(int row) {
    // The core owns the selection: the list's moves wait for the summary that follows.
    if (std::optional<EntryKey> key = list.keyAt(row)) {
        send(Action(Action::kSelect{*key}));
        leaveQuietEpisode();
    }
}

void SessionBridge::move(int by) {
    // Review works out where a move lands, from its own selection: a move asked for before it has
    // answered the one before still moves on from where that one landed.
    send(Action(Action::kMove{by}));
    leaveQuietEpisode();
}

void SessionBridge::toggle() { send(Action(Action::kToggle{})); }

void SessionBridge::accept() { send(Action(Action::kAccept{})); }

void SessionBridge::reject() { send(Action(Action::kReject{})); }

void SessionBridge::clear() { send(Action(Action::kClear{})); }

void SessionBridge::undo() {
    send(Action(Action::kUndo{}));
    leaveQuietEpisode();
}

void SessionBridge::play() { send(Action(Action::kPlay{})); }

void SessionBridge::closePlayer() { send(Action(Action::kClosePlayer{})); }

void SessionBridge::pause() { playback(Playback(Playback::kPause{})); }

void SessionBridge::seek(int seconds) { playback(Playback(Playback::kSeek{seconds})); }

void SessionBridge::nextFrame() { playback(Playback(Playback::kNextFrame{})); }

void SessionBridge::previousFrame() { playback(Playback(Playback::kPreviousFrame{})); }

void SessionBridge::setFilter(int row) {
    if (std::optional<Filter> filter = shows.filterAt(row)) {
        send(Action(Action::kSetFilter{*filter}));
    }
}

void SessionBridge::cycleFilter() { send(Action(Action::kSetFilter{shows.next()})); }

void SessionBridge::nextEpisode() {
    send(Action(Action::kNextFile{}));
    leaveQuietEpisode();
}

void SessionBridge::previousEpisode() {
    send(Action(Action::kPreviousFile{}));
    leaveQuietEpisode();
}

void SessionBridge::switchView() { send(Action(Action::kSwitchView{})); }

void SessionBridge::openFolder(int row) {
    if (std::optional<std::string> path = reviews.pathAt(row)) {
        send(Action(Action::kOpenFolder{*path}));
    }
}

void SessionBridge::openEpisode(int row) {
    const std::optional<int> at = order.sourceRow(row);
    const std::optional<std::string> path = at ? files.pathAt(*at) : std::nullopt;
    if (!path) {
        return;
    }
    if (!files.hasNoFindingsAt(*at)) {
        send(Action(Action::kSelectFile{*path}));
        send(Action(Action::kShowFileFindings{}));
        leaveQuietEpisode();
        return;
    }
    send(Action(Action::kSelectFile{*path}));
    // Its view shows with review's answer to this click, and not before: a change of review's that
    // comes first answers an action sent before the click, and has the file selected before it.
    // The answer tells too whether review was asking as it took the click, whatever the bridge last
    // heard of a question: then it only let the question go, and the view stays as it was.
    beforeClick = BeforeClick{lastClick, viewChange};
    lastClick = asked;
    viewChange = ViewChange{asked, path};
}

void SessionBridge::playback(Playback playback) {
    send(Action(Action::kPlayback{std::move(playback)}));
}

void SessionBridge::quit() {
    // With the fatal end's dialog up, closing the window is closing the dialog.
    if (!fatalText.isEmpty()) {
        dismiss();
        return;
    }
    if (!session || quitting) {
        return;
    }
    quitting = true;
    emit summaryChanged();
    // Two `Interrupt`s: the first quits, or with a job running asks, and the second quits, stopping
    // the job. A `Quit` under a question would only let the question go, and the window would wait
    // out the give-up.
    send(Action(Action::kInterrupt{}));
    send(Action(Action::kInterrupt{}));
    // A send that threw has ended review as a fatal end, whose dialog waits for the person.
    if (session) {
        quitWait.start();
    }
}

void SessionBridge::dismiss() {
    if (!fatalText.isEmpty()) {
        finish(1);
    }
}

void SessionBridge::recolour() {
    list.recolour();
    emit details.changed();
    emit example.changed();
}

void SessionBridge::apply(const Event &event) {
    // What the pump read after the session was let go, as `close` warns of.
    if (!session) {
        return;
    }
    std::visit(Overloaded{
                   [this](const Event::kChanged &change) { changed(change); },
                   [this](const Event::kAnswered &answer) { answered(answer.answered); },
                   [this](const Event::kPlayer &player) { port.run(player.v1); },
                   [this](const Event::kEnded &end) { ended(end); },
               },
               event.get_variant());
}

void SessionBridge::changed(const Event::kChanged &change) {
    if (change.rows && !list.apply(*change.rows)) {
        fail("the list is out of step with review");
        return;
    }
    if (change.files && !files.apply(*change.files)) {
        fail("the episodes are out of step with review");
        return;
    }
    if (change.folders) {
        reviews.set(*change.folders);
    }
    if (change.detail) {
        details.show(change.detail->detail);
    }
    if (change.logs) {
        logs = change.logs->logs;
    }
    // Whether review was asking as it took the action this change answers: the change before says.
    const bool asking = summary.question.has_value();
    summary = change.summary;
    shows.show(summary);
    opened = true;
    const std::optional<EntryKey> entry =
        summary.selected ? list.keyAt(static_cast<int>(*summary.selected)) : std::nullopt;
    const bool moved = !sameEntry(selectedEntry, entry);
    selectedEntry = entry;
    const bool shown = settleView(moved, asking).shown;
    markEpisode();
    emit summaryChanged();
    if (moved) {
        emit selectionMoved();
    }
    if (shown) {
        emit episodeShown();
    }
}

void SessionBridge::answered(uint64_t count) {
    // The summary as last sent stands, with this count: review asked, or did not, as it said then.
    const bool asking = summary.question.has_value();
    summary.answered = count;
    const Settled view = settleView(false, asking);
    if (!view.changed) {
        return;
    }
    markEpisode();
    emit summaryChanged();
    if (view.shown) {
        emit episodeShown();
    }
}

SessionBridge::Settled SessionBridge::settleView(bool moved, bool asking) {
    // Review's answer to the last action that changes a greyed episode's view: the view as that
    // action leaves it, a click's episode shown and scrolled to, or none. A click's answer that
    // leaves another file selected shows none. Another finding selected ends the view too, but not
    // in a change that answers an action sent before the last greyed click: that click's view would
    // end before it had shown. A click review took while it asked only let the question go: what it
    // put aside is put back, and another finding selected meanwhile, in a change that would have
    // ended the view but for the click, ends it now. While the view shows, its logs follow review's
    // for its file, which a check can add to; once review has another file selected, with no answer
    // awaited that changes the view, it ends.
    const std::optional<std::string> file = selectedFile();
    const auto held = [this] {
        return quiet ? std::optional<std::string>(quiet->path) : std::nullopt;
    };
    const std::optional<std::string> before = held();
    bool shown = false;
    if (viewChange && summary.answered >= viewChange->at) {
        const std::optional<std::string> path = std::move(viewChange->path);
        viewChange.reset();
        const std::optional<BeforeClick> aside = std::exchange(beforeClick, std::nullopt);
        if (!path || !asking) {
            shown = showView(path, file);
        } else if (aside) {
            lastClick = aside->lastClick;
            viewChange = aside->viewChange;
            if (viewChange && summary.answered >= viewChange->at) {
                shown = showView(std::exchange(viewChange, std::nullopt)->path, file);
            }
            if (aside->moved) {
                quiet.reset();
                shown = false;
            }
        }
    } else if (moved && summary.answered >= lastClick) {
        quiet.reset();
    } else if (moved && beforeClick && summary.answered >= beforeClick->lastClick) {
        beforeClick->moved = true;
    }
    if (quiet && logs && file == quiet->path) {
        quiet->logs = *logs;
    }
    if (quiet && !viewChange && file != quiet->path) {
        quiet.reset();
    }
    return Settled{.changed = shown || before != held(), .shown = shown};
}

bool SessionBridge::showView(const std::optional<std::string> &path,
                             const std::optional<std::string> &file) {
    quiet.reset();
    if (path && logs && file == path) {
        quiet = QuietView{*path, *logs};
    }
    return showsQuietEpisode();
}

void SessionBridge::markEpisode() {
    // The greyed episode whose view shows; or the one last clicked while it is the selected
    // finding's or one of its row's, so that a click on an episode whose first finding is in a row
    // of several marks it; else the selected finding's own. Each is the episode the details panel
    // shows.
    std::optional<std::string> marked;
    if (showsQuietEpisode()) {
        marked = quiet->path;
    } else if (!episodeName().isEmpty()) {
        marked = selectedFile();
    }
    files.follow(marked ? *marked : details.file());
}

bool SessionBridge::showsQuietEpisode() const {
    return quiet && files.labelOf(quiet->path) && (viewChange || selectedFile() == quiet->path);
}

std::optional<std::string> SessionBridge::selectedFile() const {
    return summary.file_selected ? files.pathAt(static_cast<int>(*summary.file_selected))
                                 : std::nullopt;
}

void SessionBridge::leaveQuietEpisode() {
    viewChange = ViewChange{asked, std::nullopt};
    beforeClick.reset();
}

void SessionBridge::ended(const Event::kEnded &end) {
    letGo();
    // As the terminal: what is left to say of the jobs on stderr; then the error, on stderr, or
    // the decisions' counts, on stdout.
    say(end);
    if (end.fatal) {
        fail(*end.fatal);
        return;
    }
    out << summary.title << ": " << summary.accepted << " accepted, " << summary.rejected
        << " rejected, " << summary.pending << " pending\n";
    out.flush();
    finish(0);
}

void SessionBridge::send(Action action) {
    if (session) {
        ++asked;
        guarded([&] { session->send(std::move(action)); });
    }
}

// The player reports in its own time: after review has ended, to no one.
void SessionBridge::report(PlayerReport report) {
    if (session) {
        guarded([&] { session->report(std::move(report)); });
    }
}

void SessionBridge::fail(const std::string &why) {
    if (std::optional<Event::kEnded> end = letGo()) {
        say(*end);
    }
    const std::string said = why.empty() ? std::string(noReason) : why;
    err << "subcheck-qt: " << said << '\n';
    err.flush();
    fatalText = QString::fromStdString(said);
    emit fatalChanged();
}

void SessionBridge::giveUp() {
    if (std::optional<Event::kEnded> end = letGo()) {
        say(*end);
    }
    err << "subcheck-qt: review had not ended "
        << std::chrono::duration<double>(quitWait.intervalAsDuration()).count()
        << " s after quitting, so it was closed\n";
    err.flush();
    finish(1);
}

void SessionBridge::closeSession(Session &session) { session.close(); }

// On every way out: close, so that the pump's wait ends whatever the core thread is doing; join
// the pump; then let go. A pump left waiting would hold the session and its core thread for good.
// Returns the stream's last end, which the pump keeps: close's own, whose outro says how to finish
// a job the close stopped, or review's, should it have come first. None with no session, or after
// a close that threw.
std::optional<Event::kEnded> SessionBridge::letGo() {
    quitWait.stop();
    port.release();
    if (!session) {
        return std::nullopt;
    }
    std::optional<Event::kEnded> end;
    try {
        closeSession(*session);
        if (pump) {
            pump->wait();
            pump.reset();
        }
        end = std::move(relay->end);
    } catch (const std::exception &) {
        // Close's generated call throws only on a Rust panic, or on a contract mismatch, which
        // open, the first call, would have met. The stream might then never end, so the pump is
        // let go of, neither joined, which could wait for good, nor destroyed running, which
        // aborts; and the relay no longer hands it on to this bridge, which may be gone by then.
        {
            const std::lock_guard<std::mutex> held(relay->lock);
            relay->to = nullptr;
        }
        static_cast<void>(pump.release());
    }
    session.reset();
    return end;
}

// An end's outro, on stderr, as the terminal says it: what the jobs came to, and how to finish one
// the end stopped.
void SessionBridge::say(const Event::kEnded &end) {
    for (const std::string &line : end.outro) {
        err << line << '\n';
    }
    err.flush();
}

void SessionBridge::finish(int status) {
    if (!done) {
        done = true;
        emit finished(status);
    }
}
