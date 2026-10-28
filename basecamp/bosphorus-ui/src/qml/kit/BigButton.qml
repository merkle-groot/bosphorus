import QtQuick
import Logos.Theme
import Logos.Controls

// The one main action per screen: a white pill (or an outlined one with `outline`).
Rectangle {
    id: button
    property alias text: label.text
    property bool outline: false
    property real buttonHeight: 54
    property real fontSize: 16
    property color ink: "#0B1424"
    signal clicked()
    implicitHeight: buttonHeight
    implicitWidth: label.implicitWidth + 2 * Math.round(buttonHeight * 0.55)
    radius: height / 2
    color: !enabled ? "#33FFFFFF" : (outline ? "transparent" : "#FFFFFF")
    border.width: outline ? 1.5 : 0
    border.color: "#8CFFFFFF"
    LogosText {
        id: label
        anchors.centerIn: parent
        font.pixelSize: button.fontSize
        font.weight: Font.ExtraBold
        color: !button.enabled ? "#B3FFFFFF" : (button.outline ? "#FFFFFF" : button.ink)
    }
    MouseArea {
        anchors.fill: parent
        enabled: button.enabled
        cursorShape: button.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: button.clicked()
    }
}
