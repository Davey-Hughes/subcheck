// A Two Lines row of the findings: its badge, where it is and its decision on the first line; on
// the second its line with the change marked, or for a timing row whose shown members' fixes differ
// those fixes, counted; cut short with an ellipsis where it does not fit, and a marked line cut
// before the change when the whole line would hide it. A member of an expanded row sits in under
// its row. A click picks it; a double-click opens it.
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
    required property bool member

    signal picked
    signal opened

    implicitHeight: lines.implicitHeight + 10

    // Drawn nowhere: the width of the whole line as far as its change's end, in the fonts the line
    // is drawn in, the new words' bold among them.
    Label {
        id: reach

        objectName: "rowReach"
        visible: false
        text: row.lineReach
        textFormat: Text.StyledText
    }

    ColumnLayout {
        id: lines

        anchors.left: parent.left
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        anchors.leftMargin: row.member ? 28 : 8
        anchors.rightMargin: 8
        spacing: 2

        RowLayout {
            spacing: 6

            Rectangle {
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
                elide: Text.ElideRight
                Layout.fillWidth: true
            }
            Label {
                objectName: "rowState"
                text: row.glyph + " " + row.stateWord
                textFormat: Text.PlainText
                color: row.stateTone
            }
        }
        Label {
            objectName: "rowLine"
            text: reach.implicitWidth <= width ? row.line : row.cutLine
            textFormat: Text.StyledText
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
