import QtQuick
import Logos.Theme
import Logos.Controls

// A pill choice (account picker, op picker, "All", nav): white and filled when on.
Rectangle {
    id: pill
    property alias text: label.text
    property bool on: false
    property int pillHeight: 38
    property real fontSize: 14
    property bool dark: false          // on a dark panel rather than water
    property color offColor: dark ? "transparent" : "#26000000"
    signal clicked()
    implicitHeight: pillHeight
    implicitWidth: label.implicitWidth + 32
    radius: height / 2
    color: on ? "#FFFFFF" : offColor
    border.width: 1.5
    border.color: on ? "#FFFFFF" : "#4DFFFFFF"
    opacity: enabled ? 1 : 0.5
    LogosText {
        id: label
        anchors.centerIn: parent
        font.pixelSize: pill.fontSize
        font.weight: Font.Bold
        color: pill.on ? "#0B1424" : "#FFFFFF"
    }
    MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: pill.clicked() }
}
