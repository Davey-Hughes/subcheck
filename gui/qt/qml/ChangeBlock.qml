// One side of a change, as the details panel and the Settings window's sample draw it: a bar and a
// label in the side's tone, then the line with its change marked, on the style's alternate
// background. The label is written in capitals by its font, not in its text, so that a screen
// reader reads it as words. Its text sizes are the block's font's, which it takes from where it
// is put. The line can keep room for more lines than it has, so that what is below the block keeps
// its place from one line to the next.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Control {
    id: block

    // What the line is, "− In the subtitles now" or "+ With the fix" say; the line, marked as
    // DetailView marks it; and the tone of the side it is on.
    required property string label
    required property string line
    required property color tone
    // How many lines' room the line keeps at the least, at its own font, so that what is below the
    // block stays put for any line no longer than that.
    property int minLines: 1

    // The bar's width, which the left padding counts, so that the label and the line clear it.
    readonly property int barWidth: 3

    leftPadding: barWidth + 8
    rightPadding: 8
    topPadding: 6
    bottomPadding: 6

    background: Rectangle {
        color: block.palette.alternateBase
        topRightRadius: 4
        bottomRightRadius: 4

        Rectangle {
            objectName: "blockBar"
            width: block.barWidth
            height: parent.height
            color: block.tone
        }
    }

    contentItem: ColumnLayout {
        // The block's baseline is its label's, so that a label beside the block can sit level with
        // it.
        baselineOffset: blockLabel.y + blockLabel.baselineOffset
        spacing: 2

        Label {
            id: blockLabel

            objectName: "blockLabel"
            text: block.label
            textFormat: Text.PlainText
            color: block.tone
            font.bold: true
            font.capitalization: Font.AllUppercase
            font.pointSize: block.font.pointSize - 2
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }
        Label {
            id: blockLine

            objectName: "blockLine"
            text: block.line
            textFormat: Text.RichText
            font.pointSize: block.font.pointSize + 2
            wrapMode: Text.Wrap
            Layout.fillWidth: true
            // Each line of a rich text is laid out its font's line spacing tall, rounded up to a
            // whole pixel. The room is counted in the block's own font: a line drawn partly in a
            // fallback font, for kanji, an emoji or ♪, can be taller than that, and then moves what
            // is below by the difference.
            Layout.minimumHeight: block.minLines * Math.ceil(lineMetrics.lineSpacing) + topPadding
                                  + bottomPadding

            FontMetrics {
                id: lineMetrics

                font: blockLine.font
            }
        }
    }
}
