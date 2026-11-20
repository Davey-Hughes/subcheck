// The generated bindings over a real session, on a run seeded afresh at each start of the test
// (tests/seeded.cmake) in the folder argv[1] names: two copies of typos.mkv, E01 and E02, so that
// each of its two rows has a member in each. The first event; of a change, the summary's filter,
// pane, edit, question, job, LLM costs and kind of view, the rows' note, checking, mixed, check
// and marked change, and the show folders; a decision, a dry run, a question, a re-check, the
// player's round (open, its reports, a seek, a report), another view that is not there, the run's
// view and the folder's again, the end; then a session closed, a run that is not there, and the
// process's finish. Every call goes through the generated code, whose first call checks that it was
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

// The summary's message as `V`, or none when it has none or another.
template <typename V> const V *message(const Summary &summary) {
    return summary.status ? as<V>(*summary.status) : nullptr;
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
    if (as<Event::kAnswered>(*e)) {
        return "an answer alone";
    }
    return as<Event::kChanged>(*e) ? "a change" : "a player command";
}

// What a UI keeps of a session: each list, each splice applied as it comes, the last summary, the
// detail, the logs and the folders as last sent, and the player commands it was given.
struct Ui {
    Session &s;
    std::vector<EntryView> rows{};
    std::vector<FileView> files{};
    std::optional<Summary> summary{};
    std::optional<Detail> detail{};
    std::optional<FileLogs> logs{};
    std::vector<FolderView> folders{};
    std::vector<PlayerCommand> played{};

    // Takes events until `done` holds, after `what`; none may be the end.
    void until(const std::string &what, const std::function<bool(const Ui &)> &done) {
        while (!done(*this)) {
            std::optional<Event> e = s.next_event();
            if (const Event::kChanged *c = e ? as<Event::kChanged>(*e) : nullptr) {
                if (c->rows) {
                    apply(rows, c->rows->at, c->rows->removed, c->rows->inserted, what);
                }
                if (c->files) {
                    apply(files, c->files->at, c->files->removed, c->files->inserted, what);
                }
                // Absent when unchanged; present, it holds what is so now, none included.
                if (c->detail) {
                    detail = c->detail->detail;
                }
                if (c->logs) {
                    logs = c->logs->logs;
                }
                if (c->folders) {
                    folders = *c->folders;
                }
                summary = c->summary;
            } else if (const Event::kAnswered *a = e ? as<Event::kAnswered>(*e) : nullptr) {
                // An action's answer alone: the summary as last sent, with this count.
                if (summary) {
                    summary->answered = a->answered;
                }
            } else if (const Event::kPlayer *p = e ? as<Event::kPlayer>(*e) : nullptr) {
                played.push_back(p->v1);
            } else {
                std::cerr << "FAIL never came, after " << what << "; came: " << came(e) << "\n";
                std::exit(1);
            }
        }
    }

