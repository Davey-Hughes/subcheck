// The column at a row's left with the triangle that opens a row of several into its members and
// closes it: "▸" closed, "▾" open. The whole column, as tall as the row and from its edge to its
// parts, takes the click, so that it is more than the glyph's few pixels to hit; and a screen
// reader names it as Finding ▸ Show Each Episode, checked while the row is open. Its clicks are its
// own, never the row's, whose handler would take two of them for a double-click, which opens the
// video. A row of one keeps the column, so that every row's parts start in one place, but draws
// nothing there and takes no click, which then picks the row as anywhere on it, and a screen reader
// is told nothing of it.
import QtQuick
import QtQuick.Controls

Item {
    id: disclosure

    // Whether the row is open, and whether it opens at all.
    property bool open: false
    property bool shown: true
    // The glyph drawn.
    readonly property alias text: glyph.text
    // The glyph's width, as wide closed as open, so that the row's parts stay where they are as it
    // opens and closes.
    readonly property int glyphWidth: Math.ceil(Math.max(closedGlyph.advanceWidth,
                                                         openGlyph.advanceWidth))

    signal toggled

    objectName: "rowDisclosure"
    // The row's margin, the glyph, and the gap before the row's parts.
    implicitWidth: 8 + glyphWidth + 6
    implicitHeight: glyph.implicitHeight
    opacity: shown ? 1 : 0

    Accessible.role: Accessible.Button
    Accessible.name: qsTr("Show Each Episode")
    Accessible.checkable: true
    Accessible.checked: open
    Accessible.ignored: !shown
    Accessible.onPressAction: {
        if (disclosure.shown) {
            disclosure.toggled();
        }
    }

    Label {
        id: glyph

        x: 8
        width: disclosure.glyphWidth
        anchors.verticalCenter: parent.verticalCenter
        text: disclosure.open ? "▾" : "▸"
        textFormat: Text.PlainText
        horizontalAlignment: Text.AlignHCenter
        // The column is named; the glyph read out as well would say "▸" beside it.
        Accessible.ignored: true
    }

    TextMetrics {
        id: closedGlyph

        font: glyph.font
        text: "▸"
    }
    TextMetrics {
        id: openGlyph

        font: glyph.font
        text: "▾"
    }

    // Takes the press, so that the row's handler never sees it, and gives each click of a double
    // one its own toggle. Not a TapHandler: one that takes the press within its bounds gets no tap
    // at all inside the list, a Flickable, and one that does not shares each tap with the row's.
    MouseArea {
        anchors.fill: parent
        enabled: disclosure.shown
        onClicked: disclosure.toggled()
    }
}
