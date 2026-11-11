// The window: the findings above the status line, and the fatal end's dialog.
// Layout and bindings only; what a key or a click does is the bridge's (session_bridge.cpp).
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import Subcheck

ApplicationWindow {
    id: window

    required property SessionBridge bridge

    width: 1000
    height: 600
    visible: true
    title: bridge.title

    // Review decides when the window ends: closing it is quitting.
    onClosing: close => {
        close.accepted = false;
        bridge.quit();
    }

    ListView {
        id: list

        objectName: "findings"
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: statusLine.top
        clip: true
        focus: true
        model: window.bridge.findings
        currentIndex: window.bridge.selected
        highlightMoveDuration: 0
        highlight: Rectangle {
            // A tint, so that the selected row's decision colour still reads over it, and an
            // outline at full strength, so that the selection shows on a pale highlight too.
            color: Qt.alpha(window.palette.highlight, 0.3)
            border.color: window.palette.highlight
            border.width: 1
        }
        Keys.onPressed: event => event.accepted = window.bridge.key(event.key, event.text)

        delegate: Text {
            required property int index
            required property string line
            required property string decision

            width: ListView.view.width
            padding: 2
            text: line
            font.family: "monospace"
            color: decision === "accepted" ? "forestgreen"
                 : decision === "rejected" ? "firebrick"
                 : decision === "mixed" ? "darkgoldenrod" : window.palette.text

            TapHandler {
                onTapped: window.bridge.select(parent.index)
            }
        }
    }

    Label {
        objectName: "empty"
        anchors.centerIn: list
        visible: window.bridge.empty
        text: qsTr("No findings in this run")
    }

    Label {
        id: statusLine

        objectName: "statusLine"
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        padding: 4
        text: window.bridge.statusLine
    }

    Dialog {
        objectName: "fatal"
        anchors.centerIn: parent
        // A width of its own: left to size itself, the dialog's width and its label's loop.
        width: Math.min(window.width - 40, 640)
        modal: true
        title: qsTr("subcheck")
        standardButtons: Dialog.Ok
        visible: window.bridge.fatal !== ""
        onClosed: window.bridge.dismiss()

        Label {
            anchors.fill: parent
            text: window.bridge.fatal
            wrapMode: Text.Wrap
        }
    }
}
