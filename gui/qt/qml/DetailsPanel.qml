// The details panel: the video, 16:9 at the panel's width, black until a finding is shown in it;
// then the selected finding as the window words it, with Reject and Accept Fix; its file's
// particulars under a disclosure; and what the check said of the episode selected, while that is
// the finding's. Nothing is drawn over the video.
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Subcheck

Item {
    id: panel

    required property SessionBridge bridge
    readonly property DetailView detail: bridge.detail

    component Heading: Label {
        topPadding: 6
        font.bold: true
        opacity: 0.7
    }

    component Lines: ColumnLayout {
        id: lines

        property list<string> each

        spacing: 0
        visible: each.length > 0

        Repeater {
            model: lines.each

            Label {
                required property string modelData

                text: modelData
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }
        }
    }

    // Another finding opens at its top; the same one, refreshed, stays where it was scrolled to.
    Connections {
        target: panel.bridge

        function onSelectionMoved() {
            (scroll.contentItem as Flickable).contentY = 0;
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Black behind the video, so that its place shows before mpv first draws there.
        Rectangle {
            color: "black"
            Layout.fillWidth: true
            Layout.preferredHeight: width * 9 / 16

            MpvItem {
                objectName: "video"
                anchors.fill: parent
                player: panel.bridge.player
            }
        }

        ScrollView {
            id: scroll

            objectName: "detailsScroll"
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            // Its lines wrap at the panel's width: left to the column, the content is as wide as
            // its longest line unwrapped, and scrolls sideways.
            contentWidth: availableWidth

            ColumnLayout {
                width: scroll.availableWidth - 16
                x: 8
                spacing: 2
                visible: panel.detail.shown

                RowLayout {
                    Layout.topMargin: 8
                    spacing: 6

                    Rectangle {
                        color: panel.detail.kindTone
                        radius: 3
                        implicitWidth: kind.implicitWidth + 10
                        implicitHeight: kind.implicitHeight + 2

                        Label {
                            id: kind

                            objectName: "detailKind"
                            anchors.centerIn: parent
                            text: panel.detail.kind
                            textFormat: Text.PlainText
                        }
                    }
                    Label {
                        objectName: "place"
                        text: panel.detail.place
                        textFormat: Text.PlainText
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                    Label {
                        objectName: "state"
                        text: panel.detail.glyph + " " + panel.detail.stateWord
                        textFormat: Text.PlainText
                        color: panel.detail.stateTone
                    }
                }
                Lines {
                    objectName: "banners"
                    each: panel.detail.banners
                    Layout.fillWidth: true
                }
                Label {
                    objectName: "mark"
                    text: panel.detail.mark
                    textFormat: Text.PlainText
                    visible: text !== ""
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }

                Heading {
                    objectName: "nowHeading"
                    text: qsTr("In the subtitles now")
                }
                Label {
                    objectName: "now"
                    text: panel.detail.now
                    textFormat: Text.RichText
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }
                Heading {
                    objectName: "fixedHeading"
                    text: panel.detail.edited ? qsTr("With your wording") : qsTr("With the fix")
                }
                Label {
                    objectName: "fixed"
                    text: panel.detail.fixed
                    textFormat: Text.RichText
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }

                Heading {
                    text: qsTr("Why")
                    visible: panel.detail.reason !== ""
                }
                Label {
                    objectName: "reason"
                    text: panel.detail.reason
                    textFormat: Text.PlainText
                    visible: text !== ""
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }

                Heading {
                    text: qsTr("Around it")
                    visible: panel.detail.before.length + panel.detail.after.length > 0
                }
                Lines {
                    objectName: "before"
                    each: panel.detail.before
                    opacity: 0.7
                    Layout.fillWidth: true
                }
                // The line itself between its neighbours, so that it shows which come before it.
                Label {
                    objectName: "between"
                    text: panel.detail.now
                    textFormat: Text.RichText
                    visible: panel.detail.before.length + panel.detail.after.length > 0
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }
                Lines {
                    objectName: "after"
                    each: panel.detail.after
                    opacity: 0.7
                    Layout.fillWidth: true
                }

                Label {
                    objectName: "episodes"
                    text: panel.detail.episodes
                    textFormat: Text.PlainText
                    visible: text !== ""
                    topPadding: 6
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }
                Lines {
                    objectName: "timing"
                    each: panel.detail.timing
                    Layout.topMargin: 6
                    Layout.fillWidth: true
                }

                RowLayout {
                    Layout.topMargin: 10
                    Layout.alignment: Qt.AlignRight

                    Button {
                        objectName: "rejectButton"
                        text: qsTr("Reject")
                        focusPolicy: Qt.NoFocus
                        onClicked: panel.bridge.reject()
                    }
                    Button {
                        objectName: "acceptButton"
                        text: qsTr("Accept Fix")
                        focusPolicy: Qt.NoFocus
                        highlighted: true
                        onClicked: panel.bridge.accept()
                    }
                }

                ToolButton {
                    id: technical

                    objectName: "technical"
                    text: (checked ? "▾ " : "▸ ") + qsTr("Technical details")
                    checkable: true
                    focusPolicy: Qt.NoFocus
                    Layout.topMargin: 6
                }
                Lines {
                    objectName: "technicalLines"
                    each: panel.detail.technical
                    visible: technical.checked
                    opacity: 0.7
                    Layout.fillWidth: true
                }

                Heading {
                    objectName: "episodeLogHeading"
                    text: qsTr("What the check said of %1").arg(panel.bridge.episodeName)
                    textFormat: Text.PlainText
                    visible: panel.bridge.episodeName !== ""
                }
                Lines {
                    objectName: "episodeLog"
                    each: panel.bridge.episodeLog
                    Layout.fillWidth: true
                }
            }
        }
    }
}
