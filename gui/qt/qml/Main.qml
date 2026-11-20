// The window: its menus; a toolbar with the view's name and counts; three columns, the sidebar,
// the findings and the details panel; the status bar; the Settings window; and the fatal end's
// dialog. Layout, bindings and one-line handlers only: what each asks of review is the bridge's
// (session_bridge.cpp), and what the window remembers the settings' (window_settings.cpp). Its
// keys are the window's whatever has focus, but for a text field's, which takes the keys it types,
// an open menu's, which takes them all (menu_keys.h), and the Settings window's, which holds all of
// review's back.
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Subcheck

ApplicationWindow {
    id: window

    required property SessionBridge bridge
    required property WindowSettings settings

    // The width of a column handle's line, which the handles' room below and the window's least
    // width count, so that they follow it.
    readonly property int handleWidth: 1

    // Each column's least width, and room for the handles between them: two, between three
    // columns. As the window narrows, the list gives way first, then the details panel, then the
    // sidebar, whose widest leaves the other two their least.
    readonly property int sidebarLeast: 160
    readonly property int listLeast: 260
    readonly property int detailsLeast: 340
    readonly property int handles: 2 * handleWidth

    // The lines between the window's parts. Qt Quick's palette has no separator role, so window
    // text at 20% alpha stands in for the style's, and follows light and dark with it.
    readonly property color separator: Qt.alpha(palette.windowText, 0.2)

    // Whether the window's keys reach review: not while Settings is open. Qt counts this window
    // active while Settings, which is transient for it, has the keys, so that a key typed into
    // Settings matches this window's shortcuts and menu items too; and a menu bar outside the
    // window, where one still offers its items, could run them by key or click. Disabled, an item
    // or a shortcut runs on neither.
    readonly property bool keysToReview: !settingsWindow.visible

    // The window's own size of text, which its menus and dialogs take too; the list and the details
    // panel each have their own below.
    font.pointSize: settings.windowTextSize

    minimumWidth: sidebarLeast + listLeast + detailsLeast + handles
    minimumHeight: 480
    title: bridge.title

    // Review decides when the window ends: closing it is quitting.
    onClosing: close => {
        close.accepted = false;
        bridge.quit();
    }

    // An open menu has the keys, each of which is otherwise one of the window's shortcuts.
    MenuKeys {
        target: window
        menuOpen: bar.menus.some(function (menu: Menu): bool {
            return menu.visible;
        })
    }

    // One filter is always the one shown, so that choosing it again keeps its check.
    ActionGroup {
        id: shownFilter
    }

    // A palette chosen in Settings shows at once, everywhere it is drawn.
    Connections {
        target: window.settings

        function onColoursChanged() {
            window.bridge.recolour();
        }
    }

    // The sidebar lists the episodes in the order the View menu chose, from the first frame.
    Binding {
        target: window.bridge.episodes
        property: "noFindingsLast"
        value: window.settings.noFindingsLast
    }

    menuBar: MenuBar {
        id: bar

        Menu {
            title: qsTr("&File")

            // Not StandardKey.Preferences, which some platforms leave with no key. Enabled while
            // Settings is open, so that where a menu bar still offers it, it brings Settings to
            // the front rather than do nothing; but not once review has ended in an error, when
            // Settings, application-modal, would open over the error's dialog and hold back its
            // OK and Escape.
            Action {
                objectName: "settings"
                enabled: window.bridge.fatal === ""
                text: qsTr("&Settings…")
                shortcut: "Ctrl+,"
                onTriggered: settingsWindow.open()
            }
            MenuSeparator {}
            // Not StandardKey.Close, which on Linux has a second key, Ctrl+F4, that KDE's styles
            // warn of as they draw the item. Qt reads Ctrl as Command on macOS, here and below.
            Action {
                objectName: "close"
                enabled: window.keysToReview
                text: qsTr("&Close")
                shortcut: "Ctrl+W"
                onTriggered: window.bridge.quit()
            }
            // Not StandardKey.Quit, which some platforms leave with no key, Qt's offscreen one
            // among them.
            Action {
                objectName: "quit"
                enabled: window.keysToReview
                text: qsTr("&Quit")
                shortcut: "Ctrl+Q"
                onTriggered: window.bridge.quit()
            }
        }
        Menu {
            title: qsTr("&Edit")

            // Not StandardKey.Undo, which has further keys on some platforms, that KDE's styles
            // warn of as they draw the item.
            Action {
                objectName: "undo"
                enabled: window.keysToReview
                text: qsTr("&Undo")
                shortcut: "Ctrl+Z"
                onTriggered: window.bridge.undo()
            }
        }
        Menu {
            title: qsTr("&View")

            Menu {
                id: showMenu

                title: qsTr("&Show")

                Instantiator {
                    model: window.bridge.filters
                    delegate: Action {
                        required property int index
                        required property string name
                        required property bool current

                        objectName: "show" + index
                        enabled: window.keysToReview
                        text: name
                        checkable: true
                        checked: current
                        ActionGroup.group: shownFilter
                        onTriggered: window.bridge.setFilter(index)
                    }
                    onObjectAdded: (index, item) => showMenu.insertAction(index, item as Action)
                    onObjectRemoved: (index, item) => showMenu.removeAction(item as Action)
                }
            }
            Action {
                objectName: "noFindingsLast"
                text: qsTr("&Episodes Without Findings Last")
                checkable: true
                checked: window.settings.noFindingsLast
                onTriggered: window.settings.noFindingsLast = checked
            }
            Action {
                objectName: "switchView"
                enabled: window.keysToReview
                text: window.bridge.switchLabel
                shortcut: "V"
                onTriggered: window.bridge.switchView()
            }
        }
        Menu {
            title: qsTr("F&inding")

            // These four, and Enter, wait while a greyed episode's view shows: review still has the
            // finding selected, but the panel shows the episode's log in its place, so a key would
            // act on a finding not in view.
            Action {
                objectName: "accept"
                enabled: window.keysToReview && !window.bridge.showingEpisode
                text: qsTr("&Accept Fix")
                shortcut: "A"
                onTriggered: window.bridge.accept()
            }
            Action {
                objectName: "reject"
                enabled: window.keysToReview && !window.bridge.showingEpisode
                text: qsTr("&Reject Fix")
                shortcut: "R"
                onTriggered: window.bridge.reject()
            }
            Action {
                objectName: "clear"
                enabled: window.keysToReview && !window.bridge.showingEpisode
                text: qsTr("&Back to Undecided")
                shortcut: "X"
                onTriggered: window.bridge.clear()
            }
            MenuSeparator {}
            Action {
                objectName: "play"
                enabled: window.keysToReview && !window.bridge.showingEpisode
                text: qsTr("Show in &Video")
                shortcut: "O"
                onTriggered: window.bridge.play()
            }
            MenuSeparator {}
            Action {
                objectName: "nextEpisode"
                enabled: window.keysToReview
                text: qsTr("&Next Episode")
                shortcut: "N"
                onTriggered: window.bridge.nextEpisode()
            }
            Action {
                objectName: "previousEpisode"
                enabled: window.keysToReview
                text: qsTr("&Previous Episode")
                shortcut: "P"
                onTriggered: window.bridge.previousEpisode()
            }
        }
        Menu {
            title: qsTr("Vi&deo")

            Action {
                objectName: "pause"
                enabled: window.keysToReview
                text: qsTr("&Play/Pause")
                shortcut: "Space"
                onTriggered: window.bridge.pause()
            }
            Action {
                objectName: "back5"
                enabled: window.keysToReview
                text: qsTr("&Back 5 s")
                shortcut: "Left"
                onTriggered: window.bridge.seek(-5)
            }
            Action {
                objectName: "forward5"
                enabled: window.keysToReview
                text: qsTr("&Forward 5 s")
                shortcut: "Right"
                onTriggered: window.bridge.seek(5)
            }
            Action {
                objectName: "back1"
                enabled: window.keysToReview
                text: qsTr("Back 1 s")
                shortcut: ","
                onTriggered: window.bridge.seek(-1)
            }
            Action {
                objectName: "forward1"
                enabled: window.keysToReview
                text: qsTr("Forward 1 s")
                shortcut: "."
                onTriggered: window.bridge.seek(1)
            }
            Action {
                objectName: "previousFrame"
                enabled: window.keysToReview
                text: qsTr("Pre&vious Frame")
                shortcut: "["
                onTriggered: window.bridge.previousFrame()
            }
            Action {
                objectName: "nextFrame"
                enabled: window.keysToReview
                text: qsTr("Ne&xt Frame")
                shortcut: "]"
                onTriggered: window.bridge.nextFrame()
            }
            MenuSeparator {}
            Action {
                objectName: "closeVideo"
                enabled: window.keysToReview
                text: qsTr("&Close Video")
                shortcut: "Shift+O"
                onTriggered: window.bridge.closePlayer()
            }
        }
    }

    // The terminal's keys that no menu item has.
    Shortcut {
        enabled: window.keysToReview
        sequences: ["J", "Down"]
        onActivated: window.bridge.move(1)
    }
    Shortcut {
        enabled: window.keysToReview
        sequences: ["K", "Up"]
        onActivated: window.bridge.move(-1)
    }
    // As the Finding menu's items, it waits while a greyed episode's view shows.
    Shortcut {
        enabled: window.keysToReview && !window.bridge.showingEpisode
        sequences: ["Return", "Enter"]
        onActivated: window.bridge.toggle()
    }
    Shortcut {
        enabled: window.keysToReview
        sequence: "F"
        onActivated: window.bridge.cycleFilter()
    }
    Shortcut {
        enabled: window.keysToReview
        sequence: "U"
        onActivated: window.bridge.undo()
    }
    Shortcut {
        enabled: window.keysToReview
        sequence: "Q"
        onActivated: window.bridge.quit()
    }

    header: ColumnLayout {
        spacing: 0

        // Where the window draws the menu bar, a line between it and the toolbar: the two are
        // otherwise one band. It shows only where the window draws the bar.
        Rectangle {
            objectName: "menuLine"
            visible: bar.visible && bar.height > 0
            color: window.separator
            implicitHeight: 1
            Layout.fillWidth: true
        }

        ToolBar {
            objectName: "toolbar"
            Layout.fillWidth: true

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 8
                anchors.rightMargin: 8

                // Takes what Undo leaves, so that a long title is cut rather than push Undo out.
                ColumnLayout {
                    spacing: 0

                    Label {
                        objectName: "title"
                        text: window.bridge.title
                        textFormat: Text.PlainText
                        font.bold: true
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                    Label {
                        objectName: "counts"
                        text: window.bridge.counts
                        textFormat: Text.PlainText
                        opacity: 0.7
                    }
                }
                ToolButton {
                    objectName: "undoButton"
                    text: qsTr("Undo")
                    focusPolicy: Qt.NoFocus
                    onClicked: window.bridge.undo()
                }
            }
        }
    }

    SplitView {
        id: columns

        objectName: "columns"
        anchors.fill: parent
        // A thin line with a wide place to take hold of it.
        handle: Rectangle {
            id: columnHandle

            readonly property bool active: grip.hovered || SplitHandle.pressed

            // The handle at the sidebar's right edge, where its scroll bars end.
            readonly property bool besideSidebar: Math.abs(x - (sidebar.x + sidebar.width)) < 1

            objectName: "columnHandle"
            implicitWidth: window.handleWidth
            color: active ? window.palette.highlight : window.separator
            containmentMask: gripArea

            // An item of its own, as hover reaches only what an item's own bounds hold. It stays
            // within the columns' 8 px margins, so that it covers no content, but for 3 px over
            // the sidebar's scroll bars, which run to its right edge.
            Item {
                id: gripArea

                readonly property int reachLeft: columnHandle.besideSidebar ? 3 : 8

                x: -reachLeft
                width: reachLeft + columnHandle.width + 8
                height: columnHandle.height

                HoverHandler {
                    id: grip

                    cursorShape: Qt.SplitHCursor
                }
            }
        }
        // Kept once a drag ends, rather than at each step of it. Each column's own width, not the
        // one it has now: a narrow window squeezes a column the drag did not touch.
        onResizingChanged: {
            if (!resizing) {
                window.settings.keepColumns(sidebar.SplitView.preferredWidth,
                                            details.SplitView.preferredWidth)
            }
        }

        Sidebar {
            id: sidebar

            objectName: "sidebar"
            bridge: window.bridge
            SplitView.preferredWidth: window.settings.sidebarWidth
            SplitView.minimumWidth: window.sidebarLeast
            SplitView.maximumWidth: columns.width - window.listLeast - window.detailsLeast
                                    - window.handles
        }

        // The list takes what the other two leave, so that it gives way first. A Control, so that
        // the labels in it take the list's size of text; its hover stays on, as a Control's is by
        // default, so that what is in it still follows the pointer.
        Control {
            SplitView.fillWidth: true
            SplitView.minimumWidth: window.listLeast
            font.pointSize: window.settings.listTextSize

            Label {
                id: heading

                objectName: "heading"
                anchors.left: parent.left
                anchors.right: parent.right
                padding: 6
                font.bold: true
                text: window.bridge.heading
                textFormat: Text.PlainText
            }

            ListView {
                id: list

                objectName: "findings"
                anchors.top: heading.bottom
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                clip: true
                // The selection is review's: the list only shows it.
                keyNavigationEnabled: false
                model: window.bridge.findings
                // Where the list is in a long run, and a handle to drag through it.
                ScrollBar.vertical: ListScrollBar {
                    id: listBar

                    objectName: "findingsScrollBar"
                }
                currentIndex: window.bridge.selected
                highlightMoveDuration: 0
                highlightResizeDuration: 0
                highlight: Rectangle {
                    // The inactive grey while a greyed episode's view shows: the finding is still
                    // review's selection, where a move goes on from, but not the one in view.
                    readonly property color tone: window.bridge.showingEpisode
                                                  ? window.palette.mid : window.palette.highlight

                    // As wide as the rows, which give way to the scroll bar.
                    width: ListView.view.width - listBar.room
                    // A tint, so that the row's own colours still read over it, and an outline
                    // at full strength, so that the selection shows on a pale highlight too.
                    color: Qt.alpha(tone, 0.3)
                    border.color: tone
                    border.width: 1
                }
                delegate: FindingRow {
                    width: ListView.view.width - listBar.room
                    onPicked: window.bridge.select(index)
                    onOpened: window.bridge.play()
                }
            }

            Label {
                objectName: "empty"
                anchors.centerIn: list
                visible: text !== ""
                text: window.bridge.emptyWords
            }
        }

        DetailsPanel {
            id: details

            objectName: "details"
            bridge: window.bridge
            separator: window.separator
            columnsResizing: columns.resizing
            font.pointSize: window.settings.detailsTextSize
            SplitView.preferredWidth: window.settings.detailsWidth
            SplitView.minimumWidth: window.detailsLeast
        }
    }

    footer: Label {
        objectName: "statusLine"
        padding: 4
        leftPadding: 8
        elide: Text.ElideRight
        text: window.bridge.statusLine
        textFormat: Text.PlainText
    }

    SettingsWindow {
        id: settingsWindow

        settings: window.settings
        bridge: window.bridge
        separator: window.separator
    }

    Dialog {
        objectName: "fatal"
        anchors.centerIn: parent
        // A width of its own: left to size itself, the dialog's width and its label's loop.
        width: Math.min(window.width - 40, 640)
        modal: true
        // Closing it ends the program, which a stray click beside it should not.
        closePolicy: Popup.CloseOnEscape
        title: qsTr("subcheck")
        standardButtons: Dialog.Ok
        visible: window.bridge.fatal !== ""
        // Settings, modal, would hold the keys from the window this dialog is in, and with them
        // the keys that close it.
        onAboutToShow: settingsWindow.close()
        onClosed: window.bridge.dismiss()

        Label {
            anchors.fill: parent
            text: window.bridge.fatal
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
    }
}
