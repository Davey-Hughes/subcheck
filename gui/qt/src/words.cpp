#include "words.h"

#include "overloaded.h"

#include <QGuiApplication>
#include <QPalette>

#include <algorithm>
#include <cstdlib>
#include <variant>

using namespace subcheck_ffi;

namespace {

// The most of a row's cut line kept before its change, in UTF-16 units: a few words to read the
// change in, and few enough to bring the change near the row's start.
constexpr qsizetype rowContext = 24;

// A colour for a light window and one for a dark.
QColor pick(const char *light, const char *forDark) { return QColor(dark() ? forDark : light); }

QColor removedTone() { return pick("#c62828", "#ff7b72"); }

QColor addedTone() { return pick("#1a7f37", "#56d364"); }

// `text`'s units from `from` up to `to`, escaped; the span cut to the text.
QString part(const QString &text, qsizetype from, qsizetype to) {
    from = std::clamp<qsizetype>(from, 0, text.size());
    to = std::clamp<qsizetype>(to, from, text.size());
    return text.mid(from, to - from).toHtmlEscaped();
}

// `inner`, escaped already, marked as taken out or put in.
QString mark(const QString &inner, bool takenOut, Marking how) {
    if (inner.isEmpty()) {
        return inner;
    }
    if (how == Marking::InRow) {
        // A struck part keeps its weight, so that the line's length reads as it was.
        return takenOut ? QStringLiteral("<s><font color=\"%1\">%2</font></s>")
                              .arg(removedTone().name(), inner)
                        : QStringLiteral("<font color=\"%1\"><b>%2</b></font>")
                              .arg(addedTone().name(), inner);
    }
    // Struck or bold as well as tinted, so that a reader who cannot tell the two tints apart
    // still tells the words apart.
    const QColor tint = takenOut ? pick("#ffd7d5", "#5a1d1d") : pick("#ccffd8", "#1d4428");
    return QStringLiteral("<span style=\"background-color:%1\">%2</span>")
        .arg(tint.name(),
             (takenOut ? QStringLiteral("<s>%1</s>") : QStringLiteral("<b>%1</b>")).arg(inner));
}

} // namespace

QString text(const std::string &s) { return QString::fromStdString(s); }

QString stateName(State state) {
    switch (state) {
    case State::kPending:
        return QStringLiteral("To review");
    case State::kAccepted:
        return QStringLiteral("Accepted");
    case State::kRejected:
        return QStringLiteral("Rejected");
    }
    return QStringLiteral("?");
}

StateLook stateLook(std::optional<State> state, bool mixed) {
    if (!state) {
        return {QStringLiteral("◐"), QStringLiteral("Mixed"), pick("#9a6700", "#e3b341")};
    }
    const QString glyph = mixed ? QStringLiteral("◐") : QString();
    switch (*state) {
    case State::kPending:
        return {glyph.isEmpty() ? QStringLiteral("●") : glyph, stateName(*state),
                pick("#1f5fbf", "#8ab4f8")};
    case State::kAccepted:
        return {glyph.isEmpty() ? QStringLiteral("✓") : glyph, stateName(*state), addedTone()};
    case State::kRejected:
        return {glyph.isEmpty() ? QStringLiteral("✕") : glyph, stateName(*state), removedTone()};
    }
    return {QStringLiteral("?"), QStringLiteral("?"), QColor()};
}

QString kindWord(Check check) {
    return check == Check::kTiming ? QStringLiteral("Timing") : QStringLiteral("Typo");
}

QColor kindTone(Check check) {
    return check == Check::kTiming ? pick("#efe0fb", "#3b2752") : pick("#dbe7fb", "#1e3557");
}

QString clock(int64_t ms) {
    const int64_t s = std::max<int64_t>(ms, 0) / 1000;
    if (s >= 3600) {
        return QStringLiteral("%1:%2:%3")
            .arg(s / 3600)
            .arg(s / 60 % 60, 2, 10, QLatin1Char('0'))
            .arg(s % 60, 2, 10, QLatin1Char('0'));
    }
    return QStringLiteral("%1:%2").arg(s / 60).arg(s % 60, 2, 10, QLatin1Char('0'));
}

QString exactClock(int64_t ms) {
    ms = std::max<int64_t>(ms, 0);
    return QStringLiteral("%1:%2:%3.%4")
        .arg(ms / 3600000)
        .arg(ms / 60000 % 60, 2, 10, QLatin1Char('0'))
        .arg(ms / 1000 % 60, 2, 10, QLatin1Char('0'))
        .arg(ms % 1000, 3, 10, QLatin1Char('0'));
}

QString counted(uint64_t n, const QString &one, const QString &many) {
    return QStringLiteral("%1 %2").arg(n).arg(n == 1 ? one : many);
}

QString framesOff(int64_t frames) {
    if (frames == 0) {
        return QStringLiteral("in sync");
    }
    return QStringLiteral("%1 %2").arg(counted(static_cast<uint64_t>(std::abs(frames)),
                                               QStringLiteral("frame"), QStringLiteral("frames")),
                                       frames > 0 ? QStringLiteral("late")
                                                  : QStringLiteral("early"));
}

QString fixWords(const Fix &fix) {
    return std::visit(
        Overloaded{[](const Fix::kShift &s) { return framesOff(s.frames); },
                   [](const Fix::kPartial &) { return QStringLiteral("partly off"); }},
        fix.get_variant());
}

QString marked(const QString &text, const Span &changed, bool takenOut, Marking how) {
    const auto start = static_cast<qsizetype>(changed.start);
    const auto end = static_cast<qsizetype>(changed.end);
    return part(text, 0, start) + mark(part(text, start, end), takenOut, how) +
           part(text, end, text.size());
}

RowLine rowLine(const QString &original, const Span &originalChanged, const QString &wording,
                const Span &wordingChanged) {
    const auto start =
        std::clamp<qsizetype>(static_cast<qsizetype>(originalChanged.start), 0, original.size());
    const auto end = static_cast<qsizetype>(originalChanged.end);
    const QString out = mark(part(original, start, end), true, Marking::InRow);
    const QString in = mark(part(wording, static_cast<qsizetype>(wordingChanged.start),
                                 static_cast<qsizetype>(wordingChanged.end)),
                            false, Marking::InRow);
    const QString between = out.isEmpty() || in.isEmpty() ? QString() : QStringLiteral(" ");
    const QString change = out + between + in;
    const QString after = part(original, end, original.size());
    const QString upToChange = part(original, 0, start) + change;
    if (start <= rowContext) {
        return {upToChange + after, upToChange + after, QString()};
    }
    // Cut at a space, never inside a character that takes two units; with no space to cut at,
    // the line starts at its change.
    const qsizetype space = original.indexOf(QLatin1Char(' '), start - rowContext);
    const qsizetype from = space >= 0 && space < start ? space + 1 : start;
    const QString ellipsis = QStringLiteral("…");
    return {upToChange + after, ellipsis + part(original, from, start) + change + after,
            upToChange + (after.isEmpty() ? QString() : ellipsis)};
}

bool dark() {
    // A program with no GUI application has no palette to read: asking for one there reads a
    // QGuiApplication that is not one, and crashes.
    if (!qobject_cast<QGuiApplication *>(QCoreApplication::instance())) {
        return false;
    }
    return QGuiApplication::palette().color(QPalette::Window).lightness() < 128;
}
