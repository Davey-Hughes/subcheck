// The generated bindings over a real session, on the run `seed_a_run` made of the folder argv[1]
// names: the first event, a decision, the player's round (open, a report, a seek, a report), the
// end, then a session closed and a run that is not there. Every call goes through the generated
// code, whose first call checks that it was generated from this library. Three sessions in one
// process, where a UI opens one: none reaches group::end, which is what makes that the rule.
#include "subcheck_ffi.hpp"

#include <cstdint>
#include <cstdlib>
#include <exception>
#include <iostream>
#include <memory>
#include <optional>
#include <string>
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

// The next event, which must be a change.
Event::kChanged changed(Session &s, const std::string &after) {
    std::optional<Event> e = s.next_event();
    const Event::kChanged *c = e ? as<Event::kChanged>(*e) : nullptr;
    if (!c) {
        std::cerr << "FAIL no change after " << after << "; came: " << came(e) << "\n";
        std::exit(1);
    }
    return *c;
}

// The next event, which must be a command for the player.
PlayerCommand player(Session &s, const std::string &after) {
    std::optional<Event> e = s.next_event();
    const Event::kPlayer *p = e ? as<Event::kPlayer>(*e) : nullptr;
    if (!p) {
        std::cerr << "FAIL no player command after " << after << "; came: " << came(e) << "\n";
        std::exit(1);
    }
    return p->v1;
}

void a_run_opens_takes_a_decision_plays_and_ends(const std::string &folder) {
    std::shared_ptr<Session> s = Session::open({folder});

    Event::kChanged first = changed(*s, "opening");
    check(first.rows.has_value(), "the first change has rows");
    if (first.rows) {
        const Splice &rows = *first.rows;
        check(rows.at == 0 && rows.removed == 0 && rows.inserted.size() == 2,
              "the first splice is the run's two rows, from an empty list");
        if (rows.inserted.size() == 2) {
            const EntryView &row = rows.inserted[0];
            const EntryKey::kRow *key = as<EntryKey::kRow>(row.key);
            check(key && key->v1 == std::vector<uint64_t>{1}, "the first row is finding 1's");
            check(row.state == State::kPending && !row.is_member, "a pending row");
            check(row.at_ms == 1000, "at 0:00:01");
            check(row.original == "We should leave befor it rains.", "its line");
            check(row.wording == "We should leave before it rains.", "its fix");
        }
    }
    check(first.summary.selected == 0u, "the first row is selected");
    check(first.summary.pending == 2 && first.summary.accepted == 0, "two findings pending");

    s->send(Action(Action::kAccept{}));
    Event::kChanged accepted = changed(*s, "accepting");
    check(accepted.rows && accepted.rows->at == 0 && accepted.rows->removed == 1 &&
              accepted.rows->inserted.size() == 1 &&
              accepted.rows->inserted[0].state == State::kAccepted,
          "accepting changes the first row in place");
    check(accepted.summary.pending == 1 && accepted.summary.accepted == 1, "one accepted");
    check(accepted.summary.selected == 1u, "the selection moves on to the pending row");

    s->send(Action(Action::kPlay{}));
    std::optional<std::string> opening = changed(*s, "playing").summary.status;
    check(opening == "opening typos at 0:00:18.00", "the status line says what opens");
    PlayerCommand open = player(*s, "playing");
    const PlayerCommand::kOpen *o = as<PlayerCommand::kOpen>(open);
    check(o && o->v1.file.ends_with("/media/typos.mkv") && o->v1.sid == 1u && !o->v1.paused,
          "play opens the run's file on its subtitle track, playing");
    check(o && o->v1.start_ms == 18000, "two seconds before the selected row's line");

    s->report(PlayerReport(PlayerReport::kOpened{std::nullopt}));
    check(changed(*s, "opened").summary.status == "opened in mpv", "the report reaches review");

    s->send(Action(Action::kPlayback{Playback(Playback::kSeek{5})}));
    check(!changed(*s, "seeking").summary.status, "a seek clears the status line");
    PlayerCommand seek = player(*s, "seeking");
    const PlayerCommand::kPlayback *pb = as<PlayerCommand::kPlayback>(seek);
    const Playback::kSeek *sk = pb ? as<Playback::kSeek>(pb->v1) : nullptr;
    check(sk && sk->v1 == 5, "a seek of 5 s goes to the player");

    s->report(PlayerReport(PlayerReport::kPlayed{PlayState{true, 6000}}));
    check(changed(*s, "played").summary.status == "⏸ 0:00:06.000", "where it played, paused");

    s->send(Action(Action::kQuit{}));
    std::optional<Event> e;
    while ((e = s->next_event()) && !as<Event::kEnded>(*e)) {
    }
    const Event::kEnded *end = e ? as<Event::kEnded>(*e) : nullptr;
    check(end && !end->fatal, "quitting ends the session, with nothing fatal; came: " + came(e));
    check(!s->next_event(), "nothing after the end");
}

// What was queued before close() may still come out, and then nothing, at once.
void a_closed_session_ends_its_stream(const std::string &folder) {
    std::shared_ptr<Session> s = Session::open({folder});
    s->close();
    while (s->next_event()) {
    }
    check(!s->next_event(), "nothing after close");
}

void a_run_that_is_not_there_ends_at_once() {
    std::shared_ptr<Session> s = Session::open({"no-such-run"});
    std::optional<Event> e = s->next_event();
    const Event::kEnded *end = e ? as<Event::kEnded>(*e) : nullptr;
    check(end && end->fatal == "no run named no-such-run",
          "the first event says why; came: " + came(e));
}

} // namespace

int main(int argc, char **argv) {
    // It decides a finding in whatever store it opens: run by hand without CTest's environment,
    // that would be the user's own.
    if (argc != 2 || !std::getenv("SUBCHECK_DB")) {
        std::cerr << "usage: SUBCHECK_DB=<a store of its own> bindings_smoke <folder of the seeded "
                     "run>\n";
        return 2;
    }
    try {
        a_run_opens_takes_a_decision_plays_and_ends(argv[1]);
        a_closed_session_ends_its_stream(argv[1]);
        a_run_that_is_not_there_ends_at_once();
    } catch (const std::exception &e) {
        std::cerr << "FAIL threw: " << e.what() << "\n";
        return 1;
    }
    return failed == 0 ? 0 : 1;
}
