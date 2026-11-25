// A Two Lines or a Rich row of the findings. Two Lines: its badge, where it is and its decision on
// the first line; on the second its line with the change marked, or for a timing row whose shown
// members' fixes differ those fixes, counted; cut short with an ellipsis where it does not fit, and
// a marked line cut before the change when the whole line would hide it. Rich adds a third line,
// why it was found, and at the row's left a tile with its line drawn on it as the video would show
// it. Before all of it, a row of several's triangle, which opens and closes it; a member of an
// expanded row has none, and sits in from its row's parts. A click picks it; a double-click opens
// it; a right-click asks for its menu.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: row

    required property int index
    required property string kind
    required property color kindTone
    required property string several
    required property string place
    required property string glyph
    required property string stateWord
    required property color stateTone
    required property string line
    required property string cutLine
    required property string lineReach
    required property string reason
    required property string tileLine
    required property bool member
    required property bool expandable
    required property bool expanded

    // Whether it is drawn Rich: with its reason and its tile.
    property bool rich: false
    // The window's colour for the lines between its parts, which edges the tile.
    required property color separator

    // Where the row's parts start: a row's after its triangle's column, and a member's further in
    // by the triangle and the gap after it, so that it sits in from its row's at every size of
    // text.
    readonly property int indent: disclosure.width + (member ? disclosure.glyphWidth + 6 : 0)

    signal picked
    signal opened
    signal disclosed
    signal menuAsked(point at)

    implicitHeight: lines.implicitHeight + 10
    ContextMenu.onRequested: position => row.menuAsked(position)

    // Drawn nowhere: the width of the whole line as far as its change's end, in the fonts the line
    // is drawn in, the new words' bold among them.
    Label {
        id: reach

        objectName: "rowReach"
        visible: false
        text: row.lineReach
        textFormat: Text.StyledText
    }

    Disclosure {
        id: disclosure

        anchors.top: parent.top
        anchors.bottom: parent.bottom
        visible: !row.member
        shown: row.expandable
        open: row.expanded
        onToggled: row.disclosed()
    }

    // Drawn, not taken from the video: dark in every window, as a frame of the video would be, and
    // its line at its foot, where a subtitle sits. 16:9 and as tall as the lines beside it, but no
    // wider than a quarter of the row, nor than leaves the first line its badge and its whole
    // state, which give way last, so that a narrow list keeps its room for the lines.
    Rectangle {
        id: tile

        objectName: "rowTile"
        visible: row.rich
        x: row.indent
        anchors.verticalCenter: parent.verticalCenter
        // Whole pixels for the first line, as its layout rounds each part's width up.
        width: Math.max(0, Math.min(lines.height * 16 / 9, row.width / 4,
                                    row.width - row.indent - 8 - Math.ceil(badge.implicitWidth)
                                    - Math.ceil(decision.implicitWidth) - 8))
        height: width * 9 / 16
        color: "#202020"
        border.width: 1
        border.color: row.separator

        Label {
            objectName: "rowTileLine"
            anchors.fill: parent
            anchors.margins: 4
            text: row.tileLine
            textFormat: Text.StyledText
            color: "#f0f0f0"
            // Smaller than the list's size, which every other label of the row takes, as a
            // subtitle is small in its frame.
            font.pointSize: reach.font.pointSize - 2
            wrapMode: Text.Wrap
            maximumLineCount: 3
            elide: Text.ElideRight
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignBottom
        }
    }

    ColumnLayout {
        id: lines

        anchors.left: parent.left
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        anchors.leftMargin: row.indent + (row.rich ? tile.width + 8 : 0)
        anchors.rightMargin: 8
        spacing: 2

        // The gap before each label is its own padding rather than the layout's spacing, so that a
        // label squeezed to nothing gives up its gap too, and the state keeps its room beside
        // Rich's tile in a narrow list.
        RowLayout {
            spacing: 0

            Rectangle {
                id: badge

                objectName: "badge"
                color: row.kindTone
                radius: 3
                implicitWidth: kind.implicitWidth + 10
                implicitHeight: kind.implicitHeight + 2

                Label {
                    id: kind

                    objectName: "rowKind"
                    anchors.centerIn: parent
                    text: row.kind
                    textFormat: Text.PlainText
                }
            }
            Label {
                objectName: "rowSeveral"
                text: row.several
                textFormat: Text.PlainText
                visible: text !== ""
                leftPadding: 6
                opacity: 0.7
                // Gives way on a narrow row, so that the state's glyph and word stay in view. Up to
                // a whole pixel, as the layout rounds a width down, and a fraction short elides.
                elide: Text.ElideRight
                Layout.fillWidth: true
                Layout.maximumWidth: Math.ceil(implicitWidth)
            }
            Label {
                objectName: "rowPlace"
                text: row.place
                textFormat: Text.PlainText
                leftPadding: 6
                elide: Text.ElideRight
                Layout.fillWidth: true
            }
            Label {
                id: decision

                objectName: "rowState"
                text: row.glyph + " " + row.stateWord
                textFormat: Text.PlainText
                leftPadding: 6
                color: row.stateTone
                // Whole wherever the badge leaves it room, which the count and the place give up
                // to it first; in a row too narrow for that, its word is cut short, never its
                // glyph.
                elide: Text.ElideRight
                Layout.preferredWidth: Math.max(leftPadding + Math.ceil(glyphRoom.advanceWidth),
                                                Math.min(Math.ceil(implicitWidth),
                                                         lines.width
                                                         - Math.ceil(badge.implicitWidth)))

                TextMetrics {
                    id: glyphRoom

                    font: decision.font
                    text: row.glyph + " …"
                }
            }
        }
        Label {
            objectName: "rowLine"
            text: reach.implicitWidth <= width ? row.line : row.cutLine
            textFormat: Text.StyledText
            elide: Text.ElideRight
            Layout.fillWidth: true
        }
        Label {
            objectName: "rowReason"
            visible: row.rich
            text: row.reason
            textFormat: Text.PlainText
            opacity: 0.7
            elide: Text.ElideRight
            Layout.fillWidth: true
        }
    }

    TapHandler {
        // A click held however long is still a click, which a long press would swallow.
        longPressThreshold: 0
        onTapped: row.picked()
        onDoubleTapped: row.opened()
    }
}
