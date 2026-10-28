import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Logos.Theme
import Logos.Controls
import "../kit" as Kit
import "../kit/units.js" as Units
import "../kit/tide.js" as Tide

// Wallet v2 (docs/design/wallet-v2/HANDOFF.md): one Everyday account above the surface and one
// Private account below it, each a list of its non-zero tokens. A row's "Dive ↓" / "Surface ↑" /
// "Send" opens the move sheet with the token and direction filled in; a move under way shows as a
// pill in the header of the zone it lands in. Anything touching a private account is proven here.
Item {
    id: page
    objectName: "walletPage"
    property var app
    property int topInset: 70

    readonly property var everyday: app.accounts.filter(function(a) { return !Units.isPrivate(a.account) })
    readonly property var privates: app.accounts.filter(function(a) { return Units.isPrivate(a.account) })
    // The account each zone shows ("" until there is one); the "⋯" menu switches.
    property string abovePick: ""
    property string belowPick: ""
    readonly property string aboveId: pick(everyday, abovePick)
    readonly property string belowId: pick(privates, belowPick)
    function pick(list, id) {
        for (var i = 0; i < list.length; i++) if (list[i].account === id) return id
        return list.length > 0 ? list[0].account : ""
    }

    // Tokens leave a private account like LEZ does (client/src/funds.rs); off would keep a private
    // token row's Surface and Send for lending ops only.
    readonly property bool privateTokenMoves: true

    // The move on its way: { text, zone, proving, record } (record: the dive it shows when opened).
    property var inflight: null
    property bool creating: false

    function nativeOf(id) {
        for (var i = 0; i < app.accounts.length; i++) if (app.accounts[i].account === id) return app.accounts[i].native
        return null
    }

    // An account's rows: tokens by tokenOrder, then LEZ; zero and unknown balances left out.
    function rows(id) {
        if (id === "") return []
        var out = []
        app.tokens.slice().sort(function(a, b) { return Tide.tokenRank(a.symbol) - Tide.tokenRank(b.symbol) || (a.symbol < b.symbol ? -1 : 1) })
            .forEach(function(t) {
                var b = app.tokenBalance(id, t.id)
                if (b !== null && !Units.isZero(b)) out.push({ token: t.id, symbol: t.symbol, decimals: t.decimals, raw: String(b) })
            })
        var n = nativeOf(id)
        if (n !== null && n !== undefined && !Units.isZero(String(n))) out.push({ token: "", symbol: "LEZ", decimals: 0, raw: String(n) })
        return out
    }
    readonly property var aboveRows: rows(aboveId)
    readonly property var belowRows: rows(belowId)
    readonly property bool belowUnknown: belowId !== "" && nativeOf(belowId) === null

    // "Main", "2": the account's own name, without its zone.
    function shortName(id) {
        var n = app.accountName(id)
        var i = n.indexOf(" · ")
        return i >= 0 ? n.slice(i + 3) : n
    }

    function newAccount(priv) {
        if (page.creating) return
        page.creating = true
        page.app.call("new_account", { private: priv }, function() { page.creating = false; page.app.refreshAll() }, function(e) { page.creating = false; page.app.report(e) })
    }

    // The private zone's freshness note: shown only when its balances may be old.
    readonly property bool showFresh: app.syncing || !app.online || app.weather === "behind"
    readonly property string freshText: {
        if (app.syncing) return qsTr("Checking…")
        if (app.lastSyncAt > 0) return qsTr("Last checked %1 ago").arg(Units.ago(app.now - app.lastSyncAt))
        return qsTr("Not checked lately")
    }

    // ---- the move sheet -----------------------------------------------------------------

    property string kind: "dive"       // dive | surface | send
    property string zone: "above"      // where it leaves from
    property var row: null             // the token row that opened it
    readonly property string fromId: zone === "above" ? aboveId : belowId
    readonly property string toId: kind === "dive" ? belowId : kind === "surface" ? aboveId : toInput.text.trim()
    readonly property bool proving: kind !== "send" ? true : zone === "below"
    readonly property var have: {
        if (!row) return null
        return row.token === "" ? nativeOf(fromId) : app.tokenBalance(fromId, row.token)
    }
    readonly property string raw: row ? Units.parse(amountInput.text, row.decimals) : ""
    readonly property bool tooMany: amountInput.text.trim() !== "" && raw === ""
    readonly property bool over: raw !== "" && have !== null && Units.cmp(raw, String(have)) > 0
    readonly property bool badAddress: kind === "send" && toId !== "" && !/^Public\/[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(toId)
    readonly property string blocker: {
        if (!app.online) return qsTr("No connection")
        if (page.inflight !== null) return qsTr("Wait for the last move")
        if (raw === "" || Units.isZero(raw)) return tooMany ? qsTr("Up to %1 decimals").arg(row ? row.decimals : 0) : qsTr("Enter an amount")
        if (over) return qsTr("More than you have")
        if (kind === "send" && toId === "") return qsTr("Add an address")
        if (badAddress) return qsTr("Check the address")
        return ""
    }
    readonly property string amountText: row && raw !== "" ? Units.format(raw, row.decimals, row.decimals) + " " + row.symbol : ""

    function open(k, z, r) {
        page.kind = k
        page.zone = z
        page.row = r
        amountInput.text = ""
        toInput.text = ""
        sheet.open()
        amountInput.forceActiveFocus()
    }

    function closeSheet() { sheet.close() }

    function confirm() {
        if (page.blocker !== "" || !page.row) return
        var p = { from: page.fromId, to: page.toId, amount: page.raw }
        if (page.row.token !== "") p.token = page.row.token
        var what = page.amountText
        var proving = Units.isPrivate(page.fromId) || Units.isPrivate(page.toId)
        var record = { title: page.kind === "send" ? qsTr("Sending %1 to %2").arg(what).arg(Units.shortId(page.toId)) : qsTr("Moving %1 to %2").arg(what).arg(page.app.accountName(page.toId)),
                       started: Date.now(), done: false,
                       seen: Units.isPrivate(page.fromId) ? qsTr("%1 arrived somewhere. Not where it came from.").arg(what)
                                                          : qsTr("%1 left your Everyday account. Not where it went.").arg(what) }
        var arrow = page.kind === "dive" ? " ↓" : page.kind === "surface" ? " ↑" : ""
        page.inflight = {
            zone: page.kind === "dive" ? "below" : page.kind === "surface" ? "above" : page.zone,
            text: page.kind === "send" ? (proving ? qsTr("Sending %1 · %2").arg(what).arg(page.app.diveTime) : qsTr("Sending %1…").arg(what))
                                       : qsTr("%1 on its way%2 · %3").arg(what).arg(arrow).arg(page.app.diveTime),
            proving: proving, record: record
        }
        sheet.close()
        var mine = function() { return page.app.dive && page.app.dive.started === record.started }
        page.app.call("transfer", p, function(r) {
            page.inflight = null
            if (mine()) { var d = Object.assign({}, page.app.dive); d.done = true; d.tx = r.tx; page.app.dive = d }
            else page.app.notify(qsTr("Moved %1").arg(what), qsTr("tx %1").arg(Units.shortId(r.tx)))
            page.app.refreshAll()
        }, function(e) {
            page.inflight = null
            if (mine()) { var d = Object.assign({}, page.app.dive); d.error = e; page.app.dive = d }
            else page.app.report(e)
            page.app.refreshAll()
        })
    }

    // Dev: settings.json's prefill fills the open sheet (tests drive the view by clicks).
    Connections {
        target: page.app
        function onPrefillChanged() {
            var p = page.app.prefill
            if (p.transfer_to !== undefined) toInput.text = p.transfer_to
            if (p.transfer_amount !== undefined) amountInput.text = p.transfer_amount
        }
    }

    // ---- pieces -------------------------------------------------------------------------

    component Badge: Rectangle {
        property string symbol: ""
        property int size: 32
        readonly property var look: Tide.look(symbol)
        implicitWidth: size
        implicitHeight: size
        radius: size / 2
        color: look[1]
        LogosText { anchors.centerIn: parent; text: parent.look[0]; font.pixelSize: Math.round(parent.size * 0.45); font.weight: Font.ExtraBold; color: parent.look[2] }
    }

    // A row action: an outline pill, or quiet text.
    component Action: Rectangle {
        id: action
        property alias text: actionLabel.text
        property bool outline: true
        property string why: ""        // a tooltip while disabled
        signal clicked()
        implicitHeight: 32
        implicitWidth: actionLabel.implicitWidth + (outline ? 24 : 20)
        radius: 16
        color: "transparent"
        border.width: outline ? 1.5 : 0
        border.color: "#66FFFFFF"
        opacity: enabled ? 1 : 0.4
        LogosText { id: actionLabel; anchors.centerIn: parent; font.pixelSize: 13; font.weight: action.outline ? Font.Bold : Font.DemiBold; color: action.outline ? "#FFFFFF" : "#D9FFFFFF" }
        // Outside the disabled item's input, so the tooltip still shows.
        MouseArea {
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: action.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
            onClicked: if (action.enabled) action.clicked()
            ToolTip.visible: containsMouse && !action.enabled && action.why !== ""
            ToolTip.text: action.why
            ToolTip.delay: 300
        }
    }

    component TokenRow: Item {
        id: tr
        property var r
        property bool deep: false
        Layout.fillWidth: true
        implicitHeight: 50
        readonly property bool tokenStuck: deep && r.token !== "" && !page.privateTokenMoves
        Rectangle { width: parent.width; height: 1; color: tr.deep ? "#1FFFFFFF" : "#14FFFFFF" }
        Badge { id: badge; symbol: tr.r.symbol; anchors.verticalCenter: parent.verticalCenter }
        Column {
            anchors.left: badge.right
            anchors.leftMargin: 14
            anchors.right: amount.left
            anchors.rightMargin: 14
            anchors.verticalCenter: parent.verticalCenter
            LogosText { text: tr.r.symbol; font.pixelSize: 15; font.weight: Font.Bold }
            LogosText { visible: text !== ""; text: Tide.look(tr.r.symbol)[3]; font.pixelSize: 12; color: tr.deep ? "#B8FFFFFF" : "#A6FFFFFF" }
        }
        LogosText {
            id: amount
            anchors.right: actions.left
            anchors.rightMargin: 14
            anchors.verticalCenter: parent.verticalCenter
            text: Units.format(tr.r.raw, tr.r.decimals, Math.min(tr.r.decimals, 8))
            font.pixelSize: 17; font.weight: Font.DemiBold
        }
        Row {
            id: actions
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            width: 168
            layoutDirection: Qt.RightToLeft
            spacing: 4
            Action {
                outline: false
                text: qsTr("Send")
                enabled: page.app.online && page.inflight === null && !tr.tokenStuck
                why: tr.tokenStuck ? qsTr("For now, tokens leave Private only through Markets") : ""
                onClicked: page.open("send", tr.deep ? "below" : "above", tr.r)
            }
            Action {
                text: tr.deep ? qsTr("Surface ↑") : qsTr("Dive ↓")
                enabled: page.app.online && page.inflight === null && !tr.tokenStuck && (tr.deep ? page.aboveId !== "" : page.belowId !== "")
                why: tr.tokenStuck ? qsTr("For now, tokens leave Private only through Markets")
                   : tr.deep ? (page.aboveId === "" ? qsTr("Make an Everyday account first") : "")
                   : (page.belowId === "" ? qsTr("Make a Private account first") : "")
                onClicked: page.open(tr.deep ? "surface" : "dive", tr.deep ? "below" : "above", tr.r)
            }
        }
    }

    component MenuLine: Rectangle {
        id: line
        property alias text: lineLabel.text
        property bool on: false
        signal picked()
        width: 208
        height: 36
        radius: 8
        color: lineMouse.containsMouse ? "#14FFFFFF" : "transparent"
        LogosText { id: lineLabel; x: 10; anchors.verticalCenter: parent.verticalCenter; font.pixelSize: 13; font.weight: line.on ? Font.Bold : Font.Normal }
        LogosText { visible: line.on; anchors.right: parent.right; anchors.rightMargin: 10; anchors.verticalCenter: parent.verticalCenter; text: "✓"; font.pixelSize: 13 }
        MouseArea { id: lineMouse; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: line.picked() }
    }

    // A zone's header: title, subtitle, notes, the account chip (copies) and the "⋯" menu.
    component ZoneHeader: RowLayout {
        id: zh
        property bool deep: false
        property string account: ""
        property alias title: zhTitle.text
        property alias subtitle: zhSub.text
        readonly property var list: deep ? page.privates : page.everyday
        readonly property color chipColor: deep ? "#4D000A1E" : "#14FFFFFF"
        Layout.fillWidth: true
        Layout.preferredHeight: 52
        spacing: 12
        LogosText { id: zhTitle; font.pixelSize: 22; font.weight: Font.ExtraBold; font.letterSpacing: -0.4 }
        LogosText { id: zhSub; font.pixelSize: 13; color: zh.deep ? "#C7FFFFFF" : "#B8FFFFFF"; Layout.fillWidth: true; elide: Text.ElideRight }
        // Freshness (private only).
        Row {
            visible: zh.deep && page.showFresh
            spacing: 6
            LogosText { text: page.freshText; font.pixelSize: 13; color: "#D9FFFFFF"; anchors.verticalCenter: parent.verticalCenter }
            LogosText { visible: !page.app.syncing && page.app.online; text: "·"; font.pixelSize: 13; color: "#D9FFFFFF"; anchors.verticalCenter: parent.verticalCenter }
            LogosText {
                visible: !page.app.syncing && page.app.online
                text: qsTr("Refresh"); font.pixelSize: 13; font.weight: Font.Bold; font.underline: true
                anchors.verticalCenter: parent.verticalCenter
                MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: page.app.syncWallet() }
            }
        }
        // A move landing here.
        Rectangle {
            readonly property bool here: page.inflight !== null && page.inflight.zone === (zh.deep ? "below" : "above")
            visible: here
            implicitHeight: 30
            implicitWidth: inflightLabel.implicitWidth + 24
            radius: 15
            color: zh.deep ? "#4D000A1E" : "#1AFFFFFF"
            LogosText { id: inflightLabel; anchors.centerIn: parent; text: parent.here ? page.inflight.text : ""; font.pixelSize: 13 }
            MouseArea {
                anchors.fill: parent
                cursorShape: parent.here && page.inflight.proving ? Qt.PointingHandCursor : Qt.ArrowCursor
                onClicked: if (parent.here && page.inflight.proving) page.app.dive = page.inflight.record
            }
        }
        Rectangle {
            id: chip
            visible: zh.account !== ""
            property bool copied: false
            implicitHeight: 30
            implicitWidth: chipRow.implicitWidth + 20
            radius: 15
            color: zh.chipColor
            Row {
                id: chipRow
                anchors.centerIn: parent
                spacing: 6
                LogosText { text: chip.copied ? qsTr("Copied") : page.shortName(zh.account); font.pixelSize: 13; font.weight: Font.DemiBold; anchors.verticalCenter: parent.verticalCenter }
                LogosText { text: "⧉"; font.pixelSize: 13; anchors.verticalCenter: parent.verticalCenter }
            }
            Timer { id: copiedTimer; interval: 1500; onTriggered: chip.copied = false }
            MouseArea {
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: { page.app.copy(zh.account, true); chip.copied = true; copiedTimer.restart() }
                ToolTip.visible: containsMouse
                ToolTip.text: zh.account
                ToolTip.delay: 300
            }
        }
        Item {
            implicitWidth: 30
            implicitHeight: 30
            LogosText { anchors.centerIn: parent; text: "⋯"; font.pixelSize: 18; color: "#D9FFFFFF" }
            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: menu.open() }
            Popup {
                id: menu
                y: 36
                x: parent.width - width
                width: 220
                padding: 6
                background: Rectangle { color: "#13233D"; border.color: "#24FFFFFF"; radius: 12 }
                contentItem: Column {
                    spacing: 0
                    Repeater {
                        model: zh.list.length > 1 ? zh.list : []
                        delegate: MenuLine {
                            text: page.app.accountName(modelData.account)
                            on: modelData.account === zh.account
                            onPicked: { menu.close(); if (zh.deep) page.belowPick = modelData.account; else page.abovePick = modelData.account }
                        }
                    }
                    MenuLine {
                        text: zh.deep ? qsTr("New Private account") : qsTr("New Everyday account")
                        enabled: !page.creating
                        onPicked: { menu.close(); page.newAccount(zh.deep) }
                    }
                }
            }
        }
    }

    // ---- layout -------------------------------------------------------------------------

    ColumnLayout {
        anchors.fill: parent
        anchors.topMargin: page.topInset
        spacing: 0

        // Above the surface.
        Item {
            Layout.fillWidth: true
            Layout.preferredHeight: Math.min(aboveCol.implicitHeight + 28, page.height * 0.48)
            Flickable {
                anchors.fill: parent
                anchors.leftMargin: 32
                anchors.rightMargin: 32
                contentHeight: aboveCol.implicitHeight
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                ColumnLayout {
                    id: aboveCol
                    width: Math.min(parent.width, 880)
                    x: (parent.width - width) / 2
                    spacing: 0
                    ZoneHeader {
                        title: qsTr("Public Accounts")
                        subtitle: qsTr("Everyday · anyone can see")
                        account: page.aboveId
                    }
                    Repeater { model: page.aboveRows; delegate: TokenRow { r: modelData } }
                    LogosText {
                        visible: page.aboveRows.length === 0
                        Layout.topMargin: 6
                        text: page.aboveId === "" ? qsTr("No Everyday account yet. Make one from the ⋯ menu.") : qsTr("Nothing here yet. Copy your address to get paid.")
                        font.pixelSize: 14; color: Tide.text3
                    }
                }
            }
        }

        // Below the surface.
        Rectangle {
            Layout.fillWidth: true
            Layout.fillHeight: true
            color: Tide.waterDeep
            Kit.Waves { width: parent.width; color: Tide.waterDeep }
            Flickable {
                anchors.fill: parent
                anchors.leftMargin: 32
                anchors.rightMargin: 32
                anchors.topMargin: 12
                contentHeight: belowCol.implicitHeight + 16
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                ColumnLayout {
                    id: belowCol
                    width: Math.min(parent.width, 880)
                    x: (parent.width - width) / 2
                    spacing: 0
                    opacity: page.app.online ? 1 : 0.85
                    ZoneHeader {
                        deep: true
                        title: qsTr("Private Accounts")
                        subtitle: qsTr("Private · only this computer can see")
                        account: page.belowId
                    }
                    Repeater { model: page.belowRows; delegate: TokenRow { r: modelData; deep: true } }
                    LogosText {
                        visible: page.belowRows.length === 0
                        Layout.topMargin: 6
                        text: page.belowId === "" ? qsTr("No Private account yet. Make one from the ⋯ menu.")
                            : page.belowUnknown ? qsTr("Refresh to see what's down here.")
                            : qsTr("Nothing down here yet. Dive something below to keep it out of sight.")
                        font.pixelSize: 14; color: "#C7FFFFFF"
                    }
                }
            }
        }
    }

    // The move sheet: centred on a plain dim backdrop.
    Popup {
        id: sheet
        parent: Overlay.overlay
        anchors.centerIn: parent
        width: 420
        modal: true
        focus: true
        padding: 24
        closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside
        Overlay.modal: Rectangle { color: "#9E040914" }
        background: Rectangle { color: "#0E1B30"; border.color: "#24FFFFFF"; radius: 24 }
        contentItem: ColumnLayout {
            spacing: 18
            RowLayout {
                spacing: 12
                Badge { symbol: page.row ? page.row.symbol : ""; size: 36 }
                LogosText {
                    Layout.fillWidth: true
                    text: !page.row ? "" : page.kind === "dive" ? qsTr("Dive %1 below").arg(page.row.symbol)
                        : page.kind === "surface" ? qsTr("Bring %1 up").arg(page.row.symbol) : qsTr("Send %1").arg(page.row.symbol)
                    font.pixelSize: 21; font.weight: Font.ExtraBold; font.letterSpacing: -0.4
                }
                Rectangle {
                    implicitWidth: 36; implicitHeight: 36; radius: 18
                    color: "#14FFFFFF"
                    LogosText { anchors.centerIn: parent; text: "✕"; font.pixelSize: 15 }
                    MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: sheet.close() }
                }
            }

            // The route (dive / surface), or who to send to.
            RowLayout {
                visible: page.kind !== "send"
                spacing: 10
                component RouteChip: Rectangle {
                    property alias text: routeLabel.text
                    property bool deep: false
                    implicitHeight: 30
                    implicitWidth: routeLabel.implicitWidth + 24
                    radius: 15
                    color: deep ? Tide.waterDeep : "#1AFFFFFF"
                    LogosText { id: routeLabel; anchors.centerIn: parent; font.pixelSize: 13; font.weight: Font.Bold }
                }
                RouteChip { text: page.app.accountName(page.fromId); deep: page.zone === "below" }
                LogosText { text: page.kind === "dive" ? "↓" : "↑"; font.weight: Font.ExtraBold }
                RouteChip { text: page.toId !== "" ? page.app.accountName(page.toId) : ""; deep: page.zone === "above" }
            }
            ColumnLayout {
                visible: page.kind === "send"
                spacing: 6
                LogosText { text: qsTr("To · from %1").arg(page.app.accountName(page.fromId)); font.pixelSize: 13; color: "#BFFFFFFF" }
                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: 44
                    radius: 12
                    color: Tide.sky
                    border.color: page.badAddress ? Tide.bad : "#33FFFFFF"
                    TextInput {
                        id: toInput
                        anchors.fill: parent
                        anchors.leftMargin: 14
                        anchors.rightMargin: 14
                        verticalAlignment: TextInput.AlignVCenter
                        font.pixelSize: 14
                        color: "#FFFFFF"
                        selectionColor: Tide.waterCalm
                        clip: true
                        onAccepted: page.confirm()
                        LogosText { visible: toInput.text === ""; anchors.verticalCenter: parent.verticalCenter; text: qsTr("Their Everyday address (Public/…)"); font.pixelSize: 14; color: "#59FFFFFF" }
                    }
                }
            }

            ColumnLayout {
                spacing: 6
                LogosText { text: qsTr("Amount"); font.pixelSize: 13; color: "#BFFFFFFF" }
                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: 64
                    radius: 16
                    color: Tide.sky
                    border.width: 1.5
                    border.color: page.over || page.tooMany ? Tide.bad : "#33FFFFFF"
                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: 16
                        anchors.rightMargin: 8
                        spacing: 10
                        TextInput {
                            id: amountInput
                            objectName: "moveAmount"
                            Layout.fillWidth: true
                            font.family: Theme.typography.publicSans
                            font.pixelSize: 30
                            font.weight: Font.ExtraBold
                            color: "#FFFFFF"
                            selectionColor: Tide.waterCalm
                            clip: true
                            validator: RegularExpressionValidator { regularExpression: /[0-9.,]*/ }
                            onAccepted: page.confirm()
                            LogosText { visible: amountInput.text === ""; text: "0"; font.pixelSize: 30; font.weight: Font.ExtraBold; color: "#59FFFFFF" }
                        }
                        LogosText { text: page.row ? page.row.symbol : ""; font.weight: Font.Bold; color: "#BFFFFFFF" }
                        Rectangle {
                            implicitWidth: maxLabel.implicitWidth + 28; implicitHeight: 36; radius: 18
                            color: "#1FFFFFFF"
                            LogosText { id: maxLabel; anchors.centerIn: parent; text: qsTr("Max"); font.pixelSize: 13; font.weight: Font.ExtraBold }
                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: if (page.have !== null && page.row) amountInput.text = Units.exact(String(page.have), page.row.decimals)
                            }
                        }
                    }
                }
                LogosText {
                    readonly property string has: page.row && page.have !== null ? Units.format(String(page.have), page.row.decimals, page.row.decimals) + " " + page.row.symbol : "–"
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    text: page.over ? qsTr("That's more than you have. You have %1.").arg(has)
                        : page.zone === "above" ? qsTr("You have %1 above the surface").arg(has) : qsTr("You have %1 below the surface").arg(has)
                    font.pixelSize: 13
                    color: page.over ? Tide.badText : "#B8FFFFFF"
                }
            }

            ColumnLayout {
                spacing: 4
                LogosText {
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    font.pixelSize: 14
                    text: "◉  " + (page.kind === "dive" ? qsTr("People see it leave Everyday, not where it went.")
                        : page.kind === "surface" ? qsTr("Everyone will see it arrive in Everyday.")
                        : page.zone === "above" ? qsTr("Anyone can see this payment and that it came from you.")
                        : qsTr("They'll see it arrive. No one can tell it came from you."))
                }
                LogosText {
                    Layout.fillWidth: true
                    font.pixelSize: 14
                    color: "#BFFFFFFF"
                    text: "◷  " + (page.proving ? qsTr("Takes %1 on this computer.").arg(page.app.diveTime) : qsTr("Takes a few seconds."))
                }
            }

            Kit.BigButton {
                objectName: "moveButton"
                Layout.fillWidth: true
                enabled: page.blocker === ""
                text: page.blocker !== "" ? page.blocker
                    : (page.kind === "dive" ? qsTr("Dive %1 ↓") : page.kind === "surface" ? qsTr("Surface %1 ↑") : qsTr("Send %1")).arg(page.amountText)
                onClicked: page.confirm()
            }
        }
    }
}
