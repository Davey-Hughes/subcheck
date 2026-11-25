// A one-line text field that keeps the keys a person types in it, moves through it with and ends it
// by. The window's shortcuts hear every key before the field does, so a plain TextField loses
// Return, Enter, Up, Down and Esc to them, the list's moves and Show Each Episode among them. This
// one keeps those, and PgUp and PgDn, so that no page move the window has or gains takes them: a
// move selects another entry, which ends an edit and drops its typed wording. Home, End, Left,
// Right, the letters, Space, Ctrl+Z and Ctrl+C are a TextField's own already. `entered` is Return
// or Enter, `back` whether Shift was held, as a search goes back with it; `escaped` is Esc. Up,
// Down, PgUp and PgDn have nowhere to go in one line, and do nothing, in the field or in what it
// sits in.
import QtQuick
import QtQuick.Controls

TextField {
    id: field

    signal entered(bool back)
    signal escaped

    Keys.onShortcutOverride: event => {
        event.accepted = [Qt.Key_Return, Qt.Key_Enter, Qt.Key_Up, Qt.Key_Down, Qt.Key_Escape,
                          Qt.Key_PageUp, Qt.Key_PageDown].includes(event.key);
    }
    Keys.onReturnPressed: event => field.entered((event.modifiers & Qt.ShiftModifier) !== 0)
    Keys.onEnterPressed: event => field.entered((event.modifiers & Qt.ShiftModifier) !== 0)
    Keys.onEscapePressed: field.escaped()
    Keys.onPressed: event => {
        event.accepted = [Qt.Key_Up, Qt.Key_Down, Qt.Key_PageUp, Qt.Key_PageDown].includes(
                    event.key);
    }
}
