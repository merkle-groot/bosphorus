import QtQuick
import Logos.Theme
import Logos.Controls
import "tide.js" as Tide

// The liquidation point: a solid red rule at `at` (0..1 of its parent, from the bottom), labelled
// above it on the right. Solid so it never reads like the dashed white preview of where an op
// would take the water.
Item {
    id: line
    property string label: qsTr("If the water reaches here, some of your collateral is sold")
    anchors.left: parent.left
    anchors.right: parent.right
    property real at: 0.8
    y: parent.height * (1 - at)
    height: 2
    Rectangle { anchors.fill: parent; color: Tide.bad; opacity: 0.9 }
    LogosText {
        anchors.right: parent.right
        anchors.rightMargin: 28
        anchors.bottom: parent.top
        anchors.bottomMargin: 7
        text: "▲ " + line.label
        font.pixelSize: 12
        font.weight: Font.Bold
        color: Tide.badText
    }
}
