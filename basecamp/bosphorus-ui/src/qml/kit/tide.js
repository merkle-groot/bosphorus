.pragma library
// D1 "Tide" (docs/design/d1-tide/HANDOFF.md): its colours and its plain-language map. The screen is
// the strait: the water level is debt against borrowing capacity, private things live below the
// surface, proving is a dive. Colours are #AARRGGBB where they carry alpha.

var sky = "#0B1424"
var panel = "#0E1B30"
var panelBorder = "#14FFFFFF"     // rgba(255,255,255,.08)
var glass = "#14FFFFFF"
var glassBorder = "#1FFFFFFF"     // .12
var glassStrong = "#2EFFFFFF"     // .18
var text = "#FFFFFF"
var text2 = "#CCFFFFFF"           // .8
var text3 = "#B3FFFFFF"           // .7
var text4 = "#8CFFFFFF"           // .55
var waterCalm = "#0A5BB8"
var waterDeep = "#083E80"
var waterPrivate = "#0A4FA0"
var waterRising = "#9A5A00"
var waterDanger = "#A3221B"
var good = "#5EE08A"
var warn = "#FFB340", warnTint = "#5A3A06", warnText = "#FFE2B0"
var bad = "#FF7A6E", badTint = "#5A1611", badText = "#FFD6D1"
var info = "#6FB3FF", infoTint = "#123B6B", infoText = "#CFE6FF"
var dive = ["#0A5BB8", "#073F86", "#042553", "#02142F"]
var shade = "#26000000"           // rgba(0,0,0,.15), on round buttons over water
var card = "#66000000"            // the error card

// Wallet v2 (docs/design/wallet-v2/HANDOFF.md): a token's badge [glyph, fill, text colour] and its
// second line, by symbol; tokens list in `tokenOrder`, then by symbol, LEZ last.
var tokenLooks = {
    USDC: ["$", "#2775CA", "#FFFFFF", "US dollar coin"],
    WBTC: ["₿", "#F7931A", "#0B1424", "Wrapped bitcoin"],
    TKA: ["A", "#6B5BD6", "#FFFFFF", "Test token A"],
    TKB: ["B", "#2BB3A3", "#0B1424", "Test token B"],
    LEZ: ["L", "#E6ECF5", "#0B1424", "Pays network fees"]
}
var tokenOrder = ["USDC", "WBTC", "TKA", "TKB"]

function look(symbol) {
    return tokenLooks[symbol] || [String(symbol || "?").charAt(0), "#3A4A66", "#FFFFFF", ""]
}

function tokenRank(symbol) {
    var i = tokenOrder.indexOf(symbol)
    return i >= 0 ? i : tokenOrder.length
}

// Health factor (a number; Infinity with no debt) → the word, and the water's colour.
function word(hf) {
    if (hf === null || hf === undefined || hf === Infinity || hf >= 2) return "Calm water"
    if (hf >= 1.5) return "Safe"
    if (hf >= 1.2) return "Rising"
    if (hf >= 1) return "Nearly over"
    return "Too much"
}

function water(hf, isPrivate) {
    if (hf === null || hf === undefined || hf >= 1.5) return isPrivate ? waterPrivate : waterCalm
    return hf >= 1.2 ? waterRising : waterDanger
}

// Water height (0..1 of the window) for debt ÷ capacity, with the sell line at `sellAt`.
function level(ratio, sellAt) {
    if (!(ratio > 0)) return 0.07
    return Math.min(ratio, 1.1) * sellAt
}

var ops = [
    { op: "supply", label: "Save", act: "Save %1", doing: "Saving %1", token: "loan", all: false },
    { op: "withdraw", label: "Take back", act: "Take back %1", doing: "Taking back %1", token: "loan", all: true },
    { op: "supply_collateral", label: "Add collateral", short: "Add", act: "Add %1 as collateral", doing: "Adding %1 as collateral", token: "collateral", all: false },
    { op: "withdraw_collateral", label: "Remove collateral", short: "Remove", act: "Remove %1 of collateral", doing: "Removing %1 of collateral", token: "collateral", all: true },
    { op: "borrow", label: "Borrow", act: "Borrow %1", doing: "Borrowing %1", token: "loan", all: false },
    { op: "repay", label: "Pay back", act: "Pay back %1", doing: "Paying back %1", token: "loan", all: true }
]

