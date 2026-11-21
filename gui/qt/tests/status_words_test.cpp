// The status bar's words for every form of every message, with no session: values made up and
// unlike each other, so that a word taken from the wrong field shows.
#include "status_words.h"

#include <QRegularExpression>
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
            {Status(Status::kDecided{State::kAccepted, 3, 0, std::nullopt}), "Accepted 3 findings"},
            {Status(Status::kDecided{State::kRejected, 1, 0, std::nullopt}), "Rejected 1 finding"},
            {Status(Status::kDecided{State::kAccepted, 1, 1, std::nullopt}),
             "Accepted 1 finding, replacing your wording on 1 (Undo puts it back)"},
            {Status(Status::kDecided{State::kAccepted, 2, 0, Kept{1, State::kRejected}}),
             "Accepted 2 findings, leaving 1 rejected"},
            {Status(Status::kDecided{State::kRejected, 4, 2, Kept{3, State::kPending}}),
             "Rejected 4 findings, replacing your wording on 2 (Undo puts it back), leaving 3 to "
             "review"},
            {Status(Status::kCleared{3}), "3 findings back to undecided"},
            {Status(Status::kCleared{1}), "1 finding back to undecided"},
            {Status(Status::kUndone{1, {"E04"}}), "Undone: 1 finding in E04"},
            {Status(Status::kUndone{2, {"E05", "E04"}}), "Undone: 2 findings in E05, E04"},
            {Status(Status::kInherited{"E04", 1, State::kAccepted}),
             "E04: a new finding took its row's decision, accepted (Undo puts it back)"},
            {Status(Status::kInherited{"E06", 2, State::kRejected}),
             "E06: 2 new findings took their rows' decisions (Undo puts them back)"},
            {Status(Status::kReplaced{"E02", false}),
             "E02 was checked again; your decision went to its new finding"},
            {Status(Status::kReplaced{"E03", true}), "E03 was checked again; that finding is gone"},
            {Status(Status::kEditEnded{"E07"}),
             "E07 was checked again; its finding changed, so the edit ended"},
            {refused(Refusal(Refusal::kSelectionStays{})),
             "That finding changed; the selection stays"},
            {refused(Refusal(Refusal::kNotInView{"E08"})), "E08 is not in this view"},
            {refused(Refusal(Refusal::kTimingFix{})),
             "A timing fix is accepted or rejected, not reworded"},
            {refused(Refusal(Refusal::kEmptyWording{})), "The wording cannot be empty"},
            {refused(Refusal(Refusal::kEditOutOfDate{})),
             "The edit is out of date; nothing was decided"},
            {refused(Refusal(Refusal::kAlready{State::kRejected})), "Already rejected"},
            {refused(Refusal(Refusal::kAlready{State::kPending})), "Already to review"},
            {refused(Refusal(Refusal::kNothingToClear{})),
             "Nothing to put back: none of it is decided"},
            {refused(Refusal(Refusal::kNothingToUndo{})), "Nothing to undo"},
            {refused(Refusal(Refusal::kLastFile{})), "This is the last episode"},
            {refused(Refusal(Refusal::kFirstFile{})), "This is the first episode"},
            {refused(Refusal(Refusal::kFileMissing{"/m/s1/E09.mkv"})),
             "The file is missing: /m/s1/E09.mkv"},
            {refused(Refusal(Refusal::kNothingToOpen{})),
             "Nothing to show: no line of this track moves"},
            {refused(Refusal(Refusal::kScanRunning{})), "A check is still running"},
            {refused(Refusal(Refusal::kApplyRunning{})), "Fixes are still being applied"},
            {refused(Refusal(Refusal::kAlreadyApplying{})), "Fixes are being applied already"},
            {refused(Refusal(Refusal::kNoAcceptedFixes{})), "No accepted fixes to apply"},
            {refused(Refusal(Refusal::kBeingChecked{"E10"})), "E10 is being checked now"},
            {refused(Refusal(Refusal::kNoFindingsShown{"E11"})), "No findings of E11 are shown"},
            {Status(Status::kQueuedAgain{"E12"}),
             "E12 will be checked again when the check under way ends"},
            {Status(Status::kNoSuchFile{"E13"}), "E13 is not on disk, so it was not checked again"},
            {Status(Status::kOpeningView{}), "Opening the view…"},
            {Status(Status::kOpening{"E14", 165060}), "Opening E14 at 0:02:45.060"},
            {Status(Status::kOpening{"E17", -1500}), "Opening E17 at 0:00:00.000"},
            {Status(Status::kNotAnswering{"E18"}), "E18: the share isn't answering, still waiting"},
            {Status(Status::kStoppedWaiting{"E19"}), "Stopped waiting for E19"},
            {Status(Status::kOpened{true}), "Showing it, paused"},
            {Status(Status::kOpened{false}), "Showing it, playing"},
            {Status(Status::kPlayed{PlayState{true, 171060}}), "Paused at 0:02:51.060"},
            {Status(Status::kPlayed{PlayState{false, 3723004}}), "Playing at 1:02:03.004"},
            {Status(Status::kClosed{}), "Video closed"},
            {Status(Status::kNotOpen{Asked::kPlayback}),
             "No video is open; Show in Video opens one"},
            {Status(Status::kNotOpen{Asked::kOpen}), "No video is open"},
            {Status(Status::kNotOpen{Asked::kClose}), "No video is open"},
            {Status(Status::kNoFile{}), "The video has no file open; Show in Video opens one"},
            {Status(Status::kStillOpening{}), "The video is still opening the file"},
            {Status(Status::kCouldNotOpen{"E15"}), "The video could not open E15"},
            {Status(Status::kNoAnswer{}), "The video did not answer"},
            {Status(Status::kHungUp{}), "The video stopped without answering"},
            {Status(Status::kNoVideo{}), "The video pane could not show video"},
            {failed(Failure(Failure::kSave{}), "disk full"), "Not saved: disk full"},
            {failed(Failure(Failure::kSwitch{}), "nothing recorded under '/m/x'"),
             "Could not open that view: nothing recorded under '/m/x'"},
            {failed(Failure(Failure::kReadBack{"E16"}), "database is locked"),
             "Could not read E16 back: database is locked"},
            {failed(Failure(Failure::kRecheck{}), "run r: no LLM is called gpt"),
             "Could not check again: run r: no LLM is called gpt"},
            {failed(Failure(Failure::kMpv{}), "invalid parameter"),
             "The video refused: invalid parameter"},
            {failed(Failure(Failure::kPlayer{}), "no such program"),
             "The video failed: no such program"},
            {Status(Status::kWarned{"warning: /m/locked: skipped", 3}),
             "warning: /m/locked: skipped (and 3 more)"},
            {Status(Status::kWarned{"warning: /m/lost: skipped", 0}), "warning: /m/lost: skipped"},
            // Text that is not the window's goes in as it is, a `%1` in it included.
            {Status(Status::kWarned{"warning: /m/100%1: skipped", 2}),
             "warning: /m/100%1: skipped (and 2 more)"},
            {Status(Status::kInherited{"E%2", 3, State::kAccepted}),
             "E%2: 3 new findings took their rows' decisions (Undo puts them back)"},
            {Status(Status::kReplaced{"E2%1", false}),
             "E2%1 was checked again; your decision went to its new finding"},
            {Status(Status::kOpening{"E3%1", 61025}), "Opening E3%1 at 0:01:01.025"},
            {failed(Failure(Failure::kReadBack{"E4%1"}), "disk I/O error"),
             "Could not read E4%1 back: disk I/O error"},
        };
        // The window's words name review's actions as its menus do, never a terminal key: in
        // brackets, or a lone letter that does something. The check is shown what it is there to
        // catch, so that a pattern that has stopped matching cannot pass for clean words.
        const QRegularExpression key(QStringLiteral(
            "\\((u|o|Esc|y/n)\\b|\\bmpv\\b|"
            "\\b[a-zA-Z] (accepts|rejects|clears|undoes|opens|closes|quits|moves|plays|edits)\\b"));
        for (const char *terminal : {"o opens it", "u undoes", "q quits", "(u undoes)"}) {
            QVERIFY2(key.match(QLatin1String(terminal)).hasMatch(), terminal);
        }
        for (const auto &[status, words] : said) {
            QCOMPARE(statusWords(status), words);
            QVERIFY2(!key.match(words).hasMatch(), qPrintable(words));
        }
    }
};

QTEST_GUILESS_MAIN(StatusWordsTest)
#include "status_words_test.moc"
