import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Logos.Theme
import Logos.Controls
import "../kit" as Kit
import "../kit/units.js" as Units
import "../kit/tide.js" as Tide

// TPosition: one position in one market, as two cards, Lending and Borrowing; each action opens
// a panel with one amount and one button. The water is the loan only: the position's debt against
// its borrowing capacity (debt ÷ (collateral × price × LLTV)), in the Borrowing card as it stands
// and in the panel of an op that moves it, where a dashed line previews where the op takes it.
// Lending has no water. When a submitted op lands (docs/design/wallet-v2/HANDOFF.md, "Position"):
// the water settles, coins fall in or rise out, the figure that changed glows and a note drops
// in. Every position is private: ops are dives.
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
    property bool sheet: false        // an op's panel is open

    // The position: held, a private slot, or the public one.
    readonly property var positions: m ? app.accountPositions(marketId, account) : []
    // Another slot holds something (or one is chosen): the slot switcher shows.
    readonly property bool otherSlots: app.slot !== 0 || positions.some(function(p) { return !p.held && p.index !== 0 && app.hasAnything(p) })
    // The slot (collateral, debt, maybe lending) and the lending held on the account itself (E9).
    readonly property var slotPos: {
        for (var i = 0; i < positions.length; i++) if (!positions[i].held && (!isPrivate || positions[i].index === app.slot)) return positions[i]
        return null
    }
    readonly property var heldPos: {
        for (var i = 0; i < positions.length; i++) if (positions[i].held) return positions[i]
        return null
    }
    readonly property var pos: useHeld ? heldPos : slotPos
    readonly property string saved: pos && pos.supplied_assets ? pos.supplied_assets : "0"
    readonly property string coll: slotPos ? slotPos.collateral : "0"
    readonly property string debt: slotPos && slotPos.debt ? slotPos.debt : "0"
    readonly property string heldLent: heldPos && heldPos.supplied_assets ? heldPos.supplied_assets : "0"
    readonly property string slotLent: slotPos && slotPos.supplied_assets ? slotPos.supplied_assets : "0"
    readonly property string lent: Units.add(heldLent, slotLent)
    // Where an op's tokens come from, and what that account holds of them (null: unknown).
    readonly property string payFrom: from !== "" ? from : account
    readonly property string payToken: m ? (op.token === "loan" ? m.loan_asset : m.collateral_asset) : ""
    readonly property var payBalance: m ? app.payable(m.market, payFrom, payToken) : null
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
    // What the market lets a private op borrow against: its health check prices the collateral
    // 2% under the feed (client ClaimConfig::private, price_width), as the price may move while proving.
    readonly property real claimPrice: 0.98
    function borrowable(c) { return capacity(c) * claimPrice }
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
        if ((op.op === "borrow" || op.op === "withdraw_collateral") && px > 0 && parseFloat(after.debt) > borrowable(after.coll))
            return Tide.refusal({ code: "unhealthy" }, { symbol: loanSym, maxBorrow: op.op === "borrow" && safeMax() !== "0" ? Units.format(safeMax(), loanDec) : "" })
        return null
    }
    readonly property var problem: localProblem !== null ? localProblem : (quoteError ? Tide.refusal(quoteError, { symbol: symbol, saved: Units.format(saved, loanDec), has: payBalance !== null ? Units.format(payBalance, decimals) : "", maxBorrow: op.op === "borrow" && safeMax() !== "0" ? Units.format(safeMax(), loanDec) : "" }) : null)
    readonly property bool spills: problem !== null && (localProblem !== null ? (op.op === "borrow" || op.op === "withdraw_collateral") && parseFloat(after.debt) > borrowable(after.coll) : quoteError && quoteError.code === "unhealthy")

    // 99% of what the market would still lend, in loan base units ("0" if nothing).
    function safeMax() {
        var room = borrowable(coll) - parseFloat(debt)
        if (!(room > 0)) return "0"
        var v = Math.floor(room * 0.99)
        return v > 0 ? v.toFixed(0) : "0"
    }

    // The water: debt ÷ capacity now, and after the op.
    readonly property real ratioNow: {
        var cap = capacity(coll), d = parseFloat(debt)
        return d > 0 ? (cap > 0 ? d / cap : 1.1) : 0
    }
    readonly property real ratioShown: {
        var c = problem ? coll : after.coll, d = problem ? debt : after.debt
        var cap = capacity(c)
        return parseFloat(d) > 0 ? (cap > 0 ? parseFloat(d) / cap : 1.1) : 0
    }
    // The sell line: 80% up the card; in the panel, under its title.
    readonly property real cardAt: 0.78
    readonly property real sheetAt: 0.7
    readonly property real level: spills ? sheetAt + 0.03 : Tide.level(ratioShown, sheetAt)
    readonly property real levelNow: Tide.level(ratioNow, sheetAt)
    // The tour shows it on an empty position too.
    readonly property bool showLine: parseFloat(debt) > 0 || !!app.showSellLine
    // The panel has water when its op moves the loan: borrowing, or anything once there's debt.
    readonly property bool sheetWater: !lend && (op.op === "borrow" || parseFloat(debt) > 0 || spills)
    readonly property bool preview: !working && (spills || (request !== null && problem === null && Math.abs(level - levelNow) > 0.004))

    // "Safe unless WBTC falls below 35,000 USDC": the collateral price at the sell line, for c and d.
    function sellPrice(c, d) {
        if (!(parseFloat(d) > 0 && parseFloat(c) > 0 && lltv > 0)) return ""
        var liq = (parseFloat(d) / Math.pow(10, loanDec)) / ((parseFloat(c) / Math.pow(10, collDec)) * lltv)
        return Number(liq.toPrecision(4)).toLocaleString(Qt.locale("en_US"), "f", liq >= 100 ? 0 : 2)
    }
    readonly property string priceNow: feed ? Units.price(feed.price, loanDec, collDec) : "–"
    // What the collateral lets you borrow in all, in loan base units.
    function limitOf(c) { var v = Math.floor(borrowable(c)); return v > 0 ? v.toFixed(0) : "0" }

    // The market menu is open (the guide points into it).
    readonly property bool marketMenuOpen: marketMenu.opened

    // ---- the panel ----------------------------------------------------------------------

    function open(i) {
        var o = Tide.ops[i].op
        // Lending goes on the account itself unless only the slot has some to take back.
        if (o === "supply") held = true
        if (o === "withdraw") held = !(Units.isZero(heldLent) && !Units.isZero(slotLent))
        opIndex = i; useAll = false; amountInput.text = ""; more = false; from = ""; to = ""; payer = ""
        marketMenu.close()
        sheet = true
    }
    function openOp(o) { for (var i = 0; i < Tide.ops.length; i++) if (Tide.ops[i].op === o) open(i) }
    function close() { sheet = false; useAll = false; amountInput.text = "" }

    readonly property string opTitle: op.token === "collateral" ? qsTr("%1 %2").arg(op.short).arg(collSym) : qsTr("%1 %2").arg(op.label).arg(loanSym)
    readonly property string opSub: {
        if (!m) return ""
        var has = payBalance !== null ? Units.format(payBalance, decimals, 2) + " " + symbol : ""
        switch (op.op) {
        case "supply": case "supply_collateral": return has !== "" ? qsTr("You have %1 in %2").arg(has).arg(app.accountName(payFrom)) : ""
        case "repay": return qsTr("You owe %1 %2").arg(Units.format(debt, loanDec, 2)).arg(loanSym) + (has !== "" ? qsTr(" · you have %1").arg(has) : "")
        case "withdraw": return qsTr("You've lent %1 %2").arg(Units.format(saved, loanDec, 2)).arg(loanSym)
        case "withdraw_collateral": return qsTr("%1 %2 put up").arg(Units.format(coll, collDec, 2)).arg(collSym)
        case "borrow": return Units.isZero(coll) ? qsTr("Add %1 first: a loan needs collateral").arg(collSym) : qsTr("Against your %1 %2").arg(Units.format(coll, collDec, 2)).arg(collSym)
        }
        return ""
    }
    // Half of a base-unit amount, rounded down.
    function half(raw) {
        var s = Units.stripZeros(String(raw)), out = "", r = 0
        for (var i = 0; i < s.length; i++) { var n = r * 10 + (s.charCodeAt(i) - 48); out += String(Math.floor(n / 2)); r = n % 2 }
        return Units.stripZeros(out) || "0"
    }
    // The quick amounts: half and the most it can be ("all" fills in the whole position).
    readonly property var picks: {
        if (!m) return []
        var top = "0", all = false
        switch (op.op) {
        case "supply": case "supply_collateral": top = payBalance || "0"; break
        case "withdraw": top = saved; all = true; break
        case "borrow": top = safeMax(); break
        case "repay": all = payBalance === null || Units.cmp(payBalance, debt) >= 0; top = all ? debt : payBalance; break
        case "withdraw_collateral":
            if (parseFloat(debt) > 0) { var keep = parseFloat(debt) / (px * lltv * claimPrice) * 1.01; top = px > 0 ? Math.max(0, Math.floor(parseFloat(coll) - keep)).toFixed(0) : "0" }
            else { top = coll; all = true }
            break
        }
        if (Units.isZero(top)) return []
        var max = op.op === "borrow" || (op.op === "withdraw_collateral" && !all) ? qsTr("Max") : qsTr("All")
        return [{ label: qsTr("Half · %1").arg(Units.format(half(top), decimals, 2)), raw: half(top), all: false },
                { label: max + " · " + Units.format(top, decimals, 2), raw: top, all: all }]
    }

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
        // Landed: the panel closes so the cards show it.
        if (trackedJob.state === "done") { landed = t; sheet = false; landedWait.restart() }
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
    // While a tracked op runs: its progress, from the job's state (proving 1–2 min on a GPU).
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
        case "supply": title = qsTr("Lent %1").arg(amt); sub = qsTr("Now earning %1% a year").arg(t.apr) + (t.isPrivate ? qsTr(" · below the surface") : ""); break
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
            if (key !== "saved") splash.run(out, t.symbol, (out ? "+" : "−") + amt)
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
        case "supply": return page.useHeld ? qsTr("%1 came into the market. New market totals. No position.").arg(a) : qsTr("Some position lent %1. New market totals.").arg(a)
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
        // Markets' Lend / Borrow open the panel; a borrow with nothing put up starts with the collateral.
        function onPresetOpChanged() {
            var o = page.app.presetOp
            if (o === "") return
            page.app.presetOp = ""
            page.openOp(o === "borrow" && Units.isZero(page.coll) ? "supply_collateral" : o)
        }
        function onPrefillChanged() { if (page.app.prefill.amount) amountInput.text = page.app.prefill.amount }
    }
    // The guide's "Use …" button: an amount in base units, typed in for the user.
    function fill(raw) { page.useAll = false; amountInput.text = Units.exact(raw, page.decimals) }

    // ---- the two cards ----------------------------------------------------------------

    ColumnLayout {
        anchors.fill: parent
        anchors.topMargin: page.topInset
        anchors.leftMargin: 28
        anchors.rightMargin: 28
        anchors.bottomMargin: 24
        spacing: 16

        // Which market (and, when there's a choice, which account and slot).
        RowLayout {
            Layout.fillWidth: true
            spacing: 10
            Kit.Pill {
                objectName: "positionMarket"
                visible: page.app.markets.length > 1
                text: page.loanSym + " · " + page.collSym + (page.m && page.m.pool ? " · " + qsTr("Screened") : "") + (marketMenu.opened ? " ▴" : " ▾")
                on: marketMenu.opened
                onClicked: marketMenu.opened ? marketMenu.close() : marketMenu.open()
                // Every market, the open one ticked.
                Popup {
                    id: marketMenu
                    y: parent.height + 8
                    width: 300
                    padding: 6
                    background: Rectangle { color: "#13233D"; border.color: "#24FFFFFF"; radius: 14 }
                    contentItem: Column {
                        spacing: 2
                        Repeater {
                            model: page.app.markets
                            delegate: Rectangle {
                                readonly property string loan: page.app.tokenSymbol(modelData.loan_asset)
                                readonly property string collateral: page.app.tokenSymbol(modelData.collateral_asset)
                                readonly property bool here: modelData.market === page.marketId
                                objectName: modelData.market === page.app.marketFor(collateral) ? "positionMarket-" + collateral : ""
                                width: 288; height: 54; radius: 10
                                color: hover.containsMouse ? "#1FFFFFFF" : here ? "#12FFFFFF" : "transparent"
                                Column {
                                    x: 12; anchors.verticalCenter: parent.verticalCenter; spacing: 2
                                    LogosText { text: loan + " · " + collateral + (modelData.pool ? " · " + qsTr("Screened") : ""); font.pixelSize: 15; font.weight: Font.Bold }
                                    LogosText { text: modelData.pool ? qsTr("Screened money only") : qsTr("Borrow %1 with %2").arg(loan).arg(collateral); font.pixelSize: 12; color: Tide.text3 }
                                }
                                LogosText { visible: here; anchors.right: parent.right; anchors.rightMargin: 14; anchors.verticalCenter: parent.verticalCenter; text: "✓"; font.pixelSize: 15; font.weight: Font.Bold }
                                MouseArea {
                                    id: hover
                                    anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor
                                    onClicked: { marketMenu.close(); page.app.openMarket = modelData.market }
                                }
                            }
                        }
                    }
                }
            }
            Flow {
                objectName: "positionWho"
                spacing: 6
                // Private accounts only: lending and borrowing stay below the surface.
                Repeater {
                    model: {
                        var r = page.app.accounts.filter(function(a) { return Units.isPrivate(a.account) })
                        return r.length > 1 ? r : []
                    }
                    delegate: Kit.Pill {
                        objectName: "who-private"
                        text: page.app.accountName(modelData.account)
                        on: page.account === modelData.account
                        onClicked: { page.app.actAs = modelData.account; page.app.slot = 0 }
                    }
                }
            }
            SlotPicker { visible: page.otherSlots }
            Item { Layout.fillWidth: true }
            LogosText { visible: !page.m; text: qsTr("No market yet"); font.pixelSize: 14; color: "#E0FFFFFF" }
        }

        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.minimumHeight: 300
            Layout.maximumHeight: 470
            spacing: 24

            // Lending: no water. What's lent, what it earns.
            Rectangle {
                objectName: "positionLend"
                Layout.fillWidth: true
                Layout.fillHeight: true
                radius: 22
                color: Tide.panel
                border.color: Tide.glassBorder
                ColumnLayout {
                    anchors.fill: parent; anchors.margins: 32; anchors.topMargin: borrowCard.height * (1 - page.cardAt) + 22; spacing: 8
                    LogosText { text: qsTr("Lending"); font.pixelSize: 15; font.weight: Font.Bold; color: Tide.text3 }
                    Figure { key: "saved"; text: Units.format(page.lent, page.loanDec, 2) + " " + page.loanSym }
                    LogosText {
                        Layout.fillWidth: true; wrapMode: Text.Wrap
                        text: Units.isZero(page.lent) ? qsTr("Lend %1 and earn %2% a year. Take it back whenever you like.").arg(page.loanSym).arg(page.apr)
                                                      : qsTr("Earning %1% a year. Take it back whenever you like.").arg(page.apr)
                        font.pixelSize: 16; color: Tide.text2
                    }
                    Item { Layout.fillHeight: true }
                    RowLayout {
                        spacing: 12
                        Kit.BigButton { objectName: "op-supply"; enabled: !!page.m; text: qsTr("Lend %1").arg(page.loanSym); buttonHeight: 50; onClicked: page.openOp("supply") }
                        Kit.BigButton { objectName: "op-withdraw"; visible: !Units.isZero(page.lent); text: qsTr("Take back"); buttonHeight: 50; outline: true; onClicked: page.openOp("withdraw") }
                    }
                }
            }

            // Borrowing: the water is the loan, as it stands.
            Rectangle {
                id: borrowCard
                objectName: "positionBorrow"
                Layout.fillWidth: true
                Layout.fillHeight: true
                radius: 22
                color: Tide.panel
                border.color: Tide.glassBorder
                clip: true
                readonly property bool owes: parseFloat(page.debt) > 0
                readonly property bool hasColl: !Units.isZero(page.coll)
                Kit.Water {
                    id: cardWater
                    visible: borrowCard.owes || borrowCard.hasColl
                    anchors.margins: 1
                    bottomLeftRadius: 21; bottomRightRadius: 21
                    level: Tide.level(page.ratioNow, page.cardAt)
                    color: Tide.water(page.hfNow, true)
                    settleMs: 1400
                    overshoot: 0.9
                }
                Kit.SellLine { objectName: "sellLine"; at: page.cardAt; visible: page.showLine; label: qsTr("%1 is sold if the water gets here").arg(page.collSym) }
                Kit.Splash { id: splash; surfaceY: cardWater.y; ceiling: 10 }
                ColumnLayout {
                    anchors.fill: parent; anchors.margins: 32; anchors.topMargin: borrowCard.height * (1 - page.cardAt) + 22; spacing: 8
                    LogosText { text: qsTr("Borrowing"); font.pixelSize: 15; font.weight: Font.Bold; color: Tide.text3 }
                    Figure { key: "debt"; text: Units.format(page.debt, page.loanDec, 2) + " " + page.loanSym }
                    LogosText {
                        Layout.fillWidth: true; wrapMode: Text.Wrap
                        text: borrowCard.owes ? (page.hfNow >= 1.5 ? qsTr("Against %1 %2. Safe unless %2 falls below %3 (now %4).") : Tide.word(page.hfNow) + qsTr(": against %1 %2, sold if %2 falls below %3 (now %4)."))
                                                    .arg(Units.format(page.coll, page.collDec, 2)).arg(page.collSym).arg(page.sellPrice(page.coll, page.debt)).arg(page.priceNow)
                            : borrowCard.hasColl ? qsTr("%1 %2 put up. You can borrow up to %3 %4.").arg(Units.format(page.coll, page.collDec, 2)).arg(page.collSym)
                                                       .arg(Units.format(page.limitOf(page.coll), page.loanDec, 0)).arg(page.loanSym)
                            : qsTr("Put up %1 to borrow %2 against it. The %1 stays yours.").arg(page.collSym).arg(page.loanSym)
                        font.pixelSize: 15; color: Tide.text2
                    }
                    Item { Layout.fillHeight: true }
                    RowLayout {
                        spacing: 12
                        Kit.BigButton {
                            objectName: borrowCard.hasColl || borrowCard.owes ? "op-borrow" : "op-supply_collateral"
                            enabled: !!page.m
                            text: borrowCard.owes ? qsTr("Borrow more") : borrowCard.hasColl ? qsTr("Borrow %1").arg(page.loanSym) : qsTr("Add %1").arg(page.collSym)
                            buttonHeight: 50
                            onClicked: page.openOp(borrowCard.hasColl || borrowCard.owes ? "borrow" : "supply_collateral")
                        }
                        Kit.BigButton { objectName: "op-repay"; visible: borrowCard.owes; text: qsTr("Pay back"); buttonHeight: 50; outline: true; onClicked: page.openOp("repay") }
                        Item { Layout.fillWidth: true }
                        Row {
                            visible: borrowCard.hasColl
                            spacing: 18
                            CardLink { objectName: "op-supply_collateral-link"; text: qsTr("Add %1").arg(page.collSym); onClicked: page.openOp("supply_collateral") }
                            CardLink { objectName: "op-withdraw_collateral"; text: qsTr("Remove %1").arg(page.collSym); onClicked: page.openOp("withdraw_collateral") }
                        }
                    }
                }
            }
        }
        LogosText { text: qsTr("Private: only you know these are yours."); font.pixelSize: 13; color: Tide.text3 }
        Item { Layout.fillHeight: true; Layout.maximumHeight: Math.max(0, page.height - page.topInset - 560) }
    }

    readonly property string apr: m ? (parseFloat(m.supply_apr) * 100).toFixed(2) : "–"

    // A card's big figure; it glows when an op just changed it.
    component Figure: Rectangle {
        property string key: ""
        property alias text: figureText.text
        implicitWidth: figureText.implicitWidth + 16
        implicitHeight: figureText.implicitHeight + 4
        Layout.leftMargin: -8
        radius: 12
        color: Qt.rgba(1, 1, 1, page.glow === key ? 0.18 * page.glowLevel : 0)
        LogosText { id: figureText; x: 8; anchors.verticalCenter: parent.verticalCenter; font.pixelSize: 52; font.weight: Font.ExtraBold; font.letterSpacing: -1.5 }
    }
    component CardLink: LogosText {
        signal clicked()
        font.pixelSize: 14; font.underline: true; font.weight: Font.DemiBold; color: "#FFFFFF"
        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: parent.clicked() }
    }
    component SlotPicker: Rectangle {
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

    // ---- an op's panel ----------------------------------------------------------------


    Rectangle {
        anchors.fill: parent
        visible: page.sheet
        color: "#B3050A14"
        MouseArea { anchors.fill: parent; onClicked: page.close() }
    }
    Rectangle {
        id: frame
        objectName: "positionSheet"
        visible: page.sheet
        anchors.horizontalCenter: parent.horizontalCenter
        y: page.topInset + 6
        width: Math.max(0, Math.min(640, page.width - 56))
        height: Math.max(0, Math.min(660, page.height - page.topInset - 20))
        radius: 22
        color: "#122440"
        border.color: Tide.glassBorder
        clip: true
        MouseArea { anchors.fill: parent }   // clicks stay in the panel

        // The water, when the op moves the loan.
        Item {
            anchors.fill: parent
            visible: page.sheetWater
            Kit.Water { id: water; anchors.margins: 1; bottomLeftRadius: 21; bottomRightRadius: 21; level: page.levelNow; color: Tide.water(page.hfNow, true); settleMs: 1400; overshoot: 0.9 }
            Kit.SellLine { at: page.sheetAt; label: qsTr("%1 is sold if the water gets here").arg(page.collSym) }
            // The rise (or fall) the op would make, shaded between the water and the preview line.
            Rectangle {
                anchors.left: parent.left; anchors.right: parent.right
                y: Math.min(previewLine.y + 2, water.y)
                height: Math.abs(water.y - previewLine.y - 2)
                color: page.spills ? Qt.rgba(1, 0.33, 0.33, 0.22) : Qt.rgba(1, 1, 1, 0.07)
                opacity: previewLine.opacity
            }
            Item {
                id: previewLine
                anchors.left: parent.left; anchors.right: parent.right
                height: 2
                // Kept above the quote and button (paying it all back would put it under them),
                // but a falling preview never above the water it falls from.
                readonly property real target: page.spills ? frame.height * (1 - page.sheetAt) - 32 : frame.height * (1 - page.level)
                y: Math.min(target, Math.max(actions.y - 16, water.y + 12))
                opacity: page.preview ? 1 : 0
                Behavior on y { NumberAnimation { duration: 320; easing.type: Easing.OutCubic } }
                Behavior on opacity { NumberAnimation { duration: 220 } }
                Row { spacing: 6; Repeater { model: Math.max(0, Math.ceil(previewLine.width / 14)); delegate: Rectangle { width: 8; height: 2; color: "#FFFFFF"; opacity: 0.85 } } }
                LogosText {
                    x: 24; anchors.bottom: parent.top; anchors.bottomMargin: 6
                    text: page.spills ? qsTr("After: too much, your %1 could be sold").arg(page.collSym) : qsTr("After: %1").arg(Tide.word(page.hfAfter).toLowerCase())
                    font.pixelSize: 12; font.weight: Font.Bold; color: page.spills ? Tide.badText : "#FFFFFF"
                }
            }
        }

        // Title.
        LogosText { x: 36; y: 28; text: page.opTitle; font.pixelSize: 24; font.weight: Font.ExtraBold }
        LogosText { x: 36; y: 64; width: frame.width - 72; elide: Text.ElideRight; text: page.opSub; font.pixelSize: 15; color: Tide.text3 }
        Rectangle {
            objectName: "positionClose"
            anchors.right: parent.right; anchors.rightMargin: 22; y: 24
            width: 36; height: 36; radius: 18; color: Tide.glass
            LogosText { anchors.centerIn: parent; text: "✕"; font.pixelSize: 15; color: Tide.text2 }
            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: page.close() }
        }

        // The amount.
        ColumnLayout {
            objectName: "positionAmountBox"
            anchors.horizontalCenter: parent.horizontalCenter
            y: frame.height * (1 - page.sheetAt) + 14
            spacing: 4
            Item {
                Layout.alignment: Qt.AlignHCenter
                implicitWidth: Math.max(220, amountInput.contentWidth + 20)
                implicitHeight: 84
                TextInput {
                    id: amountInput
                    objectName: "positionAmount"
                    visible: !page.useAll
                    anchors.fill: parent
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    font.family: Theme.typography.publicSans
                    font.pixelSize: 76
                    font.weight: Font.ExtraBold
                    font.letterSpacing: -4
                    color: "#FFFFFF"
                    selectionColor: Tide.waterDeep
                    focus: page.sheet
                    onTextChanged: if (text !== "") page.useAll = false
                }
                LogosText {
                    anchors.centerIn: parent
                    visible: page.useAll || amountInput.text === ""
                    text: page.useAll ? qsTr("All") : "0"
                    font.pixelSize: 76; font.weight: Font.ExtraBold; font.letterSpacing: -4
                    color: page.useAll ? "#FFFFFF" : "#4DFFFFFF"
                }
            }
            LogosText {
                Layout.alignment: Qt.AlignHCenter
                text: page.useAll ? Units.format(page.amount, page.decimals) + " " + page.symbol : page.symbol
                font.pixelSize: 17; font.weight: Font.DemiBold; opacity: 0.85
            }
            Row {
                Layout.alignment: Qt.AlignHCenter
                Layout.topMargin: 4
                spacing: 8
                Repeater {
                    model: page.picks
                    delegate: Kit.Pill {
                        pillHeight: 32; fontSize: 13; dark: true
                        text: modelData.label
                        on: modelData.all ? page.useAll : (!page.useAll && page.raw === modelData.raw)
                        onClicked: {
                            if (modelData.all) { page.useAll = true; amountInput.text = "" }
                            else { page.useAll = false; amountInput.text = Units.exact(modelData.raw, page.decimals) }
                        }
                    }
                }
            }
        }

        // What happens, and go.
        ColumnLayout {
            id: actions
            anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
            anchors.margins: 32; anchors.bottomMargin: 24
            spacing: 10

            // The quote, in a sentence or two.
            ColumnLayout {
                visible: page.problem === null && page.request !== null && !page.working
                spacing: 4
                LogosText { visible: page.quoting && page.quote === null; text: qsTr("Asking the market…"); font.pixelSize: 15; font.weight: Font.DemiBold; opacity: 0.8 }
                Repeater {
                    model: {
                        if (page.quote === null) return []
                        var a = page.after, q = []
                        var fmtL = function(v) { return Units.format(v, page.loanDec, 2) + " " + page.loanSym }
                        switch (page.op.op) {
                        case "supply":
                            var yr = parseFloat(page.amount) / Math.pow(10, page.loanDec) * parseFloat(page.m.supply_apr)
                            q.push(qsTr("Earns %1% a year: about %2 %3 a year.").arg(page.apr).arg(yr.toLocaleString(Qt.locale("en_US"), "f", 2)).arg(page.loanSym))
                            q.push(qsTr("Take it back whenever you like, while the market has %1 free (%2 now).").arg(page.loanSym).arg(fmtL(Units.sub(page.m.supplied, page.m.borrowed))))
                            break
                        case "withdraw": q.push(qsTr("You'll have %1 lent.").arg(fmtL(a.saved))); break
                        case "borrow": case "repay":
                            q.push(Units.isZero(a.debt) ? qsTr("You'll owe nothing. All paid off.") : qsTr("You'll owe %1.").arg(fmtL(a.debt)))
                            if (!Units.isZero(a.debt)) q.push(qsTr("Safe unless %1 falls below %2 %3 (now %4).").arg(page.collSym).arg(page.sellPrice(a.coll, a.debt)).arg(page.loanSym).arg(page.priceNow))
                            break
                        case "supply_collateral": case "withdraw_collateral":
                            if (parseFloat(page.debt) > 0)
                                q.push(qsTr("The water goes %1. Safe unless %2 falls below %3 %4 (was %5).").arg(page.op.op === "supply_collateral" ? qsTr("down") : qsTr("up"))
                                       .arg(page.collSym).arg(page.sellPrice(a.coll, a.debt)).arg(page.loanSym).arg(page.sellPrice(page.coll, page.debt)))
                            else q.push(qsTr("You'll be able to borrow up to %1.").arg(fmtL(page.limitOf(a.coll))))
                            break
                        }
                        return q
                    }
                    delegate: LogosText { Layout.fillWidth: true; wrapMode: Text.Wrap; text: modelData; font.pixelSize: 15; font.weight: index === 0 ? Font.DemiBold : Font.Normal; color: index === 0 ? "#FFFFFF" : Tide.text2 }
                }
                LogosText {
                    visible: page.quote !== null && !!page.quote.valid_until && page.quote.valid_until - page.now < 120000
                    text: {
                        if (!page.quote || !page.quote.valid_until) return ""
                        var s = Math.max(0, Math.round((page.quote.valid_until - page.now) / 1000))
                        return qsTr("Offer holds for %1 more seconds.").arg(s)
                    }
                    font.pixelSize: 13; color: Tide.text2
                }
            }

            // A refusal, with its one-tap fix.
            Rectangle {
                visible: page.problem !== null && !page.working
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

            // The op on its way, in place of the button.
            Rectangle {
                visible: page.working
                Layout.fillWidth: true
                implicitHeight: workCol.implicitHeight + 28
                radius: 18
                color: Tide.card
                border.color: "#40FFFFFF"
                ColumnLayout {
                    id: workCol
                    anchors.fill: parent; anchors.margins: 14; anchors.leftMargin: 16; anchors.rightMargin: 16
                    spacing: 8
                    LogosText { text: qsTr("Diving below… %1%").arg(Math.round(page.workPct)); font.pixelSize: 16; font.weight: Font.ExtraBold }
                    Rectangle {
                        Layout.fillWidth: true
                        implicitHeight: 6
                        radius: 3
                        color: "#26FFFFFF"
                        Rectangle {
                            height: parent.height; radius: 3; color: "#FFFFFF"
                            width: parent.width * page.workPct / 100
                            Behavior on width { NumberAnimation { duration: 260; easing.type: Easing.OutCubic } }
                        }
                    }
                    LogosText { text: qsTr("About %1 on this computer").arg(page.app.diveTime); font.pixelSize: 12; color: Tide.text2 }
                }
            }
            Kit.BigButton {
                visible: !page.working
                objectName: "positionGo"
                Layout.fillWidth: true
                buttonHeight: 56
                fontSize: 17
                enabled: page.problem === null && page.quote !== null && page.quotedKey === page.requestKey && !page.submitting
                text: page.request === null || page.problem !== null ? page.opTitle
                    : Tide.opText(page.op.op, (page.useAll ? qsTr("all") : Units.format(page.amount, page.decimals)) + " " + page.symbol, false)
                onClicked: page.submit()
            }
            LogosText {
                Layout.alignment: Qt.AlignHCenter
                visible: !page.working
                text: qsTr("Private · takes %1").arg(page.app.diveTime)
                font.pixelSize: 12; color: Tide.text2
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
