// The status line's words for every form of every message, with no session: values made up and
// unlike each other, so that a word taken from the wrong field shows.
#include "status_words.h"

#include <QTest>

#include <string>
#include <utility>
#include <vector>

using namespace subcheck_ffi;

class StatusWordsTest : public QObject {
    Q_OBJECT

private slots:
    void eachFormOfEachMessageHasItsWords() {
        const auto refused = [](Refusal why) { return Status(Status::kRefused{std::move(why)}); };
        const auto failed = [](Failure what, std::string error) {
            return Status(Status::kFailed{std::move(what), std::move(error)});
        };
        const std::vector<std::pair<Status, QString>> said = {
            {Status(Status::kDecided{State::kAccepted, 3, 0, std::nullopt}), "accepted 3 findings"},
            {Status(Status::kDecided{State::kRejected, 1, 0, std::nullopt}), "rejected 1 finding"},
            {Status(Status::kDecided{State::kAccepted, 1, 1, std::nullopt}),
             "accepted 1 finding, replacing your wording on 1 (u undoes)"},
            {Status(Status::kDecided{State::kAccepted, 2, 0, Kept{1, State::kRejected}}),
             "accepted 2 findings, 1 kept as rejected"},
            {Status(Status::kDecided{State::kAccepted, 4, 2, Kept{3, State::kPending}}),
             "accepted 4 findings, replacing your wording on 2 (u undoes), 3 kept as pending"},
            {Status(Status::kCleared{3}), "cleared 3 findings"},
            {Status(Status::kCleared{1}), "cleared 1 finding"},
            {Status(Status::kUndone{1, {"E04"}}), "undone: 1 finding · E04"},
            {Status(Status::kUndone{2, {"E05", "E04"}}), "undone: 2 findings · E05, E04"},
            {Status(Status::kInherited{"E04", 1, State::kAccepted}),
             "E04: a new finding took its row's decision: accepted (u undoes)"},
            {Status(Status::kInherited{"E06", 2, State::kRejected}),
             "E06: 2 new findings took their rows' decisions (u undoes)"},
            {Status(Status::kReplaced{"E02", false}),
             "E02 was checked again; your decision went to its new finding"},
            {Status(Status::kReplaced{"E03", true}), "E03 was checked again; that finding is gone"},
            {Status(Status::kEditEnded{"E07"}), "E07 was checked again; its finding changed"},
            {refused(Refusal(Refusal::kSelectionStays{})),
             "that finding changed; the selection stays"},
            {refused(Refusal(Refusal::kNotInView{"E08"})), "E08 is not in this view"},
            {refused(Refusal(Refusal::kTimingFix{})),
             "a timing fix is accepted or rejected, not edited"},
            {refused(Refusal(Refusal::kEmptyWording{})),
             "the wording cannot be empty (Esc cancels)"},
            {refused(Refusal(Refusal::kEditOutOfDate{})),
             "the edit is out of date; nothing was decided"},
            {refused(Refusal(Refusal::kAlready{State::kRejected})), "already rejected"},
            {refused(Refusal(Refusal::kAlready{State::kAccepted})), "already accepted"},
            {refused(Refusal(Refusal::kNothingToClear{})), "nothing to clear"},
            {refused(Refusal(Refusal::kNothingToUndo{})), "nothing to undo"},
            {refused(Refusal(Refusal::kLastFile{})), "last file"},
            {refused(Refusal(Refusal::kFirstFile{})), "first file"},
            {refused(Refusal(Refusal::kFileMissing{"/m/s1/E09.mkv"})),
             "file missing: /m/s1/E09.mkv"},
            {refused(Refusal(Refusal::kNothingToOpen{})),
             "nothing to open: no line of this track moves"},
            {refused(Refusal(Refusal::kScanRunning{})), "the scan is still running"},
            {refused(Refusal(Refusal::kApplyRunning{})), "apply is still running"},
            {refused(Refusal(Refusal::kAlreadyApplying{})), "already applying"},
            {refused(Refusal(Refusal::kNoAcceptedFixes{})), "no accepted fixes"},
            {refused(Refusal(Refusal::kBeingChecked{"E10"})), "E10 is being checked now"},
            {refused(Refusal(Refusal::kNoFindingsShown{"E11"})), "no findings of E11 shown"},
            {Status(Status::kQueuedAgain{"E12"}),
             "E12 will be checked again when the scan is done"},
            {Status(Status::kNoSuchFile{"E13"}), "E13: no such file"},
            {Status(Status::kOpening{"E14", 165060}), "opening E14 at 0:02:45.060"},
            {Status(Status::kOpening{"E17", -1500}), "opening E17 at 0:00:00.000"},
            {Status(Status::kOpened{true}), "opened in mpv, paused"},
            {Status(Status::kOpened{false}), "opened in mpv, playing"},
            {Status(Status::kPlayed{PlayState{true, 171060}}), "⏸ 0:02:51.060"},
            {Status(Status::kPlayed{PlayState{false, 1000}}), "▶ 0:00:01.000"},
            {Status(Status::kPlayed{PlayState{false, 3723004}}), "▶ 1:02:03.004"},
            {Status(Status::kClosed{}), "closed mpv"},
            {Status(Status::kNotOpen{Asked::kPlayback}), "mpv is not open (o opens it)"},
            {Status(Status::kNotOpen{Asked::kOpen}), "mpv is not open"},
            {Status(Status::kNotOpen{Asked::kClose}), "mpv is not open"},
            {Status(Status::kNoFile{}), "mpv has no file open (o opens one)"},
            {Status(Status::kStillOpening{}), "mpv is still opening the file"},
            {Status(Status::kCouldNotOpen{"E15"}), "mpv could not open E15"},
            {Status(Status::kNoAnswer{}), "mpv did not answer"},
            {Status(Status::kHungUp{}), "mpv closed the connection without answering"},
            {Status(Status::kNoVideo{}), "the video pane could not show video"},
            {failed(Failure(Failure::kSave{}), "disk full"), "not saved: disk full"},
            {failed(Failure(Failure::kSwitch{}), "no run named nope"),
             "could not switch views: no run named nope"},
            {failed(Failure(Failure::kReadBack{"E16"}), "database is locked"),
             "could not read E16 back: database is locked"},
            {failed(Failure(Failure::kRecheck{}), "run r: no LLM is called gpt"),
             "run r: no LLM is called gpt"},
            {failed(Failure(Failure::kMpv{}), "invalid parameter"), "mpv: invalid parameter"},
            {failed(Failure(Failure::kPlayer{}), "mpv not found: install mpv to open findings"),
             "mpv not found: install mpv to open findings"},
            {Status(Status::kWarned{"warning: /m/locked: skipped: Permission denied (os error 13)",
                                    3}),
             "warning: /m/locked: skipped: Permission denied (os error 13) · +3 more"},
            {Status(
                 Status::kWarned{"warning: /m/lost: skipped: Input/output error (os error 5)", 0}),
             "warning: /m/lost: skipped: Input/output error (os error 5)"},
            // Text that is not review's goes in as it is, a `%1` in it included.
            {Status(Status::kWarned{"warning: /m/100%1: skipped", 2}),
             "warning: /m/100%1: skipped · +2 more"},
            {Status(Status::kInherited{"E%2", 3, State::kAccepted}),
             "E%2: 3 new findings took their rows' decisions (u undoes)"},
            {Status(Status::kReplaced{"E2%1", false}),
             "E2%1 was checked again; your decision went to its new finding"},
            {Status(Status::kOpening{"E3%1", 61025}), "opening E3%1 at 0:01:01.025"},
            {failed(Failure(Failure::kReadBack{"E4%1"}), "disk I/O error"),
             "could not read E4%1 back: disk I/O error"},
        };
        for (const auto &[status, words] : said) {
            QCOMPARE(statusWords(status), words);
        }
    }
};

QTEST_GUILESS_MAIN(StatusWordsTest)
#include "status_words_test.moc"
