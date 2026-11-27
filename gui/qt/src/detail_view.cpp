#include "detail_view.h"

#include "overloaded.h"
#include "words.h"

#include <QLocale>

#include <algorithm>
#include <variant>

using namespace subcheck_ffi;

namespace {

QStringList lines(const std::vector<std::string> &all) {
    QStringList out;
    for (const std::string &line : all) {
        out << text(line);
    }
    return out;
}

bool isTiming(const Detail &d) {
    return std::holds_alternative<Kind::kTiming>(d.kind.get_variant());
}

QString stretch(const Stretch &s) {
    const QString from = s.from_ms ? QStringLiteral("from %1").arg(exactClock(*s.from_ms))
                                   : QStringLiteral("from the start");
    const QString to =
        s.to_ms ? QStringLiteral("to %1").arg(exactClock(*s.to_ms)) : QStringLiteral("to the end");
    QString said = framesOff(s.frames);
    said[0] = said[0].toUpper();
    return QStringLiteral("%1, %2 %3").arg(said, from, to);
}

QString member(const Member &m) {
    if (!m.fix) {
        return text(m.label);
    }
    return QStringLiteral("%1 (%2)").arg(text(m.label), fixWords(*m.fix));
}

QString markWords(const Mark &mark) {
    return std::visit(
        Overloaded{
            [](const Mark::kByHand &m) {
                return QStringLiteral("Left for a fix by hand: %1").arg(text(m.why));
            },
            [](const Mark::kNotFound &) { return QStringLiteral("Not found in the file"); },
            [](const Mark::kWouldApply &) { return QStringLiteral("The preview would apply it"); },
            [](const Mark::kAlreadyApplied &) {
                return QStringLiteral("The preview found it in the file already");
            },
            [](const Mark::kWouldBeByHand &m) {
                return QStringLiteral("The preview would leave it for a fix by hand: %1")
                    .arg(text(m.why));
            },
            [](const Mark::kWouldNotBeFound &) {
                return QStringLiteral("The preview would not find it in the file");
            },
            [](const Mark::kFailed &m) {
                return QStringLiteral("Applying it failed: %1").arg(text(m.why));
            },
            [](const Mark::kWouldFail &m) {
                return QStringLiteral("The preview failed: %1").arg(text(m.why));
            },
            [](const Mark::kApplied &) {
                return QStringLiteral("Applied: the fix is in the file");
            },
        },
        mark.get_variant());
}

} // namespace

void DetailView::show(const std::optional<Detail> &now) {
    detail = now;
    emit changed();
}

std::string DetailView::file() const { return detail ? detail->file : std::string(); }

bool DetailView::isOf(const std::string &label) const {
    if (!detail) {
        return false;
    }
    if (detail->label == label) {
        return true;
    }
    return detail->group &&
           std::any_of(detail->group->members.begin(), detail->group->members.end(),
                       [&label](const Member &m) { return m.label == label; });
}

bool DetailView::shown() const { return detail.has_value(); }

QString DetailView::kind() const {
    return detail ? kindWord(isTiming(*detail) ? Check::kTiming : Check::kLlm) : QString();
}

QColor DetailView::kindTone() const {
    return detail ? ::kindTone(isTiming(*detail) ? Check::kTiming : Check::kLlm) : QColor();
}

QString DetailView::place() const {
    if (!detail) {
        return {};
    }
    if (!detail->at_ms) {
        return text(detail->label);
    }
    return QStringLiteral("%1 · %2").arg(text(detail->label), clock(*detail->at_ms));
}

QString DetailView::glyph() const {
    return detail ? stateLook(detail->state, false).glyph : QString();
}

QString DetailView::stateWord() const {
    return detail ? stateLook(detail->state, false).word : QString();
}

QColor DetailView::stateTone() const {
    return detail ? stateLook(detail->state, false).tone : QColor();
}

QStringList DetailView::glyphs() const {
    QStringList each;
    for (const State state : {State::kPending, State::kAccepted, State::kRejected}) {
        each << stateLook(state, false).glyph;
    }
    return each;
}

QColor DetailView::oldTone() const { return tones(colours(), dark()).old; }

QColor DetailView::newTone() const { return tones(colours(), dark()).added; }

bool DetailView::edited() const { return detail && detail->edited; }

QString DetailView::now() const {
    return detail ? marked(text(detail->original), detail->original_changed, true, Marking::InPanel)
                  : QString();
}

