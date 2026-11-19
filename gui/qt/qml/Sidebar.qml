// The sidebar: the store's reviews, what the list shows, and the view's episodes, each a list of
// review's that a click asks review about. The reviews scroll on their own, in a third of the
// sidebar at most, so that the filters and the episodes stay in view however many reviews the
// store holds; the episodes scroll in what is left, and on a short window keep room for a few
// entries, which the reviews give up. None takes the window's keys.
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

    // An entry: its name, with a count or a stage on its right; highlighted while it is shown.
    component Entry: ItemDelegate {
        id: entry

        property string side

        Layout.fillWidth: true
        focusPolicy: Qt.NoFocus
        contentItem: RowLayout {
            Label {
                text: entry.text
                textFormat: Text.PlainText
                elide: Text.ElideRight
                color: entry.highlighted ? entry.palette.highlightedText : entry.palette.text
                Layout.fillWidth: true
            }
            Label {
                objectName: "entrySide"
                text: entry.side
                textFormat: Text.PlainText
                opacity: 0.7
                color: entry.highlighted ? entry.palette.highlightedText : entry.palette.text
            }
        }
    }

    Heading {
        text: qsTr("Reviews")
    }
    ListView {
        objectName: "reviews"
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        model: sidebar.bridge.folders
        Layout.fillWidth: true
        // Filling, so that it can give way to the episodes' least; never past its own height.
        Layout.fillHeight: true
        Layout.preferredHeight: Math.min(contentHeight, sidebar.height / 3)
        Layout.maximumHeight: Layout.preferredHeight
        ScrollBar.vertical: ScrollBar {}

        delegate: Entry {
            id: review

            required property int index
            required property string name
            required property string path
            required property int count
            required property bool open

            objectName: "review" + index
            width: ListView.view.width
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
        objectName: "episodes"
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        model: sidebar.bridge.episodes
        Layout.fillWidth: true
        Layout.fillHeight: true
        Layout.minimumHeight: 3 * check.implicitHeight
        ScrollBar.vertical: ScrollBar {}

        delegate: Entry {
            required property int index
            required property string label
            required property string stage
            required property bool current

            objectName: "episode" + index
            width: ListView.view.width
            text: label
            side: stage
            highlighted: current
            onClicked: sidebar.bridge.openEpisode(index)
        }
    }
}
