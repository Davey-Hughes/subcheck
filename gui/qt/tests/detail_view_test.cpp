// The details panel's words for each part of review's `Detail`, with no session: values made up
// and unlike each other, so that a word taken from the wrong field shows.
#include "detail_view.h"
#include "offscreen_main.h"
#include "words.h"

#include <QSignalSpy>
#include <QTest>

#include <utility>
#include <vector>

using namespace subcheck_ffi;

namespace {

// A line finding in E03, as review would send one selected.
Detail line() {
    return Detail{
        .file = "/m/S1/E03.mkv",
        .label = "E03",
        .group = std::nullopt,
        .at_ms = 754'450,
        .track = Track{3, 2, "Full", "en"},
        .more = 2,
        .note = std::nullopt,
        .checking = std::nullopt,
        .truncated = std::nullopt,
        .state = State::kAccepted,
        .edited = false,
        .mark = std::nullopt,
        .original = "𝄞 Your going <there> & back",
        .wording = "𝄞 You're going <there> & back",
        .original_changed = Span{3, 7},
        .wording_changed = Span{3, 9},
        .reason = "your for you're",
        .kind = Kind(Kind::kLine{{"First before", "Second before"}, {"After"}}),
    };
}

} // namespace

class DetailViewTest : public QObject {
    Q_OBJECT

private slots:
    void nothingSelectedShowsNothing() {
        DetailView view;
        QSignalSpy changed(&view, &DetailView::changed);
        view.show(line());
        view.show(std::nullopt);
        QCOMPARE(changed.count(), 2);
        QVERIFY(!view.shown());
        QCOMPARE(view.file(), "");
        QCOMPARE(view.kind(), "");
        QCOMPARE(view.place(), "");
        QCOMPARE(view.now(), "");
        QCOMPARE(view.technical(), QStringList());
        QCOMPARE(view.before(), QStringList());
        QVERIFY(!view.edited());
    }

    void aLineFindingIsItsKindPlaceStateChangeReasonAndNeighbours() {
        DetailView view;
        view.show(line());
        QVERIFY(view.shown());
        QCOMPARE(view.file(), "/m/S1/E03.mkv");
        QCOMPARE(view.kind(), "Typo");
        QCOMPARE(view.kindTone(), kindTone(Check::kLlm));
        QCOMPARE(view.place(), "E03 · 12:34");
        QCOMPARE(view.glyph(), "✓");
        QCOMPARE(view.stateWord(), "Accepted");
        QCOMPARE(view.stateTone(), stateLook(State::kAccepted, false).tone);
        QCOMPARE(view.now(),
                 marked("𝄞 Your going <there> & back", Span{3, 7}, true, Marking::InPanel));
        QVERIFY(view.now().contains("<s>Your</s>"));
        QVERIFY(view.now().contains("&lt;there&gt; &amp; back"));
        QCOMPARE(view.fixed(),
                 marked("𝄞 You're going <there> & back", Span{3, 9}, false, Marking::InPanel));
        QVERIFY(view.fixed().contains(">You&#39;re<") || view.fixed().contains(">You're<"));
        QCOMPARE(view.reason(), "your for you're");
        QCOMPARE(view.before(), (QStringList{"First before", "Second before"}));
        QCOMPARE(view.after(), QStringList{"After"});
        QCOMPARE(view.timing(), QStringList());
        QCOMPARE(view.episodes(), "");
        QCOMPARE(view.banners(), QStringList());
        QCOMPARE(view.mark(), "");
        QCOMPARE(view.technical(),
                 (QStringList{"File: /m/S1/E03.mkv", "Track 3, sid 2: Full (en)",
                              "Starts at 0:12:34.450", "Also in 2 other places"}));
    }

