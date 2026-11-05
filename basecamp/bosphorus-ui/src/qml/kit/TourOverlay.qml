import QtQuick
import Logos.Theme
import Logos.Controls
import "tide.js" as Tide

// A guided tour over the view: the screen dims, the items a step names (objectName) show through
// rounded holes with a ring, each with a callout (a leader line, a dot on the item's edge, a
// short label), and a card explains the step. Steps are data (tour.js); `stepShown` lets the view
// switch tabs or set things up first. Items that aren't on screen are left out of the step.
// `coach`: the guided first steps instead: one step at a time (show), clicks go through to the
// lit items, and the card has the goal, an optional button (`action`, `act`) and Stop.
Item {
    id: tour
    property Item scope                  // where to look for targets
    property bool coach: false
    property real bottomInset: 0         // kept clear at the bottom (a toast)
    property var steps: []
    property int index: 0
    readonly property var step: index >= 0 && index < steps.length ? steps[index] : null
    readonly property bool last: index === steps.length - 1
    signal stepShown(var step)
    signal finished(bool skipped)

    // The lit items: [{ x, y, w, h, label, side }] in this item's coordinates.
    property var rects: []

    function start() { index = 0; visible = true; stepShown(step); measureSoon.restart() }
    function go(i) {
        if (i < 0) return
        if (i >= steps.length) { visible = false; finished(false); return }
        index = i
        rects = []
        stepShown(step)
        measureSoon.restart()
    }
    function skip() { visible = false; finished(true) }
    // Coach: the step on screen now (it changes as the user acts).
    function show(s) {
        var same = tour.visible && tour.step && tour.step.title === s.title
        steps = [s]; index = 0; visible = true
        if (!same) { rects = []; measureSoon.restart() }
        else measure()
    }

    function shown(item) {
        for (var p = item; p; p = p.parent) if (!p.visible || p.opacity === 0) return false
        return item.width > 0 && item.height > 0
    }
    // The first visible item under `item` with this objectName.
    function find(item, name) {
        if (!item || !item.visible) return null
        if (item.objectName === name) return item
        var kids = item.children || []
        for (var i = 0; i < kids.length; i++) { var f = find(kids[i], name); if (f) return f }
        // An open popup's content lives in the window's overlay, not among the children.
        var data = item.data || []
        for (var j = 0; j < data.length; j++) if (data[j] && data[j].opened && data[j].contentItem) { var g = find(data[j].contentItem, name); if (g) return g }
        return null
    }
    function measure() {
        if (!tour.visible || !tour.step || !tour.scope) return
        var out = [], pad = 8
        tour.step.targets.forEach(function(t) {
            var item = find(tour.scope, t.name)
            if (!item || !shown(item)) return
            // `fit`: the container's contents, not all of its (stretched) area.
            var box = t.fit && item.childrenRect.width > 0 ? item.childrenRect : Qt.rect(0, 0, item.width, item.height)
            var p = item.mapToItem(tour, box.x, box.y)
            var r = { x: p.x - pad, y: p.y - pad, w: box.width + 2 * pad, h: box.height + 2 * pad, label: t.label, side: t.side }
            // Clip to the window, so a tall list still shows a sensible hole.
            r.y = Math.max(r.y, 4); r.h = Math.min(r.h, tour.height - 4 - r.y)
            r.x = Math.max(r.x, 4); r.w = Math.min(r.w, tour.width - 4 - r.x)
            out.push(r)
        })
        if (JSON.stringify(out) !== JSON.stringify(tour.rects)) tour.rects = out
    }
    // Pages lay out after a tab switch; measure a little later, then keep following.
    Timer { id: measureSoon; interval: 350; onTriggered: tour.measure() }
    Timer { interval: 500; repeat: true; running: tour.visible; onTriggered: tour.measure() }
    onWidthChanged: measure()
    onHeightChanged: measure()

    // Clicks stop here; the card's buttons drive the tour.
    MouseArea { enabled: !tour.coach; anchors.fill: parent; hoverEnabled: true; onWheel: function(w) { w.accepted = true } }

    Canvas {
        id: dim
        anchors.fill: parent
        property var holes: tour.rects
        onHolesChanged: requestPaint()
        onWidthChanged: requestPaint()
        onHeightChanged: requestPaint()
        onPaint: {
            var ctx = getContext("2d")
            ctx.reset()
            ctx.fillStyle = tour.coach ? "rgba(3, 8, 18, 0.55)" : "rgba(3, 8, 18, 0.80)"
            ctx.fillRule = Qt.OddEvenFill
            ctx.beginPath()
            ctx.rect(0, 0, width, height)
            holes.forEach(function(h) { ctx.roundedRect(h.x, h.y, h.w, h.h, 14, 14) })
            ctx.fill()
        }
    }

    // A ring and a callout per lit item.
    Repeater {
        model: tour.rects
        delegate: Item {
            id: lit
            readonly property var r: modelData
            readonly property real cx: r.x + r.w / 2
            readonly property real cy: r.y + r.h / 2
            readonly property bool vertical: r.side === "above" || r.side === "below"
            readonly property real reach: 34
            anchors.fill: parent

            Rectangle {
                x: lit.r.x; y: lit.r.y; width: lit.r.w; height: lit.r.h
                radius: 14
                color: "transparent"
                border.width: 2
                border.color: "#FFFFFF"
                SequentialAnimation on opacity {
                    loops: Animation.Infinite
                    NumberAnimation { from: 1; to: 0.45; duration: 900; easing.type: Easing.InOutSine }
                    NumberAnimation { from: 0.45; to: 1; duration: 900; easing.type: Easing.InOutSine }
                }
            }
            // The leader: from a dot on the ring out to the label.
            Rectangle {
                readonly property real ex: lit.r.side === "left" ? lit.r.x : lit.r.side === "right" ? lit.r.x + lit.r.w : lit.cx
                readonly property real ey: lit.r.side === "above" ? lit.r.y : lit.r.side === "below" ? lit.r.y + lit.r.h : lit.cy
                x: ex - 4; y: ey - 4; width: 8; height: 8; radius: 4
                color: "#FFFFFF"
            }
            Rectangle {
                color: "#FFFFFF"
                width: lit.vertical ? 1.5 : lit.reach
                height: lit.vertical ? lit.reach : 1.5
                x: lit.r.side === "left" ? lit.r.x - lit.reach : lit.r.side === "right" ? lit.r.x + lit.r.w : lit.cx - 0.75
                y: lit.r.side === "above" ? lit.r.y - lit.reach : lit.r.side === "below" ? lit.r.y + lit.r.h : lit.cy - 0.75
            }
            // A dark backing, so a label reads over whatever it crosses.
            Rectangle {
                x: label.x - 10; y: label.y - 5
                width: label.width + 20; height: label.height + 10
                radius: 10
                color: "#F2081226"
                border.color: "#33FFFFFF"
            }
            LogosText {
                id: label
                text: lit.r.label
                width: Math.min(implicitWidth, 320)
                wrapMode: Text.Wrap
                horizontalAlignment: lit.vertical ? Text.AlignHCenter : lit.r.side === "left" ? Text.AlignRight : Text.AlignLeft
                font.pixelSize: 15
                font.weight: Font.Bold
                color: "#FFFFFF"
                readonly property real wantX: lit.r.side === "left" ? lit.r.x - lit.reach - 8 - width
                                            : lit.r.side === "right" ? lit.r.x + lit.r.w + lit.reach + 8 : lit.cx - width / 2
                readonly property real wantY: lit.r.side === "above" ? lit.r.y - lit.reach - 6 - height
                                            : lit.r.side === "below" ? lit.r.y + lit.r.h + lit.reach + 6 : lit.cy - height / 2
                x: Math.max(20, Math.min(tour.width - width - 20, wantX))
                y: Math.max(12, Math.min(tour.height - height - 12, wantY))
            }
        }
    }

    // The step's card: away from what it explains.
    Rectangle {
        id: card
        // Bottom, top or middle: wherever it covers least of the lit items and their callouts.
        readonly property real place: {
            var h = height, spots = [tour.height - h - 28 - tour.bottomInset, 84, (tour.height - h) / 2]
            var cover = function(y0) {
                var n = 0
                tour.rects.forEach(function(r) {
                    var top = r.y - (r.side === "above" ? 90 : 0), bottom = r.y + r.h + (r.side === "below" ? 90 : 0)
                    n += Math.max(0, Math.min(bottom, y0 + h) - Math.max(top, y0))
                })
                return n
            }
            var best = spots[0]
            spots.forEach(function(y) { if (cover(y) < cover(best)) best = y })
            return best
        }
        width: Math.min(tour.width - 48, 500)
        height: cardCol.implicitHeight + 40
        x: (tour.width - width) / 2
        y: place
        radius: 22
        color: "#0E1B30"
        border.color: "#33FFFFFF"
        Behavior on y { NumberAnimation { duration: 260; easing.type: Easing.OutCubic } }

        Column {
            id: cardCol
            x: 22; y: 20
            width: parent.width - 44
            spacing: 10
            Item {
                width: parent.width
                height: stepOf.implicitHeight
                LogosText { id: stepOf; text: tour.coach ? (tour.step ? tour.step.goal || "" : "") : qsTr("%1 of %2").arg(tour.index + 1).arg(tour.steps.length); font.pixelSize: 12; font.weight: Font.Bold; color: "#99FFFFFF" }
                LogosText {
                    anchors.right: parent.right
                    text: tour.coach ? qsTr("Stop guide") : qsTr("Skip tour"); font.pixelSize: 13; font.underline: true; color: "#CCFFFFFF"
                    MouseArea { anchors.fill: parent; anchors.margins: -6; cursorShape: Qt.PointingHandCursor; onClicked: tour.skip() }
                }
            }
            LogosText { width: parent.width; wrapMode: Text.Wrap; text: tour.step ? tour.step.title : ""; font.pixelSize: 22; font.weight: Font.ExtraBold; font.letterSpacing: -0.4 }
            LogosText { width: parent.width; wrapMode: Text.Wrap; text: tour.step ? tour.step.body : ""; font.pixelSize: 14; color: "#D9FFFFFF"; lineHeight: 1.15 }
            Item {
                visible: tour.coach && !!(tour.step && tour.step.action)
                width: parent.width
                height: visible ? 44 : 0
                BigButton { objectName: "coachAct"; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter; buttonHeight: 40; fontSize: 14
                            text: tour.step && tour.step.action ? tour.step.action : ""; onClicked: if (tour.step && tour.step.act) tour.step.act() }
            }
            Item {
                visible: !tour.coach
                width: parent.width
                height: visible ? 44 : 0
                Row {
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 6
                    Repeater {
                        model: tour.steps.length
                        delegate: Rectangle { width: index === tour.index ? 18 : 6; height: 6; radius: 3; color: index === tour.index ? "#FFFFFF" : "#4DFFFFFF"; anchors.verticalCenter: parent.verticalCenter }
                    }
                }
                Row {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 8
                    BigButton { visible: tour.index > 0; outline: true; buttonHeight: 40; fontSize: 14; text: qsTr("Back"); onClicked: tour.go(tour.index - 1) }
                    BigButton { objectName: "tourNext"; buttonHeight: 40; fontSize: 14; text: tour.last ? qsTr("Done") : qsTr("Next"); onClicked: tour.go(tour.index + 1) }
                }
            }
        }
    }
}
