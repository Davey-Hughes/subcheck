// The Find box at the toolbar's right: a field for the words, and beside it ‹ and › for the row
// before and after the selected one that holds them. Return or Enter in the field is ›, and with
// Shift ‹. A search that finds nothing says "No match" inside the field's right end, until the
// words change or the field takes the keys again, by Edit ▸ Find…, a click, Tab or Shift+Tab;
// words that are empty, or spaces alone, are no search, and say nothing. Esc, or a press of the
// pointer anywhere else in the window while no menu is open (press_away.h), gives the window its
// keys back and keeps the words. Always there, so that nothing moves as it is used. Greyed while
// an edit is open: from its field, Return would select another row, which ends the edit and
// drops its typed wording; disabled, the field gives up the keys. Its buttons take no keys from
// the field.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Subcheck

Control {
    id: box

    required property SessionBridge bridge
    // Whether the last search found nothing; reset as the words change and as the field takes the
    // keys.
    property bool missed: false
    // The least width that keeps each part whole: the field's room for 8 characters and "No
    // match", and the two buttons.
    readonly property real leastWidth: field.Layout.minimumWidth + previous.implicitWidth
                                       + next.implicitWidth + 2 * parts.spacing + box.leftPadding
                                       + box.rightPadding

    // Gives the field the keys, its words selected, so that new words typed take their place.
    function take() {
        box.missed = false;
        field.selectAll();
        field.forceActiveFocus();
    }

    // Gives the window its keys back from the field, which keeps its words.
    function release() {
        field.focus = false;
    }

    // Selects the row after the selected one that holds the words, or before it with `back`; with
    // none, says so. Words that are empty, or spaces alone, are no search, and say nothing: the
    // list's find matches no row for them, which would say "No match" of words never typed.
    function search(back: bool) {
        if (field.text.trim() === "") {
            return;
        }
        box.missed = !box.bridge.findNext(field.text, back);
    }

    enabled: !box.bridge.editing
    // No room of its own around its parts, as the toolbar's row has its margins: each side set, as
    // a style's Control can set each over `padding` (Breeze's does).
    leftPadding: 0
    rightPadding: 0
    topPadding: 0
    bottomPadding: 0

    FontMetrics {
        id: metrics

        font: field.font
    }

    // A field of the style's own, never shown: its right padding is the style's inset at a field's
    // right, which the Find field's own is not, as it adds the room "No match" takes. Not the
    // field's left padding mirrored: a style can set the two apart, as Basic's and Universal's do.
    TextField {
        id: plain

        objectName: "plainField"
        font: field.font
        visible: false
        Accessible.ignored: true
    }

    contentItem: RowLayout {
        id: parts

        spacing: 2

        KeyField {
            id: field

            objectName: "findField"
            placeholderText: qsTr("Find")
            Accessible.name: qsTr("Find")
            // Room for 24 of its font's average characters, and at least for 8 and "No match"
            // beside them, so that the box keeps one width whether or not "No match" shows.
            Layout.preferredWidth: Math.ceil(24 * metrics.averageCharacterWidth)
                                   + field.leftPadding + plain.rightPadding
            Layout.minimumWidth: Math.ceil(8 * metrics.averageCharacterWidth + 6
                                           + noMatch.implicitWidth) + field.leftPadding
                                 + plain.rightPadding
            Layout.fillWidth: true
            // The style's inset, and while "No match" shows the room it takes and 6 px more, so
            // that the words end short of it rather than run under it.
            rightPadding: plain.rightPadding + (box.missed ? noMatch.implicitWidth + 6 : 0)
            onEntered: back => box.search(back)
            onEscaped: box.release()
            onTextChanged: box.missed = false
            // A click, Tab or Shift+Tab that gives the field the keys ends the last search's "No
            // match", as Ctrl+F does: the person comes back to the box. The keys it gets back as a
            // menu over it closes, or as its window is active again, do not: nothing was typed or
            // searched since.
            onActiveFocusChanged: {
                if (field.activeFocus && [Qt.MouseFocusReason, Qt.TabFocusReason,
                                          Qt.BacktabFocusReason].includes(field.focusReason)) {
                    box.missed = false;
                }
            }

            Label {
                id: noMatch

                objectName: "noMatch"
                text: qsTr("No match")
                // The field's own colour, as it is drawn on the field's background.
                color: field.color
                visible: box.missed
                anchors.right: parent.right
                anchors.rightMargin: plain.rightPadding
                anchors.verticalCenter: parent.verticalCenter
            }
        }
        ToolButton {
            id: previous

            objectName: "findPrevious"
            text: "‹"
            Accessible.name: qsTr("Previous Match")
            focusPolicy: Qt.NoFocus
            onClicked: box.search(true)
        }
        ToolButton {
            id: next

            objectName: "findNext"
            text: "›"
            Accessible.name: qsTr("Next Match")
            focusPolicy: Qt.NoFocus
            onClicked: box.search(false)
        }
    }
}
