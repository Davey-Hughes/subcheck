// The sidebar: the store's reviews, what the list shows, and the view's episodes, each a list of
// review's that a click asks review about; an episode the check found nothing in is greyed, and
// its word says so, so that the grey is never the only sign. The reviews scroll on their own, in
// a third of the sidebar at most, so that the filters and the episodes stay in view however many
// reviews the store holds; the episodes scroll in what is left, and on a short window keep room
// for a few entries, which the reviews give up. Each list keeps its mark in view, the open review
// and the marked episode, so that neither is one to scroll to. None takes the window's keys.
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Subcheck

ColumnLayout {
    id: sidebar

    required property SessionBridge bridge

    spacing: 0

    component Heading: Label {
        topPadding: 10
        leftPadding: 8
        bottomPadding: 2
        font.bold: true
        opacity: 0.7
    }

    // An entry: its name, with a count or a stage on its right; highlighted while it is shown, and
    // greyed while it has nothing to show, yet still clickable.
    component Entry: ItemDelegate {
        id: entry

        property string side
        property bool greyed
        // Both labels' colour: the highlight's own while highlighted, so that a greyed entry
        // shown stays readable on it.
        readonly property color ink: entry.highlighted ? entry.palette.highlightedText
            : entry.greyed ? entry.palette.disabled.text : entry.palette.text

        Layout.fillWidth: true
        focusPolicy: Qt.NoFocus
        contentItem: RowLayout {
            Label {
                text: entry.text
                textFormat: Text.PlainText
                elide: Text.ElideRight
                color: entry.ink
                Layout.fillWidth: true
            }
            // Dimmed beside the name, but for a greyed entry's: its word is the one sign of the
            // grey that is not a colour, and the disabled colour alone dims it enough.
            Label {
                objectName: "entrySide"
                text: entry.side
                textFormat: Text.PlainText
                opacity: entry.greyed ? 1 : 0.7
                color: entry.ink
            }
        }
    }

    Heading {
        text: qsTr("Reviews")
    }
    ListView {
        id: reviewList

        objectName: "reviews"
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        model: sidebar.bridge.folders
        Layout.fillWidth: true
        // Filling, so that it can give way to the episodes' least; never past its own height.
        Layout.fillHeight: true
        Layout.preferredHeight: Math.min(contentHeight, sidebar.height / 3)
        Layout.maximumHeight: Layout.preferredHeight
        ScrollBar.vertical: ListScrollBar {
            id: reviewsBar

            objectName: "reviewsScrollBar"
        }

        // The open review kept in view as review opens another or sends a new list, which a view
        // shows from its top; with none open, nothing is scrolled to. Placed as the marked episode
        // is, once review's change is applied, with the sidebar laid out first: the list's height
        // follows a new list's length only at the next frame, and the review is placed against the
        // height it will be drawn at.
        function showOpen() {
            sidebar.ensurePolished();
            const row = sidebar.bridge.folders.openRow();
            if (row >= 0) {
                reviewList.positionViewAtIndex(row, ListView.Contain);
            }
        }

        Connections {
            target: sidebar.bridge.folders

            function onOpenMoved() {
                Qt.callLater(reviewList.showOpen);
            }
        }

        delegate: Entry {
            id: review

            required property int index
            required property string name
            required property string path
            required property int count
            required property bool open

            objectName: "review" + index
            width: ListView.view.width - reviewsBar.room
            text: name
            side: count > 0 ? count : ""
            highlighted: open
            onClicked: sidebar.bridge.openFolder(index)

            // A tooltip of its own, as the attached one reads a path's angle brackets as markup.
            ToolTip {
                id: tip

                objectName: "reviewPath"
                text: review.path
                visible: review.hovered
                delay: 800

                contentItem: Text {
                    text: tip.text
                    textFormat: Text.PlainText
                    font: tip.font
                    color: tip.palette.toolTipText
                    wrapMode: Text.Wrap
                }
            }
        }
    }
    // Sub-project 4's way to start a check, drawn until it is built.
    Entry {
        id: check

        objectName: "checkAFolder"
        text: qsTr("Check a Folder…")
        enabled: false
    }

    Heading {
        text: qsTr("Show")
    }
    Repeater {
        model: sidebar.bridge.filters

        Entry {
            required property int index
            required property string name
            required property int count
            required property bool current

            objectName: "filter" + index
            text: name
            side: count
            highlighted: current
            onClicked: sidebar.bridge.setFilter(index)
        }
    }

    Heading {
        text: qsTr("Episodes")
    }
    ListView {
        id: episodeList

        objectName: "episodes"
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        model: sidebar.bridge.episodes
        Layout.fillWidth: true
        Layout.fillHeight: true
        Layout.minimumHeight: 3 * check.implicitHeight
        ScrollBar.vertical: ListScrollBar {
            id: episodesBar

            objectName: "episodesScrollBar"
        }

        // The marked episode kept in view as the View menu changes the order and as the mark
        // moves, so that a person keeps their place in the list and need not look for the episode
        // shown; with none marked, nothing is scrolled to. The sidebar is laid out first, and a
        // moved mark placed once review's change is applied whole: a new list of reviews in the
        // change, which can come after the mark, takes room from the episodes only at the next
        // frame, and the mark placed in the taller view would be left out of the shorter one.
        function showMarked() {
            sidebar.ensurePolished();
            const row = sidebar.bridge.episodes.markedRow();
            if (row >= 0) {
                episodeList.positionViewAtIndex(row, ListView.Contain);
            }
        }

        Connections {
            target: sidebar.bridge.episodes

            function onReordered() {
                episodeList.showMarked();
            }
            function onMarkMoved() {
                Qt.callLater(episodeList.showMarked);
            }
        }

        delegate: Entry {
            required property int index
            required property string label
            required property string stage
            required property bool current
            required property bool noFindings

            objectName: "episode" + index
            width: ListView.view.width - episodesBar.room
            text: label
            side: stage
            highlighted: current
            greyed: noFindings
            onClicked: sidebar.bridge.openEpisode(index)
        }
    }
}
