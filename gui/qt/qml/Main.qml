// The window: its menus, and a row's right-click menu; a toolbar with the view's name and counts,
// and at its right the Find box, which Edit ▸ Find… gives the keys; three columns, the sidebar, the
// findings and the details panel, with Undo, Reject and Accept under its video; the video, in a
// window of its own that the details panel holds, or that View ▸ Pop Out Video takes out beside
// this one; the status bar; the Settings window; and the fatal end's dialog. Layout, bindings and
// one-line handlers only: what each asks of review is the bridge's (session_bridge.cpp), and what
// the window remembers the settings' (window_settings.cpp). Its keys are the window's whatever has
// focus, but for a text field's, which takes the keys it types and those it moves through and ends
// by (KeyField.qml), an open menu's, which takes them all (menu_keys.h), and the Settings window's,
// which holds all of review's back. A press anywhere in the window or the video's but on the Find
// box, while no menu has the keys, takes the keys back from the box (press_away.h).
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
    // Never narrower than the details panel's three buttons, whose width follows the window's size
    // of text and the style.
    readonly property int detailsLeast: Math.max(340, Math.ceil(details.leastWidth))
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

    // Whether Accept, Reject, Edit Wording… and Back to Undecided have a finding to act on, by
    // menu, key or the buttons under the video alike: one shown, and not a greyed episode's log in
    // its place. Not while an edit is open, which its own Save and Accept and Cancel decide: a
    // click beside the field would decide the plain fix and drop the typed wording.
    readonly property bool canDecide: keysToReview && !bridge.showingEpisode && bridge.detail.shown
                                      && !bridge.editing

    // Whether the video's pane draws in a window of its own, which the details panel holds at the
    // video's place, rather than in this window's scene at that place. Pop Out needs it: only a
    // window of its own can be taken out of this one.
    property bool videoWindowed: true
    // Whether the video's window is out, a window beside this one, rather than held by the details
    // panel.
    property bool videoOut: false

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

    // Takes the video's window out of the details panel, a window of its own beside this one. Its
    // transient parent is set again, to this window, so that this window's shortcuts and menu
    // items hear the keys typed in it: a window a container has held has lost its transient
    // parent, though it can still read this one, and setting the value it reads changes nothing;
    // hence null first. Its size is set and it is shown in this same handler: let go, the window
    // stays where the container held it, at the pane's size, and would show there first.
    function popOut() {
        if (!window.videoWindowed || window.videoOut) {
            return;
        }
        window.videoOut = true;
        videoWindow.transientParent = null;
        videoWindow.transientParent = window;
        videoWindow.width = 960;
        videoWindow.height = 540;
        videoWindow.visible = true;
        videoWindow.requestActivate();
    }

    // Puts the video's window back in the details panel, and gives this window the keys, which
    // the pop-out had.
    function popIn() {
        window.videoOut = false;
        window.requestActivate();
    }

    // An open menu has the keys, each of which is otherwise one of the window's shortcuts: a menu
    // of the bar by this binding, any other by the popup it is drawn in (menu_keys.h).
    MenuKeys {
        id: menuKeys

        target: window
        menuOpen: bar.menus.some(function (menu: Menu): bool {
            return menu.visible;
        })
    }

    // A press of the pointer anywhere in the window but on the Find box, while no menu has the
    // keys, gives the window its keys back from the box, its words kept (press_away.h).
    PressAway {
        target: window
        keeper: findBox
        menus: menuKeys
        onPressedAway: findBox.release()
    }
    // The same for a press in the video's window, held or out: no part of it keeps the box's keys.
    PressAway {
        target: videoWindow
        menus: menuKeys
        onPressedAway: findBox.release()
    }

    // One filter is always the one shown, so that choosing it again keeps its check.
    ActionGroup {
        id: shownFilter
    }

    // One row style is always the one drawn, chosen from View ▸ Rows ▸ or the list's heading.
    ActionGroup {
        id: rowStyles
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

        // Each menu of the bar opens in a window of its own, the View menu's two too: drawn in the
        // window's scene, a menu draws under the video's window wherever the two meet. Each is
        // given the window's font, which a menu of the bar in a window of its own does not take.
        WholeMenu {
            title: qsTr("&File")
            popupType: Popup.Window
            font: window.font

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
            // Typed in the popped-out video, it puts the video back rather than quit: a person
            // closes the window the key is typed in, whose keys are this window's. Which window
            // has the keys tells, not whether the pop-out is active: transient for this window, it
            // reads active whenever this window does.
            Action {
                objectName: "close"
                enabled: window.keysToReview
                text: qsTr("&Close")
                shortcut: "Ctrl+W"
                onTriggered: {
                    if (window.videoOut && videoWindow.activeFocusItem !== null) {
                        window.popIn();
                    } else {
                        window.bridge.quit();
                    }
                }
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
        WholeMenu {
            title: qsTr("&Edit")
            popupType: Popup.Window
            font: window.font

            // Not StandardKey.Undo, which has further keys on some platforms, that KDE's styles
            // warn of as they draw the item. It waits while an edit is open, as its U and the
            // button under the video do: review selects what an undo takes back, and the edit,
            // with the wording typed, ends as the selection leaves it. Ctrl+Z in the field undoes
            // the typing.
            Action {
                objectName: "undo"
                enabled: window.keysToReview && !window.bridge.editing
                text: qsTr("&Undo")
                shortcut: "Ctrl+Z"
                onTriggered: window.bridge.undo()
            }
            MenuSeparator {}
            // The selected row's line, as its file has it; none for a timing fix, whose words are
            // the timing check's. Not StandardKey.Copy, which has further keys on some platforms,
            // that KDE's styles warn of as they draw the item. It waits while a greyed episode's
            // view shows, as the finding's keys do. Ctrl+C in a text field is the field's own.
            Action {
                id: copyLineAction

                objectName: "copyLine"
                enabled: window.keysToReview && !window.bridge.showingEpisode
                         && window.bridge.canCopy
                text: qsTr("&Copy Subtitle Line")
                shortcut: "Ctrl+C"
                onTriggered: window.bridge.copyLine(window.bridge.selected)
            }
            // Gives the Find box the keys, its words selected. It waits while an edit is open: from
            // the box, Return selects another row, which ends the edit and drops its typed wording.
            // It does not wait while a greyed episode's view shows: what it does is select a row,
            // which ends that view, as a click on a row does.
            Action {
                objectName: "find"
                enabled: window.keysToReview && !window.bridge.editing
                text: qsTr("&Find…")
                shortcut: "Ctrl+F"
                onTriggered: findBox.take()
            }
        }
        WholeMenu {
            title: qsTr("&View")
            popupType: Popup.Window
            font: window.font

            WholeMenu {
                id: showMenu

                title: qsTr("&Show")
                popupType: Popup.Window
                font: window.font

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
            // Each draws every row in its style at once, and is kept for the next start. The list's
            // heading opens these three under its Rows button too.
            WholeMenu {
                title: qsTr("&Rows")
                popupType: Popup.Window
                font: window.font

                Action {
                    id: rowsCompactAction

                    objectName: "rowsCompact"
                    text: qsTr("&Compact")
                    checkable: true
                    checked: window.settings.rowStyle === "compact"
                    ActionGroup.group: rowStyles
                    onTriggered: window.settings.rowStyle = "compact"
                }
                Action {
                    id: rowsTwoLinesAction

                    objectName: "rowsTwoLines"
                    text: qsTr("&Two Lines")
                    checkable: true
                    checked: window.settings.rowStyle === "twoLines"
                    ActionGroup.group: rowStyles
                    onTriggered: window.settings.rowStyle = "twoLines"
                }
                Action {
                    id: rowsRichAction

                    objectName: "rowsRich"
                    text: qsTr("&Rich")
                    checkable: true
                    checked: window.settings.rowStyle === "rich"
                    ActionGroup.group: rowStyles
                    onTriggered: window.settings.rowStyle = "rich"
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
            // Takes the video out into a window of its own, or puts it back, as its check now
            // says; that window's close, and Ctrl+W typed in it, put it back too. Greyed where the
            // video has no window of its own, and once review has ended in an error, while the
            // video's window stays hidden.
            Action {
                objectName: "popOutVideo"
                enabled: window.keysToReview && window.videoWindowed && window.bridge.fatal === ""
                text: qsTr("&Pop Out Video")
                checkable: true
                checked: window.videoOut
                onTriggered: {
                    if (checked) {
                        window.popOut();
                    } else {
                        window.popIn();
                    }
                }
            }
        }
        WholeMenu {
            title: qsTr("F&inding")
            popupType: Popup.Window
            font: window.font

            // These six, and the keypad's Enter, wait while a greyed episode's view shows: review
            // still has the finding selected, but the panel shows the episode's log in its place,
            // so a key would act on a finding not in view. The first four wait too while no finding
            // shows or an edit is open, as canDecide says; Edit Wording… while the selected entry's
            // wording cannot be edited; and Show Each Episode while the selected entry is neither a
            // row of several nor one of its members.
            Action {
                id: acceptAction

                objectName: "accept"
                enabled: window.canDecide
                text: qsTr("&Accept")
                shortcut: "A"
                onTriggered: window.bridge.accept()
            }
            Action {
                id: rejectAction

                objectName: "reject"
                enabled: window.canDecide
                text: qsTr("&Reject")
                shortcut: "R"
                onTriggered: window.bridge.reject()
            }
            Action {
                id: editAction

                objectName: "edit"
                enabled: window.canDecide && window.bridge.canEdit
                text: qsTr("&Edit Wording…")
                shortcut: "E"
                // Asked here, by E, this item or the right-click menu's, the edit opens with the
                // cursor at the wording's end: a click on the fix that review has yet to answer
                // leaves its place behind, which is for the edit that click asked for alone.
                onTriggered: {
                    details.cursorAt = -1;
                    window.bridge.edit();
                }
            }
            Action {
                id: clearAction

                objectName: "clear"
                enabled: window.canDecide
                text: qsTr("&Back to Undecided")
                shortcut: "X"
                onTriggered: window.bridge.clear()
            }
            MenuSeparator {}
            Action {
                id: playAction

                objectName: "play"
                enabled: window.keysToReview && !window.bridge.showingEpisode
                text: qsTr("Show in &Video")
                shortcut: "O"
                onTriggered: window.bridge.play()
            }
            // Checked while the selected row is open, or one of its members is selected.
            Action {
                id: showEachEpisodeAction

                objectName: "showEachEpisode"
                enabled: window.keysToReview && !window.bridge.showingEpisode
                         && window.bridge.canExpand
                text: qsTr("&Show Each Episode")
                shortcut: "Return"
                checkable: true
                checked: window.bridge.expanded
                onTriggered: window.bridge.toggle()
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
        WholeMenu {
            title: qsTr("Vi&deo")
            popupType: Popup.Window
            font: window.font

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

    // The terminal's keys that no menu item has, and the keypad's Enter beside Show Each Episode's
    // Return.
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
    // Moves, like J and K. A text field takes Home, End, PgUp, PgDn and letters itself, so these
    // reach the list only when no field has the focus.
    Shortcut {
        enabled: window.keysToReview
        sequences: ["Home", "G"]
        onActivated: window.bridge.select(0)
    }
    Shortcut {
        enabled: window.keysToReview
        sequences: ["End", "Shift+G"]
        onActivated: window.bridge.select(list.count - 1)
    }
    Shortcut {
        enabled: window.keysToReview
        sequence: "PgUp"
        onActivated: window.bridge.move(-list.pageRows)
    }
    Shortcut {
        enabled: window.keysToReview
        sequence: "PgDown"
        onActivated: window.bridge.move(list.pageRows)
    }
    // The keypad's Enter, waiting as Show Each Episode does. A shortcut of its own, as Return is
    // the menu item's alone: of two enabled shortcuts on one key, Qt runs neither.
    Shortcut {
        enabled: showEachEpisodeAction.enabled
        sequence: "Enter"
        onActivated: window.bridge.toggle()
    }
    Shortcut {
        enabled: window.keysToReview
        sequence: "F"
        onActivated: window.bridge.cycleFilter()
    }
    Shortcut {
        enabled: window.keysToReview && !window.bridge.editing
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
            id: toolbar

            objectName: "toolbar"
            Layout.fillWidth: true

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 8
                anchors.rightMargin: 8

                // Takes what the Find box leaves, so that a long title or long counts are cut
                // first, not run on.
                ColumnLayout {
                    spacing: 0
                    Layout.fillWidth: true
                    Layout.preferredWidth: 0

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
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                }
                FindBox {
                    id: findBox

                    objectName: "findBox"
                    bridge: window.bridge
                    Layout.preferredWidth: implicitWidth
                    Layout.minimumWidth: leastWidth
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

            // The filter shown and its count, cut before the Rows button, which opens the View
            // menu's Rows items under it.
            RowLayout {
                id: headingRow

                anchors.left: parent.left
                anchors.right: parent.right
                spacing: 0

                Label {
                    objectName: "heading"
                    padding: 6
                    font.bold: true
                    text: window.bridge.heading
                    textFormat: Text.PlainText
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                }
                ToolButton {
                    id: rowStyleButton

                    objectName: "rowStyleButton"
                    text: qsTr("Rows")
                    focusPolicy: Qt.NoFocus
                    onClicked: rowsMenu.open()

                    // Its right edge at the button's, so that it opens over the list it styles
                    // rather than the details beside it. In the window's scene, unlike the bar's
                    // menus: it opens over the list, away from the video, and in the scene it
                    // keeps its place under its button.
                    WholeMenu {
                        id: rowsMenu

                        objectName: "rowsMenu"
                        popupType: Popup.Item
                        x: rowStyleButton.width - width
                        y: rowStyleButton.height

                        MenuItem {
                            action: rowsCompactAction
                        }
                        MenuItem {
                            action: rowsTwoLinesAction
                        }
                        MenuItem {
                            action: rowsRichAction
                        }
                    }
                }
            }

            ListView {
                id: list

                objectName: "findings"
                anchors.top: headingRow.bottom
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                clip: true
                // A page is the rows on screen: the window knows its height, where the terminal
                // pages by a fixed ten.
                readonly property int pageRows: currentItem
                    ? Math.max(1, Math.floor(height / currentItem.height)) : 1
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
                // Rows of the style chosen. Two Lines and Rich are one row's two looks, so that a
                // change between them redraws the rows in place; Compact's rows are their own. Two
                // Components, not a Loader, which would give the rows none of the model's required
                // properties.
                delegate: window.settings.rowStyle === "compact" ? compactRow : findingRow

                Component {
                    id: compactRow

                    CompactRow {
                        id: compactDelegate

                        width: ListView.view.width - listBar.room
                        onPicked: window.bridge.select(index)
                        onOpened: window.bridge.play()
                        onDisclosed: window.bridge.toggleRow(index)
                        onMenuAsked: at => {
                            window.bridge.select(index);
                            rowMenu.openFor(index, compactDelegate, at);
                        }
                    }
                }
                Component {
                    id: findingRow

                    FindingRow {
                        id: findingDelegate

                        width: ListView.view.width - listBar.room
                        rich: window.settings.rowStyle === "rich"
                        separator: window.separator
                        onPicked: window.bridge.select(index)
                        onOpened: window.bridge.play()
                        onDisclosed: window.bridge.toggleRow(index)
                        onMenuAsked: at => {
                            window.bridge.select(index);
                            rowMenu.openFor(index, findingDelegate, at);
                        }
                    }
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
            canDecide: window.canDecide
            windowFont: window.font
            font.pointSize: window.settings.detailsTextSize
            SplitView.preferredWidth: window.settings.detailsWidth
            SplitView.minimumWidth: window.detailsLeast
            onMenuAsked: at => rowMenu.openFor(window.bridge.selected, details, at)
        }
    }

    // The video's own window, which the container holds in the details panel, or which Pop Out
    // takes out. Held, it takes no focus: a clicked window takes the keys on X11, and the next
    // letter would then be one of this window's keys, not the field's that had them, an open
    // edit's among them. Out, it takes the keys, which are this window's (popOut). Its close puts
    // it back rather than close it.
    Window {
        id: videoWindow

        objectName: "videoWindow"
        color: "black"
        title: "subcheck"
        flags: Qt.Window | (window.videoOut ? 0 : Qt.WindowDoesNotAcceptFocus)
        onClosing: close => {
            close.accepted = false;
            window.popIn();
        }
    }

    // Holds the video's window at its place in the details panel while the video is in. One
    // container for the window's whole life, which never goes while it holds the window: a
    // container destroyed holding a window destroys that window, and mpv's video with it. So no
    // Loader holds it, nor a layout that drops what it does not show: to move the video, the
    // container moves.
    WindowContainer {
        id: videoContainer

        objectName: "videoContainer"
        parent: details.videoSlot
        anchors.fill: parent
        // Tab passes it by, as it passes the video by: given the focus, the container would hand
        // it to the window it holds, which takes none while held.
        activeFocusOnTab: false
    }
    // The window the container holds, bound from outside it: inside, `window` names the
    // container's own property, not this window. None once review has ended in an error, while the
    // error's dialog shows.
    Binding {
        target: videoContainer
        property: "window"
        value: window.videoWindowed && !window.videoOut && window.bridge.fatal === ""
               ? videoWindow : null
    }
    // Once review has ended in an error, the video's window hides, held or out: let go by the
    // container, a window stays shown, on its own at the place it was held. This and the binding
    // above both follow the error's one change, before the window's next frame, so that whichever
    // runs first, no frame shows the video's window let go.
    Connections {
        target: window.bridge

        function onFatalChanged() {
            if (window.bridge.fatal !== "") {
                videoWindow.visible = false;
            }
        }
    }

    // In the video's window, or where the video has none, in this window's scene at the video's
    // place. Given this window's font, which a window of its own does not take from this one.
    VideoPane {
        id: videoPane

        parent: window.videoWindowed ? videoWindow.contentItem : details.videoSlot
        anchors.fill: parent
        bridge: window.bridge
        font: window.font
        stretching: columns.resizing && !window.videoOut
    }

    footer: Label {
        objectName: "statusLine"
        padding: 4
        leftPadding: 8
        elide: Text.ElideRight
        text: window.bridge.statusLine
        textFormat: Text.PlainText
    }

    // Where the right-click menu opens from: a pixel square, moved to each click before the menu
    // opens (rowMenu.openFor). In the window's content, which outlives every row.
    Item {
        id: menuAnchor

        width: 1
        height: 1
    }

    // The right-click menu of a row of the list, or of the finding in the details panel, for the
    // row openFor is given, at the point of the click. Its first six are the Finding menu's own
    // items, which act on review's selection: a right-click on a row selects it first, and review
    // takes actions in order. Copy Subtitle Line and Show File in Folder ask review nothing, so
    // they read the row clicked at once, as review may not have answered its Select yet (while a
    // job runs, say): items of their own, so that Copy shows no key, Ctrl+C copying the selected
    // row's line. Native where Qt has the platform's menus (on macOS, say), so that it is the
    // system's own right-click menu, which has the keys while open. Wherever Qt has none, offscreen
    // among them, Qt Quick draws it: in a popup window of its own over the window where the
    // platform has such windows, and in the window where it has none. Either way the window's
    // shortcuts hear each key first: MenuKeys' popup rule then gives the open menu its keys
    // (menu_keys.h).
    WholeMenu {
        id: rowMenu

        // The row it was last opened for.
        property int row: -1
        // Whether that row has a subtitle line to copy, read again as the menu opens and at each
        // change of review's: either can find another entry at the same row number, a timing fix's
        // in a line's place or the reverse, which a binding on the row number alone would miss.
        property bool copyable: false

        // Opens it for the list's row `index`, at `where` in `from`'s coordinates: the point the
        // request came with, as Wayland gives a window no pointer position on the screen. It opens
        // from menuAnchor, moved to that point first, and not at a point in a larger item: Wayland
        // places a menu Qt draws in a popup window of its own below the rect of the item it opens
        // from, whatever point it is given, so that item is the click's, a pixel square. Not the
        // row's delegate either: popup makes the item it is given the menu's parent, and a row's
        // delegate goes when the list changes under it, a filter or a style change, taking an open
        // menu's parent with it.
        function openFor(index: int, from: Item, where: point) {
            rowMenu.row = index;
            rowMenu.copyable = window.bridge.canCopyAt(index);
            const at = window.contentItem.mapFromItem(from, where);
            menuAnchor.x = at.x;
            menuAnchor.y = at.y;
            rowMenu.popup(menuAnchor, Qt.point(0, 0));
        }

        objectName: "rowMenu"
        popupType: Popup.Native
        // The rect Qt hands Wayland is the item's moved right by the menu's overlap and narrowed by
        // twice it. A style's overlap shows only on a submenu, which this menu never is, so none:
        // the rect is then menuAnchor's own, at the click, in every style.
        overlap: 0

        MenuItem {
            objectName: "rowAccept"
            action: acceptAction
        }
        MenuItem {
            objectName: "rowReject"
            action: rejectAction
        }
        MenuItem {
            objectName: "rowEdit"
            action: editAction
        }
        MenuItem {
            objectName: "rowClear"
            action: clearAction
        }
        MenuSeparator {}
        MenuItem {
            objectName: "rowPlay"
            action: playAction
        }
        MenuItem {
            objectName: "rowShowEachEpisode"
            action: showEachEpisodeAction
        }
        MenuSeparator {}
        MenuItem {
            objectName: "rowCopy"
            text: qsTr("&Copy Subtitle Line")
            enabled: window.keysToReview && rowMenu.copyable
            onTriggered: window.bridge.copyLine(rowMenu.row)
        }
        MenuItem {
            objectName: "rowShowInFolder"
            text: qsTr("Show &File in Folder")
            enabled: window.keysToReview && rowMenu.row >= 0
            onTriggered: window.bridge.showInFolder(rowMenu.row)
        }

        Connections {
            target: window.bridge

            function onSummaryChanged() {
                rowMenu.copyable = window.bridge.canCopyAt(rowMenu.row);
            }
        }
    }

    SettingsWindow {
        id: settingsWindow

        settings: window.settings
        bridge: window.bridge
        separator: window.separator
    }

    // What ended review in an error; closing it ends the program. Drawn in the window's scene,
    // which draws under the video's window wherever the two meet: so the video's window hides while
    // it shows, held or out (the handler beside the video's container), as the error has ended
    // review and the video has nothing more to show.
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
