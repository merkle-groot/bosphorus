import QtQuick
import QtQuick.Shapes
import Logos.Controls

// The Welcome intro: the lockup (the mark from icons/bosphorus.svg and the wordmark) flashes in
// large, front and centre, holds, then settles small at the top left, where it stays. `done`
// once it has; the page fades the rest in then.
Item {
    id: intro
    property real endScale: 0.29
    property real endMargin: 32
    property real endTop: 22
    property real p: 0                      // 0: centre, 1: settled
    property real flash: 0                  // the entrance, 0..1
    readonly property bool done: !anim.running && intro.p >= 1

    function play() { anim.restart() }
    function finish() { anim.stop(); intro.flash = 1; intro.p = 1 }

    function mix(a, b, t) { return a + (b - a) * t }
    readonly property real sc: mix(1, endScale, p) * mix(0.92, 1, flash)

    Item {
        id: lockup
        width: row.width; height: row.height
        transformOrigin: Item.TopLeft
        scale: intro.sc
        x: intro.mix((intro.width - width * intro.sc) / 2, intro.endMargin, intro.p)
        y: intro.mix((intro.height - height * intro.sc) / 2 - 20, intro.endTop, intro.p)
        opacity: intro.flash

        Row {
            id: row
            spacing: 22
            // The mark: the logo's two bands, drawn in the SVG's units (x 32..224, y 48..208).
            Shape {
                width: 192 * 0.5; height: 160 * 0.5
                anchors.verticalCenter: parent.verticalCenter
                preferredRendererType: Shape.CurveRenderer
                ShapePath {
                    fillColor: "#F4F2EB"; strokeWidth: -1
                    PathSvg { path: "M0 32C28 32 36 0 64 0H96V16H64C44 16 36 48 0 48Z" }
                }
                ShapePath {
                    fillColor: "#9CAEFF"; strokeWidth: -1
                    PathSvg { path: "M0 64C28 64 36 32 64 32H96V48H64C44 48 36 80 0 80Z" }
                }
            }
            LogosText {
                anchors.verticalCenter: parent.verticalCenter
                text: "bosphorus"
                font.pixelSize: 84; font.weight: Font.ExtraBold; font.letterSpacing: -84 * 0.045
            }
        }
    }

    SequentialAnimation {
        id: anim
        PropertyAction { target: intro; property: "p"; value: 0 }
        NumberAnimation { target: intro; property: "flash"; from: 0; to: 1; duration: 700; easing.type: Easing.OutCubic }
        PauseAnimation { duration: 900 }
        NumberAnimation { target: intro; property: "p"; from: 0; to: 1; duration: 900; easing.type: Easing.InOutCubic }
    }
}
