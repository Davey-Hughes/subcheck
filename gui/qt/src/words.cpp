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

// Every palette's tones, for a light window and then a dark, in Tones' order. One table, so that a
// tone is changed here and nowhere else.
struct Row {
    const char *old;
    const char *added;
    const char *oldTint;
    const char *addedTint;
    const char *pending;
    const char *mixed;
    const char *typoBadge;
    const char *timingBadge;
};
constexpr Row table[everyColours.size()][2] = {
    // Default
    {{"#c62828", "#1a7f37", "#f19d9d", "#4ce678", "#1f5fbf", "#9a6700", "#dbe7fb", "#f1e5c6"},
     {"#ff7b72", "#56d364", "#943841", "#1d633a", "#8ab4f8", "#e3b341", "#1e3557", "#4a3b12"}},
    // Deuteranomaly/deuteranopia
    {{"#c73800", "#0073ad", "#ed9d7e", "#c9e8f7", "#57606a", "#80680a", "#dbe7fb", "#f1e5c6"},
     {"#ff9166", "#5cc8ff", "#70301a", "#0e4d73", "#d0d7de", "#f2d45c", "#1e3557", "#4a3b12"}},
    // Protanomaly/protanopia
    {{"#8f6300", "#0069d1", "#e6b64c", "#7eb5ed", "#57606a", "#d02536", "#dbe7fb", "#f1e5c6"},
     {"#ffb000", "#6cb6ff", "#6e4600", "#114a80", "#d0d7de", "#e87d88", "#1e3557", "#4a3b12"}},
    // Tritanomaly/tritanopia
    {{"#d61100", "#00757f", "#f09b94", "#4cd9e6", "#57606a", "#3367cc", "#dbe7fb", "#f1c6f1"},
     {"#ff6052", "#1aecff", "#a12217", "#0b4b50", "#d0d7de", "#648bd8", "#1e3557", "#8b188b"}},
    // Little or no colour vision
    {{"#646a73", "#111315", "#e3e5e7", "#b4b9c0", "#57606a", "#6a7079", "#e3e5e7", "#c9cdd1"},
     {"#a9afb7", "#ffffff", "#33373c", "#6a7079", "#c8cdd3", "#8f959d", "#2e3238", "#565b63"}},
};

// The palette in use, which every colour here is read from as it is asked for.
Colours drawn = Colours::Default;

// The tones the window draws with now: its palette's, for its background.
Tones now() { return tones(drawn, dark()); }

// `text`'s units from `from` up to `to`, escaped; the span cut to the text.
QString part(const QString &text, qsizetype from, qsizetype to) {
    from = std::clamp<qsizetype>(from, 0, text.size());
    to = std::clamp<qsizetype>(to, from, text.size());
    return text.mid(from, to - from).toHtmlEscaped();
}

// `inner`, escaped already, marked as taken out or put in, in `tone`.
QString mark(const QString &inner, bool takenOut, Marking how, const Tones &tone) {
    if (inner.isEmpty()) {
        return inner;
    }
    if (how == Marking::InRow) {
        // A struck part keeps its weight, so that the line's length reads as it was.
        return takenOut ? QStringLiteral("<s><font color=\"%1\">%2</font></s>")
                              .arg(tone.old.name(), inner)
                        : QStringLiteral("<font color=\"%1\"><b>%2</b></font>")
                              .arg(tone.added.name(), inner);
    }
    // Struck or bold as well as tinted, so that a reader who cannot tell the two tints apart
    // still tells the words apart.
    const QColor tint = takenOut ? tone.oldTint : tone.addedTint;
    return QStringLiteral("<span style=\"background-color:%1\">%2</span>")
        .arg(tint.name(),
             (takenOut ? QStringLiteral("<s>%1</s>") : QStringLiteral("<b>%1</b>")).arg(inner));
}

} // namespace

Tones tones(Colours colours, bool dark) {
    const Row &row = table[static_cast<size_t>(colours)][dark ? 1 : 0];
    return {QColor(row.old),     QColor(row.added), QColor(row.oldTint),   QColor(row.addedTint),
            QColor(row.pending), QColor(row.mixed), QColor(row.typoBadge), QColor(row.timingBadge)};
}

Colours colours() { return drawn; }

