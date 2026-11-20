// The busy animation over the video while Show in Video waits for it: Qt Quick Controls' Material
// arc, whatever style draws the rest of the window, as Material's arc reads as loading in every
// style; in the palette's highlight colour. The control is 48 px across, big enough to read at a
// glance over the video and small enough to leave the frame around it in view; Material draws the
// arc inside the control's padding. The only file that imports Material, and it holds no other
// control: beside one, qmllint takes the controls Material re-exports from the Basic style, Control
// among them, for types renamed to Material's.
import QtQuick
import QtQuick.Controls.Material as Material

Material.BusyIndicator {
    id: arc

    implicitWidth: 48
    implicitHeight: 48
    Material.Material.accent: arc.palette.highlight
}
