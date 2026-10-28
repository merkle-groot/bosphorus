import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Logos.Theme
import Logos.Controls
import "../kit" as Kit
import "../kit/units.js" as Units
import "../kit/tide.js" as Tide

// TPosition: one position in one market, and one op on it. The water is the position's debt
// against its borrowing capacity (debt ÷ (collateral × price × LLTV), the sell line at 80% or just below the op row), as
// it stands; a dashed line previews where the op would take it. When a submitted op lands
// (docs/design/wallet-v2/HANDOFF.md, "Position"): the water settles, coins fall in or rise out,
// the figure that changed glows and a note drops in. Private accounts act below the surface:
// their ops are dives.
Item {
    id: page
    objectName: "positionPage"
    property var app
    property int topInset: 70
    property bool active: false

    // Where.
    readonly property string marketId: app.openMarket !== "" ? app.openMarket : (app.markets.length > 0 ? app.markets[0].market : "")
    readonly property var m: app.marketById(marketId)
    readonly property var detail: app.marketDetails[marketId] || null
    readonly property var feed: detail && detail.feed && !detail.feed.error ? detail.feed : null
    readonly property string account: app.actAs
    readonly property bool isPrivate: Units.isPrivate(account)
    readonly property string loanSym: m ? app.tokenSymbol(m.loan_asset) : ""
    readonly property string collSym: m ? app.tokenSymbol(m.collateral_asset) : ""
    readonly property int loanDec: m ? (m.loan_decimals || 0) : 0
    readonly property int collDec: m ? (m.collateral_decimals || 0) : 0
    readonly property real lltv: m ? parseFloat(m.lltv) : 0
    // Loan base units per collateral base unit.
    readonly property real px: feed ? parseFloat(feed.price) / 1e36 : 0

    // What.
    property int opIndex: 4
    readonly property var op: Tide.ops[opIndex]
    readonly property bool lend: op.op === "supply" || op.op === "withdraw"
    readonly property int decimals: op.token === "loan" ? loanDec : collDec
    readonly property string symbol: op.token === "loan" ? loanSym : collSym
    property bool useAll: false
    property bool held: true
    readonly property bool useHeld: isPrivate && lend && held
    property string from: ""          // "" = the account itself
    property string to: ""            // "" = the account itself
    property string payer: ""         // "" = the signer
    property bool more: false

    // The position: held, a private slot, or the public one.
    readonly property var positions: m ? app.accountPositions(marketId, account) : []
    readonly property var pos: {
        for (var i = 0; i < positions.length; i++) {
            var p = positions[i]
            if (useHeld ? p.held : (!p.held && (!isPrivate || p.index === app.slot))) return p
        }
        return null
    }
    readonly property string saved: pos && pos.supplied_assets ? pos.supplied_assets : "0"
    readonly property string coll: pos ? pos.collateral : "0"
    readonly property string debt: pos && pos.debt ? pos.debt : "0"
    // Where an op's tokens come from, and what that account holds of them (null: unknown).
    readonly property string payFrom: from !== "" ? from : account
    readonly property string payToken: m ? (op.token === "loan" ? m.loan_asset : m.collateral_asset) : ""
    readonly property var payBalance: app.tokenBalance(payFrom, payToken)
    readonly property bool pays: op.op === "supply" || op.op === "supply_collateral" || op.op === "repay"

    // How much (base units) and what the op leaves.
    readonly property string raw: Units.parse(amountInput.text, decimals)
    readonly property string amount: useAll ? (op.op === "withdraw" ? saved : op.op === "repay" ? debt : coll) : (raw === "" ? "0" : raw)
    readonly property var after: {
        var s = saved, c = coll, d = debt, a = amount
        switch (op.op) {
        case "supply": s = Units.add(s, a); break
        case "withdraw": s = Units.sub(s, a); break
        case "supply_collateral": c = Units.add(c, a); break
        case "withdraw_collateral": c = Units.sub(c, a); break
        case "borrow": d = Units.add(d, a); break
        case "repay": d = Units.sub(d, a); break
        }
        return { saved: s, coll: c, debt: d }
    }
    function capacity(c) { return parseFloat(c) * px * lltv }
    function hfOf(c, d) { return parseFloat(d) > 0 ? (capacity(c) > 0 ? capacity(c) / parseFloat(d) : 0) : Infinity }
    readonly property real hfNow: hfOf(coll, debt)
    readonly property real hfAfter: quote && quote.health_factor_after !== null && quote.health_factor_after !== undefined ? parseFloat(quote.health_factor_after) : hfOf(after.coll, after.debt)

    // Checks the app can't make (they aren't refusals) and the app's own refusal.
    readonly property var localProblem: {
        if (!app.online) return { title: qsTr("No connection"), body: qsTr("Bosphorus can’t reach the network right now. Your money stays where it is; this works again when the status shows Calm."), fix: "" }
        if (!m) return null
        if (app.info && app.info.lending_problem || detail && detail.code_problem) return { title: qsTr("Turned off for your safety"), body: qsTr("This market’s programs aren’t the ones Bosphorus expects, so it won’t send anything here."), fix: "" }
        if (amountInput.text.trim() === "" && !useAll) return null
        if (!useAll && (raw === "" || Units.isZero(raw))) return { title: qsTr("Enter an amount"), body: qsTr("Up to %1 decimals.").arg(decimals), fix: "" }
        if (op.op === "repay" && Units.isZero(debt)) return { title: qsTr("Nothing to pay back"), body: qsTr("This slot doesn’t owe anything."), fix: "" }
        if (op.op === "withdraw_collateral" && Units.cmp(amount, coll) > 0) return { title: qsTr("Not that much set aside"), body: qsTr("This slot has %1 %2 of collateral.").arg(Units.format(coll, collDec)).arg(collSym), fix: Units.isZero(coll) ? "" : "all" }
        if (op.op === "withdraw" && Units.cmp(amount, saved) > 0) return Tide.refusal({ code: "insufficient_shares" }, { symbol: loanSym, saved: Units.format(saved, loanDec) })
        if (pays && payBalance !== null && Units.cmp(amount, payBalance) > 0) return Tide.refusal({ code: "insufficient_balance" }, { symbol: symbol, has: Units.format(payBalance, decimals) })
        if ((op.op === "borrow" || op.op === "withdraw_collateral") && px > 0 && hfOf(after.coll, after.debt) < 1)
            return Tide.refusal({ code: "unhealthy" }, { symbol: loanSym, maxBorrow: op.op === "borrow" && safeMax() !== "0" ? Units.format(safeMax(), loanDec) : "" })
        return null
    }
    readonly property var problem: localProblem !== null ? localProblem : (quoteError ? Tide.refusal(quoteError, { symbol: symbol, saved: Units.format(saved, loanDec), has: payBalance !== null ? Units.format(payBalance, decimals) : "", maxBorrow: op.op === "borrow" && safeMax() !== "0" ? Units.format(safeMax(), loanDec) : "" }) : null)
    readonly property bool spills: problem !== null && (localProblem !== null ? (op.op === "borrow" || op.op === "withdraw_collateral") && hfOf(after.coll, after.debt) < 1 : quoteError && quoteError.code === "unhealthy")

    // 99% of what's left under the line, in loan base units ("0" if nothing).
    function safeMax() {
        var room = capacity(coll) - parseFloat(debt)
        if (!(room > 0)) return "0"
        var v = Math.floor(room * 0.99)
        return v > 0 ? String(v).replace(/e\+?\d+$/, "") : "0"
    }

    // The water.
    readonly property real ratioShown: {
        var c = problem ? coll : after.coll, d = problem ? debt : after.debt
        var cap = capacity(c)
        return parseFloat(d) > 0 ? (cap > 0 ? parseFloat(d) / cap : 1.1) : 0
    }
    // The sell line: at 80% of the window, or lower if that would crowd the op row.
    readonly property real sellAt: height > 0 ? Math.min(0.8, 1 - (topInset + opRow.y + opRow.height + 48) / height) : 0.8
    readonly property real level: spills ? sellAt + 0.03 : Tide.level(ratioShown, sellAt)
    readonly property real hfShown: spills ? 0.5 : (problem ? hfNow : hfAfter)
    readonly property bool showLine: parseFloat(debt) > 0 || parseFloat(after.debt) > 0 || spills
    // The water as the position stands, and whether to preview where the op takes it.
    readonly property real levelNow: {
        var cap = capacity(coll), d = parseFloat(debt)
        return Tide.level(d > 0 ? (cap > 0 ? d / cap : 1.1) : 0, sellAt)
    }
    readonly property bool preview: !working && (spills || (request !== null && problem === null && Math.abs(level - levelNow) > 0.004))

    // ---- after an op lands --------------------------------------------------------------

    // The op on its way: what it was and the position it started from (see play()).
    property var tracking: null
    readonly property var trackedJob: {
        if (!tracking) return null
        for (var i = 0; i < app.jobs.length; i++) if (app.jobs[i].id === tracking.id) return app.jobs[i]
        return null
    }
    readonly property bool working: tracking !== null && (trackedJob === null || !app.isTerminal(trackedJob.state))
    readonly property string posKey: marketId + "|" + account + "|" + (useHeld ? "held" : isPrivate ? app.slot : "public")
    property var landed: null          // a tracked op that landed, waiting to be shown
    onTrackedJobChanged: {
        if (!tracking || !trackedJob || !app.isTerminal(trackedJob.state)) return
        var t = tracking
        tracking = null
        if (trackedJob.state === "done") { landed = t; landedWait.restart() }
        // A private op's failure is on the dive screen; a public one's, here.
        else if (!(app.dive && app.dive.job === t.id))
            app.report(trackedJob.error || { code: trackedJob.state, message: String(trackedJob.note || qsTr("It didn't go through. See Activity.")) })
    }
    // Plays once the position has refreshed (basis changes), or after a short wait; never under
    // the dive screen or on another tab.
    Timer { id: landedWait; interval: 2500; onTriggered: page.play() }
    Timer { id: refreshedWait; interval: 250; onTriggered: page.play() }
    onActiveChanged: if (landed) landedWait.restart()
    Connections { target: page.app; function onDiveChanged() { if (page.landed && page.app.dive === null) landedWait.restart() } }

    property string glow: ""           // "saved" | "coll" | "debt": the summary figure to brighten
    property real glowLevel: 0
    SequentialAnimation {
        id: glowAnim
        NumberAnimation { target: page; property: "glowLevel"; from: 0; to: 1; duration: 420; easing.type: Easing.OutQuad }
        NumberAnimation { target: page; property: "glowLevel"; to: 0; duration: 980; easing.type: Easing.InOutQuad }
        ScriptAction { script: page.glow = "" }
    }
    // While a tracked op runs: its progress, from the job's state (proving ~1 min on a GPU).
    property real tick: Date.now()
    Timer { interval: 250; repeat: true; running: page.active && page.working; onTriggered: page.tick = Date.now() }
    readonly property real workPct: {
        var j = trackedJob
        if (!j) return 2
        switch (j.state) {
        case "queued": return 3
        case "preparing": return 8
        case "proving": return 10 + 82 * Math.min(1, (tick - j.since_ms) / (app.gpu ? 75000 : 600000))
        case "sending": return 93
        case "submitted": case "checking": return 96
        }
        return 100
    }
    function play() {
        var t = page.landed
        if (!t || !page.active || page.app.dive !== null) return
        page.landed = null
        var key = t.op === "supply" || t.op === "withdraw" ? "saved" : t.op === "borrow" || t.op === "repay" ? "debt" : "coll"
        var here = t.key === page.posKey
        var now = here ? { saved: page.saved, coll: page.coll, debt: page.debt } : null
        var delta = now ? Units.sub(Units.cmp(now[key], t.before[key]) >= 0 ? now[key] : t.before[key], Units.cmp(now[key], t.before[key]) >= 0 ? t.before[key] : now[key]) : "0"
        var dec = key === "coll" ? t.collDec : t.loanDec
        var amt = (!Units.isZero(delta) ? Units.format(delta, dec, 2) : t.amountText) + " " + t.symbol
        var out = ["borrow", "withdraw", "withdraw_collateral"].indexOf(t.op) >= 0
        var safety = Tide.word(now ? page.hfOf(now.coll, now.debt) : Infinity)
        var where = t.to !== "" ? page.app.accountName(t.to) : page.app.accountName(t.account)
        var title = "", sub = ""
        switch (t.op) {
        case "supply": title = qsTr("Saved %1").arg(amt); sub = qsTr("Now earning %1% a year").arg(t.apr) + (t.isPrivate ? qsTr(" · below the surface") : ""); break
        case "withdraw": title = qsTr("Took back %1").arg(amt); sub = qsTr("Back in %1").arg(where); break
        case "supply_collateral": title = qsTr("Added %1 as collateral").arg(amt); sub = qsTr("Safety: %1").arg(safety); break
        case "withdraw_collateral": title = qsTr("Removed %1").arg(amt); sub = qsTr("Back in %1 · %2").arg(where).arg(safety); break
        case "borrow": title = qsTr("Borrowed %1").arg(amt); sub = qsTr("In %1 · %2").arg(where).arg(safety); break
        case "repay":
            title = qsTr("Paid back %1").arg(amt)
            sub = now && Units.isZero(now.debt) ? qsTr("All paid off. Calm water.") : now ? qsTr("You owe %1 %2 · %3").arg(Units.format(now.debt, t.loanDec, 2)).arg(t.loanSym).arg(safety) : ""
            break
        }
        if (here) {
            splash.run(out, t.symbol, (out ? "+" : "−") + amt)
            page.glow = key
            glowAnim.restart()
        }
        note.show(t.symbol, title, sub)
    }

    // The request, its quote.
    readonly property var request: {
        if (!m || account === "" || localProblem !== null || (!useAll && (raw === "" || Units.isZero(raw)))) return null
        var r = { op: op.op, market: marketId, amount: useAll ? "all" : raw }
        if (["supply", "supply_collateral", "repay"].indexOf(op.op) >= 0) r.from = from !== "" ? from : account
        r.owner = account
        if (useHeld) r.held = true
        else if (isPrivate) r.index = app.slot
        if (!useAll && (op.op === "withdraw" || op.op === "repay")) r.unit = "assets"
        if (["withdraw", "withdraw_collateral", "borrow"].indexOf(op.op) >= 0 && to !== "") r.to = to
        if (!isPrivate && payer !== "") r.payer = payer
        return r
    }
    readonly property string requestKey: request ? JSON.stringify(request) : ""
    property var quote: null
    property string quotedKey: ""
    property var quoteError: null
    property bool quoting: false
    property bool sent: false
    property bool submitting: false
    onRequestKeyChanged: { quote = null; quoteError = null; sent = false; if (requestKey !== "") quoteTimer.restart() }
    // Ask again when the position moves under the quote (a job landed, a refresh) or its offer
    // runs out.
    readonly property string basis: saved + "|" + coll + "|" + debt
    onBasisChanged: { requote(); if (landed) refreshedWait.restart() }
    function requote() { if (requestKey !== "" && !quoting) { quote = null; quoteError = null; quoteTimer.restart() } }
    Timer { id: expiryTimer; onTriggered: page.requote() }

    Timer {
        id: quoteTimer
        interval: 450
        onTriggered: {
            var key = page.requestKey
            if (key === "") return
            page.quoting = true
            page.app.call("quote", page.request, function(q) {
                if (key !== page.requestKey) return
                page.quoting = false; page.quote = q; page.quotedKey = key
                if (q.valid_until) { expiryTimer.interval = Math.max(2000, q.valid_until - Date.now() - 15000); expiryTimer.restart() }
            }, function(e) {
                if (key !== page.requestKey) return
                page.quoting = false; page.quoteError = e
            })
        }
    }
    // The quote's window, counted down.
    property real now: Date.now()
    Timer { interval: 1000; repeat: true; running: page.active && page.quote !== null; onTriggered: page.now = Date.now() }

    function submit() {
        var key = page.requestKey, req = page.request
        // One click, one job: a second click before the first answers would do it twice.
        if (key === "" || key !== page.quotedKey || page.submitting) return
        var title = Tide.opText(page.op.op, (page.useAll ? qsTr("all") : Units.format(page.amount, page.decimals)) + " " + page.symbol, true)
        var seen = page.seenFromAbove(), who = page.app.accountName(page.account)
        var track = { op: page.op.op, symbol: page.symbol, loanSym: page.loanSym, loanDec: page.loanDec, collDec: page.collDec,
                      amountText: page.useAll ? qsTr("all") : Units.format(page.amount, page.decimals, 2),
                      key: page.posKey, before: { saved: page.saved, coll: page.coll, debt: page.debt },
                      account: page.account, to: req.to || "", isPrivate: page.isPrivate,
                      apr: page.m ? (parseFloat(page.m.supply_apr) * 100).toFixed(2) : "–" }
        page.submitting = true
        page.app.call("submit", req, function(job) {
            page.submitting = false
            page.quote = null; page.quotedKey = ""; page.sent = true
            amountInput.text = ""; page.useAll = false
            track.id = job.id
            page.tracking = track
            page.app.refreshJobs()
            if (Units.isPrivate(req.owner)) page.app.dive = { job: job.id, title: title + qsTr(" · %1").arg(who), seen: seen }
        }, function(e) { page.submitting = false; page.app.report(e) })
    }

    // What stays visible after a private op ("Seen from above").
    function seenFromAbove() {
        var a = Units.format(page.amount, page.decimals) + " " + page.symbol
        switch (page.op.op) {
        case "supply": return page.useHeld ? qsTr("%1 came into the market. New market totals. No position.").arg(a) : qsTr("Some position saved %1. New market totals.").arg(a)
        case "withdraw": return page.useHeld ? qsTr("%1 left the market. New market totals. No position.").arg(a) : qsTr("Some position took back %1.").arg(a)
        case "supply_collateral": return qsTr("Some position added %1 of collateral.").arg(a)
        case "withdraw_collateral": return qsTr("Some position removed %1 of collateral.").arg(a)
        case "borrow": return qsTr("Some account now owes %1 more. %1 left the market.").arg(a)
        case "repay": return qsTr("Some position paid back %1.").arg(a)
        }
        return ""
    }

    function nudge(up) {
        var b = op.token === "loan" ? (op.op === "supply" ? app.tokenBalance(account, m ? m.loan_asset : "") : debt) : coll
        var unit = "1" + "0".repeat(decimals)
        var stepRaw = b && !Units.isZero(b) && Units.stripZeros(String(b)).length > 2 ? "1" + "0".repeat(Units.stripZeros(String(b)).length - 2) : unit
        var cur = raw === "" ? "0" : raw
        useAll = false
        amountInput.text = Units.exact(up ? Units.add(cur, stepRaw) : Units.sub(cur, stepRaw), decimals)
    }

    function applyFix(fix) {
        if (fix === "all") { page.useAll = true }
        else if (fix === "max") { amountInput.text = Units.exact(page.payBalance, page.decimals); page.useAll = false }
        else if (fix === "maxBorrow") { amountInput.text = Units.exact(page.safeMax(), page.loanDec); page.useAll = false }
        else if (fix === "payer") { page.cycle("payer") }
    }

    // More options: each pill cycles through its choices.
    function choices(kind) {
        var out = [""]
        page.app.accounts.forEach(function(a) {
            if (a.account === page.account) return
            if (kind === "payer" && Units.isPrivate(a.account)) return
            if (kind === "from" && Units.isPrivate(a.account) !== page.isPrivate) return
            out.push(a.account)
        })
        return out
    }
    function cycle(kind) {
        var list = choices(kind), cur = page[kind]
        page[kind] = list[(list.indexOf(cur) + 1) % list.length]
    }
    function routeName(kind) {
        var v = page[kind]
        if (v === "") return kind === "payer" ? page.app.accountName(page.account) : qsTr("this account")
        return page.app.accountName(v)
    }

    Connections {
        target: page.app
        function onPresetOpChanged() {
            if (page.app.presetOp === "") return
            for (var i = 0; i < Tide.ops.length; i++) if (Tide.ops[i].op === page.app.presetOp) page.opIndex = i
            page.app.presetOp = ""
        }
        function onPrefillChanged() { if (page.app.prefill.amount) amountInput.text = page.app.prefill.amount }
    }

    // ---- the strait -------------------------------------------------------------------

    Kit.Water {
        id: water
        level: page.levelNow
        color: Tide.water(page.hfNow, page.isPrivate)
        settleMs: 1400
        overshoot: 0.9
    }
    Kit.SellLine { at: page.sellAt; visible: page.showLine; label: qsTr("If the water reaches here, some %1 is sold").arg(page.collSym) }

    // The rise the op would add, shaded from the water up to the preview line: red when it
    // crosses the sell line.
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        y: previewLine.y + 2
        height: Math.max(0, water.y - y)
        color: page.spills ? Qt.rgba(1, 0.33, 0.33, 0.22) : Qt.rgba(1, 1, 1, 0.07)
        opacity: previewLine.opacity
    }

    // Where the op would take the water.
    Item {
        id: previewLine
        anchors.left: parent.left
        anchors.right: parent.right
        height: 2
        // Spilling: just above the sell line, rather than up in the header.
        y: page.spills ? page.height * (1 - page.sellAt) - 22 : page.height * (1 - page.level)
        opacity: page.preview ? 1 : 0
        Behavior on y { NumberAnimation { duration: 320; easing.type: Easing.OutCubic } }
        Behavior on opacity { NumberAnimation { duration: 220 } }
        readonly property color ink: page.spills ? Tide.bad : "#FFFFFF"
        Row {
            spacing: 6
            Repeater {
                model: Math.ceil(previewLine.width / 14)
                delegate: Rectangle { width: 8; height: 2; color: previewLine.ink; opacity: 0.85 }
            }
        }
        LogosText {
            x: 28
            anchors.bottom: parent.top
            anchors.bottomMargin: 6
            text: page.spills ? qsTr("Too much: past the sell line, your %1 could be sold").arg(page.collSym) : qsTr("After: %1").arg(Tide.word(page.hfAfter))
            font.pixelSize: 12; font.weight: Font.Bold
            color: previewLine.ink
        }
    }

    Kit.Splash { id: splash; surfaceY: water.y; ceiling: page.topInset + 10 }

    ColumnLayout {
        anchors.fill: parent
        anchors.topMargin: page.topInset
        spacing: 0

        // Who and which.
        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 28
            Layout.rightMargin: 28
            spacing: 10
            Flow {
                Layout.fillWidth: true
                spacing: 6
                Repeater {
                    model: page.app.accounts
                    delegate: Kit.Pill {
                        text: page.app.accountName(modelData.account)
                        on: page.account === modelData.account
                        onClicked: { page.app.actAs = modelData.account; page.app.slot = 0; page.useAll = false; page.from = ""; page.to = ""; page.payer = "" }
                    }
                }
                Rectangle {
                    visible: page.isPrivate && !page.useHeld
                    height: 36; width: slotRow.implicitWidth + 4; radius: 18
                    color: Tide.glass
                    Row {
                        id: slotRow
                        anchors.centerIn: parent
                        Kit.RoundButton { size: 32; text: "‹"; border.width: 0; color: "transparent"; onClicked: page.app.slot = Math.max(0, page.app.slot - 1) }
                        LogosText { width: 60; horizontalAlignment: Text.AlignHCenter; anchors.verticalCenter: parent.verticalCenter; text: qsTr("Slot #%1").arg(page.app.slot); font.pixelSize: 13; font.weight: Font.Bold }
                        Kit.RoundButton { size: 32; text: "›"; border.width: 0; color: "transparent"; onClicked: page.app.slot = page.app.slot + 1 }
                    }
                }
                Kit.Pill {
                    visible: page.app.markets.length > 1
                    text: page.loanSym + " · " + page.collSym + " ▾"
                    onClicked: {
                        var ms = page.app.markets, i = 0
                        for (; i < ms.length; i++) if (ms[i].market === page.marketId) break
                        page.app.openMarket = ms[(i + 1) % ms.length].market
                    }
                }
            }
            // Saved · Collateral · Owe; the one an op just changed glows.
            Row {
                spacing: 2
                LogosText { visible: !page.m; text: qsTr("No market yet"); font.pixelSize: 14; color: "#E0FFFFFF" }
                Repeater {
                    model: page.m ? [["saved", qsTr("Saved %1 %2").arg(Units.format(page.saved, page.loanDec, 2)).arg(page.loanSym)],
                                     ["coll", qsTr("Collateral %1 %2").arg(Units.format(page.coll, page.collDec, 2)).arg(page.collSym)],
                                     ["debt", qsTr("Owe %1 %2").arg(Units.format(page.debt, page.loanDec, 2)).arg(page.loanSym)]] : []
                    delegate: Row {
                        spacing: 2
                        LogosText { visible: index > 0; text: "·"; font.pixelSize: 14; color: "#99FFFFFF"; anchors.verticalCenter: parent.verticalCenter }
                        Rectangle {
                            implicitWidth: figure.implicitWidth + 12
                            implicitHeight: figure.implicitHeight + 6
                            radius: 8
                            color: Qt.rgba(1, 1, 1, page.glow === modelData[0] ? 0.18 * page.glowLevel : 0)
                            LogosText { id: figure; anchors.centerIn: parent; text: modelData[1]; font.pixelSize: 14; color: "#E0FFFFFF" }
                        }
                    }
                }
            }
        }

        // The op: Tide.ops in pairs (in, out), one labelled capsule per pair.
        Flow {
            id: opRow
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: 16
            spacing: 14
            Repeater {
                model: [qsTr("Savings"), qsTr("Collateral"), qsTr("Loan")]
                delegate: Rectangle {
                    id: group
                    readonly property int first: index * 2
                    color: Tide.panel
                    radius: height / 2
                    border.width: 1.5
                    border.color: "#4DFFFFFF"
                    implicitHeight: 44
                    implicitWidth: groupRow.implicitWidth + 8
                    Row {
                        id: groupRow
                        anchors.verticalCenter: parent.verticalCenter
                        x: 4
                        spacing: 2
                        LogosText {
                            anchors.verticalCenter: parent.verticalCenter
                            leftPadding: 12; rightPadding: 8
                            text: modelData
                            font.pixelSize: 12; font.weight: Font.Bold
                            color: "#99FFFFFF"
                        }
                        Repeater {
                            model: 2
                            delegate: Kit.Pill {
                                readonly property int op: group.first + index
                                pillHeight: 34
                                text: Tide.ops[op].short || Tide.ops[op].label
                                offColor: "transparent"
                                border.width: on ? 1.5 : 0
                                on: op === page.opIndex
                                onClicked: { page.opIndex = op; page.useAll = false; amountInput.text = "" }
                            }
                        }
                    }
                }
            }
        }

        // The amount.
        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true
        ColumnLayout {
            anchors.centerIn: parent
            spacing: 6
            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                spacing: 26
                Kit.RoundButton { text: "−"; onClicked: page.nudge(false) }
                Item {
                    implicitWidth: Math.max(220, amountInput.contentWidth + 20)
                    implicitHeight: 104
                    TextInput {
                        id: amountInput
                        objectName: "positionAmount"
                        visible: !page.useAll
                        anchors.fill: parent
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                        font.family: Theme.typography.publicSans
                        font.pixelSize: 100
                        font.weight: Font.ExtraBold
                        font.letterSpacing: -5
                        color: "#FFFFFF"
                        selectionColor: Tide.waterDeep
                        onTextChanged: if (text !== "") page.useAll = false
                    }
                    LogosText {
                        anchors.centerIn: parent
                        visible: page.useAll || amountInput.text === ""
                        text: page.useAll ? qsTr("All") : "0"
                        font.pixelSize: 100; font.weight: Font.ExtraBold; font.letterSpacing: -5
                        color: page.useAll ? "#FFFFFF" : "#4DFFFFFF"
                    }
                }
                Kit.RoundButton { text: "+"; onClicked: page.nudge(true) }
            }
            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                spacing: 10
                LogosText {
                    text: page.useAll ? Units.format(page.amount, page.decimals) + " " + page.symbol : page.symbol
                    font.pixelSize: 17; font.weight: Font.DemiBold; opacity: 0.85
                }
                Kit.Pill {
                    visible: page.op.all
                    pillHeight: 30; fontSize: 13
                    text: qsTr("All")
                    on: page.useAll
                    onClicked: { page.useAll = !page.useAll; if (page.useAll) amountInput.text = "" }
                }
            }
            LogosText {
                Layout.alignment: Qt.AlignHCenter
                Layout.topMargin: 10
                text: page.spills ? qsTr("Too much") : Tide.word(page.hfShown)
                font.pixelSize: 40; font.weight: Font.ExtraBold; font.letterSpacing: -1.2
            }
        }
        }

        // What happens, and go.
        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 28
            Layout.rightMargin: 28
            Layout.bottomMargin: 28
            Layout.topMargin: 20
            spacing: 20
            ColumnLayout {
                Layout.fillWidth: true
                Layout.maximumWidth: 620
                Layout.alignment: Qt.AlignBottom
                spacing: 10

                // The quote.
                ColumnLayout {
                    visible: page.problem === null && page.request !== null
                    spacing: 4
                    LogosText { visible: page.quoting && page.quote === null; text: qsTr("Asking the market…"); font.pixelSize: 16; font.weight: Font.DemiBold; opacity: 0.8 }
                    Repeater {
                        model: {
                            if (page.quote === null) return []
                            var q = [], a = page.after
                            if (a.saved !== page.saved) q.push(qsTr("Saved %1 → %2 %3").arg(Units.format(page.saved, page.loanDec, 2)).arg(Units.format(a.saved, page.loanDec, 2)).arg(page.loanSym))
                            if (a.coll !== page.coll) q.push(qsTr("Collateral %1 → %2 %3").arg(Units.format(page.coll, page.collDec, 2)).arg(Units.format(a.coll, page.collDec, 2)).arg(page.collSym))
                            if (a.debt !== page.debt) q.push(qsTr("You owe %1 → %2 %3").arg(Units.format(page.debt, page.loanDec, 2)).arg(Units.format(a.debt, page.loanDec, 2)).arg(page.loanSym))
                            if (parseFloat(a.debt) > 0 && parseFloat(a.coll) > 0 && page.lltv > 0) {
                                var liq = (parseFloat(a.debt) / Math.pow(10, page.loanDec)) / ((parseFloat(a.coll) / Math.pow(10, page.collDec)) * page.lltv)
                                q.push(qsTr("The water only reaches the line if %1 falls below %2 %3 (now %4).").arg(page.collSym).arg(liq.toPrecision(4)).arg(page.loanSym).arg(page.feed ? Units.price(page.feed.price, page.loanDec, page.collDec) : "–"))
                            }
                            return q
                        }
                        delegate: LogosText { text: modelData; font.pixelSize: 16; font.weight: Font.DemiBold; wrapMode: Text.Wrap; Layout.fillWidth: true }
                    }
                    LogosText {
                        visible: page.quote !== null && !!page.quote.valid_until
                        text: {
                            if (!page.quote || !page.quote.valid_until) return ""
                            var s = Math.max(0, Math.round((page.quote.valid_until - page.now) / 1000))
                            return qsTr("Offer holds for about %1 (until %2).").arg(s >= 120 ? qsTr("%1 minutes").arg(Math.round(s / 60)) : qsTr("%1 seconds").arg(s)).arg(new Date(page.quote.valid_until).toLocaleTimeString(Qt.locale("en_GB"), "HH:mm"))
                        }
                        font.pixelSize: 13; color: Tide.text2
                    }
                }

                // A refusal, with its one-tap fix.
                Rectangle {
                    visible: page.problem !== null
                    Layout.fillWidth: true
                    implicitHeight: errCol.implicitHeight + 28
                    radius: 18
                    color: Tide.card
                    border.color: "#40FFFFFF"
                    ColumnLayout {
                        id: errCol
                        anchors.fill: parent; anchors.margins: 14; anchors.leftMargin: 16
                        spacing: 6
                        LogosText { text: page.problem ? page.problem.title : ""; font.pixelSize: 16; font.weight: Font.ExtraBold }
                        LogosText { Layout.fillWidth: true; wrapMode: Text.Wrap; text: page.problem ? page.problem.body : ""; textFormat: Text.PlainText; font.pixelSize: 14; color: "#E6FFFFFF" }
                        RowLayout {
                            spacing: 12
                            Kit.BigButton {
                                visible: page.problem !== null && page.problem.fix !== ""
                                buttonHeight: 36; fontSize: 13.5
                                text: !page.problem ? "" : page.problem.fix === "all" ? (page.op.op === "withdraw" ? qsTr("Take back all") : qsTr("Remove all"))
                                    : page.problem.fix === "maxBorrow" ? qsTr("Borrow %1 instead").arg(Units.format(page.safeMax(), page.loanDec, 2))
                                    : page.problem.fix === "max" ? qsTr("Use all of it")
                                    : qsTr("Pay the fee from another account")
                                onClicked: page.applyFix(page.problem.fix)
                            }
                            LogosText { visible: page.quoteError !== null && page.localProblem === null; text: page.quoteError ? String(page.quoteError.code) : ""; font.family: Theme.typography.mono; font.pixelSize: 11; opacity: 0.75 }
                        }
                    }
                }

                LogosText {
                    visible: page.isPrivate
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    textFormat: Text.StyledText
                    font.pixelSize: 13; color: "#E0FFFFFF"
                    text: page.useHeld ? qsTr("<b>Below the surface:</b> that this is yours, the savings receipt, where it lands. <b>Seen from above:</b> the amount and the market totals.")
                                       : qsTr("<b>Below the surface:</b> that this is yours, your secret code, where it lands. <b>Seen from above:</b> the position's numbers and the market totals.")
                }
                Rectangle {
                    visible: page.isPrivate && page.lend
                    implicitHeight: 36
                    implicitWidth: heldRow.implicitWidth + 18
                    radius: 18
                    color: "#33000000"
                    border.color: "#4DFFFFFF"
                    Row {
                        id: heldRow
                        anchors.verticalCenter: parent.verticalCenter
                        x: 4
                        spacing: 10
                        Rectangle { width: 28; height: 28; radius: 14; color: page.held ? Tide.good : "#4DFFFFFF" }
                        LogosText {
                            anchors.verticalCenter: parent.verticalCenter
                            text: page.op.op === "supply" ? qsTr("Keep the savings receipt below the surface") : qsTr("From the savings kept below the surface")
                            font.pixelSize: 13; font.weight: Font.DemiBold
                        }
                    }
                    MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: page.held = !page.held }
                }
                LogosText {
                    text: page.more ? qsTr("Fewer options") : qsTr("More options")
                    font.pixelSize: 13; font.underline: true; color: "#E6FFFFFF"
                    MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: page.more = !page.more }
                }
                Flow {
                    visible: page.more
                    Layout.fillWidth: true
                    spacing: 8
                    Repeater {
                        model: {
                            var r = []
                            if (["supply", "supply_collateral", "repay"].indexOf(page.op.op) >= 0) r.push(["from", qsTr("Pay from")])
                            if (["withdraw", "withdraw_collateral", "borrow"].indexOf(page.op.op) >= 0) r.push(["to", qsTr("Send to")])
                            if (!page.isPrivate) r.push(["payer", qsTr("Fee paid by")])
                            return r
                        }
                        delegate: Rectangle {
                            height: 34; radius: 17
                            width: routeText.implicitWidth + 24
                            color: "#33000000"
                            border.color: "#4DFFFFFF"
                            LogosText {
                                id: routeText
                                anchors.centerIn: parent
                                textFormat: Text.StyledText
                                text: "<span style='opacity:.8'>" + modelData[1] + "</span> <b>" + page.routeName(modelData[0]) + "</b> ▾"
                                font.pixelSize: 12
                            }
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: page.cycle(modelData[0]) }
                        }
                    }
                }
            }
            Item { Layout.fillWidth: true }
            ColumnLayout {
                Layout.alignment: Qt.AlignBottom | Qt.AlignRight
                spacing: 6
                // The op on its way, in place of the button.
                Rectangle {
                    visible: page.working
                    Layout.alignment: Qt.AlignRight
                    implicitWidth: 300
                    implicitHeight: workCol.implicitHeight + 28
                    radius: 18
                    color: Tide.card
                    border.color: "#40FFFFFF"
                    ColumnLayout {
                        id: workCol
                        anchors.fill: parent; anchors.margins: 14; anchors.leftMargin: 16; anchors.rightMargin: 16
                        spacing: 8
                        LogosText {
                            text: page.tracking && page.tracking.isPrivate ? qsTr("Diving below… %1%").arg(Math.round(page.workPct)) : qsTr("Sending…")
                            font.pixelSize: 16; font.weight: Font.ExtraBold
                        }
                        Rectangle {
                            Layout.fillWidth: true
                            implicitHeight: 6
                            radius: 3
                            color: "#26FFFFFF"
                            clip: true
                            Rectangle {
                                id: workFill
                                height: parent.height
                                radius: 3
                                color: "#FFFFFF"
                                width: page.tracking && page.tracking.isPrivate ? parent.width * page.workPct / 100 : parent.width * 0.3
                                Behavior on width { NumberAnimation { duration: 260; easing.type: Easing.OutCubic } }
                                // Public: a gliding segment while it lands.
                                SequentialAnimation on x {
                                    running: page.working && !(page.tracking && page.tracking.isPrivate)
                                    loops: Animation.Infinite
                                    NumberAnimation { from: -workFill.width; to: workFill.parent.width; duration: 1300; easing.type: Easing.InOutSine }
                                }
                            }
                        }
                        LogosText {
                            text: page.tracking && page.tracking.isPrivate ? qsTr("About %1 on this computer").arg(page.app.diveTime) : qsTr("Usually a few seconds")
                            font.pixelSize: 12; color: Tide.text2
                        }
                    }
                }
                Kit.BigButton {
                    visible: !page.working
                    objectName: "positionGo"
                    Layout.alignment: Qt.AlignRight
                    buttonHeight: 58
                    fontSize: 17
                    enabled: page.problem === null && page.quote !== null && page.quotedKey === page.requestKey && !page.submitting
                    text: page.problem !== null ? page.op.label
                        : page.isPrivate ? qsTr("Dive & %1 ↓").arg(page.op.label.toLowerCase())
                        : (page.sent ? qsTr("Sent ✓") : page.op.label)
                    onClicked: page.submit()
                }
                LogosText {
                    Layout.alignment: Qt.AlignRight
                    visible: page.problem === null && !page.working
                    text: page.isPrivate ? qsTr("Diving takes %1 on this computer").arg(page.app.diveTime) : qsTr("Above the surface · anyone can see this is you")
                    font.pixelSize: 12; color: Tide.text2
                }
            }
        }
    }

    // The note an op leaves when it lands: drops in under the top bar, hides itself after ~5 s.
    Rectangle {
        id: note
        property string symbol: ""
        property real drop: 0
        function show(sym, title, sub) {
            symbol = sym; noteTitle.text = title; noteSub.text = sub
            hideAnim.stop(); dropAnim.restart(); noteTimer.restart()
        }
        anchors.horizontalCenter: parent.horizontalCenter
        y: page.topInset + 6 - 16 * (1 - drop)
        opacity: 0
        visible: opacity > 0
        z: 20
        implicitWidth: noteRow.implicitWidth + 28
        implicitHeight: 56
        radius: 28
        color: "#FFFFFF"
        readonly property var look: Tide.look(symbol)
        Row {
            id: noteRow
            anchors.verticalCenter: parent.verticalCenter
            x: 10
            spacing: 12
            Rectangle {
                width: 36; height: 36; radius: 18
                anchors.verticalCenter: parent.verticalCenter
                color: note.look[1]
                LogosText { anchors.centerIn: parent; text: note.look[0]; font.pixelSize: 16; font.weight: Font.ExtraBold; color: note.look[2] }
            }
            Column {
                anchors.verticalCenter: parent.verticalCenter
                LogosText { id: noteTitle; font.pixelSize: 15; font.weight: Font.Bold; color: Tide.sky }
                LogosText { id: noteSub; visible: text !== ""; font.pixelSize: 13; color: "#3A4A63" }
            }
            Rectangle {
                width: 28; height: 28; radius: 14
                anchors.verticalCenter: parent.verticalCenter
                color: "#E8EDF5"
                LogosText { anchors.centerIn: parent; text: "✕"; font.pixelSize: 12; color: Tide.sky }
                MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: { noteTimer.stop(); hideAnim.restart() } }
            }
        }
        ParallelAnimation {
            id: dropAnim
            NumberAnimation { target: note; property: "drop"; from: 0; to: 1; duration: 560; easing.type: Easing.OutBack; easing.overshoot: 1.1 }
            NumberAnimation { target: note; property: "opacity"; from: 0; to: 1; duration: 260; easing.type: Easing.OutQuad }
        }
        ParallelAnimation {
            id: hideAnim
            NumberAnimation { target: note; property: "opacity"; to: 0; duration: 320; easing.type: Easing.InQuad }
            NumberAnimation { target: note; property: "drop"; to: 0.6; duration: 320; easing.type: Easing.InQuad }
        }
        Timer { id: noteTimer; interval: 5000; onTriggered: hideAnim.restart() }
    }
}
