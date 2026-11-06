import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Logos.Theme
import Logos.Controls
import "../kit" as Kit
import "../kit/units.js" as Units
import "../kit/tide.js" as Tide

// Wallet v3 (direction C, output/wallet-directions/C-water-simplified.png): one Everyday account
// above the surface and one Private account below it, one row per token on each side. A row's
// "Move" opens the move sheet (dive from above, surface from below; the sheet's arrow swaps), its
// "⋯" holds Send and, below, Take back for pending deposits. With a screened pool (E10), a token's
// screened lots fold into its Private row as a badge ("3,000 screened · 500 pending"); the sheet
// picks Private or Screened as the source. LEZ (fees) sits in a footer card, not among the tokens.
// A move under way shows as a pill in the header of the zone it lands in.
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

    readonly property var sortedTokens: app.tokens.slice().sort(function(a, b) { return Tide.tokenRank(a.symbol) - Tide.tokenRank(b.symbol) || (a.symbol < b.symbol ? -1 : 1) })
    readonly property var lez: ({ id: "", symbol: "LEZ", decimals: 0 })
    function balanceOf(id, t) { return t.id === "" ? nativeOf(id) : app.tokenBalance(id, t.id) }
    function plainRow(id, t) {
        var b = id === "" ? null : balanceOf(id, t)
        return b !== null && b !== undefined && !Units.isZero(String(b)) ? { token: t.id, symbol: t.symbol, decimals: t.decimals, raw: String(b) } : null
    }

    // Everyday: one row per token with a balance (LEZ is in the fees footer).
    readonly property var aboveRows: aboveId === "" ? [] : sortedTokens.map(function(t) { return plainRow(aboveId, t) }).filter(function(r) { return r !== null })
    readonly property var aboveLez: plainRow(aboveId, lez)
    function aboveOf(token) {
        if (token === "") return aboveLez
        for (var i = 0; i < aboveRows.length; i++) if (aboveRows[i].token === token) return aboveRows[i]
        return null
    }
    // The Everyday row of a token (Get started's "Dive WETH ↓"), or null.
    function rowFor(symbol) {
        for (var i = 0; i < aboveRows.length; i++) if (aboveRows[i].symbol === symbol) return aboveRows[i]
        return null
    }

    // An account's screened lots: approved ones summed per token, pending ones one by one.
    function screenedRows(id) {
        if (id === "" || !app.pool) return []
        var out = [], approved = {}
        app.pool.lots.forEach(function(l) {
            if (l.account !== id) return
            var token = l.token || ""
            var t = { token: token, symbol: token === "" ? "LEZ" : app.tokenSymbol(token), decimals: token === "" ? 0 : app.tokenDecimals(token) }
            // Market lots (what screened markets paid out) spend like approved ones.
            if (l.state === "approved" || l.state === "market") {
                if (approved[token] === undefined) { approved[token] = out.length; out.push(Object.assign({ raw: "0", screened: "approved" }, t)) }
                out[approved[token]].raw = Units.add(out[approved[token]].raw, l.amount)
            } else out.push(Object.assign({ raw: l.amount, screened: "pending", label: l.label, depositor: l.depositor, canTakeBack: l.can_take_back }, t))
        })
        return out
    }
    readonly property var belowScreened: screenedRows(belowId)
    readonly property bool belowUnknown: belowId !== "" && nativeOf(belowId) === null

    // A Private token: its plain balance, approved screened lots and pending deposits in one row.
    // `raw` is what can move now (plain + approved); `pendingRaw` waits for the pool.
    function mergedOf(t) {
        var plain = plainRow(belowId, t), approved = null, pending = []
        belowScreened.forEach(function(l) {
            if (l.token !== t.id) return
            if (l.screened === "approved") approved = l; else pending.push(l)
        })
        if (!plain && !approved && pending.length === 0) return null
        var pendingRaw = "0"
        pending.forEach(function(l) { pendingRaw = Units.add(pendingRaw, l.raw) })
        return { token: t.id, symbol: t.symbol, decimals: t.decimals, plain: plain, approved: approved, pending: pending, pendingRaw: pendingRaw,
                 raw: Units.add(plain ? plain.raw : "0", approved ? approved.raw : "0") }
    }
    readonly property var belowRows: sortedTokens.map(mergedOf).filter(function(r) { return r !== null })
    readonly property var belowLez: belowId === "" ? null : mergedOf(lez)
    function belowOf(token) {
        if (token === "") return belowLez
        for (var i = 0; i < belowRows.length; i++) if (belowRows[i].token === token) return belowRows[i]
        return null
    }
    readonly property bool anyScreened: belowRows.concat(belowLez ? [belowLez] : []).some(function(m) { return m.approved !== null || m.pending.length > 0 })
    readonly property bool anyPending: belowRows.concat(belowLez ? [belowLez] : []).some(function(m) { return m.pending.length > 0 })

    // "3,000 screened · 500 pending" (a Private row's badge), or "".
    function screenedText(m) {
        if (!m) return ""
        var parts = []
        if (m.approved) parts.push(qsTr("%1 screened").arg(Units.format(m.approved.raw, m.decimals, Math.min(m.decimals, 4))))
        if (m.pending.length > 0) parts.push(qsTr("%1 pending").arg(Units.format(m.pendingRaw, m.decimals, Math.min(m.decimals, 4))))
        return parts.join("  ·  ")
    }

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

    property string kind: "dive"       // dive | surface | send | takeback (a pending lot, back to its depositor)
    property string zone: "above"      // where it leaves from
    property var row: null             // what it moves: a plain balance, an approved lot or a pending lot
    property var merged: null          // the token's Private row (a surface or send can come from either part)
    readonly property string fromId: zone === "above" ? aboveId : belowId
    readonly property string toId: kind === "dive" ? belowId : kind === "surface" ? aboveId : kind === "takeback" ? (row ? row.depositor || "" : "") : toInput.text.trim()
    readonly property bool proving: kind !== "send" ? true : zone === "below"
    // A dive through the screened pool (the default when there is one).
    property bool screened: true
    readonly property bool viaPool: kind === "dive" && app.pool !== null && screened
    readonly property var have: {
        if (!row) return null
        if (row.screened) return row.raw
        return row.token === "" ? nativeOf(fromId) : app.tokenBalance(fromId, row.token)
    }
    readonly property string raw: !row ? "" : kind === "takeback" ? row.raw : Units.parse(amountInput.text, row.decimals)
    readonly property bool tooMany: amountInput.text.trim() !== "" && raw === ""
    readonly property bool over: raw !== "" && have !== null && Units.cmp(raw, String(have)) > 0
    readonly property bool badAddress: kind === "send" && toId !== "" && !/^Public\/[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(toId)
    readonly property string blocker: {
        if (!app.online) return qsTr("No connection")
        if (page.inflight !== null) return qsTr("Wait for the last move")
        if (kind === "takeback") return row && row.canTakeBack ? "" : qsTr("Only the account it came from can take it back")
        if (raw === "" || Units.isZero(raw)) return tooMany ? qsTr("Up to %1 decimals").arg(row ? row.decimals : 0) : qsTr("Enter an amount")
        if (over) return qsTr("More than you have")
        if (kind === "send" && toId === "") return qsTr("Add an address")
        if (badAddress) return qsTr("Check the address")
        return ""
    }
    readonly property string amountText: row && raw !== "" ? Units.format(raw, row.decimals, row.decimals) + " " + row.symbol : ""

    function open(k, z, r, m) {
        page.kind = k
        page.zone = z
        page.row = r
        page.merged = m || null
        screenedBox.checked = true
        amountInput.text = ""
        toInput.text = ""
        sheet.open()
        amountInput.forceActiveFocus()
    }

    function closeSheet() { sheet.close() }

    // A Private row's Move / Send: from the plain balance if there is one, else the screened lots.
    function openBelow(k, m) { page.open(k, "below", m.plain || m.approved, m) }

    // The sheet's arrow: the same token the other way, if that side has some.
    readonly property bool canSwap: {
        if (!row || (kind !== "dive" && kind !== "surface")) return false
        if (kind === "dive") { var m = belowOf(row.token); return m !== null && (m.plain !== null || m.approved !== null) && aboveId !== "" }
        return aboveOf(row.token) !== null && belowId !== ""
    }
    function swap() {
        if (!page.canSwap) return
        if (page.kind === "dive") page.openBelow("surface", page.belowOf(page.row.token))
        else page.open("dive", "above", page.aboveOf(page.row.token))
    }

    function confirm() {
        if (page.blocker !== "" || !page.row) return
        var p = { from: page.fromId, to: page.toId, amount: page.raw }
        if (page.row.token !== "") p.token = page.row.token
        var method = page.kind === "takeback" ? "pool_ragequit" : page.row.screened ? "pool_withdraw" : page.viaPool ? "pool_deposit" : "transfer"
        if (method === "pool_ragequit") p = { holder: page.fromId, label: page.row.label }
        var what = page.amountText
        var proving = Units.isPrivate(page.fromId) || Units.isPrivate(page.toId)
        var record = { title: page.kind === "send" ? qsTr("Sending %1 to %2").arg(what).arg(Units.shortId(page.toId)) : qsTr("Moving %1 to %2").arg(what).arg(page.app.accountName(page.toId)),
                       doneTitle: page.kind === "send" ? qsTr("Sent %1 to %2").arg(what).arg(Units.shortId(page.toId)) : qsTr("Moved %1 to %2").arg(what).arg(page.app.accountName(page.toId)),
                       started: Date.now(), done: false,
                       seen: Units.isPrivate(page.fromId) ? qsTr("%1 arrived somewhere. Not where it came from.").arg(what)
                                                          : qsTr("%1 left your Everyday account. Not where it went.").arg(what) }
        var arrow = page.kind === "dive" ? " ↓" : page.kind === "surface" || page.kind === "takeback" ? " ↑" : ""
        page.inflight = {
            zone: page.kind === "dive" ? "below" : page.kind === "surface" || page.kind === "takeback" ? "above" : page.zone,
            text: page.kind === "send" ? (proving ? qsTr("Sending %1 · %2").arg(what).arg(page.app.diveTime) : qsTr("Sending %1…").arg(what))
                                       : qsTr("%1 on its way%2 · %3").arg(what).arg(arrow).arg(page.app.diveTime),
            proving: proving, record: record
        }
        sheet.close()
        var mine = function() { return page.app.dive && page.app.dive.started === record.started }
        page.app.call(method, p, function(r) {
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

    component MenuLine: Rectangle {
        id: line
        property alias text: lineLabel.text
        property bool on: false
        signal picked()
        width: 228
        height: 36
        radius: 8
        opacity: enabled ? 1 : 0.4
        color: lineMouse.containsMouse && line.enabled ? "#14FFFFFF" : "transparent"
        LogosText { id: lineLabel; x: 10; width: parent.width - 34; elide: Text.ElideRight; anchors.verticalCenter: parent.verticalCenter; font.pixelSize: 13; font.weight: line.on ? Font.Bold : Font.Normal }
        LogosText { visible: line.on; anchors.right: parent.right; anchors.rightMargin: 10; anchors.verticalCenter: parent.verticalCenter; text: "✓"; font.pixelSize: 13 }
        MouseArea { id: lineMouse; anchors.fill: parent; hoverEnabled: true; cursorShape: line.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor; onClicked: if (line.enabled) line.picked() }
    }

    // "⋯" and its menu: `items` is [{ text, enabled, act }].
    component MoreMenu: Item {
        id: more
        property var items: []
        implicitWidth: 30
        implicitHeight: 30
        opacity: items.length > 0 ? 1 : 0.35
        LogosText { anchors.centerIn: parent; text: "⋯"; font.pixelSize: 18; color: "#D9FFFFFF" }
        MouseArea { anchors.fill: parent; cursorShape: more.items.length > 0 ? Qt.PointingHandCursor : Qt.ArrowCursor; onClicked: if (more.items.length > 0) moreMenu.open() }
        Popup {
            id: moreMenu
            y: 34
            x: parent.width - width
            width: 240
            padding: 6
            background: Rectangle { color: "#13233D"; border.color: "#24FFFFFF"; radius: 12 }
            contentItem: Column {
                Repeater {
                    model: more.items
                    delegate: MenuLine { text: modelData.text; enabled: modelData.enabled !== false; onPicked: { moreMenu.close(); modelData.act() } }
                }
            }
        }
    }

    // A move's ⋯ items for a token: Send, and (below) Take back for each pending deposit.
    function sendItems(r, deep) {
        var out = []
        var can = page.app.online && page.inflight === null
        if (!deep) out.push({ text: qsTr("Send %1").arg(r.symbol), enabled: can, act: function() { page.open("send", "above", r) } })
        else {
            if (r.plain || r.approved) out.push({ text: qsTr("Send %1").arg(r.symbol), enabled: can && !(r.token !== "" && !page.privateTokenMoves), act: function() { page.openBelow("send", r) } })
            r.pending.forEach(function(l) {
                out.push({ text: qsTr("Take back %1 %2 (pending)").arg(Units.format(l.raw, l.decimals, Math.min(l.decimals, 4))).arg(l.symbol),
                           enabled: can && !!l.canTakeBack, act: function() { page.open("takeback", "below", l, r) } })
            })
        }
        return out
    }

    // One token on one side: Everyday (a plain row) or Private (a merged row, screened as a badge).
    component TokenRow: Item {
        id: tr
        property var r
        property bool deep: false
        Layout.fillWidth: true
        implicitHeight: 56
        readonly property bool tokenStuck: deep && r.token !== "" && !page.privateTokenMoves
        readonly property bool usable: !Units.isZero(r.raw)
        readonly property string badgeText: deep ? page.screenedText(r) : ""
        Rectangle { width: parent.width; height: 1; color: tr.deep ? "#1FFFFFFF" : "#14FFFFFF" }
        Badge { id: badge; symbol: tr.r.symbol; anchors.verticalCenter: parent.verticalCenter }
        Column {
            anchors.left: badge.right
            anchors.leftMargin: 14
            anchors.right: screenedPill.visible ? screenedPill.left : amount.left
            anchors.rightMargin: 14
            anchors.verticalCenter: parent.verticalCenter
            LogosText { text: tr.r.symbol; font.pixelSize: 15; font.weight: Font.Bold }
            LogosText { visible: text !== ""; text: Tide.look(tr.r.symbol)[3]; font.pixelSize: 12; color: tr.deep ? "#B8FFFFFF" : "#A6FFFFFF" }
        }
        Rectangle {
            id: screenedPill
            visible: tr.badgeText !== ""
            anchors.right: amount.left
            anchors.rightMargin: 16
            anchors.verticalCenter: parent.verticalCenter
            implicitHeight: 30
            implicitWidth: pillLabel.implicitWidth + 24
            radius: 15
            color: "#1AFFFFFF"
            border.color: "#40FFFFFF"
            LogosText { id: pillLabel; anchors.centerIn: parent; text: tr.badgeText; font.pixelSize: 13; font.weight: Font.DemiBold }
            MouseArea {
                anchors.fill: parent
                hoverEnabled: true
                ToolTip.visible: containsMouse
                ToolTip.delay: 300
                ToolTip.text: !tr.deep ? "" : (tr.r.approved ? qsTr("Screened: cleared by the pool, ready for screened markets.") : "")
                              + (tr.r.approved && tr.r.pending.length > 0 ? " " : "")
                              + (tr.r.pending.length > 0 ? qsTr("Pending: waiting for the pool's approval; you can take it back (in public) from ⋯.") : "")
            }
        }
        LogosText {
            id: amount
            anchors.right: actions.left
            anchors.rightMargin: 16
            anchors.verticalCenter: parent.verticalCenter
            // Below: what can move now (plain + screened); only pending: that, dimmed.
            text: Units.format(tr.usable || !tr.deep ? tr.r.raw : tr.r.pendingRaw, tr.r.decimals, Math.min(tr.r.decimals, 8))
            opacity: tr.usable ? 1 : 0.6
            font.pixelSize: 17; font.weight: Font.DemiBold
        }
        Row {
            id: actions
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            spacing: 8
            Action {
                objectName: tr.deep ? "" : "rowActions"
                anchors.verticalCenter: parent.verticalCenter
                text: tr.deep ? qsTr("Move ↑") : qsTr("Move ↓")
                enabled: page.app.online && page.inflight === null && tr.usable && !tr.tokenStuck && (tr.deep ? page.aboveId !== "" : page.belowId !== "")
                why: tr.tokenStuck ? qsTr("For now, tokens leave Private only through Markets")
                   : !tr.usable ? qsTr("Waiting for the pool's approval")
                   : tr.deep ? (page.aboveId === "" ? qsTr("Make an Everyday account first") : "")
                   : (page.belowId === "" ? qsTr("Make a Private account first") : "")
                onClicked: if (tr.deep) page.openBelow("surface", tr.r); else page.open("dive", "above", tr.r)
            }
            MoreMenu { anchors.verticalCenter: parent.verticalCenter; items: page.sendItems(tr.r, tr.deep) }
        }
    }

    // A zone's header: title, subtitle, notes, a move landing here, the account picker and address.
    component ZoneHeader: RowLayout {
        id: zh
        property bool deep: false
        property string account: ""
        property alias title: zhTitle.text
        property alias subtitle: zhSub.text
        readonly property var list: deep ? page.privates : page.everyday
        readonly property color chipColor: deep ? "#4D000A1E" : "#14FFFFFF"
        Layout.fillWidth: true
        Layout.preferredHeight: 60
        spacing: 12
        LogosText { id: zhTitle; font.pixelSize: 24; font.weight: Font.ExtraBold; font.letterSpacing: -0.4 }
        Rectangle { implicitWidth: 1; implicitHeight: 22; color: "#33FFFFFF" }
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
        // The account picker: its name and ▾ (switch, or make another).
        Rectangle {
            implicitHeight: 32
            implicitWidth: pickRow.implicitWidth + 28
            radius: 16
            color: "transparent"
            border.width: 1.5
            border.color: "#66FFFFFF"
            Row {
                id: pickRow
                anchors.centerIn: parent
                spacing: 8
                LogosText { text: zh.account !== "" ? page.shortName(zh.account) : qsTr("No account"); font.pixelSize: 13; font.weight: Font.Bold; anchors.verticalCenter: parent.verticalCenter }
                LogosText { text: "▾"; font.pixelSize: 12; anchors.verticalCenter: parent.verticalCenter }
            }
            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: menu.open() }
            Popup {
                id: menu
                y: 38
                x: parent.width - width
                width: 240
                padding: 6
                background: Rectangle { color: "#13233D"; border.color: "#24FFFFFF"; radius: 12 }
                contentItem: Column {
                    spacing: 0
                    Repeater {
                        model: zh.list
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
        // The address: click to copy.
        Rectangle {
            id: chip
            objectName: zh.deep ? "" : "addressChip"
            visible: zh.account !== ""
            property bool copied: false
            implicitHeight: 32
            implicitWidth: chipRow.implicitWidth + 24
            radius: 16
            color: zh.chipColor
            Row {
                id: chipRow
                anchors.centerIn: parent
                spacing: 8
                LogosText { text: Units.shortId(zh.account); font.pixelSize: 13; font.family: "Menlo"; anchors.verticalCenter: parent.verticalCenter }
                LogosText { text: "⧉"; font.pixelSize: 13; anchors.verticalCenter: parent.verticalCenter }
                LogosText { text: chip.copied ? qsTr("Copied") : qsTr("Copy address"); font.pixelSize: 13; font.weight: Font.DemiBold; anchors.verticalCenter: parent.verticalCenter }
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
    }

    // LEZ only pays network fees: one quiet card, Everyday / Private / Screened side by side.
    component FeesCard: Rectangle {
        id: fees
        readonly property var below: page.belowLez
        readonly property var parts: {
            var out = []
            if (page.aboveLez) out.push({ label: qsTr("Everyday LEZ (public)"), raw: page.aboveLez.raw })
            if (below && below.plain) out.push({ label: qsTr("Private LEZ"), raw: below.plain.raw })
            if (below && below.approved) out.push({ label: qsTr("Screened LEZ"), raw: below.approved.raw })
            if (below && below.pending.length > 0) out.push({ label: qsTr("Pending LEZ"), raw: below.pendingRaw })
            return out
        }
        readonly property var items: {
            var out = []
            var can = page.app.online && page.inflight === null
            if (page.aboveLez && page.belowId !== "") out.push({ text: qsTr("Move LEZ to Private ↓"), enabled: can, act: function() { page.open("dive", "above", page.aboveLez) } })
            if (below && (below.plain || below.approved) && page.aboveId !== "") out.push({ text: qsTr("Move LEZ to Everyday ↑"), enabled: can, act: function() { page.openBelow("surface", below) } })
            if (page.aboveLez) out = out.concat(page.sendItems(page.aboveLez, false).map(function(i) { return Object.assign({}, i, { text: qsTr("Send LEZ from Everyday") }) }))
            if (below) out = out.concat(page.sendItems(below, true).map(function(i) { return i.text.indexOf(qsTr("Send")) === 0 ? Object.assign({}, i, { text: qsTr("Send LEZ from Private") }) : i }))
            return out
        }
        visible: parts.length > 0
        Layout.fillWidth: true
        Layout.topMargin: 24
        implicitHeight: 84
        radius: 18
        color: "#14FFFFFF"
        border.color: "#24FFFFFF"
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 20
            anchors.rightMargin: 14
            spacing: 18
            Badge { symbol: "LEZ"; size: 40 }
            Column {
                Layout.preferredWidth: 190
                LogosText { text: qsTr("LEZ · Network fees"); font.pixelSize: 16; font.weight: Font.ExtraBold }
                LogosText { text: qsTr("Only pays for transactions."); font.pixelSize: 12; color: "#B8FFFFFF" }
            }
            Repeater {
                model: fees.parts
                delegate: RowLayout {
                    spacing: 18
                    Rectangle { implicitWidth: 1; implicitHeight: 40; color: "#26FFFFFF" }
                    Column {
                        LogosText { text: modelData.label; font.pixelSize: 12; color: "#B8FFFFFF" }
                        LogosText { text: Units.format(modelData.raw, 0, 0); font.pixelSize: 19; font.weight: Font.DemiBold }
                    }
                }
            }
            Item { Layout.fillWidth: true }
            MoreMenu { items: fees.items }
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
            Layout.preferredHeight: Math.min(aboveCol.implicitHeight + 28, page.height * (getStarted.visible ? 0.62 : 0.48))
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
                    GetStarted {
                        id: getStarted
                        visible: page.app.onboarding
                        app: page.app
                        wallet: page
                    }
                    ZoneHeader {
                        objectName: "zoneAbove"
                        title: qsTr("Everyday")
                        subtitle: qsTr("Public account. Everyone can see.")
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
                        objectName: "zoneBelow"
                        deep: true
                        title: qsTr("Private")
                        subtitle: qsTr("Private account. Visible only on this computer.")
                        account: page.belowId
                    }
                    Repeater { model: page.belowRows; delegate: TokenRow { r: modelData; deep: true } }
                    LogosText {
                        visible: page.belowRows.length === 0 && !page.belowLez
                        Layout.topMargin: 6
                        text: page.belowId === "" ? qsTr("No Private account yet. Make one from the ⋯ menu.")
                            : page.belowUnknown ? qsTr("Refresh to see what's down here.")
                            : qsTr("Nothing here yet. Move something down to keep it out of sight.")
                        font.pixelSize: 14; color: "#C7FFFFFF"
                    }
                    // What screened and pending mean, and how long a move takes.
                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.topMargin: 18
                        spacing: 6
                        visible: page.belowId !== ""
                        LogosText { visible: page.anyScreened; text: "✓  " + qsTr("Screened money is ready for screened markets."); font.pixelSize: 14; color: "#D9FFFFFF" }
                        LogosText { visible: page.anyPending; text: "ⓘ  " + qsTr("Pending money waits for the pool's approval. Until then you can take it back, in public."); font.pixelSize: 14; color: "#D9FFFFFF"; Layout.fillWidth: true; wrapMode: Text.Wrap }
                        LogosText { text: "◷  " + qsTr("Moving between Everyday and Private takes %1 of proving on this computer.").arg(page.app.diveTime); font.pixelSize: 14; color: "#D9FFFFFF"; Layout.fillWidth: true; wrapMode: Text.Wrap }
                    }
                    FeesCard { objectName: "feesCard" }
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
                    text: !page.row ? "" : page.kind === "dive" ? qsTr("Move %1 to Private").arg(page.row.symbol)
                        : page.kind === "surface" ? qsTr("Move %1 to Everyday").arg(page.row.symbol)
                        : page.kind === "takeback" ? qsTr("Take %1 back").arg(page.row.symbol) : qsTr("Send %1").arg(page.row.symbol)
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
                RouteChip { text: page.app.accountName(page.fromId) + (page.row && page.row.screened ? " · " + qsTr("Screened") : ""); deep: page.zone === "below" }
                // The arrow swaps direction (same token, the other way) when that side has some.
                Rectangle {
                    objectName: "swapArrow"
                    implicitWidth: 30; implicitHeight: 30; radius: 15
                    color: swapMouse.containsMouse && page.canSwap ? "#26FFFFFF" : "transparent"
                    border.width: page.canSwap ? 1 : 0
                    border.color: "#40FFFFFF"
                    LogosText { anchors.centerIn: parent; text: page.kind === "dive" ? "↓" : "↑"; font.weight: Font.ExtraBold }
                    MouseArea {
                        id: swapMouse
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: page.canSwap ? Qt.PointingHandCursor : Qt.ArrowCursor
                        onClicked: page.swap()
                        ToolTip.visible: containsMouse && page.canSwap
                        ToolTip.text: qsTr("The other way")
                        ToolTip.delay: 300
                    }
                }
                RouteChip { text: page.toId !== "" ? page.app.accountName(page.toId) + (page.viaPool ? " · " + qsTr("Screened") : "") : ""; deep: page.zone === "above" }
            }
            // From Private or its screened part, when the token has both.
            RowLayout {
                readonly property bool both: page.merged !== null && page.merged.plain !== null && page.merged.approved !== null
                visible: both && page.zone === "below" && (page.kind === "surface" || page.kind === "send")
                spacing: 8
                LogosText { text: qsTr("From"); font.pixelSize: 13; color: "#BFFFFFFF" }
                Repeater {
                    // (by index: the model copies the rows, so `row === modelData.r` never holds)
                    model: parent.both ? [{ r: page.merged.plain, label: qsTr("Private") }, { r: page.merged.approved, label: qsTr("Screened") }] : []
                    delegate: Rectangle {
                        readonly property bool on: page.row !== null && !!page.row.screened === (index === 1)
                        implicitHeight: 30
                        implicitWidth: srcLabel.implicitWidth + 24
                        radius: 15
                        color: on ? "#FFFFFF" : "transparent"
                        border.width: on ? 0 : 1
                        border.color: "#40FFFFFF"
                        LogosText {
                            id: srcLabel
                            anchors.centerIn: parent
                            text: modelData.label + " · " + Units.format(modelData.r.raw, modelData.r.decimals, Math.min(modelData.r.decimals, 4))
                            font.pixelSize: 13; font.weight: Font.Bold
                            color: parent.on ? "#0B1424" : "#FFFFFF"
                        }
                        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: page.row = index === 1 ? page.merged.approved : page.merged.plain }
                    }
                }
            }
            LogosCheckbox {
                id: screenedBox
                objectName: "screenedBox"
                visible: page.kind === "dive" && page.app.pool !== null
                checked: true
                onCheckedChanged: page.screened = checked
                text: qsTr("Screened")
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

            LogosText {
                visible: page.kind === "takeback"
                text: page.amountText
                font.pixelSize: 30; font.weight: Font.ExtraBold
            }
            ColumnLayout {
                visible: page.kind !== "takeback"
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
                        : page.row && page.row.screened ? qsTr("You have %1 screened and approved").arg(has)
                        : page.zone === "above" ? qsTr("You have %1 in Everyday").arg(has) : qsTr("You have %1 in Private").arg(has)
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
                    text: "◉  " + (page.kind === "takeback" ? qsTr("It goes back, in public, to %1, where it came from. Anyone can link the two.").arg(page.app.accountName(page.toId))
                        : page.viaPool ? qsTr("People see it go into the screened pool, not where it went.")
                        : page.kind === "dive" ? qsTr("People see it leave Everyday, not where it went.")
                        : page.row && page.row.screened && page.kind === "surface" ? qsTr("Everyone will see it arrive in Everyday from the pool, not which deposit it was.")
                        : page.row && page.row.screened ? qsTr("They'll see it come from the pool. No one can tell which deposit it was.")
                        : page.kind === "surface" ? qsTr("Everyone will see it arrive in Everyday.")
                        : page.zone === "above" ? qsTr("Anyone can see this payment and that it came from you.")
                        : qsTr("They'll see it arrive. No one can tell it came from you."))
                }
                LogosText {
                    visible: page.viaPool
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    font.pixelSize: 14
                    color: "#BFFFFFFF"
                    text: "✓  " + qsTr("The pool checks where it came from, so you can show it didn't come from a listed address. It can move up or into screened markets once approved; until then you can take it back.")
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
                    : (page.kind === "dive" ? qsTr("Move %1 ↓") : page.kind === "surface" ? qsTr("Move %1 ↑")
                       : page.kind === "takeback" ? qsTr("Take %1 back") : qsTr("Send %1")).arg(page.amountText)
                onClicked: page.confirm()
            }
        }
    }
}