void useColours(Colours colours) { drawn = colours; }

QString coloursKey(Colours colours) {
    switch (colours) {
    case Colours::Default:
        return QStringLiteral("default");
    case Colours::Deuteran:
        return QStringLiteral("deuteran");
    case Colours::Protan:
        return QStringLiteral("protan");
    case Colours::Tritan:
        return QStringLiteral("tritan");
    case Colours::Monochrome:
        return QStringLiteral("monochrome");
    }
    return QStringLiteral("default");
}

Colours coloursNamed(const QString &key) {
    for (const Colours each : everyColours) {
        if (coloursKey(each) == key) {
            return each;
        }
    }
    return Colours::Default;
}

QString coloursLabel(Colours colours) {
    switch (colours) {
    case Colours::Default:
        return QStringLiteral("Default");
    case Colours::Deuteran:
        return QStringLiteral("Deuteranomaly/deuteranopia");
    case Colours::Protan:
        return QStringLiteral("Protanomaly/protanopia");
    case Colours::Tritan:
        return QStringLiteral("Tritanomaly/tritanopia");
    case Colours::Monochrome:
        return QStringLiteral("Little or no colour vision");
    }
    return QStringLiteral("Default");
}

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
        return {QStringLiteral("◐"), QStringLiteral("Mixed"), now().mixed};
    }
    const QString glyph = mixed ? QStringLiteral("◐") : QString();
    switch (*state) {
    case State::kPending:
        return {glyph.isEmpty() ? QStringLiteral("●") : glyph, stateName(*state), now().pending};
    case State::kAccepted:
        return {glyph.isEmpty() ? QStringLiteral("✓") : glyph, stateName(*state), now().added};
    case State::kRejected:
        return {glyph.isEmpty() ? QStringLiteral("✕") : glyph, stateName(*state), now().old};
    }
    return {QStringLiteral("?"), QStringLiteral("?"), QColor()};
}

QString kindWord(Check check) {
    return check == Check::kTiming ? QStringLiteral("Timing") : QStringLiteral("Typo");
}

QColor kindTone(Check check) {
    return check == Check::kTiming ? now().timingBadge : now().typoBadge;
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
    return marked(text, changed, takenOut, how, dark());
}

QString marked(const QString &text, const Span &changed, bool takenOut, Marking how, bool dark) {
    const auto start = static_cast<qsizetype>(changed.start);
    const auto end = static_cast<qsizetype>(changed.end);
    return part(text, 0, start) +
           mark(part(text, start, end), takenOut, how, tones(colours(), dark)) +
           part(text, end, text.size());
}

RowLine rowLine(const QString &original, const Span &originalChanged, const QString &wording,
                const Span &wordingChanged) {
    const auto start =
        std::clamp<qsizetype>(static_cast<qsizetype>(originalChanged.start), 0, original.size());
    const auto end = static_cast<qsizetype>(originalChanged.end);
    const Tones tone = now();
    const QString out = mark(part(original, start, end), true, Marking::InRow, tone);
    const QString in = mark(part(wording, static_cast<qsizetype>(wordingChanged.start),
                                 static_cast<qsizetype>(wordingChanged.end)),
                            false, Marking::InRow, tone);
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

QString changeLine(const QString &original, const Span &originalChanged, const QString &wording,
                   const Span &wordingChanged) {
    const Tones tone = now();
    const QString out = mark(part(original, static_cast<qsizetype>(originalChanged.start),
                                  static_cast<qsizetype>(originalChanged.end)),
                             true, Marking::InRow, tone);
    const QString in = mark(part(wording, static_cast<qsizetype>(wordingChanged.start),
                                 static_cast<qsizetype>(wordingChanged.end)),
                            false, Marking::InRow, tone);
    const QString arrow = QStringLiteral("→");
    return (out.isEmpty() ? arrow : out + QLatin1Char(' ') + arrow) +
           (in.isEmpty() ? QString() : QLatin1Char(' ') + in);
}

bool dark() {
    // A program with no GUI application has no palette to read: asking for one there reads a
    // QGuiApplication that is not one, and crashes.
    if (!qobject_cast<QGuiApplication *>(QCoreApplication::instance())) {
        return false;
    }
    return QGuiApplication::palette().color(QPalette::Window).lightness() < 128;
}
