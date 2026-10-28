import QtQuick
import Logos.Controls

// The round − / + either side of a big amount.
Rectangle {
    id: round
    property alias text: label.text
    property int size: 60
    signal clicked()
    width: size
    height: size
    radius: size / 2
    color: "#26000000"
    border.width: 2
    border.color: "#80FFFFFF"
    LogosText { id: label; anchors.centerIn: parent; font.pixelSize: round.size / 2; color: "#FFFFFF" }
    MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: round.clicked() }
}
