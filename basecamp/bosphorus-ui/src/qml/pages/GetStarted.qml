import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Logos.Theme
import Logos.Controls
import "../kit" as Kit
import "../kit/units.js" as Units
import "../kit/tide.js" as Tide

// The next-step bar (onboarding v2): one step at a time from the starter WBTC to a private borrow
// and a private save, each done when the wallet shows it (the view's `checklist`). Borrow and
// save are done on Markets and Position, by the user: "Show me" starts the view's guide there.
Rectangle {
    id: bar
    objectName: "getStarted"
    property var app
    property var wallet                // the Wallet page (unused; kept for the page's binding)
    Layout.fillWidth: true
    Layout.topMargin: 4
    Layout.bottomMargin: 18
    implicitHeight: col.implicitHeight + 36
    radius: 22
    color: "#0E1B30"
    border.color: "#24FFFFFF"

    readonly property var steps: app.checklist
    function done(key) { for (var i = 0; i < steps.length; i++) if (steps[i].key === key) return steps[i].done; return false }
    // The first step not done yet; "" once all are, when the bar offers the tour.
    readonly property string next: { for (var i = 0; i < steps.length; i++) if (!steps[i].done) return steps[i].key; return "" }
    property real now: Date.now()
    Timer { interval: 1000; repeat: true; running: bar.visible && bar.claimingWbtc; onTriggered: bar.now = Date.now() }

    readonly property string usdc: app.tokenId("USDC")
    readonly property string wbtc: app.tokenId("WBTC")
    readonly property bool claimingWbtc: !!app.claiming[wbtc]
    function amountText(raw, id, places) { return Units.format(raw, app.tokenDecimals(id), places === undefined ? 2 : places) }
    function dropText(id) { var d = app.dropOf(id); return d ? amountText(d.amount, id, 4) : "" }
    function since(t) { return Units.ago(now - t) }
    readonly property string borrowRaw: app.suggestedBorrow()
    readonly property string saveRaw: app.suggestedSave()

    // What the bar says now: { title, detail, button, enabled, act, alt } (act/alt: functions).
    readonly property var say: {
        var a = app
        if (next === "starter") {
            if (claimingWbtc) return { title: qsTr("Getting your WBTC"), detail: qsTr("Your computer is proving the claim privately: %1. %2 so far.").arg(a.diveTime).arg(since(a.starterStarted || now)) }
            var wait = a.claimWait("WBTC")
            return { title: qsTr("Get some test WBTC"), detail: wait !== "" ? wait : qsTr("%1 WBTC, straight into your Private account. Free, and worth nothing.").arg(dropText(wbtc)),
                     button: qsTr("Get WBTC"), enabled: a.canClaim("WBTC"), act: function() { a.startStarter() } }
        }
        if (next === "borrow") {
            var coaching = a.coachGoal === "borrow"
            return { title: borrowRaw !== "0" ? qsTr("Borrow %1 USDC against your WBTC").arg(amountText(borrowRaw, usdc, 0)) : qsTr("Borrow USDC against your WBTC"),
                     detail: (borrowRaw !== "0" ? qsTr("WBTC could fall about %1% before any is sold. Private: nobody can see who borrowed. ").arg(a.fallPct) : "")
                             + (coaching ? qsTr("Follow the highlights: Markets, then Position.") : qsTr("We'll show you where to click, on Markets and then Position.")),
                     button: coaching ? qsTr("Show me again") : qsTr("Show me"), enabled: true, act: function() { a.startCoach("borrow") } }
        }
        if (next === "save") {
            if (a.claiming[usdc]) return { title: qsTr("Getting USDC to lend"), detail: qsTr("%1 USDC from the faucet. Your computer is proving the claim: %2.").arg(dropText(usdc)).arg(a.diveTime) }
            if (!a.hasSaveUsdc && !a.claimedLately(usdc)) {
                var usdcWait = a.claimWait("USDC")
                return { title: qsTr("Get USDC to lend"), detail: usdcWait !== "" ? usdcWait : qsTr("%1 USDC from the faucet, into your Private account.").arg(dropText(usdc)),
                         button: qsTr("Get USDC"), enabled: a.canClaim("USDC"), act: function() { a.claimToken("USDC") } }
            }
            return { title: qsTr("Lend %1 USDC").arg(amountText(saveRaw, usdc, 0)),
                     detail: qsTr("Lenders earn what borrowers pay. Private too: the market sees new lending, not whose. ")
                             + (a.coachGoal === "save" ? qsTr("Follow the highlights.") : qsTr("We'll show you where.")),
                     button: a.coachGoal === "save" ? qsTr("Show me again") : qsTr("Show me"), enabled: true, act: function() { a.startCoach("save") } }
        }
        return { title: qsTr("You're all set"), detail: qsTr("You borrowed and lent without anyone seeing it was you. A one-minute tour shows the rest."),
                 button: qsTr("Take the tour"), enabled: true, act: function() { a.startTour() },
                 alt: qsTr("Hide"), altAct: function() { a.saveUi({ tourDone: true }) } }
    }

    ColumnLayout {
        id: col
        x: 20; y: 18
        width: parent.width - 40
        spacing: 12

        RowLayout {
            Layout.fillWidth: true
            spacing: 6
            Repeater {
                model: [{ key: "starter", label: qsTr("WBTC") }, { key: "borrow", label: qsTr("Borrow") }, { key: "save", label: qsTr("Lend") }]
                delegate: Rectangle {
                    readonly property bool isDone: bar.done(modelData.key)
                    readonly property bool isNext: bar.next === modelData.key
                    implicitHeight: 26
                    implicitWidth: chip.implicitWidth + 22
                    radius: 13
                    color: isDone ? "#1A5EE08A" : isNext ? "#FFFFFF" : "#14FFFFFF"
                    LogosText { id: chip; anchors.centerIn: parent; text: (isDone ? "✓  " : "") + modelData.label; font.pixelSize: 12; font.weight: Font.Bold
                                color: isDone ? "#B8F5CB" : isNext ? "#0B1424" : "#99FFFFFF" }
                }
            }
            Item { Layout.fillWidth: true }
            LogosText { text: qsTr("Testnet money: free, and worth nothing"); font.pixelSize: 12; color: "#80FFFFFF" }
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: 16
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 3
                LogosText { Layout.fillWidth: true; wrapMode: Text.Wrap; text: bar.say.title; font.pixelSize: 19; font.weight: Font.ExtraBold; font.letterSpacing: -0.3 }
                LogosText { Layout.fillWidth: true; wrapMode: Text.Wrap; text: bar.say.detail || ""; font.pixelSize: 13; color: "#B8FFFFFF"; visible: text !== "" }
            }
            LogosSpinner { visible: !bar.say.button; Layout.preferredWidth: 26; Layout.preferredHeight: 26; ringColor: "#FFFFFF" }
            ColumnLayout {
                visible: !!bar.say.button
                spacing: 6
                Kit.BigButton {
                    objectName: "nextStep"
                    Layout.alignment: Qt.AlignRight
                    buttonHeight: 40
                    fontSize: 14
                    text: bar.say.button || ""
                    enabled: !!bar.say.enabled
                    onClicked: if (bar.say.act) bar.say.act()
                }
                LogosText {
                    visible: !!bar.say.alt
                    Layout.alignment: Qt.AlignRight
                    text: bar.say.alt || ""
                    font.pixelSize: 13; font.underline: true; color: "#CCFFFFFF"
                    MouseArea { anchors.fill: parent; anchors.margins: -6; cursorShape: Qt.PointingHandCursor; onClicked: if (bar.say.altAct) bar.say.altAct() }
                }
            }
        }
    }
}
