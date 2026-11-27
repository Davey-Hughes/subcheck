// One side of a change, as the details panel and the Settings window's sample draw it: a bar and a
// label in the side's tone, then the line with its change marked, on the style's alternate
// background. The label is written in capitals by its font, not in its text, so that a screen
// reader reads it as words. Its text sizes are the block's font's, which it takes from where it
// is put. The line can keep room for more lines than it has, so that what is below the block keeps
// its place from one line to the next. While the block edits, the line gives way to a field for
// the wording, in the line's font, over Save and Accept and Cancel, which take no keys from it.
// A click anywhere on a block that can be reworded asks for its edit, at the line's character
// nearest the click.
// The line is rich text, and a Text cannot tell which character is under a point, so an unseen
// TextEdit lays the same text out as the line does and tells it. The drawn line's characters are
// the wording's one for one, as core collapses a line's spaces, but for a wording typed with a run
// of spaces, which draws as one: past it, a click lands early by the spaces the drawing dropped.
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
    // Whether the field shows in the line's place. What shows the block sets the field's text; the
    // block says only what the person chose: the wording saved, by Save and Accept, Return or
    // Enter, or the edit cancelled, by Cancel or Esc.
    property bool editing: false
    readonly property alias field: editField
    // Whether a click on the block asks for its edit, as rewordAsked says, while it is not editing;
    // a screen reader is told of the line as a button then.
    property bool rewordable: false
    // The block's width at the narrowest the window makes it, and how far below the block's top the
    // first line of its line ends at that width: its top padding, its label as it wraps there, the
    // room under the label and a line of the line's own font. From the label's words and the fonts
    // alone, never the line shown, so that it is the same for every line and for none, whether the
    // block shows or not: the window keeps room for it under the video.
    property real leastWidth: width
    readonly property real leastFirstLineFoot: topPadding + leastLabel.implicitHeight
                                               + column.spacing + blockLine.topPadding
                                               + Math.ceil(lineMetrics.lineSpacing)

    signal saved(string wording)
    signal cancelled
    // A click on the block asked for its edit: `at` is the index in the line of the character
    // nearest the click, -1 for the line's end.
    signal rewordAsked(int at)

    // The bar's width, which the left padding counts, so that the label and the line clear it.
    readonly property int barWidth: 3

    // Review has answered the edit a click on the block asked for with none open, as under a
    // question, which the Edit only lets go: that ends the click's ask as an edit opened ends it,
    // so that the next click on the block asks again, however soon after the last it comes.
    function answered() {
        rewordArea.answeredSinceAsk = true;
    }

    leftPadding: barWidth + 8
    rightPadding: 8
    topPadding: 6
    bottomPadding: 6

    onEditingChanged: {
        if (block.editing) {
            rewordArea.answeredSinceAsk = true;
        }
    }

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
        id: column

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
        // The line, and the field that takes its place while the block edits, in a slot as tall as
        // the line at the least, so that what is under the slot starts no higher while the block
        // edits than under the line.
        Item {
            id: lineSlot

            implicitWidth: block.editing ? editField.implicitWidth : blockLine.implicitWidth
            implicitHeight: Math.max(blockLine.height, block.editing ? editField.implicitHeight : 0)
            Layout.fillWidth: true

            Label {
                id: blockLine

                objectName: "blockLine"
                width: lineSlot.width
                // Each line of a rich text is laid out its font's line spacing tall, rounded up to
                // a whole pixel. The room is counted in the block's own font: a line drawn partly
                // in a fallback font, for kanji, an emoji or ♪, can be taller than that, and then
                // moves what is below by the difference.
                height: Math.max(implicitHeight, block.minLines * Math.ceil(lineMetrics.lineSpacing)
                                 + topPadding + bottomPadding)
                text: block.line
                textFormat: Text.RichText
                font.pointSize: block.font.pointSize + 2
                wrapMode: Text.Wrap
                visible: !block.editing

                FontMetrics {
                    id: lineMetrics

                    font: blockLine.font
                }
                // The line laid out again, unseen, in the same place: a TextEdit lays rich text out
                // as the Label does, given the same text, font, width, wrap and padding, and tells
                // the character under a point. It takes no input, and a screen reader is told only
                // of the line.
                TextEdit {
                    id: lineLayout

                    objectName: "lineLayout"
                    width: blockLine.width
                    text: blockLine.text
                    textFormat: TextEdit.RichText
                    font: blockLine.font
                    wrapMode: TextEdit.Wrap
                    leftPadding: blockLine.leftPadding
                    rightPadding: blockLine.rightPadding
                    topPadding: blockLine.topPadding
                    bottomPadding: blockLine.bottomPadding
                    readOnly: true
                    enabled: false
                    visible: false
                    Accessible.ignored: true
                }
                // What a screen reader is told of the click: a button over the line.
                Item {
                    objectName: "rewordButton"
                    anchors.fill: parent
                    visible: rewordArea.visible

                    Accessible.role: Accessible.Button
                    Accessible.name: qsTr("Edit Wording…")
                    // A hidden item is still in what a screen reader is told of.
                    Accessible.ignored: !visible
                    Accessible.onPressAction: block.rewordAsked(-1)
                }
            }
            KeyField {
                id: editField

                objectName: "editField"
                width: lineSlot.width
                font: blockLine.font
                visible: block.editing
                // A hidden item keeps the keys it has, and the window's letters would type into it
                // once the edit ends; a disabled one gives them up.
                enabled: block.editing
                onEntered: block.saved(editField.text)
                onEscaped: block.cancelled()
            }
        }
        // Save and Accept and Cancel keep the block's bottom padding above them too, below the
        // slot: they then start below every point of the block as it was before the edit opened,
        // the padding under the line included, so that the second click of a double-click on the
        // block, which comes once the edit has opened, lands on the field or empty room, never on
        // a button.
        RowLayout {
            visible: block.editing
            spacing: 6
            Layout.topMargin: block.bottomPadding

            Button {
                objectName: "saveButton"
                text: qsTr("Save and Accept")
                highlighted: true
                focusPolicy: Qt.NoFocus
                onClicked: block.saved(editField.text)
            }
            Button {
                objectName: "cancelButton"
                text: qsTr("Cancel")
                focusPolicy: Qt.NoFocus
                onClicked: block.cancelled()
            }
        }
    }

    // The label laid out again, unseen, at the width it has were the block `leastWidth` wide.
    Label {
        id: leastLabel

        width: Math.max(0, block.leastWidth - block.leftPadding - block.rightPadding)
        text: blockLabel.text
        textFormat: blockLabel.textFormat
        font: blockLabel.font
        wrapMode: blockLabel.wrapMode
        visible: false
        Accessible.ignored: true
    }

    // Over the whole block, its bar, its label and the padding around its line as well as the
    // line. Gone while the block edits, as the field takes its own clicks. Left clicks only: a
    // right-click is still the finding's menu. Only the first tap of a double-click asks, as a
    // double-click on words to select them is still one click on the fix: should the second come
    // before review has answered the first, it would ask again.
    Item {
        id: rewordArea

        // Whether review has answered since a tap last asked for an edit, by opening one or with
        // none (answered). The tap handler counts a tap soon after the last as a double-click's
        // second even with an answer between the two, and that tap asks all the same.
        property bool answeredSinceAsk: false

        // The index in the line of the character nearest `where`, in the area: the drawn line's at
        // that height; above the text, as the label is, the first drawn line's at that x; below it,
        // as the padding under the line is, the last's; beside a drawn line, its start or end. The
        // point is moved into the text first, as the layout puts a point above or below the text
        // at the line's start or end, whatever its x.
        function characterAt(where: point): int {
            const at = rewordArea.mapToItem(lineLayout, where);
            const first = lineLayout.positionToRectangle(0);
            const last = lineLayout.positionToRectangle(lineLayout.length);
            const y = Math.max(first.y + first.height / 2,
                               Math.min(at.y, last.y + last.height / 2));
            return lineLayout.positionAt(at.x, y);
        }

        objectName: "rewordArea"
        anchors.fill: parent
        visible: block.rewordable && !block.editing

        // Told of by rewordButton over the line alone: a button over the whole block would sit on
        // the label too, and a screen reader's pointer review of the label would meet the button
        // instead of its words.
        Accessible.ignored: true

        HoverHandler {
            cursorShape: Qt.IBeamCursor
        }
        TapHandler {
            id: rewordTap

            acceptedButtons: Qt.LeftButton
            // A click held however long is still a click, which a long press would swallow.
            longPressThreshold: 0
            onTapped: eventPoint => {
                if (rewordTap.tapCount === 1 || rewordArea.answeredSinceAsk) {
                    rewordArea.answeredSinceAsk = false;
                    block.rewordAsked(rewordArea.characterAt(eventPoint.position));
                }
            }
        }
    }
}
