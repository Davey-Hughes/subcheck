// The video's pane: mpv's video above its control bar (ControlBar), black until a finding is shown
// in it; over the video, whether it plays or is paused, a busy animation while Show in Video waits
// for it, in the highlight colour (LoadingArc) on a backing in the window colour: a highlight can
// be as dark as the pane is until mpv draws there, and every palette sets its highlight apart from
// its window colour; and while it is black, a quiet line saying how to show a finding in it. The
// marks over the video and the bar are the pane's own, drawn in the video's window with it: what
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
    // For the control bar, from the window: whether the video's window is out of the details
    // panel, whether it can go out and come back, and whether the window's keys reach review.
    property bool out
    property bool canPopOut
    property bool keysToReview
    // As mpv says: whether a file shows, and whether it is paused.
    readonly property bool showing: video.showing
    readonly property bool paused: video.paused
    // The control bar's height, at the pane's width, and the least width that keeps each of its
    // groups whole: the room the video's place leaves for it.
    readonly property real barHeight: bar.implicitHeight
    readonly property real leastWidth: bar.leastWidth

    // The bar's Pop Out, or Pop In, was clicked.
    signal popAsked()

    // The control bar's height were the pane `width` wide.
    function barHeightAt(width: real): real {
        return bar.heightAt(width);
    }

    objectName: "videoPane"

    MpvItem {
        id: video

        objectName: "video"
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: bar.top
        player: pane.bridge.player
        stretching: pane.stretching
    }
    PlayIndicator {
        objectName: "playIndicator"
        anchors.top: video.top
        anchors.right: video.right
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
    // While the video is black, how to show a finding in it. A fixed white, as the pane is black in
    // every palette, where the palette's own text can be as dark as the pane. At the video's foot,
    // so that nothing is drawn over its middle, where the busy animation shows: wrapped, it keeps
    // inside the video's sides and grows up from its foot.
    Label {
        objectName: "quietLine"
        anchors.horizontalCenter: video.horizontalCenter
        anchors.bottom: video.bottom
        anchors.bottomMargin: 8
        width: Math.min(implicitWidth, video.width - 16)
        text: qsTr("Show in Video, or press O")
        textFormat: Text.PlainText
        color: Qt.rgba(1, 1, 1, 0.7)
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
        visible: !video.showing && !loading.running
    }
    ControlBar {
        id: bar

        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        bridge: pane.bridge
        // Nothing plays while nothing shows.
        paused: !video.showing || video.paused
        clock: video.clock
        out: pane.out
        canPopOut: pane.canPopOut
        keysToReview: pane.keysToReview
        onPopAsked: pane.popAsked()
    }
}
