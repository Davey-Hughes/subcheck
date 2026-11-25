// A Compact row of the findings, on one line: a row of several's triangle, which opens and closes
// it; its state's glyph, where it is in short, and its change as the list words it
// (findings_model.h). Where the line does not fit, the change is cut short with an ellipsis, and
// the place only once the change has no room left. A member of an expanded row has no triangle,
// and sits in from its row's parts. A click picks it; a double-click opens it; a right-click asks
// for its menu.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: row

    required property int index
    required property string glyph
    required property color stateTone
    required property string shortPlace
    required property string change
    required property bool member
    required property bool expandable
    required property bool expanded

    // Where the row's parts start: a row's after its triangle's column, and a member's further in
    // by the triangle and the gap after it, so that it sits in from its row's at every size of
    // text.
    readonly property int indent: disclosure.width + (member ? disclosure.glyphWidth + 6 : 0)

    signal picked
    signal opened
    signal disclosed
    signal menuAsked(point at)

    implicitHeight: line.implicitHeight + 10
    ContextMenu.onRequested: position => row.menuAsked(position)

    Disclosure {
        id: disclosure

        anchors.top: parent.top
        anchors.bottom: parent.bottom
        visible: !row.member
        shown: row.expandable
        open: row.expanded
        onToggled: row.disclosed()
    }

    RowLayout {
        id: line

        anchors.left: parent.left
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        anchors.leftMargin: row.indent
        anchors.rightMargin: 8
        spacing: 6

        Label {
            objectName: "rowGlyph"
            text: row.glyph
            textFormat: Text.PlainText
            color: row.stateTone
        }
        Label {
            objectName: "rowShortPlace"
            text: row.shortPlace
            textFormat: Text.PlainText
            // No wider than its words, up to a whole pixel, as the layout rounds a width down, and
            // a fraction short elides.
            elide: Text.ElideRight
            Layout.fillWidth: true
            Layout.maximumWidth: Math.ceil(implicitWidth)
        }
        Label {
            objectName: "rowChange"
            text: row.change
            textFormat: Text.StyledText
            elide: Text.ElideRight
            // Asks for no width of its own, so that a row too narrow for the whole line takes the
            // room from the change before the place.
            Layout.fillWidth: true
            Layout.preferredWidth: 0
        }
    }

    TapHandler {
        // A click held however long is still a click, which a long press would swallow.
        longPressThreshold: 0
        onTapped: row.picked()
        onDoubleTapped: row.opened()
    }
}
