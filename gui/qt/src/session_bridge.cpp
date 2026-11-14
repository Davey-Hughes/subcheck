#include "session_bridge.h"

#include "overloaded.h"

#include <chrono>
#include <exception>
#include <utility>
#include <variant>

using namespace subcheck_ffi;

namespace {

// What an error that gives no reason says. A failure is known by its text alone: the dialog, the
// one way out of a failed review, shows only with text, and quit and dismiss look for it.
const char *const noReason = "review ended on an error that gave no reason";

} // namespace

SessionBridge::SessionBridge(std::vector<std::string> selector, PlayerPort &player,
                             std::ostream &out, std::ostream &err, std::chrono::milliseconds wait,
                             QObject *parent)
    : QObject(parent), selector(std::move(selector)), port(player), out(out), err(err) {
    connect(&port, &PlayerPort::reported, this, &SessionBridge::report);
    // Queued: each event the pump reads is applied on this object's thread, in the pump's order.
    connect(this, &SessionBridge::eventArrived, this, &SessionBridge::apply, Qt::QueuedConnection);
    // Precise, as a coarse timer may fire early, before review has had the whole wait.
    quitWait.setTimerType(Qt::PreciseTimer);
    quitWait.setSingleShot(true);
    quitWait.setInterval(wait);
    connect(&quitWait, &QTimer::timeout, this, &SessionBridge::giveUp);
}

SessionBridge::~SessionBridge() { letGo(); }

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
    guarded([this] { session = Session::open(selector); });
    if (!session) {
        return;
    }
    // The pump reads a session it holds itself, never the bridge's, which letGo lets go of. A
    // throw is a Rust panic, which ends review as a fatal end does.
    pump.reset(QThread::create([this, session = session] {
        try {
            while (std::optional<Event> e = session->next_event()) {
                emit eventArrived(*e);
            }
        } catch (const std::exception &e) {
            emit eventArrived(Event(Event::kEnded{std::string(e.what()), {}}));
        }
    }));
    pump->start();
}

FindingsModel *SessionBridge::findings() { return &list; }

PlayerPort *SessionBridge::player() { return &port; }

QString SessionBridge::title() const {
    return opened ? QString::fromStdString(summary.title) : QStringLiteral("subcheck");
}

QString SessionBridge::statusLine() const {
    // Until the end comes, which can take the whole wait, so that the window says why it stays.
    if (quitting) {
        return QStringLiteral("quitting: waiting for review to end");
    }
    if (!opened) {
        return QStringLiteral("opening…");
    }
    if (summary.status) {
        return QString::fromStdString(*summary.status);
    }
    return QStringLiteral("%1 pending · %2 accepted · %3 rejected")
        .arg(summary.pending)
        .arg(summary.accepted)
        .arg(summary.rejected);
}

int SessionBridge::selected() const {
    return summary.selected ? static_cast<int>(*summary.selected) : -1;
}

bool SessionBridge::empty() const { return opened && list.rowCount() == 0; }

QString SessionBridge::fatal() const { return fatalText; }

bool SessionBridge::key(int key, const QString &text) {
    if (key == Qt::Key_Down || text == u"j") {
        move(1);
    } else if (key == Qt::Key_Up || text == u"k") {
        move(-1);
    } else if (key == Qt::Key_Return || key == Qt::Key_Enter) {
        toggle();
    } else if (text == u"a") {
        accept();
    } else if (text == u"r") {
        reject();
    } else if (text == u"u") {
        undo();
    } else if (text == u"o") {
        play();
    } else if (text == u"O") {
        closePlayer();
    } else if (key == Qt::Key_Space) {
        playback(Playback(Playback::kPause{}));
    } else if (key == Qt::Key_Left) {
        playback(Playback(Playback::kSeek{-5}));
    } else if (key == Qt::Key_Right) {
        playback(Playback(Playback::kSeek{5}));
    } else if (text == u",") {
        playback(Playback(Playback::kSeek{-1}));
    } else if (text == u".") {
        playback(Playback(Playback::kSeek{1}));
    } else if (text == u"[") {
        playback(Playback(Playback::kPreviousFrame{}));
    } else if (text == u"]") {
        playback(Playback(Playback::kNextFrame{}));
    } else if (text == u"q") {
        quit();
    } else {
        return false;
    }
    return true;
}

void SessionBridge::select(int row) {
    // The core owns the selection: the list's moves wait for the summary that follows.
    if (std::optional<EntryKey> key = list.keyAt(row)) {
        send(Action(Action::kSelect{*key}));
    }
}

void SessionBridge::move(int by) {
    // Review works out where a move lands, from its own selection: a key pressed before it has
    // answered the one before still moves on from where that one landed.
    send(Action(Action::kMove{by}));
}

void SessionBridge::toggle() { send(Action(Action::kToggle{})); }

void SessionBridge::accept() { send(Action(Action::kAccept{})); }

void SessionBridge::reject() { send(Action(Action::kReject{})); }

void SessionBridge::undo() { send(Action(Action::kUndo{})); }

void SessionBridge::play() { send(Action(Action::kPlay{})); }

void SessionBridge::closePlayer() { send(Action(Action::kClosePlayer{})); }

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
    send(Action(Action::kQuit{}));
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

void SessionBridge::apply(const Event &event) {
    // What the pump read after the session was let go, as `close` warns of.
    if (!session) {
        return;
    }
    std::visit(Overloaded{
                   [this](const Event::kChanged &change) { changed(change); },
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
    summary = change.summary;
    opened = true;
    emit summaryChanged();
}

void SessionBridge::ended(const Event::kEnded &end) {
    letGo();
    // As the terminal: what is left to say of the jobs on stderr; then the error, on stderr, or
    // the decisions' counts, on stdout.
    for (const std::string &line : end.outro) {
        err << line << '\n';
    }
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
    letGo();
    const std::string said = why.empty() ? std::string(noReason) : why;
    err << "subcheck-qt: " << said << '\n';
    err.flush();
    fatalText = QString::fromStdString(said);
    emit fatalChanged();
}

void SessionBridge::giveUp() {
    letGo();
    err << "subcheck-qt: review had not ended "
        << std::chrono::duration<double>(quitWait.intervalAsDuration()).count()
        << " s after quitting, so it was closed\n";
    err.flush();
    finish(1);
}

// On every way out: close, so that the pump's wait ends whatever the core thread is doing; join
// the pump; then let go. A pump left waiting would hold the session and its core thread for good.
void SessionBridge::letGo() {
    quitWait.stop();
    port.release();
    if (session) {
        try {
            session->close();
        } catch (const std::exception &) {
            // Nothing reaches this today: close only queues two messages, and takes no text, so
            // its generated call throws only on a Rust panic, or on a contract mismatch, which
            // open, the first call, would have met. Were it reached, the stream might never end,
            // so the pump is let go of, neither joined, which could wait for good, nor destroyed
            // running, which aborts; it would still hold this bridge, and emit what it reads on a
            // destroyed object.
            static_cast<void>(pump.release());
        }
    }
    if (pump) {
        pump->wait();
        pump.reset();
    }
    session.reset();
}

void SessionBridge::finish(int status) {
    if (!done) {
        done = true;
        emit finished(status);
    }
}
