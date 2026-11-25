// The window's own words and colours (words.h), with no session: every palette's tones, a
// decision's look, a badge, a time, a count, and a line with its change marked, escaped and counted
// in UTF-16 as review's spans are, as each row style draws it.
#include "offscreen_main.h"
#include "words.h"

#include <QGuiApplication>
#include <QPalette>
#include <QScopeGuard>
#include <QStringList>
#include <QTest>

#include <tuple>
#include <vector>

using namespace subcheck_ffi;

namespace {

// The application's palette, a light or a dark window, for as long as it is held.
struct Window {
    explicit Window(bool dark) {
        QPalette palette = QGuiApplication::palette();
        palette.setColor(QPalette::Window, dark ? QColor("#202020") : QColor("#f0f0f0"));
        QGuiApplication::setPalette(palette);
    }
    ~Window() { QGuiApplication::setPalette(QPalette()); }
};

// A palette's tones on one background as one line, in Tones' order.
QString said(const Tones &t) {
    QStringList each;
    for (const QColor &c :
         {t.old, t.added, t.oldTint, t.addedTint, t.pending, t.mixed, t.typoBadge, t.timingBadge}) {
        each << c.name();
    }
    return each.join(QLatin1Char(' '));
}

} // namespace

class WordsTest : public QObject {
    Q_OBJECT

private slots:
    // Every tone of every palette, on a light background and a dark, as the design chose them:
    // Old, New, Old tint, New tint, To review, Mixed, Typo badge, Timing badge.
    void everyPaletteHasItsEightTonesOnEachBackground() {
        const std::vector<std::tuple<Colours, QString, QString>> expected = {
            {Colours::Default, "#c62828 #1a7f37 #f19d9d #4ce678 #1f5fbf #9a6700 #dbe7fb #f1e5c6",
             "#ff7b72 #56d364 #943841 #1d633a #8ab4f8 #e3b341 #1e3557 #4a3b12"},
            {Colours::Deuteran, "#c73800 #0073ad #ed9d7e #c9e8f7 #57606a #80680a #dbe7fb #f1e5c6",
             "#ff9166 #5cc8ff #70301a #0e4d73 #d0d7de #f2d45c #1e3557 #4a3b12"},
            {Colours::Protan, "#8f6300 #0069d1 #e6b64c #7eb5ed #57606a #d02536 #dbe7fb #f1e5c6",
             "#ffb000 #6cb6ff #6e4600 #114a80 #d0d7de #e87d88 #1e3557 #4a3b12"},
            {Colours::Tritan, "#d61100 #00757f #f09b94 #4cd9e6 #57606a #3367cc #dbe7fb #f1c6f1",
             "#ff6052 #1aecff #a12217 #0b4b50 #d0d7de #648bd8 #1e3557 #8b188b"},
            {Colours::Monochrome, "#646a73 #111315 #e3e5e7 #b4b9c0 #57606a #6a7079 #e3e5e7 #c9cdd1",
             "#a9afb7 #ffffff #33373c #6a7079 #c8cdd3 #8f959d #2e3238 #565b63"},
        };
        QCOMPARE(expected.size(), everyColours.size());
        for (size_t i = 0; i < expected.size(); ++i) {
            const auto &[colours, light, dark] = expected[i];
            QCOMPARE(everyColours[i], colours);
            QCOMPARE(said(tones(colours, false)), light);
            QCOMPARE(said(tones(colours, true)), dark);
        }
    }

    // Each palette has a name that is kept, which names it back, and words for the Settings
    // screen; a name no palette has is the default's.
    void aPaletteIsKeptByANameThatNamesItBack() {
        const QStringList keys = {"default", "deuteran", "protan", "tritan", "monochrome"};
        const QStringList labels = {"Default", "Deuteranomaly/deuteranopia",
                                    "Protanomaly/protanopia", "Tritanomaly/tritanopia",
                                    "Little or no colour vision"};
        for (size_t i = 0; i < everyColours.size(); ++i) {
            const auto at = static_cast<qsizetype>(i);
            QCOMPARE(coloursKey(everyColours[i]), keys[at]);
            QCOMPARE(coloursNamed(keys[at]), everyColours[i]);
            QCOMPARE(coloursLabel(everyColours[i]), labels[at]);
        }
        QCOMPARE(coloursNamed("sepia"), Colours::Default);
        QCOMPARE(coloursNamed(""), Colours::Default);
        QCOMPARE(coloursNamed("Protan"), Colours::Default);
    }

