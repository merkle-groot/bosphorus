import QtQuick
import Logos.Controls

// A small rounded tag (job state, market flag).
Rectangle {
    property alias text: label.text
    property color fg: "#FFFFFF"
    property bool big: false
    implicitHeight: label.implicitHeight + (big ? 12 : 6)
    implicitWidth: label.implicitWidth + (big ? 28 : 20)
    radius: 14
    LogosText { id: label; anchors.centerIn: parent; font.pixelSize: parent.big ? 13.5 : 12; font.weight: Font.ExtraBold; color: parent.fg }
}
