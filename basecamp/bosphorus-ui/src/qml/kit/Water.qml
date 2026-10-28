import QtQuick

// The water: fills the bottom `level` (0..1) of its parent in `color`, with waves on top. Height
// and colour ease, so a change in the position reads as the tide moving; Position settles it with
// a gentle overshoot (`settleMs`, `overshoot`).
Rectangle {
    id: water
    property real level: 0.2
    property bool waves: true
    property bool mini: false
    property int inset: 0
    property int settleMs: 600
    property real overshoot: 0      // 0: OutCubic; above 0: OutBack with this overshoot
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.bottom: parent.bottom
    anchors.margins: inset
    height: (parent.height - 2 * inset) * Math.max(0, level)
    color: "#0A5BB8"
    Behavior on height { NumberAnimation { duration: water.settleMs; easing.type: water.overshoot > 0 ? Easing.OutBack : Easing.OutCubic; easing.overshoot: water.overshoot } }
    Behavior on color { ColorAnimation { duration: Math.max(600, water.settleMs * 0.6) } }
    Waves { visible: water.waves && water.height > 0; mini: water.mini; width: parent.width; color: water.color }
}