    // Every colour the window draws is the palette's it was last told to use, read as it is asked
    // for: a decision's, a badge's, and a change's in a row and in the panel.
    void theColoursAreThePalettesInUse() {
        const auto restore = qScopeGuard([] { useColours(Colours::Default); });
        QCOMPARE(colours(), Colours::Default);
        for (const bool isDark : {false, true}) {
            const Window window(isDark);
            for (const Colours each : everyColours) {
                useColours(each);
                QCOMPARE(colours(), each);
                const Tones t = tones(each, isDark);
                QCOMPARE(stateLook(State::kPending, false).tone, t.pending);
                QCOMPARE(stateLook(State::kAccepted, false).tone, t.added);
                QCOMPARE(stateLook(State::kRejected, false).tone, t.old);
                QCOMPARE(stateLook(std::nullopt, false).tone, t.mixed);
                QCOMPARE(kindTone(Check::kLlm), t.typoBadge);
                QCOMPARE(kindTone(Check::kTiming), t.timingBadge);
                QCOMPARE(marked("ab", Span{1, 2}, true, Marking::InRow),
                         QStringLiteral("a<s><font color=\"%1\">b</font></s>").arg(t.old.name()));
                QCOMPARE(marked("ab", Span{1, 2}, false, Marking::InRow),
                         QStringLiteral("a<font color=\"%1\"><b>b</b></font>").arg(t.added.name()));
                QCOMPARE(marked("ab", Span{1, 2}, true, Marking::InPanel),
                         QStringLiteral("a<span style=\"background-color:%1\"><s>b</s></span>")
                             .arg(t.oldTint.name()));
                QCOMPARE(marked("ab", Span{1, 2}, false, Marking::InPanel),
                         QStringLiteral("a<span style=\"background-color:%1\"><b>b</b></span>")
                             .arg(t.addedTint.name()));
            }
        }
    }

    void aDecisionShowsAGlyphAndAWordInItsOwnColour() {
        const Window light(false);
        const auto look = [](std::optional<State> s, bool mixed) {
            const StateLook l = stateLook(s, mixed);
            return l.glyph + " " + l.word + " " + l.tone.name();
        };
        QCOMPARE(look(State::kPending, false), "● To review #1f5fbf");
        QCOMPARE(look(State::kAccepted, false), "✓ Accepted #1a7f37");
        QCOMPARE(look(State::kRejected, false), "✕ Rejected #c62828");
        QCOMPARE(look(std::nullopt, false), "◐ Mixed #9a6700");
        // Some still to review, some decided: the glyph says it, the word what is left.
        QCOMPARE(look(State::kPending, true), "◐ To review #1f5fbf");
        QCOMPARE(look(State::kAccepted, true), "◐ Accepted #1a7f37");
        QCOMPARE(stateName(State::kRejected), "Rejected");
    }

    void eachColourHasOneForADarkWindow() {
        const Window window(true);
        QVERIFY(dark());
        QCOMPARE(stateLook(State::kPending, false).tone.name(), "#8ab4f8");
        QCOMPARE(stateLook(State::kAccepted, false).tone.name(), "#56d364");
        QCOMPARE(stateLook(State::kRejected, false).tone.name(), "#ff7b72");
        QCOMPARE(stateLook(std::nullopt, false).tone.name(), "#e3b341");
        QCOMPARE(kindTone(Check::kLlm).name(), "#1e3557");
        QCOMPARE(kindTone(Check::kTiming).name(), "#4a3b12");
        QCOMPARE(marked("x<y", Span{1, 2}, true, Marking::InPanel),
                 "x<span style=\"background-color:#943841\"><s>&lt;</s></span>y");
        QCOMPARE(marked("x<y", Span{1, 2}, false, Marking::InPanel),
                 "x<span style=\"background-color:#1d633a\"><b>&lt;</b></span>y");
    }