    // A splice's items in place of those it removes from `list`, as a UI applies it.
    template <typename T>
    static void apply(std::vector<T> &list, uint64_t at, uint64_t removed,
                      const std::vector<T> &inserted, const std::string &what) {
        if (at > list.size() || removed > list.size() - at) {
            std::cerr << "FAIL a splice that does not fit, after " << what << "\n";
            std::exit(1);
        }
        const auto from = list.begin() + static_cast<std::ptrdiff_t>(at);
        list.erase(from, from + static_cast<std::ptrdiff_t>(removed));
        list.insert(list.begin() + static_cast<std::ptrdiff_t>(at), inserted.begin(),
                    inserted.end());
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
    Ui ui{*s};

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
        check(row.check == Check::kLlm && row.original_changed.start == 16u &&
                  row.original_changed.end == 21u && row.wording_changed.start == 16u &&
                  row.wording_changed.end == 22u,
              "the LLM's finding, befor and before marked on its row");
    }
    // The store's one show folder, the seeded run's, which the view is of.
    check(ui.folders.size() == 1, "one show folder");
    if (ui.folders.size() == 1) {
        const FolderView &media = ui.folders[0];
        check(media.path == folder && media.name == "media" && media.to_review == 4 && media.open,
              "the run's folder, named as its view is, four to review, open");
    }
    // The seed's LLM, a plain command, records no calls, so the view's costs are all zero: this
    // shows the field crosses, not its values, which the FFI's own conversion test pins.
    check(first.spent.calls == 0 && first.spent.priced == 0 && first.spent.cost_usd == 0.0,
          "the view's LLM calls, none recorded");
    check(first.of_paths, "a folder's view is of paths");
    check(first.selected == 0u && first.pending == 4 && first.accepted == 0, "four pending");
    // The files list, its first file selected, and that file's log; the selected row's detail.
    check(ui.files.size() == 2 && first.file_selected == 0u, "the run's two files, E01 selected");
    if (ui.files.size() == 2) {
        const FileView &file = ui.files[1];
        const ScanStage::kDone *done = as<ScanStage::kDone>(file.stage);
        check(file.path == folder + "/typos - E02.mkv" && file.label == "E02", "E02's line");
        check(done && done->findings == 2 && file.lines > 0u && !file.timing_only && !file.applying,
              "E02 checked, with its findings and lines");
    }
    check(ui.logs && !ui.logs->log.empty() && ui.logs->apply_log.empty(), "E01's log");
    check(ui.detail && ui.detail->label == "E01" && ui.detail->state == State::kPending,
          "the first row's detail");
    if (ui.detail) {
        const Detail &d = *ui.detail;
        check(d.group && d.group->members.size() == 2 && d.group->members[1].label == "E02" &&
                  d.group->hidden == 0,
              "a row of two, none hidden");
        check(d.track && d.track->sid == 1u && d.at_ms == 1000 && !d.mark, "its line, on track 1");
        check(d.original_changed.start == 16u && d.original_changed.end == 21u &&
                  d.wording_changed.end == 22u,
              "befor, before");
        const Kind::kLine *line = as<Kind::kLine>(d.kind);
        check(line && !line->after.empty(), "the lines after it");
        check(d.file == e01 && d.original == "We should leave befor it rains." &&
                  d.wording == "We should leave before it rains." && !d.reason.empty() &&
                  !d.edited && d.more == 0,
              "its file, its line, its fix and why");
        check(d.note == FileNote::kChanged && !d.checking && !d.truncated,
              "its file changed since its scan, not being checked, read whole");
        check(d.track && d.track->id == 0 && d.track->name == "English Full" &&
                  d.track->language == "en" && d.group && !d.group->members.empty() &&
                  !d.group->members[0].fix,
              "its track as the file's record has it, and no fix for its members");
    }
    check(first.filter == Filter::kAll && first.pane == Pane::kFindings, "all, in the findings");
    check(!first.edit && !first.question && !first.job, "no edit, question or job");

    // A member decided: its row's members differ, and the row is still to do.
    s->send(Action(Action::kToggle{}));
    ui.until("expanding", [](const Ui &ui) { return ui.rows.size() == 4; });
    s->send(Action(Action::kMove{1}));
    ui.until("moving", [](const Ui &ui) { return ui.summary->selected == 1u; });
    s->send(Action(Action::kAccept{}));
    ui.until("accepting", [](const Ui &ui) { return ui.summary->accepted == 1; });
    check(!ui.rows.empty() && ui.rows[0].mixed && ui.rows[0].state == State::kPending,
          "the row's members differ");
    check(ui.rows.size() > 1 && ui.rows[1].state == State::kAccepted && !ui.rows[1].mixed,
          "the member accepted");
    const Status::kDecided *decided = message<Status::kDecided>(*ui.summary);
    check(decided && decided->state == State::kAccepted && decided->findings == 1 &&
              decided->replaced == 0 && !decided->kept,
          "the decision's message: one finding accepted, no wording replaced, no member kept");
    check(ui.folders.size() == 1 && ui.folders[0].to_review == 3,
          "the folder read again once the decision is written: three to review");

    s->send(Action(Action::kSetFilter{Filter::kAccepted}));
    check(ui.next("filtering").filter == Filter::kAccepted, "the filter crosses");
    s->send(Action(Action::kSetFilter{Filter::kAll}));
    ui.until("filtering", [](const Ui &ui) { return ui.summary->filter == Filter::kAll; });
    // Nothing is rejected, so no row shows: the detail crosses as none now, in the filter's own
    // change, and the logs stay as they were, as the selected file is the same.
    const std::optional<FileLogs> kept = ui.logs;
    s->send(Action(Action::kSetFilter{Filter::kRejected}));
    check(ui.next("filtering").filter == Filter::kRejected && !ui.detail && ui.logs && kept &&
              ui.logs->log == kept->log && ui.logs->apply_log == kept->apply_log,
          "nothing rejected: no detail, the logs as they were");
    s->send(Action(Action::kSetFilter{Filter::kAll}));
    ui.until("filtering", [](const Ui &ui) { return ui.summary->filter == Filter::kAll; });
    check(ui.detail && ui.detail->label == "E01", "every row again, and the first one's detail");

