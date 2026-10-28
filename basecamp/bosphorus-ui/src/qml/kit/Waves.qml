import QtQuick
import QtQuick.Shapes

// Two wave strips riding on top of a water surface (TWallet/TPosition): a back wave at 45%
// opacity drifting one way over 11 s, a front wave the other way over 7 s. Put it at the water's
// top edge; it draws upwards from y = 0.
Item {
    id: waves
    property color color: "#0A5BB8"
    // One small wave (a market glass) instead of the two full-width strips.
    property bool mini: false
    height: 0
    clip: false

    component Strip: Item {
        id: strip
        property real stripHeight: 30
        property string path: ""
        property real viewWidth: 2400
        property int duration: 7000
        property bool reverse: false
        width: waves.width * 2
        height: stripHeight
        Shape {
            anchors.fill: parent
            preferredRendererType: Shape.CurveRenderer
            ShapePath {
                fillColor: waves.color
                strokeWidth: -1
                PathSvg { path: strip.path }
                scale: Qt.size(strip.width / strip.viewWidth, 1)
            }
        }
        NumberAnimation on x {
            from: strip.reverse ? -waves.width : 0
            to: strip.reverse ? 0 : -waves.width
            duration: strip.duration
            loops: Animation.Infinite
            running: waves.visible && waves.width > 0
        }
    }

    Strip {
        visible: waves.mini
        y: -10
        stripHeight: 14
        viewWidth: 600
        duration: 7000
        path: "M0 7 Q37 0 75 7 T150 7 T225 7 T300 7 T375 7 T450 7 T525 7 T600 7 V14 H0Z"
    }
    Strip {
        visible: !waves.mini
        y: -30
        opacity: 0.45
        stripHeight: 40
        duration: 11000
        reverse: true
        path: "M0 20 Q150 0 300 20 T600 20 T900 20 T1200 20 T1500 20 T1800 20 T2100 20 T2400 20 V40 H0Z"
    }
    Strip {
        visible: !waves.mini
        y: -22
        stripHeight: 30
        duration: 7000
        path: "M0 15 Q100 0 200 15 T400 15 T600 15 T800 15 T1000 15 T1200 15 T1400 15 T1600 15 T1800 15 T2000 15 T2200 15 T2400 15 V30 H0Z"
    }
}
