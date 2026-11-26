// The details panel: the video's place, 16:9 at the panel's width with the video's control bar
// under it, which the window fills with the video (Main.qml), while the video is not out in a
// window of its own; under it a row of its own with Undo at the left and Reject and Accept at the
// right, which stays between the video and what scrolls; then the selected finding as the window
// words it: its line as it is now above its line with the fix, each a block of its own that keeps
// its place from one finding to the next for a line of one or two lines at the block's own font
// (one of three or more, or one drawn partly in a taller fallback font, moves what is below it
// down); a click on the block with the fix opens its edit at the nearest character, while it can be
// reworded, and while an edit is open the line gives way to a field for its wording; below them its
// banners, its mark and why it was found; its file's particulars under a disclosure; and what the
// check said of the episode selected while that is the finding's: below the finding, set off by a
// line, or alone with none selected. For a click on an episode the check found nothing in, its log
// shows alone, in the finding's place, while review keeps the finding selected: below the finding,
// the two would read as one, and the log is not the finding's.
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Subcheck

// A Control, so that the labels in it take the size of text the window gives the panel. Its hover
// stays on, as a Control's is by default, so that what is in it still follows the pointer.
Control {
    id: panel

    required property SessionBridge bridge
    readonly property DetailView detail: bridge.detail
    // The video's place, which nothing here fills: the window puts the video there, in a window of
    // its own or in its own scene, so that the video can leave the panel.
    property alias videoSlot: slot
    // The video's control bar's height, which the place holds under the video, and the least width
    // that keeps each of its groups whole, which the panel keeps: the window gives both, from the
    // video's pane.
    property real videoBarHeight
    property real videoLeastWidth
    // Whether the video is out, a window of its own: its place goes, and the decisions' row is the
    // panel's first.
    property bool videoOut
    // The window's line between parts, drawn here between the finding and its episode's log.
    required property color separator
    // Whether Reject and Accept have a finding to act on, which the window decides, as its menu and
    // keys act on the same one.
    required property bool canDecide
    // The window's own font, which the decision row takes rather than this panel's: its buttons
    // follow the window's size of text, as the menus and the toolbar do.
    required property font windowFont
    // The least width that shows the decision row's three buttons whole and each group of the
    // video's control bar whole, which follow the window's size of text and the style's buttons;
    // the window never makes the panel narrower. The bar counts while the video is out too, on
    // purpose: the window then does not grow wider at once as the video comes back in.
    readonly property real leastWidth: Math.max(decisions.implicitWidth, videoLeastWidth)
    // The least height the panel keeps under the video's place: the decisions' row whole, and
    // under it the finding's first three lines of the panel's text below its top margin, so that
    // what Accept and Reject act on can be read. The window counts it in its least height.
    readonly property real leastHeightUnderVideo: decisions.implicitHeight
                                                  + 3 * detailsLine.lineSpacing + 8
    // Whether a greyed episode's log shows alone, in the finding's place. Its end can move no
    // selection, as a click on the finding's own row does not, so the finding comes back at its
    // top here, as another finding opens at its own, rather than as far down as the log was. An
    // edit stays open under it, as opening the episode moves no selection; the edit's field, hidden
    // with the finding, gives the window its keys meanwhile, and takes them back with the words
    // typed in it when the finding shows again.
    readonly property bool showingEpisode: bridge.showingEpisode
    // Where the field's cursor goes as the edit a click on the fix asked for opens: at the line's
    // character nearest the click, or at the end for -1. It waits for editOpened, as the field
    // takes review's wording only then, and goes with review's answer to the Edit, which can open
    // none, with the selection, and with an Edit asked otherwise, by E or a menu (Main.qml), so
    // that a click's place never reaches a later edit, another finding's, or one asked before
    // review answered it.
    property int cursorAt: -1

    // A right-click on the finding asks for its row's menu, at `at` in the panel's coordinates. A
    // greyed episode's log, in the finding's place, has none: the finding it would act on is not in
    // view.
    signal menuAsked(point at)

    onShowingEpisodeChanged: {
        if (!panel.showingEpisode) {
            (scroll.contentItem as Flickable).contentY = 0;
            if (panel.bridge.editing) {
                panel.focusField();
            }
        }
    }

    FontMetrics {
        id: detailsLine

        font: panel.font
    }

    component Heading: Label {
        topPadding: 6
        font.bold: true
        opacity: 0.7
    }

    component Lines: ColumnLayout {
        id: lines

        property list<string> each

        spacing: 0
        visible: each.length > 0

        Repeater {
            model: lines.each

            Label {
                required property string modelData

                text: modelData
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }
        }
    }

    // The log's heading at the top of the panel, or the panel scrolled as far as it goes. The
    // column is laid out first, as the log has only just shown and would be placed at the next
    // frame.
    function scrollToLog() {
        content.ensurePolished();
        const view = scroll.contentItem as Flickable;
        view.contentY = Math.max(0, Math.min(logHeading.y, view.contentHeight - view.height));
    }

    // Gives the edit's field the keys, scrolled into view the least way if the panel was scrolled
    // past it, so that what is typed is seen. The field and the column are laid out first, as the
    // field has only just shown and would be placed at the next frame.
    function focusField() {
        const field = fixedBlock.field;
        fixedBlock.contentItem.ensurePolished();
        content.ensurePolished();
        const view = scroll.contentItem as Flickable;
        const top = field.mapToItem(view.contentItem, 0, 0).y;
        if (top < view.contentY) {
            view.contentY = top;
        } else if (top + field.height > view.contentY + view.height) {
            view.contentY = top + field.height - view.height;
        }
        field.forceActiveFocus();
    }

    // Another finding opens at its top; the same one, refreshed, stays where it was scrolled to.
    // What the check said of an episode with no findings, shown for a click on it, is scrolled to:
    // the panel can be scrolled down the finding it takes the place of.
    Connections {
        target: panel.bridge

        function onSelectionMoved() {
            (scroll.contentItem as Flickable).contentY = 0;
            panel.cursorAt = -1;
        }
        function onEpisodeShown() {
            panel.scrollToLog();
        }
        // The field starts from review's wording once, as the edit opens, and is the person's from
        // then on: bound to the bridge, it would put review's wording back over the typing at every
        // change review sends while the edit is open.
        function onEditOpened() {
            const field = fixedBlock.field;
            field.text = panel.bridge.editWording;
            field.cursorPosition = panel.cursorAt >= 0 ? Math.min(panel.cursorAt, field.length)
                                                        : field.length;
            panel.cursorAt = -1;
            panel.focusField();
        }
        // With no edit open, review took the Edit while it asked, say: the click's ask ends.
        function onEditAnswered() {
            panel.cursorAt = -1;
            if (!panel.bridge.editing) {
                fixedBlock.answered();
            }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Black, so that the video's place shows before the video first draws there. The video,
        // 16:9, and its control bar under it.
        Rectangle {
            id: slot

            objectName: "videoSlot"
            color: "black"
            visible: !panel.videoOut
            Layout.fillWidth: true
            Layout.preferredHeight: width * 9 / 16 + panel.videoBarHeight
        }

        // Fixed between the video and what scrolls, so that it never scrolls away nor moves with
        // the finding's length. Greyed with nothing to act on, never hidden, so that it keeps its
        // place.
        Control {
            id: decisions

            objectName: "decisions"
            font: panel.windowFont
            leftPadding: 8
            rightPadding: 8
            topPadding: 6
            bottomPadding: 6
            Layout.fillWidth: true

            contentItem: RowLayout {
                spacing: 6

                Button {
                    objectName: "undoButton"
                    text: qsTr("Undo")
                    enabled: !panel.bridge.editing
                    focusPolicy: Qt.NoFocus
                    onClicked: panel.bridge.undo()
                }
                Item {
                    Layout.fillWidth: true
                }
                Button {
                    objectName: "rejectButton"
                    text: qsTr("Reject")
                    enabled: panel.canDecide
                    focusPolicy: Qt.NoFocus
                    onClicked: panel.bridge.reject()
                }
                Button {
                    objectName: "acceptButton"
                    text: qsTr("Accept")
                    enabled: panel.canDecide
                    focusPolicy: Qt.NoFocus
                    highlighted: true
                    onClicked: panel.bridge.accept()
                }
            }
        }

        ScrollView {
            id: scroll

            objectName: "detailsScroll"
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            // Its lines wrap at the panel's width: left to the column, the content is as wide as
            // its longest line unwrapped, and scrolls sideways.
            contentWidth: availableWidth

            // The finding, then what the check said of the episode selected, which shows with no
            // finding selected too.
            ColumnLayout {
                id: content

                width: scroll.availableWidth - 16
                x: 8
                spacing: 2

                ColumnLayout {
                    id: finding

                    spacing: 2
                    visible: panel.detail.shown && !panel.bridge.showingEpisode
                    Layout.fillWidth: true
                    ContextMenu.onRequested: position => {
                        panel.menuAsked(finding.mapToItem(panel, position));
                    }

                    RowLayout {
                        id: row

                        Layout.topMargin: 8
                        Layout.fillWidth: true
                        spacing: 6

                        Rectangle {
                            id: badge

                            color: panel.detail.kindTone
                            radius: 3
                            implicitWidth: kind.implicitWidth + 10
                            implicitHeight: kind.implicitHeight + 2

                            Label {
                                id: kind

                                objectName: "detailKind"
                                anchors.centerIn: parent
                                text: panel.detail.kind
                                textFormat: Text.PlainText
                            }
                        }
                        Label {
                            objectName: "place"
                            text: panel.detail.place
                            textFormat: Text.PlainText
                            elide: Text.ElideRight
                            Layout.fillWidth: true
                        }
                        // No wider than the room the badge leaves it, so that the row never passes
                        // the column: the place, which fills what is left, gives way first, and the
                        // state, which each decision changes, is cut short only once the place has
                        // no room left.
                        Label {
                            objectName: "state"
                            text: panel.detail.glyph + " " + panel.detail.stateWord
                            textFormat: Text.PlainText
                            color: panel.detail.stateTone
                            elide: Text.ElideRight
                            Layout.preferredWidth: Math.min(implicitWidth,
                                                            Math.max(0, row.width - badge.width
                                                                        - 2 * row.spacing))
                        }
                    }

                    // Right under the kind row, which is always one line, and each with room for
                    // two lines, so that a line of one or two lines moves neither block: the
                    // banners and the mark, which come and go, are below them.
                    ChangeBlock {
                        objectName: "nowBlock"
                        label: qsTr("− In the subtitles now")
                        line: panel.detail.now
                        tone: panel.detail.oldTone
                        minLines: 2
                        Layout.topMargin: 8
                        Layout.fillWidth: true
                    }
                    ChangeBlock {
                        id: fixedBlock

                        objectName: "fixedBlock"
                        label: panel.detail.edited ? qsTr("+ With your wording")
                                                   : qsTr("+ With the fix")
                        line: panel.detail.fixed
                        tone: panel.detail.newTone
                        minLines: 2
                        editing: panel.bridge.editing && !panel.showingEpisode
                        rewordable: panel.canDecide && panel.bridge.canEdit
                        Layout.topMargin: 8
                        Layout.bottomMargin: 4
                        Layout.fillWidth: true
                        onSaved: wording => panel.bridge.reword(wording)
                        onCancelled: panel.bridge.cancelEdit()
                        onRewordAsked: at => {
                            panel.cursorAt = at;
                            panel.bridge.edit();
                        }
                    }
                    Lines {
                        objectName: "banners"
                        each: panel.detail.banners
                        Layout.fillWidth: true
                    }
                    Label {
                        objectName: "mark"
                        text: panel.detail.mark
                        textFormat: Text.PlainText
                        visible: text !== ""
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                    }

                    Heading {
                        text: qsTr("Why")
                        visible: panel.detail.reason !== ""
                    }
                    Label {
                        objectName: "reason"
                        text: panel.detail.reason
                        textFormat: Text.PlainText
                        visible: text !== ""
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                    }

                    Heading {
                        text: qsTr("Around it")
                        visible: panel.detail.before.length + panel.detail.after.length > 0
                    }
                    Lines {
                        objectName: "before"
                        each: panel.detail.before
                        opacity: 0.7
                        Layout.fillWidth: true
                    }
                    // The line itself between its neighbours, so that it shows which come before
                    // it.
                    Label {
                        objectName: "between"
                        text: panel.detail.now
                        textFormat: Text.RichText
                        visible: panel.detail.before.length + panel.detail.after.length > 0
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                    }
                    Lines {
                        objectName: "after"
                        each: panel.detail.after
                        opacity: 0.7
                        Layout.fillWidth: true
                    }

                    Label {
                        objectName: "episodes"
                        text: panel.detail.episodes
                        textFormat: Text.PlainText
                        visible: text !== ""
                        topPadding: 6
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                    }
                    Lines {
                        objectName: "timing"
                        each: panel.detail.timing
                        Layout.topMargin: 6
                        Layout.fillWidth: true
                    }

                    ToolButton {
                        id: technical

                        objectName: "technical"
                        text: (checked ? "▾ " : "▸ ") + qsTr("Technical details")
                        checkable: true
                        focusPolicy: Qt.NoFocus
                        Layout.topMargin: 6
                    }
                    Lines {
                        objectName: "technicalLines"
                        each: panel.detail.technical
                        visible: technical.checked
                        opacity: 0.7
                        Layout.fillWidth: true
                    }
                }

                // Only between the two: alone, the log has nothing above it to be set off from.
                Rectangle {
                    objectName: "episodeLogDivider"
                    color: panel.separator
                    implicitHeight: 1
                    visible: logHeading.visible && finding.visible
                    Layout.fillWidth: true
                    Layout.topMargin: 10
                }
                // Wrapped at the column's width: unwrapped, the heading is as wide as its words at
                // the least, the column lays out all it holds at least that wide, and the view cuts
                // every block of the finding at its right edge.
                Heading {
                    id: logHeading

                    objectName: "episodeLogHeading"
                    text: qsTr("What the check said of %1").arg(panel.bridge.episodeName)
                    textFormat: Text.PlainText
                    visible: panel.bridge.episodeName !== ""
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }
                Lines {
                    objectName: "episodeLog"
                    each: panel.bridge.episodeLog
                    Layout.fillWidth: true
                }
            }
        }
    }
}
