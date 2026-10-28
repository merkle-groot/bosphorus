import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Shapes
import Logos.Theme
import Logos.Controls
import "../kit" as Kit
import "../kit/units.js" as Units
import "../kit/tide.js" as Tide

// TDive: a private op being proven, as a coin diving out of sight. Bound to app.dive: a market op's
// job (progress from its state and how long it has been proving), or a transfer (progress from the
// time since it started; the call returns when it lands). Proving takes ~1 min on the GPU, ~10 on a
// CPU; the estimate never claims to be done before the app says so.
Rectangle {
    id: dive
    property var app
    color: Tide.sky

    readonly property var d: app.dive
    readonly property var job: {
        if (!d || !d.job) return null
        for (var i = 0; i < app.jobs.length; i++) if (app.jobs[i].id === d.job) return app.jobs[i]
        return null
    }
    readonly property bool gpu: app.gpu
    readonly property real expectMs: gpu ? 75000 : 600000
    property real now: Date.now()
    Timer { interval: 250; repeat: true; running: dive.visible && !dive.finished; onTriggered: dive.now = Date.now() }

    // A transfer's failure is its call's error (d.error); a job's, its end state.
    readonly property bool failed: job !== null ? ["failed", "interrupted", "unknown"].indexOf(job.state) >= 0 : !!(d && d.error)
    readonly property bool finished: failed || (job !== null ? job.state === "done" : !!(d && d.done))
    readonly property string endState: job !== null ? job.state : (d && d.error ? (d.error.code === "unconfirmed" ? "unknown" : "failed") : "")
    // Proving longer than expected: say so rather than count down past zero.
    readonly property bool late: !finished && (job !== null ? job.state === "proving" && now - job.since_ms > expectMs : !!(d && d.started) && now - d.started > expectMs)
    readonly property real progress: {
        if (!d) return 0
        if (job === null && d.job) return 2
        if (job !== null) {
            switch (job.state) {
            case "queued": return 3
            case "preparing": return 8
            case "proving": return 10 + 82 * Math.min(1, (now - job.since_ms) / expectMs)
            case "sending": return 93
            case "submitted": case "checking": return 96
            case "done": return 100
            }
            return 50
        }
        return d.done ? 100 : Math.min(95, 2 + 93 * (now - d.started) / expectMs)
    }
    readonly property real p: Math.round(progress)
    readonly property real leftS: Math.max(1, Math.round((100 - p) / 100 * expectMs / 1000))

    readonly property var steps: [
        { title: qsTr("Checking your balance"), body: qsTr("Read here, on this computer."), end: 10 },
        { title: qsTr("Hiding who you are"), body: qsTr("Your account, your code and where the money lands go below the surface."), end: 30 },
        { title: qsTr("Proving it’s safe"), body: qsTr("Shows the change keeps the water below the line, without showing whose it is."), end: 92 },
        { title: qsTr("Sending"), body: qsTr("Only the proof comes back up."), end: 100 }
    ]

    MouseArea { anchors.fill: parent }   // the screen under the dive isn't live

    // The sky.
    RowLayout {
        id: top
        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
        anchors.leftMargin: 28; anchors.rightMargin: 28
        height: 70
        LogosText { text: "Bosphorus"; font.pixelSize: 17; font.weight: Font.ExtraBold; Layout.fillWidth: true }
        LogosText { text: dive.gpu ? qsTr("Diving on the graphics card · about 1 minute") : qsTr("Diving on the processor · about 10 minutes"); font.pixelSize: 13; color: "#D9FFFFFF"; Layout.fillWidth: true; horizontalAlignment: Text.AlignHCenter }
        Item {
            Layout.fillWidth: true
            implicitHeight: 36
            Kit.Pill { anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter; pillHeight: 36; text: qsTr("Hide"); onClicked: dive.app.dive = null }
        }
    }
    RowLayout {
        anchors.left: parent.left; anchors.right: parent.right; anchors.top: top.bottom
        anchors.leftMargin: 28; anchors.rightMargin: 28; anchors.topMargin: 4
        spacing: 20
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            LogosText {
                text: dive.failed ? (dive.endState === "interrupted" ? qsTr("Paused.") : dive.endState === "unknown" ? qsTr("Unsure.") : qsTr("Didn’t go through."))
                    : dive.finished ? qsTr("Out of sight.") : qsTr("Diving…")
                font.pixelSize: 52; font.weight: Font.ExtraBold; font.letterSpacing: -52 * 0.045
            }
            LogosText { text: dive.d ? dive.d.title : ""; textFormat: Text.PlainText; font.pixelSize: 17; color: "#D9FFFFFF" }
        }
        ColumnLayout {
            Layout.maximumWidth: 330
            Layout.alignment: Qt.AlignBottom
            spacing: 2
            LogosText { text: qsTr("Seen from above"); font.pixelSize: 14; font.weight: Font.Bold }
            LogosText { Layout.fillWidth: true; wrapMode: Text.Wrap; text: dive.d ? dive.d.seen : ""; textFormat: Text.PlainText; font.pixelSize: 14; color: "#D9FFFFFF" }
        }
    }

    // The sea, deeper as it goes down.
    Rectangle {
        id: sea
        anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
        height: parent.height * 0.7
        gradient: Gradient {
            GradientStop { position: 0.0; color: Tide.dive[0] }
            GradientStop { position: 0.45; color: Tide.dive[1] }
            GradientStop { position: 0.8; color: Tide.dive[2] }
            GradientStop { position: 1.0; color: Tide.dive[3] }
        }
        Kit.Waves { width: parent.width; color: Tide.dive[0] }

        // Steps.
        Column {
            x: 28; y: 34
            width: 300
            spacing: 16
            Repeater {
                model: dive.steps
                delegate: Row {
                    readonly property real start: index === 0 ? 0 : dive.steps[index - 1].end
                    readonly property bool done: dive.p >= modelData.end || (dive.finished && !dive.failed)
                    readonly property bool now: !done && dive.p >= start
                    spacing: 12
                    opacity: done || now ? 1 : 0.5
                    Rectangle {
                        width: 22; height: 22; radius: 11
                        color: done ? "#FFFFFF" : "transparent"
                        border.width: done ? 0 : (now ? 2 : 1.5)
                        border.color: now ? "#FFFFFF" : "#73FFFFFF"
                        LogosText { anchors.centerIn: parent; visible: done; text: "✓"; font.pixelSize: 12; font.weight: Font.ExtraBold; color: Tide.sky }
                    }
                    Column {
                        width: 266
                        spacing: 1
                        LogosText { text: modelData.title; font.pixelSize: 15; font.weight: Font.Bold }
                        LogosText { width: parent.width; wrapMode: Text.Wrap; text: modelData.body; font.pixelSize: 13; color: "#D9FFFFFF" }
                    }
                }
            }
        }

        // The coin.
        Rectangle {
            id: coin
            width: 76; height: 76; radius: 38
            x: (sea.width - width) / 2
            y: sea.height * (0.04 + Math.min(dive.p, 100) * 0.0078) - 10
            Behavior on y { NumberAnimation { duration: 400; easing.type: Easing.OutCubic } }
            color: "#FFFFFF"
            Rectangle { anchors.centerIn: parent; width: 96; height: 96; radius: 48; color: "transparent"; border.width: 10; border.color: "#1FFFFFFF" }
            // A padlock.
            Shape {
                anchors.centerIn: parent
                width: 30; height: 30
                preferredRendererType: Shape.CurveRenderer
                ShapePath {
                    strokeColor: Tide.sky; strokeWidth: 3; fillColor: "transparent"; capStyle: ShapePath.RoundCap
                    scale: Qt.size(30 / 18, 30 / 18)
                    PathSvg { path: "M5.5 8 h7 a2 2 0 0 1 2 2 v3.5 a2 2 0 0 1 -2 2 h-7 a2 2 0 0 1 -2 -2 v-3.5 a2 2 0 0 1 2 -2 Z M6 8 V5.8 a3 3 0 0 1 6 0 V8" }
                }
            }
            // Bubbles while it works.
            Repeater {
                model: dive.finished ? 0 : 2
                delegate: Rectangle {
                    width: index === 0 ? 8 : 6; height: width; radius: width / 2
                    color: "transparent"; border.width: 1.5; border.color: "#B3FFFFFF"
                    x: index === 0 ? 12 : 58
                    SequentialAnimation on y {
                        loops: Animation.Infinite
                        PauseAnimation { duration: index * 1100 }
                        NumberAnimation { from: -6; to: -60; duration: 2200 }
                    }
                    SequentialAnimation on opacity {
                        loops: Animation.Infinite
                        PauseAnimation { duration: index * 1100 }
                        NumberAnimation { from: 1; to: 0; duration: 2200 }
                    }
                }
            }
        }
        LogosText {
            visible: !dive.finished
            x: coin.x + coin.width + 18
            y: coin.y + 20
            text: dive.p + "%"
            font.pixelSize: 28; font.weight: Font.ExtraBold
        }

        // The depth ruler: what has gone under.
        Item {
            anchors.right: parent.right; anchors.rightMargin: 28
            anchors.top: parent.top; anchors.topMargin: 10
            anchors.bottom: parent.bottom; anchors.bottomMargin: 24
            width: 120
            Rectangle { anchors.right: parent.right; width: 2; height: parent.height; color: "#59FFFFFF" }
            Repeater {
                model: [qsTr("Surface"), qsTr("Who you are"), qsTr("Your code"), qsTr("Where it lands"), qsTr("Out of sight")]
                delegate: LogosText {
                    anchors.right: parent.right; anchors.rightMargin: 12
                    y: (parent.height - height) * index / 4
                    text: modelData; font.pixelSize: 12; font.weight: Font.Bold; color: "#D9FFFFFF"
                }
            }
        }

        // Footer.
        ColumnLayout {
            anchors.left: parent.left; anchors.leftMargin: 28
            anchors.right: parent.right; anchors.rightMargin: 180
            anchors.bottom: parent.bottom; anchors.bottomMargin: 26
            spacing: 12
            ColumnLayout {
                visible: !dive.finished
                spacing: 2
                LogosText {
                    text: {
                        var s = dive.job !== null ? dive.job.state : ""
                        if (s === "queued") return qsTr("Waiting for the one before it")
                        if (s === "submitted" || s === "checking") return qsTr("Sent. Waiting for the network to take it")
                        if (dive.late) return qsTr("Taking longer than usual…")
                        return dive.leftS >= 60 ? qsTr("About %1 minute(s) left").arg(Math.ceil(dive.leftS / 60)) : qsTr("About %1 seconds left").arg(dive.leftS)
                    }
                    font.pixelSize: 17; font.weight: Font.Bold
                }
                LogosText {
                    Layout.maximumWidth: 520
                    wrapMode: Text.Wrap
                    text: dive.job !== null && dive.job.state === "submitted" && dive.job.note ? dive.app.sentText(dive.job)
                        : dive.d && dive.d.job ? qsTr("Keep Bosphorus open. If you close it now, the dive stops before anything is sent; you can start it again from Activity.")
                        : qsTr("Keep Bosphorus open until it lands.")
                    textFormat: Text.PlainText
                    font.pixelSize: 14; color: "#D9FFFFFF"
                }
            }
            LogosText {
                visible: dive.finished
                Layout.maximumWidth: 520
                wrapMode: Text.Wrap
                font.pixelSize: 15; color: "#E6FFFFFF"
                textFormat: Text.PlainText
                text: dive.failed ? (dive.job === null ? dive.app.explain(dive.d.error)
                                     : dive.job.state === "interrupted" ? qsTr("Bosphorus stopped before this was sent: nothing moved. You can start it again from Activity.")
                                     : dive.job.error ? dive.app.explain(dive.job.error) : String(dive.job.note || qsTr("See Activity for what happened.")))
                    : (dive.d && dive.d.job ? qsTr("Sent and landed. It’s in your position now.") : qsTr("Sent and landed. It’s in the account now."))
            }
            Row {
                visible: dive.finished
                spacing: 10
                Kit.BigButton { buttonHeight: 48; outline: true; text: qsTr("See in Activity"); onClicked: { if (dive.d && dive.d.job) dive.app.selectedJob = dive.d.job; dive.app.tab = 3; dive.app.dive = null } }
                Kit.BigButton { buttonHeight: 48; text: dive.d && dive.d.job ? qsTr("Back to position") : qsTr("Back to wallet"); onClicked: { dive.app.tab = dive.d && dive.d.job ? 2 : 0; dive.app.dive = null } }
            }
        }
    }
}
