// A menu as wide as its items ask, each drawn whole with its shortcut. The styles size a menu item
// without its shortcut's text: Breeze gives the shortcut only the width the item has to spare and
// draws it over the label when there is none, and Fusion hides it. Fusion's menu keeps its
// background's width, too, whatever its items ask. So each item asks for its own width, the
// spacing and its shortcut's text laid end to end, and the menu takes the widest: an end-to-end
// sum, so that a style that already counts the shortcut gets a menu a little wider than it needs.
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Templates as T

Menu {
    id: menu

    // What the item at `index` of `of` asks, the text of its shortcut measured in its font. A
    // separator asks its own width.
    component Need: TextMetrics {
        id: need

        required property T.Menu of
        required property int index

        readonly property Item drawn: of.itemAt(index)
        readonly property T.MenuItem item: drawn as T.MenuItem
        // The text the platform shows for the item's shortcut. Disabled, so that it catches no key,
        // which its action's own shortcut takes.
        readonly property Shortcut keys: Shortcut {
            enabled: false
            sequence: need.item?.action?.shortcut ?? ""
        }
        readonly property real asked: !item ? (drawn?.implicitWidth ?? 0)
                                            : item.implicitWidth + item.spacing
                                              + (text === "" ? 0 : advanceWidth)

        font: item?.font ?? of.font
        text: keys.nativeText
    }

    // The widest that an item asks.
    readonly property real widest: {
        let widest = 0;
        for (let i = 0; i < needs.count; ++i) {
            const each = needs.objectAt(i) as Need;
            if (each) {
                widest = Math.max(widest, each.asked);
            }
        }
        return widest;
    }

    // Whole pixels, rounded up: a menu drawn in a window of its own is cut to whole pixels, which
    // would take the fraction from the widest item's shortcut.
    width: Math.ceil(Math.max(implicitWidth, leftPadding + rightPadding + widest))

    Instantiator {
        id: needs

        objectName: "itemNeeds"
        model: menu.count
        delegate: Need {
            of: menu
        }
    }
}
