import QtQuick
import Logos.Theme
import Logos.Controls

// The liquidation point: a dashed rule at `at` (0..1 of the window, from the bottom), labelled.
Item {
    id: line
    property string label: qsTr("If the water reaches here, some of your collateral is sold")
    anchors.left: parent.left
    anchors.right: parent.right
    property real at: 0.8
    y: parent.height * (1 - at)
    height: 2
    Row {
        spacing: 6
        Repeater {
            model: Math.ceil(line.width / 12)
            delegate: Rectangle { width: 6; height: 2; color: "#8CFFFFFF" }
        }
    }
    LogosText {
        anchors.right: parent.right
        anchors.rightMargin: 28
        anchors.bottom: parent.top
        anchors.bottomMargin: 8
        text: line.label
        font.pixelSize: 12
        font.weight: Font.Bold
        color: "#D9FFFFFF"
    }
}
