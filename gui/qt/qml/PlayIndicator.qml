// Over the video: whether it plays or is paused. Drawn from shapes, as a font can draw the media
// symbols as colour emoji; white on a dark backing, to read over any frame in any palette. It only
// shows the state, and a click goes through it.
import QtQuick

Rectangle {
    id: indicator

    required property bool paused
    // The state in a word, for a screen reader.
    readonly property string said: paused ? qsTr("Paused") : qsTr("Playing")

    implicitWidth: 28
    implicitHeight: 28
    radius: 4
    color: Qt.rgba(0, 0, 0, 0.55)
    Accessible.role: Accessible.Indicator
    Accessible.name: said

    Row {
        objectName: "pausedBars"
        anchors.centerIn: parent
        spacing: 4
        visible: indicator.paused

        Repeater {
            model: 2

            Rectangle {
                width: 4
                height: 14
                color: "white"
            }
        }
    }

    // A square turned on its corner, of which the clip keeps the half that points right.
    Item {
        objectName: "playingTriangle"
        anchors.centerIn: parent
        anchors.horizontalCenterOffset: 1
        width: 8
        height: 16
        clip: true
        visible: !indicator.paused

        Rectangle {
            x: -width / 2
            y: (parent.height - height) / 2
            width: parent.height / Math.SQRT2
            height: width
            rotation: 45
            antialiasing: true
            color: "white"
        }
    }
}
