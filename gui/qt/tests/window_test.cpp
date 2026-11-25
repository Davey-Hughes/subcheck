// The real window, its QML loaded from the module as the app loads it, offscreen, on real sessions
// over the runs seeded for it: keys pressed, menu items chosen and rows, entries and buttons
// clicked as a person would, through the QML's handlers to review and back. Every test leaves the
// runs as it found them. Its video pane makes a real mpv, which offscreen can never show video:
// the player's own tests run it on a fake. Its settings are in a directory each window makes and
// removes, never the user's.
#include "key_names.h"
#include "menu_keys.h"
#include "mpv_item.h"
#include "offscreen_main.h"
#include "press_away.h"
#include "said.h"
#include "told.h"
#include "window_settings.h"
#include "words.h"

#include <QAccessible>
#include <QClipboard>
#include <QDir>
#include <QElapsedTimer>
#include <QFile>
#include <QFileInfo>
#include <QFont>
#include <QFontInfo>
#include <QFontMetricsF>
#include <QKeyEvent>
#include <QKeySequence>
#include <QPointingDevice>
#include <QQmlApplicationEngine>
#include <QQmlComponent>
#include <QQmlEngine>
#include <QQuickItem>
#include <QQuickWindow>
#include <QRegularExpression>
#include <QScopeGuard>
#include <QScreen>
#include <QSignalSpy>
#include <QStandardPaths>
#include <QStyleHints>
#include <QTemporaryDir>
#include <QTest>
#include <QtMath>
#include <QtQml/QQmlExtensionPlugin>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <functional>
#include <memory>
#include <optional>
#include <sstream>
#include <string>
#include <tuple>
#include <utility>
#include <variant>
#include <vector>

Q_IMPORT_QML_PLUGIN(SubcheckPlugin)

namespace {

std::string media() { return qEnvironmentVariable("SUBCHECK_TEST_MEDIA").toStdString(); }

std::string clean() { return qEnvironmentVariable("SUBCHECK_TEST_CLEAN").toStdString(); }

// The message handler quietOffscreen was installed over, which every message but the three it
// drops goes on to, and whether it was installed, so that the end of the run puts that handler
// back.
QtMessageHandler testsOwn = nullptr;
bool quieted = false;

// The offscreen platform can neither raise a window, pass a window's least size on nor put a
// window in another, and says so each time Settings is raised or its contents grow and each time
// the details panel takes the video's window in or lets it go: nothing here rests on any of them.
// With no handler beneath it, a message goes to stderr, as Qt's own handler would write it.
void quietOffscreen(QtMsgType type, const QMessageLogContext &context, const QString &message) {
    if (message == QLatin1String("This plugin does not support raise()") ||
        message == QLatin1String("This plugin does not support propagateSizeHints()") ||
        message == QLatin1String("This plugin does not support setParent!")) {
        return;
    }
    if (testsOwn) {
        testsOwn(type, context, message);
    } else {
        std::fprintf(stderr, "%s\n", qPrintable(qFormatLogMessage(type, context, message)));
    }
}

// More reviews than the sidebar has room for, each with `count` findings.
std::vector<subcheck_ffi::FolderView> manyReviews(uint64_t count = 0) {
    std::vector<subcheck_ffi::FolderView> folders;
    for (int i = 0; i < 30; ++i) {
        const std::string name = "Show " + std::to_string(i);
        folders.push_back(subcheck_ffi::FolderView{"/m/" + name, name, count, false});
    }
    return folders;
}

// More episodes than the sidebar has room for at the window's least height: E00 to E39, of which
// the first twenty have no findings.
std::vector<subcheck_ffi::FileView> manyEpisodes() {
    std::vector<subcheck_ffi::FileView> files;
    for (int i = 0; i < 40; ++i) {
        const std::string label = (i < 10 ? "E0" : "E") + std::to_string(i);
        const uint64_t findings = i < 20 ? 0 : 2;
        files.push_back(subcheck_ffi::FileView{
            .path = "/m/S1/" + label + ".mkv",
            .label = label,
            .stage = subcheck_ffi::ScanStage(subcheck_ffi::ScanStage::kDone{findings}),
            .applying = std::nullopt,
            .lines = 400,
            .timing_only = false,
        });
    }
    return files;
}

// A finding with every part the details panel draws, each unlike the others.
subcheck_ffi::Detail everyPartOfAFinding() {
    using namespace subcheck_ffi;
    return Detail{
        .file = "/m/S1/E03.mkv",
        .label = "E03",
        .group = Group{{{"E03", std::nullopt}, {"E04", std::nullopt}}, 1},
        .at_ms = 754'450,
        .track = Track{3, 2, "Full", "en"},
        .more = 2,
        .note = FileNote::kChanged,
        .checking = Checking(Checking::kQueued{}),
        .truncated = Truncated{2'000'000, 1'000'000},
        .state = State::kRejected,
        .edited = false,
        .mark = Mark(Mark::kApplied{}),
        .original = "Your going",
        .wording = "You're going",
        .original_changed = Span{0, 4},
        .wording_changed = Span{0, 6},
        .reason = "your for you're",
        .kind = Kind(Kind::kLine{{"First before", "Second before"}, {"After"}}),
    };
}

// A row of 12 under `key`, 4 of them shown, with these fixes.
subcheck_ffi::EntryView rowOfTwelve(const subcheck_ffi::EntryKey &key,
                                    std::vector<subcheck_ffi::FixCount> fixes = {}) {
    return subcheck_ffi::EntryView{
        .key = key,
        .state = subcheck_ffi::State::kPending,
        .is_member = false,
        .expanded = false,
        .label = "E01",
        .shown = 4,
        .total = 12,
        .at_ms = 1000,
        .original = "befor",
        .wording = "before",
        .style = "Default",
        .marks = {},
        .track = std::nullopt,
        .fixes = std::move(fixes),
        .checking = false,
        .note = std::nullopt,
        .mixed = false,
        .check = subcheck_ffi::Check::kLlm,
        .original_changed = {0, 5},
        .wording_changed = {0, 6},
        .reason = "befor for before",
        .file = "/m/S1/E01.mkv",
        .editable = true,
    };
}

// The window over a bridge on `selector`, made as main.cpp makes it but for a short wait after a
// quit, as in the bridge's tests: the store is the test's own, so no lock held elsewhere slows
// review's end, and a review that does not end fails its test sooner. Its player gives an Open a
// long wait for the pane, which offscreen never shows video, so that a Show in Video still waits
// there however slow the runner. Its settings are kept in `settings`, a directory given, or else
// its own. Its bridge tells each action it sends, and holds them back from review once a test asks
// it to, so that a test sees what the window does before review answers. The window's other
// initial properties are `more`, given as main.cpp would give them.
struct Shown {
    std::ostringstream out;
    std::ostringstream err;
    PlayerPort player{std::chrono::seconds(1), std::chrono::seconds(2), std::chrono::minutes(1)};
    Holding bridge;
    QTemporaryDir own;
    WindowSettings settings;
    QQmlApplicationEngine engine;
    QQuickWindow *window = nullptr;

    explicit Shown(std::vector<std::string> selector, const QString &kept = {},
                   const QVariantMap &more = {})
        : bridge(std::move(selector), player, out, err, std::chrono::seconds(1)),
          settings(std::make_unique<QSettings>((kept.isEmpty() ? own.path() : kept) +
                                                   "/subcheck-qt.conf",
                                               QSettings::IniFormat),
                   QGuiApplication::platformName()) {
        QVariantMap properties{{QStringLiteral("bridge"), QVariant::fromValue(&bridge)},
                               {QStringLiteral("settings"), QVariant::fromValue(&settings)}};
        properties.insert(more);
        engine.setInitialProperties(properties);
        engine.loadFromModule("Subcheck", "Main");
        if (!engine.rootObjects().isEmpty()) {
            window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        }
    }

    // The window shown, as main.cpp shows it, and active, as a person's would be; the session not
    // started yet.
    bool show() {
        if (!window) {
            return false;
        }
        settings.show(window);
        return exposedAndActive();
    }

    // The window shown and the session started in main.cpp's order, review before the window's
    // first frame, so that what review says before then is covered too; then the window active.
    bool start() {
        if (!window) {
            return false;
        }
        settings.show(window);
        bridge.start();
        return exposedAndActive();
    }

    QObject *find(const char *name) const { return window->findChild<QObject *>(name); }

    // The item called `name` in what the window draws, a Repeater's among them, which are no
    // QObject's children.
    QQuickItem *item(const char *name) const { return item(window->contentItem(), name); }

    QQuickWindow *settingsWindow() const {
        return qobject_cast<QQuickWindow *>(find("settingsWindow"));
    }
    // The item called `name` in the Settings window, which is not in the main window's scene.
    QQuickItem *settingsItem(const char *name) const {
        QQuickWindow *settings = settingsWindow();
        return settings ? item(settings->contentItem(), name) : nullptr;
    }

    // The video's own window, which the details panel holds, or which is out on its own. Out, it is
    // no child of the main window, which it is while held and where it is never held: the pane
    // drawn in it names it then.
    QQuickWindow *videoWindow() const {
        if (auto *child = qobject_cast<QQuickWindow *>(find("videoWindow"))) {
            return child;
        }
        QQuickItem *pane = videoItem("videoPane");
        return pane && pane->window() != window ? pane->window() : nullptr;
    }
    // The item called `name` in the video's pane, whichever window draws it.
    QQuickItem *videoItem(const char *name) const {
        auto *pane = qobject_cast<QQuickItem *>(find("videoPane"));
        return pane ? item(pane, name) : nullptr;
    }

    static QQuickItem *item(QQuickItem *in, const char *name) {
        if (in->objectName() == QLatin1String(name)) {
            return in;
        }
        const QList<QQuickItem *> children = in->childItems();
        for (QQuickItem *child : children) {
            if (QQuickItem *found = item(child, name)) {
                return found;
            }
        }
        return nullptr;
    }

    // The text the label called `name` in `in` draws.
    static QString text(QQuickItem *in, const char *name) {
        QQuickItem *label = item(in, name);
        return label ? label->property("text").toString() : QStringLiteral("(no %1)").arg(name);
    }

    // The lines a list of labels draws, in order: its Repeater, among its children, has no text.
    static QStringList lines(QQuickItem *of) {
        QStringList drawn;
        const QList<QQuickItem *> children = of->childItems();
        for (QQuickItem *child : children) {
            const QVariant line = child->property("text");
            if (line.isValid()) {
                drawn << line.toString();
            }
        }
        return drawn;
    }

    // The button in `in` that says `words`.
    static QQuickItem *button(QQuickItem *in, const QString &words) {
        if (in->inherits("QQuickAbstractButton") && in->property("text").toString() == words) {
            return in;
        }
        const QList<QQuickItem *> children = in->childItems();
        for (QQuickItem *child : children) {
            if (QQuickItem *found = button(child, words)) {
                return found;
            }
        }
        return nullptr;
    }

    // The menu of the bar, or under one of its menus, with the title `title`.
    QObject *menu(const char *title) const {
        QObject *bar = window->property("menuBar").value<QObject *>();
        for (QObject *each : bar->findChildren<QObject *>()) {
            if (each->inherits("QQuickMenu") && each->property("title").toString() == title) {
                return each;
            }
        }
        return nullptr;
    }

    // The entry of the bar that opens its menu at `index`.
    QQuickItem *barEntry(int index) const {
        QObject *bar = window->property("menuBar").value<QObject *>();
        QQuickItem *entry = nullptr;
        QMetaObject::invokeMethod(bar, "itemAt", Q_RETURN_ARG(QQuickItem *, entry),
                                  Q_ARG(int, index));
        return entry;
    }

    int rows() { return bridge.findings()->rowCount(); }

    QString decision(int row) {
        FindingsModel *list = bridge.findings();
        if (row >= list->rowCount()) {
            return {};
        }
        return list->data(list->index(row), FindingsModel::StateWordRole).toString();
    }

    // The delegate showing `row`, wherever the list has put it: never the highlight, which is the
    // list's child too, with no index of its own.
    QQuickItem *delegate(int row) const {
        auto *content = find("findings")->property("contentItem").value<QQuickItem *>();
        const QList<QQuickItem *> items = content->childItems();
        for (QQuickItem *each : items) {
            const QVariant index = each->property("index");
            if (index.isValid() && index.toInt() == row) {
                return each;
            }
        }
        return nullptr;
    }

    QPoint middle(QQuickItem *of) const {
        return of->mapToScene(QPointF(of->width() / 2, of->height() / 2)).toPoint();
    }

    // In the window the item is drawn in: an item of a menu Qt draws in a popup window of its own,
    // the right-click menu's among them, takes its clicks in that window.
    void click(QQuickItem *of) { click(of->window(), of); }
    void click(QQuickWindow *in, QQuickItem *of) {
        QTest::mouseClick(in, Qt::LeftButton, {}, middle(of));
    }
    void rightClick(QQuickItem *of) { QTest::mouseClick(window, Qt::RightButton, {}, middle(of)); }

    // The middle of the handle between the column `left` ends and the one `right` starts.
    QPoint handle(QQuickItem *left, QQuickItem *right) const {
        const qreal end = left->mapToScene(QPointF(left->width(), 0)).x();
        const qreal start = right->mapToScene(QPointF(0, 0)).x();
        return {qRound((end + start) / 2), window->height() / 2};
    }

    // That handle dragged `by` across.
    void drag(QQuickItem *left, QQuickItem *right, int by) {
        const QPoint at = handle(left, right);
        QTest::mousePress(window, Qt::LeftButton, {}, at);
        QTest::mouseMove(window, at + QPoint(by / 3, 0));
        QTest::mouseMove(window, at + QPoint(by, 0));
        QTest::mouseRelease(window, Qt::LeftButton, {}, at + QPoint(by, 0));
    }

    // The actions sent since `from`, the count of those sent before; and the last, none before
    // the first.
    QStringList since(qsizetype from) const { return bridge.sent.mid(from); }
    QString last() const { return bridge.sent.isEmpty() ? QString() : bridge.sent.last(); }

private:
    // The window shown waited for until it is exposed, then made active, as a person's would be.
    bool exposedAndActive() {
        if (!QTest::qWaitForWindowExposed(window)) {
            return false;
        }
        window->requestActivate();
        return QTest::qWaitForWindowActive(window);
    }
};

// The file of the episode the check found nothing in, in a view of the run with no findings and
// the run with two episodes: first in review's order, as its folder's path sorts first, so the file
// review selects as that view opens.
QString noFindingsFile() { return QString::fromStdString(clean()) + "/typos.mkv"; }

// The details panel's view of what it scrolls.
QQuickItem *detailsView(const Shown &w) {
    return w.item("detailsScroll")->property("contentItem").value<QQuickItem *>();
}

// Where `item` starts in the details panel's view, and whether it lies wholly inside it.
qreal topInDetails(const Shown &w, QQuickItem *item) {
    return item->mapToItem(detailsView(w), QPointF(0, 0)).y();
}
bool inDetailsView(const Shown &w, QQuickItem *item) {
    const qreal top = topInDetails(w, item);
    return top >= -0.5 && top + item->height() <= detailsView(w)->height() + 0.5;
}

// What keeps the details panel's block with the fix from being laid out as the window draws it
// next, the panel `width` wide, nothing once it is: the block as wide as the column it is in, and
// its line inside it. A change of the text's size or the window's moves the line's own size at
// once, and the panel and the block only as the window lays out what changed, before its next
// frame.
QString fixNotLaidOut(const Shown &w, qreal width) {
    QQuickItem *panel = w.item("details");
    QQuickItem *fixed = w.item("fixedBlock");
    QQuickItem *line = Shown::item(fixed, "blockLine");
    const QRectF drawn = line->mapRectToItem(fixed, line->boundingRect());
    QStringList off;
    if (qAbs(panel->width() - width) >= 0.5) {
        off << QStringLiteral("the panel is %1 wide, not %2").arg(panel->width()).arg(width);
    }
    if (qAbs(fixed->width() - fixed->parentItem()->width()) >= 0.5) {
        off << QStringLiteral("the block is %1 wide in a column %2 wide")
                   .arg(fixed->width())
                   .arg(fixed->parentItem()->width());
    }
    if (!fixed->boundingRect().adjusted(-0.5, -0.5, 0.5, 0.5).contains(drawn)) {
        off << QStringLiteral("the line is at %1,%2 %3x%4 in a block %5x%6")
                   .arg(drawn.x())
                   .arg(drawn.y())
                   .arg(drawn.width())
                   .arg(drawn.height())
                   .arg(fixed->width())
                   .arg(fixed->height());
    }
    return off.join(QStringLiteral("; "));
}

// The fix's line drawn on one line: the details panel is given one fixed width at once, the
// window with it (`roomForOneLine`, which callers pass to fixNotLaidOut), an allowance that the
// wait below checks and that Row 0's fix fits in any font this suite meets, so that what a test
// says of a line of one never counts on a font. The window's height grows with the video above the
// line, which is as tall as the panel is wide times 9/16, so the details' view keeps the room it
// had. Fails the test, saying what it could not get, when the panel does not take the width or the
// line is not drawn on one line at it.
constexpr int roomForOneLine = 900;

void askRoomForOneLine(Shown &w, QQuickItem *line) {
    constexpr int asked = roomForOneLine;
    const QFont font = line->property("font").value<QFont>();
    const auto drawnLines = [line, font] {
        return qRound(line->property("contentHeight").toReal() / QFontMetricsF(font).lineSpacing());
    };
    const int videoGrowth = qMax(0, qCeil((asked - w.item("details")->width()) * 9.0 / 16.0));
    w.window->resize(w.settings.sidebarWidth() + asked + w.window->property("listLeast").toInt() +
                         w.window->property("handles").toInt(),
                     w.window->height() + videoGrowth);
    w.settings.keepColumns(w.settings.sidebarWidth(), asked);
    QTRY_VERIFY2(fixNotLaidOut(w, asked).isEmpty() && drawnLines() == 1,
                 qPrintable(QStringLiteral("asked for a panel %1 wide; it is %2 wide and draws the "
                                           "line in %3 lines; %4")
                                .arg(asked)
                                .arg(w.item("details")->width())
                                .arg(drawnLines())
                                .arg(fixNotLaidOut(w, asked))));
}

// The entry `list` draws for its row `row`; none while it draws none.
QQuickItem *drawnEntry(QQuickItem *list, int row) {
    QQuickItem *each = nullptr;
    QMetaObject::invokeMethod(list, "itemAtIndex", Q_RETURN_ARG(QQuickItem *, each),
                              Q_ARG(int, row));
    return each;
}

// Where `entry` starts in `list`'s view, and whether it lies wholly inside it.
qreal topInList(QQuickItem *list, QQuickItem *entry) {
    return entry->mapToItem(list, QPointF(0, 0)).y();
}
bool inList(QQuickItem *list, QQuickItem *entry) {
    if (!entry) {
        return false;
    }
    const qreal top = topInList(list, entry);
    return top >= -0.5 && top + entry->height() <= list->height() + 0.5;
}

// The part called `name` of the details panel's block with the fix, where its edit opens: the field
// and its buttons, which the block above it has too, never shown.
QQuickItem *inEdit(const Shown &w, const char *name) {
    QQuickItem *fixed = w.item("fixedBlock");
    return fixed ? Shown::item(fixed, name) : nullptr;
}

// `text` typed a key at a time into what has the window's keys.
void typeIn(const Shown &w, const char *text) {
    for (const char *key = text; *key; ++key) {
        QTest::keyClick(w.window, *key);
    }
}

// The colour of the window palette's `role`.
QColor paletteColour(const Shown &w, const char *role) {
    return w.window->property("palette").value<QObject *>()->property(role).value<QColor>();
}

// The outline of the findings list's highlight, which marks the selected row.
QColor selectionOutline(const Shown &w) {
    auto *selection = w.find("findings")->property("highlightItem").value<QQuickItem *>();
    auto *border = selection ? selection->property("border").value<QObject *>() : nullptr;
    return border ? border->property("color").value<QColor>() : QColor();
}

// Counts the key events that reach the object it watches. Installed before another filter, it
// hears only what that filter lets through, as Qt asks the filter installed last first.
struct HeardKeys : QObject {
    int heard = 0;

    bool eventFilter(QObject *, QEvent *event) override {
        if (event->type() == QEvent::ShortcutOverride || event->type() == QEvent::KeyPress) {
            ++heard;
        }
        return false;
    }
};

// The style `row` is drawn in, told by a part only that style draws: Compact's glyph, or Rich's
// tile.
QString drawnStyle(QQuickItem *row) {
    if (Shown::item(row, "rowGlyph")) {
        return QStringLiteral("compact");
    }
    QQuickItem *tile = Shown::item(row, "rowTile");
    return tile && tile->isVisible() ? QStringLiteral("rich") : QStringLiteral("twoLines");
}

// The style of each row the list draws now.
QStringList drawnStyles(const Shown &w) {
    auto *content = w.find("findings")->property("contentItem").value<QQuickItem *>();
    QStringList styles;
    const QList<QQuickItem *> rows = content->childItems();
    for (QQuickItem *row : rows) {
        if (row->property("index").isValid() && row->isVisible()) {
            styles << drawnStyle(row);
        }
    }
    return styles;
}

// The names a screen reader is told of under `in`, in the order of its tree.
QStringList accessibleNames(QAccessibleInterface *in) {
    QStringList names;
    for (int i = 0; in && i < in->childCount(); ++i) {
        QAccessibleInterface *child = in->child(i);
        if (child) {
            names << child->text(QAccessible::Name) << accessibleNames(child);
        }
    }
    return names;
}

// The View menu's Rows items checked now, by name.
QStringList checkedRowStyles(const Shown &w) {
    QStringList checked;
    for (const char *name : {"rowsCompact", "rowsTwoLines", "rowsRich"}) {
        QObject *action = w.find(name);
        if (!action) {
            checked << QStringLiteral("(no %1)").arg(QLatin1String(name));
        } else if (action->property("checked").toBool()) {
            checked << QString::fromLatin1(name);
        }
    }
    return checked;
}

// What `menu` holds, in order: each item's words, and "-" for a separator.
QStringList menuWords(QObject *menu) {
    QStringList words;
    const int count = menu->property("count").toInt();
    for (int i = 0; i < count; ++i) {
        QQuickItem *each = nullptr;
        QMetaObject::invokeMethod(menu, "itemAt", Q_RETURN_ARG(QQuickItem *, each), Q_ARG(int, i));
        if (!each) {
            words << QStringLiteral("(no item %1)").arg(i);
        } else if (each->inherits("QQuickMenuSeparator")) {
            words << QStringLiteral("-");
        } else {
            words << each->property("text").toString();
        }
    }
    return words;
}

// What keeps a person from reading `item` of a menu whole, said of each of its words: a text cut
// short, one running past the item's left or right edge, two running into each other, a text over
// the item's check box, and a shortcut of its action's that it does not show, as Fusion hides one
// it has no room for, where the platform shows shortcuts in menus. Empty for an item drawn whole.
QStringList notWhole(QQuickItem *item) {
    QList<QQuickItem *> texts;
    const std::function<void(QQuickItem *)> collect = [&texts, &collect](QQuickItem *in) {
        const QList<QQuickItem *> children = in->childItems();
        for (QQuickItem *child : children) {
            if (child->isVisible() && child->inherits("QQuickText") &&
                !child->property("text").toString().isEmpty()) {
                texts << child;
            }
            collect(child);
        }
    };
    collect(item);
    // Where `part` draws across the item: for a text, its words, which run out of its box where the
    // box is narrower than they are, to the box's left for words aligned to its right.
    const auto across = [item](QQuickItem *part) {
        return part->mapRectToItem(item, part->boundingRect());
    };
    const auto meet = [](const QRectF &one, const QRectF &other) {
        return one.left() < other.right() - 0.5 && other.left() < one.right() - 0.5;
    };
    QStringList wrong;
    auto *box = item->property("indicator").value<QQuickItem *>();
    for (qsizetype i = 0; i < texts.size(); ++i) {
        const QString words = texts[i]->property("text").toString();
        const QRectF drawn = across(texts[i]);
        if (texts[i]->property("truncated").toBool()) {
            wrong << QStringLiteral("\"%1\" is cut short").arg(words);
        }
        if (drawn.left() < -0.5 || drawn.right() > item->width() + 0.5) {
            wrong << QStringLiteral("\"%1\" runs past the item's edge").arg(words);
        }
        for (qsizetype j = i + 1; j < texts.size(); ++j) {
            if (meet(drawn, across(texts[j]))) {
                wrong << QStringLiteral("\"%1\" runs into \"%2\"")
                             .arg(words, texts[j]->property("text").toString());
            }
        }
        if (box && box->isVisible() && meet(drawn, across(box))) {
            wrong << QStringLiteral("\"%1\" runs over the check box").arg(words);
        }
    }
    auto *action = item->property("action").value<QObject *>();
    const QString keys = action ? action->property("shortcut").toString() : QString();
    if (!keys.isEmpty() && QGuiApplication::styleHints()->showShortcutsInContextMenus()) {
        const QString shown = QKeySequence(keys).toString(QKeySequence::NativeText);
        if (std::none_of(texts.begin(), texts.end(), [&shown](QQuickItem *text) {
                return text->property("text").toString() == shown;
            })) {
            wrong << QStringLiteral("its shortcut \"%1\" is not shown").arg(shown);
        }
    }
    return wrong;
}

// What keeps a person from reading each item of `menu` but its separators whole, each said with
// the menu and the item.
QStringList itemsNotWhole(QObject *menu) {
    const QString title = menu->property("title").toString();
    const QString name = title.isEmpty() ? menu->objectName() : title;
    QStringList wrong;
    const int count = menu->property("count").toInt();
    for (int i = 0; i < count; ++i) {
        QQuickItem *each = nullptr;
        QMetaObject::invokeMethod(menu, "itemAt", Q_RETURN_ARG(QQuickItem *, each), Q_ARG(int, i));
        if (!each) {
            wrong << QStringLiteral("%1: no item %2").arg(name).arg(i);
        } else if (!each->inherits("QQuickMenuSeparator")) {
            const QString words = each->property("text").toString();
            const QStringList found = notWhole(each);
            for (const QString &what : found) {
                wrong << QStringLiteral("%1 ▸ %2: %3").arg(name, words, what);
            }
        }
    }
    return wrong;
}

// The popup windows open, none once every one is closed. Qt hands each press to the popup window
// open last, and Qt Quick's popup window takes a press on the menu bar as one outside it: it
// closes and keeps the press from the bar, whose menu then never opens.
QString popupWindowsOpen() {
    QStringList open;
    const QWindowList windows = QGuiApplication::allWindows();
    for (QWindow *each : windows) {
        if (each->type() == Qt::Popup && each->isVisible()) {
            open << QString::fromLatin1(each->metaObject()->className());
        }
    }
    return open.join(QStringLiteral(", "));
}

// The rect Qt hands Wayland to place a menu it draws in a popup window of its own, below the rect's
// bottom-left corner, whatever the menu's place in its parent: the parent item's rect in the
// window, moved right by the menu's overlap and narrowed by twice it, to no less than a quarter of
// its width. It follows Qt 6.12's QQuickPopupWindow::parentControlGeometry, which is private and
// can change in any release, so a newer Qt needs it checked against that function again.
QRect waylandAnchor(QObject *menu) {
    auto *parent = menu->property("parent").value<QQuickItem *>();
    const qreal overlap = menu->property("overlap").toReal();
    const QRectF bounds = parent->boundingRect();
    const QPointF at = parent->mapToScene(bounds.topLeft());
    return {qFloor(at.x() + overlap), qFloor(at.y()),
            qCeil(qMax(qAbs(bounds.width() - overlap * 2), bounds.width() / 4)),
            qCeil(bounds.height())};
}

// The windows shown, the one with the keys and the bar's entries it takes as current: what a menu
// that did not open met.
QString menusMet(const Shown &w) {
    QStringList shown;
    const QWindowList windows = QGuiApplication::allWindows();
    for (QWindow *each : windows) {
        if (each->isVisible()) {
            shown << QStringLiteral("%1 (type %2)")
                         .arg(QString::fromLatin1(each->metaObject()->className()))
                         .arg(int(each->type()), 0, 16);
        }
    }
    QStringList current;
    for (int i = 0; i < 5; ++i) {
        QQuickItem *entry = w.barEntry(i);
        if (entry && entry->property("highlighted").toBool()) {
            current << entry->property("text").toString();
        }
    }
    QWindow *keys = QGuiApplication::focusWindow();
    return QStringLiteral("windows shown: %1; the keys: %2; the bar's current entry: %3")
        .arg(shown.join(QStringLiteral(", ")),
             keys ? QString::fromLatin1(keys->metaObject()->className()) : QStringLiteral("none"),
             current.join(QStringLiteral(", ")));
}

// The item of `menu` that opens its menu `sub`; none while it has none.
QQuickItem *subMenuItem(QObject *menu, QObject *sub) {
    const int count = menu->property("count").toInt();
    for (int i = 0; i < count; ++i) {
        QQuickItem *each = nullptr;
        QMetaObject::invokeMethod(menu, "itemAt", Q_RETURN_ARG(QQuickItem *, each), Q_ARG(int, i));
        if (each && each->property("subMenu").value<QObject *>() == sub) {
            return each;
        }
    }
    return nullptr;
}

// The action a click on the list's row at `row` sends: its Select.
QString selecting(Shown &w, int row) {
    const subcheck_ffi::EntryView *entry = w.bridge.findings()->at(row);
    return entry ? told(subcheck_ffi::Action(subcheck_ffi::Action::kSelect{entry->key}))
                 : QStringLiteral("(no row %1)").arg(row);
}

// The item called `name` of the right-click menu, open or not.
QQuickItem *rowMenuItem(const Shown &w, const char *name) {
    return qobject_cast<QQuickItem *>(w.find(name));
}

// A timing row made up in the selected row's place, drawn as review's next change would: its fix
// is accepted or rejected, never reworded. False if the list would not take it.
bool timingRowAtTheSelection(Shown &w) {
    subcheck_ffi::EntryView timing =
        rowOfTwelve(subcheck_ffi::EntryKey(subcheck_ffi::EntryKey::kRow{{500}}));
    timing.check = subcheck_ffi::Check::kTiming;
    timing.editable = false;
    if (!w.bridge.findings()->apply(
            subcheck_ffi::Splice{static_cast<uint64_t>(w.bridge.selected()), 1, {timing}})) {
        return false;
    }
    emit w.bridge.summaryChanged();
    return true;
}

// The window on that view at its least size, E01 clicked: its file selected and its first
// finding, so that what its check said shows below the finding, out of view. Review answers a
// click on the episode with no findings after this with a change, as it selects another file.
void clickE01AtTheLeastSize(Shown &w) {
    QVERIFY(w.start());
    QTRY_COMPARE(w.rows(), 2);
    w.window->resize(w.window->minimumSize());
    QTRY_COMPARE(w.window->height(), 480);
    QTRY_VERIFY(w.item("episode0") && w.item("episode1"));
    QCOMPARE(w.bridge.episodes()
                 ->data(w.bridge.episodes()->index(0, 0), EpisodesModel::PathRole)
                 .toString(),
             noFindingsFile());
    w.click(w.item("episode1"));
    QCOMPARE(w.bridge.sent,
             (QStringList{"SelectFile " + QString::fromStdString(media()) + "/typos - E01.mkv",
                          "ShowFileFindings"}));
    QTRY_COMPARE(w.bridge.episodeName(), "E01");
    QQuickItem *heading = w.item("episodeLogHeading");
    QTRY_VERIFY(heading->isVisible());
    QTRY_VERIFY(topInDetails(w, heading) >= detailsView(w)->height());
}

} // namespace

class WindowTest : public QObject {
    Q_OBJECT

    // As main.cpp, made after the application and before the first window (key_names.h).
    KeyNames keyNames;

private slots:
    void initTestCase() {
        // A test decides findings in whatever store it opens: without CTest's environment, that
        // would be the user's own.
        QVERIFY2(qEnvironmentVariableIsSet("SUBCHECK_DB") &&
                     qEnvironmentVariableIsSet("SUBCHECK_TEST_MEDIA"),
                 "run through ctest, which seeds a store of the test's own");
        // Its mpv reads mpvqt/mpvqt.conf in Qt's config location, which CTest moves into the build
        // directory, beside this test: on Linux by XDG_CONFIG_HOME, on macOS by the home
        // CoreFoundation takes from CFFIXED_USER_HOME. Should either not take, every test stops
        // here, before any makes an mpv, rather than run on the user's own file. The location and
        // the file are checked apart, so that a failure on a runner no one can step through names
        // the half to fix.
#ifdef Q_OS_MACOS
        const QString config =
            QCoreApplication::applicationDirPath() + "/test-home/Library/Preferences";
#else
        const QString config = QCoreApplication::applicationDirPath() + "/test-config";
#endif
        const QString location = QStandardPaths::writableLocation(QStandardPaths::ConfigLocation);
        QVERIFY2(QDir::cleanPath(location) == QDir::cleanPath(config) ||
                     QFileInfo(location).canonicalFilePath() ==
                         QFileInfo(config).canonicalFilePath(),
                 qPrintable("Qt's config location is " + location + ", not the test's " + config));
        const QString conf = config + "/mpvqt/mpvqt.conf";
        QVERIFY2(
            QFile::exists(conf),
            qPrintable("the test's mpvqt.conf, which CMake writes as it configures, is not at " +
                       conf));
        // The fixed style CTest gives every test, so that sizes here are alike on every machine;
        // and a screen larger than the window's default, which the offscreen platform's is not.
        QCOMPARE(qEnvironmentVariable("QT_QUICK_CONTROLS_STYLE"), "Fusion");
        QVERIFY2(QGuiApplication::primaryScreen()->availableSize() == QSize(1920, 1080),
                 "run through ctest, which gives this test a large screen");
        // As main.cpp, before the first window.
        QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL);
        // Only where the three messages it drops are the platform's.
        if (QGuiApplication::platformName() == QLatin1String("offscreen")) {
            testsOwn = qInstallMessageHandler(quietOffscreen);
            quieted = true;
        }
    }

    void cleanupTestCase() {
        if (quieted) {
            qInstallMessageHandler(testsOwn);
            quieted = false;
        }
    }

    void init() {
        // A warning from the QML at run time, a binding loop say, which qmllint cannot see, fails
        // the test that met it.
        QTest::failOnWarning(QRegularExpression(QStringLiteral("\\.qml:\\d+")));
    }

    void theKeysAndAClickDriveReview() {
        Shown w({media()});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *list = w.find("findings");
        QObject *status = w.find("statusLine");
        QObject *counts = w.find("counts");
        QVERIFY(list && status && counts);
        QObject *empty = w.find("empty");
        QVERIFY(empty && !empty->property("visible").toBool());
        QCOMPARE(counts->property("text").toString(), "4 to review · 0 accepted · 0 rejected");
        QCOMPARE(status->property("text").toString(), "");

        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 1);
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 4);
        QTest::keyClick(w.window, Qt::Key_K);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 0);
        QTest::keyClick(w.window, Qt::Key_A);
        QTRY_COMPARE(w.decision(0), "Accepted");
        QTRY_COMPARE(status->property("text").toString(), "Accepted 2 findings");
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.decision(0), "To review");
        QTest::keyClick(w.window, Qt::Key_R);
        QTRY_COMPARE(w.decision(0), "Rejected");
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.decision(0), "To review");

        // The last of the expanded rows' members.
        QTRY_VERIFY(w.delegate(3));
        QQuickItem *member = w.delegate(3);
        w.click(member);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 3);
        // Collapsed from its member, the row is selected, and the list follows the splice.
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 2);
        QCOMPARE(list->property("currentIndex").toInt(), 1);

        QTest::keyClick(w.window, Qt::Key_Q);
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 0);
        QCOMPARE(w.out.str(), "media: 0 accepted, 0 rejected, 4 pending\n");
    }

    // Keys pressed without waiting for the window to catch up land where the terminal's would: two
    // moves add up, and a move after an accept starts where the accept moved on to.
    void keysPressedFasterThanTheWindowCatchesUpLandAsTheTerminalsDo() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *list = w.find("findings");
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 1);
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 4);
        QTest::keyClick(w.window, Qt::Key_K);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 0);
        // Read once the move's answer is in, which lets the accept's message go; checked after
        // the undo, so that the run is left as found whatever it shows.
        QTest::keyClick(w.window, Qt::Key_A);
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(w.bridge.counts(), "2 to review · 2 accepted · 0 rejected");
        QTRY_COMPARE(w.bridge.statusLine(), "");
        const int landed = list->property("currentIndex").toInt();
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.decision(0), "To review");
        QCOMPARE(landed, 2);
        QTest::keyClick(w.window, Qt::Key_J);
        QTest::keyClick(w.window, Qt::Key_J);
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 3);
        QTest::keyClick(w.window, Qt::Key_K);
        QTest::keyClick(w.window, Qt::Key_K);
        QTRY_COMPARE(list->property("currentIndex").toInt(), 1);
    }

    // Home and g go to the first row, End and Shift+G to the last, and a page is the rows the list
    // shows at once: fewer in Rich than in Compact, as its rows are taller.
    void homeEndAndPagesMoveAsTheTerminalsDo() {
        int pages[2] = {0, 0};
        int at = 0;
        for (const char *style : {"compact", "rich"}) {
            Shown w({media()});
            w.settings.setRowStyle(QString::fromLatin1(style));
            QVERIFY(w.start());
            QTRY_COMPARE(w.rows(), 2);
            w.window->resize(w.window->minimumSize());
            QTRY_COMPARE(w.window->height(), 480);
            std::vector<subcheck_ffi::EntryView> more;
            for (uint64_t id = 100; id < 120; ++id) {
                more.push_back(
                    rowOfTwelve(subcheck_ffi::EntryKey(subcheck_ffi::EntryKey::kRow{{id}})));
            }
            QVERIFY(w.bridge.findings()->apply(subcheck_ffi::Splice{2, 0, more}));
            auto *list = qobject_cast<QQuickItem *>(w.find("findings"));
            QTRY_VERIFY(list->property("contentHeight").toReal() > list->height());
            QTRY_VERIFY(w.delegate(0));
            QCOMPARE(w.bridge.selected(), 0);
            const int rows = int(std::floor(list->height() / w.delegate(0)->height()));
            QVERIFY(rows > 1);
            QCOMPARE(list->property("pageRows").toInt(), rows);
            const QString last = selecting(w, w.bridge.findings()->rowCount() - 1);
            QVERIFY2(last.contains("119"), qPrintable(last));
            const qsizetype from = w.bridge.sent.size();
            for (const auto &[key, modifiers] :
                 {std::pair{Qt::Key_End, Qt::KeyboardModifiers{}},
                  std::pair{Qt::Key_G, Qt::KeyboardModifiers{Qt::ShiftModifier}}}) {
                QTest::keyClick(w.window, key, modifiers);
                QCOMPARE(w.last(), last);
            }
            for (const Qt::Key key : {Qt::Key_Home, Qt::Key_G}) {
                QTest::keyClick(w.window, key);
                QCOMPARE(w.last(), selecting(w, 0));
            }
            QTest::keyClick(w.window, Qt::Key_PageDown);
            QCOMPARE(w.last(), QString("Move %1").arg(rows));
            QTest::keyClick(w.window, Qt::Key_PageUp);
            QCOMPARE(w.last(), QString("Move -%1").arg(rows));
            QCOMPARE(w.since(from).size(), 6);
            pages[at++] = rows;
        }
        QVERIFY2(pages[1] < pages[0], qPrintable(QString("%1 %2").arg(pages[0]).arg(pages[1])));
    }

    // Every key the window has sends its action; review takes them in order, so only a key whose
    // action the window works out from what review last said waits for that, and the run is left
    // as found. The keys that quit are the next test's.
    void everyKeySendsItsAction() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *heading = w.find("heading");
        QVERIFY(heading);
        // Ctrl+C copies the selected row's line, and asks review nothing.
        QGuiApplication::clipboard()->clear();
        QCOMPARE(w.bridge.selected(), 0);
        QTest::keyClick(w.window, Qt::Key_C, Qt::ControlModifier);
        QCOMPARE(w.bridge.sent, QStringList());
        QCOMPARE(QGuiApplication::clipboard()->text(),
                 QString::fromStdString(w.bridge.findings()->at(0)->original));
        const int page = w.item("findings")->property("pageRows").toInt();
        QVERIFY(page > 1);
        const std::vector<std::tuple<int, Qt::KeyboardModifiers, QString>> keys = {
            {Qt::Key_J, {}, "Move 1"},
            {Qt::Key_Down, {}, "Move 1"},
            {Qt::Key_K, {}, "Move -1"},
            {Qt::Key_Up, {}, "Move -1"},
            {Qt::Key_Home, {}, selecting(w, 0)},
            {Qt::Key_End, {}, selecting(w, 1)},
            {Qt::Key_G, {}, selecting(w, 0)},
            {Qt::Key_G, Qt::ShiftModifier, selecting(w, 1)},
            {Qt::Key_PageUp, {}, QString("Move -%1").arg(page)},
            {Qt::Key_PageDown, {}, QString("Move %1").arg(page)},
            {Qt::Key_Return, {}, "Toggle"},
            {Qt::Key_Enter, Qt::KeypadModifier, "Toggle"},
            {Qt::Key_A, {}, "Accept"},
            {Qt::Key_U, {}, "Undo"},
            {Qt::Key_R, {}, "Reject"},
            {Qt::Key_Z, Qt::ControlModifier, "Undo"},
            {Qt::Key_X, {}, "Clear"},
            {Qt::Key_N, {}, "NextFile"},
            {Qt::Key_P, {}, "PreviousFile"},
            {Qt::Key_O, {}, "Play"},
            {Qt::Key_Space, {}, "Playback Pause"},
            {Qt::Key_Left, {}, "Playback Seek -5"},
            {Qt::Key_Right, {}, "Playback Seek 5"},
            {Qt::Key_Comma, {}, "Playback Seek -1"},
            {Qt::Key_Period, {}, "Playback Seek 1"},
            {Qt::Key_BracketLeft, {}, "Playback PreviousFrame"},
            {Qt::Key_BracketRight, {}, "Playback NextFrame"},
            {Qt::Key_O, Qt::ShiftModifier, "ClosePlayer"},
        };
        for (const auto &[key, modifiers, action] : keys) {
            const qsizetype from = w.bridge.sent.size();
            QTest::keyClick(w.window, Qt::Key(key), modifiers);
            QCOMPARE(w.since(from), QStringList{action});
        }
        QTRY_COMPARE(w.decision(0), "To review");
        // Each switch opens another view, from the one shown when review takes it.
        for (const char *now : {"Show Folders", "Show Newest Check"}) {
            QTest::keyClick(w.window, Qt::Key_V);
            QCOMPARE(w.last(), "SwitchView");
            QTRY_COMPARE(w.bridge.switchLabel(), now);
        }
        QCOMPARE(w.bridge.title(), "media");
        // The filter steps from the one shown, so each is waited for.
        for (const char *after :
             {"To Review · 4", "Accepted · 0", "Rejected · 0", "All Findings · 4"}) {
            QTest::keyClick(w.window, Qt::Key_F);
            QTRY_COMPARE(heading->property("text").toString(), after);
        }
        QCOMPARE(w.bridge.sent.mid(w.bridge.sent.size() - 4),
                 (QStringList{"SetFilter Pending", "SetFilter Accepted", "SetFilter Rejected",
                              "SetFilter All"}));
        // Last, as the edit it opens and the Find box each have the keys: Esc, their field's, ends
        // the edit and gives the window its keys back from the box, which stays. Ctrl+F asks review
        // nothing.
        const qsizetype from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_E);
        QCOMPARE(w.since(from), QStringList{"Edit"});
        QTRY_VERIFY(inEdit(w, "editField")->hasActiveFocus());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit"}));
        QTRY_VERIFY(!w.bridge.editing());
        QQuickItem *findBox = w.item("findBox");
        QQuickItem *findField = w.item("findField");
        QVERIFY(findBox && findField);
        QVERIFY(findBox->isVisible());
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(findField->hasActiveFocus());
        QVERIFY(findBox->isVisible());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QVERIFY(!findField->hasActiveFocus());
        QVERIFY(findBox->isVisible());
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit"}));
    }

    // The window's quit key, Close and Quit each quit: two Interrupts, the second stopping a job a
    // first asks about.
    void eachWayToQuitQuits() {
        const std::vector<std::pair<int, Qt::KeyboardModifiers>> keys = {
            {Qt::Key_Q, {}}, {Qt::Key_W, Qt::ControlModifier}, {Qt::Key_Q, Qt::ControlModifier}};
        for (const auto &[key, modifiers] : keys) {
            Shown w({media()});
            QSignalSpy finished(&w.bridge, &SessionBridge::finished);
            QVERIFY(w.start());
            QTRY_COMPARE(w.rows(), 2);
            QTest::keyClick(w.window, Qt::Key(key), modifiers);
            QVERIFY2(w.bridge.sent == (QStringList{"Interrupt", "Interrupt"}),
                     qPrintable(QKeySequence(key | modifiers).toString() + ": " +
                                w.bridge.sent.join(" | ")));
            QTRY_COMPARE(finished.count(), 1);
            QCOMPARE(finished[0][0].toInt(), 0);
        }
    }

    // The window's own close quits, as the others do; the window stays until review has ended,
    // and the counts are printed.
    void closingTheWindowQuits() {
        Shown w({media()});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->close();
        QCOMPARE(w.bridge.sent, (QStringList{"Interrupt", "Interrupt"}));
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 0);
        QVERIFY(w.window->isVisible());
        QCOMPARE(w.out.str(), "media: 0 accepted, 0 rejected, 4 pending\n");
    }

    // Each menu holds the items the window has now, each sending its action; the View menu's
    // words follow the kind of view shown, and its Show menu checks the filter shown.
    void everyMenuItemSendsItsAction() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        // Copy Subtitle Line copies the selected row's line, and asks review nothing.
        QGuiApplication::clipboard()->clear();
        QCOMPARE(w.bridge.selected(), 0);
        QObject *copyLine = w.find("copyLine");
        QVERIFY(copyLine);
        QVERIFY(QMetaObject::invokeMethod(copyLine, "trigger"));
        QCOMPARE(w.bridge.sent, QStringList());
        QCOMPARE(QGuiApplication::clipboard()->text(),
                 QString::fromStdString(w.bridge.findings()->at(0)->original));
        const std::vector<std::pair<const char *, QStringList>> items = {
            {"accept", {"Accept"}},
            {"undo", {"Undo"}},
            {"reject", {"Reject"}},
            {"undo", {"Undo"}},
            {"clear", {"Clear"}},
            {"edit", {"Edit"}},
            {"nextEpisode", {"NextFile"}},
            {"previousEpisode", {"PreviousFile"}},
            {"play", {"Play"}},
            {"showEachEpisode", {"Toggle"}},
            {"pause", {"Playback Pause"}},
            {"back5", {"Playback Seek -5"}},
            {"forward5", {"Playback Seek 5"}},
            {"back1", {"Playback Seek -1"}},
            {"forward1", {"Playback Seek 1"}},
            {"previousFrame", {"Playback PreviousFrame"}},
            {"nextFrame", {"Playback NextFrame"}},
            {"closeVideo", {"ClosePlayer"}},
            {"show2", {"SetFilter Accepted"}},
            {"show3", {"SetFilter Rejected"}},
            {"show1", {"SetFilter Pending"}},
            {"show0", {"SetFilter All"}},
        };
        for (const auto &[name, actions] : items) {
            QObject *item = w.find(name);
            QVERIFY2(item, name);
            const qsizetype from = w.bridge.sent.size();
            QVERIFY(QMetaObject::invokeMethod(item, "trigger"));
            QCOMPARE(w.since(from), actions);
            // The edit it opens would hold the decisions after it back.
            if (actions == QStringList{"Edit"}) {
                w.bridge.cancelEdit();
            }
        }
        QTRY_COMPARE(w.decision(0), "To review");
        QObject *show0 = w.find("show0");
        QObject *show2 = w.find("show2");
        QTRY_VERIFY(show0->property("checked").toBool());
        QVERIFY(!show2->property("checked").toBool());
        QCOMPARE(show2->property("text").toString(), "Accepted");

        QObject *switchView = w.find("switchView");
        QCOMPARE(switchView->property("text").toString(), "Show Newest Check");
        QVERIFY(QMetaObject::invokeMethod(switchView, "trigger"));
        QTRY_COMPARE(switchView->property("text").toString(), "Show Folders");
        QVERIFY(QMetaObject::invokeMethod(switchView, "trigger"));
        QTRY_COMPARE(switchView->property("text").toString(), "Show Newest Check");

        // Pop Out Video asks review nothing: it takes the video out, and again puts it back.
        QObject *popOut = w.find("popOutVideo");
        QVERIFY(popOut);
        for (const bool out : {true, false}) {
            const qsizetype from = w.bridge.sent.size();
            QVERIFY(QMetaObject::invokeMethod(popOut, "trigger"));
            QCOMPARE(w.since(from), QStringList());
            QCOMPARE(w.window->property("videoOut").toBool(), out);
        }

        // Find asks review nothing: it gives the Find box's field the keys.
        QObject *find = w.find("find");
        QVERIFY(find);
        QCOMPARE(find->property("text").toString(), "&Find…");
        const qsizetype from = w.bridge.sent.size();
        QVERIFY(QMetaObject::invokeMethod(find, "trigger"));
        QCOMPARE(w.since(from), QStringList());
        QTRY_VERIFY(w.item("findField")->hasActiveFocus());

        // What each menu holds: the items above, and only those.
        const std::vector<std::pair<const char *, int>> counts = {
            {"&File", 4}, {"&Edit", 4},     {"&View", 5}, {"&Show", 4},
            {"&Rows", 3}, {"F&inding", 10}, {"Vi&deo", 9}};
        for (const auto &[title, count] : counts) {
            QObject *each = w.menu(title);
            QVERIFY2(each, title);
            QCOMPARE(each->property("count").toInt(), count);
        }
        // One key, as StandardKey.Undo's several draw a warning on KDE's styles.
        QCOMPARE(w.find("undo")->property("shortcut").toString(), "Ctrl+Z");
        // Return, which the menu item shows; the keypad's Enter is a shortcut of its own.
        QCOMPARE(w.find("showEachEpisode")->property("shortcut").toString(), "Return");
        QCOMPARE(w.find("edit")->property("shortcut").toString(), "E");
        QCOMPARE(copyLine->property("shortcut").toString(), "Ctrl+C");
        QCOMPARE(find->property("shortcut").toString(), "Ctrl+F");
    }

    // The Show menu checks the filter shown and no other, whichever that is, and choosing the one
    // already shown leaves it checked.
    void theShowMenuChecksTheFilterShown() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        FiltersModel *filters = w.bridge.filters();
        const auto checked = [&w] {
            QList<bool> each;
            for (const char *name : {"show0", "show1", "show2", "show3"}) {
                each << w.find(name)->property("checked").toBool();
            }
            return each;
        };
        for (const int shown : {2, 3, 1, 0}) {
            QList<bool> only{false, false, false, false};
            only[shown] = true;
            QObject *item = w.find(qPrintable(QStringLiteral("show%1").arg(shown)));
            QVERIFY(QMetaObject::invokeMethod(item, "trigger"));
            // Read once review has answered: until then the check is only the click's.
            QTRY_VERIFY(filters->data(filters->index(shown), FiltersModel::CurrentRole).toBool());
            QTRY_COMPARE(checked(), only);
            QTRY_COMPARE(w.bridge.statusLine(), "");
            const qsizetype from = w.bridge.sent.size();
            QVERIFY(QMetaObject::invokeMethod(item, "trigger"));
            QCOMPARE(w.since(from).size(), qsizetype(1));
            // Review takes its actions in order, so this answer comes after the filter's.
            w.bridge.undo();
            QTRY_COMPARE(w.bridge.statusLine(), "Nothing to undo");
            QCOMPARE(checked(), only);
        }
    }

    // While a menu of the bar is open the keys are the menu's: a letter chooses the item it
    // underlines there, whatever shortcut it is elsewhere; the arrows move along the bar; and a
    // letter no item underlines does nothing. With the menu closed, each is the window's again.
    void anOpenMenuHasTheKeys() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        const QList<const char *> titles = {"&File", "&Edit", "&View", "F&inding", "Vi&deo"};
        const auto noneOpen = [&] {
            return std::none_of(titles.begin(), titles.end(), [&w](const char *title) {
                return w.menu(title)->property("visible").toBool();
            });
        };
        struct Case {
            int menu;
            Qt::Key key;
            QStringList sends;
            int opens;
        };
        // Qt has no mnemonics on Apple platforms, so there a letter chooses no item: it still stays
        // with the menu, and no shortcut fires.
#ifdef Q_OS_MACOS
        const auto chosen = [](const char *) { return QStringList(); };
#else
        const auto chosen = [](const char *action) { return QStringList{action}; };
#endif
        const std::vector<Case> cases = {
            {4, Qt::Key_X, chosen("Playback NextFrame"), -1},
            {4, Qt::Key_P, chosen("Playback Pause"), -1},
            {4, Qt::Key_V, chosen("Playback PreviousFrame"), -1},
            {3, Qt::Key_V, chosen("Play"), -1},
            {0, Qt::Key_Right, {}, 1},
            {4, Qt::Key_Left, {}, 3},
            {0, Qt::Key_A, {}, -1},
        };
        for (const Case &each : cases) {
            QTRY_VERIFY(noneOpen());
            QQuickItem *entry = w.barEntry(each.menu);
            QVERIFY(entry);
            w.click(entry);
            QTRY_VERIFY(w.menu(titles[each.menu])->property("opened").toBool());
            const qsizetype from = w.bridge.sent.size();
            QTest::keyClick(w.window, each.key);
            if (each.opens >= 0) {
                QTRY_VERIFY(w.menu(titles[each.opens])->property("opened").toBool());
            }
            QTRY_COMPARE(w.since(from), each.sends);
            if (each.sends.isEmpty()) {
                QTest::keyClick(w.window, Qt::Key_Escape);
            }
            QTRY_VERIFY(noneOpen());
            QCOMPARE(w.since(from), each.sends);
        }
        QTest::keyClick(w.window, Qt::Key_X);
        QCOMPARE(w.last(), "Clear");
    }

    // A menu's keys are kept from the window's shortcuts only while it is open, and only for a bar
    // the window draws: the system's own bar has the keys of its open menu before the window does.
    void theKeysAreKeptForAnOpenMenuOfABarTheWindowDraws() {
        const auto kept = [](const QString &platform, bool open, QEvent::Type type) {
            MenuKeys keys(platform);
            QObject window;
            keys.setTarget(&window);
            keys.setProperty("menuOpen", open);
            QKeyEvent event(type, Qt::Key_X, Qt::NoModifier);
            event.ignore();
            QCoreApplication::sendEvent(&window, &event);
            return event.isAccepted();
        };
        for (const char *platform : {"xcb", "wayland", "wayland-egl", "offscreen", "windows"}) {
            QVERIFY2(kept(platform, true, QEvent::ShortcutOverride), platform);
            QVERIFY2(!kept(platform, false, QEvent::ShortcutOverride), platform);
            QVERIFY2(!kept(platform, true, QEvent::KeyPress), platform);
        }
        QVERIFY(!kept("cocoa", true, QEvent::ShortcutOverride));
        // An app can ask Qt to draw the bar on macOS too.
        QCoreApplication::setAttribute(Qt::AA_DontUseNativeMenuBar, true);
        const bool drawnWhenAsked = kept("cocoa", true, QEvent::ShortcutOverride);
        QCoreApplication::setAttribute(Qt::AA_DontUseNativeMenuBar, false);
        QVERIFY(drawnWhenAsked);

        // A window it no longer watches keeps its shortcuts, and a new one gives way.
        MenuKeys keys(QStringLiteral("xcb"));
        QObject first;
        QObject second;
        keys.setProperty("menuOpen", true);
        keys.setTarget(&first);
        QCOMPARE(keys.target(), &first);
        keys.setTarget(&first);
        keys.setTarget(&second);
        QKeyEvent toFirst(QEvent::ShortcutOverride, Qt::Key_X, Qt::NoModifier);
        toFirst.ignore();
        QCoreApplication::sendEvent(&first, &toFirst);
        QVERIFY(!toFirst.isAccepted());
        QKeyEvent toSecond(QEvent::ShortcutOverride, Qt::Key_X, Qt::NoModifier);
        toSecond.ignore();
        QCoreApplication::sendEvent(&second, &toSecond);
        QVERIFY(toSecond.isAccepted());
    }

    // An open popup the window draws that has the keys keeps them from the window's shortcuts on
    // every platform, a Mac's among them: a menu off the bar, a text field's own menu, a sheet.
    // Closed, it keeps nothing, and on a Mac an open menu of the bar alone still keeps nothing.
    void aPopupWithTheKeysHoldsThemOnEveryPlatform() {
        QQmlEngine engine;
        QQmlComponent component(&engine);
        component.setData(QByteArrayLiteral(R"(
import QtQuick
import QtQuick.Controls

Window {
    width: 200
    height: 200

    Item {
        objectName: "outer"
        focus: true
    }
    Popup {
        objectName: "popup"
        focus: true

        Item {
            objectName: "inner"
            focus: true
        }
    }
}
)"),
                          QUrl(QStringLiteral("popup.qml")));
        const std::unique_ptr<QObject> made(component.create());
        auto *window = qobject_cast<QQuickWindow *>(made.get());
        QVERIFY2(window, qPrintable(component.errorString()));
        window->show();
        QVERIFY(QTest::qWaitForWindowExposed(window));
        window->requestActivate();
        QVERIFY(QTest::qWaitForWindowActive(window));
        QObject *popup = window->findChild<QObject *>("popup");
        auto *outer = window->findChild<QQuickItem *>("outer");
        auto *inner = popup ? popup->findChild<QQuickItem *>("inner") : nullptr;
        QVERIFY(popup && outer && inner);
        // Whether `platform`'s MenuKeys, watching the window, keeps a key sent to it as `type`:
        // accepted, so that it is no shortcut, and none of the window's to handle. Whether the
        // window then accepts a key it is given is Qt's: an open popup accepts every key press.
        const auto kept = [window](const QString &platform, QEvent::Type type,
                                   bool menuOpen = false) {
            HeardKeys past;
            window->installEventFilter(&past);
            MenuKeys keys(platform);
            keys.setTarget(window);
            keys.setProperty("menuOpen", menuOpen);
            QKeyEvent event(type, Qt::Key_X, Qt::NoModifier);
            event.ignore();
            QCoreApplication::sendEvent(window, &event);
            window->removeEventFilter(&past);
            return event.isAccepted() && past.heard == 0;
        };
        const QStringList platforms = {"xcb", "wayland", "offscreen", "windows", "cocoa"};

        QTRY_VERIFY(outer->hasActiveFocus());
        for (const QString &platform : platforms) {
            QVERIFY2(!kept(platform, QEvent::ShortcutOverride), qPrintable(platform));
            QVERIFY2(!kept(platform, QEvent::KeyPress), qPrintable(platform));
        }
        QVERIFY(!kept("cocoa", QEvent::ShortcutOverride, true));

        QVERIFY(QMetaObject::invokeMethod(popup, "open"));
        QTRY_VERIFY(popup->property("opened").toBool());
        QTRY_VERIFY(inner->hasActiveFocus());
        for (const QString &platform : platforms) {
            QVERIFY2(kept(platform, QEvent::ShortcutOverride), qPrintable(platform));
            QVERIFY2(!kept(platform, QEvent::KeyPress), qPrintable(platform));
        }

        QVERIFY(QMetaObject::invokeMethod(popup, "close"));
        QTRY_VERIFY(!popup->property("visible").toBool());
        outer->forceActiveFocus();
        QTRY_VERIFY(outer->hasActiveFocus());
        for (const QString &platform : platforms) {
            QVERIFY2(!kept(platform, QEvent::ShortcutOverride), qPrintable(platform));
            QVERIFY2(!kept(platform, QEvent::KeyPress), qPrintable(platform));
        }
    }

    // A popup Qt Quick opens in a window of its own over the window, as Fusion's text field menu
    // is, holds the keys too on every platform, whichever window a key reaches: its own, where the
    // platform sends a key while it is open, or the window, both of whose shortcuts Qt asks about
    // it. Closed, it holds none.
    void aPopupInAWindowOfItsOwnHoldsTheKeysOnEveryPlatform() {
        QQmlEngine engine;
        QQmlComponent component(&engine);
        component.setData(QByteArrayLiteral(R"(
import QtQuick
import QtQuick.Controls

Window {
    width: 200
    height: 200

    Item {
        objectName: "outer"
        focus: true
    }
    Popup {
        objectName: "popup"
        popupType: Popup.Window
        focus: true
        width: 50
        height: 50

        Item {
            objectName: "inner"
            focus: true
        }
    }
}
)"),
                          QUrl(QStringLiteral("popup-window.qml")));
        const std::unique_ptr<QObject> made(component.create());
        auto *window = qobject_cast<QQuickWindow *>(made.get());
        QVERIFY2(window, qPrintable(component.errorString()));
        window->show();
        QVERIFY(QTest::qWaitForWindowExposed(window));
        window->requestActivate();
        QVERIFY(QTest::qWaitForWindowActive(window));
        QObject *popup = window->findChild<QObject *>("popup");
        QVERIFY(popup);
        // Whether `platform`'s MenuKeys, watching the window, keeps a key sent to `to` from every
        // shortcut.
        const auto kept = [window](const QString &platform, QWindow *to) {
            MenuKeys keys(platform);
            keys.setTarget(window);
            QKeyEvent event(QEvent::ShortcutOverride, Qt::Key_X, Qt::NoModifier);
            event.ignore();
            QCoreApplication::sendEvent(to, &event);
            return event.isAccepted();
        };
        const QStringList platforms = {"xcb", "wayland", "offscreen", "windows", "cocoa"};
        for (const QString &platform : platforms) {
            QVERIFY2(!kept(platform, window), qPrintable(platform));
        }

        QVERIFY(QMetaObject::invokeMethod(popup, "open"));
        QTRY_VERIFY(popup->property("opened").toBool());
        QTRY_VERIFY(QGuiApplication::focusWindow() != window);
        QWindow *own = QGuiApplication::focusWindow();
        QVERIFY(own);
        QCOMPARE(own->type(), Qt::Popup);
        QCOMPARE(own->transientParent(), window);
        for (const QString &platform : platforms) {
            QVERIFY2(kept(platform, own), qPrintable(platform));
            QVERIFY2(kept(platform, window), qPrintable(platform));
        }

        QVERIFY(QMetaObject::invokeMethod(popup, "close"));
        QTRY_VERIFY(!popup->property("visible").toBool());
        for (const QString &platform : platforms) {
            QVERIFY2(!kept(platform, window), qPrintable(platform));
        }
    }

    // A press in the window away from the keeper is told, by the mouse or by touch, and one on the
    // keeper is not; nor is one while a popup the window draws has the keys, on the popup or
    // beside it, closing it. With no keeper every press is away, and a window it no longer watches
    // tells nothing.
    void aPressAwayFromTheKeeperIsToldWhileNoPopupHasTheKeys() {
        QQmlEngine engine;
        QQmlComponent component(&engine);
        component.setData(QByteArrayLiteral(R"(
import QtQuick
import QtQuick.Controls

Window {
    width: 200
    height: 200

    Item {
        objectName: "keeper"
        width: 50
        height: 50
    }
    Item {
        objectName: "outer"
        focus: true
    }
    Popup {
        objectName: "popup"
        popupType: Popup.Item
        focus: true
        x: 100
        width: 50
        height: 50

        Item {
            objectName: "inner"
            focus: true
        }
    }
}
)"),
                          QUrl(QStringLiteral("press-away.qml")));
        const std::unique_ptr<QObject> made(component.create());
        auto *window = qobject_cast<QQuickWindow *>(made.get());
        QVERIFY2(window, qPrintable(component.errorString()));
        window->show();
        QVERIFY(QTest::qWaitForWindowExposed(window));
        window->requestActivate();
        QVERIFY(QTest::qWaitForWindowActive(window));
        QObject *popup = window->findChild<QObject *>("popup");
        auto *keeper = window->findChild<QQuickItem *>("keeper");
        auto *outer = window->findChild<QQuickItem *>("outer");
        auto *inner = popup ? popup->findChild<QQuickItem *>("inner") : nullptr;
        QVERIFY(popup && keeper && outer && inner);
        PressAway away;
        away.setTarget(window);
        away.setTarget(window);
        QCOMPARE(away.target(), window);
        away.setKeeper(keeper);
        away.setKeeper(keeper);
        QCOMPARE(away.keeper(), keeper);
        QSignalSpy told(&away, &PressAway::pressedAway);
        // Whether `press` is told.
        const auto toldOf = [&told](const std::function<void()> &press) {
            const qsizetype before = told.count();
            press();
            return told.count() > before;
        };
        static QPointingDevice *const touch = QTest::createTouchDevice();
        const auto tap = [window](QPoint at) {
            QTest::touchEvent(window, touch).press(0, at, window);
            QTest::touchEvent(window, touch).release(0, at, window);
        };
        const QPoint onKeeper(25, 25);
        const QPoint elsewhere(25, 150);
        const QPoint onPopup(125, 25);
        for (const Qt::MouseButton button : {Qt::LeftButton, Qt::RightButton}) {
            QVERIFY(!toldOf([&] { QTest::mouseClick(window, button, {}, onKeeper); }));
            QVERIFY(toldOf([&] { QTest::mouseClick(window, button, {}, elsewhere); }));
        }
        QVERIFY(!toldOf([&] { tap(onKeeper); }));
        QVERIFY(toldOf([&] { tap(elsewhere); }));

        QVERIFY(QMetaObject::invokeMethod(popup, "open"));
        QTRY_VERIFY(popup->property("opened").toBool());
        QTRY_VERIFY(inner->hasActiveFocus());
        QVERIFY(!toldOf([&] { QTest::mouseClick(window, Qt::LeftButton, {}, onPopup); }));
        QVERIFY(popup->property("visible").toBool());
        QVERIFY(!toldOf([&] { QTest::mouseClick(window, Qt::LeftButton, {}, elsewhere); }));
        QTRY_VERIFY(!popup->property("visible").toBool());
        outer->forceActiveFocus();
        QTRY_VERIFY(outer->hasActiveFocus());
        QVERIFY(toldOf([&] { QTest::mouseClick(window, Qt::LeftButton, {}, elsewhere); }));

        away.setKeeper(nullptr);
        QVERIFY(toldOf([&] { QTest::mouseClick(window, Qt::LeftButton, {}, onKeeper); }));
        away.setTarget(nullptr);
        QVERIFY(!toldOf([&] { QTest::mouseClick(window, Qt::LeftButton, {}, elsewhere); }));
    }

    // While a menu of a bar the window draws is open, a press away from the keeper is the menu's,
    // as its keys are (menu_keys.h): it is not told. On a Mac the bar is the system's, and a press
    // that reaches the window is told; so is one once the menu has closed.
    void aPressWhileAMenuOfTheDrawnBarIsOpenIsNotTold() {
        QQmlEngine engine;
        QQmlComponent component(&engine);
        component.setData(QByteArrayLiteral(R"(
import QtQuick

Window {
    width: 200
    height: 200

    Item {
        objectName: "keeper"
        width: 50
        height: 50
    }
}
)"),
                          QUrl(QStringLiteral("press-away-bar.qml")));
        const std::unique_ptr<QObject> made(component.create());
        auto *window = qobject_cast<QQuickWindow *>(made.get());
        QVERIFY2(window, qPrintable(component.errorString()));
        window->show();
        QVERIFY(QTest::qWaitForWindowExposed(window));
        auto *keeper = window->findChild<QQuickItem *>("keeper");
        QVERIFY(keeper);
        // Whether a press away from the keeper is told while `platform`'s bar has a menu `open`.
        const auto told = [window, keeper](const QString &platform, bool open) {
            MenuKeys keys(platform);
            keys.setTarget(window);
            keys.setProperty("menuOpen", open);
            PressAway away;
            away.setTarget(window);
            away.setKeeper(keeper);
            away.setMenus(&keys);
            away.setMenus(&keys);
            if (away.menus() != &keys) {
                return false;
            }
            QSignalSpy spy(&away, &PressAway::pressedAway);
            QTest::mouseClick(window, Qt::LeftButton, {}, QPoint(150, 150));
            return spy.count() == 1;
        };
        for (const char *platform : {"xcb", "wayland", "offscreen", "windows"}) {
            QVERIFY2(!told(platform, true), platform);
            QVERIFY2(told(platform, false), platform);
        }
        QVERIFY(told("cocoa", true));
    }

    // While the list's Rows menu is open the keys are its own, as an open menu of the bar has
    // them: a letter that is a shortcut elsewhere and no item's here does nothing, and Esc closes
    // the menu. Closed, each key is the window's again.
    void theRowsMenuHasTheKeys() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *menu = w.find("rowsMenu");
        QQuickItem *button = w.item("rowStyleButton");
        QVERIFY(menu && button);
        w.click(button);
        QTRY_VERIFY(menu->property("opened").toBool());
        const qsizetype from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_X);
        QTest::keyClick(w.window, Qt::Key_J);
        QVERIFY(menu->property("visible").toBool());
        QCOMPARE(w.since(from), QStringList());
        QCOMPARE(w.settings.rowStyle(), "twoLines");
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!menu->property("visible").toBool());
        QCOMPARE(w.since(from), QStringList());
        QTest::keyClick(w.window, Qt::Key_X);
        QCOMPARE(w.last(), "Clear");
    }

    // The bar's seven menus, the View menu's two among them, open in windows of their own, over the
    // video's window, under which the window's scene draws wherever the two meet. The list's Rows
    // menu, which opens over the list, away from the video, stays in the scene, and the right-click
    // menu is the platform's own where it has one. Qt Quick's Popup.Item, Popup.Window and
    // Popup.Native are 0, 1 and 2.
    void theBarsMenusOpenInWindowsOfTheirOwn() {
        Shown w({media()});
        QVERIFY(w.show());
        QStringList inTheScene;
        for (const char *title :
             {"&File", "&Edit", "&View", "&Show", "&Rows", "F&inding", "Vi&deo"}) {
            QObject *menu = w.menu(title);
            QVERIFY2(menu, title);
            if (menu->property("popupType").toInt() != 1) {
                inTheScene << QString::fromUtf8(title);
            }
        }
        QCOMPARE(inTheScene, QStringList());
        QObject *rowsMenu = w.find("rowsMenu");
        QObject *rowMenu = w.find("rowMenu");
        QVERIFY(rowsMenu && rowMenu);
        QCOMPARE(rowsMenu->property("popupType").toInt(), 0);
        QCOMPARE(rowMenu->property("popupType").toInt(), 2);
    }

    // Every menu the window draws is as wide as its items, each drawn whole with its shortcut, at
    // the platform's text size and at the largest Settings offers: the right-click menu, the list's
    // Rows menu, each menu of the bar and the View menu's two, Show among them, whose items come
    // from review after the window starts. Each takes the window's size of text, the Rows menu
    // too, which opens over the list.
    void everyMenuDrawsEachItemWhole_data() {
        QTest::addColumn<bool>("largest");
        QTest::newRow("the platform's size") << false;
        QTest::newRow("the largest size") << true;
    }
    void everyMenuDrawsEachItemWhole() {
        QFETCH(bool, largest);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        if (largest) {
            QSignalSpy laidOut(w.window, &QQuickWindow::afterAnimating);
            w.settings.setWindowTextSize(w.settings.mostHalfPoints() / 2.0);
            // The bar's entries take their new widths at once, but the bar moves them only as the
            // window lays itself out for its next frame, which it says by afterAnimating: until
            // then an entry runs over its neighbour's old place, and a click at its middle lands on
            // the neighbour, which lies above it.
            QTRY_VERIFY(!laidOut.isEmpty());
        }
        QTRY_VERIFY(w.delegate(0));
        QQuickItem *rowStyleButton = w.item("rowStyleButton");
        QVERIFY(rowStyleButton);
        // Each menu, what opens it, and the menu it opens from, which closes after it.
        std::vector<std::tuple<QObject *, std::function<void()>, QObject *>> menus = {
            {w.find("rowMenu"), [&w] { w.rightClick(w.delegate(0)); }, nullptr},
            {w.find("rowsMenu"), [&w, rowStyleButton] { w.click(rowStyleButton); }, nullptr},
        };
        for (int i = 0; i < 5; ++i) {
            QQuickItem *entry = w.barEntry(i);
            QVERIFY(entry);
            menus.emplace_back(
                entry->property("menu").value<QObject *>(), [&w, entry] { w.click(entry); },
                nullptr);
        }
        QObject *view = w.menu("&View");
        QVERIFY(view);
        for (const char *title : {"&Show", "&Rows"}) {
            QObject *sub = w.menu(title);
            QVERIFY(sub);
            menus.emplace_back(
                sub,
                [&w, view, sub] {
                    w.click(w.barEntry(2));
                    QTRY_VERIFY(view->property("opened").toBool());
                    QQuickItem *item = subMenuItem(view, sub);
                    QVERIFY(item);
                    w.click(item);
                },
                view);
        }
        for (const auto &[menu, open, from] : menus) {
            QVERIFY(menu);
            const QString name = menu->property("title").toString() + menu->objectName();
            QTRY_COMPARE(popupWindowsOpen(), QString());
            open();
            QTRY_VERIFY2(menu->property("opened").toBool(), qPrintable(name + ": " + menusMet(w)));
            QCOMPARE(menu->property("font").value<QFont>().pointSizeF(),
                     w.settings.windowTextSize());
            QStringList wrong;
            QTRY_VERIFY2((wrong = itemsNotWhole(menu)).isEmpty(), qPrintable(wrong.join(" | ")));
            QTest::keyClick(w.window, Qt::Key_Escape);
            QTRY_VERIFY2(!menu->property("visible").toBool(), qPrintable(name));
            if (from) {
                QTest::keyClick(w.window, Qt::Key_Escape);
                QTRY_VERIFY2(!from->property("visible").toBool(), qPrintable(name));
            }
            // The window manager gives the keys back to the window a menu drawn in a window of its
            // own was over, which offscreen has none to do.
            w.window->requestActivate();
            QTRY_COMPARE(QGuiApplication::focusWindow(), static_cast<QWindow *>(w.window));
        }
    }

    // Return is named by its symbol, ⏎, wherever Qt names it for a person, Shift+Return as
    // Shift+⏎, so that the word beside a menu item's words does not read as more of them; the
    // keypad's Enter keeps its name, and the window's "Return" still names the key. A menu measures
    // the symbol: the Finding menu's Show Each Episode asks room for it. macOS names these keys by
    // symbols of its own.
    void returnIsNamedByItsSymbol() {
#ifdef Q_OS_MACOS
        const QString symbol = QStringLiteral("↵");
        const QString shifted = QStringLiteral("⇧↵");
        const QString enter = QStringLiteral("⌤");
#else
        const QString symbol = QStringLiteral("⏎");
        const QString shifted = QStringLiteral("Shift+⏎");
        const QString enter = QStringLiteral("Enter");
#endif
        QCOMPARE(QKeySequence(Qt::Key_Return).toString(QKeySequence::NativeText), symbol);
        QCOMPARE(QKeySequence(Qt::SHIFT | Qt::Key_Return).toString(QKeySequence::NativeText),
                 shifted);
        QCOMPARE(QKeySequence(Qt::Key_Enter).toString(QKeySequence::NativeText), enter);
        QCOMPARE(QKeySequence(Qt::Key_Return).toString(QKeySequence::PortableText), "Return");
        QCOMPARE(QKeySequence(QStringLiteral("Return")), QKeySequence(Qt::Key_Return));
        QCOMPARE(QKeySequence(QStringLiteral("Shift+Return")),
                 QKeySequence(Qt::SHIFT | Qt::Key_Return));

        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *finding = w.menu("F&inding");
        QObject *show = w.find("showEachEpisode");
        QVERIFY(finding && show);
        // WholeMenu's measures, one for each item, in the menu's order.
        auto *needs =
            finding->findChild<QObject *>(QStringLiteral("itemNeeds"), Qt::FindDirectChildrenOnly);
        QVERIFY(needs);
        QObject *need = nullptr;
        const int count = finding->property("count").toInt();
        for (int i = 0; i < count; ++i) {
            QQuickItem *each = nullptr;
            QMetaObject::invokeMethod(finding, "itemAt", Q_RETURN_ARG(QQuickItem *, each),
                                      Q_ARG(int, i));
            if (each && each->property("action").value<QObject *>() == show) {
                QMetaObject::invokeMethod(needs, "objectAt", Q_RETURN_ARG(QObject *, need),
                                          Q_ARG(int, i));
            }
        }
        QVERIFY(need);
        QCOMPARE(need->property("text").toString(), symbol);
        QCOMPARE(need->property("advanceWidth").toReal(),
                 QFontMetricsF(need->property("font").value<QFont>()).horizontalAdvance(symbol));
    }

    void aRightClickMenuOpensWhereTheClickWas_data() {
        QTest::addColumn<QString>("where");
        for (const char *where : {"a Two Lines row", "a Compact row", "a Rich row",
                                  "a row's triangle", "the finding in the details"}) {
            QTest::newRow(where) << QString::fromLatin1(where);
        }
    }
    // The menu opens at the click, whatever the pointer's position on the screen is: Wayland gives
    // a window none. Offscreen goes by the menu's place in its parent, but Wayland places a menu Qt
    // draws in a popup window of its own by the rect waylandAnchor gives, its parent's: so the menu
    // opens from an item a pixel square at the click, never from one of the window's parts. The
    // click is a quarter across and a third down the part, the triangle's the middle of it, so that
    // no part's middle is the point.
    void aRightClickMenuOpensWhereTheClickWas() {
        QFETCH(QString, where);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        const bool compact = where == "a Compact row";
        w.settings.setRowStyle(compact ? "compact" : where == "a Rich row" ? "rich" : "twoLines");
        QTRY_VERIFY(w.delegate(1));
        QObject *menu = w.find("rowMenu");
        QVERIFY(menu);
        QQuickItem *of = nullptr;
        QPoint at;
        if (where == "the finding in the details") {
            QTest::keyClick(w.window, Qt::Key_J);
            QTRY_COMPARE(w.bridge.selected(), 1);
            of = w.item("nowBlock");
            QTRY_VERIFY(of->isVisible());
            QVERIFY(inDetailsView(w, of));
        } else if (where == "a row's triangle") {
            of = Shown::item(w.delegate(0), "rowDisclosure");
            QVERIFY(of);
            at = w.middle(of);
        } else {
            of = w.delegate(1);
        }
        if (at.isNull()) {
            at = of->mapToScene(QPointF(of->width() / 4, of->height() / 3)).toPoint();
        }
        QCursor::setPos(w.window->mapToGlobal(QPoint(5, w.window->height() - 5)));
        QTest::mouseClick(w.window, Qt::RightButton, {}, at);
        QTRY_VERIFY(menu->property("opened").toBool());
        auto *parent = menu->property("parent").value<QQuickItem *>();
        QVERIFY(parent);
        QVERIFY(parent != w.window->property("contentItem").value<QQuickItem *>());
        QCOMPARE(parent->size(), QSizeF(1, 1));
        QCOMPARE(parent->mapToScene(QPointF(0, 0)), QPointF(at));
        QCOMPARE(waylandAnchor(menu), QRect(at, QSize(1, 1)));
        const QPointF opened =
            parent->mapToScene(QPointF(menu->property("x").toReal(), menu->property("y").toReal()));
        QVERIFY2(qAbs(opened.x() - at.x()) <= 3 && qAbs(opened.y() - at.y()) <= 3,
                 qPrintable(QStringLiteral("opened at %1,%2 for a click at %3,%4")
                                .arg(opened.x())
                                .arg(opened.y())
                                .arg(at.x())
                                .arg(at.y())));
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!menu->property("visible").toBool());
    }

    // A right-click on a row selects it, as a click does, then opens the row's menu over it: the
    // Finding menu's items that act on a finding, then Copy Subtitle Line and Show File in Folder,
    // in each row style. Check Episode Again… is not among them: the window has no way to check an
    // episode again.
    void aRightClickSelectsTheRowThenOpensItsMenu() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *menu = w.find("rowMenu");
        QVERIFY(menu);
        const QStringList words = {"&Accept",
                                   "&Reject",
                                   "&Edit Wording…",
                                   "&Back to Undecided",
                                   "-",
                                   "Show in &Video",
                                   "&Show Each Episode",
                                   "-",
                                   "&Copy Subtitle Line",
                                   "Show &File in Folder"};
        for (const char *style : {"twoLines", "compact", "rich"}) {
            w.settings.setRowStyle(style);
            QTRY_COMPARE(drawnStyles(w), (QStringList{style, style}));
            QTRY_VERIFY(w.delegate(1));
            const QString select = selecting(w, 1);
            const qsizetype from = w.bridge.sent.size();
            w.rightClick(w.delegate(1));
            QVERIFY2(w.since(from) == QStringList{select},
                     qPrintable(QString::fromLatin1(style) + ": " + w.since(from).join(" | ")));
            QTRY_VERIFY2(menu->property("opened").toBool(), style);
            QCOMPARE(menu->property("row").toInt(), 1);
            QCOMPARE(menuWords(menu), words);
            QVERIFY(!menuWords(menu).contains("Check Episode Again…"));
            QTest::keyClick(w.window, Qt::Key_Escape);
            QTRY_VERIFY2(!menu->property("visible").toBool(), style);
            QCOMPARE(w.since(from), QStringList{select});
            QTRY_COMPARE(w.bridge.selected(), 1);
        }
        // The triangle's clicks are its own, but not a right-click, which is the row's.
        QQuickItem *triangle = Shown::item(w.delegate(0), "rowDisclosure");
        QVERIFY(triangle && triangle->isVisible() && triangle->opacity() > 0);
        const QString select = selecting(w, 0);
        const qsizetype from = w.bridge.sent.size();
        w.rightClick(triangle);
        QCOMPARE(w.since(from), QStringList{select});
        QTRY_VERIFY(menu->property("opened").toBool());
        QCOMPARE(menu->property("row").toInt(), 0);
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!menu->property("visible").toBool());
        QCOMPARE(w.rows(), 2);
    }

    // Each item of a row's menu, chosen on a fresh right-click of row 0, does what its item of the
    // bar's menus does, after the right-click's Select; and the menu closes once one is chosen.
    // Back to Undecided comes after Reject, which moves review on to row 1, so that it takes the
    // row clicked, not the one review had selected before the click.
    void eachRightClickItemDoesWhatItsMenuItemDoes() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *menu = w.find("rowMenu");
        QVERIFY(menu);
        const QString line = text(w.bridge.findings()->at(0)->original);
        const QString file = text(w.bridge.findings()->at(0)->file);
        QGuiApplication::clipboard()->clear();
        // What each item sends after the Select, and what the test then checks, or does to leave
        // the run as found.
        const std::vector<std::tuple<const char *, QStringList, std::function<void()>>> items = {
            {"rowAccept",
             {"Accept"},
             [&w] {
                 QTRY_COMPARE(w.decision(0), "Accepted");
                 w.bridge.undo();
                 QTRY_COMPARE(w.decision(0), "To review");
             }},
            {"rowReject",
             {"Reject"},
             [&w] {
                 QTRY_COMPARE(w.decision(0), "Rejected");
                 QTRY_COMPARE(w.bridge.selected(), 1);
             }},
            {"rowClear",
             {"Clear"},
             [&w] {
                 QTRY_COMPARE(w.decision(0), "To review");
                 w.bridge.undo();
                 QTRY_COMPARE(w.decision(0), "Rejected");
                 w.bridge.undo();
                 QTRY_COMPARE(w.decision(0), "To review");
             }},
            {"rowPlay", {"Play"}, [] {}},
            {"rowShowEachEpisode", {"Toggle"}, [&w] { QTRY_COMPARE(w.rows(), 4); }},
            {"rowEdit",
             {"Edit"},
             [&w] {
                 QTRY_VERIFY(w.bridge.editing());
                 QQuickItem *cancel = inEdit(w, "cancelButton");
                 QTRY_VERIFY(cancel && cancel->isVisible());
                 const qsizetype from = w.bridge.sent.size();
                 w.click(cancel);
                 QCOMPARE(w.since(from), QStringList{"CancelEdit"});
                 QTRY_VERIFY(!w.bridge.editing());
             }},
            {"rowCopy", {}, [line] { QCOMPARE(QGuiApplication::clipboard()->text(), line); }},
            {"rowShowInFolder", {}, [&w, file] { QCOMPARE(w.bridge.revealed, QStringList{file}); }},
        };
        for (const auto &[name, actions, then] : items) {
            QTRY_VERIFY(w.delegate(0));
            const QString select = selecting(w, 0);
            const qsizetype from = w.bridge.sent.size();
            w.rightClick(w.delegate(0));
            QTRY_VERIFY2(menu->property("opened").toBool(), name);
            QQuickItem *item = rowMenuItem(w, name);
            QVERIFY2(item, name);
            QTRY_VERIFY2(item->property("enabled").toBool(), name);
            w.click(item);
            QTRY_VERIFY2(!menu->property("visible").toBool(), name);
            QVERIFY2(w.since(from) == QStringList{select} + actions,
                     qPrintable(QString::fromLatin1(name) + ": " + w.since(from).join(" | ")));
            then();
            if (QTest::currentTestFailed()) {
                return;
            }
        }
    }

    // While a row's menu is open the keys are its own, as an open menu of the bar has them: a
    // letter that is a shortcut elsewhere and no item's here does nothing; one an item underlines
    // chooses that item; and Esc closes the menu. Closed, each key is the window's again.
    void anOpenRowMenuHasTheKeys() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *menu = w.find("rowMenu");
        QVERIFY(menu);
        // Qt has no mnemonics on Apple platforms, so there a letter chooses no item: it still stays
        // with the menu, and no shortcut fires.
#ifdef Q_OS_MACOS
        const QStringList chosen;
#else
        const QStringList chosen{"Accept"};
#endif
        QTRY_VERIFY(w.delegate(0));
        w.rightClick(w.delegate(0));
        QTRY_VERIFY(menu->property("opened").toBool());
        qsizetype from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_X);
        QVERIFY(menu->property("visible").toBool());
        QCOMPARE(w.since(from), QStringList());
        QTest::keyClick(w.window, Qt::Key_A);
        QTRY_COMPARE(w.since(from), chosen);
        if (!chosen.isEmpty()) {
            QTRY_VERIFY(!menu->property("visible").toBool());
            QTRY_COMPARE(w.decision(0), "Accepted");
            w.bridge.undo();
            QTRY_COMPARE(w.decision(0), "To review");
            w.rightClick(w.delegate(0));
            QTRY_VERIFY(menu->property("opened").toBool());
            from = w.bridge.sent.size();
        }
        QVERIFY(menu->property("visible").toBool());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!menu->property("visible").toBool());
        QCOMPARE(w.since(from), QStringList());
        QTest::keyClick(w.window, Qt::Key_X);
        QCOMPARE(w.last(), "Clear");
    }

    // A timing row's menu greys what a timing fix cannot take, Edit Wording… and Copy Subtitle
    // Line, and offers the rest, Show Each Episode among them, as the row is one of several.
    void aTimingRowsMenuGreysWhatItCannotTake() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *menu = w.find("rowMenu");
        QVERIFY(menu);
        // A timing row of three made up in the selected row's place, drawn as review's next change
        // would. Review is held, as the right-click's Select names an entry review never sent.
        const int at = w.bridge.selected();
        subcheck_ffi::EntryView timing =
            rowOfTwelve(subcheck_ffi::EntryKey(subcheck_ffi::EntryKey::kRow{{500}}));
        timing.shown = 3;
        timing.total = 3;
        timing.check = subcheck_ffi::Check::kTiming;
        timing.editable = false;
        QVERIFY(w.bridge.findings()->apply(
            subcheck_ffi::Splice{static_cast<uint64_t>(at), 1, {timing}}));
        emit w.bridge.summaryChanged();
        w.bridge.holding = true;
        QTRY_VERIFY(w.delegate(at));
        w.rightClick(w.delegate(at));
        QTRY_VERIFY(menu->property("opened").toBool());
        QCOMPARE(menu->property("row").toInt(), at);
        const auto enabled = [&w](const char *name) {
            QQuickItem *item = rowMenuItem(w, name);
            return item && item->property("enabled").toBool();
        };
        QVERIFY(!enabled("rowEdit"));
        QVERIFY(!enabled("rowCopy"));
        for (const char *name : {"rowAccept", "rowReject", "rowClear", "rowPlay",
                                 "rowShowEachEpisode", "rowShowInFolder"}) {
            QVERIFY2(enabled(name), name);
        }
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!menu->property("visible").toBool());
    }

    // Copy Subtitle Line reads its row again at each opening of the menu, and while the menu is
    // open at each change of review's, as a change can put another entry at the same row number: a
    // timing row decided under a filter that hides it leaves the list, and the text row after it
    // takes its number. Review is held, and its changes stood in for, as the timing row is made up.
    void theRowMenusCopyFollowsTheEntryAtItsRow() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *menu = w.find("rowMenu");
        QVERIFY(menu);
        subcheck_ffi::EntryView timing =
            rowOfTwelve(subcheck_ffi::EntryKey(subcheck_ffi::EntryKey::kRow{{500}}));
        timing.check = subcheck_ffi::Check::kTiming;
        timing.editable = false;
        // Review's change at row 1, drawn as review's would be.
        const auto change = [&w](uint64_t removed, std::vector<subcheck_ffi::EntryView> inserted) {
            const bool applied =
                w.bridge.findings()->apply(subcheck_ffi::Splice{1, removed, std::move(inserted)});
            emit w.bridge.summaryChanged();
            return applied;
        };
        const auto copyEnabled = [&w] {
            QQuickItem *item = rowMenuItem(w, "rowCopy");
            return item && item->property("enabled").toBool();
        };
        QVERIFY(change(0, {timing}));
        QCOMPARE(w.rows(), 3);
        w.bridge.holding = true;
        QTRY_VERIFY(w.delegate(1));
        w.rightClick(w.delegate(1));
        QTRY_VERIFY(menu->property("opened").toBool());
        QCOMPARE(menu->property("row").toInt(), 1);
        QVERIFY(!copyEnabled());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!menu->property("visible").toBool());

        // Decided, the timing row leaves, and the text row after it is row 1.
        QVERIFY(change(1, {}));
        QCOMPARE(w.rows(), 2);
        QTRY_VERIFY(w.delegate(1));
        w.rightClick(w.delegate(1));
        QTRY_VERIFY(menu->property("opened").toBool());
        QCOMPARE(menu->property("row").toInt(), 1);
        QVERIFY(copyEnabled());
        // The decision undone while the menu is open: the timing row is row 1 again.
        QVERIFY(change(0, {timing}));
        QVERIFY(!copyEnabled());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!menu->property("visible").toBool());
    }

    // Copy Subtitle Line and Show File in Folder take the row right-clicked at once, though review
    // has not answered the right-click's Select, as while a job runs, and the selection stays where
    // review has it. The row is row 1's member in the second episode, whose line and file are both
    // unlike row 0's: rows 0 and 1 are both the first episode's.
    void copyAndShowFileInFolderReadTheClickedRowAtOnce() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *menu = w.find("rowMenu");
        QVERIFY(menu);
        w.bridge.toggleRow(1);
        QTRY_COMPARE(w.rows(), 4);
        QTRY_COMPARE(w.bridge.selected(), 1);
        w.bridge.select(0);
        QTRY_COMPARE(w.bridge.selected(), 0);
        const subcheck_ffi::EntryView *clicked = w.bridge.findings()->at(3);
        const subcheck_ffi::EntryView *selected = w.bridge.findings()->at(0);
        QVERIFY(clicked && selected);
        QVERIFY(clicked->original != selected->original);
        QVERIFY(clicked->file != selected->file);
        const QString line = text(clicked->original);
        const QString file = text(clicked->file);
        const QString select = selecting(w, 3);
        w.bridge.holding = true;
        QGuiApplication::clipboard()->clear();
        const qsizetype from = w.bridge.sent.size();

        QTRY_VERIFY(w.delegate(3));
        w.rightClick(w.delegate(3));
        QTRY_VERIFY(menu->property("opened").toBool());
        QCOMPARE(menu->property("row").toInt(), 3);
        w.click(rowMenuItem(w, "rowShowInFolder"));
        QTRY_VERIFY(!menu->property("visible").toBool());
        QCOMPARE(w.bridge.revealed, QStringList{file});
        QCOMPARE(w.bridge.selected(), 0);

        w.rightClick(w.delegate(3));
        QTRY_VERIFY(menu->property("opened").toBool());
        w.click(rowMenuItem(w, "rowCopy"));
        QTRY_VERIFY(!menu->property("visible").toBool());
        QCOMPARE(QGuiApplication::clipboard()->text(), line);
        QCOMPARE(w.bridge.selected(), 0);
        QCOMPARE(w.since(from), (QStringList{select, select}));
    }

    // A right-click on the finding in the details panel opens the menu of its row, the one
    // selected, and sends nothing, as review has that row selected already. A greyed episode's log
    // in the finding's place has no menu.
    void aRightClickOnTheDetailsOpensTheMenuForTheFindingShown() {
        Shown w({clean(), media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *menu = w.find("rowMenu");
        QVERIFY(menu);
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(w.bridge.selected(), 1);
        QQuickItem *now = w.item("nowBlock");
        QTRY_VERIFY(now->isVisible());
        QVERIFY(inDetailsView(w, now));
        qsizetype from = w.bridge.sent.size();
        w.rightClick(now);
        QTRY_VERIFY(menu->property("opened").toBool());
        QCOMPARE(menu->property("row").toInt(), 1);
        QCOMPARE(w.since(from), QStringList());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!menu->property("visible").toBool());

        QTRY_VERIFY(w.item("episode0"));
        w.click(w.item("episode0"));
        QTRY_VERIFY(w.bridge.showingEpisode());
        QQuickItem *heading = w.item("episodeLogHeading");
        QTRY_VERIFY(heading->isVisible());
        QTRY_VERIFY(inDetailsView(w, heading));
        from = w.bridge.sent.size();
        w.rightClick(heading);
        QVERIFY(!menu->property("visible").toBool());
        QCOMPARE(w.since(from), QStringList());
    }

    // The window's Close and Quit, from their menu.
    void theFileMenusItemsQuit() {
        for (const char *name : {"close", "quit"}) {
            Shown w({media()});
            QSignalSpy finished(&w.bridge, &SessionBridge::finished);
            QVERIFY(w.start());
            QTRY_COMPARE(w.rows(), 2);
            QVERIFY(QMetaObject::invokeMethod(w.find(name), "trigger"));
            QCOMPARE(w.bridge.sent, (QStringList{"Interrupt", "Interrupt"}));
            QTRY_COMPARE(finished.count(), 1);
        }
    }

    // The sidebar: a review opens its folder, a filter is set, an episode selected and then its
    // first finding; Check a Folder… is drawn, and does nothing. A click takes no keys from the
    // window, so a key after it still sends its action.
    void theSidebarsEntriesAskReviewAndLeaveTheKeysWorking() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        const QString folder = QString::fromStdString(media());
        const QString clean = QFileInfo(folder).path() + "/clean";
        // A list makes its entries when it next lays itself out, which can come after the rows.
        QTRY_VERIFY(w.item("review0") && w.item("episode1"));
        QQuickItem *review = w.item("review0");
        QQuickItem *filter = w.item("filter2");
        QQuickItem *episode = w.item("episode1");
        QQuickItem *check = w.item("checkAFolder");
        QVERIFY(review && filter && episode && check);
        QCOMPARE(review->property("text").toString(), "clean");
        QVERIFY(!check->isEnabled());
        w.click(check);
        QCOMPARE(w.bridge.sent, QStringList());

        w.click(episode);
        QCOMPARE(w.bridge.sent,
                 (QStringList{"SelectFile " + folder + "/typos - E02.mkv", "ShowFileFindings"}));
        QTRY_COMPARE(w.bridge.episodeName(), "E02");
        QTest::keyClick(w.window, Qt::Key_J);
        QCOMPARE(w.last(), "Move 1");

        w.click(filter);
        QCOMPARE(w.last(), "SetFilter Accepted");
        QTRY_COMPARE(w.rows(), 0);
        QTest::keyClick(w.window, Qt::Key_F);
        QCOMPARE(w.last(), "SetFilter Rejected");

        w.click(review);
        QCOMPARE(w.last(), "OpenFolder " + clean);
        QTRY_COMPARE(w.bridge.title(), "clean");
        QTRY_VERIFY(w.item("review0") && w.item("review0")->property("highlighted").toBool());
        QTest::keyClick(w.window, Qt::Key_K);
        QCOMPARE(w.last(), "Move -1");
    }

    // Each entry of each of the sidebar's lists asks for its own row, says what review says of
    // that row, and is marked while it is the one shown.
    void eachSidebarEntryIsItsOwnRow() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("review1") && w.item("episode1"));
        const auto entry = [&w](const char *list, int row) {
            return w.item(qPrintable(QStringLiteral("%1%2").arg(list).arg(row)));
        };
        const auto marked = [&entry](const char *list, int row) {
            return entry(list, row)->property("highlighted").toBool();
        };

        FiltersModel *filters = w.bridge.filters();
        const QStringList names = {"All", "Pending", "Accepted", "Rejected"};
        QCOMPARE(qsizetype(filters->rowCount()), names.size());
        for (const int row : {1, 2, 3, 0}) {
            const QVariant count = filters->data(filters->index(row), FiltersModel::CountRole);
            QCOMPARE(Shown::text(entry("filter", row), "entrySide"), count.toString());
            w.click(entry("filter", row));
            QCOMPARE(w.last(), "SetFilter " + names[row]);
            QTRY_VERIFY(marked("filter", row));
            QVERIFY(!marked("filter", (row + 1) % 4));
        }
        QCOMPARE(Shown::text(entry("filter", 0), "entrySide"), "4");
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(entry("episode", 1));

        EpisodeOrder *files = w.bridge.episodes();
        QCOMPARE(files->rowCount(), 2);
        QTRY_VERIFY(marked("episode", 0));
        QVERIFY(!marked("episode", 1));
        for (const int row : {1, 0}) {
            const QModelIndex at = files->index(row, 0);
            const QString stage = files->data(at, EpisodesModel::StageRole).toString();
            QVERIFY(!stage.isEmpty());
            QCOMPARE(Shown::text(entry("episode", row), "entrySide"), stage);
            const QString path = files->data(at, EpisodesModel::PathRole).toString();
            const qsizetype from = w.bridge.sent.size();
            w.click(entry("episode", row));
            QCOMPARE(w.since(from), (QStringList{"SelectFile " + path, "ShowFileFindings"}));
            QTRY_COMPARE(w.bridge.episodeName(),
                         files->data(at, EpisodesModel::LabelRole).toString());
            QTRY_VERIFY(marked("episode", row));
            QVERIFY(!marked("episode", 1 - row));
        }

        FoldersModel *folders = w.bridge.folders();
        QCOMPARE(folders->rowCount(), 2);
        QTRY_VERIFY(marked("review", 1));
        QVERIFY(!marked("review", 0));
        for (const int row : {0, 1}) {
            const QModelIndex at = folders->index(row);
            const QString path = folders->data(at, FoldersModel::PathRole).toString();
            const int count = folders->data(at, FoldersModel::CountRole).toInt();
            QCOMPARE(Shown::text(entry("review", row), "entrySide"),
                     count > 0 ? QString::number(count) : QString());
            w.click(entry("review", row));
            QCOMPARE(w.last(), "OpenFolder " + path);
            QTRY_COMPARE(w.bridge.title(), folders->data(at, FoldersModel::NameRole).toString());
            QTRY_VERIFY(entry("review", row) && marked("review", row));
            QVERIFY(!marked("review", 1 - row));
        }
    }

    // View ▸ Episodes Without Findings Last lists the episodes the check found nothing in after the
    // rest, and keeps the choice; a click on an entry then opens the episode it shows, not the one
    // review has at that row, and N and P still step through review's own order.
    void theViewMenuPutsEpisodesWithoutFindingsLastAndAClickOpensTheRowUnderIt() {
        Shown w({clean(), media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        EpisodeOrder *episodes = w.bridge.episodes();
        QTRY_COMPARE(episodes->rowCount(), 3);
        const auto paths = [episodes] {
            QStringList each;
            for (int row = 0; row < episodes->rowCount(); ++row) {
                each << episodes->data(episodes->index(row, 0), EpisodesModel::PathRole).toString();
            }
            return each;
        };
        const auto drawn = [&w] {
            QStringList labels;
            for (const char *name : {"episode0", "episode1", "episode2"}) {
                QQuickItem *entry = w.item(name);
                labels << (entry ? entry->property("text").toString() : QString());
            }
            return labels;
        };
        // Review lists the folder with no findings first, as its path sorts first.
        const QString folder = QString::fromStdString(media());
        const QString none = QString::fromStdString(clean()) + "/typos.mkv";
        const QString e01 = folder + "/typos - E01.mkv";
        const QString e02 = folder + "/typos - E02.mkv";
        QCOMPARE(paths(), (QStringList{none, e01, e02}));
        const QString noneLabel =
            episodes->data(episodes->index(0, 0), EpisodesModel::LabelRole).toString();
        QTRY_COMPARE(drawn(), (QStringList{noneLabel, "E01", "E02"}));

        QObject *last = w.find("noFindingsLast");
        QVERIFY(last);
        QCOMPARE(last->property("text").toString(), "&Episodes Without Findings Last");
        QVERIFY(!last->property("checked").toBool());
        QVERIFY(!w.settings.noFindingsLast());
        QVERIFY(QMetaObject::invokeMethod(last, "trigger"));
        QVERIFY(last->property("checked").toBool());
        QVERIFY(w.settings.noFindingsLast());
        QVERIFY(episodes->noFindingsLast());
        QCOMPARE(paths(), (QStringList{e01, e02, none}));
        QTRY_COMPARE(drawn(), (QStringList{"E01", "E02", noneLabel}));
        // The greyed entry says why wherever it is listed, in the disabled colour alone: the word
        // is the one sign of the grey that is not a colour, so it is not dimmed as other words are.
        QCOMPARE(Shown::text(w.item("episode2"), "entrySide"), "No findings");
        QQuickItem *greyedWord = Shown::item(w.item("episode2"), "entrySide");
        QQuickItem *word = Shown::item(w.item("episode0"), "entrySide");
        QVERIFY(greyedWord && word);
        QCOMPARE(greyedWord->opacity(), 1.0);
        QCOMPARE(word->opacity(), 0.7);

        const qsizetype from = w.bridge.sent.size();
        w.click(w.item("episode0"));
        QCOMPARE(w.since(from), (QStringList{"SelectFile " + e01, "ShowFileFindings"}));
        QTRY_COMPARE(w.bridge.episodeName(), "E01");
        QTRY_VERIFY(w.item("episode0")->property("highlighted").toBool());
        QVERIFY(!w.item("episode2")->property("highlighted").toBool());
        // The greyed entry, last now, opens its own file too, which has no finding to go to.
        const qsizetype quiet = w.bridge.sent.size();
        w.click(w.item("episode2"));
        QCOMPARE(w.since(quiet), QStringList{"SelectFile " + none});
        QTRY_COMPARE(w.bridge.episodeName(), noneLabel);
        QTRY_VERIFY(w.item("episode2")->property("highlighted").toBool());
        QVERIFY(!w.item("episode0")->property("highlighted").toBool());

        QTest::keyClick(w.window, Qt::Key_N);
        QCOMPARE(w.last(), "NextFile");
        QTest::keyClick(w.window, Qt::Key_P);
        QCOMPARE(w.last(), "PreviousFile");
    }

    // View ▸ Episodes Without Findings Last, on and then off again, keeps the marked episode in
    // view at its row in the new order, which the list would otherwise not scroll to. The episodes
    // are made up, in a window whose review is not started, at the least height, where the list
    // shows fewer than ten: the marked episode's two rows are twenty apart, and the nearer is ten
    // from the top.
    void theSortKeepsTheMarkedEpisodeInView() {
        Shown w({media()});
        QVERIFY(w.show());
        auto *files = qobject_cast<EpisodesModel *>(w.bridge.episodes()->sourceModel());
        QVERIFY(files);
        QVERIFY(files->apply(subcheck_ffi::FilesSplice{0, 0, manyEpisodes()}));
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->height(), 480);
        QQuickItem *sidebar = w.item("sidebar");
        QVERIFY(sidebar);
        QQuickItem *list = Shown::item(sidebar, "episodes");
        QVERIFY(list);
        EpisodeOrder *order = w.bridge.episodes();
        const QString marked = QStringLiteral("/m/S1/E30.mkv");
        // The marked episode's row in the list's order now, and the entry the list draws for it.
        const auto row = [order, &marked] {
            for (int each = 0; each < order->rowCount(); ++each) {
                if (order->data(order->index(each, 0), EpisodesModel::PathRole).toString() ==
                    marked) {
                    return each;
                }
            }
            return -1;
        };
        const auto drawn = [list](int at) {
            QQuickItem *each = nullptr;
            QMetaObject::invokeMethod(list, "itemAtIndex", Q_RETURN_ARG(QQuickItem *, each),
                                      Q_ARG(int, at));
            return each;
        };
        const auto entry = [&drawn, &row] { return drawn(row()); };
        const auto inView = [list](QQuickItem *of) {
            if (!of) {
                return false;
            }
            const qreal top = of->mapToItem(list, QPointF(0, 0)).y();
            return top >= -0.5 && top + of->height() <= list->height() + 0.5;
        };

        // Laid out at the least height.
        QTRY_VERIFY(drawn(0) && list->height() < 10 * drawn(0)->height());

        files->follow(marked.toStdString());
        QCOMPARE(row(), 30);
        // Scrolled to it, as a person would to find it: ListView.Beginning.
        const int beginning = 0;
        QVERIFY(QMetaObject::invokeMethod(list, "positionViewAtIndex", Q_ARG(int, row()),
                                          Q_ARG(int, beginning)));
        QTRY_VERIFY(inView(entry()));
        QVERIFY(entry()->property("highlighted").toBool());

        QObject *last = w.find("noFindingsLast");
        QVERIFY(last);
        QVERIFY(QMetaObject::invokeMethod(last, "trigger"));
        QVERIFY(order->noFindingsLast());
        QCOMPARE(row(), 10);
        QTRY_VERIFY(inView(entry()));
        QVERIFY(entry()->property("highlighted").toBool());

        QVERIFY(QMetaObject::invokeMethod(last, "trigger"));
        QVERIFY(!order->noFindingsLast());
        QCOMPARE(row(), 30);
        QTRY_VERIFY(inView(entry()));
        QVERIFY(entry()->property("highlighted").toBool());
    }

    // The sidebar's lists keep their marks in view as the marks move, which nothing else scrolls
    // them to: the marked episode, far down the list and then back at its top, and the open review
    // of a new list. The reviews and episodes are made up, in a window whose review is not started,
    // at the least size, where each list is longer than its view.
    void theSidebarsListsFollowTheirMarks() {
        Shown w({media()});
        QVERIFY(w.show());
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->height(), 480);
        auto *files = qobject_cast<EpisodesModel *>(w.bridge.episodes()->sourceModel());
        QVERIFY(files);
        QVERIFY(files->apply(subcheck_ffi::FilesSplice{0, 0, manyEpisodes()}));
        QQuickItem *episodes = w.item("episodes");
        QQuickItem *reviews = w.item("reviews");
        QVERIFY(episodes && reviews);
        // Laid out, showing fewer than ten from the top.
        QTRY_VERIFY(drawnEntry(episodes, 0) &&
                    episodes->height() < 10 * drawnEntry(episodes, 0)->height());
        QCOMPARE(episodes->property("contentY").toReal(), 0.0);

        files->follow("/m/S1/E35.mkv");
        QTRY_VERIFY(inList(episodes, drawnEntry(episodes, 35)));
        QVERIFY(drawnEntry(episodes, 35)->property("highlighted").toBool());

        files->follow("/m/S1/E00.mkv");
        QTRY_VERIFY(drawnEntry(episodes, 0) &&
                    qAbs(topInList(episodes, drawnEntry(episodes, 0))) < 0.5);
        QVERIFY(inList(episodes, drawnEntry(episodes, 0)));
        QVERIFY(drawnEntry(episodes, 0)->property("highlighted").toBool());

        // A new list of reviews, open at row 25, and the mark moved in the same go, as one change
        // of review's can bring them, the mark first: the reviews take room from the episodes,
        // and each list still shows its mark once both are laid out.
        std::vector<subcheck_ffi::FolderView> folders = manyReviews();
        folders[25].open = true;
        const qreal tall = episodes->height();
        files->follow("/m/S1/E35.mkv");
        w.bridge.folders()->set(folders);
        QTRY_VERIFY(inList(reviews, drawnEntry(reviews, 25)));
        QVERIFY(drawnEntry(reviews, 25)->property("highlighted").toBool());
        // Scrolled to it: the list shows only a few from its top.
        QVERIFY(reviews->property("contentY").toReal() > 0);
        QTRY_VERIFY(episodes->height() < tall);
        QTRY_VERIFY(inList(episodes, drawnEntry(episodes, 35)));
        QVERIFY(drawnEntry(episodes, 35)->property("highlighted").toBool());
    }

    // A click on an episode the check found nothing in selects its file alone: it has no finding
    // to go to, and review's word for that would take the status bar. What the check said of it
    // shows, scrolled into view, while review keeps the finding selected; and the sidebar marks the
    // episode clicked.
    void aClickOnAnEpisodeWithNoFindingsShowsItsLogAndKeepsTheFinding() {
        Shown w({clean(), media()});
        QSignalSpy shown(&w.bridge, &SessionBridge::episodeShown);
        clickE01AtTheLeastSize(w);
        if (QTest::currentTestFailed()) {
            return;
        }
        const QStringList e01Log = w.bridge.episodeLog();
        DetailView *detail = w.bridge.detail();
        QVERIFY(detail->shown() && detail->isOf("E01"));
        const QString place = detail->place();
        const QString label = w.bridge.episodes()
                                  ->data(w.bridge.episodes()->index(0, 0), EpisodesModel::LabelRole)
                                  .toString();
        QVERIFY(!label.isEmpty() && label != "E01");

        const qsizetype from = w.bridge.sent.size();
        w.click(w.item("episode0"));
        QCOMPARE(w.since(from), QStringList{"SelectFile " + noFindingsFile()});
        QTRY_COMPARE(w.bridge.episodeName(), label);
        QCOMPARE(shown.count(), 1);
        QQuickItem *heading = w.item("episodeLogHeading");
        QCOMPARE(heading->property("text").toString(), "What the check said of " + label);
        QVERIFY(heading->isVisible());
        QTRY_VERIFY2(inDetailsView(w, heading),
                     qPrintable(QStringLiteral("the heading starts at %1 of %2")
                                    .arg(topInDetails(w, heading))
                                    .arg(detailsView(w)->height())));
        QVERIFY(!w.bridge.episodeLog().isEmpty());
        QVERIFY(w.bridge.episodeLog() != e01Log);
        QTRY_COMPARE(Shown::lines(w.item("episodeLog")), w.bridge.episodeLog());
        QCOMPARE(w.bridge.selected(), 0);
        QVERIFY(detail->shown() && detail->isOf("E01"));
        QCOMPARE(detail->place(), place);
        QTRY_VERIFY(w.item("episode0")->property("highlighted").toBool());
        QVERIFY(!w.item("episode1")->property("highlighted").toBool());
        QCOMPARE(w.bridge.statusLine(), QString());
    }

    // As the view opens, review's file is its first episode's, here the one with no findings. A
    // click on it changes nothing of review's but its count, yet what its check said, never shown
    // till then, shows with review's answer and is scrolled into view.
    void aClickOnTheEpisodeSelectedAsTheViewOpensShowsItsLog() {
        Shown w({clean(), media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->height(), 480);
        QTRY_VERIFY(w.item("episode0") && w.item("episode1"));
        QQuickItem *heading = w.item("episodeLogHeading");
        QVERIFY(!heading->isVisible());
        QTRY_VERIFY(w.item("episode1")->property("highlighted").toBool());
        // The finding alone is taller than the panel at this size, so that a log after it would be
        // out of view.
        QTRY_VERIFY(detailsView(w)->property("contentHeight").toReal() > detailsView(w)->height());

        w.click(w.item("episode0"));
        QCOMPARE(w.bridge.sent, QStringList{"SelectFile " + noFindingsFile()});
        QTRY_VERIFY(heading->isVisible());
        // In the finding's place, at the top of what the panel scrolls; and in view.
        QTRY_VERIFY(!w.item("nowBlock")->isVisible());
        QTRY_COMPARE(heading->y(), 0.0);
        QTRY_COMPARE(Shown::lines(w.item("episodeLog")), w.bridge.episodeLog());
        QVERIFY2(inDetailsView(w, heading),
                 qPrintable(QStringLiteral("the heading starts at %1 of %2")
                                .arg(topInDetails(w, heading))
                                .arg(detailsView(w)->height())));
        QTRY_VERIFY(w.item("episode0")->property("highlighted").toBool());
        QVERIFY(!w.item("episode1")->property("highlighted").toBool());
    }

    // What the check said of an episode with no findings goes once another finding is selected,
    // and the sidebar marks that finding's episode again.
    void theLogGoesOnceTheSelectedFindingChanges() {
        Shown w({clean(), media()});
        clickE01AtTheLeastSize(w);
        if (QTest::currentTestFailed()) {
            return;
        }
        w.click(w.item("episode0"));
        QTRY_VERIFY(w.item("episode0")->property("highlighted").toBool());
        QVERIFY(!w.bridge.episodeName().isEmpty() && w.bridge.episodeName() != "E01");

        QTest::keyClick(w.window, Qt::Key_Down);
        QCOMPARE(w.last(), "Move 1");
        QTRY_COMPARE(w.bridge.selected(), 1);
        QVERIFY(w.bridge.detail()->isOf("E01"));
        QCOMPARE(w.bridge.episodeName(), QString());
        QCOMPARE(w.bridge.episodeLog(), QStringList());
        QTRY_VERIFY(!w.item("episodeLogHeading")->isVisible());
        QTRY_VERIFY(w.item("episode1")->property("highlighted").toBool());
        QVERIFY(!w.item("episode0")->property("highlighted").toBool());
    }

    // Under a filter that shows no finding, a click on an episode with no findings still shows
    // what its check said: alone in the details panel, with nothing of a finding, and Accept
    // greyed.
    void aClickOnAnEpisodeWithNoFindingsShowsItsLogWithNoFindingSelected() {
        Shown w({clean(), media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("episode0") && w.item("episode1") && w.item("filter2"));
        // E01's file first, so that review answers the click with a change.
        w.click(w.item("episode1"));
        QTRY_COMPARE(w.bridge.episodeName(), "E01");
        w.click(w.item("filter2"));
        QCOMPARE(w.last(), "SetFilter Accepted");
        QTRY_COMPARE(w.rows(), 0);
        QTRY_VERIFY(!w.bridge.detail()->shown());
        QQuickItem *heading = w.item("episodeLogHeading");
        QTRY_VERIFY(!heading->isVisible());

        const qsizetype from = w.bridge.sent.size();
        w.click(w.item("episode0"));
        QCOMPARE(w.since(from), QStringList{"SelectFile " + noFindingsFile()});
        QTRY_VERIFY(!w.bridge.episodeName().isEmpty());
        QCOMPARE(heading->property("text").toString(),
                 "What the check said of " + w.bridge.episodeName());
        QTRY_VERIFY(heading->isVisible());
        QTRY_VERIFY(inDetailsView(w, heading));
        QVERIFY(!w.bridge.episodeLog().isEmpty());
        QTRY_COMPARE(Shown::lines(w.item("episodeLog")), w.bridge.episodeLog());
        QQuickItem *accept = w.item("acceptButton");
        QVERIFY(accept->isVisible());
        QVERIFY(!accept->property("enabled").toBool());
        QTRY_VERIFY(w.item("episode0")->property("highlighted").toBool());
        QCOMPARE(w.bridge.statusLine(), QString());
    }

    // A click on an episode with no findings while a question is open only lets the question go,
    // as review then selects nothing: no log of it shows, and the mark stays. As the view opens,
    // that episode's file is the one selected, which the click names again; then with E01's
    // selected, which the click would change.
    void aClickOnAnEpisodeWithNoFindingsUnderAQuestionOnlyLetsItGo() {
        Shown w({clean(), media()});
        bool asked = false;
        connect(&w.bridge, &SessionBridge::eventArrived, &w.bridge,
                [&asked](const subcheck_ffi::Event &e) {
                    if (const auto *change =
                            std::get_if<subcheck_ffi::Event::kChanged>(&e.get_variant())) {
                        asked = change->summary.question.has_value();
                    }
                });
        QSignalSpy shown(&w.bridge, &SessionBridge::episodeShown);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("episode0") && w.item("episode1"));
        QTRY_VERIFY(w.item("episode1")->property("highlighted").toBool());
        QCOMPARE(w.bridge.episodeName(), QString());
        // Recheck asks first. The window has no way to ask it yet, so the test sends it.
        w.bridge.send(subcheck_ffi::Action(subcheck_ffi::Action::kRecheck{}));
        QTRY_VERIFY(asked);
        qsizetype from = w.bridge.sent.size();
        w.click(w.item("episode0"));
        QCOMPARE(w.since(from), QStringList{"SelectFile " + noFindingsFile()});
        QTRY_VERIFY(!asked);
        QCOMPARE(w.bridge.episodeName(), QString());
        QVERIFY(!w.item("episodeLogHeading")->isVisible());
        QVERIFY(w.item("episode1")->property("highlighted").toBool());
        QVERIFY(!w.item("episode0")->property("highlighted").toBool());

        w.click(w.item("episode1"));
        QTRY_COMPARE(w.bridge.episodeName(), "E01");
        w.bridge.send(subcheck_ffi::Action(subcheck_ffi::Action::kRecheck{}));
        QTRY_VERIFY(asked);
        from = w.bridge.sent.size();
        w.click(w.item("episode0"));
        QCOMPARE(w.since(from), QStringList{"SelectFile " + noFindingsFile()});
        QTRY_VERIFY(!asked);
        QCOMPARE(w.bridge.episodeName(), "E01");
        QCOMPARE(w.item("episodeLogHeading")->property("text").toString(),
                 "What the check said of E01");
        QVERIFY(w.item("episode1")->property("highlighted").toBool());
        QVERIFY(!w.item("episode0")->property("highlighted").toBool());
        QCOMPARE(shown.count(), 0);
    }

    // A click on an episode the check found nothing in, with a finding selected, shows what its
    // check said alone under the video: nothing of the finding, which review still has selected,
    // and no line above the log, which has nothing above it to set it off from. Reject and Accept
    // stay under the video, greyed. The list draws the finding's row in the inactive grey.
    void aGreyedEpisodesViewShowsItsLogAlone() {
        Shown w({clean(), media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("episode0") && w.item("episode1"));
        QTRY_VERIFY(w.item("nowBlock")->isVisible());
        const QString label = w.bridge.episodes()
                                  ->data(w.bridge.episodes()->index(0, 0), EpisodesModel::LabelRole)
                                  .toString();
        w.click(w.item("episode0"));
        QTRY_VERIFY(w.bridge.showingEpisode());
        QCOMPARE(w.bridge.selected(), 0);
        for (const char *name : {"nowBlock", "fixedBlock", "detailKind"}) {
            QTRY_VERIFY2(!w.item(name)->isVisible(), name);
        }
        for (const char *name : {"acceptButton", "rejectButton"}) {
            QVERIFY2(w.item(name)->isVisible(), name);
            QVERIFY2(!w.item(name)->property("enabled").toBool(), name);
        }
        QQuickItem *heading = w.item("episodeLogHeading");
        QVERIFY(heading->isVisible());
        QCOMPARE(heading->property("text").toString(), "What the check said of " + label);
        QTRY_COMPARE(Shown::lines(w.item("episodeLog")), w.bridge.episodeLog());
        QQuickItem *divider = w.item("episodeLogDivider");
        QVERIFY(divider);
        QVERIFY(!divider->isVisible());

        const QColor mid = paletteColour(w, "mid");
        QVERIFY(mid != paletteColour(w, "highlight"));
        QTRY_COMPARE(selectionOutline(w), mid);
        auto *selection = w.find("findings")->property("highlightItem").value<QQuickItem *>();
        const QColor tint = selection->property("color").value<QColor>();
        QCOMPARE(tint.rgb(), mid.rgb());
        QVERIFY(qAbs(tint.alphaF() - 0.3) < 0.01);
    }

    // While a greyed episode's view shows, the keys, menu items and buttons that act on the finding
    // wait, as the finding is not shown: Accept, Reject, Back to Undecided, Edit Wording…, Show in
    // Video, Show Each Episode, by Return or Enter, and Copy Subtitle Line, by Ctrl+C. The rest
    // still work. Undo, which takes a decision back and with it the selection, ends the view.
    void theFindingKeysWaitWhileAnEpisodesViewShows() {
        Shown w({clean(), media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("episode0"));
        w.click(w.item("episode0"));
        QTRY_VERIFY(w.bridge.showingEpisode());

        const qsizetype from = w.bridge.sent.size();
        QGuiApplication::clipboard()->setText(QStringLiteral("held before the view"));
        for (const Qt::Key key :
             {Qt::Key_A, Qt::Key_R, Qt::Key_X, Qt::Key_E, Qt::Key_O, Qt::Key_Return}) {
            QTest::keyClick(w.window, key);
        }
        QTest::keyClick(w.window, Qt::Key_Enter, Qt::KeypadModifier);
        QTest::keyClick(w.window, Qt::Key_C, Qt::ControlModifier);
        QCOMPARE(w.since(from), QStringList());
        QCOMPARE(QGuiApplication::clipboard()->text(), "held before the view");
        for (const char *name :
             {"accept", "reject", "clear", "edit", "play", "showEachEpisode", "copyLine"}) {
            QObject *action = w.find(name);
            QVERIFY2(action, name);
            QVERIFY2(!action->property("enabled").toBool(), name);
        }
        QVERIFY(w.find("undo")->property("enabled").toBool());
        QVERIFY(w.find("nextEpisode")->property("enabled").toBool());

        QTest::keyClick(w.window, Qt::Key_O, Qt::ShiftModifier);
        QCOMPARE(w.since(from), QStringList{"ClosePlayer"});
        QTest::keyClick(w.window, Qt::Key_Space);
        QCOMPARE(w.since(from), (QStringList{"ClosePlayer", "Playback Pause"}));
        QVERIFY(w.bridge.showingEpisode());

        QTest::keyClick(w.window, Qt::Key_U);
        QCOMPARE(w.since(from), (QStringList{"ClosePlayer", "Playback Pause", "Undo"}));
        QTRY_VERIFY(!w.bridge.showingEpisode());
        QTRY_VERIFY(w.item("nowBlock")->isVisible());
        QTRY_COMPARE(selectionOutline(w), paletteColour(w, "highlight"));
        for (const char *name :
             {"accept", "reject", "clear", "edit", "play", "showEachEpisode", "copyLine"}) {
            QVERIFY2(w.find(name)->property("enabled").toBool(), name);
        }

        // A right-click on a row ends the view as a click does, once review answers its Select;
        // the menu's items then act on that row.
        QObject *menu = w.find("rowMenu");
        QVERIFY(menu);
        w.click(w.item("episode0"));
        QTRY_VERIFY(w.bridge.showingEpisode());
        QTRY_VERIFY(w.delegate(1));
        const QString select = selecting(w, 1);
        const qsizetype right = w.bridge.sent.size();
        w.rightClick(w.delegate(1));
        QCOMPARE(w.since(right), QStringList{select});
        QTRY_VERIFY(menu->property("opened").toBool());
        QTRY_VERIFY(!w.bridge.showingEpisode());
        QCOMPARE(w.bridge.selected(), 1);
        QQuickItem *accept = rowMenuItem(w, "rowAccept");
        QVERIFY(accept);
        QTRY_VERIFY(accept->property("enabled").toBool());
        w.click(accept);
        QCOMPARE(w.since(right), (QStringList{select, "Accept"}));
        QTRY_COMPARE(w.decision(1), "Accepted");
        w.bridge.undo();
        QTRY_COMPARE(w.decision(1), "To review");
    }

    // A click on the grey row, the row already selected, moves nothing in review, and still ends a
    // greyed episode's view with review's answer: its finding shows again, Accept with it, and the
    // sidebar marks the finding's episode.
    void aClickOnTheGreyRowEndsTheEpisodesView() {
        Shown w({clean(), media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("episode0") && w.item("episode1") && w.delegate(0));
        QTRY_VERIFY(w.item("episode1")->property("highlighted").toBool());
        w.click(w.item("episode0"));
        QTRY_VERIFY(w.bridge.showingEpisode());
        QTRY_VERIFY(w.item("episode0")->property("highlighted").toBool());
        QTRY_VERIFY(!w.item("nowBlock")->isVisible());

        const qsizetype from = w.bridge.sent.size();
        w.click(w.delegate(w.bridge.selected()));
        const QStringList sent = w.since(from);
        QVERIFY2(sent.size() == 1 && sent.first().startsWith("Select row "),
                 qPrintable(sent.join(" | ")));
        QTRY_VERIFY(!w.bridge.showingEpisode());
        QTRY_VERIFY(w.item("nowBlock")->isVisible());
        QVERIFY(w.item("acceptButton")->property("enabled").toBool());
        QTRY_VERIFY(w.item("episode1")->property("highlighted").toBool());
        QVERIFY(!w.item("episode0")->property("highlighted").toBool());
        QCOMPARE(w.bridge.selected(), 0);
    }

    // A greyed episode's view scrolled down its log ends at a click on the grey row, which moves no
    // selection: the finding comes back at its top, as another finding opens at its own. The
    // details column is made wide enough that its video leaves the log too little room.
    void theFindingComesBackAtItsTopWhenAnEpisodesViewEnds() {
        Shown w({clean(), media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("episode0") && w.delegate(0));
        w.window->resize(1400, 480);
        QTRY_COMPARE(w.window->size(), QSize(1400, 480));
        QQuickItem *details = w.item("details");
        QTRY_VERIFY(details->height() < 480);
        const int wide = qRound((details->height() - 24) * 16 / 9);
        w.settings.keepColumns(w.settings.sidebarWidth(), wide);
        QTRY_COMPARE(qRound(details->width()), wide);

        w.click(w.item("episode0"));
        QTRY_VERIFY(w.bridge.showingEpisode());
        QTRY_VERIFY(!w.item("nowBlock")->isVisible());
        QQuickItem *view = detailsView(w);
        QTRY_VERIFY(view->property("contentHeight").toReal() - view->height() > 10);
        const qreal down = view->property("contentHeight").toReal() - view->height();
        view->setProperty("contentY", down);
        QCOMPARE(view->property("contentY").toReal(), down);

        const qsizetype from = w.bridge.sent.size();
        w.click(w.delegate(w.bridge.selected()));
        QVERIFY(w.since(from).size() == 1 && w.since(from).first().startsWith("Select row "));
        QTRY_VERIFY(!w.bridge.showingEpisode());
        QTRY_VERIFY(w.item("nowBlock")->isVisible());
        QTRY_COMPARE(view->property("contentY").toReal(), 0.0);
    }

    // J at the last finding moves nothing, and review's answer says nothing else: the view still
    // ends with it, and the finding and its keys are back.
    void jAtTheLastFindingEndsTheEpisodesView() {
        Shown w({clean(), media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("episode0") && w.item("episode1"));
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(w.bridge.selected(), 1);
        w.click(w.item("episode0"));
        QTRY_VERIFY(w.bridge.showingEpisode());
        QTRY_VERIFY(!w.item("nowBlock")->isVisible());

        const qsizetype from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_J);
        QCOMPARE(w.since(from), QStringList{"Move 1"});
        QTRY_VERIFY(w.item("nowBlock")->isVisible());
        QCOMPARE(selectionOutline(w), paletteColour(w, "highlight"));
        QVERIFY(w.find("accept")->property("enabled").toBool());
        QTRY_VERIFY(w.item("episode1")->property("highlighted").toBool());
        QCOMPARE(w.bridge.selected(), 1);
    }

    // A list longer than its view has a scroll bar, the style's own, and the rows give way to it
    // while it shows, so that it covers none of their words: at the least size, where a row is at
    // its narrowest, its state still ends before the bar. The highlight that marks the selected row
    // is as wide as the rows, with the bar and without it.
    void aLongListOfFindingsHasAScrollBar() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->height(), 480);
        auto *list = qobject_cast<QQuickItem *>(w.find("findings"));
        QQuickItem *bar = w.item("findingsScrollBar");
        QVERIFY(list && bar);
        QCOMPARE(bar->property("orientation").toInt(), int(Qt::Vertical));
        QTRY_COMPARE(bar->property("size").toReal(), 1.0);
        QTRY_VERIFY(w.delegate(0));
        QCOMPARE(w.delegate(0)->width(), list->width());
        auto *selection = list->property("highlightItem").value<QQuickItem *>();
        QVERIFY(selection);
        QCOMPARE(selection->width(), w.delegate(0)->width());
        // Rows made up after review's two, enough for the list to scroll.
        std::vector<subcheck_ffi::EntryView> more;
        for (uint64_t id = 100; id < 120; ++id) {
            more.push_back(rowOfTwelve(subcheck_ffi::EntryKey(subcheck_ffi::EntryKey::kRow{{id}})));
        }
        QVERIFY(w.bridge.findings()->apply(subcheck_ffi::Splice{2, 0, more}));
        QTRY_VERIFY(list->property("contentHeight").toReal() > list->height());
        QTRY_VERIFY(bar->property("size").toReal() < 1);
        QVERIFY(bar->isVisible());
        QTRY_VERIFY(w.delegate(0));
        QTRY_COMPARE(w.delegate(0)->width(), list->width() - bar->width());
        QTRY_COMPARE(selection->width(), w.delegate(0)->width());
        QQuickItem *state = Shown::item(w.delegate(0), "rowState");
        QVERIFY(state);
        const auto stateEnds = [list, state] {
            return state->mapToItem(list, QPointF(state->width(), 0)).x();
        };
        QTRY_VERIFY2(stateEnds() <= bar->x() + 0.5,
                     qPrintable(QStringLiteral("the state ends at %1, the bar starts at %2")
                                    .arg(stateEnds())
                                    .arg(bar->x())));
    }

    // The sidebar's lists give way to their scroll bars too, while they show: a review's count and
    // an episode's word end before the bar. The reviews and episodes are made up, in a window whose
    // review is not started, at the least size, where each list is longer than its view.
    void theSidebarsEntriesGiveWayToTheirScrollBars() {
        Shown w({media()});
        QVERIFY(w.show());
        w.bridge.folders()->set(manyReviews(12));
        auto *files = qobject_cast<EpisodesModel *>(w.bridge.episodes()->sourceModel());
        QVERIFY(files);
        QVERIFY(files->apply(subcheck_ffi::FilesSplice{0, 0, manyEpisodes()}));
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->height(), 480);
        for (const auto &[listName, barName, entryName] :
             {std::tuple{"reviews", "reviewsScrollBar", "review0"},
              std::tuple{"episodes", "episodesScrollBar", "episode0"}}) {
            QQuickItem *list = w.item(listName);
            QQuickItem *bar = w.item(barName);
            QVERIFY2(list && bar, barName);
            QTRY_VERIFY2(bar->property("size").toReal() < 1, barName);
            QVERIFY2(bar->isVisible(), barName);
            QTRY_VERIFY2(w.item(entryName), entryName);
            QQuickItem *entry = w.item(entryName);
            QTRY_COMPARE(entry->width(), list->width() - bar->width());
            QQuickItem *side = Shown::item(entry, "entrySide");
            QVERIFY2(side && !side->property("text").toString().isEmpty(), entryName);
            const qreal sideEnds = side->mapToItem(list, QPointF(side->width(), 0)).x();
            QVERIFY2(sideEnds <= bar->x() + 0.5,
                     qPrintable(QStringLiteral("%1's word ends at %2, the bar starts at %3")
                                    .arg(entryName)
                                    .arg(sideEnds)
                                    .arg(bar->x())));
        }
    }

    // The log of the selected finding's own episode sits below the finding, set off by a line in
    // the colour of the window's other lines.
    void theLogUnderAFindingSitsBelowADivider() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.bridge.detail()->shown());
        QQuickItem *heading = w.item("episodeLogHeading");
        QQuickItem *divider = w.item("episodeLogDivider");
        QVERIFY(heading && divider);
        QTRY_VERIFY(heading->isVisible());
        QVERIFY(w.item("nowBlock")->isVisible());
        QVERIFY(divider->isVisible());
        QCOMPARE(divider->height(), 1.0);
        QCOMPARE(divider->property("color").value<QColor>(),
                 w.window->property("separator").value<QColor>());
        QTRY_VERIFY2(topInDetails(w, divider) < topInDetails(w, heading),
                     qPrintable(QStringLiteral("the line starts at %1, the heading at %2")
                                    .arg(topInDetails(w, divider))
                                    .arg(topInDetails(w, heading))));
        QVERIFY(topInDetails(w, w.item("technical")) < topInDetails(w, divider));
    }

    // Each review's path shows as its tooltip while the pointer is on it. Whether controls follow
    // the pointer is the platform's to say, and the offscreen one says not, so this test asks.
    void aReviewsPathIsItsTooltip() {
        QStyleHints *hints = QGuiApplication::styleHints();
        const auto restore = qScopeGuard(
            [hints, was = hints->useHoverEffects()] { hints->setUseHoverEffects(was); });
        hints->setUseHoverEffects(true);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("review1"));
        FoldersModel *folders = w.bridge.folders();
        for (const int row : {0, 1}) {
            const QString name = QStringLiteral("review%1").arg(row);
            QQuickItem *review = w.item(qPrintable(name));
            QVERIFY(review);
            auto *tip = review->findChild<QObject *>("reviewPath");
            QVERIFY(tip);
            QVERIFY(!tip->property("visible").toBool());
            QTest::mouseMove(w.window, w.middle(review));
            // Long past the tooltip's own delay, however busy the machine.
            QTRY_VERIFY_WITH_TIMEOUT(tip->property("visible").toBool(), 15000);
            const QString path =
                folders->data(folders->index(row), FoldersModel::PathRole).toString();
            QCOMPARE(tip->property("text").toString(), path);
            auto *drawn = tip->property("contentItem").value<QQuickItem *>();
            QVERIFY(drawn);
            QCOMPARE(drawn->property("text").toString(), path);
        }
    }

    // A row and the details panel each draw what review says of the finding, every part in its
    // own place.
    void aRowAndTheDetailsDrawWhatReviewSays() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        FindingsModel *list = w.bridge.findings();
        for (const int row : {0, 1}) {
            QTRY_VERIFY(w.delegate(row));
            QQuickItem *drawn = w.delegate(row);
            const auto says = [list, row](int role) {
                return list->data(list->index(row), role).toString();
            };
            QVERIFY(!says(FindingsModel::SeveralRole).isEmpty());
            QCOMPARE(Shown::text(drawn, "rowKind"), says(FindingsModel::KindRole));
            QCOMPARE(Shown::text(drawn, "rowSeveral"), says(FindingsModel::SeveralRole));
            QCOMPARE(Shown::text(drawn, "rowPlace"), says(FindingsModel::PlaceRole));
            QCOMPARE(Shown::text(drawn, "rowState"),
                     says(FindingsModel::GlyphRole) + " " + says(FindingsModel::StateWordRole));
            const auto colour = [list, row](int role) {
                return list->data(list->index(row), role).value<QColor>();
            };
            QCOMPARE(Shown::item(drawn, "rowState")->property("color").value<QColor>(),
                     colour(FindingsModel::StateToneRole));
            QCOMPARE(Shown::item(drawn, "badge")->property("color").value<QColor>(),
                     colour(FindingsModel::KindToneRole));
        }
        QTRY_COMPARE(w.window->title(), w.bridge.title());
        QCOMPARE(w.window->title(), "media");

        // The selected row: the style's selection colour as a tint, outlined at full strength.
        auto *selection = w.find("findings")->property("highlightItem").value<QQuickItem *>();
        QVERIFY(selection);
        const QColor highlight =
            w.window->property("palette").value<QObject *>()->property("highlight").value<QColor>();
        const QColor tint = selection->property("color").value<QColor>();
        QCOMPARE(tint.rgb(), highlight.rgb());
        QVERIFY(qAbs(tint.alphaF() - 0.3) < 0.01);
        auto *outline = selection->property("border").value<QObject *>();
        QVERIFY(outline);
        QCOMPARE(outline->property("color").value<QColor>(), highlight);
        QCOMPARE(outline->property("width").toInt(), 1);

        // A row's parts start where its triangle's column ends; a member, which has none, sits in
        // under its row by the triangle's glyph and the gap after it.
        const auto sitsAt = [&w](int row) -> qreal {
            QQuickItem *drawn = w.delegate(row);
            QQuickItem *line = drawn ? Shown::item(drawn, "rowLine") : nullptr;
            return line ? line->mapToItem(drawn, QPointF(0, 0)).x() : -1;
        };
        QQuickItem *triangle = Shown::item(w.delegate(0), "rowDisclosure");
        QVERIFY(triangle);
        const qreal start = triangle->mapToItem(w.delegate(0), QPointF(triangle->width(), 0)).x();
        const qreal glyph = triangle->property("glyphWidth").toReal();
        QVERIFY(glyph > 0);
        QTRY_COMPARE(sitsAt(0), start);
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 4);
        QTRY_COMPARE(sitsAt(1), start + glyph + 6);
        QTRY_COMPARE(sitsAt(0), start);
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 2);

        DetailView *detail = w.bridge.detail();
        QTRY_VERIFY(detail->shown());
        QQuickItem *panel = w.item("details");
        QVERIFY(panel);
        // Parts unlike each other, so that one drawn in another's place shows.
        QVERIFY(detail->now() != detail->fixed());
        QVERIFY(!detail->reason().isEmpty());
        QVERIFY(detail->reason() != detail->mark());
        QCOMPARE(Shown::text(panel, "detailKind"), detail->kind());
        QCOMPARE(Shown::text(panel, "place"), detail->place());
        QCOMPARE(Shown::text(panel, "state"), detail->glyph() + " " + detail->stateWord());
        QQuickItem *now = Shown::item(panel, "nowBlock");
        QQuickItem *fixed = Shown::item(panel, "fixedBlock");
        QVERIFY(now && fixed);
        QCOMPARE(Shown::text(now, "blockLabel"), "− In the subtitles now");
        QCOMPARE(Shown::text(now, "blockLine"), detail->now());
        QCOMPARE(Shown::text(fixed, "blockLabel"), "+ With the fix");
        QCOMPARE(Shown::text(fixed, "blockLine"), detail->fixed());
        QCOMPARE(Shown::text(panel, "reason"), detail->reason());
        QCOMPARE(Shown::text(panel, "mark"), detail->mark());
    }

    // Each row style draws its parts of a row from what review says of it. Compact, on one line
    // and so shorter than Two Lines' row: the state's glyph, where the row is in short, and its
    // change. Rich, taller: Two Lines' two lines and why the row was found, beside a dark tile
    // with its line, 16:9 and as tall as the lines, but no wider than a quarter of the row, so
    // that a narrow list keeps room for the lines.
    void eachRowStyleDrawsItsParts() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        FindingsModel *list = w.bridge.findings();
        const auto says = [list](int row, int role) { return list->data(list->index(row), role); };
        QTRY_VERIFY(w.delegate(0) && w.delegate(1));
        const qreal twoLines[] = {w.delegate(0)->height(), w.delegate(1)->height()};
        // Row 1's place, as Two Lines words it and in short.
        QCOMPARE(says(1, FindingsModel::PlaceRole).toString(), "E01 · 0:20");
        QCOMPARE(says(1, FindingsModel::ShortPlaceRole).toString(), "E01 0:20");

        w.settings.setRowStyle("compact");
        for (const int row : {0, 1}) {
            QTRY_VERIFY(w.delegate(row) && drawnStyle(w.delegate(row)) == "compact");
            QQuickItem *drawn = w.delegate(row);
            QCOMPARE(Shown::text(drawn, "rowGlyph"),
                     says(row, FindingsModel::GlyphRole).toString());
            QCOMPARE(Shown::item(drawn, "rowGlyph")->property("color").value<QColor>(),
                     says(row, FindingsModel::StateToneRole).value<QColor>());
            QCOMPARE(Shown::text(drawn, "rowShortPlace"),
                     says(row, FindingsModel::ShortPlaceRole).toString());
            QCOMPARE(Shown::text(drawn, "rowChange"),
                     says(row, FindingsModel::ChangeRole).toString());
            QVERIFY(!Shown::item(drawn, "rowLine"));
            QVERIFY2(drawn->height() < twoLines[row],
                     qPrintable(
                         QStringLiteral("%1 against %2").arg(drawn->height()).arg(twoLines[row])));
        }
        QCOMPARE(Shown::text(w.delegate(1), "rowShortPlace"), "E01 0:20");

        w.settings.setRowStyle("rich");
        const QColor separator = w.window->property("separator").value<QColor>();
        // The tile's width against the row's lines' height, and its height against its width.
        const auto tileIn = [](QQuickItem *row) {
            QQuickItem *tile = Shown::item(row, "rowTile");
            QQuickItem *lines = Shown::item(row, "rowReason")->parentItem();
            return QStringLiteral("a tile %1 by %2 beside lines %3 high in a row %4 wide")
                .arg(tile->width())
                .arg(tile->height())
                .arg(lines->height())
                .arg(row->width());
        };
        for (const int row : {0, 1}) {
            QTRY_VERIFY(w.delegate(row) && drawnStyle(w.delegate(row)) == "rich");
            QQuickItem *drawn = w.delegate(row);
            QQuickItem *tile = Shown::item(drawn, "rowTile");
            QQuickItem *tileLine = Shown::item(drawn, "rowTileLine");
            QQuickItem *reason = Shown::item(drawn, "rowReason");
            QVERIFY(tile && tileLine && reason);
            QVERIFY(!says(row, FindingsModel::ReasonRole).toString().isEmpty());
            QCOMPARE(Shown::text(drawn, "rowTileLine"),
                     says(row, FindingsModel::TileLineRole).toString());
            QCOMPARE(tileLine->property("color").value<QColor>(), QColor("#f0f0f0"));
            QCOMPARE(tileLine->property("font").value<QFont>().pointSizeF(),
                     reason->property("font").value<QFont>().pointSizeF() - 2);
            QCOMPARE(tile->property("color").value<QColor>(), QColor("#202020"));
            auto *edge = tile->property("border").value<QObject *>();
            QCOMPARE(edge->property("width").toInt(), 1);
            QCOMPARE(edge->property("color").value<QColor>(), separator);
            QCOMPARE(Shown::text(drawn, "rowReason"),
                     says(row, FindingsModel::ReasonRole).toString());
            QVERIFY(reason->isVisible());
            QVERIFY(Shown::item(drawn, "rowLine")->isVisible());
            QVERIFY2(drawn->height() > twoLines[row],
                     qPrintable(
                         QStringLiteral("%1 against %2").arg(drawn->height()).arg(twoLines[row])));
            QQuickItem *lines = reason->parentItem();
            QTRY_VERIFY2(qAbs(tile->width() - lines->height() * 16 / 9) <= 1,
                         qPrintable(tileIn(drawn)));
            QVERIFY2(qAbs(tile->height() - tile->width() * 9 / 16) <= 1, qPrintable(tileIn(drawn)));
        }

        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(qRound(w.item("findings")->width()), 260);
        for (const int row : {0, 1}) {
            QQuickItem *drawn = w.delegate(row);
            QQuickItem *tile = Shown::item(drawn, "rowTile");
            QTRY_VERIFY2(tile->width() <= drawn->width() / 4 + 0.5, qPrintable(tileIn(drawn)));
            QVERIFY2(qAbs(tile->height() - tile->width() * 9 / 16) <= 1, qPrintable(tileIn(drawn)));
        }
    }

    // Every part of a finding is drawn in the details panel, from what review says of it: the
    // lines around it with the line itself between them, what sets its file apart, its row's
    // episodes, a timing finding's stretches, and what the check said of its episode.
    void everyPartOfAFindingIsDrawnInTheDetails() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        DetailView *detail = w.bridge.detail();
        QTRY_VERIFY(detail->shown());
        QQuickItem *panel = w.item("details");
        QVERIFY(panel);
        const auto drawn = [panel](const char *name) {
            return Shown::lines(Shown::item(panel, name));
        };

        QTRY_VERIFY(!w.bridge.episodeLog().isEmpty());
        QQuickItem *heading = Shown::item(panel, "episodeLogHeading");
        QVERIFY(heading);
        QTRY_VERIFY(heading->isVisible());
        QCOMPARE(heading->property("text").toString(),
                 "What the check said of " + w.bridge.episodeName());
        QTRY_COMPARE(drawn("episodeLog"), w.bridge.episodeLog());

        subcheck_ffi::Detail finding = everyPartOfAFinding();
        detail->show(finding);
        QVERIFY(!detail->before().isEmpty() && !detail->after().isEmpty());
        QVERIFY(detail->before() != detail->after());
        QVERIFY(!detail->banners().isEmpty());
        QVERIFY(!detail->episodes().isEmpty());
        QTRY_COMPARE(drawn("before"), detail->before());
        QTRY_COMPARE(drawn("after"), detail->after());
        QTRY_COMPARE(drawn("banners"), detail->banners());
        QCOMPARE(Shown::text(panel, "between"), detail->now());
        QCOMPARE(Shown::text(panel, "episodes"), detail->episodes());
        QCOMPARE(Shown::item(panel, "state")->property("color").value<QColor>(),
                 detail->stateTone());

        finding.kind = subcheck_ffi::Kind(subcheck_ffi::Kind::kTiming{
            {{2, std::nullopt, 60'000}}, {{1'000, 2'000}}, std::nullopt});
        detail->show(finding);
        QVERIFY(!detail->timing().isEmpty());
        QTRY_COMPARE(drawn("timing"), detail->timing());
    }

    // Another finding opens at the top of the details panel, however far the last was scrolled;
    // the same finding, refreshed in place by a decision on it, stays where it was.
    void anotherFindingOpensAtTheTopOfTheDetails() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->height(), 480);
        auto *scroll = w.item("detailsScroll")->property("contentItem").value<QQuickItem *>();
        QVERIFY(scroll);
        const qreal down = 40;
        QTRY_VERIFY(scroll->property("contentHeight").toReal() - scroll->height() > down);

        // The last row: a decision on it has no row to move on to, so it refreshes it in place.
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(w.bridge.selected(), 1);
        scroll->setProperty("contentY", down);
        QCOMPARE(scroll->property("contentY").toReal(), down);
        QTest::keyClick(w.window, Qt::Key_A);
        QTRY_COMPARE(w.bridge.detail()->stateWord(), "Accepted");
        QCOMPARE(w.bridge.selected(), 1);
        QCOMPARE(scroll->property("contentY").toReal(), down);
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.bridge.detail()->stateWord(), "To review");
        QCOMPARE(w.bridge.selected(), 1);
        QCOMPARE(scroll->property("contentY").toReal(), down);

        QTest::keyClick(w.window, Qt::Key_K);
        QTRY_COMPARE(w.bridge.selected(), 0);
        QTRY_COMPARE(scroll->property("contentY").toReal(), 0.0);
    }

    // However long a row's count of places, at the list's least width its state stays inside
    // the row: a state is never shown by colour alone, so its glyph and word must be in view.
    void aNarrowRowKeepsItsStateInView() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(qRound(w.item("findings")->width()), 260);
        FindingsModel *findings = w.bridge.findings();
        const std::optional<subcheck_ffi::EntryKey> key = findings->keyAt(0);
        QVERIFY(key);
        QVERIFY(findings->apply(subcheck_ffi::Splice{0, 1, {rowOfTwelve(*key)}}));
        const QString several = "In 12 places, 4 shown";
        QTRY_VERIFY(w.delegate(0) && Shown::text(w.delegate(0), "rowSeveral") == several);
        QQuickItem *row = w.delegate(0);
        QQuickItem *count = Shown::item(row, "rowSeveral");
        QQuickItem *state = Shown::item(row, "rowState");
        // The row lays itself out in more than one pass, so what is waited for is the end of it:
        // the count cut short, and the state inside the row's margin.
        const auto stateEnds = [row, state] {
            return state->mapToItem(row, QPointF(state->width(), 0)).x();
        };
        QTRY_VERIFY2(count->property("truncated").toBool() && stateEnds() <= row->width() - 8 + 0.5,
                     qPrintable(QStringLiteral("the state ends at %1").arg(stateEnds())));

        // With room to spare, the count is drawn whole and takes no more than that.
        w.window->resize(1280, 800);
        QTRY_VERIFY(row->width() > 400);
        QTRY_VERIFY(!count->property("truncated").toBool());
        QTRY_COMPARE(qRound(count->width()), qCeil(count->implicitWidth()));
    }

    // A row's fixes are one line like any other: too many for the row, they end at its margin.
    void aLongListOfFixesElidesAtTheRowsEdge() {
        using subcheck_ffi::Fix;
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(qRound(w.item("findings")->width()), 260);
        FindingsModel *findings = w.bridge.findings();
        const std::optional<subcheck_ffi::EntryKey> key = findings->keyAt(0);
        QVERIFY(key);
        QVERIFY(findings->apply(
            subcheck_ffi::Splice{0,
                                 1,
                                 {rowOfTwelve(*key, {{Fix(Fix::kShift{4}), 5},
                                                     {Fix(Fix::kShift{3}), 3},
                                                     {Fix(Fix::kShift{2}), 2},
                                                     {Fix(Fix::kShift{1}), 1},
                                                     {Fix(Fix::kPartial{}), 1}})}}));
        const QString said = "4 frames late × 5 · 3 frames late × 3 · 2 frames late × 2 · "
                             "1 frame late × 1 · partly off × 1";
        QTRY_VERIFY(w.delegate(0) && Shown::text(w.delegate(0), "rowLine") == said);
        QQuickItem *row = w.delegate(0);
        QQuickItem *line = Shown::item(row, "rowLine");
        QTRY_VERIFY(line->property("truncated").toBool());
        QVERIFY(line->mapToItem(row, QPointF(line->width(), 0)).x() <= row->width() - 8 + 0.5);
    }

    // A click held past a long press is still a click on its row.
    void aClickHeldLongStillSelectsItsRow() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.delegate(1));
        const QPoint at = w.middle(w.delegate(1));
        QTest::mousePress(w.window, Qt::LeftButton, {}, at);
        QTest::qWait(QGuiApplication::styleHints()->mousePressAndHoldInterval() + 200);
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, at);
        QTRY_COMPARE(w.find("findings")->property("currentIndex").toInt(), 1);
        QVERIFY2(w.last().startsWith("Select row "), qPrintable(w.last()));
    }

    // Whatever the finding's lines, the details panel wraps them at its width and never scrolls
    // sideways: its technical details hold the longest, a file's whole path.
    void theDetailsPanelScrollsOnlyUpAndDown() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *scroll = w.item("detailsScroll");
        QQuickItem *technical = w.item("technical");
        QVERIFY(scroll && technical);
        w.click(technical);
        QTRY_VERIFY(w.item("technicalLines")->isVisible());
        QTRY_COMPARE(scroll->property("contentWidth").toReal(),
                     scroll->property("availableWidth").toReal());
    }

    // The toolbar holds the view's name and counts at its left, the Find box at its right, and no
    // decision. At the window's least width, at the platform's text size and at the largest
    // Settings offers, the Find field and its ‹ and › lie after the title and counts, left to
    // right, each whole in the bar, at the window's size of text. A title and counts too long for
    // the bar are cut, and leave the box its width. "No match" shows whole inside the field's right
    // end, clear of the words typed, and moves nothing.
    void theToolbarHoldsTheTitleCountsAndFind_data() {
        QTest::addColumn<bool>("largest");
        QTest::newRow("the platform's size") << false;
        QTest::newRow("the largest size") << true;
    }
    void theToolbarHoldsTheTitleCountsAndFind() {
        QFETCH(bool, largest);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        if (largest) {
            w.settings.setWindowTextSize(w.settings.mostHalfPoints() / 2.0);
            QCOMPARE(w.settings.windowTextSize(), 24.0);
        }
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->width(), w.window->minimumWidth());
        QQuickItem *toolbar = w.item("toolbar");
        QQuickItem *title = w.item("title");
        QQuickItem *counts = w.item("counts");
        QQuickItem *box = w.item("findBox");
        QQuickItem *field = w.item("findField");
        QQuickItem *previous = w.item("findPrevious");
        QQuickItem *next = w.item("findNext");
        QQuickItem *noMatch = w.item("noMatch");
        QVERIFY(toolbar && title && counts && box && field && previous && next && noMatch);
        for (const QString words : {"Reject", "Accept", "Undo", "Edit Wording…"}) {
            QVERIFY2(!Shown::button(toolbar, words), qPrintable(words));
        }
        QVERIFY(!w.item("editButton"));
        // The two glyphs, and what they find in words.
        const std::vector<std::tuple<QQuickItem *, QString, QString>> glyphs = {
            {previous, "‹", "Previous Match"}, {next, "›", "Next Match"}};
        for (const auto &[button, glyph, name] : glyphs) {
            QCOMPARE(button->property("text").toString(), glyph);
            QAccessibleInterface *said = QAccessible::queryAccessibleInterface(button);
            QVERIFY(said);
            QCOMPARE(said->text(QAccessible::Name), name);
            QCOMPARE(button->property("focusPolicy").value<Qt::FocusPolicy>(), Qt::NoFocus);
        }
        QCOMPARE(field->property("placeholderText").toString(), "Find");
        QAccessibleInterface *fieldSaid = QAccessible::queryAccessibleInterface(field);
        QVERIFY(fieldSaid);
        QCOMPARE(fieldSaid->text(QAccessible::Name), "Find");
        QCOMPARE(field->property("font").value<QFont>().pointSizeF(), w.settings.windowTextSize());
        const QList<QQuickItem *> parts = {box, field, previous, next};
        for (QQuickItem *part : parts) {
            QVERIFY2(toolbar->isAncestorOf(part), qPrintable(part->objectName()));
        }

        // The title and the counts are a column of their own, before the box.
        QQuickItem *column = title->parentItem();
        QVERIFY(column->isAncestorOf(counts));
        const auto starts = [toolbar](QQuickItem *of) {
            return of->mapToItem(toolbar, QPointF()).x();
        };
        const auto ends = [toolbar](QQuickItem *of) {
            return of->mapToItem(toolbar, QPointF(of->width(), 0)).x();
        };
        // What is out of place in the bar, nothing once it is laid out at the window's width: the
        // column ends before the field starts, the field, ‹ and › follow each other, each part is
        // whole inside the bar, and the box is no narrower than its least.
        const auto outOfPlace = [&] {
            QStringList out;
            const QList<QQuickItem *> inTurn = {column, field, previous, next};
            for (qsizetype i = 1; i < inTurn.size(); ++i) {
                if (ends(inTurn[i - 1]) > starts(inTurn[i]) + 0.5) {
                    out << QStringLiteral("%1 ends at %2, %3 starts at %4")
                               .arg(inTurn[i - 1]->objectName())
                               .arg(ends(inTurn[i - 1]))
                               .arg(inTurn[i]->objectName())
                               .arg(starts(inTurn[i]));
                }
            }
            for (QQuickItem *part : parts) {
                const qreal top = part->mapToItem(toolbar, QPointF()).y();
                if (starts(part) < -0.5 || ends(part) > toolbar->width() + 0.5 || top < -0.5 ||
                    top + part->height() > toolbar->height() + 0.5) {
                    out << QStringLiteral("%1 runs from %2 to %3, %4 to %5 down")
                               .arg(part->objectName())
                               .arg(starts(part))
                               .arg(ends(part))
                               .arg(top)
                               .arg(top + part->height());
                }
            }
            for (QQuickItem *button : {previous, next}) {
                if (button->width() < button->implicitWidth() - 0.5) {
                    out << QStringLiteral("%1 is %2 of its %3 wide")
                               .arg(button->objectName())
                               .arg(button->width())
                               .arg(button->implicitWidth());
                }
            }
            const qreal least = box->property("leastWidth").toReal();
            if (least <= 0 || box->width() < least - 0.5) {
                out << QStringLiteral("the box is %1 of its least %2 wide")
                           .arg(box->width())
                           .arg(least);
            }
            return out;
        };
        QStringList out;
        QTRY_VERIFY2((out = outOfPlace()).isEmpty(), qPrintable(out.join("; ")));

        // A title and counts too long for the bar, and the bar laid out for them, as the window
        // lays out what changed before each frame it draws.
        const qreal kept = box->width();
        QSignalSpy laidOut(w.window, &QQuickWindow::afterAnimating);
        title->setProperty("text", QStringLiteral("A very long name for a show ").repeated(10));
        counts->setProperty("text", QStringLiteral("40 to review · 10 accepted · ").repeated(10));
        QVERIFY(laidOut.wait());
        QVERIFY(title->property("truncated").toBool());
        QVERIFY(counts->property("truncated").toBool());
        QVERIFY2(qAbs(box->width() - kept) <= 0.5, qPrintable(QString::number(box->width())));
        out = outOfPlace();
        QVERIFY2(out.isEmpty(), qPrintable(out.join("; ")));

        // Words no row holds: "No match" inside the field's right end, past the room for the words.
        const QSizeF fieldWas = field->size();
        const qreal toolbarWas = toolbar->height();
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(field->hasActiveFocus());
        typeIn(w, "zzz");
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_VERIFY(noMatch->isVisible());
        QVERIFY(laidOut.wait());
        QCOMPARE(noMatch->property("text").toString(), "No match");
        QVERIFY(!noMatch->property("truncated").toBool());
        QVERIFY(field->isAncestorOf(noMatch));
        const QRectF drawn =
            noMatch->mapRectToItem(field, QRectF(0, 0, noMatch->width(), noMatch->height()));
        QVERIFY2(drawn.left() >= -0.5 && drawn.right() <= field->width() + 0.5 &&
                     drawn.top() >= -0.5 && drawn.bottom() <= field->height() + 0.5 &&
                     noMatch->width() >= noMatch->implicitWidth() - 0.5,
                 qPrintable(QStringLiteral("%1, %2 to %3, %4 in a field %5 by %6")
                                .arg(drawn.left())
                                .arg(drawn.top())
                                .arg(drawn.right())
                                .arg(drawn.bottom())
                                .arg(field->width())
                                .arg(field->height())));
        const qreal wordsEnd = field->width() - field->property("rightPadding").toReal();
        QVERIFY2(drawn.left() >= wordsEnd + 6 - 0.5,
                 qPrintable(QStringLiteral("starts at %1, the words end at %2")
                                .arg(drawn.left())
                                .arg(wordsEnd)));
        QCOMPARE(field->size(), fieldWas);
        QVERIFY(qAbs(box->width() - kept) <= 0.5);
        QCOMPARE(toolbar->height(), toolbarWas);
    }

    // The Find field's right inset, with no "No match" in it, is the style's own at a field's
    // right, as a shown field of the style has it, the edit's: the field never shown that the Find
    // field reads it from has it as a shown one does.
    void theFindFieldsRightInsetIsAShownFieldsOfTheStyle() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *find = w.item("findField");
        QQuickItem *plain = w.item("plainField");
        QQuickItem *noMatch = w.item("noMatch");
        QQuickItem *edit = inEdit(w, "editField");
        QVERIFY(find && plain && noMatch && edit);
        QVERIFY(!plain->isVisible());
        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(edit->hasActiveFocus());
        QVERIFY(edit->isVisible());
        QVERIFY(!noMatch->isVisible());
        const qreal shown = edit->property("rightPadding").toReal();
        QCOMPARE(plain->property("rightPadding").toReal(), shown);
        QCOMPARE(find->property("rightPadding").toReal(), shown);
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!w.bridge.editing());
    }

    // The buttons under the video, a click on the fix, whose edit Cancel ends, and a double-click
    // on a row; and the finding's particulars under their disclosure.
    void theButtonsAndADoubleClickAskReview() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *accept = w.item("acceptButton");
        QQuickItem *reject = w.item("rejectButton");
        QQuickItem *undo = w.item("undoButton");
        QQuickItem *technical = w.item("technical");
        QVERIFY(accept && reject && undo && technical);
        w.click(accept);
        QTRY_COMPARE(w.decision(0), "Accepted");
        w.click(undo);
        QTRY_COMPARE(w.decision(0), "To review");
        w.click(reject);
        QTRY_COMPARE(w.decision(0), "Rejected");
        w.click(undo);
        QTRY_COMPARE(w.decision(0), "To review");
        QCOMPARE(w.bridge.sent, (QStringList{"Accept", "Undo", "Reject", "Undo"}));
        QQuickItem *cancel = inEdit(w, "cancelButton");
        QQuickItem *line = inEdit(w, "blockLine");
        QVERIFY(cancel && line);
        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());
        w.click(line);
        QTRY_VERIFY(cancel->isVisible());
        w.click(cancel);
        QCOMPARE(w.bridge.sent.mid(4), (QStringList{"Edit", "CancelEdit"}));
        QTRY_VERIFY(!cancel->isVisible());

        QQuickItem *particulars = w.item("technicalLines");
        QVERIFY(particulars);
        QVERIFY(!technical->property("checked").toBool());
        QVERIFY(!particulars->isVisible());
        w.click(technical);
        QVERIFY(technical->property("checked").toBool());
        QTRY_VERIFY(particulars->isVisible());
        QVERIFY(!w.bridge.detail()->technical().isEmpty());
        QTRY_COMPARE(Shown::lines(particulars), w.bridge.detail()->technical());

        QTRY_VERIFY(w.delegate(1));
        QQuickItem *second = w.delegate(1);
        const qsizetype from = w.bridge.sent.size();
        QTest::mouseDClick(w.window, Qt::LeftButton, {}, w.middle(second));
        const QStringList sent = w.since(from);
        QVERIFY2(sent.size() >= 2 && sent.last() == "Play" &&
                     sent.first().startsWith("Select row "),
                 qPrintable(sent.join(" | ")));
        QTRY_COMPARE(w.bridge.statusLine(), "Opening E01 at 0:00:20.000");
    }

    void aRowOfSeveralOpensAndClosesByItsTriangle_data() {
        QTest::addColumn<QString>("style");
        for (const char *style : {"twoLines", "compact", "rich"}) {
            QTest::newRow(style) << QString::fromLatin1(style);
        }
    }
    // A click on a row of several's triangle selects the row and opens it, and another closes it:
    // the triangle says which, as Finding ▸ Show Each Episode's check does, and a screen reader is
    // told so in those words, and can press it. It comes first in a row of every style, in a column
    // as tall as the row that takes the click anywhere in it; the row's members have none, and sit
    // in from the row's parts. Two quick clicks on it open and close the row, and are no
    // double-click on the row, which would open the video.
    void aRowOfSeveralOpensAndClosesByItsTriangle() {
        QFETCH(QString, style);
        Shown w({media()});
        w.settings.setRowStyle(style);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.delegate(0) && drawnStyle(w.delegate(0)) == style);
        QObject *showEach = w.find("showEachEpisode");
        QVERIFY(showEach);
        // Row 0's triangle, wherever the list has put the row; and where a row's first part after
        // the triangle starts in it.
        const auto triangle = [&w] {
            QQuickItem *row = w.delegate(0);
            return row ? Shown::item(row, "rowDisclosure") : nullptr;
        };
        const char *first = style == "compact" ? "rowGlyph" : style == "rich" ? "rowTile" : "badge";
        const auto starts = [first](QQuickItem *row) {
            QQuickItem *part = Shown::item(row, first);
            return part ? part->mapToItem(row, QPointF(0, 0)).x() : -1.0;
        };
        QVERIFY(triangle() && triangle()->isVisible());
        QCOMPARE(triangle()->property("text").toString(), "▸");
        QCOMPARE(triangle()->mapToItem(w.delegate(0), QPointF(0, 0)), QPointF(0, 0));
        QCOMPARE(triangle()->height(), w.delegate(0)->height());
        // A point in the triangle's column, `x` and `y` from its top left. The column's first
        // 8 px are under the hold of the handle between the sidebar and the list, which takes a
        // press there.
        const auto inColumn = [&triangle](qreal x, qreal y) {
            return triangle()->mapToScene(QPointF(x, y)).toPoint();
        };
        const qreal ends =
            triangle()->mapToItem(w.delegate(0), QPointF(triangle()->width(), 0)).x();
        QVERIFY2(ends <= starts(w.delegate(0)),
                 qPrintable(QStringLiteral("the triangle ends at %1, the row's %2 starts at %3")
                                .arg(ends)
                                .arg(QLatin1String(first))
                                .arg(starts(w.delegate(0)))));
        QVERIFY(!showEach->property("checked").toBool());

        qsizetype from = w.bridge.sent.size();
        QTest::mouseClick(w.window, Qt::LeftButton, {}, inColumn(9, 1));
        QStringList sent = w.since(from);
        QVERIFY2(sent.size() == 2 && sent[0].startsWith("Select row ") && sent[1] == "Toggle",
                 qPrintable(sent.join(" | ")));
        QTRY_COMPARE(w.rows(), 4);
        QTRY_COMPARE(triangle()->property("text").toString(), "▾");
        QTRY_VERIFY(showEach->property("checked").toBool());
        for (const int member : {1, 2}) {
            QTRY_VERIFY(w.delegate(member));
            QQuickItem *drawn = Shown::item(w.delegate(member), "rowDisclosure");
            QVERIFY2(!drawn || !drawn->isVisible(), qPrintable(QString::number(member)));
            QVERIFY2(starts(w.delegate(member)) > starts(w.delegate(0)) + 0.5,
                     qPrintable(QStringLiteral("member %1's %2 starts at %3, the row's at %4")
                                    .arg(member)
                                    .arg(QLatin1String(first))
                                    .arg(starts(w.delegate(member)))
                                    .arg(starts(w.delegate(0)))));
        }

        from = w.bridge.sent.size();
        QTest::mouseClick(w.window, Qt::LeftButton, {},
                          inColumn(triangle()->width() - 1, triangle()->height() - 1));
        sent = w.since(from);
        QVERIFY2(sent.size() == 2 && sent[0].startsWith("Select row ") && sent[1] == "Toggle",
                 qPrintable(sent.join(" | ")));
        QTRY_COMPARE(w.rows(), 2);
        QTRY_COMPARE(triangle()->property("text").toString(), "▸");
        QTRY_VERIFY(!showEach->property("checked").toBool());

        from = w.bridge.sent.size();
        QTest::mouseDClick(w.window, Qt::LeftButton, {}, w.middle(triangle()));
        sent = w.since(from);
        QVERIFY2(sent.size() == 4 && sent[0].startsWith("Select row ") && sent[1] == "Toggle" &&
                     sent[2] == sent[0] && sent[3] == "Toggle",
                 qPrintable(sent.join(" | ")));
        // Review takes the actions in order: once it has answered a move sent after the clicks, it
        // has answered them too.
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(w.bridge.selected(), 1);
        QCOMPARE(w.rows(), 2);
        QCOMPARE(triangle()->property("text").toString(), "▸");
        QVERIFY(!showEach->property("checked").toBool());

        QAccessibleInterface *said = QAccessible::queryAccessibleInterface(triangle());
        QVERIFY(said && said->actionInterface());
        QCOMPARE(said->role(), QAccessible::Button);
        QCOMPARE(said->text(QAccessible::Name), "Show Each Episode");
        QVERIFY(said->state().checkable);
        QVERIFY(!said->state().checked);
        for (const int rows : {4, 2}) {
            from = w.bridge.sent.size();
            said->actionInterface()->doAction(QAccessibleActionInterface::pressAction());
            sent = w.since(from);
            QVERIFY2(sent.size() == 2 && sent[0] == selecting(w, 0) && sent[1] == "Toggle",
                     qPrintable(sent.join(" | ")));
            QTRY_COMPARE(w.rows(), rows);
            QTRY_COMPARE(QAccessible::queryAccessibleInterface(triangle())->state().checked,
                         rows == 4);
        }
    }

    // A row of one keeps the triangle's room, so that its parts start where a row of several's do,
    // but draws no triangle there, and a click there is the row's; with accessibility on, a screen
    // reader is told of the one triangle there is, named for what it does, and of no glyph. Open or
    // closed, the triangle is one width, so that the row's parts stay put as it opens.
    void aRowOfOneKeepsTheTrianglesRoomAndDrawsNone() {
        QAccessible::setActive(true);
        const auto inactive = qScopeGuard([] { QAccessible::setActive(false); });
        QQmlEngine engine;
        QQmlComponent component(&engine);
        component.setData(QByteArrayLiteral(R"(
import QtQuick
import Subcheck

Window {
    width: 200
    height: 100

    Row {
        Disclosure {
            objectName: "several"
        }
        Disclosure {
            objectName: "one"
            shown: false
        }
    }
}
)"),
                          QUrl(QStringLiteral("disclosure.qml")));
        const std::unique_ptr<QObject> made(component.create());
        auto *window = qobject_cast<QQuickWindow *>(made.get());
        QVERIFY2(window, qPrintable(component.errorString()));
        window->show();
        QVERIFY(QTest::qWaitForWindowExposed(window));
        auto *several = window->findChild<QQuickItem *>("several");
        auto *one = window->findChild<QQuickItem *>("one");
        QVERIFY(several && one);
        QSignalSpy severalToggled(several, SIGNAL(toggled()));
        QSignalSpy oneToggled(one, SIGNAL(toggled()));
        QVERIFY(several->width() > 0);
        QCOMPARE(one->width(), several->width());
        QCOMPARE(several->opacity(), 1.0);
        QCOMPARE(one->opacity(), 0.0);
        const qreal closed = several->width();
        several->setProperty("open", true);
        QCOMPARE(several->property("text").toString(), "▾");
        QCOMPARE(several->width(), closed);
        const auto middle = [](QQuickItem *of) {
            return of->mapToScene(QPointF(of->width() / 2, of->height() / 2)).toPoint();
        };
        QTest::mouseClick(window, Qt::LeftButton, {}, middle(several));
        QTest::mouseClick(window, Qt::LeftButton, {}, middle(one));
        QCOMPARE(severalToggled.count(), 1);
        QCOMPARE(oneToggled.count(), 0);

        QCOMPARE(accessibleNames(QAccessible::queryAccessibleInterface(window)),
                 QStringList{"Show Each Episode"});
        QAccessibleInterface *said = QAccessible::queryAccessibleInterface(several);
        QVERIFY(said && said->actionInterface());
        QVERIFY(said->state().checked);
        said->actionInterface()->doAction(QAccessibleActionInterface::pressAction());
        QCOMPARE(severalToggled.count(), 2);
    }

    void aMemberSitsInFromItsRowAtTheSmallestAndLargestSize_data() {
        QTest::addColumn<QString>("style");
        QTest::addColumn<qreal>("points");
        for (const char *style : {"compact", "twoLines", "rich"}) {
            for (const qreal points : {6.0, 24.0}) {
                QTest::addRow("%s at %g pt", style, points) << QString::fromLatin1(style) << points;
            }
        }
    }
    // An open row's members sit in from the row's parts at every size of text, from the least
    // Settings offers to the largest: each member's first part starts right of its row's.
    void aMemberSitsInFromItsRowAtTheSmallestAndLargestSize() {
        QFETCH(QString, style);
        QFETCH(qreal, points);
        Shown w({media()});
        w.settings.setRowStyle(style);
        w.settings.setListTextSize(points);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.find("showEachEpisode")->property("enabled").toBool());
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 4);
        const char *first = style == "compact" ? "rowGlyph" : style == "rich" ? "rowTile" : "badge";
        const auto starts = [&w, first](int at) {
            QQuickItem *row = w.delegate(at);
            QQuickItem *part = row ? Shown::item(row, first) : nullptr;
            return part ? part->mapToItem(row, QPointF(0, 0)).x() : -1.0;
        };
        QTRY_VERIFY(w.delegate(0) && w.delegate(1) && w.delegate(2));
        const char *label = style == "compact" ? "rowGlyph" : "rowPlace";
        QCOMPARE(Shown::item(w.delegate(0), label)->property("font").value<QFont>().pointSizeF(),
                 points);
        for (const int member : {1, 2}) {
            QTRY_VERIFY2(starts(member) > starts(0) + 0.5,
                         qPrintable(QStringLiteral("member %1's %2 starts at %3, the row's at %4")
                                        .arg(member)
                                        .arg(QLatin1String(first))
                                        .arg(starts(member))
                                        .arg(starts(0))));
        }
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 2);
    }

    // Undo, Reject and Accept sit in a row of their own under the video, Undo at its left and the
    // other two at its right, whole at each size of text the window takes, and the details panel
    // and the window are never narrower than the row. The row takes the window's size of text, not
    // the details panel's, takes no focus, and stays where it is as the details scroll, the finding
    // changes and a decision is made and taken back.
    void theDecisionsSitUnderTheVideo_data() {
        QTest::addColumn<qreal>("points");
        // Only for its least and most sizes.
        QTemporaryDir dir;
        WindowSettings sizes(
            std::make_unique<QSettings>(dir.path() + "/subcheck-qt.conf", QSettings::IniFormat),
            QGuiApplication::platformName());
        QTest::newRow("the least size") << sizes.leastHalfPoints() / 2.0;
        QTest::newRow("the platform's size") << qreal(0);
        QTest::newRow("the largest size") << sizes.mostHalfPoints() / 2.0;
    }
    void theDecisionsSitUnderTheVideo() {
        QFETCH(qreal, points);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        if (points > 0) {
            w.settings.setWindowTextSize(points);
        }
        // Another size for the details panel, so that the row is seen to take the window's.
        w.settings.setDetailsTextSize(w.settings.windowTextSize() == 24 ? 6 : 24);
        QQuickItem *panel = w.item("details");
        QQuickItem *decisions = w.item("decisions");
        QQuickItem *toolbar = w.item("toolbar");
        QQuickItem *scroll = w.item("detailsScroll");
        QQuickItem *slot = w.item("videoSlot");
        QVERIFY(panel && decisions && toolbar && scroll && slot);
        // Read each time: the row's width follows its font's, and settles after the text size does.
        const auto least = [decisions] {
            return 160 + 260 + 2 + std::max(340, int(std::ceil(decisions->implicitWidth())));
        };
        // A window is never shrunk to a lowered minimum, so it is asked for the least each time the
        // least has moved.
        QTRY_VERIFY2(
            [&] {
                w.window->resize(w.window->minimumWidth(), w.window->minimumHeight());
                return w.window->minimumWidth() == least() && w.window->width() == least();
            }(),
            qPrintable(QStringLiteral("minimumWidth %1, width %2, least %3")
                           .arg(w.window->minimumWidth())
                           .arg(w.window->width())
                           .arg(least())));
        QTRY_COMPARE(panel->width(), qreal(least() - 160 - 260 - 2));
        const std::vector<std::pair<const char *, QString>> words = {
            {"undoButton", "Undo"}, {"rejectButton", "Reject"}, {"acceptButton", "Accept"}};
        QList<QQuickItem *> buttons;
        for (const auto &[name, text] : words) {
            QQuickItem *button = w.item(name);
            QVERIFY2(button, name);
            QCOMPARE(button->property("text").toString(), text);
            QVERIFY2(panel->isAncestorOf(button), name);
            QVERIFY2(!toolbar->isAncestorOf(button), name);
            QVERIFY2(!scroll->isAncestorOf(button), name);
            QCOMPARE(button->property("focusPolicy").value<Qt::FocusPolicy>(), Qt::NoFocus);
            QCOMPARE(button->property("font").value<QFont>().pointSizeF(),
                     w.settings.windowTextSize());
            QVERIFY2(button->width() >= button->implicitWidth() - 0.5, name);
            const QPointF at = button->mapToItem(panel, QPointF(0, 0));
            QVERIFY2(at.x() >= 0 && at.x() + button->width() <= panel->width() + 0.5, name);
            buttons << button;
        }
        QVERIFY(buttons[2]->property("highlighted").toBool());
        const auto left = [panel](QQuickItem *of) { return of->mapToItem(panel, QPointF()).x(); };
        QVERIFY(left(buttons[0]) < left(buttons[1]));
        QVERIFY(left(buttons[1]) < left(buttons[2]));
        QVERIFY(qAbs(left(buttons[0]) - 8) < 0.5);
        QVERIFY(qAbs(left(buttons[2]) + buttons[2]->width() - (panel->width() - 8)) < 0.5);
        const qreal videoBottom = slot->mapToItem(panel, QPointF(0, slot->height())).y();
        const qreal top = decisions->mapToItem(panel, QPointF()).y();
        QVERIFY(top >= videoBottom - 0.5);
        QVERIFY(top + decisions->height() <= scroll->mapToItem(panel, QPointF()).y() + 0.5);

        // Where the row is in the window, kept and compared after what would move it.
        const auto sceneY = [decisions] { return decisions->mapToScene(QPointF()).y(); };
        const qreal kept = sceneY();
        QQuickItem *view = detailsView(w);
        QTRY_VERIFY(view->property("contentHeight").toReal() > view->height());
        view->setProperty("contentY", view->property("contentHeight").toReal() - view->height());
        QCOMPARE(sceneY(), kept);
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(w.bridge.selected(), 1);
        QCOMPARE(sceneY(), kept);
        QTest::keyClick(w.window, Qt::Key_A);
        QTRY_COMPARE(w.bridge.detail()->stateWord(), "Accepted");
        QCOMPARE(sceneY(), kept);
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.bridge.detail()->stateWord(), "To review");
        QCOMPARE(sceneY(), kept);
    }

    // Accept and Reject act on the finding shown, so with none to act on they are greyed, never
    // hidden, so that the row under the video keeps its place: while a greyed episode's log shows
    // in the finding's place, while Settings holds the keys and while the filter shows no finding.
    // Undo is neither hidden nor greyed in any of them. A click on one then asks review nothing.
    // The Finding menu's Accept, Reject and Back to Undecided, and with them their keys, follow the
    // same rule.
    void acceptAndRejectGreyWithNoFindingToActOn() {
        Shown w({clean(), media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("episode0") && w.item("filter2") && w.delegate(0));
        QQuickItem *accept = w.item("acceptButton");
        QQuickItem *reject = w.item("rejectButton");
        QQuickItem *undo = w.item("undoButton");
        QQuickItem *row = w.item("decisions");
        QVERIFY(accept && reject && undo && row);
        // Whether each is enabled and whether it shows, Accept's, Reject's, then Undo's, which
        // never waits for a finding, and the row's, which shows.
        const auto state = [accept, reject, undo, row] {
            return QList<bool>{accept->property("enabled").toBool(),
                               accept->isVisible(),
                               reject->property("enabled").toBool(),
                               reject->isVisible(),
                               undo->property("enabled").toBool(),
                               undo->isVisible(),
                               row->isVisible()};
        };
        const QList<bool> enabled{true, true, true, true, true, true, true};
        const QList<bool> greyed{false, true, false, true, true, true, true};
        // Whether the Finding menu's Accept, Reject and Back to Undecided are each enabled.
        const auto items = [&w] {
            QList<bool> each;
            for (const char *name : {"accept", "reject", "clear"}) {
                each << w.find(name)->property("enabled").toBool();
            }
            return each;
        };
        const QList<bool> itemsOn{true, true, true};
        const QList<bool> itemsOff{false, false, false};
        QTRY_VERIFY(w.bridge.detail()->shown());
        QTRY_COMPARE(state(), enabled);
        QCOMPARE(items(), itemsOn);

        w.click(w.item("episode0"));
        QTRY_VERIFY(w.bridge.showingEpisode());
        QTRY_COMPARE(state(), greyed);
        QCOMPARE(items(), itemsOff);
        qsizetype from = w.bridge.sent.size();
        w.click(accept);
        w.click(reject);
        QCOMPARE(w.since(from), QStringList());
        w.click(w.delegate(w.bridge.selected()));
        QTRY_VERIFY(!w.bridge.showingEpisode());
        QTRY_COMPARE(state(), enabled);
        QCOMPARE(items(), itemsOn);

        QQuickWindow *settings = w.settingsWindow();
        QVERIFY(settings);
        QTest::keyClick(w.window, Qt::Key_Comma, Qt::ControlModifier);
        QTRY_VERIFY(settings->isVisible());
        QCOMPARE(state(), greyed);
        QCOMPARE(items(), itemsOff);
        QTest::keyClick(settings, Qt::Key_Escape);
        QTRY_VERIFY(!settings->isVisible());
        QCOMPARE(state(), enabled);
        QCOMPARE(items(), itemsOn);

        w.click(w.item("filter2"));
        QTRY_COMPARE(w.rows(), 0);
        QTRY_VERIFY(!w.bridge.detail()->shown());
        QTRY_COMPARE(state(), greyed);
        QCOMPARE(items(), itemsOff);
        from = w.bridge.sent.size();
        w.click(accept);
        w.click(reject);
        for (const Qt::Key key : {Qt::Key_A, Qt::Key_R, Qt::Key_X}) {
            QTest::keyClick(w.window, key);
        }
        QCOMPARE(w.since(from), QStringList());
    }

    // E opens the edit: the fix's line gives way to a field in its font holding review's wording,
    // the cursor at its end, and the field has the keys. What is typed asks review nothing until
    // Return saves it, which accepts the finding and closes the field. The keypad's Enter saves as
    // Return does. Both decisions are undone, so that the run is left as found.
    void editWordingOpensTheFieldWithReviewsWordingAndSaves() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *field = inEdit(w, "editField");
        QQuickItem *line = inEdit(w, "blockLine");
        QVERIFY(field && line);
        QVERIFY(!field->isVisible());
        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());

        qsizetype from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_E);
        QCOMPARE(w.since(from), QStringList{"Edit"});
        QTRY_VERIFY(field->hasActiveFocus());
        QVERIFY(field->isVisible());
        QVERIFY(!line->isVisible());
        const QString wording = w.bridge.editWording();
        QVERIFY(!wording.isEmpty());
        QCOMPARE(field->property("text").toString(), wording);
        QCOMPARE(field->property("cursorPosition").toInt(), wording.size());
        QCOMPARE(field->property("font").value<QFont>(), line->property("font").value<QFont>());

        typeIn(w, "!");
        QCOMPARE(field->property("text").toString(), wording + "!");
        QCOMPARE(w.since(from), QStringList{"Edit"});
        QTest::keyClick(w.window, Qt::Key_Return);
        QCOMPARE(w.since(from), (QStringList{"Edit", "Reword " + wording + "!"}));
        QTRY_COMPARE(w.decision(0), "Accepted");
        QTRY_VERIFY(!field->isVisible());
        QVERIFY(line->isVisible());

        // The accept moved on, to the row the second edit is of.
        QTRY_COMPARE(w.bridge.selected(), 1);
        from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        const QString second = w.bridge.editWording();
        QCOMPARE(field->property("text").toString(), second);
        QTest::keyClick(w.window, Qt::Key_Enter, Qt::KeypadModifier);
        QCOMPARE(w.since(from), (QStringList{"Edit", "Reword " + second}));
        QTRY_COMPARE(w.decision(1), "Accepted");
        QTRY_VERIFY(!field->isVisible());

        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.decision(1), "To review");
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.decision(0), "To review");
    }

    // Esc in the field, or Cancel, ends the edit, as review is asked to. Save and Accept with no
    // wording is refused by review, on the status bar, and the edit goes on, the field with the
    // keys.
    void escOrCancelEndsTheEditAndAnEmptyWordingIsRefused() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *field = inEdit(w, "editField");
        QQuickItem *save = inEdit(w, "saveButton");
        QQuickItem *cancel = inEdit(w, "cancelButton");
        QVERIFY(field && save && cancel);
        QCOMPARE(save->property("text").toString(), "Save and Accept");
        QVERIFY(save->property("highlighted").toBool());
        QCOMPARE(cancel->property("text").toString(), "Cancel");
        for (QQuickItem *button : {save, cancel}) {
            QCOMPARE(button->property("focusPolicy").value<Qt::FocusPolicy>(), Qt::NoFocus);
        }
        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());

        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.bridge.sent, (QStringList{"Edit", "CancelEdit"}));
        QTRY_VERIFY(!field->isVisible());

        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());
        QMetaObject::invokeMethod(w.find("edit"), "trigger");
        QTRY_VERIFY(field->hasActiveFocus());
        QVERIFY(cancel->isVisible());
        w.click(cancel);
        QCOMPARE(w.bridge.sent.mid(2), (QStringList{"Edit", "CancelEdit"}));
        QTRY_VERIFY(!field->isVisible());

        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        field->setProperty("text", QString());
        const qsizetype from = w.bridge.sent.size();
        w.click(save);
        QCOMPARE(w.since(from), QStringList{"Reword "});
        QTRY_COMPARE(w.bridge.statusLine(), "The wording cannot be empty");
        QVERIFY(w.bridge.editing());
        QVERIFY(field->isVisible());
        QVERIFY(field->hasActiveFocus());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!field->isVisible());
        QCOMPARE(w.decision(0), "To review");
    }

    // While the field has the keys, those it types, moves through or has no use for in one line are
    // its own: none reaches the window's shortcuts, the list's moves, the video's and the View
    // menu's letter among them, nor any shortcut the window gains.
    void theEditFieldTakesItsKeys() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *field = inEdit(w, "editField");
        QVERIFY(field);
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        const QString wording = field->property("text").toString();
        const qsizetype from = w.bridge.sent.size();
        for (const Qt::Key key :
             {Qt::Key_Up, Qt::Key_Down, Qt::Key_PageUp, Qt::Key_PageDown, Qt::Key_Home, Qt::Key_End,
              Qt::Key_Space, Qt::Key_A, Qt::Key_Left, Qt::Key_Right, Qt::Key_V}) {
            const QString name = QKeySequence(key).toString();
            QKeyEvent override(QEvent::ShortcutOverride, key, Qt::NoModifier);
            override.ignore();
            QCoreApplication::sendEvent(field, &override);
            QVERIFY2(override.isAccepted(), qPrintable(name));
            QTest::keyClick(w.window, key);
            QVERIFY2(w.since(from).isEmpty(), qPrintable(name + ": " + w.since(from).join(" | ")));
            QVERIFY2(field->hasActiveFocus(), qPrintable(name));
        }
        QCOMPARE(field->property("text").toString(), wording + " av");
        QTest::qWait(100);
        QCOMPARE(w.since(from), QStringList());
        QVERIFY(w.bridge.editing());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), QStringList{"CancelEdit"});
        QTRY_VERIFY(!w.bridge.editing());
    }

    // Ctrl+Z in the field undoes what was typed there, back to the wording it opened with, and asks
    // review nothing: the decision before the edit stays. It is undone, so that the run is left as
    // found.
    void ctrlZInTheFieldUndoesTheTypingNotADecision() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *field = inEdit(w, "editField");
        QVERIFY(field);
        QTest::keyClick(w.window, Qt::Key_A);
        QTRY_COMPARE(w.decision(0), "Accepted");
        QTRY_COMPARE(w.bridge.selected(), 1);
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        const QString opened = field->property("text").toString();
        typeIn(w, "zz");
        QCOMPARE(field->property("text").toString(), opened + "zz");
        const qsizetype from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_Z, Qt::ControlModifier);
        QCOMPARE(field->property("text").toString(), opened);
        QTest::qWait(100);
        QCOMPARE(w.since(from), QStringList());
        QCOMPARE(w.decision(0), "Accepted");
        QVERIFY(w.bridge.editing());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!w.bridge.editing());
        QTest::keyClick(w.window, Qt::Key_U);
        QTRY_COMPARE(w.decision(0), "To review");
    }

    // What is typed stays, and the field keeps the keys, whatever review or the window changes
    // while it is open: review's message, a palette chosen, and the same edit with its entry
    // renamed, as a re-check renames it.
    void aChangeWhileTypingKeepsWhatWasTyped() {
        const auto restore = qScopeGuard([] { useColours(Colours::Default); });
        Shown w({media()});
        std::optional<subcheck_ffi::Event::kChanged> last;
        QObject::connect(&w.bridge, &SessionBridge::eventArrived, &w.bridge,
                         [&last](const subcheck_ffi::Event &e) {
                             const auto *change =
                                 std::get_if<subcheck_ffi::Event::kChanged>(&e.get_variant());
                             if (change) {
                                 last = *change;
                             }
                         });
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *field = inEdit(w, "editField");
        QQuickItem *bar = inEdit(w, "blockBar");
        QVERIFY(field && bar);
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        typeIn(w, "abc");
        const QString typed = field->property("text").toString();
        QVERIFY(typed.endsWith("abc"));
        // Whether the field still holds what was typed, and the keys.
        const auto kept = [field, &typed] {
            return field->isVisible() && field->hasActiveFocus() &&
                   field->property("text").toString() == typed;
        };

        w.bridge.undo();
        QTRY_COMPARE(w.bridge.statusLine(), "Nothing to undo");
        QVERIFY(kept());

        w.settings.setColours(QStringLiteral("deuteran"));
        QTRY_COMPARE(bar->property("color").value<QColor>(),
                     tones(Colours::Deuteran, dark()).added);
        QVERIFY(kept());

        QVERIFY(last && last->summary.edit);
        subcheck_ffi::Summary renamed = last->summary;
        renamed.edit->entry = subcheck_ffi::EntryKey(subcheck_ffi::EntryKey::kRow{{9001, 9002}});
        QSignalSpy changed(&w.bridge, &SessionBridge::summaryChanged);
        emit w.bridge.eventArrived(subcheck_ffi::Event(subcheck_ffi::Event::kChanged{
            std::nullopt, renamed, std::nullopt, std::nullopt, std::nullopt, std::nullopt}));
        QTRY_VERIFY(changed.count() > 0);
        QVERIFY(w.bridge.editing());
        QVERIFY(kept());

        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!w.bridge.editing());
    }

    // A text field's own right-click menu, the edit's or the Find box's, has the keys while it is
    // open, as a menu of the window's has them, whichever window a key reaches, the menu's own or
    // the window: the window's letters ask review nothing. Esc closes it, and the field has the
    // keys again, the Find field with the "No match" it showed, as it took no keys by a click or
    // Ctrl+F; Esc again ends the edit or gives the window its keys back from the box, which stays,
    // and the window's keys are its own again.
    void aTextFieldsOwnMenuHasTheKeys() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *field = inEdit(w, "editField");
        QVERIFY(field);
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        const QString opened = field->property("text").toString();
        // Whether the menu of the field `of` is open.
        const auto menuOpen = [](QQuickItem *of) {
            const QList<QObject *> parts = of->findChildren<QObject *>();
            return std::any_of(parts.begin(), parts.end(), [](QObject *part) {
                return part->inherits("QQuickMenu") && part->property("opened").toBool();
            });
        };
        QTest::mouseClick(w.window, Qt::RightButton, {}, w.middle(field));
        QTRY_VERIFY(menuOpen(field));
        QWindow *menu = QGuiApplication::focusWindow();
        QVERIFY(menu);
        const qsizetype from = w.bridge.sent.size();
        for (const Qt::Key key : {Qt::Key_A, Qt::Key_X, Qt::Key_N}) {
            for (QWindow *to : {menu, static_cast<QWindow *>(w.window)}) {
                QTest::keyClick(to, key);
            }
        }
        QTest::qWait(100);
        QCOMPARE(w.since(from), QStringList());
        QVERIFY(menuOpen(field));
        QVERIFY(w.bridge.editing());
        QTest::keyClick(menu, Qt::Key_Escape);
        QTRY_VERIFY(!menuOpen(field));
        // The window manager gives the keys back to the window the menu was over, which offscreen
        // has none to do.
        w.window->requestActivate();
        QVERIFY(QTest::qWaitForWindowActive(w.window));
        QTRY_VERIFY(field->hasActiveFocus());
        QCOMPARE(field->property("text").toString(), opened);
        QTest::keyClick(w.window, Qt::Key_End);
        QTest::keyClick(w.window, Qt::Key_X);
        QCOMPARE(field->property("text").toString(), opened + "x");
        QCOMPARE(w.since(from), QStringList());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), QStringList{"CancelEdit"});
        QTRY_VERIFY(!w.bridge.editing());
        QTRY_VERIFY(w.find("clear")->property("enabled").toBool());
        QTest::keyClick(w.window, Qt::Key_X);
        QCOMPARE(w.last(), "Clear");

        QQuickItem *findBox = w.item("findBox");
        QQuickItem *findField = w.item("findField");
        QQuickItem *noMatch = w.item("noMatch");
        QVERIFY(findBox && findField && noMatch);
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(findField->hasActiveFocus());
        typeIn(w, "zzz");
        QTest::keyClick(w.window, Qt::Key_Return);
        QVERIFY(noMatch->isVisible());
        QTest::mouseClick(w.window, Qt::RightButton, {}, w.middle(findField));
        QTRY_VERIFY(menuOpen(findField));
        QWindow *findMenu = QGuiApplication::focusWindow();
        QVERIFY(findMenu);
        const qsizetype found = w.bridge.sent.size();
        for (const Qt::Key key : {Qt::Key_A, Qt::Key_X, Qt::Key_N}) {
            for (QWindow *to : {findMenu, static_cast<QWindow *>(w.window)}) {
                QTest::keyClick(to, key);
            }
        }
        QTest::qWait(100);
        QCOMPARE(w.since(found), QStringList());
        QVERIFY(menuOpen(findField));
        QTest::keyClick(findMenu, Qt::Key_Escape);
        QTRY_VERIFY(!menuOpen(findField));
        w.window->requestActivate();
        QVERIFY(QTest::qWaitForWindowActive(w.window));
        QTRY_VERIFY(findField->hasActiveFocus());
        QVERIFY(noMatch->isVisible());
        QCOMPARE(findField->property("text").toString(), "zzz");
        QTest::keyClick(w.window, Qt::Key_Escape);
        QVERIFY(!findField->hasActiveFocus());
        QVERIFY(findBox->isVisible());
        QCOMPARE(w.since(found), QStringList());
        QTest::keyClick(w.window, Qt::Key_X);
        QCOMPARE(w.since(found), QStringList{"Clear"});
    }

    // A click on another row selects it, and review ends the edit, the words typed with it: the
    // field closes, and the row it was of is undecided still.
    void movingOffAnEditEndsIt() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.delegate(1));
        QQuickItem *field = inEdit(w, "editField");
        QVERIFY(field);
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        typeIn(w, "abc");
        const qsizetype from = w.bridge.sent.size();
        w.click(w.delegate(1));
        const QStringList sent = w.since(from);
        QVERIFY2(sent.size() == 1 && sent[0].startsWith("Select row "),
                 qPrintable(sent.join(" | ")));
        QTRY_COMPARE(w.bridge.selected(), 1);
        QTRY_VERIFY(!field->isVisible());
        QVERIFY(!w.bridge.editing());
        QCOMPARE(w.decision(0), "To review");
        QCOMPARE(w.since(from), sent);
    }

    // A greyed episode clicked while an edit is open moves no selection, so the edit stays open
    // under the episode's view, but its field, hidden there, gives the window its keys: Return and
    // the keypad's Enter send nothing, as Show Each Episode waits in that view, and Space plays and
    // pauses. A click on the grey row ends the view, and the field has its words and the keys
    // again.
    void aGreyedEpisodesViewHasTheKeysOfAnOpenEditUntilItEnds() {
        Shown w({clean(), media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("episode0") && w.delegate(0));
        QQuickItem *field = inEdit(w, "editField");
        QVERIFY(field);
        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        typeIn(w, "abc");
        const QString typed = field->property("text").toString();
        QVERIFY(typed.endsWith("abc"));

        qsizetype from = w.bridge.sent.size();
        w.click(w.item("episode0"));
        QVERIFY2(w.since(from).size() == 1 && w.since(from).first().startsWith("SelectFile "),
                 qPrintable(w.since(from).join(" | ")));
        QTRY_VERIFY(w.bridge.showingEpisode());
        QTRY_VERIFY(!field->isVisible());
        QVERIFY(w.bridge.editing());
        QVERIFY(!field->hasActiveFocus());
        from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_Return);
        QTest::keyClick(w.window, Qt::Key_Enter, Qt::KeypadModifier);
        QCOMPARE(w.since(from), QStringList());
        QTest::keyClick(w.window, Qt::Key_Space);
        QCOMPARE(w.since(from), QStringList{"Playback Pause"});
        QCOMPARE(field->property("text").toString(), typed);
        QVERIFY(w.bridge.editing());

        from = w.bridge.sent.size();
        w.click(w.delegate(w.bridge.selected()));
        QVERIFY2(w.since(from).size() == 1 && w.since(from).first().startsWith("Select row "),
                 qPrintable(w.since(from).join(" | ")));
        QTRY_VERIFY(!w.bridge.showingEpisode());
        QVERIFY(w.bridge.editing());
        QTRY_VERIFY(field->isVisible() && field->hasActiveFocus());
        QCOMPARE(field->property("text").toString(), typed);
        QCOMPARE(detailsView(w)->property("contentY").toReal(), 0.0);
        QVERIFY(inDetailsView(w, field));
        typeIn(w, "d");
        QCOMPARE(field->property("text").toString(), typed + "d");
        QCOMPARE(w.since(from).size(), 1);
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.last(), "CancelEdit");
        QTRY_VERIFY(!w.bridge.editing());
    }

    // An edit opened with the details panel scrolled down, to the log under the finding, brings
    // its field into view, so that what is typed is seen.
    void anEditOpenedScrolledPastItsFieldBringsTheFieldIntoView() {
        Shown w({clean(), media()});
        clickE01AtTheLeastSize(w);
        QQuickItem *view = detailsView(w);
        QQuickItem *field = inEdit(w, "editField");
        QQuickItem *fixed = w.item("fixedBlock");
        QVERIFY(field && fixed);
        const qreal down = view->property("contentHeight").toReal() - view->height();
        QVERIFY(down > 0);
        view->setProperty("contentY", down);
        QCOMPARE(view->property("contentY").toReal(), down);
        QVERIFY(topInDetails(w, fixed) + fixed->height() <= 0);
        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        QTRY_VERIFY2(inDetailsView(w, field),
                     qPrintable(QStringLiteral("the field starts at %1 in a view %2 tall")
                                    .arg(topInDetails(w, field))
                                    .arg(view->height())));
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!w.bridge.editing());
    }

    // While an edit is open, the decisions and Undo wait, by menu, key and button alike: a click
    // beside the field would decide the plain fix, or undo another, and drop the typed wording.
    // Save and Accept and Cancel decide the edit. Once it ends they are back.
    void theDecisionsWaitWhileAnEditIsOpen() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *field = inEdit(w, "editField");
        QQuickItem *cancel = inEdit(w, "cancelButton");
        QVERIFY(field && cancel);
        const QStringList actions{"accept", "reject", "clear", "edit", "undo"};
        const QStringList buttons{"rejectButton", "acceptButton", "undoButton"};
        // Which of them are enabled, by name.
        const auto enabled = [&] {
            QStringList on;
            for (const QString &name : actions) {
                if (w.find(qPrintable(name))->property("enabled").toBool()) {
                    on << name;
                }
            }
            for (const QString &name : buttons) {
                QQuickItem *button = w.item(qPrintable(name));
                if (!button->isVisible()) {
                    on << name + " hidden";
                }
                if (button->property("enabled").toBool()) {
                    on << name;
                }
            }
            return on;
        };
        QTRY_COMPARE(enabled(), actions + buttons);
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        QCOMPARE(enabled(), QStringList());
        const qsizetype from = w.bridge.sent.size();
        for (const QString &name : buttons) {
            w.click(w.item(qPrintable(name)));
        }
        QCOMPARE(w.since(from), QStringList());
        QVERIFY(field->hasActiveFocus());
        w.click(cancel);
        QCOMPARE(w.since(from), QStringList{"CancelEdit"});
        QTRY_COMPARE(enabled(), actions + buttons);
    }

    // A timing fix is accepted or rejected, never reworded: with one selected, Edit Wording… is
    // greyed in the menu, and a click on the fix asks for no edit.
    void aTimingRowCannotBeReworded() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *edit = w.find("edit");
        QQuickItem *area = inEdit(w, "rewordArea");
        QVERIFY(edit && area);
        QTRY_VERIFY(edit->property("enabled").toBool());
        QVERIFY(area->isVisible());
        QVERIFY(timingRowAtTheSelection(w));
        QVERIFY(!w.bridge.canEdit());
        QTRY_VERIFY(!edit->property("enabled").toBool());
        QVERIFY(!area->isVisible());
        QVERIFY(w.find("accept")->property("enabled").toBool());
    }

    // A click on the line with the fix opens its edit, the field's cursor where the click was: on
    // the line's first drawn line, and on its second where it wraps, at a large size in a narrow
    // panel. The pointer shows a text cursor over it. Open, the field takes its own clicks; E
    // opens the edit with the cursor at its end, as a click's place is not kept. A click while the
    // Find box has the keys gives them to the edit's field and greys the box, so that Return saves
    // and never finds.
    void aClickOnTheFixOpensTheEditWithTheCursorThere() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *area = inEdit(w, "rewordArea");
        QQuickItem *line = inEdit(w, "blockLine");
        QQuickItem *field = inEdit(w, "editField");
        QQuickItem *findBox = w.item("findBox");
        QQuickItem *findField = w.item("findField");
        QVERIFY(area && line && field && findBox && findField);
        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());
        const QString wording = QStringLiteral("We should leave before it rains.");
        QCOMPARE(text(w.bridge.findings()->at(0)->wording), wording);
        QVERIFY(area->isVisible());
        const QList<QObject *> handlers = area->children();
        const auto hover = std::find_if(handlers.begin(), handlers.end(), [](QObject *each) {
            return each->inherits("QQuickHoverHandler");
        });
        QVERIFY(hover != handlers.end());
        QCOMPARE((*hover)->property("cursorShape").value<Qt::CursorShape>(), Qt::IBeamCursor);

        // A point `across` and `down` the line, in the window.
        const auto onLine = [line](qreal across, qreal down) {
            return line->mapToScene(QPointF(across, down)).toPoint();
        };
        const qreal left = line->property("leftPadding").toReal();
        const qreal top = line->property("topPadding").toReal();
        // A quarter into the first character of a word before the change, which the plain font's
        // metrics place.
        const QFontMetricsF m(line->property("font").value<QFont>());
        const qsizetype s = wording.indexOf("should");
        qsizetype from = w.bridge.sent.size();
        QTest::mouseClick(w.window, Qt::LeftButton, {},
                          onLine(left + m.horizontalAdvance(wording.left(s)) +
                                     m.horizontalAdvance(wording.at(s)) / 4,
                                 top + m.height() / 2));
        QCOMPARE(w.since(from), QStringList{"Edit"});
        QTRY_VERIFY(field->hasActiveFocus());
        QCOMPARE(field->property("text").toString(), w.bridge.editWording());
        QCOMPARE(field->property("cursorPosition").toInt(), s);

        QVERIFY(!area->isVisible());
        QTest::mouseClick(w.window, Qt::LeftButton, {},
                          field->mapToScene(QPointF(1, field->height() / 2)).toPoint());
        QCOMPARE(field->property("cursorPosition").toInt(), 0);
        QCOMPARE(w.since(from), QStringList{"Edit"});
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit"}));
        QTRY_VERIFY(!field->isVisible());

        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        QCOMPARE(field->property("cursorPosition").toInt(), w.bridge.editWording().size());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!field->isVisible());

        w.settings.setDetailsTextSize(24);
        w.window->resize(w.window->minimumWidth(), w.window->height());
        QTRY_COMPARE(w.window->width(), w.window->minimumWidth());
        QTRY_COMPARE(fixNotLaidOut(w, w.window->property("detailsLeast").toReal()), QString());
        const QFontMetricsF large(line->property("font").value<QFont>());
        QTRY_VERIFY2(line->height() >= 2 * large.lineSpacing(),
                     qPrintable(QStringLiteral("the line is %1 tall, a line %2")
                                    .arg(line->height())
                                    .arg(large.lineSpacing())));
        QTRY_VERIFY(area->isVisible());
        QTRY_VERIFY(inDetailsView(w, line));
        from = w.bridge.sent.size();
        QTest::mouseClick(w.window, Qt::LeftButton, {},
                          onLine(left + 1, top + 1.5 * large.lineSpacing()));
        QCOMPARE(w.since(from), QStringList{"Edit"});
        QTRY_VERIFY(field->hasActiveFocus());
        const int p = field->property("cursorPosition").toInt();
        QVERIFY2(p > 0 && wording.at(p - 1) == ' ',
                 qPrintable(QStringLiteral("the cursor is at %1 of \"%2\"").arg(p).arg(wording)));
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!field->isVisible());

        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(findField->hasActiveFocus());
        typeIn(w, "x");
        QCOMPARE(findField->property("text").toString(), "x");
        QTRY_VERIFY(area->isVisible());
        from = w.bridge.sent.size();
        w.click(line);
        QCOMPARE(w.since(from), QStringList{"Edit"});
        QTRY_VERIFY(field->hasActiveFocus());
        QVERIFY(!findField->hasActiveFocus());
        QVERIFY(!findBox->isEnabled());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit"}));
        QTRY_VERIFY(!w.bridge.editing());
    }

    // A click anywhere on the block with the fix opens its edit, on its bar, its label and the room
    // around its line as on the line, with the field's cursor at the line's character nearest the
    // click: on the label, the first drawn line's at the click's x; below the line, the last
    // drawn line's; beside a drawn line, its start or end. The pointer shows a text cursor over
    // all of it.
    void aClickAnywhereOnTheFixsBlockOpensTheEdit_data() {
        QTest::addColumn<QString>("where");
        for (const char *where :
             {"the label", "the padding below the line", "right of a short line", "the bar",
              "the label over a wrapped line", "below a wrapped line"}) {
            QTest::newRow(where) << QString::fromLatin1(where);
        }
    }
    void aClickAnywhereOnTheFixsBlockOpensTheEdit() {
        QFETCH(QString, where);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *fixed = w.item("fixedBlock");
        QQuickItem *area = inEdit(w, "rewordArea");
        QQuickItem *label = inEdit(w, "blockLabel");
        QQuickItem *line = inEdit(w, "blockLine");
        QQuickItem *field = inEdit(w, "editField");
        QVERIFY(fixed && area && label && line && field);
        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());
        const QString wording = QStringLiteral("We should leave before it rains.");
        QCOMPARE(text(w.bridge.findings()->at(0)->wording), wording);
        if (where.contains(QStringLiteral("wrapped"))) {
            w.settings.setDetailsTextSize(24);
            w.window->resize(w.window->minimumWidth(), w.window->height());
            QTRY_COMPARE(w.window->width(), w.window->minimumWidth());
            QTRY_COMPARE(fixNotLaidOut(w, w.window->property("detailsLeast").toReal()), QString());
            const qreal spacing =
                QFontMetricsF(line->property("font").value<QFont>()).lineSpacing();
            QTRY_VERIFY(line->property("contentHeight").toReal() >= 1.5 * spacing);
        } else {
            askRoomForOneLine(w, line);
            if (QTest::currentTestFailed()) {
                return;
            }
            QTRY_COMPARE(fixNotLaidOut(w, roomForOneLine), QString());
        }
        QTRY_VERIFY(area->isVisible());
        QTRY_VERIFY(inDetailsView(w, fixed));
        QCOMPARE(area->mapRectToScene(area->boundingRect()),
                 fixed->mapRectToScene(fixed->boundingRect()));

        // Places in the block: across, a quarter into the first character of a word before the
        // change, as the plain font's metrics place it in the line, and the line's start; down,
        // the middle of the label, of the line's first drawn line, and the block's foot.
        const QFontMetricsF m(line->property("font").value<QFont>());
        const qsizetype s = wording.indexOf("should");
        const qreal left = line->property("leftPadding").toReal();
        const qreal top = line->property("topPadding").toReal();
        const QPointF onLine =
            line->mapToItem(fixed, QPointF(left + m.horizontalAdvance(wording.left(s)) +
                                               m.horizontalAdvance(wording.at(s)) / 4,
                                           top + m.height() / 2));
        const qreal lineStart = line->mapToItem(fixed, QPointF(left + 1, 0)).x();
        const qreal onLabel = label->mapToItem(fixed, QPointF(0, label->height() / 2)).y();
        const qreal foot = fixed->height() - 2;
        QVERIFY(line->mapToItem(fixed, QPointF(0, line->height())).y() < foot);
        QPointF in;
        if (where.startsWith(QStringLiteral("the label"))) {
            in = {onLine.x(), onLabel};
        } else if (where == "the padding below the line") {
            in = {onLine.x(), foot};
        } else if (where == "right of a short line") {
            // In the padding, short of the panel's scroll bar, which lies over the block's edge.
            in = {line->mapToItem(fixed, QPointF(line->width(), 0)).x() + 2, onLine.y()};
        } else if (where == "the bar") {
            // Inside the bar and clear of the column handle's grip, which the check below proves.
            in = {2, onLine.y()};
        } else {
            in = {lineStart, foot};
        }
        const QPoint at = fixed->mapToScene(in).toPoint();
        if (where == "the bar") {
            // The handle nearest the details panel: the one furthest right of those left of the
            // block.
            QQuickItem *grip = nullptr;
            qreal gripX = -1;
            for (QQuickItem *part : w.item("columns")->childItems()) {
                const QList<QQuickItem *> inside = part->childItems();
                const qreal x = part->mapToScene(QPointF()).x();
                if (part->objectName() == QLatin1String("columnHandle") && !inside.isEmpty() &&
                    x < fixed->mapToScene(QPointF()).x() && x > gripX) {
                    grip = inside.first();
                    gripX = x;
                }
            }
            QVERIFY(grip);
            QVERIFY2(!grip->contains(grip->mapFromScene(QPointF(at))),
                     "the click is in the column handle's grip");
        }

        // Off the block first, so that the text cursor is the block's own.
        QTest::mouseMove(w.window, w.middle(w.item("nowBlock")));
        QTRY_VERIFY(w.window->cursor().shape() != Qt::IBeamCursor);
        QTest::mouseMove(w.window, at);
        QTRY_COMPARE(w.window->cursor().shape(), Qt::IBeamCursor);

        const qsizetype from = w.bridge.sent.size();
        QTest::mouseClick(w.window, Qt::LeftButton, {}, at);
        QCOMPARE(w.since(from), QStringList{"Edit"});
        QTRY_VERIFY(field->hasActiveFocus());
        const int p = field->property("cursorPosition").toInt();
        if (where == "right of a short line") {
            QCOMPARE(p, field->property("text").toString().size());
        } else if (where == "the bar") {
            QCOMPARE(p, 0);
        } else if (where == "below a wrapped line") {
            // The start of a drawn line after the first.
            QVERIFY2(
                p > 0 && wording.at(p - 1) == ' ',
                qPrintable(QStringLiteral("the cursor is at %1 of \"%2\"").arg(p).arg(wording)));
        } else {
            QCOMPARE(p, s);
        }
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit"}));
        QTRY_VERIFY(!w.bridge.editing());
    }

    // Where a click on the fix put the cursor goes with the selection: should review move to
    // another finding before it opens the edit the click asked for, that finding's edit starts
    // with the cursor at its end.
    void aClicksPlaceNeverReachesAnotherFindingsEdit() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *details = w.item("details");
        QQuickItem *field = inEdit(w, "editField");
        QVERIFY(details && field);
        // As a click leaves it while its edit waits for review.
        QVERIFY(details->setProperty("cursorAt", 3));
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(w.bridge.selected(), 1);
        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        QCOMPARE(field->property("cursorPosition").toInt(), w.bridge.editWording().size());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!w.bridge.editing());
    }

    // An edit asked other than by a click on the fix, by E, Finding ▸ Edit Wording… or the
    // right-click menu's, opens with the cursor at the wording's end, even while review has yet to
    // answer a click on the fix: the click's place is for the edit that click asked for alone.
    void anEditAskedOtherwiseOpensAtTheWordingsEnd_data() {
        QTest::addColumn<QString>("how");
        QTest::newRow("E") << "E";
        QTest::newRow("Finding ▸ Edit Wording…") << "edit";
        QTest::newRow("the right-click menu's Edit Wording…") << "rowEdit";
    }
    void anEditAskedOtherwiseOpensAtTheWordingsEnd() {
        QFETCH(QString, how);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *details = w.item("details");
        QQuickItem *field = inEdit(w, "editField");
        QObject *edit = w.find("edit");
        QObject *menu = w.find("rowMenu");
        QVERIFY(details && field && edit && menu);
        QTRY_VERIFY(edit->property("enabled").toBool());
        if (how == "rowEdit") {
            w.rightClick(w.item("nowBlock"));
            QTRY_VERIFY(menu->property("opened").toBool());
        }
        // As a click on the fix leaves it while its edit waits for review.
        QVERIFY(details->setProperty("cursorAt", 3));
        const qsizetype from = w.bridge.sent.size();
        if (how == "E") {
            QTest::keyClick(w.window, Qt::Key_E);
        } else if (how == "edit") {
            QVERIFY(QMetaObject::invokeMethod(edit, "trigger"));
        } else {
            QQuickItem *rowEdit = rowMenuItem(w, "rowEdit");
            QVERIFY(rowEdit);
            QTRY_VERIFY(rowEdit->property("enabled").toBool());
            w.click(rowEdit);
            QTRY_VERIFY(!menu->property("visible").toBool());
            // The window manager gives the keys back to the window a menu drawn in a window of its
            // own was over, which offscreen has none to do.
            w.window->requestActivate();
            QTRY_COMPARE(QGuiApplication::focusWindow(), static_cast<QWindow *>(w.window));
        }
        QCOMPARE(w.since(from), QStringList{"Edit"});
        QTRY_VERIFY(field->hasActiveFocus());
        QCOMPARE(field->property("cursorPosition").toInt(), w.bridge.editWording().size());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit"}));
        QTRY_VERIFY(!w.bridge.editing());
    }

    // A click on the fix that review answers with no edit open, as under a question, which the
    // Edit only lets go, leaves its place to no later edit: E then opens one with the cursor at its
    // end.
    void aClickReviewAnswersWithNoEditLeavesNoPlace() {
        Shown w({media()});
        bool asked = false;
        connect(&w.bridge, &SessionBridge::eventArrived, &w.bridge,
                [&asked](const subcheck_ffi::Event &e) {
                    if (const auto *change =
                            std::get_if<subcheck_ffi::Event::kChanged>(&e.get_variant())) {
                        asked = change->summary.question.has_value();
                    }
                });
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *area = inEdit(w, "rewordArea");
        QQuickItem *line = inEdit(w, "blockLine");
        QQuickItem *field = inEdit(w, "editField");
        QVERIFY(area && line && field);
        const QString wording = QStringLiteral("We should leave before it rains.");
        QCOMPARE(text(w.bridge.findings()->at(0)->wording), wording);
        // Recheck asks first. The window has no way to ask it yet, so the test sends it.
        w.bridge.send(subcheck_ffi::Action(subcheck_ffi::Action::kRecheck{}));
        QTRY_VERIFY(asked);
        QTRY_VERIFY(area->isVisible());
        // A quarter into the first character of a word before the change, as a click that opens
        // the edit puts the cursor there.
        const QFontMetricsF m(line->property("font").value<QFont>());
        const qsizetype s = wording.indexOf("should");
        const qreal across = line->property("leftPadding").toReal() +
                             m.horizontalAdvance(wording.left(s)) +
                             m.horizontalAdvance(wording.at(s)) / 4;
        const qreal down = line->property("topPadding").toReal() + m.height() / 2;
        const qsizetype from = w.bridge.sent.size();
        QTest::mouseClick(w.window, Qt::LeftButton, {},
                          line->mapToScene(QPointF(across, down)).toPoint());
        QCOMPARE(w.since(from), QStringList{"Edit"});
        QTRY_VERIFY(!asked);
        QVERIFY(!w.bridge.editing());
        QVERIFY(!field->isVisible());

        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(field->hasActiveFocus());
        QCOMPARE(field->property("cursorPosition").toInt(), w.bridge.editWording().size());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), (QStringList{"Edit", "Edit", "CancelEdit"}));
        QTRY_VERIFY(!w.bridge.editing());
    }

    // A double-click on the fix, as on words to select them, opens one edit and asks nothing else,
    // whether review answers the first click before the second comes or after it. Once the edit
    // is open, Save and Accept and Cancel lie below every point a click on the block could be at,
    // so that a second click that comes after the edit has opened lands on the field or empty
    // room: at the foot of a line of one, which keeps room for two, on the lower half of a wrapped
    // line's second, at the largest size in the narrowest window, at the block's foot, under a
    // line of one and under a wrapped one, and on the label.
    void aDoubleClickOnTheFixOpensOneEdit_data() {
        QTest::addColumn<QString>("where");
        QTest::addColumn<bool>("answered");
        QTest::newRow("before review answers") << "the middle" << false;
        QTest::newRow("at the foot of a line of one") << "the foot" << true;
        QTest::newRow("on a wrapped line's second") << "the second line" << true;
        QTest::newRow("at the block's foot") << "the block's foot" << true;
        QTest::newRow("at a wrapped line's block's foot") << "the wrapped block's foot" << true;
        QTest::newRow("on the label") << "the label" << true;
    }
    void aDoubleClickOnTheFixOpensOneEdit() {
        QFETCH(QString, where);
        QFETCH(bool, answered);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *fixed = w.item("fixedBlock");
        QQuickItem *area = inEdit(w, "rewordArea");
        QQuickItem *label = inEdit(w, "blockLabel");
        QQuickItem *line = inEdit(w, "blockLine");
        QQuickItem *field = inEdit(w, "editField");
        QQuickItem *save = inEdit(w, "saveButton");
        QQuickItem *cancel = inEdit(w, "cancelButton");
        QVERIFY(fixed && area && label && line && field && save && cancel);
        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());
        const qreal left = line->property("leftPadding").toReal();
        // How many lines the line is drawn in, as its text is laid out: a rich text's lineCount is
        // not kept.
        const auto drawnLines = [line] {
            const qreal spacing =
                QFontMetricsF(line->property("font").value<QFont>()).lineSpacing();
            return qRound(line->property("contentHeight").toReal() / spacing);
        };
        if (where == "the second line" || where == "the wrapped block's foot") {
            w.settings.setDetailsTextSize(24);
            w.window->resize(w.window->minimumWidth(), w.window->height());
            QTRY_COMPARE(w.window->width(), w.window->minimumWidth());
            QTRY_COMPARE(fixNotLaidOut(w, w.window->property("detailsLeast").toReal()), QString());
            QVERIFY2(drawnLines() >= 2, qPrintable(QString::number(drawnLines())));
        } else {
            askRoomForOneLine(w, line);
            if (QTest::currentTestFailed()) {
                return;
            }
            QTRY_COMPARE(fixNotLaidOut(w, roomForOneLine), QString());
            QCOMPARE(drawnLines(), 1);
        }
        QTRY_VERIFY(area->isVisible());
        QTRY_VERIFY(inDetailsView(w, line));
        const qreal spacing = QFontMetricsF(line->property("font").value<QFont>()).lineSpacing();
        QPoint at;
        if (where == "the middle") {
            at = w.middle(line);
        } else if (where == "the foot") {
            at = line->mapToScene(QPointF(left + 1, line->height() - 1)).toPoint();
        } else if (where == "the second line") {
            at = line->mapToScene(
                         QPointF(left + 1, line->property("topPadding").toReal() + 1.75 * spacing))
                     .toPoint();
        } else if (where == "the label") {
            at = w.middle(label);
        } else {
            // Above Save and Accept's left end, which the press would reach were the buttons not
            // kept below the block's foot.
            at = fixed
                     ->mapToScene(
                         QPointF(fixed->property("leftPadding").toReal() + 1, fixed->height() - 1))
                     .toPoint();
        }
        // Where a click asks for the edit, before it opens.
        const QRectF covered = area->mapRectToScene(QRectF(0, 0, area->width(), area->height()));
        QVERIFY(covered.contains(at));
        const qsizetype from = w.bridge.sent.size();
        if (answered) {
            // Two clicks within the double-click interval, as their times say, with the events
            // between them run, so that review answers the first and the edit opens.
            QTest::mousePress(w.window, Qt::LeftButton, {}, at, 10);
            QTest::mouseRelease(w.window, Qt::LeftButton, {}, at, 10);
            QTRY_VERIFY(field->hasActiveFocus());
            QTest::mousePress(w.window, Qt::LeftButton, {}, at, 10);
            QTest::mouseRelease(w.window, Qt::LeftButton, {}, at, 10);
        } else {
            QTest::mouseDClick(w.window, Qt::LeftButton, {}, at);
        }
        QVERIFY2(w.since(from) == QStringList{"Edit"}, qPrintable(w.since(from).join(" | ")));
        QTRY_VERIFY(field->hasActiveFocus());
        QTest::qWait(100);
        QVERIFY2(w.since(from) == QStringList{"Edit"}, qPrintable(w.since(from).join(" | ")));
        QVERIFY(field->hasActiveFocus());
        // The edit's parts laid out, as the next frame draws them.
        w.item("fixedBlock")->property("contentItem").value<QQuickItem *>()->ensurePolished();
        save->parentItem()->ensurePolished();
        for (QQuickItem *button : {save, cancel}) {
            QVERIFY(button->isVisible());
            const QRectF drawn =
                button->mapRectToScene(QRectF(0, 0, button->width(), button->height()));
            QVERIFY2(!drawn.intersects(covered),
                     qPrintable(QStringLiteral("%1 at %2,%3 %4x%5 under a click on the block, "
                                               "which covered %6,%7 %8x%9")
                                    .arg(button->property("text").toString())
                                    .arg(drawn.x())
                                    .arg(drawn.y())
                                    .arg(drawn.width())
                                    .arg(drawn.height())
                                    .arg(covered.x())
                                    .arg(covered.y())
                                    .arg(covered.width())
                                    .arg(covered.height())));
        }
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit"}));
        QTRY_VERIFY(!w.bridge.editing());
    }

    // A click on the line with the fix after the edit a click opened has ended asks for the edit
    // again, however soon after the first it comes: the edit came between the two, which are not a
    // double-click. A double-click then asks once, as any does.
    void aClickAfterTheEditAClickOpenedEndsAsksAgain() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *area = inEdit(w, "rewordArea");
        QQuickItem *line = inEdit(w, "blockLine");
        QQuickItem *field = inEdit(w, "editField");
        QVERIFY(area && line && field);
        QTRY_VERIFY(w.find("edit")->property("enabled").toBool());
        QTRY_VERIFY(area->isVisible());
        QTRY_VERIFY(inDetailsView(w, line));
        const QPoint at = w.middle(line);
        const qsizetype from = w.bridge.sent.size();
        // Each click within the double-click interval of the one before, as their times say, with
        // the events between them run.
        QTest::mousePress(w.window, Qt::LeftButton, {}, at, 10);
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, at, 10);
        QTRY_VERIFY(field->hasActiveFocus());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(area->isVisible());
        QTest::mousePress(w.window, Qt::LeftButton, {}, at, 10);
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, at, 10);
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit", "Edit"}));
        QTRY_VERIFY(field->hasActiveFocus());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(area->isVisible());
        QTest::mouseDClick(w.window, Qt::LeftButton, {}, at, 10);
        const QStringList asked{"Edit", "CancelEdit", "Edit", "CancelEdit", "Edit"};
        QCOMPARE(w.since(from), asked);
        QTRY_VERIFY(field->hasActiveFocus());
        QTest::qWait(100);
        QCOMPARE(w.since(from), asked);
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), asked + QStringList{"CancelEdit"});
        QTRY_VERIFY(!w.bridge.editing());
    }

    // A click on the line with the fix that review answers with no edit open, as under a question,
    // which the Edit only lets go, ends its ask as an edit opened does: the next click asks again,
    // however soon after the first it comes, and opens the edit.
    void aClickReviewAnswersWithNoEditEndsItsAsk() {
        Shown w({media()});
        bool asked = false;
        connect(&w.bridge, &SessionBridge::eventArrived, &w.bridge,
                [&asked](const subcheck_ffi::Event &e) {
                    if (const auto *change =
                            std::get_if<subcheck_ffi::Event::kChanged>(&e.get_variant())) {
                        asked = change->summary.question.has_value();
                    }
                });
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *area = inEdit(w, "rewordArea");
        QQuickItem *line = inEdit(w, "blockLine");
        QQuickItem *field = inEdit(w, "editField");
        QVERIFY(area && line && field);
        // Recheck asks first. The window has no way to ask it yet, so the test sends it.
        w.bridge.send(subcheck_ffi::Action(subcheck_ffi::Action::kRecheck{}));
        QTRY_VERIFY(asked);
        QTRY_VERIFY(area->isVisible());
        QTRY_VERIFY(inDetailsView(w, line));
        const QPoint at = w.middle(line);
        const qsizetype from = w.bridge.sent.size();
        // Each click within the double-click interval of the one before, as their times say, with
        // the events between them run.
        QTest::mousePress(w.window, Qt::LeftButton, {}, at, 10);
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, at, 10);
        QCOMPARE(w.since(from), QStringList{"Edit"});
        QTRY_VERIFY(!asked);
        QVERIFY(!w.bridge.editing());
        QVERIFY(area->isVisible());
        QTest::mousePress(w.window, Qt::LeftButton, {}, at, 10);
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, at, 10);
        QCOMPARE(w.since(from), (QStringList{"Edit", "Edit"}));
        QTRY_VERIFY(field->hasActiveFocus());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), (QStringList{"Edit", "Edit", "CancelEdit"}));
        QTRY_VERIFY(!w.bridge.editing());
    }

    // A click on the fix does nothing where it cannot be reworded: while Settings has the keys, as
    // the decisions wait then too, and on Settings' sample of it; on a timing fix, which shows no
    // text cursor either; and under a greyed episode's log, which hides the fix.
    void aClickOnTheFixDoesNothingWhereItCannotReword() {
        {
            Shown w({media()});
            QVERIFY(w.start());
            QTRY_COMPARE(w.rows(), 2);
            QQuickItem *fixed = w.item("fixedBlock");
            QQuickItem *area = inEdit(w, "rewordArea");
            QQuickItem *field = inEdit(w, "editField");
            QQuickWindow *sw = w.settingsWindow();
            QVERIFY(fixed && area && field && sw);
            QTRY_VERIFY(area->isVisible());
            const qsizetype from = w.bridge.sent.size();
            QTest::keyClick(w.window, Qt::Key_Comma, Qt::ControlModifier);
            QTRY_VERIFY(sw->isVisible());
            QVERIFY(!area->isVisible());
            w.click(fixed);
            QQuickItem *sample = w.settingsItem("sampleFixed");
            QVERIFY(sample);
            QQuickItem *sampleArea = Shown::item(sample, "rewordArea");
            QVERIFY(sampleArea);
            QVERIFY(!sampleArea->isVisible());
            w.click(sample);
            QTest::keyClick(sw, Qt::Key_Escape);
            QTRY_VERIFY(!sw->isVisible());
            QTRY_VERIFY(area->isVisible());

            QVERIFY(timingRowAtTheSelection(w));
            QTRY_VERIFY(!area->isVisible());
            w.click(fixed);
            QTest::qWait(100);
            QCOMPARE(w.since(from), QStringList());
            QVERIFY(!field->isVisible());
        }
        Shown w({clean(), media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *fixed = w.item("fixedBlock");
        QQuickItem *area = inEdit(w, "rewordArea");
        QVERIFY(fixed && area);
        QTRY_VERIFY(area->isVisible());
        QTRY_VERIFY(w.item("episode0"));
        w.click(w.item("episode0"));
        QTRY_VERIFY(w.bridge.showingEpisode());
        QTRY_VERIFY(!fixed->isVisible());
        QVERIFY(!area->isVisible());
    }

    // A right-click on the line with the fix is still the finding's menu, for the selected row: it
    // asks nothing and opens no edit.
    void aRightClickOnTheFixIsStillTheMenu() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *menu = w.find("rowMenu");
        QQuickItem *fixed = w.item("fixedBlock");
        QQuickItem *area = inEdit(w, "rewordArea");
        QQuickItem *field = inEdit(w, "editField");
        QVERIFY(menu && fixed && area && field);
        QTRY_VERIFY(area->isVisible());
        const qsizetype from = w.bridge.sent.size();
        w.rightClick(fixed);
        QTRY_VERIFY(menu->property("opened").toBool());
        QCOMPARE(menu->property("row").toInt(), w.bridge.selected());
        QCOMPARE(w.since(from), QStringList());
        QVERIFY(!field->isVisible());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_VERIFY(!menu->property("visible").toBool());
        QCOMPARE(w.since(from), QStringList());
        QVERIFY(!field->isVisible());
    }

    // For a screen reader the line with the fix is a button named as the menus' Edit Wording…,
    // which opens the edit with the cursor at its end; while the fix cannot be reworded, the
    // window tells of none.
    void theFixIsAButtonForAScreenReader() {
        QAccessible::setActive(true);
        const auto inactive = qScopeGuard([] { QAccessible::setActive(false); });
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *area = inEdit(w, "rewordArea");
        QQuickItem *button = inEdit(w, "rewordButton");
        QQuickItem *label = inEdit(w, "blockLabel");
        QQuickItem *field = inEdit(w, "editField");
        QVERIFY(area && button && label && field);
        QTRY_VERIFY(area->isVisible());
        const QString name = QStringLiteral("Edit Wording…");
        QAccessibleInterface *said = QAccessible::queryAccessibleInterface(button);
        QVERIFY(said && said->actionInterface());
        QCOMPARE(said->role(), QAccessible::Button);
        QCOMPARE(said->text(QAccessible::Name), name);
        const QStringList names = accessibleNames(QAccessible::queryAccessibleInterface(w.window));
        QCOMPARE(names.count(name), 1);
        // The click area over the whole block is no button of its own, and the button covers only
        // the line: the label's words stay for a screen reader's pointer review to meet.
        QAccessibleInterface *label_said = QAccessible::queryAccessibleInterface(label);
        QVERIFY(label_said);
        QVERIFY(names.contains(label_said->text(QAccessible::Name)));
        QVERIFY(!said->rect().intersects(label_said->rect()));
        const qsizetype from = w.bridge.sent.size();
        said->actionInterface()->doAction(QAccessibleActionInterface::pressAction());
        QCOMPARE(w.since(from), QStringList{"Edit"});
        QTRY_VERIFY(field->hasActiveFocus());
        QCOMPARE(field->property("cursorPosition").toInt(), w.bridge.editWording().size());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit"}));
        QTRY_VERIFY(area->isVisible());

        QVERIFY(timingRowAtTheSelection(w));
        QTRY_VERIFY(!area->isVisible());
        QVERIFY(!accessibleNames(QAccessible::queryAccessibleInterface(w.window)).contains(name));
    }

    // Ctrl+F gives the Find box at the toolbar's right the keys; the box shows from the start, and
    // what is typed there asks review nothing. Return selects the next row after the selected one
    // that holds the words, Shift+Return the one before, round past either end, as the box's › and
    // ‹ do, which take no keys from the field. Words no row holds select nothing and say "No
    // match", until the words change. None of it moves the list, which stays under its heading.
    // The first row is opened, so that its line is in three rows of four, the row and its members,
    // and the next and the one before differ.
    void theFindBoxFindsFromTheSelectionAndSaysNoMatch() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *box = w.item("findBox");
        QQuickItem *field = w.item("findField");
        QQuickItem *next = w.item("findNext");
        QQuickItem *noMatch = w.item("noMatch");
        QQuickItem *list = w.item("findings");
        QQuickItem *heading = w.item("heading");
        QVERIFY(box && field && next && noMatch && list && heading);
        QVERIFY(!w.item("findDone"));
        QVERIFY(box->isVisible());
        // Whether the list starts where its heading's row ends.
        QQuickItem *headingRow = heading->parentItem();
        const auto underItsHeading = [list, headingRow] {
            return qAbs(list->mapToItem(headingRow, QPointF()).y() - headingRow->height()) < 0.5;
        };
        QVERIFY(underItsHeading());
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 4);

        const qsizetype from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(field->hasActiveFocus());
        QVERIFY(box->isVisible());
        QVERIFY(underItsHeading());
        QVERIFY(text(w.bridge.findings()->at(0)->original).contains("befor"));
        typeIn(w, "befor");
        QCOMPARE(field->property("text").toString(), "befor");
        QCOMPARE(w.since(from), QStringList());
        // Each way to find, whether it goes back, and the row it lands on from the one selected.
        const std::vector<std::tuple<QString, bool, int>> finds = {{"Return", false, 1},
                                                                   {"Shift+Return", true, 0},
                                                                   {"findPrevious", true, 2},
                                                                   {"findNext", false, 0}};
        for (const auto &[how, back, lands] : finds) {
            QVERIFY2(w.bridge.findings()->find("befor", w.bridge.selected(), back) == lands,
                     qPrintable(how));
            const qsizetype before = w.bridge.sent.size();
            if (how == "Return") {
                QTest::keyClick(w.window, Qt::Key_Return);
            } else if (how == "Shift+Return") {
                QTest::keyClick(w.window, Qt::Key_Return, Qt::ShiftModifier);
            } else {
                w.click(w.item(qPrintable(how)));
            }
            QVERIFY2(w.since(before) == QStringList{selecting(w, lands)},
                     qPrintable(how + ": " + w.since(before).join(" | ")));
            QTRY_COMPARE(w.bridge.selected(), lands);
            QVERIFY2(field->hasActiveFocus(), qPrintable(how));
            QVERIFY2(!noMatch->isVisible(), qPrintable(how));
            QVERIFY2(underItsHeading(), qPrintable(how));
        }

        // Ctrl+F selects the words, so that the next typed take their place.
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        typeIn(w, "zzz");
        QCOMPARE(field->property("text").toString(), "zzz");
        QCOMPARE(w.bridge.findings()->find("zzz", -1, false), -1);
        const qsizetype before = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_Return);
        QVERIFY(noMatch->isVisible());
        QCOMPARE(noMatch->property("text").toString(), "No match");
        QVERIFY(underItsHeading());
        w.click(next);
        QVERIFY(noMatch->isVisible());
        typeIn(w, "z");
        QVERIFY(!noMatch->isVisible());
        QCOMPARE(w.since(before), QStringList());
        QCOMPARE(w.bridge.selected(), 0);
        QVERIFY(underItsHeading());
    }

    // "No match" is the last search's: it stays while the field gives the window its keys, and as
    // the field has them back with its window active again, and goes once the field takes them
    // again, by Ctrl+F, a click, Tab or Shift+Tab. Words that are empty, or spaces alone, are no
    // search: Return, Shift+Return, › and ‹ on them say nothing and select nothing. The box never
    // hides.
    void theFindBoxSaysNoMatchOnlyForTheSearchItShowsWith() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *box = w.item("findBox");
        QQuickItem *field = w.item("findField");
        QQuickItem *noMatch = w.item("noMatch");
        QVERIFY(box && field && noMatch);
        const qsizetype from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(field->hasActiveFocus());
        typeIn(w, "zzz");
        QTest::keyClick(w.window, Qt::Key_Return);
        QVERIFY(noMatch->isVisible());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QVERIFY(!field->hasActiveFocus());
        QVERIFY(noMatch->isVisible());
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(field->hasActiveFocus());
        QCOMPARE(field->property("selectedText").toString(), "zzz");
        QVERIFY(!noMatch->isVisible());
        QTest::keyClick(w.window, Qt::Key_Return);
        QVERIFY(noMatch->isVisible());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QVERIFY(!field->hasActiveFocus());
        QVERIFY(noMatch->isVisible());
        w.click(field);
        QTRY_VERIFY(field->hasActiveFocus());
        QVERIFY(!noMatch->isVisible());
        QCOMPARE(field->property("text").toString(), "zzz");
        for (const bool back : {false, true}) {
            QTest::keyClick(w.window, Qt::Key_Return);
            QVERIFY(noMatch->isVisible());
            QTest::keyClick(w.window, Qt::Key_Escape);
            QVERIFY(!field->hasActiveFocus());
            QVERIFY(noMatch->isVisible());
            QTest::keyClick(w.window, back ? Qt::Key_Backtab : Qt::Key_Tab,
                            back ? Qt::ShiftModifier : Qt::NoModifier);
            QTRY_VERIFY2(field->hasActiveFocus(), back ? "Shift+Tab" : "Tab");
            QVERIFY2(!noMatch->isVisible(), back ? "Shift+Tab" : "Tab");
        }
        // Another window active, then this one again, the field's keys with it.
        QTest::keyClick(w.window, Qt::Key_Return);
        QVERIFY(noMatch->isVisible());
        QWindow other;
        other.resize(100, 100);
        other.show();
        other.requestActivate();
        QVERIFY(QTest::qWaitForWindowActive(&other));
        QTRY_VERIFY(!field->hasActiveFocus());
        w.window->requestActivate();
        QVERIFY(QTest::qWaitForWindowActive(w.window));
        QTRY_VERIFY(field->hasActiveFocus());
        QVERIFY(noMatch->isVisible());
        other.close();

        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QCOMPARE(field->property("selectedText").toString(), "zzz");
        QTest::keyClick(w.window, Qt::Key_Backspace);
        QCOMPARE(field->property("text").toString(), "");
        for (const char *words : {"", "  "}) {
            typeIn(w, words);
            QCOMPARE(field->property("text").toString(), words);
            QTest::keyClick(w.window, Qt::Key_Return);
            QVERIFY2(!noMatch->isVisible(), words);
            QTest::keyClick(w.window, Qt::Key_Return, Qt::ShiftModifier);
            QVERIFY2(!noMatch->isVisible(), words);
            w.click(w.item("findNext"));
            QVERIFY2(!noMatch->isVisible(), words);
            w.click(w.item("findPrevious"));
            QVERIFY2(!noMatch->isVisible(), words);
        }
        QTest::qWait(100);
        QCOMPARE(w.since(from), QStringList());
        QCOMPARE(w.bridge.selected(), 0);
        QTest::keyClick(w.window, Qt::Key_Escape);
        QVERIFY(!field->hasActiveFocus());
        QVERIFY(box->isVisible());
    }

    // While the Find field has the keys, those it ends by, moves through or types are its own:
    // Return and the keypad's Enter find, and open or close no row; Up, Down, PgUp and PgDn move
    // nothing; Home and End move through the words; the window's letters type themselves, Shift+G
    // among them. Ctrl+F again selects what was typed. Esc gives the window its keys back and
    // keeps the words, the box still there; a click in the field gives it the keys again.
    void theFindBoxTakesItsKeysAndEscGivesThemBack() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *box = w.item("findBox");
        QQuickItem *field = w.item("findField");
        QVERIFY(box && field);
        QVERIFY(!w.item("findDone"));
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(field->hasActiveFocus());
        const qsizetype from = w.bridge.sent.size();
        const std::vector<std::pair<Qt::Key, Qt::KeyboardModifiers>> keys = {
            {Qt::Key_Return, {}}, {Qt::Key_Enter, Qt::KeypadModifier},
            {Qt::Key_Up, {}},     {Qt::Key_Down, {}},
            {Qt::Key_PageUp, {}}, {Qt::Key_PageDown, {}}};
        for (const auto &[key, modifiers] : keys) {
            const QString name = QKeySequence(key | modifiers).toString();
            QTest::keyClick(w.window, key, modifiers);
            QVERIFY2(w.since(from).isEmpty(), qPrintable(name + ": " + w.since(from).join(" | ")));
            QVERIFY2(field->hasActiveFocus(), qPrintable(name));
        }
        QTest::keyClick(w.window, Qt::Key_J);
        QTest::keyClick(w.window, Qt::Key_K);
        QCOMPARE(field->property("text").toString(), "jk");
        QTest::keyClick(w.window, Qt::Key_Home);
        QCOMPARE(field->property("cursorPosition").toInt(), 0);
        QTest::keyClick(w.window, Qt::Key_End);
        QCOMPARE(field->property("cursorPosition").toInt(), 2);
        QTest::keyClick(w.window, 'G', Qt::ShiftModifier);
        QCOMPARE(field->property("text").toString(), "jkG");
        QVERIFY(field->hasActiveFocus());
        QCOMPARE(w.since(from), QStringList());
        QCOMPARE(field->property("selectedText").toString(), "");
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QCOMPARE(field->property("selectedText").toString(), "jkG");
        QVERIFY(field->hasActiveFocus());
        QTest::qWait(100);
        QCOMPARE(w.since(from), QStringList());
        QCOMPARE(w.rows(), 2);
        QCOMPARE(w.bridge.selected(), 0);

        QTest::keyClick(w.window, Qt::Key_Escape);
        QVERIFY(box->isVisible());
        QVERIFY(!field->hasActiveFocus());
        QCOMPARE(field->property("text").toString(), "jkG");
        QCOMPARE(w.since(from), QStringList());
        QTest::keyClick(w.window, Qt::Key_J);
        QCOMPARE(w.since(from), QStringList{"Move 1"});

        // A click in the field gives it the keys again, with the words it had.
        w.click(field);
        QTRY_VERIFY(field->hasActiveFocus());
        QCOMPARE(field->property("text").toString(), "jkG");
        QTest::keyClick(w.window, Qt::Key_Escape);
        QVERIFY(!field->hasActiveFocus());
        QVERIFY(box->isVisible());
        QTest::keyClick(w.window, Qt::Key_K);
        QCOMPARE(w.since(from), (QStringList{"Move 1", "Move -1"}));
    }

    // While an edit is open, Find waits: from its box, Return would select another row, which ends
    // the edit and drops its typed wording. The box is greyed, never hidden, and Ctrl+F types
    // nothing into the edit's field. An edit opened while the box has the keys, by Finding ▸ Edit
    // Wording… as E there is a letter the Find field types, takes them: the Find field, greyed,
    // gives them up, and keeps its words. By the right-click menu's Edit Wording…, the right-click
    // has given the window its keys already, and the edit takes them alike.
    void theFindBoxWaitsWhileAnEditIsOpen() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *find = w.find("find");
        QQuickItem *findField = w.item("findField");
        QQuickItem *previous = w.item("findPrevious");
        QQuickItem *next = w.item("findNext");
        QQuickItem *editField = inEdit(w, "editField");
        QQuickItem *now = w.item("nowBlock");
        QObject *menu = w.find("rowMenu");
        QVERIFY(find && findField && previous && next && editField && now && menu);
        const QList<QQuickItem *> parts = {findField, previous, next};
        QTRY_VERIFY(find->property("enabled").toBool());
        const qsizetype from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_E);
        QTRY_VERIFY(editField->hasActiveFocus());
        QTRY_VERIFY(!find->property("enabled").toBool());
        for (QQuickItem *part : parts) {
            QVERIFY2(!part->isEnabled(), qPrintable(part->objectName()));
            QVERIFY2(part->isVisible(), qPrintable(part->objectName()));
        }
        const QString wording = editField->property("text").toString();
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QCOMPARE(editField->property("text").toString(), wording);
        QVERIFY(editField->hasActiveFocus());
        QVERIFY(!findField->hasActiveFocus());
        QCOMPARE(w.since(from), QStringList{"Edit"});
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit"}));
        QTRY_VERIFY(find->property("enabled").toBool());
        for (QQuickItem *part : parts) {
            QVERIFY2(part->isEnabled(), qPrintable(part->objectName()));
        }

        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(findField->hasActiveFocus());
        QTest::keyClick(w.window, Qt::Key_E);
        QCOMPARE(findField->property("text").toString(), "e");
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit"}));
        QObject *edit = w.find("edit");
        QTRY_VERIFY(edit->property("enabled").toBool());
        QVERIFY(QMetaObject::invokeMethod(edit, "trigger"));
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit", "Edit"}));
        QTRY_VERIFY(editField->hasActiveFocus());
        QVERIFY(!findField->hasActiveFocus());
        QVERIFY(!findField->isEnabled());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit", "Edit", "CancelEdit"}));
        QTRY_VERIFY(!w.bridge.editing());
        QCOMPARE(findField->property("text").toString(), "e");

        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(findField->hasActiveFocus());
        QTRY_VERIFY(edit->property("enabled").toBool());
        const qsizetype menuFrom = w.bridge.sent.size();
        w.rightClick(now);
        QTRY_VERIFY(menu->property("opened").toBool());
        QQuickItem *rowEdit = rowMenuItem(w, "rowEdit");
        QVERIFY(rowEdit);
        QTRY_VERIFY(rowEdit->property("enabled").toBool());
        w.click(rowEdit);
        QTRY_VERIFY(!menu->property("visible").toBool());
        QCOMPARE(w.since(menuFrom), QStringList{"Edit"});
        // The window manager gives the keys back to the window a menu drawn in a window of its own
        // was over, which offscreen has none to do.
        w.window->requestActivate();
        QTRY_COMPARE(QGuiApplication::focusWindow(), static_cast<QWindow *>(w.window));
        QTRY_VERIFY(editField->hasActiveFocus());
        QVERIFY(!findField->hasActiveFocus());
        QVERIFY(!findField->isEnabled());
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(menuFrom), (QStringList{"Edit", "CancelEdit"}));
        QTRY_VERIFY(!w.bridge.editing());
        QCOMPARE(findField->property("text").toString(), "e");
    }

    // A click on a row, or on its triangle, while the Find field has the keys selects the row, the
    // triangle opening or closing it too, and gives the window its keys back, the words kept: the
    // box stays, and a letter after the click is the window's, not typed into the field.
    void aClickOnARowTakesTheKeysFromTheFindBox_data() {
        QTest::addColumn<QString>("style");
        QTest::addColumn<bool>("triangle");
        QTest::newRow("a Two Lines row") << "twoLines" << false;
        QTest::newRow("a Two Lines row's triangle") << "twoLines" << true;
        QTest::newRow("a Compact row") << "compact" << false;
        QTest::newRow("a Compact row's triangle") << "compact" << true;
    }
    void aClickOnARowTakesTheKeysFromTheFindBox() {
        QFETCH(QString, style);
        QFETCH(bool, triangle);
        Shown w({media()});
        w.settings.setRowStyle(style);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *field = w.item("findField");
        QVERIFY(field);
        // Opened, so that the find lands on another row than the one clicked.
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 4);
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(field->hasActiveFocus());
        typeIn(w, "befor");
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.bridge.selected(), 1);
        QVERIFY(field->hasActiveFocus());
        QTRY_VERIFY(w.delegate(0) && drawnStyle(w.delegate(0)) == style);
        QStringList clicked{selecting(w, 0)};
        const qsizetype from = w.bridge.sent.size();
        if (triangle) {
            // Past the column's first 8 px, which the handle between the sidebar and the list
            // takes.
            QQuickItem *disclosure = Shown::item(w.delegate(0), "rowDisclosure");
            QVERIFY(disclosure && disclosure->isVisible());
            QTest::mouseClick(
                w.window, Qt::LeftButton, {},
                disclosure->mapToScene(QPointF(9, disclosure->height() / 2)).toPoint());
            clicked << "Toggle";
        } else {
            w.click(w.delegate(0));
        }
        QCOMPARE(w.since(from), clicked);
        QTRY_COMPARE(w.bridge.selected(), 0);
        QTRY_COMPARE(w.rows(), triangle ? 2 : 4);
        QVERIFY(!field->hasActiveFocus());
        QCOMPARE(field->property("text").toString(), "befor");
        QTRY_VERIFY(w.find("accept")->property("enabled").toBool());
        QTest::keyClick(w.window, Qt::Key_A);
        QCOMPARE(w.since(from), clicked + QStringList{"Accept"});
        QTRY_COMPARE(w.decision(0), "Accepted");
        QCOMPARE(field->property("text").toString(), "befor");
        QTest::keyClick(w.window, Qt::Key_U);
        QCOMPARE(w.since(from), (clicked + QStringList{"Accept", "Undo"}));
        QTRY_COMPARE(w.decision(0), "To review");
    }

    // A click on an entry of the sidebar, an episode, a filter or a review, while the Find field
    // has the keys asks review about it and gives the window its keys back, the words kept: the
    // box stays, and a letter after the click is the window's, not typed into the field.
    void aClickInTheSidebarTakesTheKeysFromTheFindBox_data() {
        QTest::addColumn<QString>("entry");
        QTest::newRow("an episode") << "episode1";
        QTest::newRow("a filter") << "filter1";
        QTest::newRow("a review") << "review0";
    }
    void aClickInTheSidebarTakesTheKeysFromTheFindBox() {
        QFETCH(QString, entry);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *field = w.item("findField");
        QVERIFY(field);
        // A list makes its entries when it next lays itself out, which can come after the rows.
        QTRY_VERIFY(w.item(qPrintable(entry)));
        // Opened, so that the find lands on another row.
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 4);
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(field->hasActiveFocus());
        typeIn(w, "befor");
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.bridge.selected(), 1);
        QVERIFY(field->hasActiveFocus());
        const qsizetype from = w.bridge.sent.size();
        w.click(w.item(qPrintable(entry)));
        QVERIFY(!w.since(from).isEmpty());
        QVERIFY(!field->hasActiveFocus());
        QCOMPARE(field->property("text").toString(), "befor");
        const qsizetype clicked = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_J);
        QCOMPARE(w.since(clicked), QStringList{"Move 1"});
        QCOMPARE(field->property("text").toString(), "befor");
    }

    // A right-click on a row, or on the finding in the details panel, while the Find field has the
    // keys gives the window its keys back as its menu opens, the words kept: once the menu closes,
    // by Esc or by an item, the keys are the window's, not the field's again, and a letter after it
    // is the window's, not typed into the field.
    void aRightClickMenuTakesTheKeysFromTheFindBox_data() {
        QTest::addColumn<bool>("onDetails");
        QTest::addColumn<bool>("byItem");
        QTest::newRow("a row's, closed by Esc") << false << false;
        QTest::newRow("a row's, closed by an item") << false << true;
        QTest::newRow("the details', closed by Esc") << true << false;
        QTest::newRow("the details', closed by an item") << true << true;
    }
    void aRightClickMenuTakesTheKeysFromTheFindBox() {
        QFETCH(bool, onDetails);
        QFETCH(bool, byItem);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *field = w.item("findField");
        QObject *menu = w.find("rowMenu");
        QVERIFY(field && menu);
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(field->hasActiveFocus());
        typeIn(w, "befor");
        QTRY_VERIFY(w.delegate(0));
        QQuickItem *clicked = onDetails ? w.item("nowBlock") : w.delegate(0);
        QVERIFY(clicked && clicked->isVisible());
        w.rightClick(clicked);
        QTRY_VERIFY(menu->property("opened").toBool());
        const qsizetype from = w.bridge.sent.size();
        if (byItem) {
            QQuickItem *accept = rowMenuItem(w, "rowAccept");
            QVERIFY(accept);
            QTRY_VERIFY(accept->property("enabled").toBool());
            w.click(accept);
        } else {
            QTest::keyClick(w.window, Qt::Key_Escape);
        }
        QTRY_VERIFY(!menu->property("visible").toBool());
        // The window manager gives the keys back to the window a menu drawn in a window of its own
        // was over, which offscreen has none to do.
        w.window->requestActivate();
        QTRY_COMPARE(QGuiApplication::focusWindow(), static_cast<QWindow *>(w.window));
        QVERIFY(!field->hasActiveFocus());
        QCOMPARE(field->property("text").toString(), "befor");
        // Accept, by the menu's item or by A, then Undo by U: the window's keys.
        if (!byItem) {
            QTRY_VERIFY(w.find("accept")->property("enabled").toBool());
            QTest::keyClick(w.window, Qt::Key_A);
        }
        QCOMPARE(w.since(from), QStringList{"Accept"});
        QTRY_COMPARE(w.decision(0), "Accepted");
        QTest::keyClick(w.window, Qt::Key_U);
        QCOMPARE(w.since(from), (QStringList{"Accept", "Undo"}));
        QTRY_COMPARE(w.decision(0), "To review");
        QCOMPARE(field->property("text").toString(), "befor");
    }

    // A click on Undo, Reject or Accept under the video, on the video or on the list's Rows button,
    // while the Find field has the keys, gives the window its keys back, the words kept: none of
    // them takes the keys, and the box stays, so a letter after the click is the window's, not
    // typed into the field. The Rows menu the button opens leaves the keys with the window as it
    // closes.
    void aClickUnderTheVideoOrOnRowsTakesTheKeysFromTheFindBox_data() {
        QTest::addColumn<QString>("control");
        QTest::addColumn<QStringList>("asks");
        QTest::addColumn<QString>("decided");
        QTest::newRow("Undo") << "undoButton" << QStringList{"Undo"} << "To review";
        QTest::newRow("Reject") << "rejectButton" << QStringList{"Reject"} << "Rejected";
        QTest::newRow("Accept") << "acceptButton" << QStringList{"Accept"} << "Accepted";
        QTest::newRow("the video") << "video" << QStringList() << "To review";
        QTest::newRow("the Rows button") << "rowStyleButton" << QStringList() << "To review";
    }
    void aClickUnderTheVideoOrOnRowsTakesTheKeysFromTheFindBox() {
        QFETCH(QString, control);
        QFETCH(QStringList, asks);
        QFETCH(QString, decided);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *field = w.item("findField");
        // The video in its own window, where the click lands.
        QQuickItem *clicked =
            control == "video" ? w.videoItem("video") : w.item(qPrintable(control));
        QObject *rowsMenu = w.find("rowsMenu");
        QVERIFY(field && clicked && rowsMenu);
        // Opened, so that the find lands on another row, a member.
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.rows(), 4);
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(field->hasActiveFocus());
        typeIn(w, "befor");
        QTest::keyClick(w.window, Qt::Key_Return);
        QTRY_COMPARE(w.bridge.selected(), 1);
        QVERIFY(field->hasActiveFocus());
        QTRY_VERIFY(clicked->isEnabled());
        const qsizetype from = w.bridge.sent.size();
        w.click(clicked);
        QCOMPARE(w.since(from), asks);
        if (control == "rowStyleButton") {
            QTRY_VERIFY(rowsMenu->property("opened").toBool());
            QTest::keyClick(w.window, Qt::Key_Escape);
            QTRY_VERIFY(!rowsMenu->property("visible").toBool());
            // The window manager gives the keys back to the window a menu drawn in a window of its
            // own was over, which offscreen has none to do.
            w.window->requestActivate();
            QTRY_COMPARE(QGuiApplication::focusWindow(), static_cast<QWindow *>(w.window));
        }
        QVERIFY(!field->hasActiveFocus());
        QCOMPARE(field->property("text").toString(), "befor");
        // Undone once review has decided, so that the run is left as found.
        QTRY_COMPARE(w.decision(1), decided);
        QTest::keyClick(w.window, Qt::Key_U);
        QCOMPARE(w.since(from), asks + QStringList{"Undo"});
        QCOMPARE(field->property("text").toString(), "befor");
        QTRY_COMPARE(w.decision(1), "To review");
    }

    // A click anywhere else in the window while the Find field has the keys gives the window its
    // keys back, the words kept, whatever it lands on: the finding's place in the details panel,
    // Technical details, the list below its rows, a handle between the columns, the toolbar's
    // title, or the fix while review asks, whose Edit then only lets the question go, and opens no
    // edit to take the keys.
    void aClickAnywhereElseTakesTheKeysFromTheFindBox_data() {
        QTest::addColumn<QString>("where");
        QTest::newRow("the finding's place") << "place";
        QTest::newRow("Technical details") << "technical";
        QTest::newRow("the list below its rows") << "below the rows";
        QTest::newRow("a column handle") << "a column handle";
        QTest::newRow("the toolbar's title") << "title";
        QTest::newRow("the fix while review asks") << "the fix";
    }
    void aClickAnywhereElseTakesTheKeysFromTheFindBox() {
        QFETCH(QString, where);
        Shown w({media()});
        bool asked = false;
        connect(&w.bridge, &SessionBridge::eventArrived, &w.bridge,
                [&asked](const subcheck_ffi::Event &e) {
                    if (const auto *change =
                            std::get_if<subcheck_ffi::Event::kChanged>(&e.get_variant())) {
                        asked = change->summary.question.has_value();
                    }
                });
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *box = w.item("findBox");
        QQuickItem *field = w.item("findField");
        QQuickItem *list = w.item("findings");
        QVERIFY(box && field && list);
        QTRY_VERIFY(w.delegate(1));
        QPoint at;
        QStringList asks;
        if (where == "below the rows") {
            QQuickItem *last = w.delegate(1);
            const qreal end = last->mapToItem(list, QPointF(0, last->height())).y();
            QVERIFY2(end < list->height() - 2, qPrintable(QString::number(end)));
            at = list->mapToScene(QPointF(list->width() / 2, (end + list->height()) / 2)).toPoint();
        } else if (where == "a column handle") {
            at = w.handle(w.item("sidebar"), list->parentItem());
        } else if (where == "the fix") {
            // Recheck asks first. The window has no way to ask it yet, so the test sends it.
            w.bridge.send(subcheck_ffi::Action(subcheck_ffi::Action::kRecheck{}));
            QTRY_VERIFY(asked);
            QQuickItem *area = inEdit(w, "rewordArea");
            QQuickItem *line = inEdit(w, "blockLine");
            QVERIFY(area && line);
            QTRY_VERIFY(area->isVisible());
            QTRY_VERIFY(inDetailsView(w, line));
            at = w.middle(line);
            asks << "Edit";
        } else {
            QQuickItem *clicked = w.item(qPrintable(where));
            QVERIFY(clicked && clicked->isVisible());
            if (where != "title") {
                QTRY_VERIFY(inDetailsView(w, clicked));
            }
            at = w.middle(clicked);
        }
        QVERIFY(!box->contains(box->mapFromScene(at)));
        QTest::keyClick(w.window, Qt::Key_F, Qt::ControlModifier);
        QTRY_VERIFY(field->hasActiveFocus());
        typeIn(w, "befor");
        const qsizetype from = w.bridge.sent.size();
        QTest::mouseClick(w.window, Qt::LeftButton, {}, at);
        QCOMPARE(w.since(from), asks);
        if (where == "the fix") {
            QTRY_VERIFY(!asked);
            QVERIFY(!w.bridge.editing());
        }
        QVERIFY(!field->hasActiveFocus());
        QCOMPARE(field->property("text").toString(), "befor");
        const qsizetype clicked = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_J);
        QCOMPARE(w.since(clicked), QStringList{"Move 1"});
        QCOMPARE(field->property("text").toString(), "befor");
    }

    // The three columns side by side, the list between; the video's place at the top of the
    // details panel, 16:9 at its width, and the video the mpv of the window's player; a key of the
    // video's reaches it through review, whose answer is the status bar. Offscreen, mpv never
    // becomes ready, so Show in Video waits there.
    void theColumnsAndTheVideoAreWhereTheyBelong() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *sidebar = w.item("sidebar");
        QQuickItem *list = w.item("findings");
        QQuickItem *details = w.item("details");
        QQuickItem *slot = w.item("videoSlot");
        QQuickItem *video = w.videoItem("video");
        QVERIFY(sidebar && list && details && slot && video);
        const auto left = [](QQuickItem *of) { return of->mapToScene(QPointF(0, 0)).x(); };
        QCOMPARE(left(sidebar), 0.0);
        QVERIFY(left(list) >= sidebar->width());
        QVERIFY(left(details) >= left(list) + list->width());
        QVERIFY(qAbs(left(details) + details->width() - w.window->width()) < 1);
        QCOMPARE(video->property("player").value<PlayerPort *>(), &w.player);
        QCOMPARE(left(slot), left(details));
        QCOMPARE(slot->width(), details->width());
        QVERIFY(qAbs(slot->height() - slot->width() * 9 / 16) < 1);
        QCOMPARE(slot->mapToItem(details, QPointF(0, 0)).y(), 0.0);

        QObject *status = w.find("statusLine");
        QTest::keyClick(w.window, Qt::Key_Space);
        QTRY_COMPARE(status->property("text").toString(),
                     "The video has no file open; Show in Video opens one");
        QTest::keyClick(w.window, Qt::Key_O);
        QTRY_COMPARE(status->property("text").toString(), "Opening E01 at 0:00:01.000");
        QTest::keyClick(w.window, Qt::Key_Right);
        QTRY_COMPARE(status->property("text").toString(), "The video is still opening the file");
        QTest::keyClick(w.window, Qt::Key_O, Qt::ShiftModifier);
        QTRY_COMPARE(status->property("text").toString(), "Video closed");
    }

    // The video draws in a window of its own, black, which a container in the details panel holds
    // at the video's place, 16:9 at the panel's width: the video, the mark in its corner and the
    // busy animation are that window's, and none of them is in the main window's scene. The window
    // keeps to the place as the main window is resized and as the columns are dragged.
    void theVideoDrawsInAWindowOfItsOwnThatTheDetailsPanelHolds() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickWindow *video = w.videoWindow();
        QVERIFY(video);
        QVERIFY(video->isVisible());
        QCOMPARE(video->color(), QColor(Qt::black));
        for (const char *name : {"videoPane", "video", "playIndicator", "videoLoading"}) {
            QQuickItem *each = w.videoItem(name);
            QVERIFY2(each, name);
            QVERIFY2(each->window() == video, name);
        }
        QVERIFY(!w.item("video"));
        auto *mpv = qobject_cast<MpvItem *>(w.videoItem("video"));
        QQuickItem *pane = w.videoItem("videoPane");
        QVERIFY(mpv && pane);
        QCOMPARE(mpv->player(), &w.player);
        QCOMPARE(mpv->mapRectToItem(pane, mpv->boundingRect()), pane->boundingRect());
        auto *container = qobject_cast<QQuickItem *>(w.find("videoContainer"));
        QQuickItem *slot = w.item("videoSlot");
        QQuickItem *details = w.item("details");
        QQuickItem *list = w.item("findings");
        QVERIFY(container && slot && details && list);
        QCOMPARE(container->property("window").value<QWindow *>(), video);
        QCOMPARE(container->parentItem(), slot);
        // What keeps the video's window from its place, said; nothing once it is there.
        const auto off = [&] {
            QStringList said;
            if (container->size() != slot->size()) {
                said << QStringLiteral("the container is %1x%2 in a place %3x%4")
                            .arg(container->width())
                            .arg(container->height())
                            .arg(slot->width())
                            .arg(slot->height());
            }
            if (qAbs(slot->width() - details->width()) >= 0.5 ||
                qAbs(slot->height() - slot->width() * 9 / 16) > 1) {
                said << QStringLiteral("the place is %1x%2 in a panel %3 wide")
                            .arg(slot->width())
                            .arg(slot->height())
                            .arg(details->width());
            }
            const QRect held(container->mapToScene(QPointF()).toPoint(),
                             container->size().toSize());
            if (video->geometry() != held) {
                said << QStringLiteral("the window is at %1,%2 %3x%4, held at %5,%6 %7x%8")
                            .arg(video->x())
                            .arg(video->y())
                            .arg(video->width())
                            .arg(video->height())
                            .arg(held.x())
                            .arg(held.y())
                            .arg(held.width())
                            .arg(held.height());
            }
            return said.join(QStringLiteral("; "));
        };
        const auto place = [slot] { return slot->mapRectToScene(slot->boundingRect()); };
        QTRY_VERIFY2(off().isEmpty(), qPrintable(off()));
        const QRectF first = place();
        w.window->resize(1100, 700);
        QTRY_VERIFY(place() != first);
        QTRY_VERIFY2(off().isEmpty(), qPrintable(off()));
        const QRectF resized = place();
        w.drag(list, details, 40);
        QTRY_VERIFY(place() != resized);
        QTRY_VERIFY2(off().isEmpty(), qPrintable(off()));
    }

    // View ▸ Pop Out Video takes the video's window out of the details panel, into a window of its
    // own, 960 by 540, whose keys reach review as the main window's do, once each; Ctrl+W typed in
    // it puts it back rather than quit, as a person closes the window the key is typed in. Twice
    // round, as each time out sets again what the container's hold took away. The main window has
    // its keys after.
    void popOutVideoTakesTheVideoOutAndCtrlWPutsItBack() {
        Shown w({media()});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickWindow *video = w.videoWindow();
        QObject *container = w.find("videoContainer");
        QObject *popOut = w.find("popOutVideo");
        QVERIFY(video && container && popOut);
        const std::vector<std::pair<Qt::Key, QStringList>> keys = {
            {Qt::Key_J, {"Move 1"}}, {Qt::Key_K, {"Move -1"}}, {Qt::Key_Space, {"Playback Pause"}}};
        for (int round = 1; round <= 2; ++round) {
            qsizetype from = w.bridge.sent.size();
            QVERIFY(QMetaObject::invokeMethod(popOut, "trigger"));
            QCOMPARE(w.since(from), QStringList());
            QCOMPARE(container->property("window").value<QWindow *>(), nullptr);
            QVERIFY(video->isVisible());
            QCOMPARE(video->transientParent(), w.window);
            QCOMPARE(video->size(), QSize(960, 540));
            QVERIFY(popOut->property("checked").toBool());
            video->requestActivate();
            QVERIFY(QTest::qWaitForWindowActive(video));
            for (const auto &[key, actions] : keys) {
                from = w.bridge.sent.size();
                QTest::keyClick(video, key);
                QVERIFY2(w.since(from) == actions,
                         qPrintable(QStringLiteral("round %1, %2: %3")
                                        .arg(round)
                                        .arg(QKeySequence(key).toString(),
                                             w.since(from).join(QStringLiteral(" | ")))));
            }
            from = w.bridge.sent.size();
            QTest::keyClick(video, Qt::Key_W, Qt::ControlModifier);
            QCOMPARE(w.since(from), QStringList());
            QCOMPARE(container->property("window").value<QWindow *>(), video);
            QVERIFY(!popOut->property("checked").toBool());
        }
        QCOMPARE(finished.count(), 0);
        const qsizetype from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_J);
        QCOMPARE(w.since(from), QStringList{"Move 1"});
    }

    // Ctrl+W typed in the main window while the video is out quits, as it does with the video in:
    // the pop-out, transient for the main window, reads active whenever the main window does, so
    // it is the pop-out's keys, not its being active, that make Ctrl+W put the video back.
    void ctrlWInTheMainWindowQuitsWhileTheVideoIsOut() {
        Shown w({media()});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *popOut = w.find("popOutVideo");
        QVERIFY(popOut);
        QVERIFY(QMetaObject::invokeMethod(popOut, "trigger"));
        QVERIFY(QTest::qWaitForWindowActive(w.videoWindow()));
        w.window->requestActivate();
        QTRY_COMPARE(QGuiApplication::focusWindow(), static_cast<QWindow *>(w.window));
        const qsizetype from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_W, Qt::ControlModifier);
        QCOMPARE(w.since(from), (QStringList{"Interrupt", "Interrupt"}));
        QVERIFY(w.window->property("videoOut").toBool());
        QTRY_COMPARE(finished.count(), 1);
    }

    // The popped-out window's own close puts the video back, the same window and the same video,
    // and asks review nothing.
    void closingThePoppedOutWindowPutsItBack() {
        Shown w({media()});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        const QPointer<QQuickWindow> video = w.videoWindow();
        const QPointer<QQuickItem> mpv = w.videoItem("video");
        QObject *container = w.find("videoContainer");
        QObject *popOut = w.find("popOutVideo");
        QVERIFY(video && mpv && container && popOut);
        QVERIFY(QMetaObject::invokeMethod(popOut, "trigger"));
        QVERIFY(w.window->property("videoOut").toBool());
        QCOMPARE(container->property("window").value<QWindow *>(), nullptr);
        const qsizetype from = w.bridge.sent.size();
        QVERIFY(!video->close());
        QVERIFY(!w.window->property("videoOut").toBool());
        QVERIFY(video && mpv);
        QCOMPARE(container->property("window").value<QWindow *>(), video.data());
        QCOMPARE(w.videoWindow(), video.data());
        QCOMPARE(w.videoItem("video"), mpv.data());
        QCOMPARE(w.since(from), QStringList());
        QCOMPARE(finished.count(), 0);
    }

    // Held in the details panel, the video's window takes no focus, so that a click on the video
    // leaves the keys where they were: with an open edit's field, which the next letter is typed
    // into. Out, it takes focus, and in again, none. Offscreen, no click moves the focus between
    // windows: this sees the flag, and that no part of the video takes the keys.
    void theEmbeddedVideoTakesNoFocus() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickWindow *video = w.videoWindow();
        QObject *popOut = w.find("popOutVideo");
        QVERIFY(video && popOut);
        QVERIFY(video->flags().testFlag(Qt::WindowDoesNotAcceptFocus));
        QVERIFY(QMetaObject::invokeMethod(popOut, "trigger"));
        QVERIFY(!video->flags().testFlag(Qt::WindowDoesNotAcceptFocus));
        QVERIFY(QMetaObject::invokeMethod(popOut, "trigger"));
        QVERIFY(video->flags().testFlag(Qt::WindowDoesNotAcceptFocus));
        QTRY_COMPARE(QGuiApplication::focusWindow(), static_cast<QWindow *>(w.window));

        const qsizetype from = w.bridge.sent.size();
        QTest::keyClick(w.window, Qt::Key_E);
        QQuickItem *field = inEdit(w, "editField");
        QVERIFY(field);
        QTRY_VERIFY(field->hasActiveFocus());
        const QString wording = field->property("text").toString();
        const qsizetype clicked = w.bridge.sent.size();
        w.click(w.videoItem("video"));
        QVERIFY(field->hasActiveFocus());
        QTest::keyClick(w.window, 'q');
        QCOMPARE(w.since(clicked), QStringList());
        QCOMPARE(field->property("text").toString(), wording + "q");
        // Ended unsaved, so that the run is left as found.
        QTest::keyClick(w.window, Qt::Key_Escape);
        QCOMPARE(w.since(from), (QStringList{"Edit", "CancelEdit"}));
        QTRY_VERIFY(!w.bridge.editing());
    }

    // Where the video has no window of its own, its pane draws in the main window's scene, at its
    // place in the details panel, and Pop Out is greyed: Ctrl+W quits, as it always does there.
    void withoutAWindowOfItsOwnTheVideoDrawsInTheWindowAndCannotPopOut() {
        Shown w({media()}, {}, {{QStringLiteral("videoWindowed"), false}});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *pane = w.videoItem("videoPane");
        QObject *container = w.find("videoContainer");
        QObject *popOut = w.find("popOutVideo");
        QVERIFY(pane && container && popOut && w.videoWindow());
        QCOMPARE(pane->window(), w.window);
        QCOMPARE(pane->parentItem(), w.item("videoSlot"));
        QVERIFY(w.item("video"));
        QCOMPARE(container->property("window").value<QWindow *>(), nullptr);
        QVERIFY(!w.videoWindow()->isVisible());
        QVERIFY(!popOut->property("enabled").toBool());
        QTest::keyClick(w.window, Qt::Key_W, Qt::ControlModifier);
        QCOMPARE(w.bridge.sent, (QStringList{"Interrupt", "Interrupt"}));
        QTRY_COMPARE(finished.count(), 1);
    }

    // However many reviews the store holds, they take a third of the sidebar at most, and Show and
    // the episodes stay in view below them.
    void aLongListOfReviewsLeavesShowAndTheEpisodesInView() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("episode1"));
        QQuickItem *sidebar = w.item("sidebar");
        QQuickItem *reviews = w.item("reviews");
        QVERIFY(sidebar && reviews);
        // Shorter than its third, the list takes only the height of its entries.
        QTRY_VERIFY(reviews->property("contentHeight").toReal() > 0);
        QTRY_COMPARE(reviews->height(), reviews->property("contentHeight").toReal());
        QVERIFY(reviews->height() < sidebar->height() / 3);
        w.bridge.folders()->set(manyReviews());
        // Taller than the third it has, so that the list fills that third and no more.
        QTRY_VERIFY(reviews->property("contentHeight").toReal() > sidebar->height() / 3);
        QTRY_VERIFY(qAbs(reviews->height() - sidebar->height() / 3) < 1);
        const auto inView = [sidebar](QQuickItem *of) {
            return of && of->mapToItem(sidebar, QPointF(0, of->height())).y() <= sidebar->height();
        };
        QTRY_VERIFY(inView(w.item("filter3")));
        QTRY_VERIFY(inView(w.item("episode1")));
    }

    // At the window's least height the episodes keep room for three entries, which the reviews
    // give up, and everything under the reviews stays inside the sidebar.
    void aShortWindowKeepsRoomForTheEpisodes() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.item("episode1"));
        w.bridge.folders()->set(manyReviews());
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->height(), 480);
        QQuickItem *sidebar = w.item("sidebar");
        QQuickItem *reviews = w.item("reviews");
        QQuickItem *entry = w.item("checkAFolder");
        QVERIFY(sidebar && reviews && entry);
        QQuickItem *episodes = Shown::item(sidebar, "episodes");
        QVERIFY(episodes);
        // Read once the sidebar has laid itself out at the new height, with the long list in it:
        // the episodes then end where the sidebar does.
        QTRY_VERIFY(sidebar->height() < w.window->height());
        QTRY_VERIFY(reviews->property("contentHeight").toReal() > sidebar->height() / 3);
        const auto end = [sidebar, episodes] {
            return episodes->mapToItem(sidebar, QPointF(0, episodes->height())).y();
        };
        QTRY_VERIFY(qAbs(end() - sidebar->height()) < 1);
        QVERIFY(episodes->height() >= 3 * entry->height());
        QVERIFY(reviews->height() < sidebar->height() / 3);
        QVERIFY(reviews->height() > 0);
    }

    // A row draws its whole line while all of its change ends inside the row, and the line cut
    // before its change once it would not, following the list's width as a column is dragged: the
    // change is in view at each width. The line is made up, in place of the first row's, with one
    // long word first so that its cut line fits the list's least width.
    void aRowsLineIsCutOnlyWhenTheWholeLineWouldHideItsChange() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        const QString words = QString("x").repeated(35) + " bb ";
        const auto at = static_cast<uint64_t>(words.size());
        const QString original = words + "befor it";
        const QString wording = words + "before it";
        const subcheck_ffi::Span was{at, at + 5};
        const subcheck_ffi::Span now{at, at + 6};
        FindingsModel *findings = w.bridge.findings();
        const std::optional<subcheck_ffi::EntryKey> key = findings->keyAt(0);
        QVERIFY(key);
        const auto firstRowSays = [&](const QString &said, const QString &fixed) {
            return findings->apply(subcheck_ffi::Splice{0,
                                                        1,
                                                        {subcheck_ffi::EntryView{
                                                            .key = *key,
                                                            .state = subcheck_ffi::State::kPending,
                                                            .is_member = false,
                                                            .expanded = false,
                                                            .label = "E01",
                                                            .shown = 1,
                                                            .total = 1,
                                                            .at_ms = 1000,
                                                            .original = said.toStdString(),
                                                            .wording = fixed.toStdString(),
                                                            .style = "Default",
                                                            .marks = {},
                                                            .track = std::nullopt,
                                                            .fixes = {},
                                                            .checking = false,
                                                            .note = std::nullopt,
                                                            .mixed = false,
                                                            .check = subcheck_ffi::Check::kLlm,
                                                            .original_changed = was,
                                                            .wording_changed = now,
                                                            .reason = "befor for before",
                                                            .file = "/m/S1/E01.mkv",
                                                            .editable = true,
                                                        }}});
        };
        QVERIFY(firstRowSays(original, wording));
        const RowLine forms = rowLine(original, was, wording, now);
        QTRY_VERIFY(w.delegate(0));
        QQuickItem *line = Shown::item(w.delegate(0), "rowLine");
        QQuickItem *reach = Shown::item(w.delegate(0), "rowReach");
        QQuickItem *list = w.item("findings");
        QQuickItem *details = w.item("details");
        QVERIFY(line && reach && list && details);
        QVERIFY(!reach->isVisible());

        QTRY_COMPARE(line->property("text").toString(), forms.whole);
        QVERIFY(reach->implicitWidth() > 0 && reach->implicitWidth() <= line->width());

        w.drag(list, details, -340);
        QTRY_COMPARE(line->property("text").toString(), forms.cut);
        QVERIFY(reach->implicitWidth() > line->width());
        QVERIFY(!line->property("truncated").toBool());

        w.drag(list, details, 340);
        QTRY_COMPARE(line->property("text").toString(), forms.whole);
        QVERIFY(reach->implicitWidth() <= line->width());

        // A line too long for its row stays whole while its change is in view: a row measures its
        // change, not its line, and only the row's end cuts the words after it.
        const QString tail = QString(" and the words go on").repeated(10);
        QVERIFY(firstRowSays(original + tail, wording + tail));
        const QString wholeLine = rowLine(original + tail, was, wording + tail, now).whole;
        const auto lineNow = [&w]() -> QQuickItem * {
            QQuickItem *row = w.delegate(0);
            return row ? Shown::item(row, "rowLine") : nullptr;
        };
        QTRY_VERIFY(lineNow() && lineNow()->property("text").toString() == wholeLine);
        QTRY_VERIFY(lineNow() && lineNow()->property("truncated").toBool());
    }

    // What review names is drawn as it is, so that a word in angle brackets is a word: the heading
    // over an episode's log, and a review's path in its tooltip.
    void whatReviewNamesIsDrawnAsPlainText() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_VERIFY(w.item("review0"));
        QQuickItem *heading = w.item("episodeLogHeading");
        QVERIFY(heading);
        QCOMPARE(heading->property("textFormat").toInt(), int(Qt::PlainText));
        QObject *tip = w.item("review0")->findChild<QObject *>("reviewPath");
        QVERIFY(tip);
        auto *content = tip->property("contentItem").value<QQuickItem *>();
        QVERIFY(content);
        QCOMPARE(content->property("textFormat").toInt(), int(Qt::PlainText));
    }

    // At its least width the window keeps each column's least: the list's gives way before the
    // details panel's.
    void aNarrowWindowKeepsEachColumnsLeastWidth() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->width(), 762);
        QQuickItem *sidebar = w.item("sidebar");
        QQuickItem *list = w.item("findings");
        QQuickItem *details = w.item("details");
        QTRY_COMPARE(w.item("columns")->width(), 762);
        QTRY_VERIFY(sidebar->width() <= 160);
        QVERIFY(list->width() <= 280);
        QVERIFY(list->width() >= 260);
        QVERIFY(details->width() >= 340);
        QVERIFY(sidebar->width() >= 160);
        const auto right = [](QQuickItem *of) {
            return of->mapToScene(QPointF(of->width(), 0)).x();
        };
        QVERIFY(right(details) <= w.window->width() + 0.5);
    }

    // The window opens at its default size, the sidebar and the details panel at theirs and the
    // list taking what is left; the size and the columns' widths a person leaves are those the next
    // window starts with, from the same settings.
    void theSizeAndTheColumnsStickAcrossARestart() {
        QTemporaryDir kept;
        QVERIFY(kept.isValid());
        {
            Shown w({media()}, kept.path());
            QVERIFY(w.start());
            QTRY_COMPARE(w.rows(), 2);
            QQuickItem *sidebar = w.item("sidebar");
            QQuickItem *list = w.item("findings");
            QQuickItem *details = w.item("details");
            QVERIFY(sidebar && list && details);
            QCOMPARE(w.window->size(), QSize(1280, 800));
            QTRY_COMPARE(qRound(sidebar->width()), 220);
            QCOMPARE(qRound(details->width()), 480);
            QCOMPARE(qRound(list->width()), 578);
            w.window->resize(1100, 700);
            QTRY_COMPARE(qRound(list->width()), 398);
            w.drag(sidebar, list, 30);
            QTRY_COMPARE(qRound(sidebar->width()), 250);
            w.drag(list, details, 40);
            QTRY_COMPARE(qRound(details->width()), 440);
            QCOMPARE(w.settings.sidebarWidth(), 250);
            QCOMPARE(w.settings.detailsWidth(), 440);
        }
        Shown w({media()}, kept.path());
        QVERIFY(w.start());
        QCOMPARE(w.window->size(), QSize(1100, 700));
        QTRY_COMPARE(qRound(w.item("sidebar")->width()), 250);
        QCOMPARE(qRound(w.item("details")->width()), 440);
    }

    // View ▸ Rows ▸ chooses the row style, and so does the menu the list's heading opens under its
    // Rows button, with the same three items: each draws every row in its style at once, is the
    // one checked, and is kept, so that the next start draws its rows in it from its first frame.
    void theRowStyleIsChosenFromTheViewMenuOrTheListsHeadingAndSticksAcrossARestart() {
        QTemporaryDir kept;
        QVERIFY(kept.isValid());
        const std::vector<std::pair<const char *, QString>> styles = {
            {"rowsCompact", "compact"}, {"rowsTwoLines", "twoLines"}, {"rowsRich", "rich"}};
        {
            Shown w({media()}, kept.path());
            QVERIFY(w.start());
            QTRY_COMPARE(w.rows(), 2);
            QCOMPARE(checkedRowStyles(w), QStringList{"rowsTwoLines"});
            for (const auto &[name, style] : styles) {
                QVERIFY(QMetaObject::invokeMethod(w.find(name), "trigger"));
                QCOMPARE(w.settings.rowStyle(), style);
                QTRY_COMPARE(drawnStyles(w), (QStringList{style, style}));
                QCOMPARE(checkedRowStyles(w), QStringList{name});
            }

            QObject *menu = w.find("rowsMenu");
            QQuickItem *button = w.item("rowStyleButton");
            QVERIFY(menu && button);
            QCOMPARE(button->property("text").toString(), "Rows");
            QCOMPARE(menu->property("count").toInt(), 3);
            // From Rich, each other style in turn, Compact last.
            for (const int at : {1, 2, 0}) {
                w.click(button);
                QTRY_VERIFY(menu->property("opened").toBool());
                auto *drawn = menu->property("background").value<QQuickItem *>();
                const QRectF opened =
                    drawn->mapRectToScene(QRectF(0, 0, drawn->width(), drawn->height()));
                const QRectF under =
                    button->mapRectToScene(QRectF(0, 0, button->width(), button->height()));
                QVERIFY2(
                    qAbs(opened.top() - under.bottom()) <= 1 && opened.left() < under.right() &&
                        opened.right() > under.left(),
                    qPrintable(QStringLiteral("the menu at %1, %2 to %3; the button %4, %5 to %6")
                                   .arg(opened.top())
                                   .arg(opened.left())
                                   .arg(opened.right())
                                   .arg(under.bottom())
                                   .arg(under.left())
                                   .arg(under.right())));
                QQuickItem *item = nullptr;
                QVERIFY(QMetaObject::invokeMethod(menu, "itemAt", Q_RETURN_ARG(QQuickItem *, item),
                                                  Q_ARG(int, at)));
                QVERIFY(item);
                const auto &[name, style] = styles[at];
                QCOMPARE(item->property("text").toString(),
                         w.find(name)->property("text").toString());
                w.click(item);
                QTRY_VERIFY(!menu->property("visible").toBool());
                QCOMPARE(w.settings.rowStyle(), style);
                QTRY_COMPARE(drawnStyles(w), (QStringList{style, style}));
                QCOMPARE(checkedRowStyles(w), QStringList{name});
            }
        }
        // Every row the next window draws, from its first frame on; made before the window, so
        // that it outlives every frame.
        QStringList seen;
        Shown w({media()}, kept.path());
        QObject::connect(w.window, &QQuickWindow::afterAnimating, w.window, [&seen, &w] {
            const QStringList now = drawnStyles(w);
            for (const QString &style : now) {
                if (!seen.contains(style)) {
                    seen << style;
                }
            }
        });
        QCOMPARE(checkedRowStyles(w), QStringList{"rowsCompact"});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(!seen.isEmpty());
        QCOMPARE(seen, QStringList{"compact"});
    }

    // A drag of one column's edge at a narrow window keeps that column's width and leaves the
    // other's as the person last chose it, though the window squeezes it for now: widened again,
    // each column has its own.
    void aDragAtANarrowWindowLeavesTheOtherColumnsKeptWidth() {
        for (const bool bySidebar : {true, false}) {
            Shown w({media()});
            QVERIFY(w.start());
            QTRY_COMPARE(w.rows(), 2);
            QQuickItem *sidebar = w.item("sidebar");
            QQuickItem *list = w.item("findings");
            QQuickItem *details = w.item("details");
            QVERIFY(sidebar && list && details);
            QTRY_COMPARE(qRound(details->width()), 480);
            // The list gives way first, then the details panel, then the sidebar: at 800 the
            // sidebar is squeezed only once the details panel is at its least width, which leaves
            // no room for the details drag. At 830 the details panel is squeezed but has room to
            // give.
            w.window->resize(bySidebar ? 800 : 830, 800);
            QTRY_COMPARE(qRound(list->width()), 260);
            QTRY_VERIFY(bySidebar ? sidebar->width() < 220 : qRound(sidebar->width()) == 220);
            QTRY_VERIFY(details->width() < 480);
            if (bySidebar) {
                const int dragged = qRound(sidebar->width()) - 10;
                w.drag(sidebar, list, -10);
                QTRY_COMPARE(qRound(sidebar->width()), dragged);
                QCOMPARE(w.settings.sidebarWidth(), dragged);
                QCOMPARE(w.settings.detailsWidth(), 480);
                w.window->resize(1280, 800);
                QTRY_COMPARE(qRound(details->width()), 480);
                QCOMPARE(qRound(sidebar->width()), dragged);
            } else {
                const int dragged = qRound(details->width()) - 5;
                w.drag(list, details, 5);
                QTRY_COMPARE(qRound(details->width()), dragged);
                QCOMPARE(w.settings.detailsWidth(), dragged);
                QCOMPARE(w.settings.sidebarWidth(), 220);
                w.window->resize(1280, 800);
                QTRY_COMPARE(qRound(sidebar->width()), 220);
                QCOMPARE(qRound(details->width()), dragged);
            }
        }
    }

    // The pane holds its player weakly: the same player given again keeps the pane, its command
    // under way still waiting; a player given in another's place takes the pane from it, and the
    // one before lets go, its command under way run again with no mpv, as if it came now; a
    // player that goes first leaves the pane none. Offscreen, the pane's mpv never says it is
    // ready, so the test says so for it, and the pane's mpv has not answered when the player goes.
    void aPaneLetsGoOfAnEarlierPlayerAndOutlivesItsOwn() {
        QQmlEngine engine;
        QQmlComponent component(&engine);
        component.setData("import Subcheck; MpvItem {}", QUrl());
        std::unique_ptr<QObject> made(component.create());
        auto *pane = qobject_cast<MpvItem *>(made.get());
        QVERIFY2(pane, qPrintable(component.errorString()));
        PlayerPort first(std::chrono::seconds(10));
        auto second = std::make_unique<PlayerPort>(std::chrono::seconds(10));
        QStringList reports;
        connect(&first, &PlayerPort::reported, this,
                [&reports](const subcheck_ffi::PlayerReport &r) { reports << said(r); });
        pane->setPlayer(&first);
        first.ready();
        using subcheck_ffi::Playback;
        using subcheck_ffi::PlayerCommand;
        first.run(PlayerCommand(PlayerCommand::kPlayback{Playback(Playback::kSeek{5})}));
        QCOMPARE(reports, QStringList());
        pane->setPlayer(&first);
        QCOMPARE(reports, QStringList());
        pane->setPlayer(second.get());
        QCOMPARE(reports, QStringList{"play failed: no file"});
        QCOMPARE(pane->player(), second.get());
        second.reset();
        QCOMPARE(pane->player(), nullptr);
    }

    void anEmptyRunSaysSo() {
        Shown w({clean()});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QVERIFY(w.start());
        QObject *empty = w.find("empty");
        QVERIFY(empty);
        QTRY_VERIFY(empty->property("visible").toBool());
        QCOMPARE(empty->property("text").toString(), "No findings in this run");
        QVERIFY(!w.bridge.detail()->shown());
        QTest::keyClick(w.window, Qt::Key_Q);
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(w.out.str(), "clean: 0 accepted, 0 rejected, 0 pending\n");
    }

    // A filter that hides every finding is told from a run with none, and the words go with the
    // filter that hid them.
    void aFilterThatHidesEveryFindingSaysSo() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *empty = w.find("empty");
        QVERIFY(empty);
        QVERIFY(!empty->property("visible").toBool());
        w.bridge.setFilter(3);
        QTRY_VERIFY(empty->property("visible").toBool());
        QCOMPARE(empty->property("text").toString(), "No findings under this filter");
        QCOMPARE(w.rows(), 0);
        w.bridge.setFilter(0);
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(!empty->property("visible").toBool());
    }

    // While a video shows, its corner says whether it plays or is paused, as mpv says; it takes no
    // click. The pane's first word from mpv is waited for, by a change of mpv's own, so that it
    // cannot land on what the test says for mpv after it.
    void theIndicatorSaysWhetherTheVideoPlaysWhileOneShows() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        auto *video = qobject_cast<MpvItem *>(w.videoItem("video"));
        QQuickItem *indicator = w.videoItem("playIndicator");
        QVERIFY(video && indicator);
        QVERIFY(!indicator->isVisible());
        video->MpvAbstractItem::setProperty(QStringLiteral("pause"), true);
        QTRY_VERIFY(video->paused());

        video->heard(QStringLiteral("vo-configured"), true);
        QVERIFY(video->showing());
        QVERIFY(indicator->isVisible());
        QCOMPARE(indicator->mapToItem(video, QPointF(indicator->width(), 0)),
                 QPointF(video->width() - 8, 8));
        for (const bool paused : {true, false, true}) {
            video->heard(QStringLiteral("pause"), paused);
            QCOMPARE(video->paused(), paused);
            QCOMPARE(Shown::item(indicator, "pausedBars")->isVisible(), paused);
            QCOMPARE(Shown::item(indicator, "playingTriangle")->isVisible(), !paused);
            QCOMPARE(indicator->property("said").toString(), paused ? "Paused" : "Playing");
        }
        QCOMPARE(indicator->acceptedMouseButtons(), Qt::NoButton);
        const qsizetype from = w.bridge.sent.size();
        w.click(indicator);
        QCOMPARE(w.since(from), QStringList());

        video->heard(QStringLiteral("vo-configured"), false);
        QVERIFY(!video->showing());
        QVERIFY(!indicator->isVisible());
        // As mpv makes its video output again for the next file.
        video->heard(QStringLiteral("vo-configured"), true);
        QVERIFY(indicator->isVisible());
    }

    // While Show in Video waits, a busy animation is centred on the video, once the wait has lasted
    // 500 ms, so that a quick open shows none; on a backing of the window's colour, which the
    // style's own ink reads on in any palette, as the black of the pane does not in a light one.
    // Neither takes a click, and a screen reader names the animation. Close Video ends both, and
    // so does review's end, each once the 100 ms are up that keep a load which ends as the next
    // starts from blinking. Offscreen, mpv never becomes ready, so the Open waits for the pane as
    // long as the test needs.
    void theLoadingAnimationShowsWhileAShowInVideoWaits() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *video = w.videoItem("video");
        QQuickItem *loading = w.videoItem("videoLoading");
        QQuickItem *backing = w.videoItem("videoLoadingBacking");
        QVERIFY(video && loading && backing);
        QVERIFY(!loading->property("running").toBool());
        QVERIFY(!loading->isVisible());
        QVERIFY(!backing->isVisible());
        // Whether it runs as the player takes the Open; and when it starts to, on a clock started
        // before the key, so that the time is never short of the wait's own.
        QElapsedTimer clock;
        std::optional<bool> atOnce;
        qint64 shownAt = -1;
        QObject watching;
        connect(&w.player, &PlayerPort::openingChanged, &watching, [&] {
            if (w.player.opening() && !atOnce) {
                atOnce = loading->property("running").toBool();
            }
        });
        connect(&w.bridge, &SessionBridge::videoLoadingChanged, &watching, [&] {
            if (w.bridge.videoLoading() && shownAt < 0) {
                shownAt = clock.elapsed();
            }
        });
        clock.start();
        QTest::keyClick(w.window, Qt::Key_O);
        QTRY_VERIFY(atOnce.has_value());
        QVERIFY(!*atOnce);
        QTRY_VERIFY(loading->property("running").toBool());
        QVERIFY2(shownAt >= 500, qPrintable(QString::number(shownAt)));
        QVERIFY(loading->isVisible());
        QVERIFY(backing->isVisible());
        // Each within the half pixel that centring rounds to, so that it draws sharp.
        for (QQuickItem *each : {loading, backing}) {
            const QPointF off =
                each->mapToItem(video, QPointF(each->width() / 2, each->height() / 2)) -
                QPointF(video->width() / 2, video->height() / 2);
            QVERIFY2(qAbs(off.x()) <= 0.5 && qAbs(off.y()) <= 0.5,
                     qPrintable(QStringLiteral("%1 is off by %2, %3")
                                    .arg(each->objectName())
                                    .arg(off.x())
                                    .arg(off.y())));
        }
        // The animation wholly on its backing, with room around it.
        const QRectF on = loading->mapRectToItem(backing, loading->boundingRect());
        QVERIFY2(on.left() > 0 && on.top() > 0 && on.right() < backing->width() &&
                     on.bottom() < backing->height(),
                 qPrintable(QStringLiteral("%1, %2 %3x%4 on %5x%6")
                                .arg(on.x())
                                .arg(on.y())
                                .arg(on.width())
                                .arg(on.height())
                                .arg(backing->width())
                                .arg(backing->height())));
        // The window palette's colour, mostly opaque, so that what is behind shows through too
        // little to take the ink's contrast.
        const QColor window = w.item("details")
                                  ->property("palette")
                                  .value<QObject *>()
                                  ->property("window")
                                  .value<QColor>();
        const QColor drawn = backing->property("color").value<QColor>();
        QCOMPARE(drawn.rgb(), window.rgb());
        QVERIFY2(drawn.alphaF() >= 0.8, qPrintable(QString::number(drawn.alphaF())));
        QAccessibleInterface *named = QAccessible::queryAccessibleInterface(loading);
        QVERIFY(named);
        QCOMPARE(named->text(QAccessible::Name), "Loading video");
        QCOMPARE(loading->acceptedMouseButtons(), Qt::NoButton);
        QCOMPARE(backing->acceptedMouseButtons(), Qt::NoButton);
        const qsizetype from = w.bridge.sent.size();
        w.click(loading);
        QCOMPARE(w.since(from), QStringList());

        QTest::keyClick(w.window, Qt::Key_O, Qt::ShiftModifier);
        QTRY_COMPARE(w.bridge.statusLine(), "Video closed");
        QTRY_VERIFY(!loading->property("running").toBool());
        QVERIFY(!loading->isVisible());
        QVERIFY(!backing->isVisible());

        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QTest::keyClick(w.window, Qt::Key_O);
        QTRY_VERIFY(loading->property("running").toBool());
        QVERIFY(backing->isVisible());
        QTest::keyClick(w.window, Qt::Key_Q);
        QTRY_COMPARE(finished.count(), 1);
        QTRY_VERIFY(!loading->property("running").toBool());
        QVERIFY(!backing->isVisible());
    }

    // Show in Video on a finding whose file has gone opens nothing, so nothing loads: no animation,
    // then or once its moment has passed, and the status bar says why.
    void theLoadingAnimationEndsWhenTheFileIsMissing() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *loading = w.videoItem("videoLoading");
        QVERIFY(loading);
        const QString file = QString::fromStdString(media()) + "/typos - E01.mkv";
        const QString away = file + ".away";
        QVERIFY(QFile::rename(file, away));
        // Back however the test ends: every test leaves the run as it found it.
        const auto restore = qScopeGuard([&file, &away] { QFile::rename(away, file); });
        QSignalSpy changed(&w.bridge, &SessionBridge::videoLoadingChanged);
        QTest::keyClick(w.window, Qt::Key_O);
        QTRY_COMPARE(w.bridge.statusLine(), "The file is missing: " + file);
        QTest::qWait(300);
        QVERIFY(!loading->property("running").toBool());
        QVERIFY(!w.player.opening());
        QCOMPARE(changed.count(), 0);
    }

    // The busy animation is Material's arc in every style, the tests' Fusion among them, as it
    // reads as loading in every style; drawn in the window palette's highlight colour. The control
    // is 48 px across, read at a glance over the video with the frame around it still in view, and
    // its arc inside Material's padding.
    void theLoadingArcIsMaterialsInTheHighlightColour() {
        Shown w({media()});
        QVERIFY(w.show());
        QQuickItem *loading = w.videoItem("videoLoading");
        QVERIFY(loading);
        auto *arc = loading->property("contentItem").value<QQuickItem *>();
        QVERIFY(arc);
        // Material's own class, which the instance in its style's QML derives a class from.
        QVERIFY2(arc->inherits("QQuickMaterialBusyIndicator"), arc->metaObject()->className());
        QCOMPARE(arc->property("color").value<QColor>().name(QColor::HexArgb),
                 paletteColour(w, "highlight").name(QColor::HexArgb));
        QVERIFY2(loading->size() == QSizeF(48, 48),
                 qPrintable(QStringLiteral("the control, whose arc Material draws inside its "
                                           "padding, is %1 by %2, not 48 px")
                                .arg(loading->width())
                                .arg(loading->height())));
    }

    // A row of another height takes the highlight's height at once, as a move takes its place.
    void theHighlightTakesARowsSizeAtOnce() {
        Shown w({media()});
        QVERIFY(w.start());
        QCOMPARE(w.find("findings")->property("highlightResizeDuration").toInt(), 0);
        QCOMPARE(w.find("findings")->property("highlightMoveDuration").toInt(), 0);
    }

    // Each column handle is taken within the columns' margins beside its line, over whatever
    // lies beside it, with the resize cursor, and a press there drags the column and clicks
    // nothing under it.
    void aColumnHandleIsTakenWithinTheMarginsBesideItsLine() {
        QStyleHints *hints = QGuiApplication::styleHints();
        const auto restore = qScopeGuard(
            [hints, was = hints->useHoverEffects()] { hints->setUseHoverEffects(was); });
        hints->setUseHoverEffects(true);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.delegate(0));
        QTRY_VERIFY(w.item("episode0"));
        QList<QQuickItem *> handles;
        const QList<QQuickItem *> parts = w.item("columns")->childItems();
        for (QQuickItem *part : parts) {
            if (part->objectName() == QLatin1String("columnHandle")) {
                handles << part;
            }
        }
        QCOMPARE(handles.size(), 2);
        std::sort(handles.begin(), handles.end(),
                  [](QQuickItem *a, QQuickItem *b) { return a->x() < b->x(); });
        const auto lineOf = [](QQuickItem *handle) {
            return qRound(handle->mapToScene(QPointF(handle->width() / 2, 0)).x());
        };
        const int entry = w.middle(w.item("episode0")).y();
        const int row = w.middle(w.delegate(0)).y();
        // The video's middle, in the main window, which holds the video's window at its place.
        QQuickItem *drawn = w.videoItem("video");
        QVERIFY(drawn);
        const int video = qRound(w.window
                                     ->mapFromGlobal(drawn->mapToGlobal(
                                         QPointF(drawn->width() / 2, drawn->height() / 2)))
                                     .y());
        // The grip stays within the columns' 8 px margins, but for 3 px over the sidebar's
        // scroll bars on the handle beside the sidebar. Offsets are from lineOf, which rounds the
        // line's centre up to the pixel after the handle's x (L = x + 1): the grip spans
        // [x - reach, x + 9), so the last pixel in is L + 7 on the right and L - reach - 1 on
        // the left, and the first pixel out is one past each.
        struct Place {
            QQuickItem *handle;
            int y;
            int in[2];
            int out[2];
        };
        const std::vector<Place> places{{handles[0], entry, {-4, 7}, {-5, 8}},
                                        {handles[0], row, {-4, 7}, {-5, 8}},
                                        {handles[1], row, {-9, 7}, {-10, 8}},
                                        {handles[1], video, {-9, 7}, {-10, 8}}};
        for (const Place &p : places) {
            for (const int off : p.in) {
                QTest::mouseMove(w.window, QPoint(lineOf(p.handle) + off, p.y));
                QTRY_VERIFY(p.handle->property("active").toBool());
                QTRY_COMPARE(w.window->cursor().shape(), Qt::SplitHCursor);
            }
            for (const int off : p.out) {
                QTest::mouseMove(w.window, QPoint(lineOf(p.handle) + off, p.y));
                QTRY_VERIFY(!p.handle->property("active").toBool());
            }
        }

        QQuickItem *sidebar = w.item("sidebar");
        const qreal before = sidebar->width();
        const qsizetype from = w.bridge.sent.size();
        const QPoint at(lineOf(handles[0]) - 2, entry);
        QTest::mouseMove(w.window, at);
        QTest::mousePress(w.window, Qt::LeftButton, {}, at);
        QTest::mouseMove(w.window, at + QPoint(15, 0));
        QTest::mouseMove(w.window, at + QPoint(45, 0));
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, at + QPoint(45, 0));
        QTRY_COMPARE(sidebar->width(), before + 45);
        QCOMPARE(w.since(from), QStringList());

        // Over the video too, on the handle's other side.
        QQuickItem *details = w.item("details");
        const qreal wide = details->width();
        const QPoint onVideo(lineOf(handles[1]) + 7, video);
        QTest::mouseMove(w.window, onVideo);
        QTest::mousePress(w.window, Qt::LeftButton, {}, onVideo);
        QTest::mouseMove(w.window, onVideo + QPoint(15, 0));
        QTest::mouseMove(w.window, onVideo + QPoint(45, 0));
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, onVideo + QPoint(45, 0));
        QTRY_COMPARE(details->width(), wide - 45);
        QCOMPARE(w.since(from), QStringList());

        // And over a finding row, on the first handle's right side.
        const qreal grown = sidebar->width();
        const QPoint onRow(lineOf(handles[0]) + 7, row);
        QTest::mouseMove(w.window, onRow);
        QTest::mousePress(w.window, Qt::LeftButton, {}, onRow);
        QTest::mouseMove(w.window, onRow + QPoint(15, 0));
        QTest::mouseMove(w.window, onRow + QPoint(45, 0));
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, onRow + QPoint(45, 0));
        QTRY_COMPARE(sidebar->width(), grown + 45);
        QCOMPARE(w.since(from), QStringList());
    }

    // The grip leaves the sidebar's scroll bars to them: dragging a list's own bar scrolls it and
    // does not move the column.
    void aSidebarScrollBarIsNotTakenByTheColumnHandle() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.bridge.folders()->set(manyReviews());
        QQuickItem *sidebar = w.item("sidebar");
        QQuickItem *reviews = w.item("reviews");
        QVERIFY(sidebar && reviews);
        QTRY_VERIFY(reviews->property("contentHeight").toReal() > reviews->height() + 1);
        QTRY_VERIFY(qAbs(reviews->height() - sidebar->height() / 3) < 1);
        const qreal before = sidebar->width();
        QCOMPARE(reviews->property("contentY").toReal(), 0.0);
        // The bar's thumb, at the top of the right edge of the list.
        const qreal thumb =
            reviews->height() * reviews->height() / reviews->property("contentHeight").toReal();
        const QPointF top = reviews->mapToScene(QPointF(reviews->width() - 4, thumb / 2));
        const QPoint at = top.toPoint();
        QTest::mouseMove(w.window, at);
        QTest::mousePress(w.window, Qt::LeftButton, {}, at);
        QTest::mouseMove(w.window, at + QPoint(0, 10));
        QTest::mouseMove(w.window, at + QPoint(0, 30));
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, at + QPoint(0, 30));
        QTRY_VERIFY(reviews->property("contentY").toReal() > 0);
        QCOMPARE(sidebar->width(), before);
    }

    // A column dragged stretches the video's frame across its changing size until the drag ends,
    // rather than have mpv draw it again at each step. The window's edge dragged, which resizes
    // the window alone, stretches nothing: mpv draws the frame at each size.
    void aColumnDragStretchesTheVideoAndAnEdgeDragDoesNot() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickItem *list = w.item("findings");
        QQuickItem *details = w.item("details");
        auto *video = qobject_cast<MpvItem *>(w.videoItem("video"));
        QVERIFY(list && details && video);
        QVERIFY(!video->stretching());
        QVERIFY(video->textureFollowsItemSize());
        const qreal wide = details->width();
        const QPoint at = w.handle(list, details);
        QTest::mousePress(w.window, Qt::LeftButton, {}, at);
        QTest::mouseMove(w.window, at + QPoint(-15, 0));
        QTest::mouseMove(w.window, at + QPoint(-45, 0));
        QTRY_VERIFY(video->stretching());
        QVERIFY(!video->textureFollowsItemSize());
        QTest::mouseRelease(w.window, Qt::LeftButton, {}, at + QPoint(-45, 0));
        QTRY_VERIFY(!video->stretching());
        QVERIFY(video->textureFollowsItemSize());
        QTRY_COMPARE(details->width(), wide + 45);

        // Narrow enough that the details panel, and the video with it, gives way too.
        QSignalSpy changed(video, &MpvItem::stretchingChanged);
        w.window->resize(830, 800);
        QTRY_VERIFY(video->width() < wide);
        QVERIFY(!video->stretching());
        QCOMPARE(changed.count(), 0);
    }

    // A line across the window, under the menu bar and above the toolbar's words.
    void aLineSitsBetweenTheMenuBarAndTheToolbar() {
        Shown w({media()});
        QVERIFY(w.start());
        QQuickItem *line = w.item("menuLine");
        auto *bar = w.window->property("menuBar").value<QQuickItem *>();
        QVERIFY(line && bar);
        QVERIFY(line->isVisible());
        QCOMPARE(line->height(), 1.0);
        QCOMPARE(line->width(), qreal(w.window->width()));
        QCOMPARE(line->mapToScene(QPointF(0, 0)).y(), bar->height());
        QVERIFY(w.item("title")->mapToScene(QPointF(0, 0)).y() >= bar->height() + 1);
        // The line follows the bar: with none drawn there is no line.
        bar->setVisible(false);
        QVERIFY(!line->isVisible());
        bar->setVisible(true);
        QVERIFY(line->isVisible());
    }

    // The details panel stacks the line as it is now above the line with the fix, each a block: a
    // bar 3 px wide down its left in its side's tone, its right corners rounded, on the style's
    // alternate background; a label in bold capitals in the same tone, 2 pt below the panel's text;
    // and the line, 2 pt above it. An edited finding's block says the wording is the person's.
    void theDetailsStackTheOldLineAboveTheNewAsBlocks() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        DetailView *detail = w.bridge.detail();
        QTRY_VERIFY(detail->shown());
        QQuickItem *panel = w.item("details");
        QQuickItem *now = Shown::item(panel, "nowBlock");
        QQuickItem *fixed = Shown::item(panel, "fixedBlock");
        QVERIFY(now && fixed);
        const auto top = [](QQuickItem *of) { return of->mapToScene(QPointF(0, 0)).y(); };
        QTRY_VERIFY(now->height() > 0 && top(now) + now->height() < top(fixed));
        QCOMPARE(now->mapToScene(QPointF(0, 0)).x(), fixed->mapToScene(QPointF(0, 0)).x());
        QCOMPARE(now->width(), fixed->width());
        const qreal size = panel->property("font").value<QFont>().pointSizeF();
        const QColor alternate = panel->property("palette")
                                     .value<QObject *>()
                                     ->property("alternateBase")
                                     .value<QColor>();
        const std::vector<std::tuple<QQuickItem *, QColor, QString>> blocks = {
            {now, detail->oldTone(), "− In the subtitles now"},
            {fixed, detail->newTone(), "+ With the fix"}};
        QVERIFY(detail->oldTone() != detail->newTone());
        for (const auto &[block, tone, words] : blocks) {
            QQuickItem *bar = Shown::item(block, "blockBar");
            QQuickItem *label = Shown::item(block, "blockLabel");
            QQuickItem *line = Shown::item(block, "blockLine");
            QVERIFY(bar && label && line);
            QCOMPARE(bar->width(), 3.0);
            QCOMPARE(bar->height(), block->height());
            QCOMPARE(bar->mapToItem(block, QPointF(0, 0)), QPointF(0, 0));
            QCOMPARE(bar->property("color").value<QColor>(), tone);
            QVERIFY(label->mapToItem(block, QPointF(0, 0)).x() > bar->width());
            QCOMPARE(label->property("text").toString(), words);
            QCOMPARE(label->property("color").value<QColor>(), tone);
            const QFont labelFont = label->property("font").value<QFont>();
            QVERIFY(labelFont.bold());
            QCOMPARE(labelFont.capitalization(), QFont::AllUppercase);
            QCOMPARE(labelFont.pointSizeF(), size - 2);
            QCOMPARE(line->property("font").value<QFont>().pointSizeF(), size + 2);
            QVERIFY(top(line) > top(label));
            auto *back = block->property("background").value<QQuickItem *>();
            QVERIFY(back);
            QCOMPARE(back->property("color").value<QColor>(), alternate);
            QCOMPARE(back->property("topRightRadius").toReal(), 4.0);
            QCOMPARE(back->property("bottomRightRadius").toReal(), 4.0);
            QCOMPARE(back->property("topLeftRadius").toReal(), 0.0);
            QCOMPARE(back->property("bottomLeftRadius").toReal(), 0.0);
        }
        QCOMPARE(Shown::text(now, "blockLine"), detail->now());
        QCOMPARE(Shown::text(fixed, "blockLine"), detail->fixed());

        subcheck_ffi::Detail edited = everyPartOfAFinding();
        edited.edited = true;
        detail->show(edited);
        QCOMPARE(Shown::text(fixed, "blockLabel"), "+ With your wording");
        QCOMPARE(Shown::text(now, "blockLabel"), "− In the subtitles now");
    }

    // The two blocks keep their place from one finding to the next, so that the eye finds them
    // where it left them: each has room for two lines of its text, at the panel's size and at a
    // larger one, and what sets a finding apart, its banners and its mark, comes after them. A line
    // of three pushes the second block down, clear of the first.
    void theFixBlocksKeepTheirPlaceAcrossFindings_data() {
        QTest::addColumn<bool>("larger");
        QTest::newRow("the platform's size") << false;
        QTest::newRow("24 pt") << true;
    }
    void theFixBlocksKeepTheirPlaceAcrossFindings() {
        QFETCH(bool, larger);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        DetailView *detail = w.bridge.detail();
        QTRY_VERIFY(detail->shown());
        // Review has said what it says of the start, so that none of it replaces a finding shown
        // here.
        QTRY_VERIFY(!w.bridge.episodeLog().isEmpty());
        if (larger) {
            w.settings.setDetailsTextSize(24);
        }
        QQuickItem *panel = w.item("details");
        QQuickItem *now = Shown::item(panel, "nowBlock");
        QQuickItem *fixed = Shown::item(panel, "fixedBlock");
        QQuickItem *mark = Shown::item(panel, "mark");
        QQuickItem *banners = Shown::item(panel, "banners");
        QVERIFY(now && fixed && mark && banners);
        QQuickItem *nowLine = Shown::item(now, "blockLine");
        QQuickItem *fixedLine = Shown::item(fixed, "blockLine");
        QVERIFY(nowLine && fixedLine);

        // `finding` shown and the panel laid out for it, as the window lays out what changed
        // before each frame it draws.
        const auto show = [&w, detail](const subcheck_ffi::Detail &finding) {
            QSignalSpy laidOut(w.window, &QQuickWindow::afterAnimating);
            detail->show(finding);
            return laidOut.wait();
        };
        // `line` and words after it, enough to wrap to `lines` lines of a block's line at its font
        // and width, the last about half full: each line before it wraps at a space, at most a
        // word short of full.
        const auto filling = [nowLine](std::string line, int lines) {
            const QFontMetricsF metrics(nowLine->property("font").value<QFont>());
            while (metrics.horizontalAdvance(QString::fromStdString(line)) <
                   (lines - 0.5) * nowLine->width()) {
                line += " and on";
            }
            return line;
        };
        // Where each block starts in the panel, and how tall it is.
        const auto blocks = [&w, now, fixed] {
            return QList<qreal>{topInDetails(w, now), now->height(), topInDetails(w, fixed),
                                fixed->height()};
        };
        const auto bottom = [&w](QQuickItem *item) {
            return topInDetails(w, item) + item->height();
        };

        subcheck_ffi::Detail plain = everyPartOfAFinding();
        plain.note = std::nullopt;
        plain.checking = std::nullopt;
        plain.truncated = std::nullopt;
        plain.mark = std::nullopt;
        QVERIFY(show(plain));
        QVERIFY(nowLine->implicitWidth() <= nowLine->width());
        QVERIFY(fixedLine->implicitWidth() <= fixedLine->width());
        QVERIFY(!mark->isVisible() && !banners->isVisible());
        const QList<qreal> onOneLine = blocks();
        // How many lines a block's line wraps to, from how tall its text is: Text keeps no count
        // of a rich text's lines.
        const qreal oneLine = nowLine->property("contentHeight").toReal();
        QCOMPARE(fixedLine->property("contentHeight").toReal(), oneLine);
        const auto lineCount = [oneLine](QQuickItem *line) {
            return qRound(line->property("contentHeight").toReal() / oneLine);
        };

        subcheck_ffi::Detail twoLines = plain;
        twoLines.original = filling(plain.original, 2);
        twoLines.wording = filling(plain.wording, 2);
        QVERIFY(show(twoLines));
        QCOMPARE(lineCount(nowLine), 2);
        QCOMPARE(lineCount(fixedLine), 2);
        QCOMPARE(blocks(), onOneLine);

        subcheck_ffi::Detail marked = plain;
        marked.mark = subcheck_ffi::Mark(subcheck_ffi::Mark::kApplied{});
        subcheck_ffi::Detail bannered = plain;
        bannered.note = subcheck_ffi::FileNote::kChanged;
        for (const auto &[finding, apart] :
             {std::pair{marked, mark}, std::pair{bannered, banners}}) {
            QVERIFY(show(finding));
            QVERIFY(apart->isVisible());
            QCOMPARE(blocks(), onOneLine);
            QVERIFY2(topInDetails(w, apart) >= bottom(fixed),
                     qPrintable(QStringLiteral("%1 starts at %2, the fix's block ends at %3")
                                    .arg(apart->objectName())
                                    .arg(topInDetails(w, apart))
                                    .arg(bottom(fixed))));
        }

        subcheck_ffi::Detail threeLines = plain;
        threeLines.original = filling(plain.original, 3);
        QVERIFY(show(threeLines));
        QCOMPARE(lineCount(nowLine), 3);
        QCOMPARE(topInDetails(w, now), onOneLine[0]);
        QVERIFY(topInDetails(w, fixed) > onOneLine[2]);
        QVERIFY(bottom(now) < topInDetails(w, fixed));
    }

    // A palette chosen redraws the rows' coloured parts in each row style, Compact's glyph and
    // change, Two Lines' states, badges and lines, and Rich's tile lines too, and the details'
    // state, badge, blocks and marks, at once, in the palette's tones; and moves nothing: the
    // selected row, the list's scroll and the panel's stay, and review is asked nothing.
    void aPaletteChosenRedrawsTheRowsAndTheDetailsInPlace_data() {
        QTest::addColumn<QString>("style");
        for (const char *style : {"compact", "twoLines", "rich"}) {
            QTest::newRow(style) << QString::fromLatin1(style);
        }
    }
    void aPaletteChosenRedrawsTheRowsAndTheDetailsInPlace() {
        QFETCH(QString, style);
        const auto restore = qScopeGuard([] { useColours(Colours::Default); });
        Shown w({media()});
        w.settings.setRowStyle(style);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->height(), 480);
        QTest::keyClick(w.window, Qt::Key_J);
        QTRY_COMPARE(w.bridge.selected(), 1);
        // Rows made up after review's two: enough to scroll the list at these sizes in any font,
        // with room to spare.
        FindingsModel *findings = w.bridge.findings();
        std::vector<subcheck_ffi::EntryView> more;
        for (uint64_t id = 100; id < 160; ++id) {
            more.push_back(rowOfTwelve(subcheck_ffi::EntryKey(subcheck_ffi::EntryKey::kRow{{id}})));
        }
        QVERIFY(findings->apply(subcheck_ffi::Splice{2, 0, more}));
        auto *list = qobject_cast<QQuickItem *>(w.find("findings"));
        QTRY_VERIFY2(list->property("contentHeight").toReal() > list->height() + 100,
                     qPrintable(QStringLiteral("contentHeight %1, height %2")
                                    .arg(list->property("contentHeight").toReal())
                                    .arg(list->height())));
        list->setProperty("contentY", 60);
        auto *scroll = w.item("detailsScroll")->property("contentItem").value<QQuickItem *>();
        QTRY_VERIFY(scroll->property("contentHeight").toReal() - scroll->height() > 40);
        scroll->setProperty("contentY", 40);
        QQuickItem *panel = w.item("details");
        DetailView *detail = w.bridge.detail();
        const qsizetype from = w.bridge.sent.size();

        for (const Colours each : {Colours::Deuteran, Colours::Protan, Colours::Tritan,
                                   Colours::Monochrome, Colours::Default}) {
            w.settings.setColours(coloursKey(each));
            const Tones t = tones(each, dark());
            const QString name = coloursKey(each);
            // Every row the list draws now.
            auto *content = list->property("contentItem").value<QQuickItem *>();
            int drawn = 0;
            const QList<QQuickItem *> rows = content->childItems();
            for (QQuickItem *row : rows) {
                if (!row->property("index").isValid() || !row->isVisible()) {
                    continue;
                }
                ++drawn;
                const int at = row->property("index").toInt();
                const auto says = [findings, at](int role) {
                    return findings->data(findings->index(at), role);
                };
                QCOMPARE(says(FindingsModel::StateToneRole).value<QColor>(), t.pending);
                if (style == "compact") {
                    QQuickItem *glyph = Shown::item(row, "rowGlyph");
                    QVERIFY(glyph);
                    QCOMPARE(glyph->property("color").value<QColor>(), t.pending);
                    const QString change = Shown::text(row, "rowChange");
                    QCOMPARE(change, says(FindingsModel::ChangeRole).toString());
                    QVERIFY2(change.contains(t.old.name()) && change.contains(t.added.name()),
                             qPrintable(name + ": " + change));
                    continue;
                }
                QCOMPARE(Shown::item(row, "rowState")->property("color").value<QColor>(),
                         t.pending);
                QCOMPARE(Shown::item(row, "badge")->property("color").value<QColor>(), t.typoBadge);
                const QString line = Shown::text(row, "rowLine");
                QVERIFY2(line.contains(t.old.name()) && line.contains(t.added.name()),
                         qPrintable(name + ": " + line));
                if (style == "rich") {
                    // The tile is dark in every window, so it takes the palette's dark tones.
                    const QString tile = Shown::text(row, "rowTileLine");
                    QCOMPARE(tile, says(FindingsModel::TileLineRole).toString());
                    QVERIFY2(tile.contains(tones(each, true).old.name()),
                             qPrintable(name + ": " + tile));
                }
            }
            QVERIFY(drawn > 2);
            QCOMPARE(Shown::item(panel, "state")->property("color").value<QColor>(), t.pending);
            QCOMPARE(
                Shown::item(panel, "detailKind")->parentItem()->property("color").value<QColor>(),
                t.typoBadge);
            QQuickItem *now = Shown::item(panel, "nowBlock");
            QQuickItem *fixed = Shown::item(panel, "fixedBlock");
            QCOMPARE(Shown::item(now, "blockBar")->property("color").value<QColor>(), t.old);
            QCOMPARE(Shown::item(fixed, "blockBar")->property("color").value<QColor>(), t.added);
            QCOMPARE(Shown::item(now, "blockLabel")->property("color").value<QColor>(), t.old);
            QCOMPARE(Shown::item(fixed, "blockLabel")->property("color").value<QColor>(), t.added);
            QVERIFY2(Shown::text(now, "blockLine").contains(t.oldTint.name()), qPrintable(name));
            QVERIFY2(Shown::text(fixed, "blockLine").contains(t.addedTint.name()),
                     qPrintable(name));
            QCOMPARE(Shown::text(now, "blockLine"), detail->now());
            QCOMPARE(w.bridge.sample()->oldTone(), t.old);
            QVERIFY(w.bridge.sample()->now().contains(t.oldTint.name()));

            QCOMPARE(w.bridge.selected(), 1);
            QCOMPARE(list->property("currentIndex").toInt(), 1);
            QCOMPARE(list->property("contentY").toReal(), 60.0);
            QCOMPARE(scroll->property("contentY").toReal(), 40.0);
        }
        QCOMPARE(w.since(from), QStringList());
    }

    // Each part's text is the platform's size until it is set: the window's, its menus and dialogs
    // among it, and the Settings window, which takes no font from the window it opens over; the
    // list's, with its heading; and the details panel's. Each set at once, and only its own part
    // changes.
    void eachPartsTextIsThePlatformsSizeUntilSetAndEachIsItsOwn() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.delegate(0));
        QTRY_VERIFY(w.item("episode0"));
        const qreal platform = std::round(QFontInfo(QGuiApplication::font()).pointSizeF() * 2) / 2;
        const auto size = [](QObject *of) {
            return of->property("font").value<QFont>().pointSizeF();
        };
        QQuickItem *panel = w.item("details");
        const QList<QObject *> windowParts = {w.item("title"),        w.item("rejectButton"),
                                              w.item("acceptButton"), w.item("undoButton"),
                                              w.find("statusLine"),   w.item("episode0"),
                                              w.menu("&File"),        w.settingsWindow()};
        const QList<QObject *> listParts = {w.item("heading"),
                                            Shown::item(w.delegate(0), "rowLine"),
                                            Shown::item(w.delegate(0), "rowState")};
        const QList<QObject *> detailsParts = {Shown::item(panel, "place"),
                                               Shown::item(panel, "technical"),
                                               Shown::item(panel, "nowBlock")};
        // The parts not at their size, each as its name and the size it is.
        const auto offSize = [&](qreal inWindow, qreal inList, qreal inDetails) {
            QStringList off;
            const std::vector<std::pair<const QList<QObject *> *, qreal>> each = {
                {&windowParts, inWindow}, {&listParts, inList}, {&detailsParts, inDetails}};
            for (const auto &[parts, wanted] : each) {
                for (QObject *part : *parts) {
                    if (!part || size(part) != wanted) {
                        off << QStringLiteral("%1 %2")
                                   .arg(part ? part->objectName() : "none")
                                   .arg(part ? size(part) : 0);
                    }
                }
            }
            return off;
        };
        QCOMPARE(offSize(platform, platform, platform), QStringList());
        w.settings.setWindowTextSize(platform + 3);
        QCOMPARE(offSize(platform + 3, platform, platform), QStringList());
        w.settings.setListTextSize(platform + 1);
        QCOMPARE(offSize(platform + 3, platform + 1, platform), QStringList());
        w.settings.setDetailsTextSize(platform + 2.5);
        QCOMPARE(offSize(platform + 3, platform + 1, platform + 2.5), QStringList());
        w.settings.useSystemTextSizes();
        QCOMPARE(offSize(platform, platform, platform), QStringList());
    }

    // A platform font set in pixels still gives each part its size, in points.
    void aPlatformFontSetInPixelsStillSizesEachPart() {
        const auto restore =
            qScopeGuard([was = QGuiApplication::font()] { QGuiApplication::setFont(was); });
        QFont pixels = QGuiApplication::font();
        pixels.setPixelSize(16);
        QGuiApplication::setFont(pixels);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.delegate(0));
        const auto size = [](QObject *of) {
            return of->property("font").value<QFont>().pointSizeF();
        };
        // 16 pixels at the tests' 96 dots an inch.
        QCOMPARE(size(w.item("title")), 12.0);
        QCOMPARE(size(Shown::item(w.delegate(0), "rowLine")), 12.0);
        QCOMPARE(size(w.item("place")), 12.0);
    }

    // With the details panel's text at 14 pt and the list's at 14 pt, or at the largest, 24 pt, the
    // narrowest window still has room for each column's least width, and in each row style each
    // row's state in view: Compact's glyph whole; the others' glyph and word whole at 14 pt, and at
    // 24 pt, where the line has no room for them beside the badge, the word cut short before the
    // glyph, after the count and the place have given way. Nothing on the first line passes the
    // row's margin.
    void aNarrowWindowWithLargerTextKeepsEveryColumnAndEachRowsState_data() {
        QTest::addColumn<QString>("style");
        QTest::addColumn<qreal>("points");
        for (const char *style : {"compact", "twoLines", "rich"}) {
            for (const qreal points : {14.0, 24.0}) {
                QTest::addRow("%s at %g pt", style, points) << QString::fromLatin1(style) << points;
            }
        }
    }
    void aNarrowWindowWithLargerTextKeepsEveryColumnAndEachRowsState() {
        QFETCH(QString, style);
        QFETCH(qreal, points);
        Shown w({media()});
        w.settings.setRowStyle(style);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        w.settings.setListTextSize(points);
        w.settings.setDetailsTextSize(14);
        w.window->resize(w.window->minimumSize());
        QTRY_COMPARE(w.window->width(), 762);
        QQuickItem *sidebar = w.item("sidebar");
        QQuickItem *list = w.item("findings");
        QQuickItem *details = w.item("details");
        QTRY_COMPARE(w.item("columns")->width(), 762);
        QTRY_VERIFY(list->width() >= 260 && list->width() <= 280);
        QVERIFY(sidebar->width() >= 160);
        QVERIFY(details->width() >= 340);
        QVERIFY(details->mapToScene(QPointF(details->width(), 0)).x() <= w.window->width() + 0.5);
        for (const int at : {0, 1}) {
            QTRY_VERIFY(w.delegate(at));
            QQuickItem *row = w.delegate(at);
            QQuickItem *state = Shown::item(row, style == "compact" ? "rowGlyph" : "rowState");
            QVERIFY(state);
            QCOMPARE(state->property("font").value<QFont>().pointSizeF(), points);
            // Where `part` ends in the row.
            const auto ends = [row](QQuickItem *part) {
                return part->mapToItem(row, QPointF(part->width(), 0)).x();
            };
            const bool whole = style == "compact" || points < 24;
            const auto fits = [&] {
                if (ends(state) > row->width() - 8 + 0.5) {
                    return false;
                }
                if (whole) {
                    return qRound(state->width()) >= qFloor(state->implicitWidth());
                }
                // The glyph, with the ellipsis after the word's start, in the room the label has
                // past its padding.
                const QString glyph =
                    w.bridge.findings()
                        ->data(w.bridge.findings()->index(at), FindingsModel::GlyphRole)
                        .toString();
                const qreal room = state->width() - state->property("leftPadding").toReal();
                const QFontMetricsF metrics(state->property("font").value<QFont>());
                return state->property("truncated").toBool() &&
                       room >= metrics.horizontalAdvance(glyph + QStringLiteral(" …")) - 0.5;
            };
            QTRY_VERIFY2(fits(), qPrintable(QStringLiteral("the state ends at %1, %2 wide of %3")
                                                .arg(ends(state))
                                                .arg(state->width())
                                                .arg(state->implicitWidth())));
            for (const char *name : {"badge", "rowSeveral", "rowPlace", "rowTile"}) {
                QQuickItem *part = Shown::item(row, name);
                if (part && part->isVisible()) {
                    QVERIFY2(ends(part) <= row->width() - 8 + 0.5,
                             qPrintable(QStringLiteral("%1 ends at %2").arg(name).arg(ends(part))));
                    if (!whole && QLatin1String(name) != "badge" &&
                        QLatin1String(name) != "rowTile") {
                        QVERIFY2(part->width() < 0.5, name);
                    }
                }
            }
        }
        for (const char *name : {"nowBlock", "fixedBlock"}) {
            QQuickItem *block = Shown::item(details, name);
            QVERIFY(block->mapToItem(details, QPointF(block->width(), 0)).x() <= details->width());
        }
    }

    // The toolbar and the Find box in it, and the Controls that carry the list's and the details
    // panel's sizes, leave hover on, so that what is in them still follows the pointer where the
    // platform lets it: the Find box's ‹ and ›, the buttons under the video, the list's scroll bar
    // and the details' disclosure.
    void theToolbarAndTheListAndDetailsControlsKeepHoverOn() {
        QStyleHints *hints = QGuiApplication::styleHints();
        const auto restore = qScopeGuard(
            [hints, was = hints->useHoverEffects()] { hints->setUseHoverEffects(was); });
        hints->setUseHoverEffects(true);
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        for (const char *name : {"findPrevious", "findNext", "rejectButton", "acceptButton",
                                 "undoButton", "findingsScrollBar", "technical"}) {
            QQuickItem *item = w.item(name);
            QVERIFY2(item, name);
            QVERIFY2(item->property("hoverEnabled").toBool(), name);
        }
    }

    // File ▸ Settings… opens Settings, above Close: a dialog's window of its own, titled, modal and
    // transient for the window, which opens with the keys at its contents' size and can grow, and
    // which a click in the window beside it leaves open. Chosen again while it is open, as a menu
    // bar that still offers it could, it stays the one Settings window, with the keys. A choice in
    // it applies at once, so it has no button to confirm or cancel one, and closes as a window
    // does: Escape, Ctrl+W and the title bar's close button each close it and give review its keys
    // back, and none asks review anything; closed, it leaves Ctrl+W to File ▸ Close.
    void settingsOpensFromTheFileMenuAndClosesAsAWindowDoes() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *file = w.menu("&File");
        QVERIFY(file);
        const auto entry = [file](int index) {
            QQuickItem *at = nullptr;
            QMetaObject::invokeMethod(file, "itemAt", Q_RETURN_ARG(QQuickItem *, at),
                                      Q_ARG(int, index));
            return at;
        };
        QVERIFY(entry(0) && entry(1) && entry(2));
        QCOMPARE(entry(0)->property("action").value<QObject *>(), w.find("settings"));
        QVERIFY(entry(1)->inherits("QQuickMenuSeparator"));
        QCOMPARE(entry(2)->property("action").value<QObject *>(), w.find("close"));
        QObject *item = w.find("settings");
        QCOMPARE(item->property("text").toString(), "&Settings…");
        QCOMPARE(item->property("shortcut").toString(), "Ctrl+,");
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QCOMPARE(sw->title(), "Settings");
        QCOMPARE(sw->flags() & Qt::WindowType_Mask, Qt::Dialog);
        QCOMPARE(sw->transientParent(), w.window);
        QCOMPARE(sw->modality(), Qt::ApplicationModal);
        QVERIFY(!sw->isVisible());
        // No footer, and no button shown but the palettes' and Use the System Size: its sample's
        // blocks, which never edit, keep their edit's buttons hidden.
        QVERIFY(!sw->property("footer").value<QQuickItem *>());
        QStringList buttons;
        const std::function<void(QQuickItem *)> gather = [&buttons, &gather](QQuickItem *in) {
            if (!in->isVisible()) {
                return;
            }
            if (in->inherits("QQuickAbstractButton") && !in->inherits("QQuickRadioButton")) {
                buttons << in->objectName();
            }
            const QList<QQuickItem *> children = in->childItems();
            for (QQuickItem *child : children) {
                gather(child);
            }
        };
        gather(sw->contentItem());
        QCOMPARE(buttons, QStringList{"systemSize"});
        const qsizetype from = w.bridge.sent.size();
        bool first = true;
        for (const QString way : {"Escape", "Ctrl+W", "title bar"}) {
            QVERIFY(QMetaObject::invokeMethod(item, "trigger"));
            QTRY_VERIFY2(sw->isVisible(), qPrintable(way));
            QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
            QCOMPARE(sw->width(), sw->minimumWidth());
            QCOMPARE(sw->height(), sw->minimumHeight());
            QVERIFY2(sw->maximumWidth() > sw->minimumWidth(), qPrintable(way));
            QTest::mouseClick(w.window, Qt::LeftButton, {}, QPoint(4, 4));
            QVERIFY2(sw->isVisible(), qPrintable(way));
            if (first) {
                first = false;
                QVERIFY(QMetaObject::invokeMethod(item, "trigger"));
                QVERIFY(sw->isVisible());
                QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
                QWindowList others;
                const QWindowList windows = QGuiApplication::topLevelWindows();
                for (QWindow *each : windows) {
                    if (each->isVisible() && each != w.window) {
                        others << each;
                    }
                }
                QCOMPARE(others.size(), 1);
                QCOMPARE(others.first(), sw);
            }
            if (way == "Escape") {
                QTest::keyClick(sw, Qt::Key_Escape);
            } else if (way == "Ctrl+W") {
                QTest::keyClick(sw, Qt::Key_W, Qt::ControlModifier);
            } else {
                // What the title bar's close button asks.
                sw->close();
            }
            QTRY_VERIFY2(!sw->isVisible(), qPrintable(way));
            QVERIFY2(w.window->property("keysToReview").toBool(), qPrintable(way));
            QCOMPARE(w.since(from), QStringList());
        }
        // The window manager gives the keys back to the window, which offscreen has none to do.
        // Closed, Settings leaves Ctrl+W to File ▸ Close, which quits.
        w.window->requestActivate();
        QVERIFY(QTest::qWaitForWindowActive(w.window));
        QTest::keyClick(w.window, Qt::Key_W, Qt::ControlModifier);
        QCOMPARE(w.since(from), (QStringList{"Interrupt", "Interrupt"}));
    }

    // While Settings is open nothing of the window's reaches review, the quit keys among them, and
    // nothing reaches the video: no key typed into Settings, where a person's keys go and where Qt
    // still matches the window's shortcuts, nor given to the window, and no menu item of review's,
    // chosen as a menu bar outside the window could choose it. Settings' own keys work, Down
    // choosing the next palette at once, and Escape closing it. Closed, the keys are review's
    // again; opened again on another palette, its button has them, so that Down moves on from it.
    void settingsHoldsBackEveryKeyOfReviewsWhileItIsOpen() {
        const auto restore = qScopeGuard([] { useColours(Colours::Default); });
        Shown w({media()});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        QSignalSpy played(&w.player, &PlayerPort::reported);
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QTRY_VERIFY(w.delegate(0));
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        // Every key of every menu item and shortcut the window has, but Settings' own: its menu
        // item's, and those of its window, which is the window's child.
        QList<QKeySequence> keys;
        const QList<QObject *> own = sw->findChildren<QObject *>();
        const QList<QObject *> all = w.window->findChildren<QObject *>();
        for (QObject *each : all) {
            if (own.contains(each)) {
                continue;
            }
            if (each->inherits("QQuickAction") && each->objectName() != "settings") {
                const QKeySequence key(each->property("shortcut").toString());
                if (!key.isEmpty()) {
                    keys << key;
                }
            } else if (each->inherits("QQuickShortcut")) {
                const QVariantList sequences = each->property("sequences").toList();
                for (const QVariant &sequence : sequences) {
                    keys << QKeySequence(sequence.toString());
                }
                const QKeySequence one(each->property("sequence").toString());
                if (!one.isEmpty()) {
                    keys << one;
                }
            }
        }
        for (const QKeySequence &key :
             {QKeySequence("A"), QKeySequence("E"), QKeySequence("Space"), QKeySequence("J"),
              QKeySequence("Q"), QKeySequence("Ctrl+W"), QKeySequence("Ctrl+Q"),
              QKeySequence("Down"), QKeySequence("Ctrl+C"), QKeySequence("Ctrl+F"),
              QKeySequence("Home"), QKeySequence("Shift+G"), QKeySequence("PgDown")}) {
            QVERIFY2(keys.contains(key), qPrintable(key.toString()));
        }
        // Copy Subtitle Line asks review nothing: what it would copy is held back too.
        QGuiApplication::clipboard()->setText(QStringLiteral("held before Settings"));

        QTest::keyClick(w.window, Qt::Key_Comma, Qt::ControlModifier);
        QTRY_VERIFY(sw->isVisible());
        // Settings has the keys, and in it the palette chosen.
        QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
        QTRY_COMPARE(sw->activeFocusItem(), w.settingsItem("colours0"));
        const qsizetype from = w.bridge.sent.size();
        const QString status = w.bridge.statusLine();
        for (const QKeySequence &key : std::as_const(keys)) {
            // The arrows choose palettes while Settings is open, and Ctrl+W closes it; the presses
            // after the loop prove that none of them reaches a review action either.
            if (key == QKeySequence("Down") || key == QKeySequence("Up") ||
                key == QKeySequence("Left") || key == QKeySequence("Right") ||
                key == QKeySequence("Ctrl+W")) {
                continue;
            }
            for (QQuickWindow *to : {sw, w.window}) {
                const QString sent =
                    key.toString() + (to == sw ? " in Settings" : " in the window");
                QTest::keyClick(to, key[0].key(), key[0].keyboardModifiers());
                QVERIFY2(w.since(from).isEmpty(),
                         qPrintable(sent + ": " + w.since(from).join(" | ")));
                QVERIFY2(sw->isVisible(), qPrintable(sent));
            }
        }
        // Each of review's menu items, chosen as a menu bar outside the window could choose it.
        for (QObject *each : all) {
            if (each->inherits("QQuickAction") && each->objectName() != "settings" &&
                !own.contains(each)) {
                QVERIFY(QMetaObject::invokeMethod(each, "trigger"));
                QVERIFY2(w.since(from).isEmpty(),
                         qPrintable(each->objectName() + ": " + w.since(from).join(" | ")));
            }
        }
        QCOMPARE(w.settings.colours(), "default");
        QTest::qWait(100);
        QCOMPARE(w.since(from), QStringList());
        // Find asks review nothing, so it is seen by its field, which would take the window's keys.
        QVERIFY(!w.item("findField")->hasActiveFocus());
        QCOMPARE(QGuiApplication::clipboard()->text(), "held before Settings");
        QCOMPARE(played.count(), 0);
        QCOMPARE(finished.count(), 0);
        QCOMPARE(w.bridge.statusLine(), status);
        // Given to the window, Ctrl+W is matched, as every key is, against the shortcuts of the
        // windows Qt counts active, whichever window it is given to: Settings, which has the keys,
        // and the window it is over. Close, disabled, sends nothing, and Settings' own Ctrl+W
        // closes it.
        QTest::keyClick(w.window, Qt::Key_W, Qt::ControlModifier);
        QCOMPARE(w.since(from), QStringList());
        QTRY_VERIFY(!sw->isVisible());
        QVERIFY(QMetaObject::invokeMethod(w.find("settings"), "trigger"));
        QTRY_VERIFY(sw->isVisible());
        QTRY_COMPARE(sw->activeFocusItem(), w.settingsItem("colours0"));

        QTest::keyClick(sw, Qt::Key_Down);
        QCOMPARE(w.settings.colours(), "deuteran");
        const Tones deuteran = tones(Colours::Deuteran, dark());
        QCOMPARE(Shown::item(w.delegate(0), "rowState")->property("color").value<QColor>(),
                 deuteran.pending);
        QQuickItem *sample = w.settingsItem("sampleNow");
        QVERIFY(sample);
        QCOMPARE(Shown::item(sample, "blockBar")->property("color").value<QColor>(), deuteran.old);
        QCOMPARE(Shown::text(sample, "blockLine"), w.bridge.sample()->now());
        QCOMPARE(sw->activeFocusItem(), w.settingsItem("colours1"));
        QTest::keyClick(sw, Qt::Key_Up);
        QCOMPARE(w.settings.colours(), "default");
        QCOMPARE(sw->activeFocusItem(), w.settingsItem("colours0"));
        QTest::keyClick(sw, Qt::Key_Right);
        QCOMPARE(w.settings.colours(), "deuteran");
        QCOMPARE(sw->activeFocusItem(), w.settingsItem("colours1"));
        QTest::keyClick(sw, Qt::Key_Left);
        QCOMPARE(w.settings.colours(), "default");
        QCOMPARE(sw->activeFocusItem(), w.settingsItem("colours0"));
        QCOMPARE(w.since(from), QStringList());

        QTest::keyClick(sw, Qt::Key_Escape);
        QTRY_VERIFY(!sw->isVisible());
        // The window manager gives the keys back to the window Settings was over, which offscreen
        // has none to do.
        w.window->requestActivate();
        QVERIFY(QTest::qWaitForWindowActive(w.window));
        QTest::keyClick(w.window, Qt::Key_J);
        QCOMPARE(w.since(from), QStringList{"Move 1"});
        w.settings.setColours(QStringLiteral("protan"));
        QTest::keyClick(w.window, Qt::Key_Comma, Qt::ControlModifier);
        QTRY_VERIFY(sw->isVisible());
        QTRY_COMPARE(sw->activeFocusItem(), w.settingsItem("colours2"));
        QTest::keyClick(sw, Qt::Key_Down);
        QCOMPARE(w.settings.colours(), "tritan");
        QTest::keyClick(sw, Qt::Key_Escape);
        QTRY_VERIFY(!sw->isVisible());
    }

    // In Settings each part's size steps by half a point and says so in points; a step sets that
    // part's size at once, and the system size puts all three back.
    void settingsSetsEachPartsSizeInHalfPoints() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QTest::keyClick(w.window, Qt::Key_Comma, Qt::ControlModifier);
        QTRY_VERIFY(sw->isVisible());
        // The keys typed below go where a person's do, to Settings once it has them.
        QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
        const qreal platform = w.settings.windowTextSize();
        const std::vector<std::pair<const char *, std::function<qreal()>>> parts = {
            {"windowSize", [&w] { return w.settings.windowTextSize(); }},
            {"listSize", [&w] { return w.settings.listTextSize(); }},
            {"detailsSize", [&w] { return w.settings.detailsTextSize(); }}};
        for (const auto &[name, kept] : parts) {
            QQuickItem *box = w.settingsItem(name);
            QVERIFY2(box, name);
            QCOMPARE(box->property("value").toInt(), qRound(platform * 2));
            QCOMPARE(box->property("from").toInt(), 12);
            QCOMPARE(box->property("to").toInt(), 48);
            QCOMPARE(box->property("displayText").toString(),
                     WindowSettings::sizeWords(qRound(platform * 2)));
            QVERIFY(QMetaObject::invokeMethod(box, "forceActiveFocus"));
            QTest::keyClick(sw, Qt::Key_Up);
            QCOMPARE(kept(), platform + 0.5);
            QCOMPARE(box->property("displayText").toString(),
                     WindowSettings::sizeWords(qRound(platform * 2) + 1));
        }
        QCOMPARE(w.item("title")->property("font").value<QFont>().pointSizeF(), platform + 0.5);
        w.click(sw, w.settingsItem("systemSize"));
        QCOMPARE(w.settings.windowTextSize(), platform);
        QCOMPARE(w.settings.listTextSize(), platform);
        QCOMPARE(w.settingsItem("detailsSize")->property("value").toInt(), qRound(platform * 2));
    }

    // Each size box is as wide as the style makes a box saying the widest size, and 2 px more. A
    // step of the list's or the details panel's box leaves every width as it is: at every size from
    // the first to the last, with the window's text at the platform's size and at 24 pt, both
    // keep one width, the three boxes share it, and no size shown is cut. A step of the window's
    // own box changes the text all three are drawn in, and so all three widths alike: at each of
    // its sizes the three share one width and none is cut. Stepped by a person's keys from a whole
    // point to a half and back, a box keeps its width; and typed into, it takes no size.
    void aSizeBoxKeepsOneWidthAtEverySize() {
        Shown w({media()});
        const auto restore = qScopeGuard([&w] { w.settings.useSystemTextSizes(); });
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QVERIFY(QMetaObject::invokeMethod(w.find("settings"), "trigger"));
        QTRY_VERIFY(sw->isVisible());
        QQuickItem *window = w.settingsItem("windowSize");
        QQuickItem *list = w.settingsItem("listSize");
        QQuickItem *details = w.settingsItem("detailsSize");
        QQuickItem *widest = w.settingsItem("widestSize");
        QVERIFY(window && list && details && widest);
        const QList<QQuickItem *> boxes = {window, list, details};
        const int from = window->property("from").toInt();
        const int to = window->property("to").toInt();
        QCOMPARE(from, w.settings.leastHalfPoints());
        QCOMPARE(to, w.settings.mostHalfPoints());
        QCOMPARE(to - from + 1, 37);
        for (QQuickItem *box : boxes) {
            QVERIFY2(!box->property("editable").toBool(), qPrintable(box->objectName()));
        }
        // Whether `box` says `halfPoints`.
        const auto says = [](QQuickItem *box, int halfPoints) {
            auto *shown = box->property("contentItem").value<QQuickItem *>();
            return shown->property("text").toString() == WindowSettings::sizeWords(halfPoints);
        };
        // Whether the layout has given every box the width it asks for.
        const auto settled = [&boxes] {
            return std::all_of(boxes.begin(), boxes.end(), [](QQuickItem *box) {
                return box->width() == box->implicitWidth();
            });
        };
        // The boxes not `wide`, asked or given, not 2 px wider than the box saying the widest
        // size, or whose size is cut, each by its name, its width and what it says.
        const auto off = [&boxes, widest](qreal wide) {
            QStringList odd;
            for (QQuickItem *box : boxes) {
                auto *shown = box->property("contentItem").value<QQuickItem *>();
                if (box->width() != wide || box->implicitWidth() != wide ||
                    box->implicitWidth() != widest->implicitWidth() + 2 ||
                    shown->implicitWidth() > shown->width() + 0.5) {
                    odd << QStringLiteral("%1 %2 %3")
                               .arg(box->objectName())
                               .arg(box->width())
                               .arg(shown->property("text").toString());
                }
            }
            return odd;
        };

        for (const std::optional<qreal> windowText :
             {std::optional<qreal>(), std::optional<qreal>(24.0)}) {
            if (windowText) {
                w.settings.setWindowTextSize(*windowText);
            }
            QTRY_VERIFY(settled());
            const qreal wide = window->width();
            for (int halfPoints = from; halfPoints <= to; ++halfPoints) {
                w.settings.setListTextSize(halfPoints / 2.0);
                w.settings.setDetailsTextSize(halfPoints / 2.0);
                QTRY_VERIFY(says(list, halfPoints) && says(details, halfPoints));
                QTRY_VERIFY(settled());
                QCOMPARE(off(wide), QStringList());
            }
        }
        for (int halfPoints = from; halfPoints <= to; ++halfPoints) {
            w.settings.setWindowTextSize(halfPoints / 2.0);
            QTRY_VERIFY(says(window, halfPoints));
            QTRY_VERIFY(settled());
            QCOMPARE(off(window->width()), QStringList());
        }

        QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
        w.settings.setListTextSize(20);
        QTRY_VERIFY(says(list, 40));
        QTRY_VERIFY(settled());
        const qreal wide = list->width();
        QVERIFY(QMetaObject::invokeMethod(list, "forceActiveFocus"));
        QTRY_VERIFY(list->hasActiveFocus());
        for (const auto &[key, halfPoints] :
             {std::pair(Qt::Key_Up, 41), std::pair(Qt::Key_Down, 40)}) {
            QTest::keyClick(sw, key);
            QTRY_VERIFY(says(list, halfPoints));
            QCOMPARE(w.settings.listTextSize(), halfPoints / 2.0);
            QTRY_VERIFY(settled());
            QCOMPARE(off(wide), QStringList());
        }
        // Its words chosen whole, so that digits typed would take their place were it editable.
        QTest::keyClick(sw, Qt::Key_A, Qt::ControlModifier);
        for (const Qt::Key key : {Qt::Key_1, Qt::Key_2, Qt::Key_Return}) {
            QTest::keyClick(sw, key);
        }
        QTest::qWait(50);
        QCOMPARE(list->property("value").toInt(), 40);
        QCOMPARE(w.settings.listTextSize(), 20.0);
        QVERIFY(says(list, 40));
    }

    // Every setting is in reach at the largest sizes, on a screen shorter than Settings at 24 pt.
    // Raised to 24 pt while Settings is open, its window grows at once, so that no palette's name,
    // size or button is cut short or reaches past its edge, but grows no taller than four fifths
    // of the screen: what that has no room for scrolls into view, Use the System Size the last of
    // it. Closed and opened again, it opens at that size. At each size all of it shows, or its
    // window is as tall as it is allowed.
    void everySettingIsInReachAtTheLargestSizes() {
        Shown w({media()});
        const auto restore = qScopeGuard([&w] { w.settings.useSystemTextSizes(); });
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *open = w.find("settings");
        QVERIFY(open);
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QQuickItem *scroll = w.settingsItem("settingsScroll");
        QVERIFY(scroll);
        // The window at its least size on the shorter of the tests' two screens, and Settings
        // with it, where a window manager would place it, over the window: offscreen has none, and
        // leaves a window where it is put.
        const QList<QScreen *> screens = QGuiApplication::screens();
        QScreen *shorter = *std::min_element(screens.begin(), screens.end(), [](auto *a, auto *b) {
            return a->geometry().height() < b->geometry().height();
        });
        w.window->resize(w.window->minimumSize());
        w.window->setPosition(shorter->availableGeometry().topLeft());
        sw->setScreen(shorter);
        sw->setPosition(shorter->availableGeometry().topLeft());
        QTRY_COMPARE(w.window->screen(), shorter);
        const int allowed = qFloor(0.8 * shorter->geometry().height());
        // The settings cut short or reaching past the window's right edge, each by its name.
        const auto cut = [&w, sw] {
            QStringList off;
            for (const char *name : {"colours0", "colours1", "colours2", "colours3", "colours4",
                                     "windowSize", "listSize", "detailsSize", "systemSize"}) {
                QQuickItem *setting = w.settingsItem(name);
                if (!setting) {
                    off << QStringLiteral("no %1").arg(name);
                } else if (setting->width() < setting->implicitWidth() - 0.5 ||
                           setting->mapToScene(QPointF(setting->width(), 0)).x() >
                               sw->width() + 0.5) {
                    off << QString::fromLatin1(name);
                }
            }
            return off;
        };
        const auto allOrAsTallAsAllowed = [sw, scroll, allowed] {
            return scroll->property("contentHeight").toReal() <= scroll->height() + 0.5 ||
                   sw->height() >= allowed;
        };
        // Whether all of `of` is inside the scroll view, as the scene draws them.
        const auto inView = [scroll](QQuickItem *of) {
            const QRectF at = of->mapRectToScene(QRectF(0, 0, of->width(), of->height()));
            const QRectF in =
                scroll->mapRectToScene(QRectF(0, 0, scroll->width(), scroll->height()));
            return at.left() >= in.left() - 0.5 && at.top() >= in.top() - 0.5 &&
                   at.right() <= in.right() + 0.5 && at.bottom() <= in.bottom() + 0.5;
        };

        QVERIFY(QMetaObject::invokeMethod(open, "trigger"));
        QTRY_VERIFY(sw->isVisible());
        QCOMPARE(sw->screen(), shorter);
        QTRY_COMPARE(cut(), QStringList());
        QVERIFY(allOrAsTallAsAllowed());

        w.settings.setWindowTextSize(24);
        w.settings.setListTextSize(24);
        w.settings.setDetailsTextSize(24);
        QTRY_COMPARE(cut(), QStringList());
        QVERIFY2(sw->height() <= allowed,
                 qPrintable(QStringLiteral("%1 tall, %2 allowed").arg(sw->height()).arg(allowed)));
        QVERIFY(allOrAsTallAsAllowed());
        const qreal contentHeight = scroll->property("contentHeight").toReal();
        QVERIFY2(
            contentHeight > scroll->height(),
            qPrintable(
                QStringLiteral("%1 of content in %2").arg(contentHeight).arg(scroll->height())));
        auto *view = scroll->property("contentItem").value<QQuickItem *>();
        QVERIFY(view);
        view->setProperty("contentY", contentHeight - scroll->height());
        QTRY_VERIFY(inView(w.settingsItem("systemSize")));
        QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
        QTest::keyClick(sw, Qt::Key_Escape);
        QTRY_VERIFY(!sw->isVisible());

        QVERIFY(QMetaObject::invokeMethod(open, "trigger"));
        QTRY_VERIFY(sw->isVisible());
        QCOMPARE(sw->screen(), shorter);
        QTRY_COMPARE(cut(), QStringList());
        QVERIFY(allOrAsTallAsAllowed());
    }

    // Made larger and closed, Settings opens again at its contents' size.
    void settingsOpensAgainAtItsContentsSize() {
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QObject *open = w.find("settings");
        QVERIFY(open);
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QVERIFY(QMetaObject::invokeMethod(open, "trigger"));
        QTRY_VERIFY(sw->isVisible());
        const QSize least = sw->minimumSize();
        QCOMPARE(sw->size(), least);
        sw->resize(least + QSize(120, 80));
        // Once the window has taken the size, as it takes a person's drag.
        QTRY_COMPARE(sw->contentItem()->size(), QSizeF(least + QSize(120, 80)));
        sw->close();
        QTRY_VERIFY(!sw->isVisible());
        QVERIFY(QMetaObject::invokeMethod(open, "trigger"));
        QTRY_VERIFY(sw->isVisible());
        QCOMPARE(sw->minimumSize(), least);
        QCOMPARE(sw->size(), least);
    }

    // Settings' labels sit level with the first line of what they name, whatever the sizes of the
    // two: Palette with the first palette's name, Sample with the sample's first line.
    void settingsLabelsAreLevelWithWhatTheyName() {
        Shown w({media()});
        const auto restore = qScopeGuard([&w] { w.settings.useSystemTextSizes(); });
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QVERIFY(QMetaObject::invokeMethod(w.find("settings"), "trigger"));
        QTRY_VERIFY(sw->isVisible());
        const auto baseline = [](QQuickItem *of) {
            return of->mapToScene(QPointF(0, of->baselineOffset())).y();
        };
        // How far each label's baseline is from that of the first line of what it names, by the
        // label's name: the first palette's, and the sample's label, the first line in it.
        const auto off = [&w, &baseline] {
            QStringList apart;
            QQuickItem *sample = w.settingsItem("sampleNow");
            const std::vector<std::pair<const char *, QQuickItem *>> pairs = {
                {"paletteLabel", w.settingsItem("colours0")},
                {"sampleLabel", sample ? Shown::item(sample, "blockLabel") : nullptr}};
            for (const auto &[label, named] : pairs) {
                QQuickItem *beside = w.settingsItem(label);
                if (!beside || !named) {
                    apart << QStringLiteral("%1 or what it names missing").arg(label);
                } else if (qAbs(baseline(beside) - baseline(named)) > 1) {
                    apart << QStringLiteral("%1 %2").arg(label).arg(baseline(beside) -
                                                                    baseline(named));
                }
            }
            return apart;
        };
        QTRY_COMPARE(off(), QStringList());
        w.settings.setDetailsTextSize(24);
        QTRY_COMPARE(off(), QStringList());
        w.settings.setWindowTextSize(24);
        w.settings.setDetailsTextSize(6);
        QTRY_COMPARE(off(), QStringList());
    }

    // Review ending in an error while Settings is open closes Settings, which would otherwise
    // hold the keys from the window the error's dialog is in, and Settings cannot open again over
    // the dialog: the dialog opens, and Escape in the window closes it, which ends the window
    // with 1.
    void aFatalEndClosesSettingsForItsDialog() {
        Shown w({"no-such-run"});
        QSignalSpy finished(&w.bridge, &SessionBridge::finished);
        // Review not started until Settings is open.
        QVERIFY(w.show());
        QQuickWindow *sw = w.settingsWindow();
        QVERIFY(sw);
        QObject *settings = w.find("settings");
        QVERIFY(settings);
        QVERIFY(settings->property("enabled").toBool());
        QVERIFY(QMetaObject::invokeMethod(settings, "trigger"));
        QTRY_VERIFY(sw->isVisible());
        QTRY_COMPARE(QGuiApplication::focusWindow(), sw);
        QObject *fatal = w.find("fatal");
        QVERIFY(fatal);
        QVERIFY(!fatal->property("visible").toBool());
        w.bridge.start();
        QTRY_VERIFY(!sw->isVisible());
        QTRY_VERIFY(fatal->property("opened").toBool());
        QCOMPARE(finished.count(), 0);
        // Chosen now, as a menu bar that still offers it could, Settings stays closed.
        QVERIFY(!settings->property("enabled").toBool());
        QVERIFY(QMetaObject::invokeMethod(settings, "trigger"));
        QVERIFY(!sw->isVisible());
        // The window manager gives the keys back to the window Settings was over, which offscreen
        // has none to do.
        w.window->requestActivate();
        QVERIFY(QTest::qWaitForWindowActive(w.window));
        QTest::keyClick(w.window, Qt::Key_Escape);
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 1);
    }

    // What Settings chooses is there when the next window over the same settings starts: its rows
    // are drawn in the kept palette and at the kept list size.
    void whatSettingsChoosesIsThereAtTheNextStart() {
        const auto restore = qScopeGuard([] { useColours(Colours::Default); });
        QTemporaryDir kept;
        QVERIFY(kept.isValid());
        {
            Shown w({media()}, kept.path());
            QVERIFY(w.start());
            QTRY_COMPARE(w.rows(), 2);
            w.settings.setColours(QStringLiteral("protan"));
            w.settings.setListTextSize(14);
        }
        // The palette drawn with is the process's, and outlives the window: put back to the
        // default, only the kept file can bring Protan back.
        useColours(Colours::Default);
        Shown w({media()}, kept.path());
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QCOMPARE(w.settings.colours(), "protan");
        QTRY_VERIFY(w.delegate(0));
        QQuickItem *row = w.delegate(0);
        QCOMPARE(Shown::item(row, "rowState")->property("color").value<QColor>(),
                 tones(Colours::Protan, dark()).pending);
        QCOMPARE(Shown::item(row, "rowLine")->property("font").value<QFont>().pointSizeF(), 14.0);
    }

    // Unset, a part's size is the platform's, which follows the desktop's font as it changes.
    void theWindowFollowsThePlatformsFontWhileItsSizeIsUnset() {
        const auto restore =
            qScopeGuard([was = QGuiApplication::font()] { QGuiApplication::setFont(was); });
        Shown w({media()});
        QVERIFY(w.start());
        QTRY_COMPARE(w.rows(), 2);
        QFont larger = QGuiApplication::font();
        const qreal platform = w.settings.windowTextSize();
        larger.setPointSizeF(larger.pointSizeF() + 3);
        QSignalSpy changed(&w.settings, &WindowSettings::textSizesChanged);
        QGuiApplication::setFont(larger);
        QTRY_COMPARE(w.item("title")->property("font").value<QFont>().pointSizeF(), platform + 3);
        // Qt may send the event to each window too, which must not say it again.
        QTest::qWait(50);
        QCOMPARE(changed.count(), 1);
    }

    // Esc and OK each close the dialog, which ends the window with 1; a click beside it does not.
    void aFatalEndShowsItsDialogAndClosingItEndsWithOne() {
        for (const bool byButton : {false, true}) {
            Shown w({"no-such-run"});
            QSignalSpy finished(&w.bridge, &SessionBridge::finished);
            QVERIFY(w.start());
            QObject *dialog = w.find("fatal");
            QVERIFY(dialog);
            QTRY_VERIFY(dialog->property("opened").toBool());
            QCOMPARE(w.err.str(), "subcheck-qt: no run named no-such-run\n");
            QCOMPARE(finished.count(), 0);
            QTest::mouseClick(w.window, Qt::LeftButton, {}, QPoint(4, 4));
            QVERIFY(dialog->property("opened").toBool());
            QCOMPARE(finished.count(), 0);
            if (byButton) {
                auto *footer = dialog->property("footer").value<QQuickItem *>();
                QVERIFY(footer);
                QQuickItem *ok = Shown::button(footer, QStringLiteral("OK"));
                QVERIFY(ok);
                w.click(ok);
            } else {
                QTest::keyClick(w.window, Qt::Key_Escape);
            }
            QTRY_COMPARE(finished.count(), 1);
            QCOMPARE(finished[0][0].toInt(), 1);
        }
    }

    // Review ending in an error takes the video's window from the window while the error's dialog
    // shows, so that nothing of the video draws over the dialog, which the window draws in its
    // scene; and Pop Out waits, as the window has nothing left to show. The same whether review
    // ends before the window's first frame, as main.cpp starts it, with the video's window held,
    // or with it out.
    void aFatalEndTakesTheVideoOutFromUnderItsDialog() {
        enum class Before { FirstFrame, Held, Out };
        for (const Before before : {Before::FirstFrame, Before::Held, Before::Out}) {
            const char *round = before == Before::FirstFrame ? "before the first frame"
                                : before == Before::Held     ? "held"
                                                             : "out";
            Shown w({"no-such-run"});
            QObject *dialog = w.find("fatal");
            QObject *container = w.find("videoContainer");
            QObject *popOut = w.find("popOutVideo");
            QVERIFY2(dialog && container && popOut, round);
            if (before == Before::FirstFrame) {
                QVERIFY2(w.start(), round);
            } else {
                QVERIFY2(w.show(), round);
                QQuickWindow *video = w.videoWindow();
                QVERIFY2(video, round);
                QTRY_VERIFY2(video->isVisible(), round);
                if (before == Before::Out) {
                    QVERIFY2(QMetaObject::invokeMethod(popOut, "trigger"), round);
                    QVERIFY2(w.window->property("videoOut").toBool(), round);
                    QVERIFY2(video->isVisible(), round);
                } else {
                    QCOMPARE(container->property("window").value<QWindow *>(), video);
                }
                w.bridge.start();
            }
            QTRY_VERIFY2(dialog->property("opened").toBool(), round);
            QVERIFY2(!container->property("window").value<QWindow *>(), round);
            QQuickWindow *video = w.videoWindow();
            QVERIFY2(video, round);
            QVERIFY2(!video->isVisible(), round);
            QVERIFY2(!popOut->property("enabled").toBool(), round);
        }
    }
};

SUBCHECK_OFFSCREEN_TEST_MAIN(WindowTest)
#include "window_test.moc"