    s->send(Action(Action::kEdit{}));
    ui.until("editing", [](const Ui &ui) { return ui.summary->edit.has_value(); });
    // On the first row, where the filters left the selection: E02's finding, still pending.
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
    check(ui.detail && ui.detail->mark && as<Mark::kWouldApply>(*ui.detail->mark),
          "the selected row's first finding would be applied");
    const FileApply::kDone *would = ui.files.empty() || !ui.files[0].applying
                                        ? nullptr
                                        : as<FileApply::kDone>(*ui.files[0].applying);
    check(would && would->dry_run && would->counts.applied == 1 && !would->remuxed,
          "E01's line says what the dry run would do");
    check(ui.logs && !ui.logs->apply_log.empty(), "and its log, what the dry run wrote");
    // The first row, expanded: one of its two findings would be applied, and that is its member.
    check(ui.rows.size() == 4 && ui.rows[0].marks.size() == 1 &&
              ui.rows[0].marks[0].kind == MarkKind::kWouldApply &&
              ui.rows[0].marks[0].findings == 1 && ui.rows[1].marks.size() == 1 &&
              ui.rows[2].marks.empty(),
          "the row's marks, counted, and its members'");
    check(!ui.rows.empty() && !ui.rows[0].track && ui.rows[0].fixes.empty(),
          "a line's row, with no track or fix");
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
    check(ui.summary->file_selected == 1u && ui.logs && ui.logs->apply_log.empty(),
          "E02 selected, with its own logs");
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
    const ScanStage::kDone *rechecked =
        ui.files.size() == 2 ? as<ScanStage::kDone>(ui.files[1].stage) : nullptr;
    check(rechecked && rechecked->findings == 2 && ui.logs && !ui.logs->log.empty(),
          "E02's line and log, from the scan");
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
    const Status::kOpening *opening = message<Status::kOpening>(*ui.summary);
    check(opening && opening->label == "E01" && opening->at_ms == 1000,
          "the status says what opens, and where");
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
    const Status::kOpened *opened = message<Status::kOpened>(ui.next("opened"));
    check(opened && !opened->paused, "the report reaches review, with where mpv was left");
    s->report(PlayerReport(PlayerReport::kFailed{PlayerError(PlayerError::kCouldNotOpen{e01})}));
    const Status::kCouldNotOpen *lost = message<Status::kCouldNotOpen>(ui.next("failed"));
    check(lost && lost->label == "E01", "a late failure reaches review, its file by its label");

    s->send(Action(Action::kPlayback{Playback(Playback::kSeek{5})}));
    ui.until("seeking", [](const Ui &ui) { return ui.played.size() == 2; });
    const PlayerCommand::kPlayback *pb = as<PlayerCommand::kPlayback>(ui.played[1]);
    const Playback::kSeek *sk = pb ? as<Playback::kSeek>(pb->v1) : nullptr;
    check(sk && sk->v1 == 5, "a seek of 5 s goes to the player");
    check(message<Status::kCouldNotOpen>(*ui.summary), "a seek leaves the message to its report");

    s->report(PlayerReport(PlayerReport::kPlayed{PlayState{true, 6000}}));
    const Status::kPlayed *played = message<Status::kPlayed>(ui.next("played"));
    check(played && played->state.paused && played->state.at_ms == 6000, "where it played, paused");
    s->report(PlayerReport(PlayerReport::kPlayFailed{PlayerError(PlayerError::kNoAnswer{})}));
    check(message<Status::kNoAnswer>(ui.next("no answer")), "a typed failure");
    s->send(Action(Action::kPreviousFile{}));
    const Status::kRefused *refused = message<Status::kRefused>(ui.next("the file before"));
    check(refused && std::holds_alternative<Refusal::kFirstFile>(refused->why.get_variant()),
          "on the first file, the file before is refused, and why");

    s->send(Action(Action::kOpenView{{"no-such-run"}}));
    check(!ui.next("opening a view").status, "a view asked for clears the status line");
    const Status::kFailed *notThere = message<Status::kFailed>(ui.next("a view not there"));
    check(notThere && std::holds_alternative<Failure::kSwitch>(notThere->what.get_variant()) &&
              notThere->error == "no run named no-such-run",
          "a view that is not there leaves the run shown, and says why in the store's words");

    // The run's own view, which is no folder's; then the folder's again, by its path.
    s->send(Action(Action::kSwitchView{}));
    ui.until("the run's view", [](const Ui &ui) { return !ui.summary->of_paths; });
    check(ui.summary->title.starts_with("run "), "the run's view, by its name");
    check(ui.folders.size() == 1 && !ui.folders[0].open, "no folder open in a run's view");
    s->send(Action(Action::kOpenFolder{folder}));
    ui.until("the folder's view", [](const Ui &ui) { return ui.summary->of_paths; });
    check(ui.summary->title == "media", "the folder's view, by its name");
    check(ui.folders.size() == 1 && ui.folders[0].open, "the folder open in its own view");

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
