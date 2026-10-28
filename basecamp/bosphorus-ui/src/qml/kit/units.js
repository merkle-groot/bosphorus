.pragma library
// Amounts as decimal strings of base units (the app API's u128s), converted without floats: QML's
// JavaScript has no BigInt, and a double loses base units past 2^53. Floats only for display of
// ratios (APRs, health factors, prices), never for an amount that is sent.

function digits(s) { return /^[0-9]+$/.test(String(s)) }

function stripZeros(s) {
    s = String(s).replace(/^0+/, "")
    return s === "" ? "0" : s
}

// "1234567" with 3 decimals → "1,234.567"; `maxFrac` fraction digits at most (truncated, never
// rounded up), trailing zeros dropped. A non-zero amount below the shown precision reads "<0.0001".
function format(raw, decimals, maxFrac) {
    if (raw === null || raw === undefined || !digits(raw)) return "–"
    decimals = decimals || 0
    if (maxFrac === undefined) maxFrac = 4
    var s = stripZeros(raw)
    while (s.length <= decimals) s = "0" + s
    var whole = s.slice(0, s.length - decimals)
    var frac = s.slice(s.length - decimals).slice(0, maxFrac).replace(/0+$/, "")
    var grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",")
    if (whole === "0" && frac === "" && stripZeros(raw) !== "0")
        return "<0." + "0".repeat(Math.max(maxFrac - 1, 0)) + "1"
    return frac === "" ? grouped : grouped + "." + frac
}

// "1,234.5" with 3 decimals → "1234500"; "" when it isn't an amount or has too many decimals.
function parse(text, decimals) {
    var t = String(text).trim().replace(/,/g, "")
    if (!/^[0-9]*\.?[0-9]*$/.test(t) || t === "" || t === ".") return ""
    var parts = t.split(".")
    var whole = parts[0] || "0"
    var frac = parts.length > 1 ? parts[1] : ""
    if (frac.length > (decimals || 0)) return ""
    while (frac.length < (decimals || 0)) frac += "0"
    return stripZeros(whole + frac)
}

// The full-precision decimal form of `raw`, for putting an exact amount back into a field (Max).
function exact(raw, decimals) {
    if (!digits(raw)) return ""
    var s = stripZeros(raw)
    decimals = decimals || 0
    while (s.length <= decimals) s = "0" + s
    var whole = s.slice(0, s.length - decimals)
    var frac = s.slice(s.length - decimals).replace(/0+$/, "")
    return frac === "" ? whole : whole + "." + frac
}

// -1, 0, 1 for two base-unit strings.
function cmp(a, b) {
    a = stripZeros(a); b = stripZeros(b)
    if (a.length !== b.length) return a.length < b.length ? -1 : 1
    return a < b ? -1 : (a > b ? 1 : 0)
}

function isZero(raw) { return !digits(raw) || stripZeros(raw) === "0" }

// a - b for base-unit strings, floored at 0.
function sub(a, b) {
    if (cmp(a, b) <= 0) return "0"
    a = stripZeros(a); b = stripZeros(b)
    while (b.length < a.length) b = "0" + b
    var out = "", borrow = 0
    for (var i = a.length - 1; i >= 0; i--) {
        var d = (a.charCodeAt(i) - 48) - (b.charCodeAt(i) - 48) - borrow
        borrow = d < 0 ? 1 : 0
        out = String(d < 0 ? d + 10 : d) + out
    }
    return stripZeros(out)
}

// a + b for base-unit strings.
function add(a, b) {
    a = digits(a) ? stripZeros(a) : "0"; b = digits(b) ? stripZeros(b) : "0"
    while (a.length < b.length) a = "0" + a
    while (b.length < a.length) b = "0" + b
    var out = "", carry = 0
    for (var i = a.length - 1; i >= 0; i--) {
        var d = (a.charCodeAt(i) - 48) + (b.charCodeAt(i) - 48) + carry
        carry = d >= 10 ? 1 : 0
        out = String(d % 10) + out
    }
    return stripZeros(carry ? "1" + out : out)
}

// A WAD-scaled decimal string ("0.018866…") as a percentage.
function percent(wad, places) {
    var x = parseFloat(wad)
    if (isNaN(x)) return "–"
    return (x * 100).toFixed(places === undefined ? 2 : places) + "%"
}

// A health factor (WAD string, null when there is no debt).
function health(wad) {
    if (wad === null || wad === undefined) return "∞"
    var x = parseFloat(wad)
    if (isNaN(x)) return "–"
    return x >= 100 ? "> 100" : x.toFixed(2)
}

// 0 safe, 1 watch, 2 at risk: a health factor's band.
function healthBand(wad) {
    if (wad === null || wad === undefined) return 0
    var x = parseFloat(wad)
    return x >= 1.5 ? 0 : (x >= 1.1 ? 1 : 2)
}

// The oracle price (1e36-scaled loan base units per collateral base unit) as loan tokens per
// whole collateral token.
function price(raw, loanDecimals, collateralDecimals) {
    var x = parseFloat(raw)
    if (isNaN(x)) return "–"
    var v = x / 1e36 * Math.pow(10, (collateralDecimals || 0) - (loanDecimals || 0))
    return v >= 1000 ? v.toFixed(0).replace(/\B(?=(\d{3})+(?!\d))/g, ",") : (v >= 1 ? v.toFixed(4).replace(/\.?0+$/, "") : v.toPrecision(4))
}

function ago(ms) {
    if (ms === null || ms === undefined || isNaN(ms)) return "–"
    var s = Math.max(0, Math.round(ms / 1000))
    if (s < 60) return s + " s"
    if (s < 3600) return Math.round(s / 60) + " min"
    if (s < 86400) return (s / 3600).toFixed(s < 36000 ? 1 : 0) + " h"
    return Math.round(s / 86400) + " d"
}

// "Public/MNndQX7Z…" → "MNndQX…rc2LhU"
function shortId(id) {
    var s = String(id || "").replace(/^(Public|Private)\//, "")
    return s.length > 14 ? s.slice(0, 6) + "…" + s.slice(-6) : s
}

function isPrivate(id) { return String(id || "").indexOf("Private/") === 0 }
