import QtQuick
import Logos.Controls
import "tide.js" as Tide

// After an op lands (docs/design/wallet-v2/HANDOFF.md, "Position"): money going in falls as coins
// onto the water and rings out from where each lands; money coming out rings first, then rises
// off the surface, with the amount floating up. Coins follow `surfaceY` live, so they meet the
// water wherever its settle animation has it. `run(out, symbol, number)`.
Item {
    id: fx
    anchors.fill: parent
    property real surfaceY: height
    property real ceiling: 0            // coins start and end here (below any header)
    property bool out: false
    property string symbol: ""
    property string number: ""
    readonly property var look: Tide.look(symbol)
    readonly property var xs: [14, 24, 33, 42, 50, 58, 67, 76, 86]

    function run(isOut, sym, num) {
        fx.out = isOut
        fx.symbol = sym
        fx.number = num
        for (var i = 0; i < coins.count; i++) coins.itemAt(i).go()
        if (isOut) floatAnim.restart()
    }

    Repeater {
        id: coins
        model: 9
        delegate: Item {
            id: d
            readonly property real baseX: fx.width * fx.xs[index] / 100
            // Columns of three, a beat apart, so they arrive as a shower rather than a line.
            readonly property int delay: (index % 3) * 120 + Math.floor(index / 3) * 70
            readonly property real sway: (index % 2 ? 1 : -1) * (6 + index % 3 * 2)
            property real t: 0          // fall: top → surface; rise: surface → top
            property real ringT: 0
            property real splashT: 0
            function go() {
                fall.stop(); rise.stop()
                t = 0; ringT = 0; splashT = 0
                coin.opacity = 0
                if (fx.out) rise.start(); else fall.start()
            }

            // The coin.
            Rectangle {
                id: coin
                width: 26; height: 26; radius: 13
                opacity: 0
                color: fx.look[1]
                x: d.baseX - 13 + Math.sin(d.t * Math.PI) * d.sway
                y: fx.out ? (fx.surfaceY + 24) - (fx.surfaceY + 24 - fx.ceiling) * d.t
                          : fx.ceiling + (fx.surfaceY - 13 - fx.ceiling) * d.t
                scale: fx.out ? 0.55 + 0.55 * d.t : 0.65 + 0.35 * Math.min(1, d.t * 1.4)
                LogosText { anchors.centerIn: parent; text: fx.look[0]; font.pixelSize: 13; font.weight: Font.ExtraBold; color: fx.look[2] }
            }
            // The ring where it meets the surface.
            Rectangle {
                width: 90; height: 22; radius: 11
                x: d.baseX - 45
                y: fx.surfaceY - 11
                color: "transparent"
                border.width: 2
                border.color: "#BFFFFFFF"
                scale: 0.2 + 0.8 * d.ringT
                opacity: d.ringT > 0 ? 0.9 * (1 - d.ringT) : 0
            }
            // A splash streak.
            Rectangle {
                width: 6; height: 18; radius: 3
                x: d.baseX - 3
                y: fx.surfaceY - 18 - 26 * d.splashT
                color: "#CCFFFFFF"
                opacity: d.splashT > 0 ? Math.min(1, d.splashT * 3.3) * (1 - d.splashT) * 1.3 : 0
                transform: Scale { origin.y: 18; yScale: 0.2 + 0.8 * d.splashT }
            }

            // In: fall (easing in), fade near the surface, then ring and splash as it lands.
            ParallelAnimation {
                id: fall
                SequentialAnimation {
                    PauseAnimation { duration: d.delay }
                    ParallelAnimation {
                        NumberAnimation { target: d; property: "t"; from: 0; to: 1; duration: 950; easing.type: Easing.InQuad }
                        SequentialAnimation {
                            NumberAnimation { target: coin; property: "opacity"; from: 0; to: 1; duration: 180; easing.type: Easing.OutQuad }
                            PauseAnimation { duration: 620 }
                            NumberAnimation { target: coin; property: "opacity"; to: 0; duration: 150; easing.type: Easing.InQuad }
                        }
                    }
                }
                SequentialAnimation {
                    PauseAnimation { duration: d.delay + 860 }
                    NumberAnimation { target: d; property: "ringT"; from: 0; to: 1; duration: 1100; easing.type: Easing.OutCubic }
                }
                SequentialAnimation {
                    PauseAnimation { duration: d.delay + 900 }
                    NumberAnimation { target: d; property: "splashT"; from: 0; to: 1; duration: 700; easing.type: Easing.OutQuad }
                }
            }
            // Out: ring first, then the coin lifts off the surface and fades as it rises.
            ParallelAnimation {
                id: rise
                SequentialAnimation {
                    PauseAnimation { duration: d.delay }
                    NumberAnimation { target: d; property: "ringT"; from: 0; to: 1; duration: 1100; easing.type: Easing.OutCubic }
                }
                SequentialAnimation {
                    PauseAnimation { duration: d.delay + 140 }
                    ParallelAnimation {
                        NumberAnimation { target: d; property: "t"; from: 0; to: 1; duration: 1600; easing.type: Easing.OutCubic }
                        SequentialAnimation {
                            NumberAnimation { target: coin; property: "opacity"; from: 0; to: 1; duration: 320; easing.type: Easing.OutQuad }
                            NumberAnimation { target: coin; property: "opacity"; to: 0; duration: 1280; easing.type: Easing.InQuad }
                        }
                    }
                }
            }
        }
    }

    // Out: the amount, floating up.
    LogosText {
        id: floatNum
        property real lift: 0
        anchors.horizontalCenter: parent.horizontalCenter
        y: fx.height * 0.38 - lift
        opacity: 0
        text: fx.number
        font.pixelSize: 44; font.weight: Font.ExtraBold; font.letterSpacing: -1.3
        ParallelAnimation {
            id: floatAnim
            NumberAnimation { target: floatNum; property: "lift"; from: 0; to: 90; duration: 1800; easing.type: Easing.OutCubic }
            SequentialAnimation {
                NumberAnimation { target: floatNum; property: "opacity"; from: 0; to: 1; duration: 270; easing.type: Easing.OutQuad }
                PauseAnimation { duration: 450 }
                NumberAnimation { target: floatNum; property: "opacity"; to: 0; duration: 1080; easing.type: Easing.InOutQuad }
            }
        }
    }
}