QString DetailView::fixed() const {
    return detail ? marked(text(detail->wording), detail->wording_changed, false, Marking::InPanel)
                  : QString();
}

QString DetailView::reason() const { return detail ? text(detail->reason) : QString(); }

QStringList DetailView::before() const {
    if (const auto *line =
            detail ? std::get_if<Kind::kLine>(&detail->kind.get_variant()) : nullptr) {
        return lines(line->before);
    }
    return {};
}

QStringList DetailView::after() const {
    if (const auto *line =
            detail ? std::get_if<Kind::kLine>(&detail->kind.get_variant()) : nullptr) {
        return lines(line->after);
    }
    return {};
}

QString DetailView::episodes() const {
    if (!detail || !detail->group) {
        return {};
    }
    QStringList each;
    for (const Member &m : detail->group->members) {
        each << member(m);
    }
    // Places and not episodes: a row can hold more than one finding of a file. Those the filter
    // hides count too, so that the panel and the row say one number.
    QString said = QStringLiteral("In %1: %2")
                       .arg(counted(static_cast<uint64_t>(each.size()) + detail->group->hidden,
                                    QStringLiteral("place"), QStringLiteral("places")),
                            each.join(QStringLiteral(", ")));
    if (detail->group->hidden > 0) {
        said += QStringLiteral("; %1 more the filter hides").arg(detail->group->hidden);
    }
    return said;
}

QStringList DetailView::timing() const {
    const auto *t = detail ? std::get_if<Kind::kTiming>(&detail->kind.get_variant()) : nullptr;
    if (!t) {
        return {};
    }
    QStringList out;
    for (const Stretch &s : t->stretches) {
        out << stretch(s);
    }
    if (!t->unshifted.empty()) {
        out << QStringLiteral("%1 %2").arg(
            counted(t->unshifted.size(), QStringLiteral("line"), QStringLiteral("lines")),
            t->unshifted.size() == 1 ? QStringLiteral("stays where it is")
                                     : QStringLiteral("stay where they are"));
    }
    if (t->opens) {
        out << QStringLiteral("Show in Video opens it at %1, %2")
                   .arg(exactClock(t->opens->at_ms),
                        t->opens->on_frame
                            ? QStringLiteral("on the frame that shows the error")
                            : QStringLiteral("on the first line of a stretch that moves"));
    }
    return out;
}

QStringList DetailView::banners() const {
    if (!detail) {
        return {};
    }
    QStringList out;
    if (detail->note) {
        out << (*detail->note == FileNote::kChanged
                    ? QStringLiteral("This episode changed since it was checked")
                    : QStringLiteral("This episode's file is missing"));
    }
    if (detail->checking) {
        out << (std::holds_alternative<Checking::kQueued>(detail->checking->get_variant())
                    ? QStringLiteral("Waiting to be checked again")
                    : QStringLiteral("Being checked again"));
    }
    if (detail->truncated) {
        // The C locale's, as the window's other words are English whatever the desktop's language.
        const QLocale c = QLocale::c();
        out << QStringLiteral("Checked from a copy cut short: %1 of %2")
                   .arg(c.formattedDataSize(static_cast<qint64>(detail->truncated->file_size), 1),
                        c.formattedDataSize(static_cast<qint64>(detail->truncated->declared_end),
                                            1));
    }
    return out;
}

QString DetailView::mark() const {
    return detail && detail->mark ? markWords(*detail->mark) : QString();
}

QStringList DetailView::technical() const {
    if (!detail) {
        return {};
    }
    QStringList out{QStringLiteral("File: %1").arg(text(detail->file))};
    if (const auto &t = detail->track) {
        QString said = QStringLiteral("Track %1, sid %2").arg(t->id).arg(t->sid);
        if (t->name) {
            said += QStringLiteral(": %1").arg(text(*t->name));
        }
        if (!t->language.empty()) {
            said += QStringLiteral(" (%1)").arg(text(t->language));
        }
        out << said;
    }
    if (detail->at_ms) {
        out << QStringLiteral("Starts at %1").arg(exactClock(*detail->at_ms));
    }
    if (detail->more > 0) {
        out << QStringLiteral("Also in %1")
                   .arg(counted(detail->more, QStringLiteral("other place"),
                                QStringLiteral("other places")));
    }
    return out;
}
