// The video's pane: mpv's video filling it, black until a finding is shown in it; over the video,
// whether it plays or is paused, and a busy animation while Show in Video waits for it, in the
// highlight colour (LoadingArc) on a backing in the window colour: a highlight can be as dark as
// the pane is until mpv draws there, and every palette sets its highlight apart from its window
// colour. The marks over the video are the pane's own, drawn in the video's window with it: what
// the main window draws over the place of a window it holds draws under that window, and with the
// video they go where it goes when it pops out. While the window's columns are dragged, the
// video's frame is stretched across its changing size, and drawn again at its size once the drag
// ends.
import QtQuick
import QtQuick.Controls
import Subcheck

// A Control, so that it takes the palette and the font it is given: a window of its own takes
// neither from the main window.
Control {
    id: pane

    required property SessionBridge bridge
    // Whether the window's columns are being dragged, which the video stretches its frame for.
    property bool stretching: false
    // As mpv says: whether a file shows, and whether it is paused.
    readonly property bool showing: video.showing
    readonly property bool paused: video.paused

    objectName: "videoPane"

    MpvItem {
        id: video

        objectName: "video"
        anchors.fill: parent
        player: pane.bridge.player
        stretching: pane.stretching
    }
    PlayIndicator {
        objectName: "playIndicator"
        anchors.top: parent.top
        anchors.right: parent.right
        anchors.margins: 8
        paused: video.paused
        visible: video.showing
    }
    // Only while the animation runs. The alpha is its colour's alone, as an opacity would fade the
    // animation with it; it takes no click, as the animation takes none.
    Rectangle {
        objectName: "videoLoadingBacking"
        anchors.centerIn: video
        width: loading.width + 16
        height: loading.height + 16
        radius: 6
        color: Qt.alpha(pane.palette.window, 0.85)
        visible: loading.running

        LoadingArc {
            id: loading

            objectName: "videoLoading"
            anchors.centerIn: parent
            running: pane.bridge.videoLoading
            Accessible.name: qsTr("Loading video")
        }
    }
}