    void aBadgeSaysWhatFoundTheFinding() {
        const Window light(false);
        QVERIFY(!dark());
        QCOMPARE(kindWord(Check::kLlm), "Typo");
        QCOMPARE(kindWord(Check::kTiming), "Timing");
        QCOMPARE(kindTone(Check::kLlm).name(), "#dbe7fb");
        QCOMPARE(kindTone(Check::kTiming).name(), "#f1e5c6");
    }

    void aTimeIsMinutesAndSecondsAndHoursWhenThereAreAny() {
        QCOMPARE(clock(5'400), "0:05");
        QCOMPARE(clock(754'999), "12:34");
        QCOMPARE(clock(3'723'456), "1:02:03");
        QCOMPARE(clock(-40), "0:00");
        QCOMPARE(exactClock(754'450), "0:12:34.450");
        QCOMPARE(exactClock(3'723'004), "1:02:03.004");
        QCOMPARE(exactClock(-40), "0:00:00.000");
        QCOMPARE(counted(1, "finding", "findings"), "1 finding");
        QCOMPARE(counted(3, "finding", "findings"), "3 findings");
        QCOMPARE(counted(0, "finding", "findings"), "0 findings");
    }

    // The text around the change is the text's, escaped, so a subtitle's own `<`, `&` or `"` shows
    // as itself and never as markup.
    void aMarkedLineIsEscapedAroundAndInsideItsChange() {
        const Window light(false);
        QCOMPARE(marked("a <b> & \"c\"", Span{6, 7}, false, Marking::InRow),
                 "a &lt;b&gt; <font color=\"#1a7f37\"><b>&amp;</b></font> &quot;c&quot;");
        QCOMPARE(marked("x<y", Span{1, 2}, true, Marking::InRow),
                 "x<s><font color=\"#c62828\">&lt;</font></s>y");
        QCOMPARE(marked("x<y", Span{1, 2}, true, Marking::InPanel),
                 "x<span style=\"background-color:#f19d9d\"><s>&lt;</s></span>y");
        QCOMPARE(marked("x<y", Span{1, 2}, false, Marking::InPanel),
                 "x<span style=\"background-color:#4ce678\"><b>&lt;</b></span>y");
    }

    // Review counts a span in UTF-16 code units, as QString does: a character past U+FFFF before
    // the change takes two, and a span counted in characters would mark the wrong letters.
    void aSpanIsCountedInUtf16() {
        const Window light(false);
        const QString line = QStringLiteral("𝄞 befor it");
        QCOMPARE(line.size(), 11);
        QCOMPARE(marked(line, Span{3, 8}, true, Marking::InRow),
                 "𝄞 <s><font color=\"#c62828\">befor</font></s> it");
        // Past the text's end, the span is cut to it.
        QCOMPARE(marked("ab", Span{1, 9}, false, Marking::InRow),
                 "a<font color=\"#1a7f37\"><b>b</b></font>");
        QCOMPARE(marked("ab", Span{0, 0}, false, Marking::InRow), "ab");
    }

    void aRowsLineStrikesTheOldWordsAndPutsTheNewAfterThem() {
        const Window light(false);
        const QString strike = "<s><font color=\"#c62828\">%1</font></s>";
        const QString put = "<font color=\"#1a7f37\"><b>%1</b></font>";
        QCOMPARE(rowLine("𝄞 We befor it", Span{6, 11}, "𝄞 We before it", Span{6, 12}).whole,
                 "𝄞 We " + strike.arg("befor") + " " + put.arg("before") + " it");
        // A word only taken out keeps a word of context on both sides, as review sends it.
        QCOMPARE(rowLine("We're a making", Span{0, 7}, "We're making", Span{0, 5}).whole,
                 strike.arg("We're a") + " " + put.arg("We're") + " making");
        // Nothing on one side: no space where it would be.
        QCOMPARE(rowLine("x & y", Span{2, 3}, "x  y", Span{2, 2}).whole,
                 "x " + strike.arg("&amp;") + " y");
        QCOMPARE(rowLine("same", Span{0, 0}, "same", Span{0, 0}).whole, "same");
    }

    // A change near the line's start, or none marked: one line, and nothing for a row to measure.
    void aRowsLineWithItsChangeNearItsStartHasOneForm() {
        const Window light(false);
        const RowLine near = rowLine("We befor it", Span{3, 8}, "We before it", Span{3, 9});
        QCOMPARE(near.cut, near.whole);
        QCOMPARE(near.reach, "");
        // 24 units in is the last start with one form.
        const QString xs = QString("x").repeated(23);
        const RowLine edge =
            rowLine(xs + " befor it", Span{24, 29}, xs + " before it", Span{24, 30});
        QCOMPARE(edge.reach, "");
        const QString plain = QString("word ").repeated(30);
        const RowLine none = rowLine(plain, Span{0, 0}, plain, Span{0, 0});
        QCOMPARE(none.whole, plain.toHtmlEscaped());
        QCOMPARE(none.cut, none.whole);
        QCOMPARE(none.reach, "");
    }

    // What a row measures is the whole line as far as its change's end, the new words marked as
    // they are drawn, and the "…" that an elided end would draw after them.
    void aRowsLineIsMeasuredToTheEndOfItsChange() {
        const Window light(false);
        const QString strike = "<s><font color=\"#c62828\">%1</font></s>";
        const QString put = "<font color=\"#1a7f37\"><b>%1</b></font>";
        const QString start = "one two three four five six seven ";
        const auto at = static_cast<uint64_t>(start.size());
        const RowLine tail = rowLine(start + "befor <it>", Span{at, at + 5}, start + "before <it>",
                                     Span{at, at + 6});
        QCOMPARE(tail.reach, start + strike.arg("befor") + " " + put.arg("before") + "…");
        QCOMPARE(tail.whole, start + strike.arg("befor") + " " + put.arg("before") + " &lt;it&gt;");
        // Nothing follows the change: nothing is elided after it.
        const RowLine last =
            rowLine(start + "befor", Span{at, at + 5}, start + "before", Span{at, at + 6});
        QCOMPARE(last.reach, start + strike.arg("befor") + " " + put.arg("before"));
    }

    // A change far into its line has a second form, for a row too narrow to show it: the words
    // before it are cut to the last few, after "…", at a space, never inside a character that takes
    // two units.
    void aRowsLineCutsTheWordsFarBeforeItsChange() {
        const Window light(false);
        const QString strike = "<s><font color=\"#c62828\">%1</font></s>";
        const QString put = "<font color=\"#1a7f37\"><b>%1</b></font>";
        const QString far = QStringLiteral("𝄞𝄞 one two three four five six seven befor it");
        const auto at = static_cast<uint64_t>(far.indexOf("befor"));
        QCOMPARE(at, 39u);
        QString wording = far;
        wording.replace("befor", "before");
        QCOMPARE(rowLine(far, Span{at, at + 5}, wording, Span{at, at + 6}).cut,
                 "…four five six seven " + strike.arg("befor") + " " + put.arg("before") + " it");
        // The last space before the change is the one just before it: the line starts there.
        const QString wide = QString("𝄞").repeated(20) + " befor it";
        QString widened = wide;
        widened.replace("befor", "before");
        QCOMPARE(rowLine(wide, Span{41, 46}, widened, Span{41, 47}).cut,
                 "…" + strike.arg("befor") + " " + put.arg("before") + " it");
        // No space before the change at all, and 24 units back is the second half of a pair: the
        // line starts at its change, the pair whole.
        const QString joined = "a" + QString("𝄞").repeated(20) + "-befor it";
        QVERIFY(joined.at(42 - 24).isLowSurrogate());
        QString rejoined = joined;
        rejoined.replace("befor", "before");
        QCOMPARE(rowLine(joined, Span{42, 47}, rejoined, Span{42, 48}).cut,
                 "…" + strike.arg("befor") + " " + put.arg("before") + " it");
    }

    // The 24 units are a limit on both sides: where a line gets its second form, and which space
    // its cut can fall at.
    void aRowsCutLineKeepsOnlyWhatIsWithin24UnitsOfItsChange() {
        const Window light(false);
        const QString change = "<s><font color=\"#c62828\">befor</font></s> "
                               "<font color=\"#1a7f37\"><b>before</b></font> it";
        const auto line = [](const QString &before) {
            const auto at = static_cast<uint64_t>(before.size());
            return rowLine(before + "befor it", Span{at, at + 5}, before + "before it",
                           Span{at, at + 6});
        };
        // 25 units in is the first start with a second form.
        const RowLine first = line(QString("x").repeated(24) + " ");
        QCOMPARE(first.cut, "…" + change);
        QVERIFY(!first.reach.isEmpty());
        // A space 24 units before the change is within them: what follows it is kept.
        const QString kept = QString("c").repeated(22) + " ";
        QCOMPARE(line("ab " + kept).cut, "…" + kept + change);
        // A space 25 units before it is not: the cut falls at the next one, just before the change.
        QCOMPARE(line("ab " + QString("c").repeated(23) + " ").cut, "…" + change);
    }

    // A Compact row's change: the words taken out, struck, an arrow, and the words put in, each
    // marked and escaped as a row's line is; a side with no words is left out with its space.
    void aCompactChangeIsTheOldWordsStruckThenTheNew() {
        const Window light(false);
        QCOMPARE(changeLine("Your going", Span{0, 4}, "You're going", Span{0, 6}),
                 marked("Your", Span{0, 4}, true, Marking::InRow) + " → " +
                     marked("You're", Span{0, 6}, false, Marking::InRow));
        QCOMPARE(changeLine("the the end", Span{4, 8}, "the end", Span{4, 4}),
                 marked("the ", Span{0, 4}, true, Marking::InRow) + " →");
        QCOMPARE(changeLine("the end", Span{4, 4}, "the new end", Span{4, 8}),
                 "→ " + marked("new ", Span{0, 4}, false, Marking::InRow));
        const QString escaped = changeLine("a <i> b", Span{2, 5}, "a b", Span{2, 2});
        QCOMPARE(escaped, marked("<i>", Span{0, 3}, true, Marking::InRow) + " →");
        QVERIFY2(escaped.contains("&lt;i&gt;") && !escaped.contains("<i>"), qPrintable(escaped));
    }

    // A Rich row's tile is dark in every window, so its change is marked in the palette's dark
    // tones in a light window too; and in its light tones when asked for them in a dark window:
    // the argument picks the tones, never the window.
    void aTileMarksItsChangeInThePalettesDarkTones() {
        const auto restore = qScopeGuard([] { useColours(Colours::Default); });
        for (const Colours each : everyColours) {
            useColours(each);
            const Tones onDark = tones(each, true);
            const Tones onLight = tones(each, false);
            const auto out = [](bool tile) {
                return marked("Your going", Span{0, 4}, true, Marking::InRow, tile);
            };
            const auto in = [](bool tile) {
                return marked("You're going", Span{0, 6}, false, Marking::InRow, tile);
            };
            {
                const Window light(false);
                QVERIFY2(out(true).contains(onDark.old.name()), qPrintable(out(true)));
                QVERIFY2(in(true).contains(onDark.added.name()), qPrintable(in(true)));
                for (const QColor &lit : {onLight.old, onLight.added}) {
                    if (lit != onDark.old && lit != onDark.added) {
                        QVERIFY2(!out(true).contains(lit.name()), qPrintable(out(true)));
                        QVERIFY2(!in(true).contains(lit.name()), qPrintable(in(true)));
                    }
                }
            }
            const Window window(true);
            QVERIFY(dark());
            QVERIFY2(out(false).contains(onLight.old.name()), qPrintable(out(false)));
            QVERIFY2(in(false).contains(onLight.added.name()), qPrintable(in(false)));
        }
    }
};

SUBCHECK_OFFSCREEN_TEST_MAIN(WordsTest)
#include "words_test.moc"
