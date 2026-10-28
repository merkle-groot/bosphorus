import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Logos.Theme
import Logos.Controls
import "../kit" as Kit
import "../kit/units.js" as Units
import "../kit/tide.js" as Tide

// TActivity: every op as a job (left), the chosen one in full (right), and the seabed: the status
// strip (network, your copy, prover, lending program, privacy system), amber or red when something
// is wrong.
Item {
    id: page
    property var app
    property int topInset: 70
    property bool details: false

    readonly property var cur: {
        var js = app.jobs
        for (var i = 0; i < js.length; i++) if (js[i].id === app.selectedJob) return js[i]
        return js.length > 0 ? js[0] : null
    }
    readonly property bool gpu: app.gpu
    property bool resubmitting: false

    // A newer job with the same request: this one was already done again.
    function retried(j) {
        var r = JSON.stringify(j.request)
        return page.app.jobs.some(function(k) { return k.created_ms > j.created_ms && JSON.stringify(k.request) === r })
    }

    function isPrivateJob(j) { return !!(j.expect && j.expect.private && j.expect.private.length > 0) || Units.isPrivate(j.request.owner || j.request.from) }
    function title(j) { return Tide.opText((j.request || {}).op, page.amountOf(j), false) }
    function amountOf(j) {
        var r = j.request || {}
        var m = page.app.marketById(r.market)
        var collateral = r.op === "supply_collateral" || r.op === "withdraw_collateral"
        var token = m ? (collateral ? m.collateral_asset : m.loan_asset) : ""
        var dec = m ? ((collateral ? m.collateral_decimals : m.loan_decimals) || 0) : 0
        var shares = (r.op === "withdraw" || r.op === "repay") && r.unit !== "assets" && r.amount !== "all"
        var amount = r.amount === "all" ? qsTr("all") : (shares ? Units.format(r.amount, 0) + " " + qsTr("shares") : Units.format(r.amount, dec) + " " + page.app.tokenSymbol(token))
        return amount
    }
    // What stays visible of a private op, as Position says it when submitting.
    function seenOf(j) {
        var r = j.request || {}
        var a = page.amountOf(j)
        switch (r.op) {
        case "supply": return r.held ? qsTr("%1 came into the market. New market totals. No position.").arg(a) : qsTr("Some position saved %1. New market totals.").arg(a)
        case "withdraw": return r.held ? qsTr("Savings left the market. New market totals. No position.") : qsTr("Some position took back savings.")
        case "supply_collateral": return qsTr("Some position added %1 of collateral.").arg(a)
        case "withdraw_collateral": return qsTr("Some position removed collateral.")
        case "borrow": return qsTr("Some account now owes %1 more. %1 left the market.").arg(a)
        case "repay": return qsTr("Some position paid back part of its debt.")
        }
        return qsTr("The market's totals change. Not who changed them.")
    }
    function where(j) {
        var r = j.request || {}
        var m = page.app.marketById(r.market)
        return page.app.accountName(r.owner || r.from || "") + (m ? " · " + page.app.marketName(m).replace(" / ", " · ") : "")
    }
    function explain(j) {
        var priv = page.isPrivateJob(j)
        switch (j.state) {
        case "queued": return qsTr("Waiting for the one before it. Bosphorus does one thing at a time so nothing gets tangled.")
        case "preparing": return qsTr("Reading the market and checking this the way the program will.")
        case "proving": return (page.gpu ? qsTr("Diving now, about a minute on the graphics card.") : qsTr("Diving on this computer’s processor. That takes about 10 minutes."))
                               + " " + qsTr("Keep Bosphorus open: if you close it, the dive stops before anything is sent, and you can start it again here.")
        case "sending": return qsTr("Signing and sending.")
        case "submitted": return page.app.sentText(j)
        case "checking": return qsTr("Bosphorus restarted while this was on its way. We’re asking the network what happened. Nothing for you to do.")
        case "done": return (priv ? qsTr("Done. No one can tell it’s yours.") : qsTr("Done.")) + (j.note && j.note.indexOf("sync the wallet") >= 0 ? " " + qsTr("Update your wallet to see it in your private balances.") : "")
        case "failed": return page.app.explain(j.error) + (j.tx || !j.error || ["rejected", "not_included", "network"].indexOf(j.error.code) >= 0 ? "" : " " + qsTr("Nothing moved."))
        case "interrupted": return qsTr("Bosphorus closed before this was sent. Nothing left your computer and no money moved. It’s safe to do it again.")
        case "unknown": return qsTr("This was sent, but we can’t see yet whether it arrived. Update your wallet to find out. Don’t do it again before then, or you might pay twice.") + (j.note ? "\n" + j.note : "")
        }
        return ""
    }

    readonly property var banner: {
        var i = app.info
        if (app.weather === "offline") return [qsTr("No connection."), qsTr("Bosphorus can’t reach the network. Nothing is lost: what was sent lands or not on its own, and Bosphorus picks up when the connection is back."), Tide.badTint, "#FFE0DC"]
        if (!i) return null
        if (app.weather === "stalled") return [qsTr("The network is stalled."), qsTr("No new blocks for a few minutes. Anything sent waits until it moves again; don’t send it twice."), Tide.warnTint, "#FFEBC7"]
        if (i.lending_problem) return [qsTr("Saving and borrowing are paused."), qsTr("The lending program isn’t the one Bosphorus expects. Your money is safe; new actions wait until it’s fixed."), Tide.badTint, "#FFE0DC"]
        if (i.privacy_circuit_problem) return [qsTr("Diving isn’t working right now."), qsTr("Something is wrong with the privacy system. Everyday accounts still work."), Tide.badTint, "#FFE0DC"]
        if (app.weather === "behind") return [qsTr("Catching up."), qsTr("Your private copy is %1 blocks behind. Private balances may be out of date until you Update.").arg(i.last_block - i.synced_block), Tide.infoTint, Tide.infoText]
        return null
    }
    readonly property color sea: app.weather === "calm" ? Tide.waterDeep : app.weather === "behind" ? "#1A4A7A" : app.weather === "stalled" ? "#6E4A08" : "#7A1E17"

    ColumnLayout {
        anchors.fill: parent
        anchors.topMargin: page.topInset
        spacing: 0

        Rectangle {
            visible: page.banner !== null
            Layout.fillWidth: true
            Layout.leftMargin: 28; Layout.rightMargin: 28; Layout.bottomMargin: 6
            implicitHeight: bannerRow.implicitHeight + 24
            radius: 16
            color: page.banner ? page.banner[2] : "transparent"
            RowLayout {
                id: bannerRow
                anchors.fill: parent; anchors.margins: 12; anchors.leftMargin: 16
                spacing: 10
                LogosText { text: page.banner ? page.banner[0] : ""; font.weight: Font.Bold; font.pixelSize: 14; color: page.banner ? page.banner[3] : "#FFFFFF" }
                LogosText { Layout.fillWidth: true; wrapMode: Text.Wrap; text: page.banner ? page.banner[1] : ""; font.pixelSize: 14; color: page.banner ? page.banner[3] : "#FFFFFF" }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.leftMargin: 28; Layout.rightMargin: 28; Layout.topMargin: 10; Layout.bottomMargin: 50
            spacing: 24

            // The list.
            ColumnLayout {
                Layout.preferredWidth: 400
                Layout.maximumWidth: 400
                Layout.minimumWidth: 290
                Layout.fillHeight: true
                spacing: 6
                LogosText { text: qsTr("Activity"); font.pixelSize: 40; font.weight: Font.ExtraBold; font.letterSpacing: -1.6; Layout.bottomMargin: 6 }
                LogosText { visible: page.app.jobs.length === 0; text: qsTr("Nothing yet. What you do in Position shows up here."); color: Tide.text2 }
                ListView {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    spacing: 6
                    model: page.app.jobs
                    delegate: Rectangle {
                        width: ListView.view.width
                        height: Math.max(58, jobCol.implicitHeight + 16)
                        radius: 16
                        readonly property bool on: page.cur !== null && page.cur.id === modelData.id
                        readonly property bool priv: page.isPrivateJob(modelData)
                        color: on ? Tide.glass : "transparent"
                        border.width: 1.5
                        border.color: on ? "#FFFFFF" : "transparent"
                        RowLayout {
                            anchors.fill: parent; anchors.leftMargin: 14; anchors.rightMargin: 14
                            spacing: 12
                            // How much of it sits below the waterline: private 70%, public 30%.
                            Rectangle {
                                width: 10; height: 36; radius: 5; clip: true
                                color: priv ? "#26FFFFFF" : "#FFFFFF"
                                Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: parent.height * (priv ? 0.7 : 0.3); color: Tide.waterCalm }
                            }
                            ColumnLayout {
                                id: jobCol
                                Layout.fillWidth: true
                                spacing: 0
                                LogosText { text: page.title(modelData); font.pixelSize: 14; font.weight: Font.Bold; elide: Text.ElideRight; Layout.fillWidth: true }
                                LogosText { text: page.where(modelData); font.pixelSize: 12; color: Tide.text3; elide: Text.ElideRight; Layout.fillWidth: true }
                            }
                            Kit.TagLabel { text: Tide.stateOf(modelData.state)[0]; color: Tide.stateOf(modelData.state)[2]; fg: Tide.stateOf(modelData.state)[3] }
                        }
                        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: { page.app.selectedJob = modelData.id; page.details = false } }
                    }
                }
            }

            // The chosen job.
            Rectangle {
                visible: page.cur !== null
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignTop
                implicitHeight: detailCol.implicitHeight + 48
                radius: 22
                color: Tide.panel
                border.color: Tide.panelBorder
                readonly property var k: page.cur ? Tide.stateOf(page.cur.state) : ["", "", "", ""]
                readonly property int at: page.cur ? Tide.stepOf(page.cur) : 0
                readonly property bool odd: page.cur !== null && ["failed", "interrupted", "unknown", "checking"].indexOf(page.cur.state) >= 0
                id: detail
                ColumnLayout {
                    id: detailCol
                    anchors.fill: parent; anchors.margins: 24
                    spacing: 18
                    RowLayout {
                        Layout.fillWidth: true
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2
                            LogosText { Layout.fillWidth: true; text: page.cur ? page.where(page.cur) : ""; font.pixelSize: 13; color: Tide.text3 }
                            LogosText { Layout.fillWidth: true; wrapMode: Text.Wrap; text: page.cur ? page.title(page.cur) : ""; font.pixelSize: 32; font.weight: Font.ExtraBold; font.letterSpacing: -1 }
                        }
                        Kit.TagLabel { Layout.alignment: Qt.AlignTop; big: true; text: detail.k[0]; color: detail.k[2]; fg: detail.k[3] }
                    }
                    // Five steps.
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 6
                        Repeater {
                            model: page.cur && page.isPrivateJob(page.cur) ? [qsTr("Waiting"), qsTr("Getting ready"), qsTr("Diving"), qsTr("Sent"), qsTr("Done")]
                                                                          : [qsTr("Waiting"), qsTr("Getting ready"), qsTr("Sending"), qsTr("Sent"), qsTr("Done")]
                            delegate: ColumnLayout {
                                Layout.fillWidth: true
                                Layout.preferredWidth: 1
                                spacing: 6
                                readonly property bool past: index < detail.at || (page.cur && page.cur.state === "done")
                                readonly property bool now: index === detail.at && !(page.cur && page.cur.state === "done")
                                Rectangle { Layout.fillWidth: true; height: 6; radius: 3; color: now ? detail.k[1] : (past ? "#FFFFFF" : "#2EFFFFFF") }
                                LogosText {
                                    text: now && detail.odd ? detail.k[0] : modelData
                                    font.pixelSize: 12; font.weight: now ? Font.ExtraBold : Font.DemiBold
                                    color: now ? detail.k[3] : (past ? "#FFFFFF" : "#99FFFFFF")
                                }
                            }
                        }
                    }
                    LogosText { Layout.fillWidth: true; Layout.maximumWidth: 640; wrapMode: Text.Wrap; textFormat: Text.PlainText; font.pixelSize: 16; text: page.cur ? page.explain(page.cur) : "" }
                    Flow {
                        Layout.fillWidth: true
                        spacing: 10
                        Kit.BigButton {
                            visible: page.cur !== null && page.cur.state === "proving"
                            buttonHeight: 48; fontSize: 15
                            text: qsTr("Watch the dive")
                            onClicked: page.app.dive = { job: page.cur.id, title: Tide.opText((page.cur.request || {}).op, page.amountOf(page.cur), true) + qsTr(" · %1").arg(page.app.accountName(page.cur.request.owner || page.cur.request.from)), seen: page.seenOf(page.cur) }
                        }
                        Kit.BigButton {
                            visible: page.cur !== null && page.cur.state === "interrupted" && !page.retried(page.cur)
                            buttonHeight: 48; fontSize: 15
                            enabled: !page.resubmitting && page.app.online
                            text: page.cur && page.isPrivateJob(page.cur) ? qsTr("Dive again") : qsTr("Send again")
                            onClicked: {
                                page.resubmitting = true
                                page.app.call("submit", page.cur.request, function(job) { page.resubmitting = false; page.app.selectedJob = job.id; page.app.refreshJobs() },
                                              function(e) { page.resubmitting = false; page.app.report(e) })
                            }
                        }
                        LogosText {
                            visible: page.cur !== null && page.cur.state === "interrupted" && page.retried(page.cur)
                            text: qsTr("Already started again: see the newer one above.")
                            color: Tide.text2
                        }
                        Kit.BigButton {
                            visible: page.cur !== null && page.cur.state === "unknown"
                            buttonHeight: 48; fontSize: 15
                            enabled: !page.app.syncing && page.app.online
                            text: page.app.syncing ? qsTr("Updating…") : qsTr("Update wallet")
                            onClicked: page.app.syncWallet()
                        }
                        Kit.BigButton {
                            buttonHeight: 48; fontSize: 15; outline: true
                            text: page.details ? qsTr("Hide details") : qsTr("Details")
                            onClicked: page.details = !page.details
                        }
                    }
                    Rectangle {
                        visible: page.details && page.cur !== null
                        Layout.fillWidth: true
                        implicitHeight: techCol.implicitHeight + 28
                        radius: 14
                        color: "#40000000"
                        ColumnLayout {
                            id: techCol
                            anchors.fill: parent; anchors.margins: 14
                            spacing: 6
                            Repeater {
                                model: page.cur ? [
                                    ["job", "#" + page.cur.id], ["state", page.cur.state], ["tx", page.cur.tx || "not sent yet"],
                                    ["block", page.cur.block ? String(page.cur.block) : "–"],
                                    ["proving", page.cur.proving_ms ? (page.cur.proving_ms / 1000).toFixed(1) + " s" : "–"],
                                    ["expected", page.cur.expect ? page.cur.expect.field + " " + page.cur.expect.from + " → " + page.cur.expect.to : "–"]
                                ] : []
                                delegate: RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 14
                                    LogosText { Layout.preferredWidth: 70; text: modelData[0]; font.family: Theme.typography.mono; font.pixelSize: 12; color: Tide.text4 }
                                    LogosText { Layout.fillWidth: true; text: modelData[1]; textFormat: Text.PlainText; wrapMode: Text.WrapAnywhere; font.family: Theme.typography.mono; font.pixelSize: 12; color: "#D9FFFFFF" }
                                }
                            }
                        }
                    }
                }
            }
        }

        // The seabed.
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: seabed.implicitHeight + 46
            color: page.sea
            Behavior on color { ColorAnimation { duration: 600 } }
            Kit.Waves { width: parent.width; color: page.sea }
            Flow {
                id: seabed
                anchors.left: parent.left; anchors.right: parent.right
                anchors.leftMargin: 28; anchors.rightMargin: 28
                y: 22
                spacing: 28
                Repeater {
                    model: {
                        var i = page.app.info
                        var fmt = function(n) { return Number(n).toLocaleString(Qt.locale("en_US"), "f", 0) }
                        return [
                            [qsTr("Network"), page.app.weather === "offline" ? qsTr("No connection") : !i ? "–" : page.app.stalled ? qsTr("Stalled at block %1").arg(fmt(i.last_block)) : qsTr("Block %1").arg(fmt(i.last_block))],
                            [qsTr("Your private copy"), !i ? "–" : (i.last_block - i.synced_block > 30 ? qsTr("Block %1").arg(fmt(i.synced_block)) : qsTr("Up to date"))],
                            [qsTr("Diving on"), page.gpu ? qsTr("Graphics card · ~1 min") : qsTr("Processor · ~10 min")],
                            [qsTr("Lending program"), i && i.lending_problem ? qsTr("Problem") : qsTr("Working")],
                            [qsTr("Privacy system"), i && i.privacy_circuit_problem ? qsTr("Problem") : qsTr("Working")]
                        ]
                    }
                    delegate: Column {
                        width: Math.max(140, implicitWidth)
                        spacing: 1
                        LogosText { text: modelData[0]; font.pixelSize: 12; color: Tide.text2 }
                        LogosText { text: modelData[1]; font.pixelSize: 15; font.weight: Font.ExtraBold }
                    }
                }
            }
        }
    }
}
