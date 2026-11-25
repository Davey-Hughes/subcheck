// Whether the video plays or is paused, as a mark: two bars while paused, a triangle pointing right
// while it plays. Drawn from shapes rather than a font's glyphs, as a font can draw the media
// symbols as colour emoji, which take no colour of the palette's and need not match the text
// beside them. As wide in either state, so that a change moves nothing beside it.
import QtQuick

Item {
    id: glyph

    required property bool paused
    property color colour: "white"
    // Its height, the triangle's; the bars are a little shorter, and both scale with it.
    property real size: 16

    implicitWidth: size * 3 / 4
    implicitHeight: size

    // In whole pixels, so that the two bars draw alike at any size.
    Row {
        objectName: "pausedBars"
        anchors.centerIn: parent
        spacing: Math.round(glyph.size / 4)
        visible: glyph.paused

        Rectangle {
            width: Math.round(glyph.size / 4)
            height: Math.round(glyph.size * 14 / 16)
            color: glyph.colour
        }
        Rectangle {
            width: Math.round(glyph.size / 4)
            height: Math.round(glyph.size * 14 / 16)
            color: glyph.colour
        }
    }

    // A square turned on its corner, of which the clip keeps the half that points right; set a
    // little right of the middle, where the eye takes a triangle's middle to be.
    Item {
        objectName: "playingTriangle"
        anchors.centerIn: parent
        anchors.horizontalCenterOffset: glyph.size / 16
        width: glyph.size / 2
        height: glyph.size
        clip: true
        visible: !glyph.paused

        Rectangle {
            x: -width / 2
            y: (parent.height - height) / 2
            width: parent.height / Math.SQRT2
            height: width
            rotation: 45
            antialiasing: true
            color: glyph.colour
        }
    }
}
