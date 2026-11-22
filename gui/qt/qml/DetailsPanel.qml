// The details panel: the video, 16:9 at the panel's width, black until a finding is shown in it;
// then the selected finding as the window words it: its line as it is now above its line with the
// fix, each a block of its own that keeps its place from one finding to the next for a line of one
// or two lines at the block's own font (one of three or more, or one drawn partly in a taller
// fallback font, moves what is below it down), as Reject and Accept keep theirs in the toolbar,
// beside Undo; below them its banners, its mark and why it was found; its file's particulars under
// a disclosure; and what the check said of the episode selected while that is the finding's: below
// the finding, set off by a line, or alone with none selected. For a click on an episode the check
// found nothing in, its log shows alone, in the finding's place, while review keeps the finding
// selected: below the finding, the two would read as one, and the log is not the finding's. Over
// the video, whether it plays or is paused, and a busy animation while Show in Video waits for it,
// in the highlight colour (LoadingArc) on a backing in the window's colour: a highlight can be as
// dark as the pane is until mpv draws there, and every palette sets its highlight apart from its
// window colour. While the window's columns are dragged, the video's frame is stretched across its
// changing size, and drawn again at its size once the drag ends.
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
    // Whether the window's columns are being dragged, which the video stretches its frame for.
    property bool columnsResizing: false
    // The window's line between parts, drawn here between the finding and its episode's log.
    required property color separator
    // Whether a greyed episode's log shows alone, in the finding's place. Its end can move no
    // selection, as a click on the finding's own row does not, so the finding comes back at its
    // top here, as another finding opens at its own, rather than as far down as the log was.
    readonly property bool showingEpisode: bridge.showingEpisode

    onShowingEpisodeChanged: {
        if (!panel.showingEpisode) {
            (scroll.contentItem as Flickable).contentY = 0;
        }
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

    // Another finding opens at its top; the same one, refreshed, stays where it was scrolled to.
    // What the check said of an episode with no findings, shown for a click on it, is scrolled to:
    // the panel can be scrolled down the finding it takes the place of.
    Connections {
        target: panel.bridge

        function onSelectionMoved() {
            (scroll.contentItem as Flickable).contentY = 0;
        }
        function onEpisodeShown() {
            panel.scrollToLog();
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Black behind the video, so that its place shows before mpv first draws there.
        Rectangle {
            color: "black"
            Layout.fillWidth: true
            Layout.preferredHeight: width * 9 / 16

            MpvItem {
                id: video

                objectName: "video"
                anchors.fill: parent
                player: panel.bridge.player
                stretching: panel.columnsResizing
            }
            PlayIndicator {
                objectName: "playIndicator"
                anchors.top: parent.top
                anchors.right: parent.right
                anchors.margins: 8
                paused: video.paused
                visible: video.showing
            }
            // Only while the animation runs. The alpha is its colour's alone, as an opacity would
            // fade the animation with it; it takes no click, as the animation takes none.
            Rectangle {
                objectName: "videoLoadingBacking"
                anchors.centerIn: video
                width: loading.width + 16
                height: loading.height + 16
                radius: 6
                color: Qt.alpha(panel.palette.window, 0.85)
                visible: loading.running

                LoadingArc {
                    id: loading

                    objectName: "videoLoading"
                    anchors.centerIn: parent
                    running: panel.bridge.videoLoading
                    Accessible.name: qsTr("Loading video")
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

                    RowLayout {
                        Layout.topMargin: 8
                        spacing: 6

                        Rectangle {
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
                        Label {
                            objectName: "state"
                            text: panel.detail.glyph + " " + panel.detail.stateWord
                            textFormat: Text.PlainText
                            color: panel.detail.stateTone
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
                        objectName: "fixedBlock"
                        label: panel.detail.edited ? qsTr("+ With your wording")
                                                   : qsTr("+ With the fix")
                        line: panel.detail.fixed
                        tone: panel.detail.newTone
                        minLines: 2
                        Layout.topMargin: 8
                        Layout.bottomMargin: 4
                        Layout.fillWidth: true
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
                Heading {
                    id: logHeading

                    objectName: "episodeLogHeading"
                    text: qsTr("What the check said of %1").arg(panel.bridge.episodeName)
                    textFormat: Text.PlainText
                    visible: panel.bridge.episodeName !== ""
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
