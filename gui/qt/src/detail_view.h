// What the details panel shows of the selected finding, as the window words it: review's `Detail`
// laid out in the window's words, each a property QML binds to.
#pragma once

#include "subcheck_ffi.hpp"

#include <QColor>
#include <QObject>
#include <QString>
#include <QStringList>
#include <QtQml/qqmlregistration.h>

#include <optional>

class DetailView : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("the session's bridge makes the one details panel")
    // Whether a finding is selected: none, and the panel shows nothing of one.
    Q_PROPERTY(bool shown READ shown NOTIFY changed)
    // Its badge and the badge's colour, where it is, and its decision, the selected finding's own.
    Q_PROPERTY(QString kind READ kind NOTIFY changed)
    Q_PROPERTY(QColor kindTone READ kindTone NOTIFY changed)
    Q_PROPERTY(QString place READ place NOTIFY changed)
    Q_PROPERTY(QString glyph READ glyph NOTIFY changed)
    Q_PROPERTY(QString stateWord READ stateWord NOTIFY changed)
    Q_PROPERTY(QColor stateTone READ stateTone NOTIFY changed)
    // The palette's tones for the words a change takes out and puts in, which the panel's blocks
    // draw their bars and labels in.
    Q_PROPERTY(QColor oldTone READ oldTone NOTIFY changed)
    Q_PROPERTY(QColor newTone READ newTone NOTIFY changed)
    // The wording is the person's own, not the suggestion.
    Q_PROPERTY(bool edited READ edited NOTIFY changed)
    // The line as the subtitles have it now, and with the fix, each with its change marked.
    Q_PROPERTY(QString now READ now NOTIFY changed)
    Q_PROPERTY(QString fixed READ fixed NOTIFY changed)
    // Why it was found: empty when nothing says.
    Q_PROPERTY(QString reason READ reason NOTIFY changed)
    // A line's neighbours, in time order; none for a timing finding.
    Q_PROPERTY(QStringList before READ before NOTIFY changed)
    Q_PROPERTY(QStringList after READ after NOTIFY changed)
    // A row of several: how many places it is in, the episode of each, and how many the filter
    // hides; empty for a finding alone.
    Q_PROPERTY(QString episodes READ episodes NOTIFY changed)
    // A timing finding's stretches, the lines its fix leaves, and where Show in Video opens it.
    Q_PROPERTY(QStringList timing READ timing NOTIFY changed)
    // What sets its file apart now: changed or missing since the check, being checked again, or
    // checked from a copy cut short.
    Q_PROPERTY(QStringList banners READ banners NOTIFY changed)
    // What apply did, or would do, with its fix; empty when it has said nothing.
    Q_PROPERTY(QString mark READ mark NOTIFY changed)
    // Its file, track and time, for whoever wants them.
    Q_PROPERTY(QStringList technical READ technical NOTIFY changed)

public:
    using QObject::QObject;

    // The detail review sent last, none when nothing is selected.
    void show(const std::optional<subcheck_ffi::Detail> &detail);
    // Its file as the store names it; empty when nothing is selected.
    std::string file() const;
    // Whether the episode the list calls `label` is the selected finding's, or one of its row's.
    bool isOf(const std::string &label) const;

    bool shown() const;
    QString kind() const;
    QColor kindTone() const;
    QString place() const;
    QString glyph() const;
    QString stateWord() const;
    QColor stateTone() const;
    QColor oldTone() const;
    QColor newTone() const;
    bool edited() const;
    QString now() const;
    QString fixed() const;
    QString reason() const;
    QStringList before() const;
    QStringList after() const;
    QString episodes() const;
    QStringList timing() const;
    QStringList banners() const;
    QString mark() const;
    QStringList technical() const;

signals:
    // What it shows changed: another detail, or the same in another palette.
    void changed();

private:
    std::optional<subcheck_ffi::Detail> detail;
};
