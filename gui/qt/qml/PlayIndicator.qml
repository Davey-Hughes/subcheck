// Over the video: whether it plays or is paused, its mark white on a dark backing, to read over any
// frame in any palette. It only shows the state, and a click goes through it.
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

    PlayGlyph {
        anchors.centerIn: parent
        paused: indicator.paused
    }
}
