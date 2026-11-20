// A list's scroll bar, in the style's own look, and the room its rows give way to while it shows,
// so that it covers none of their words. Its size is between 0 and 1 while the list is longer than
// its view, the one case every style shows a bar for; some styles keep the bar visible with nothing
// to scroll, and others hide it then, so both are read.
import QtQuick
import QtQuick.Controls

ScrollBar {
    id: bar

    // The width the rows leave free at the list's right edge. Set a moment after what it is read
    // from changes, never bound to it: a row the list makes lays out its height only as it is given
    // its width, which can take the list past its view, and a binding would then change the width
    // being set, which QML stops as a loop and leaves the rows at different widths.
    property real room: 0

    function place() {
        room = visible && size > 0 && size < 1 ? width : 0;
    }

    onSizeChanged: Qt.callLater(bar.place)
    onVisibleChanged: Qt.callLater(bar.place)
    onWidthChanged: Qt.callLater(bar.place)
}