function opLabel(op) {
    for (var i = 0; i < ops.length; i++) if (ops[i].op === op) return ops[i].label
    return op
}

// The op as a phrase around `amount` ("Add 1 WBTC as collateral"); `ing` while it runs.
function opText(op, amount, ing) {
    for (var i = 0; i < ops.length; i++) if (ops[i].op === op) return (ing ? ops[i].doing : ops[i].act).replace("%1", amount)
    return op + " " + amount
}

// Job state → [plain word, accent, tag background, tag text].
var states = {
    queued: ["Waiting", "#8EA3BF", "#1FFFFFFF", "#FFFFFF"],
    preparing: ["Getting ready", info, infoTint, infoText],
    proving: ["Diving", info, waterCalm, "#FFFFFF"],
    sending: ["Sending", info, infoTint, infoText],
    submitted: ["Sent", info, infoTint, infoText],
    done: ["Done", good, "#14432A", "#B8F5CB"],
    failed: ["Didn’t go through", bad, badTint, badText],
    checking: ["Checking", "#7FE0FF", "#0C4152", "#CDF3FF"],
    interrupted: ["Paused", warn, warnTint, warnText],
    unknown: ["Unsure", warn, warnTint, warnText]
}

function stateOf(s) { return states[s] || [s, info, infoTint, infoText] }

// Where a job sits on the five-step track (Waiting, Getting ready, Diving/Sending, Sent, Done).
function stepOf(job) {
    var s = job.state
    if (s === "queued") return 0
    if (s === "preparing") return 1
    if (s === "proving" || s === "sending") return 2
    if (s === "submitted" || s === "checking" || s === "unknown") return 3
    if (s === "done") return 4
    if (s === "interrupted") return 2
    if (s === "failed") return job.tx ? 3 : 1
    return 0
}

// A refused quote (an app-API error) → { title, body, fix } in the strait's words. `ctx` carries
// what the fixes need: { symbol, saved, maxBorrow }.
function refusal(e, ctx) {
    var c = e && e.code
    switch (c) {
    case "unhealthy": return { title: "This goes past the sell line", body: "Your debt would be more than your collateral covers, so some of it could be sold straight away." + (ctx.maxBorrow ? " You can borrow up to " + ctx.maxBorrow + " " + ctx.symbol + " more." : ""), fix: ctx.maxBorrow ? "maxBorrow" : "" }
    case "insufficient_shares": return { title: "You don’t have that much saved", body: "This slot has " + (ctx.saved || "0") + " " + ctx.symbol + " saved.", fix: "all" }
    case "insufficient_collateral": return { title: "Not that much set aside", body: "This slot doesn’t hold that much collateral.", fix: "all" }
    case "insufficient_liquidity": return { title: "Not enough free in this market", body: "Try less, or wait for borrowers to pay back.", fix: "" }
    case "stale_price": return { title: "The price is out of date", body: "Prices must be fresh before anyone can borrow or take collateral out. The oracle has to publish a new one; then try again.", fix: "" }
    case "payer_cannot_fund": return { title: "The fee account is empty", body: "It can’t cover the network fee. Move a little LEZ into it, or pay the fee from another account.", fix: "payer" }
    case "insufficient_balance": return { title: "Not that much in this account", body: ctx.has ? "It holds " + ctx.has + " " + ctx.symbol + "." : "It doesn’t hold that much " + ctx.symbol + ".", fix: ctx.has ? "max" : "" }
    case "network": return { title: "Can’t reach the network", body: "Bosphorus can’t get the market’s numbers right now. Nothing was sent. It tries again by itself when the connection is back.", fix: "" }
    case "rejected": return { title: "The network refused this", body: "Nothing moved. " + String((e && e.message) || ""), fix: "" }
    }
    return { title: "That didn’t work", body: String((e && e.message) || "Something went wrong"), fix: "" }
}