    void aTimingFindingIsItsStretchesAndWhereItOpens() {
        Detail d = line();
        d.at_ms = std::nullopt;
        d.track = Track{4, 3, std::nullopt, ""};
        d.more = 0;
        d.state = State::kRejected;
        d.kind = Kind(Kind::kTiming{
            {{2, std::nullopt, 60'000}, {0, 60'000, 120'500}, {-1, 120'500, std::nullopt}},
            {{1'000, 2'000}, {3'000, 4'000}},
            TimingMoment{2'500, 3, true}});
        DetailView view;
        view.show(d);
        QCOMPARE(view.kind(), "Timing");
        QCOMPARE(view.kindTone(), kindTone(Check::kTiming));
        QCOMPARE(view.place(), "E03");
        QCOMPARE(view.stateWord(), "Rejected");
        QCOMPARE(view.before(), QStringList());
        QCOMPARE(view.timing(),
                 (QStringList{"2 frames late, from the start to 0:01:00.000",
                              "In sync, from 0:01:00.000 to 0:02:00.500",
                              "1 frame early, from 0:02:00.500 to the end",
                              "2 lines stay where they are",
                              "Show in Video opens it at 0:00:02.500, on the frame that shows the "
                              "error"}));
        QCOMPARE(view.technical(), (QStringList{"File: /m/S1/E03.mkv", "Track 4, sid 3"}));
        d.kind = Kind(Kind::kTiming{
            {{1, std::nullopt, std::nullopt}}, {}, TimingMoment{9'000, std::nullopt, false}});
        view.show(d);
        QCOMPARE(view.timing(),
                 (QStringList{"1 frame late, from the start to the end",
                              "Show in Video opens it at 0:00:09.000, on the first line of a "
                              "stretch that moves"}));
        d.kind =
            Kind(Kind::kTiming{{{1, std::nullopt, std::nullopt}}, {{1'000, 2'000}}, std::nullopt});
        view.show(d);
        QCOMPARE(view.timing(), (QStringList{"1 frame late, from the start to the end",
                                             "1 line stays where it is"}));
    }

    void aRowOfSeveralNamesItsEpisodesAndHowManyTheFilterHides() {
        Detail d = line();
        d.group = Group{
            {{"E01", std::nullopt}, {"E02", Fix(Fix::kShift{-3})}, {"E04", Fix(Fix::kPartial{})}},
            5};
        DetailView view;
        view.show(d);
        // The count is the row's: the places the filter hides are among them.
        QCOMPARE(view.episodes(),
                 "In 8 places: E01, E02 (3 frames early), E04 (partly off); 5 more the filter "
                 "hides");
        d.group = Group{{{"E01", Fix(Fix::kShift{1})}}, 0};
        view.show(d);
        QCOMPARE(view.episodes(), "In 1 place: E01 (1 frame late)");
        d.group = Group{{{"E01", std::nullopt}}, 1};
        view.show(d);
        QCOMPARE(view.episodes(), "In 2 places: E01; 1 more the filter hides");
        // One typo twice in an episode is two places of one episode.
        d.group = Group{{{"E01", std::nullopt}, {"E01", std::nullopt}, {"E02", std::nullopt}}, 0};
        view.show(d);
        QCOMPARE(view.episodes(), "In 3 places: E01, E01, E02");
    }

    // The episodes the list calls by these names are the finding's own, or its row's members'.
    void anEpisodeIsTheFindingsOrOneOfItsRows() {
        DetailView view;
        QVERIFY(!view.isOf("E03"));
        Detail d = line();
        view.show(d);
        QVERIFY(view.isOf("E03"));
        QVERIFY(!view.isOf("E04"));
        d.group = Group{{{"E03", std::nullopt}, {"E04", std::nullopt}}, 1};
        view.show(d);
        QVERIFY(view.isOf("E04"));
        QVERIFY(!view.isOf("E05"));
        QVERIFY(!view.isOf(""));
    }

    void whatSetsItsFileApartIsABanner() {
        Detail d = line();
        d.note = FileNote::kChanged;
        d.checking = Checking(Checking::kAsking{1, 4});
        d.truncated = Truncated{2'000'000, 1'000'000};
        DetailView view;
        view.show(d);
        QCOMPARE(view.banners(),
                 (QStringList{"This episode changed since it was checked", "Being checked again",
                              "Checked from a copy cut short: 976.6 KiB of 1.9 "
                              "MiB"}));
        d.note = FileNote::kMissing;
        d.checking = Checking(Checking::kQueued{});
        d.truncated = std::nullopt;
        view.show(d);
        QCOMPARE(view.banners(),
                 (QStringList{"This episode's file is missing", "Waiting to be checked again"}));
    }

    void eachMarkOfApplysHasItsWords() {
        const std::vector<std::pair<Mark, QString>> said = {
            {Mark(Mark::kByHand{"styled line"}), "Left for a fix by hand: styled line"},
            {Mark(Mark::kNotFound{}), "Not found in the file"},
            {Mark(Mark::kWouldApply{}), "The preview would apply it"},
            {Mark(Mark::kAlreadyApplied{}), "The preview found it in the file already"},
            {Mark(Mark::kWouldBeByHand{"karaoke"}),
             "The preview would leave it for a fix by hand: karaoke"},
            {Mark(Mark::kWouldNotBeFound{}), "The preview would not find it in the file"},
            {Mark(Mark::kFailed{"mkvmerge said no"}), "Applying it failed: mkvmerge said no"},
            {Mark(Mark::kWouldFail{"no space"}), "The preview failed: no space"},
            {Mark(Mark::kApplied{}), "Applied: the fix is in the file"},
        };
        DetailView view;
        Detail d = line();
        for (const auto &[mark, words] : said) {
            d.mark = mark;
            view.show(d);
            QCOMPARE(view.mark(), words);
        }
        d.edited = true;
        view.show(d);
        QVERIFY(view.edited());
    }
};

SUBCHECK_OFFSCREEN_TEST_MAIN(DetailViewTest)
#include "detail_view_test.moc"
