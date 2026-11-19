// Settings, in a window of its own: the palette the window draws in, with a sample of a change
// drawn in it as the details panel draws one, and the size of each part's text, each section's
// labels beside its controls in one grid, so that the controls line up from one section to the
// next. Each choice applies, and is kept, at once: there is nothing to confirm or cancel, so it has
// no button to do either, and closes as a window does, by Escape, Ctrl+W or the title bar's close
// button. It opens on the palette chosen, whose radio button has the keys, the arrows choosing the
// one before or after it.
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Subcheck

ApplicationWindow {
    id: settingsWindow

    required property WindowSettings settings
    required property SessionBridge bridge
    // The main window's line between parts, drawn here between the sections.
    required property color separator

    // The room around the form, which the window's least size counts.
    readonly property int margin: 12

    // A size in half points, so that the arrows step by half a point, said in points.
    component HalfPoints: SpinBox {
        id: size

        required property WindowSettings settings

        from: settings.leastHalfPoints
        to: settings.mostHalfPoints
        // Never typed into, though org.kde.desktop makes its spin boxes editable: a typed "12"
        // would be read as 12 half points.
        editable: false
        textFromValue: (value, locale) => size.settings.sizeWords(value)
    }

    // Shown at its contents' size each time it opens, and brought to the front with the keys if it
    // is open already; show() brings a window it shows to the front. It takes the main window's
    // screen before its size, so that its height is reckoned against the screen it opens on.
    function open() {
        if (visible) {
            raise();
        } else {
            const over = transientParent as Window;
            if (over) {
                screen = over.screen;
            }
            width = minimumWidth;
            height = minimumHeight;
            show();
        }
        requestActivate();
    }

    objectName: "settingsWindow"
    title: qsTr("Settings")
    // Application-modal: with one main window it blocks what window-modal would, and macOS makes a
    // sheet over the main window of a window-modal dialog, where this one is a window of its own.
    flags: Qt.Dialog
    modality: Qt.ApplicationModal
    // A window of its own takes no font from the one it opens over.
    font.pointSize: settings.windowTextSize
    // No x or y: Qt and the window manager centre a transient dialog on its parent, and keep it on
    // the screen, only while its place is left to them.

    // Never smaller than its contents, so that nothing in it is cut short, and so growing with
    // them as a text size grows. The scroll view's padding counts too, where a style keeps the
    // scroll bar beside the content rather than over it. No taller than four fifths of its screen:
    // QML can measure neither the title bar nor the desktop's panels, and a fifth leaves room for
    // both, so that the whole window stays on the screen. Past that the contents scroll.
    minimumWidth: form.implicitWidth + 2 * margin + scroll.leftPadding + scroll.rightPadding
    minimumHeight: Math.min(form.implicitHeight + 2 * margin + scroll.topPadding
                            + scroll.bottomPadding, Math.floor(0.8 * Screen.height))

    onVisibleChanged: {
        if (visible) {
            palettes.checkedButton.forceActiveFocus();
        }
    }

    // Matches only while Settings is open, when File ▸ Close, which shares Ctrl+W, is disabled, so
    // that the two never meet.
    Shortcut {
        sequences: ["Escape", "Ctrl+W"]
        onActivated: settingsWindow.close()
    }

    FontMetrics {
        id: metrics

        font: settingsWindow.font
    }

    // A size box saying the widest of the sizes in the window's text, which the style measures as
    // it measures any box, its least width and its arrows with it; each size box is as wide, and
    // 2 px more, as org.kde.desktop's own text field allows, as Qt Quick can lay the words out a
    // fraction wider than QFontMetricsF measures them. A step of the Findings list's or the
    // Details panel's box then leaves every width as it is, and one of the Window's changes the
    // window's text, and with it all three widths alike. Shown, so that the style polishes it as
    // any shown box, but unseen, beneath the form, and out of the keys' and the pointer's way. A
    // screen reader skips the box itself, though not the text field the style puts in it.
    HalfPoints {
        id: widestSize

        objectName: "widestSize"
        settings: settingsWindow.settings
        value: settingsWindow.settings.widestSize(widestSize.font, widestSize.from, widestSize.to)
        opacity: 0
        enabled: false
        activeFocusOnTab: false
        Accessible.ignored: true
    }

    ScrollView {
        id: scroll

        objectName: "settingsScroll"
        anchors.fill: parent
        contentWidth: availableWidth
        contentHeight: form.implicitHeight + 2 * settingsWindow.margin

        GridLayout {
            id: form

            x: settingsWindow.margin
            y: settingsWindow.margin
            width: scroll.availableWidth - 2 * settingsWindow.margin
            columns: 2
            columnSpacing: 12
            rowSpacing: 8

            Label {
                text: qsTr("Colours")
                font.bold: true
                font.pointSize: settingsWindow.font.pointSize + 2
                Layout.columnSpan: 2
            }

            Label {
                // The first palette's radio button, whose name this is level with.
                readonly property Item first: paletteChoices.count > 0 ? paletteChoices.itemAt(0)
                                                                       : null

                objectName: "paletteLabel"
                text: qsTr("Palette:")
                Layout.alignment: Qt.AlignRight | Qt.AlignTop
                Layout.topMargin: first ? first.baselineOffset - baselineOffset : 0
            }
            ColumnLayout {
                spacing: 2
                Layout.alignment: Qt.AlignTop

                ButtonGroup {
                    id: palettes
                }
                Repeater {
                    id: paletteChoices

                    model: settingsWindow.settings.colourChoices

                    RadioButton {
                        required property int index
                        required property var modelData

                        objectName: "colours" + index
                        text: modelData.label
                        checked: settingsWindow.settings.colours === modelData.key
                        // The keys follow the choice, as the arrows choose by moving them.
                        focus: checked
                        ButtonGroup.group: palettes
                        onClicked: settingsWindow.settings.colours = modelData.key
                        // Qt Quick's radio buttons leave the arrows to whoever has them.
                        Keys.onPressed: event => {
                            const by = {
                                [Qt.Key_Up]: -1,
                                [Qt.Key_Left]: -1,
                                [Qt.Key_Down]: 1,
                                [Qt.Key_Right]: 1
                            }[event.key];
                            if (by !== undefined) {
                                settingsWindow.settings.colours =
                                    settingsWindow.settings.coloursBeside(modelData.key, by);
                                event.accepted = true;
                            }
                        }
                    }
                }
            }

            Label {
                objectName: "sampleLabel"
                text: qsTr("Sample:")
                Layout.alignment: Qt.AlignRight | Qt.AlignTop
                // Level with the sample's first line.
                Layout.topMargin: sampleNow.baselineOffset - baselineOffset
            }
            // Fills the controls' column and wraps in it, rather than widen it to the sample's
            // longest line.
            ColumnLayout {
                spacing: 8
                Layout.fillWidth: true
                Layout.preferredWidth: 30 * metrics.averageCharacterWidth

                // At the details panel's size, as the panel would draw it.
                ChangeBlock {
                    id: sampleNow

                    objectName: "sampleNow"
                    label: qsTr("− In the subtitles now")
                    line: settingsWindow.bridge.sample.now
                    tone: settingsWindow.bridge.sample.oldTone
                    font.pointSize: settingsWindow.settings.detailsTextSize
                    Layout.fillWidth: true
                }
                ChangeBlock {
                    objectName: "sampleFixed"
                    label: qsTr("+ With the fix")
                    line: settingsWindow.bridge.sample.fixed
                    tone: settingsWindow.bridge.sample.newTone
                    font.pointSize: settingsWindow.settings.detailsTextSize
                    Layout.fillWidth: true
                }
            }

            Rectangle {
                color: settingsWindow.separator
                implicitHeight: 1
                Layout.columnSpan: 2
                Layout.fillWidth: true
                Layout.topMargin: 8
                Layout.bottomMargin: 8
            }

            Label {
                text: qsTr("Text Size")
                font.bold: true
                font.pointSize: settingsWindow.font.pointSize + 2
                Layout.columnSpan: 2
            }

            Label {
                text: qsTr("Window:")
                Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
            }
            HalfPoints {
                objectName: "windowSize"
                settings: settingsWindow.settings
                implicitWidth: widestSize.implicitWidth + 2
                value: Math.round(settingsWindow.settings.windowTextSize * 2)
                onValueModified: settingsWindow.settings.windowTextSize = value / 2
                Accessible.name: qsTr("Window")
            }
            Label {
                text: qsTr("Findings list:")
                Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
            }
            HalfPoints {
                objectName: "listSize"
                settings: settingsWindow.settings
                implicitWidth: widestSize.implicitWidth + 2
                value: Math.round(settingsWindow.settings.listTextSize * 2)
                onValueModified: settingsWindow.settings.listTextSize = value / 2
                Accessible.name: qsTr("Findings list")
            }
            Label {
                text: qsTr("Details panel:")
                Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
            }
            HalfPoints {
                objectName: "detailsSize"
                settings: settingsWindow.settings
                implicitWidth: widestSize.implicitWidth + 2
                value: Math.round(settingsWindow.settings.detailsTextSize * 2)
                onValueModified: settingsWindow.settings.detailsTextSize = value / 2
                Accessible.name: qsTr("Details panel")
            }
            // The labels' cell beside Use the System Size, which sits under the sizes it resets.
            Item {}
            Button {
                objectName: "systemSize"
                text: qsTr("Use the System Size")
                onClicked: settingsWindow.settings.useSystemTextSizes()
            }
        }
    }
}
