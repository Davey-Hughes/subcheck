// The video's control bar, under the video in its window, in three groups: Play/Pause, with a mark
// and a word for whether the video plays, beside the time mpv last said; the seeks, back and
// forward 5 s and a frame each way; and Pop Out, Pop In while the video is out. Each button sends
// what the Video menu's item of its name sends. The bar wraps between its groups and never inside
// one, as a group split across lines reads as two controls; so its least width, which the details
// panel keeps, is its widest group's. Its buttons are ToolButtons, as Fusion's Button has a floor
// of 80 px that would make the bar wider than the panel at every size of text. None takes the
// keys, so that a click leaves them where they were.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Subcheck

Control {
    id: bar

    required property SessionBridge bridge
    // Whether the video is paused, or shows nothing that could play.
    required property bool paused
    // Where the video is, as mpv last said it; empty while it has no place to say.
    required property string clock
    // Whether the video's window is out of the details panel, where Pop In puts it back.
    required property bool out
    // Whether the window's keys reach review: the bar waits while Settings is open, as the Video
    // menu's items do.
    required property bool keysToReview
    // Whether the video's window can go out, and come back in.
    property bool canPopOut
    // The least width that shows each group whole: the widest group, on a line of its own.
    readonly property real leastWidth: Math.ceil(Math.max(playGroup.implicitWidth,
                                                          seekGroup.implicitWidth,
                                                          popGroup.implicitWidth))
                                       + leftPadding + rightPadding

    // Pop Out, or Pop In, was clicked: the window takes the video out or puts it back.
    signal popAsked()

    objectName: "controlBar"
    enabled: keysToReview
    leftPadding: 8
    rightPadding: 8
    topPadding: 6
    bottomPadding: 6

    background: Rectangle {
        color: bar.palette.window
    }

    // The mark as tall as the word's letters reach, so that it grows with the text.
    FontMetrics {
        id: wordFont

        font: playWord.font
    }
    // Room for the wider word, so that a toggle moves neither the button nor the clock.
    TextMetrics {
        id: pausedWord

        font: playWord.font
        text: qsTr("Paused")
    }
    TextMetrics {
        id: playingWord

        font: playWord.font
        text: qsTr("Playing")
    }
    // Room for any time under ten hours, so that the bar does not move as the time does.
    TextMetrics {
        id: clockRoom

        font: clockLabel.font
        text: "0:00:00.000"
    }

    contentItem: Flow {
        spacing: 6

        RowLayout {
            id: playGroup

            spacing: 2

            ToolButton {
                id: playButton

                objectName: "playButton"
                text: bar.paused ? qsTr("Paused") : qsTr("Playing")
                focusPolicy: Qt.NoFocus
                Accessible.name: qsTr("Play/Pause")
                onClicked: bar.bridge.pause()

                // The mark says the state, as the one in the video's corner does, and the word
                // makes plain that a click changes it.
                contentItem: RowLayout {
                    spacing: playButton.spacing

                    PlayGlyph {
                        paused: bar.paused
                        colour: playButton.palette.buttonText
                        size: wordFont.ascent
                    }
                    Label {
                        id: playWord

                        objectName: "playWord"
                        text: playButton.text
                        textFormat: Text.PlainText
                        font: playButton.font
                        color: playButton.palette.buttonText
                        Layout.preferredWidth: Math.ceil(Math.max(pausedWord.advanceWidth,
                                                                  playingWord.advanceWidth))
                    }
                }
            }
            Label {
                id: clockLabel

                objectName: "clockLabel"
                text: bar.clock
                textFormat: Text.PlainText
                // Figures of one width, where the font has them, so that every time takes the
                // room measured for one: a font whose figures differ in width would draw some
                // times wider than that.
                font.features: {
                    "tnum": 1
                }
                Layout.preferredWidth: Math.ceil(clockRoom.advanceWidth)
            }
        }
        RowLayout {
            id: seekGroup

            spacing: 2

            ToolButton {
                objectName: "back5Button"
                text: qsTr("−5 s")
                focusPolicy: Qt.NoFocus
                Accessible.name: qsTr("Back 5 s")
                onClicked: bar.bridge.seek(-5)
            }
            ToolButton {
                objectName: "previousFrameButton"
                text: qsTr("‹ Frame")
                focusPolicy: Qt.NoFocus
                Accessible.name: qsTr("Previous Frame")
                onClicked: bar.bridge.previousFrame()
            }
            ToolButton {
                objectName: "nextFrameButton"
                text: qsTr("Frame ›")
                focusPolicy: Qt.NoFocus
                Accessible.name: qsTr("Next Frame")
                onClicked: bar.bridge.nextFrame()
            }
            ToolButton {
                objectName: "forward5Button"
                text: qsTr("+5 s")
                focusPolicy: Qt.NoFocus
                Accessible.name: qsTr("Forward 5 s")
                onClicked: bar.bridge.seek(5)
            }
        }
        RowLayout {
            id: popGroup

            spacing: 2

            ToolButton {
                objectName: "popButton"
                text: bar.out ? qsTr("Pop In") : qsTr("Pop Out")
                enabled: bar.canPopOut
                focusPolicy: Qt.NoFocus
                onClicked: bar.popAsked()
            }
        }
    }
}
