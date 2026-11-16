// The generated bindings over a real session, on a run seeded afresh at each start of the test
// (tests/seeded.cmake) in the folder argv[1] names: two copies of typos.mkv, E01 and E02, so that
// each of its two rows has a member in each. The first event; of a change, the summary's filter,
// pane, edit, question and job, and the rows' note, checking and mixed; a decision, a dry run, a
// question, a re-check, the player's round (open, its reports, a seek, a report), another view
// that is not there, the end; then a session closed, a run that is not there, and the process's
// finish. Every call goes through the generated code, whose first call checks that it was
// generated from this library. Three sessions in one process, where a UI opens one: none stops a
// job, so none ends the groups, which is what makes that the rule.
#include "subcheck_ffi.hpp"

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <exception>
#include <functional>
#include <iostream>
#include <memory>
#include <optional>
#include <string>
#include <utime.h>
#include <variant>
#include <vector>

using namespace subcheck_ffi;

namespace {

int failed = 0;

void check(bool ok, const std::string &what) {
    if (!ok) {
        std::cerr << "FAIL " << what << "\n";
        ++failed;
    }
}

template <typename V, typename E> const V *as(const E &e) {
    return std::get_if<V>(&e.get_variant());
}

// What came, for a failure's message. An end's fatal is why the session ended, and for a session
// that failed to open it is all there is to go on.
std::string came(const std::optional<Event> &e) {
    if (!e) {
        return "nothing";
    }
    if (const Event::kEnded *end = as<Event::kEnded>(*e)) {
        return "the end, fatal: " + end->fatal.value_or("none");
    }
    return as<Event::kChanged>(*e) ? "a change" : "a player command";
}

// What a UI keeps of a session: the rows, each splice applied as it comes, the last summary, and
// the player commands it was given.
struct Ui {
    Session &s;
    std::vector<EntryView> rows;
    std::optional<Summary> summary;
    std::vector<PlayerCommand> played;

    // Takes events until `done` holds, after `what`; none may be the end.
    void until(const std::string &what, const std::function<bool(const Ui &)> &done) {
        while (!done(*this)) {
            std::optional<Event> e = s.next_event();
            if (const Event::kChanged *c = e ? as<Event::kChanged>(*e) : nullptr) {
                if (c->rows) {
                    apply(*c->rows, what);
                }
                summary = c->summary;
            } else if (const Event::kPlayer *p = e ? as<Event::kPlayer>(*e) : nullptr) {
                played.push_back(p->v1);
            } else {
                std::cerr << "FAIL never came, after " << what << "; came: " << came(e) << "\n";
                std::exit(1);
            }
        }
    }

    // The splice's rows in place of those it removes, as a UI applies it.
    void apply(const Splice &splice, const std::string &what) {
        if (splice.at > rows.size() || splice.removed > rows.size() - splice.at) {
            std::cerr << "FAIL a splice that does not fit, after " << what << "\n";
            std::exit(1);
        }
        const auto at = static_cast<std::ptrdiff_t>(splice.at);
        rows.erase(rows.begin() + at,
                   rows.begin() + at + static_cast<std::ptrdiff_t>(splice.removed));
        rows.insert(rows.begin() + at, splice.inserted.begin(), splice.inserted.end());
    }

    // The next event, which must be a change: its summary.
    const Summary &next(const std::string &what) {
        summary.reset();
        until(what, [](const Ui &ui) { return ui.summary.has_value(); });
        return *summary;
    }

