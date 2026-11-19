// The window's own words and colours for what review sends, shared by the list, the details panel
// and the status bar, so that one thing is called and coloured one way everywhere.
// None of them is the terminal's: they name review's actions, never a key.
#pragma once

#include "subcheck_ffi.hpp"

#include <QColor>
#include <QString>

#include <cstdint>
#include <optional>
#include <string>

// Text as review sends it, in UTF-8, as the window holds it.
QString text(const std::string &s);

// A decision in the window's words: "To review", "Accepted", "Rejected".
QString stateName(subcheck_ffi::State state);

// How a row or a finding shows its decision, never by colour alone: a glyph and a word, and the
// colour of both. `state` none is a row whose decided members differ; `mixed` a row whose shown
// members' decisions differ, which its glyph says even while some are still to review.
struct StateLook {
    QString glyph;
    QString word;
    QColor tone;
};
StateLook stateLook(std::optional<subcheck_ffi::State> state, bool mixed);

// What found a finding, as its badge says it, and the badge's colour.
QString kindWord(subcheck_ffi::Check check);
QColor kindTone(subcheck_ffi::Check check);

// A time as a row says it: 0:05, 12:34, 1:02:03; from 0 for a moment before the file's start.
QString clock(int64_t ms);
// A time to the millisecond, as the details panel and the status bar say it: 0:12:34.450.
QString exactClock(int64_t ms);

// `n` of a thing: "1 finding", "3 findings".
QString counted(uint64_t n, const QString &one, const QString &many);

// How far off a stretch of lines is, late when positive: "2 frames late", "1 frame early",
// "in sync".
QString framesOff(int64_t frames);
// What a timing finding's fix corrects: how far off its whole track is, or "partly off" for a track
// whose stretches differ.
QString fixWords(const subcheck_ffi::Fix &fix);

// Where a change is marked: in a row's one line, which elides, so only the text's colour and
// weight can mark it (Text.StyledText); or in the details panel, which wraps, so a tint behind
// each part can (Text.RichText).
enum class Marking { InRow, InPanel };

// `text` as markup, escaped, with its part `changed` (UTF-16 code units, as review sends them, and
// as QString counts) marked as taken out (struck) or as put in. A span past the text's end is cut
// to it.
QString marked(const QString &text, const subcheck_ffi::Span &changed, bool takenOut, Marking how);

// A row's one line, each way the row can draw it. `whole` is the original with its changed part
// struck and the wording's put in after it, the words the two share kept once: review's spans start
// and end on whole words, so what comes before and after the change is the same in both. `cut` is
// `whole` with the words far before the change cut to the last few, after "…". `reach` is `whole`
// as far as the change's end, with the "…" a row's elided end draws after it when words follow: a
// row measures it in its own fonts, and draws `whole` while it fits and `cut` once it does not.
// With a change near the line's start the two lines are one, and `reach` is empty.
struct RowLine {
    QString whole;
    QString cut;
    QString reach;
};
RowLine rowLine(const QString &original, const subcheck_ffi::Span &originalChanged,
                const QString &wording, const subcheck_ffi::Span &wordingChanged);

// Whether the window is dark: its colours come in pairs, one for a light window and one for a
// dark, chosen by the application palette's lightness, which the style draws with.
bool dark();