    template <typename V> const V *job() const {
        return summary && summary->job ? as<V>(*summary->job) : nullptr;
    }
};

void a_run_opens_and_each_part_of_a_change_crosses(const std::string &folder) {
    // E01's mtime set to 1970-01-02, not what its scan recorded, so its findings' note says the
    // file changed.
    const std::string e01 = folder + "/typos - E01.mkv";
    const utimbuf long_ago{86400, 86400};
    check(utime(e01.c_str(), &long_ago) == 0, "E01's time set back");
    std::shared_ptr<Session> s = Session::open({folder}, false);
    Ui ui{*s, {}, {}, {}};

    const Summary first = ui.next("opening");
    check(ui.rows.size() == 2, "the first splice is the run's two rows");
    if (ui.rows.size() == 2) {
        const EntryView &row = ui.rows[0];
        const EntryKey::kRow *key = as<EntryKey::kRow>(row.key);
        check(key && key->v1 == std::vector<uint64_t>{1, 3}, "the first row is findings 1 and 3");
        check(row.state == State::kPending && !row.is_member && row.total == 2, "a pending row");
        check(row.at_ms == 1000, "at 0:00:01");
        check(row.original == "We should leave befor it rains.", "its line");
        check(row.wording == "We should leave before it rains.", "its fix");
        check(row.note == FileNote::kChanged && ui.rows[1].note == FileNote::kChanged,
              "E01's note, on both rows E01 is the first member of");
        check(!row.checking && !row.mixed, "nothing checked again, and one decision");
    }
    check(first.selected == 0u && first.pending == 4 && first.accepted == 0, "four pending");
    check(first.filter == Filter::kAll && first.pane == Pane::kFindings, "all, in the findings");
    check(!first.edit && !first.question && !first.job, "no edit, question or job");

    // A member decided: its row's members differ, and the row is still to do.
    s->send(Action(Action::kToggle{}));
    ui.until("expanding", [](const Ui &ui) { return ui.rows.size() == 4; });
    s->send(Action(Action::kMove{1}));
    ui.until("moving", [](const Ui &ui) { return ui.summary->selected == 1u; });
    s->send(Action(Action::kAccept{}));
    ui.until("accepting", [](const Ui &ui) { return ui.summary->accepted == 1; });
    check(ui.rows[0].mixed && ui.rows[0].state == State::kPending, "the row's members differ");
    check(ui.rows[1].state == State::kAccepted && !ui.rows[1].mixed, "the member accepted");

    s->send(Action(Action::kSetFilter{Filter::kAccepted}));
    check(ui.next("filtering").filter == Filter::kAccepted, "the filter crosses");
    s->send(Action(Action::kSetFilter{Filter::kAll}));
    ui.until("filtering", [](const Ui &ui) { return ui.summary->filter == Filter::kAll; });

    s->send(Action(Action::kEdit{}));
    ui.until("editing", [](const Ui &ui) { return ui.summary->edit.has_value(); });
    // On the first row, where the filter kept the selection: E02's finding, still pending.
    const EntryKey::kRow *edited = as<EntryKey::kRow>(ui.summary->edit->entry);
    check(ui.summary->selected == 0u && edited && edited->v1 == std::vector<uint64_t>{1, 3} &&
              ui.summary->edit->wording == "We should leave before it rains.",
          "the edit's entry and the wording it starts from");
    s->send(Action(Action::kCancelEdit{}));
    ui.until("cancelling", [](const Ui &ui) { return !ui.summary->edit; });

    // A dry run of the one accepted fix, to its end.
    s->send(Action(Action::kApply{true}));
    ui.until("a dry run", [](const Ui &ui) { return ui.job<Job::kApplyDone>() != nullptr; });
    const Job::kApplyDone dry = *ui.job<Job::kApplyDone>();
    check(dry.dry_run && dry.summary.fixed == 1 && dry.summary.failed == 0,
          "the dry run would make the one fix");
    // Apply asks first; a no lets the question go.
    s->send(Action(Action::kApply{false}));
    ui.until("applying", [](const Ui &ui) { return ui.summary->question.has_value(); });
    const Question::kApply *asked = as<Question::kApply>(*ui.summary->question);
    check(asked && asked->fixes == 1 && asked->files == 1 && asked->applied == 0,
          "apply asks, saying what it would apply");
    s->send(Action(Action::kAnswer{false}));
    ui.until("a no", [](const Ui &ui) { return !ui.summary->question; });

    // E02 checked again: its question, the scan on it, its findings being checked, and its end.
    const std::string e02 = folder + "/typos - E02.mkv";
    s->send(Action(Action::kShowPane{Pane::kFiles}));
    ui.until("the files", [](const Ui &ui) { return ui.summary->pane == Pane::kFiles; });
    s->send(Action(Action::kSelectFile{e02}));
    s->send(Action(Action::kRecheck{}));
    ui.until("re-checking", [](const Ui &ui) { return ui.summary->question.has_value(); });
    const Question::kRecheck *again = as<Question::kRecheck>(*ui.summary->question);
    check(again && again->label == "E02" && again->backend.ends_with("fake-llm.sh'"),
          "the check-again question names the file and the run's LLM");
    s->send(Action(Action::kAnswer{true}));
    ui.until("the scan", [](const Ui &ui) {
        const Job::kScanning *scan = ui.job<Job::kScanning>();
        return scan && !scan->under_way.empty();
    });
    const Job::kScanning scan = *ui.job<Job::kScanning>();
    check(scan.adding && scan.queued == 1 && scan.under_way[0].file == "E02",
          "the scan adds to the run, and is on E02");
    bool checking = false;
    for (const EntryView &row : ui.rows) {
        checking = checking || row.checking;
    }
    check(checking, "E02's findings are being checked again");
    ui.until("the scan's end", [](const Ui &ui) { return ui.job<Job::kScanDone>() != nullptr; });
    const Job::kScanDone scanned = *ui.job<Job::kScanDone>();
    check(scanned.adding && scanned.summary.files == 1 && scanned.summary.findings == 2 &&
              scanned.summary.paths.size() == 1,
          "what the scan came to");
    // A plain command's LLM, as the seed's, reports no use, so there is no cost to read: the
    // summary's f64 still crosses, with its paths after it in the record.
    const Spent &spent = scanned.summary.spent;
    check(spent.unreported == spent.calls && spent.priced == 0 && spent.cost_usd == 0.0,
          "what its LLM calls used");

    // The player's round, from the first row.
    s->send(Action(Action::kShowPane{Pane::kFindings}));
    s->send(Action(Action::kMove{-9}));
    ui.until("the findings", [](const Ui &ui) {
        return ui.summary->pane == Pane::kFindings && ui.summary->selected == 0u;
    });
    s->send(Action(Action::kPlay{}));
    ui.until("playing", [](const Ui &ui) { return !ui.played.empty(); });
    check(ui.summary->status == "opening E01 at 0:00:01.000", "the status line says what opens");
    const PlayerCommand::kOpen *o = as<PlayerCommand::kOpen>(ui.played[0]);
    check(o && o->moment.file == e01 && o->moment.sid == 1u,
          "play opens the row's first file on its subtitle track");
    check(o && o->moment.start_us == 1'004'500, "on the first frame of the selected row's line");
    check(o && o->how == How::kLoad, "loaded, as the player has loaded nothing for review yet");
    check(o && o->options.size() == 2 && o->options[0].name == "hr-seek" &&
              o->options[0].value == "yes" && o->options[1].name == "rebase-start-time" &&
              o->options[1].value == "no",
          "with the options review gives every load, in their order");

    s->report(PlayerReport(PlayerReport::kOpened{false}));
    check(ui.next("opened").status == "opened in mpv, playing",
          "the report reaches review, with where mpv was left");
    s->report(PlayerReport(PlayerReport::kFailed{PlayerError(PlayerError::kCouldNotOpen{e01})}));
    check(ui.next("failed").status == "mpv could not open E01",
          "a late failure reaches review, which words it");

    s->send(Action(Action::kPlayback{Playback(Playback::kSeek{5})}));
    ui.until("seeking", [](const Ui &ui) { return ui.played.size() == 2; });
    const PlayerCommand::kPlayback *pb = as<PlayerCommand::kPlayback>(ui.played[1]);
    const Playback::kSeek *sk = pb ? as<Playback::kSeek>(pb->v1) : nullptr;
    check(sk && sk->v1 == 5, "a seek of 5 s goes to the player");
    check(!ui.summary->status, "a seek clears the status line");

    s->report(PlayerReport(PlayerReport::kPlayed{PlayState{true, 6000}}));
    check(ui.next("played").status == "⏸ 0:00:06.000", "where it played, paused");
    s->report(PlayerReport(PlayerReport::kPlayFailed{PlayerError(PlayerError::kNoAnswer{})}));
    check(ui.next("no answer").status == "mpv did not answer", "a typed failure");

    s->send(Action(Action::kOpenView{{"no-such-run"}}));
    check(!ui.next("opening a view").status, "a view asked for clears the status line");
    check(ui.next("a view not there").status == "could not switch views: no run named no-such-run",
          "a view that is not there leaves the run shown, and says why");

    s->send(Action(Action::kQuit{}));
    std::optional<Event> e;
    while ((e = s->next_event()) && !as<Event::kEnded>(*e)) {
    }
    const Event::kEnded *end = e ? as<Event::kEnded>(*e) : nullptr;
    check(end && !end->fatal, "quitting ends the session, with nothing fatal; came: " + came(e));
    check(!s->next_event(), "nothing after the end");
}

// What was queued before close() may still come out, then close()'s end, which has nothing to
// say of a job, as none ran, and then nothing, at once.
void a_closed_session_ends_its_stream(const std::string &folder) {
    std::shared_ptr<Session> s = Session::open({folder}, false);
    s->close();
    std::optional<Event> last;
    while (std::optional<Event> e = s->next_event()) {
        last = e;
    }
    const Event::kEnded *end = last ? as<Event::kEnded>(*last) : nullptr;
    check(end && !end->fatal && end->outro.empty(), "close ends the stream; came: " + came(last));
    check(!s->next_event(), "nothing after close");
}

void a_run_that_is_not_there_ends_at_once() {
    std::shared_ptr<Session> s = Session::open({"no-such-run"}, false);
    std::optional<Event> e = s->next_event();
    const Event::kEnded *end = e ? as<Event::kEnded>(*e) : nullptr;
    check(end && end->fatal == "no run named no-such-run",
          "the first event says why; came: " + came(e));
}

} // namespace

int main(int argc, char **argv) {
    // It decides findings in whatever store it opens: run by hand without CTest's environment,
    // that would be the user's own.
    if (argc != 2 || !std::getenv("SUBCHECK_DB")) {
        std::cerr << "usage: SUBCHECK_DB=<a store of its own> bindings_smoke <folder of the seeded "
                     "run>\n";
        return 2;
    }
    try {
        a_run_opens_and_each_part_of_a_change_crosses(argv[1]);
        a_closed_session_ends_its_stream(argv[1]);
        a_run_that_is_not_there_ends_at_once();
        // No signal is ending this process, so it returns at once.
        finish();
    } catch (const std::exception &e) {
        std::cerr << "FAIL threw: " << e.what() << "\n";
        return 1;
    }
    return failed == 0 ? 0 : 1;
}
