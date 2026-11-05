// Bosphorus in Basecamp: lend and borrow on LEZ, publicly or from private accounts, with a wallet
// of its own. Everything goes through the core module (`bosphorus`) as tickets:
// start(method, params) → poll take(ticket). This file holds the bridge, the shared state and the
// navigation; the screens are in pages/ and follow the D1 "Tide" handoff
// (docs/design/d1-tide/HANDOFF.md): the water is your debt against what you can borrow, private
// things live below the surface, proving is a dive.
//
// Amounts stay base-unit strings until they are shown (kit/units.js); every string this view did
// not author is drawn with Text.PlainText.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Logos.Theme
import Logos.Controls
import "kit" as Kit
import "kit/units.js" as Units
import "kit/tide.js" as Tide
import "kit/tour.js" as Tour
import "pages" as Pages

Item {
    id: root
    objectName: "bosphorusRoot"

    readonly property var backend: typeof logos !== "undefined" && logos.module ? logos.module("bosphorus_ui") : null
    property bool backendReady: false

    // "loading" → "welcome" (no wallet: start | phrase | restore, then arriving or ready) → "main".
    property string screen: "loading"
    property string welcomeStep: "start"
    property string phrase: ""
    // With a faucet, a wallet is made as Welcome opens (unconfirmed until its phrase is
    // acknowledged), so its starter claim proves while the user reads. `premade`: it exists;
    // `premaking`: being made; `wantPhrase`: "Create a wallet" was pressed before it was ready.
    property bool premade: false
    property bool premaking: false
    property bool wantPhrase: false
    property bool busy: false
    property string openError: ""
    // A restored wallet looks through the chain for its money once it opens.
    property bool restoring: false
    property bool restoreSyncing: false

    // From the core's settings.json (status.settings); the defaults are the LEZ testnet and the
    // programs deployed there (DEPLOYMENTS.md).
    property string sequencerUrl: "https://testnet.lez.logos.co"
    property string lendingId: "Public/8mnAx1h5Utbi1XCMUaHdACn6qM87Sek9NgKvdy2ewRKF"
    property string faucetId: ""
    property var scanFrom: null
    property var tokenNames: ({})
    property var marketIds: []
    property var prefill: ({})

    // Chain and wallet state, replaced (never mutated) so bindings see every change.
    property var info: null
    property var prover: null
    property var accounts: []
    property var pool: null            // `pool`: the screened pool and this wallet's lots; null without one
    property var faucet: null          // `faucet`: its drops and when each can next be claimed; null without one
    property var uiState: ({})         // `ui_state`: { tourDone, hidden }
    property var claiming: ({})        // "gas" or a token id → a claim on its way
    property bool showSellLine: false  // the tour's "Watch the water" step
    property var markets: []
    property var marketDetails: ({})   // market id → `market`
    property var positions: ({})       // "market|account" → `positions`
    property var balances: ({})        // "market|account" → `balances`
    property var jobs: []
    property bool refreshing: false
    // The network as last seen: reachable (info answered with a block), and since when the block
    // height last moved (a stalled chain makes no blocks while answering fine).
    property bool online: true
    property string networkProblem: ""
    property real blockMovedAt: Date.now()
    // When the network last answered, and when private balances were last caught up (0: not
    // this session); `syncing` while a catch-up runs.
    property real lastOnlineAt: 0
    property real lastSyncAt: 0
    property bool syncing: false
    property real now: Date.now()
    Timer { interval: 5000; repeat: true; running: true; onTriggered: root.now = Date.now() }
    readonly property bool stalled: root.online && root.info !== null && root.now - root.blockMovedAt > 120000
    readonly property bool gpu: !!(root.prover && root.prover.gpu)
    // How long a proof takes here, in words.
    readonly property string diveTime: root.gpu ? qsTr("a minute or two") : qsTr("about 10 minutes")

    // Navigation: Wallet, Markets, Position, Activity.
    property int tab: 0
    // Dev: a tab change re-reads settings.json, so a test can change the prefill between steps.
    onTabChanged: {
        if (root.screen === "main") reloadSettings()
        // A finished guide ends on the Wallet, where the bar takes over.
        if (root.tab === 0 && root.coachGoal !== "" && root.checklist[root.coachGoal === "borrow" ? 1 : 2].done) root.coachGoal = ""
    }
    property string openMarket: ""     // the market Markets shows and Position acts in
    property string actAs: ""          // the account Position acts as: always a private one
    property int slot: 0               // its position index (private accounts)
    property string presetOp: ""       // an op Markets' "Save here" / "Borrow here" asks for
    property string selectedJob: ""
    // The dive on screen: { job } for a market op, or { title, seen, started } for a transfer
    // being proven (it isn't a job). null: none.
    property var dive: null

    readonly property string networkName: {
        var u = String(root.sequencerUrl)
        if (u.indexOf("127.0.0.1") >= 0 || u.indexOf("localhost") >= 0) return qsTr("Localnet")
        if (u.indexOf("testnet") >= 0) return qsTr("Testnet")
        return u.replace(/^https?:\/\//, "")
    }

    // ---- the bridge ---------------------------------------------------------------------

    property var pending: ({})

    function watch(reply, ok, fail) {
        if (typeof logos === "undefined" || !logos.watch) { fail("The Logos bridge is not available"); return }
        logos.watch(reply, function(v) { ok(v) }, function(e) { fail(String(e || "backend error")) })
    }

    // Runs one app-API request; `done(result)` on success, else `failed(error)` if given, else a
    // toast.
    function call(method, params, done, failed) {
        watch(backend.start(method, JSON.stringify(params || {})), function(raw) {
            var r = JSON.parse(String(raw))
            if (r.error) { report(r.error, failed); return }
            var p = root.pending
            p[r.ticket] = { done: done, failed: failed }
            root.pending = p
            ticketTimer.start()
        }, function(e) { report({ code: "bridge", message: e }, failed) })
    }

    function report(error, failed) {
        if (failed) failed(error)
        else toast.showError(error)
    }

    // An app-API error in words a user can act on.
    function explain(error) {
        if (!error) return ""
        var m = String(error.message || "")
        switch (error.code) {
        case "stale_price": return qsTr("The price feed is too old for this. Borrowing and collateral withdrawals wait for a fresh price.")
        case "payer_cannot_fund": return qsTr("This account can't pay the network fee. Add some LEZ to it first.")
        case "wallet_in_use": return qsTr("Another Bosphorus has this wallet open. Close it, then try again.")
        case "bridge": return qsTr("Basecamp's module bridge isn't answering: ") + m
        case "network": return qsTr("Can't reach the network. Check your connection; nothing was sent.")
        case "rejected": return qsTr("The network refused it, so nothing moved.") + (m ? " (" + m.replace(/^the network refused the transaction: /, "") + ")" : "")
        case "insufficient_balance": return qsTr("This account doesn't hold that much.") + (m.indexOf("last sync") >= 0 ? " " + qsTr("Private balances are as of your last Update.") : "")
        case "unconfirmed": return qsTr("It was sent, but the network hasn't confirmed it yet; it may still arrive. Update your wallet in a few minutes and check before trying again.")
        case "not_included": return qsTr("The network didn't take it, so nothing moved. Update your wallet to be sure before trying again.")
        case "stopping": return qsTr("Bosphorus is closing.")
        case "not_open": return qsTr("The wallet isn't open yet.")
        case "lost": return qsTr("Bosphorus lost track of that request. Check Activity and your balances before trying again.")
        case "not_approved": return qsTr("The pool hasn't approved enough of this yet. Wait for its screening, or take it back.")
        case "no_pool": return qsTr("No screened pool is set up for this network.")
        case "faucet_cooldown": return qsTr("You've had today's share of this. Come back tomorrow.")
        case "no_faucet": return qsTr("No faucet is set up for this network.")
        }
        return m.length > 0 ? m : String(error.code || qsTr("Something went wrong"))
    }

    // A sent job, in words: normally a few seconds; or its reply was lost; or it's most likely
    // dropped but could still land until its offer ends (the app's note says which).
    function sentText(j) {
        var note = String(j.note || "")
        var until = j.expect && j.expect.valid_until ? new Date(j.expect.valid_until + 60000).toLocaleTimeString(Qt.locale("en_GB"), "HH:mm") : ""
        if (note.indexOf("most likely dropped") >= 0)
            return until ? qsTr("Not on the chain after several blocks, so the network most likely dropped it. It can’t land after %1; Bosphorus watches until then. Don’t do it again before.").arg(until)
                         : qsTr("Not on the chain after several blocks, so the network most likely dropped it. Bosphorus keeps watching for a while; don’t do it again meanwhile.")
        if (note.indexOf("answer was lost") >= 0)
            return qsTr("Sent, but the network’s reply got lost. Bosphorus is looking for it on the chain; don’t do it again meanwhile.")
        return qsTr("Sent to the network. Usually confirmed within a few seconds.")
    }

    // Background refreshes fail quietly while the status pill already says the network is away.
    function quietly(e) { if (e && e.code !== "network") toast.showError(e) }

    Timer {
        id: ticketTimer
        interval: 400
        repeat: true
        onTriggered: {
            var tickets = Object.keys(root.pending)
            if (tickets.length === 0) { stop(); return }
            tickets.forEach(function(t) {
                root.watch(root.backend.take(t), function(raw) {
                    var r = JSON.parse(String(raw))
                    if (r.state === "pending") return
                    var entry = root.pending[t]
                    var p = root.pending
                    delete p[t]
                    root.pending = p
                    if (!entry) return
                    var response = r.response || {}
                    if (r.state !== "done") root.report({ code: "lost", message: qsTr("The request was lost") }, entry.failed)
                    else if (response.error) root.report(response.error, entry.failed)
                    else if (entry.done) entry.done(response.result)
                }, function(e) {})
            })
        }
    }

    // Job events: refresh the jobs, and everything else when one ends.
    Timer {
        interval: 1000
        repeat: true
        running: root.screen === "main"
        onTriggered: root.watch(root.backend.events(), function(raw) {
            var r = JSON.parse(String(raw))
            if (r.events.length > 0 || r.lagged > 0) {
                root.refreshJobs()
                if (r.events.some(function(e) { return e.job && root.isTerminal(e.job.state) }))
                    root.refreshAll()
            }
        }, function(e) {})
    }

    // Block height and rates drift: a light refresh twice a minute; the block height every 5 s
    // while the network is away or a sync runs, to see it come back (or the sync move).
    Timer {
        interval: 30000
        repeat: true
        running: root.screen === "main"
        onTriggered: {
            root.refreshInfo(autoSync)
            if (!root.online) return
            root.refreshMarkets(false)
            // A pending lot turns approved when the ASP signs, with nothing here to notice it.
            if (root.pool && root.pool.lots.some(function(l) { return l.state === "pending" })) root.refreshPool()
        }
    }
    // Private balances fall behind as blocks pass (only a sync reads them): catch up in the
    // background once more than 30 blocks behind, while no job needs the prover.
    function autoSync() {
        if (root.weather !== "behind" || root.syncing || root.runningJobs.length > 0) return
        root.syncing = true
        call("sync", {}, function() { root.syncing = false; root.lastSyncAt = Date.now(); refreshAll() },
             function(e) { root.syncing = false; quietly(e) })
    }
    Timer {
        interval: 5000
        repeat: true
        running: root.screen === "main" && (!root.online || root.stalled || (root.info !== null && !!root.info.syncing))
        onTriggered: {
            var was = root.online
            root.refreshInfo(function() { if (!was && root.online) root.refreshAll() })
        }
    }

    Timer {
        interval: 250
        running: true
        repeat: true
        onTriggered: {
            if (root.backend !== null && typeof logos !== "undefined" && logos.isViewModuleReady("bosphorus_ui")) {
                root.backendReady = true
                stop()
                root.bootstrap()
            }
        }
    }

    // ---- flows --------------------------------------------------------------------------

    function applySettings(s) {
        if (!s) return
        if (s.sequencer) root.sequencerUrl = s.sequencer
        if (s.lending) root.lendingId = s.lending
        if (s.faucet) root.faucetId = s.faucet
        if (s.scan_from !== undefined) root.scanFrom = s.scan_from
        if (s.tokens) root.tokenNames = s.tokens
        if (s.markets) root.marketIds = s.markets
        root.prefill = s.prefill || {}
    }

    function bootstrap() {
        watch(backend.status(), function(raw) {
            var s = JSON.parse(String(raw))
            applySettings(s.settings)
            // Made in the background but its phrase never acknowledged: nobody could recover it.
            if (s.wallet_unconfirmed) call("discard_wallet", {}, discarded, function(e) { if (e.code === "confirmed") openWallet(false); else { root.openError = explain(e); root.screen = "welcome" } })
            else if (s.open) { root.prover = s.open_result ? s.open_result.prover : null; enterMain() }
            else if (s.wallet_exists) openWallet(false)
            else showWelcome()
        }, function(e) { root.openError = e; root.screen = "welcome" })
    }

    // Re-reads settings.json (token names, dev prefill) without reopening.
    function reloadSettings() {
        watch(backend.status(), function(raw) { applySettings(JSON.parse(String(raw)).settings) }, function(e) {})
    }

    function showWelcome() {
        root.screen = "welcome"
        root.welcomeStep = "start"
        premakeWallet()
    }
    // After a discard has finished (its files are gone), a new background wallet can be made.
    function discarded() { root.premade = false; root.accounts = []; root.starterQueue = []; showWelcome() }

    // The background wallet (see `premade`): made, opened, and its starter claim started.
    function premakeWallet() {
        if (root.faucetId === "" || root.premade || root.premaking) return
        root.premaking = true
        call("create_wallet", { sequencer: root.sequencerUrl }, function(result) {
            root.premaking = false
            root.premade = true
            root.phrase = result.mnemonic
            if (root.wantPhrase) { root.wantPhrase = false; root.busy = false; root.welcomeStep = "phrase" }
            openForStarter()
        }, function(e) { root.premaking = false; if (root.wantPhrase) { root.wantPhrase = false; root.busy = false; root.openError = explain(e) } })
    }
    // "Create a wallet": the background one if there is one (or will be), else a new one.
    function newWallet() {
        root.openError = ""
        if (root.premade) { root.welcomeStep = "phrase"; return }
        if (root.premaking) { root.wantPhrase = true; root.busy = true; return }
        createWallet("", "")
    }
    // "Restore": the background wallet goes first (it was never confirmed).
    function restoreWallet(words, syncFrom) {
        if (!root.premade) { createWallet(words, syncFrom); return }
        root.busy = true
        call("discard_wallet", {}, function() {
            root.premade = false; root.phrase = ""; root.accounts = []; root.starterQueue = []
            createWallet(words, syncFrom)
        }, function(e) { root.busy = false; root.openError = explain(e) })
    }
    // The phrase was shown and acknowledged: the wallet is kept for good. While its starter is on
    // its way, the arriving screen explains what comes next.
    function confirmWallet() {
        call("confirm_wallet", {}, function() {
            root.premade = false
            root.phrase = ""
            if (root.starterRunning) root.welcomeStep = "arriving"
            else if (root.accounts.length > 0) enterMain()
            else openWallet(true)
        }, function(e) { root.openError = explain(e) })
    }

    function createWallet(mnemonic, syncFrom) {
        var p = { sequencer: root.sequencerUrl }
        if (mnemonic) { p.mnemonic = mnemonic; if (syncFrom) p.sync_from = parseInt(syncFrom) }
        root.busy = true
        root.openError = ""
        root.restoring = !!mnemonic
        call("create_wallet", p, function(result) {
            root.busy = false
            if (result.mnemonic) { root.phrase = result.mnemonic; root.welcomeStep = "phrase"; openForStarter() }
            else openWallet(true)
        }, function(e) { root.busy = false; root.openError = explain(e) })
    }

    // A new wallet opens before its phrase is on screen, so the starter's proof runs meanwhile.
    function openForStarter() {
        if (root.faucetId === "") return
        call("open", openParams(), function(result) {
            root.prover = result.prover
            call("accounts", {}, function(r) { root.accounts = r; refreshMarkets(false); refreshFaucet(); startStarter() }, function() {})
        }, function() {})
    }
    function openParams() {
        var p = { lending: root.lendingId }
        if (root.faucetId !== "") p.faucet = root.faucetId
        if (root.scanFrom !== null) p.scan_from = root.scanFrom
        return p
    }

    // `fresh`: a wallet just made or restored, which lands on "You're afloat" first.
    function openWallet(fresh) {
        root.busy = true
        root.openError = ""
        if (!fresh) root.screen = "loading"
        call("open", openParams(), function(result) {
            root.busy = false
            root.prover = result.prover
            if (fresh) {
                call("accounts", {}, function(r) { root.accounts = r; root.welcomeStep = "ready" }, function() { enterMain() })
                if (root.restoring) restoreSync()
            } else enterMain()
        }, function(e) { root.busy = false; root.openError = explain(e); root.screen = "welcome" })
    }

    // Restored: scan the chain for the wallet's private notes (progress in info.syncing).
    function restoreSync() {
        root.restoreSyncing = true
        call("sync", {}, function() { root.restoreSyncing = false; root.restoring = false; root.lastSyncAt = Date.now(); if (root.screen === "main") refreshAll() },
             function(e) { root.restoreSyncing = false; toast.showError(e) })
    }
    Timer { interval: 3000; repeat: true; running: root.restoreSyncing; onTriggered: root.refreshInfo() }

    function enterMain() {
        root.screen = "main"
        refreshAll()
        refreshJobs()
        // The tour no longer starts by itself: the next-step bar offers it once the first steps
        // are done (doing beats reading about it).
        call("ui_state", {}, function(r) { root.uiState = r }, function() {})
    }

    function refreshInfo(then) {
        call("info", {}, function(r) {
            if (!root.info || r.last_block !== root.info.last_block) root.blockMovedAt = Date.now()
            // Unreachable: keep the last known block, so balances "as of block N" still make sense.
            if (r.last_block === null && root.info) { r.last_block = root.info.last_block }
            root.online = r.network_problem === null || r.network_problem === undefined
            if (root.online) root.lastOnlineAt = Date.now()
            root.networkProblem = root.online ? "" : String(r.network_problem)
            root.info = r
            if (then) then()
        }, function(e) { if (e.code === "network") root.online = false; if (then) then() })
    }
    // Catches private balances up with the chain (Wallet's Refresh, the status popover).
    function syncWallet() {
        if (root.syncing) return
        root.syncing = true
        call("sync", {}, function() { root.syncing = false; root.lastSyncAt = Date.now(); refreshAll(); refreshJobs() },
             function(e) { root.syncing = false; root.report(e) })
    }
    Timer { interval: 3000; repeat: true; running: root.syncing; onTriggered: root.refreshInfo() }

    function refreshJobs() { call("jobs", {}, function(r) { root.jobs = r.slice().reverse() }, function() {}) }

    function refreshAll() {
        root.refreshing = true
        refreshInfo()
        call("accounts", {}, function(r) {
            root.accounts = r
            if (!Units.isPrivate(root.actAs)) root.actAs = root.privateId
            refreshMarkets(true)
            refreshPool()
            refreshFaucet()
        }, function(e) { root.refreshing = false; root.quietly(e) })
    }

    function refreshPool() {
        call("pool", {}, function(r) { root.pool = r }, function(e) { if (e.code === "no_pool") root.pool = null; else root.quietly(e) })
    }

    // ---- onboarding: the faucet, the checklist, the tour ----------------------------------

    readonly property string everydayId: { for (var i = 0; i < root.accounts.length; i++) if (!Units.isPrivate(root.accounts[i].account)) return root.accounts[i].account; return "" }
    readonly property string privateId: { for (var i = 0; i < root.accounts.length; i++) if (Units.isPrivate(root.accounts[i].account)) return root.accounts[i].account; return "" }

    function refreshFaucet() {
        if (root.faucetId === "") { root.faucet = null; return }
        var p = {}
        if (root.privateId !== "") p.claimer = root.privateId
        call("faucet", p, function(r) { root.faucet = r }, function(e) { if (e.code === "no_faucet") root.faucet = null; else root.quietly(e) })
    }

    function saveUi(patch) {
        var s = Object.assign({}, root.uiState, patch)
        root.uiState = s
        call("ui_state", { set: s }, function() {}, function() {})
    }

    // A token's id from its symbol (settings.json names them), "" if none.
    function tokenId(symbol) {
        for (var id in root.tokenNames) if (root.tokenNames[id] === symbol) return id
        return ""
    }
    // The faucet's drop of a token ("" for LEZ), or null.
    function dropOf(token) {
        if (!root.faucet) return null
        for (var i = 0; i < root.faucet.drops.length; i++) if ((root.faucet.drops[i].token || "") === token) return root.faucet.drops[i]
        return null
    }
    function nativeOf(id) {
        for (var i = 0; i < root.accounts.length; i++) if (root.accounts[i].account === id) return root.accounts[i].native
        return null
    }
    // Whether any account holds `symbol`, as a balance or as collateral.
    function holds(symbol) {
        var id = tokenId(symbol)
        if (id === "") return false
        for (var i = 0; i < root.accounts.length; i++) {
            var b = tokenBalance(root.accounts[i].account, id)
            if (b !== null && !Units.isZero(String(b))) return true
        }
        return root.openPositions.some(function(o) { return o.market.collateral_asset === id && !Units.isZero(o.position.collateral) })
    }
    // Whether the Private account holds `symbol` (as of the last sync).
    function privateHolds(symbol) {
        var b = root.privateId !== "" ? tokenBalance(root.privateId, tokenId(symbol)) : null
        return b !== null && !Units.isZero(String(b))
    }
    // A drop claimed within the cooldown (its next claim is still ahead).
    function claimedLately(token) {
        var d = dropOf(token)
        return !!d && d.next_ms !== null && d.next_ms > root.now
    }
    // "Next one at 14:05" while a drop waits for its cooldown, else "".
    function claimWait(symbol) {
        var d = dropOf(tokenId(symbol))
        if (!d || d.next_ms === null || d.next_ms <= root.now) return ""
        var at = new Date(d.next_ms), time = at.toLocaleTimeString(Qt.locale("en_GB"), "HH:mm")
        return at.toDateString() === new Date(root.now).toDateString() ? qsTr("Claimed. The next one is ready at %1.").arg(time)
                                                                       : qsTr("Claimed. The next one is ready tomorrow at %1.").arg(time)
    }
    // Private claims cost nothing: only the cooldown and one claim at a time stop a click.
    function canClaim(symbol) {
        var id = tokenId(symbol), d = dropOf(id)
        return root.online && root.privateId !== "" && !!d && !root.claiming[id] && (d.next_ms === null || d.next_ms <= root.now)
    }
    // The market lending USDC against `collSym`, "" if none: an unscreened one first (the guide's
    // faucet money is a plain private balance, which a screened market doesn't take).
    function marketFor(collSym) {
        var id = tokenId(collSym), screened = ""
        for (var i = 0; i < root.markets.length; i++) {
            var m = root.markets[i]
            if (m.collateral_asset !== id) continue
            if (!m.pool) return m.market
            if (screened === "") screened = m.market
        }
        return screened
    }

    // The collateral of the market the guide lends in: USDC · WETH, else any unscreened USDC market.
    function lendCollateral() {
        var usdc = tokenId("USDC"), other = ""
        for (var i = 0; i < root.markets.length; i++) {
            var m = root.markets[i], c = tokenSymbol(m.collateral_asset)
            if (m.loan_asset !== usdc || m.pool) continue
            if (c === "WETH") return c
            if (other === "") other = c
        }
        return other || "WETH"
    }

    // Start invisible: everything below the surface, no LEZ needed (private txs are fee-exempt).
    // The path: WBTC lands privately → borrow USDC against it → save USDC (claimed meanwhile).
    // A step stays done once done: paying a loan back doesn't un-borrow (a private job of that op
    // that finished counts too).
    readonly property var checklist: {
        var mine = root.openPositions.filter(function(o) { return Units.isPrivate(o.account) })
        var wbtc = tokenId("WBTC")
        var did = function(op) {
            return root.jobs.some(function(j) { var r = j.request || {}; return j.state === "done" && r.op === op && Units.isPrivate(r.owner || r.from || "") })
        }
        return [
            { key: "starter", done: root.privateHolds("WBTC") || root.claimedLately(wbtc) || did("supply_collateral") || mine.some(function(o) { return o.market.collateral_asset === wbtc && !Units.isZero(o.position.collateral) }) },
            { key: "borrow", done: did("borrow") || mine.some(function(o) { return !Units.isZero(o.position.debt || "0") }) },
            { key: "save", done: did("supply") || mine.some(function(o) { return !Units.isZero(o.position.supplied_assets || "0") }) }
        ]
    }
    readonly property int checklistDone: root.checklist.filter(function(s) { return s.done }).length
    readonly property bool onboarding: root.faucet !== null && (root.checklistDone < root.checklist.length || !root.uiState.tourDone)

    function setClaiming(key, on) { var c = Object.assign({}, root.claiming); if (on) c[key] = true; else delete c[key]; root.claiming = c }

    // The starter: WBTC, claimed by the private account into itself (one proof), from the moment
    // a new wallet exists; also the bar's button if it's missing. `starterStarted`: when, for the
    // arriving screen's clock.
    property var starterQueue: []
    property real starterStarted: 0
    property bool starterFailed: false     // the last starter claim didn't land
    readonly property bool starterRunning: root.starterQueue.length > 0
    function startStarter() {
        if (root.starterRunning) return
        var missing = ["WBTC"].filter(function(s) { return tokenId(s) !== "" && !root.privateHolds(s) && !root.claimedLately(tokenId(s)) })
        if (missing.length === 0) return
        root.starterQueue = missing
        root.starterStarted = Date.now()
        root.starterFailed = false
        nextStarter()
    }
    function nextStarter() {
        if (root.starterQueue.length === 0) return
        var symbol = root.starterQueue[0]
        claimToken(symbol, function(ok) {
            root.starterQueue = ok ? root.starterQueue.slice(1) : []
            if (!ok) root.starterFailed = true
            if (ok && root.starterQueue.length === 0 && root.screen === "main") notify(qsTr("Your WBTC arrived"), qsTr("In your Private account, out of sight"))
            nextStarter()
        })
    }

    // A token drop, claimed by the private account into itself: a private tx, so no fee and
    // nothing public says who got it. `then(ok)` after it lands or fails.
    function claimToken(symbol, then) {
        var id = tokenId(symbol)
        if (id === "" || root.claiming[id] || root.privateId === "") { if (then) then(false); return }
        setClaiming(id, true)
        var d = dropOf(id)
        var what = d ? Units.format(d.amount, tokenDecimals(id), 4) + " " + symbol : symbol
        call("faucet_claim", { claimer: root.privateId, to: root.privateId, token: id }, function(r) {
            setClaiming(id, false)
            if (!then) notify(qsTr("Got %1").arg(what), qsTr("In your Private account"))
            refreshAll()
            if (then) then(true)
        }, function(e) {
            setClaiming(id, false)
            if (e.code !== "faucet_cooldown") report(e)
            refreshAll()
            if (then) then(e.code === "faucet_cooldown")
        })
    }

    // ---- the guide: the first steps on the real screens ---------------------------------------

    // The next-step bar's "Show me" starts it: `coachGoal` "borrow" (all the private WBTC as
    // collateral, then a borrow at health ≈ `healthTarget`) or "save" (the faucet's USDC, in
    // `lendCollateral`'s market); "" when off. Its step follows from what's on screen (tab, market, account, op,
    // amount), so wandering off only changes the hint. The user makes every click; the card's
    // button only types an amount in.
    property string coachGoal: ""
    readonly property real healthTarget: 2.3
    // How far WBTC can fall before any is sold, at the suggested borrow.
    readonly property int fallPct: Math.round((1 - 1 / root.healthTarget) * 100)

    // The private position in a market (slot 0), or null.
    function privatePosition(mk) {
        for (var i = 0; i < root.openPositions.length; i++) {
            var o = root.openPositions[i]
            if (o.account === root.privateId && o.market.market === mk && !o.position.held) return o.position
        }
        return null
    }
    // How much USDC to suggest borrowing against the private WBTC (held or already collateral):
    // what keeps health near `healthTarget`, rounded down to 100 USDC. "0" without a price.
    function suggestedBorrow() {
        var mk = marketFor("WBTC"), m = marketById(mk), d = root.marketDetails[mk], pos = privatePosition(mk)
        var held = root.privateId !== "" ? tokenBalance(root.privateId, tokenId("WBTC")) : null
        if (!m || !d || !d.feed || d.feed.error || (held === null && !pos)) return "0"
        var coll = parseFloat(held || "0") + parseFloat(pos ? pos.collateral : "0")
        var cap = coll * parseFloat(d.feed.price) / 1e36 * parseFloat(m.lltv)
        var unit = Math.pow(10, m.loan_decimals || 0) * 100
        var amt = Math.floor((cap / root.healthTarget - parseFloat(pos && pos.debt ? pos.debt : "0")) / unit) * unit
        return amt > 0 ? amt.toFixed(0) : "0"
    }
    // The USDC to save: the day's drop, or what the private account holds if less.
    function suggestedSave() {
        var id = tokenId("USDC"), d = dropOf(id), b = root.privateId !== "" ? tokenBalance(root.privateId, id) : null
        if (!d || b === null) return "0"
        return Units.cmp(String(b), String(d.amount)) < 0 ? String(b) : String(d.amount)
    }
    // Whether the private account holds a full USDC drop (what was borrowed is for spending).
    readonly property bool hasSaveUsdc: {
        var id = tokenId("USDC"), d = dropOf(id), b = root.privateId !== "" ? tokenBalance(root.privateId, id) : null
        return !!d && b !== null && Units.cmp(String(b), String(d.amount)) >= 0
    }
    // Once borrowed, the USDC to save is claimed in the background (one proof).
    readonly property bool wantsSaveUsdc: root.checklist[1].done && !root.checklist[2].done && !root.hasSaveUsdc
    onWantsSaveUsdcChanged: claimForSave()
    function claimForSave() {
        if (!root.wantsSaveUsdc || !canClaim("USDC")) return
        claimToken("USDC", function(ok) { if (ok) notify(qsTr("USDC to lend arrived"), qsTr("In your Private account")) })
    }

    function startCoach(goal) { root.coachGoal = goal; showCoach() }

    function amountOf(raw, symbol) { return Units.format(raw, tokenDecimals(tokenId(symbol)), symbol === "USDC" ? 0 : 4) + " " + symbol }

    readonly property var coachStep: {
        var goal = root.coachGoal
        if (goal === "" || root.screen !== "main") return null
        var borrow = goal === "borrow"
        var collSym = borrow ? "WBTC" : lendCollateral(), mk = marketFor(collSym), pair = "USDC · " + collSym
        var g = borrow ? qsTr("Borrowing privately") : qsTr("Lending privately")
        var step = function(title, body, targets, action, act) { return { goal: g, title: title, body: body, targets: targets, action: action || "", act: act || null } }
        if (root.checklist[borrow ? 1 : 2].done) {
            // Borrowed, with the USDC to save already in: on to saving, from here.
            if (borrow && root.hasSaveUsdc)
                return step(qsTr("Borrowed, out of sight"), qsTr("The USDC is in your Private account, and nobody can see who borrowed it. Some more USDC came in to lend: that's next."),
                            [], qsTr("Next: lend USDC"), function() { root.startCoach("save") })
            return step(borrow ? qsTr("Borrowed, out of sight") : qsTr("Lent, out of sight"),
                        borrow ? qsTr("The USDC is in your Private account, and nobody can see who borrowed it. Some more USDC to lend is on its way; the Wallet shows when it's in.")
                               : qsTr("Your USDC now earns what borrowers pay, and nobody can see it's yours."),
                        [{ name: "navWallet", label: qsTr("Back to Wallet"), side: "below" }],
                        qsTr("Back to Wallet"), function() { root.coachGoal = ""; root.tab = 0 })
        }
        if (root.tab !== 1 && root.tab !== 2)
            return step(qsTr("Open Markets"),
                        borrow ? qsTr("Each market lends one token against another. Borrowing USDC against WBTC happens in USDC · WBTC.")
                               : qsTr("Lenders put USDC in a market and earn what its borrowers pay. Yours goes to %1.").arg(pair),
                        [{ name: "navMarkets", label: qsTr("Click Markets"), side: "below" }])
        if (root.tab === 1) {
            if (marketsPage.selected !== mk)
                return step(qsTr("Pick %1").arg(pair), qsTr("Its details open below the list."),
                            [{ name: "marketRow-" + collSym, label: qsTr("Click this market"), side: "below" }])
            return borrow ? step(qsTr("Borrow from here"), qsTr("You can borrow up to 80% of what your WBTC is worth. Past that line, some WBTC is sold to pay back the loan."),
                                 [{ name: "marketBorrow", label: qsTr("Click Borrow USDC"), side: "below" }, { name: "borrowLimit", label: qsTr("The sell line"), side: "left" }])
                          : step(qsTr("Lend from here"), qsTr("What lenders earn moves with how much of the market is lent out."),
                                 [{ name: "marketSave", label: qsTr("Click Lend USDC"), side: "below" }, { name: "marketRates", label: qsTr("What lenders earn"), side: "above" }])
        }
        var P = positionPage
        if (P.working) return null
        if (P.marketId !== mk)
            return step(qsTr("Switch to %1").arg(pair), qsTr("Position acts in one market at a time."),
                        P.marketMenuOpen ? [{ name: "positionMarket-" + collSym, label: qsTr("Click %1").arg(pair), side: "right" }]
                                         : [{ name: "positionMarket", label: qsTr("Click to change market"), side: "below" }])
        if (P.account !== root.privateId)
            return step(qsTr("Act as Private"), borrow ? qsTr("Your WBTC is in your Private account. Acting from it, nobody can see who borrowed: your computer proves it instead.")
                                                       : qsTr("Your USDC is in your Private account. Lent from there, the market sees new lending, not whose."),
                        [{ name: "who-private", label: qsTr("Click Private"), side: "below" }])
        var want = borrow ? (Units.isZero(P.coll) ? "supply_collateral" : "borrow") : "supply"
        if (P.sheet && P.op.op !== want)
            return step(qsTr("Not this one"), qsTr("Close this panel to pick another action."), [{ name: "positionClose", label: qsTr("Click to close"), side: "below" }])
        if (!P.sheet)
            return want === "supply_collateral" ? step(qsTr("First, put up your WBTC"), qsTr("A loan needs collateral. The WBTC stays yours; the market holds it while you owe."),
                                                       [{ name: "op-supply_collateral", label: qsTr("Click Add WBTC"), side: "above" }])
                 : want === "borrow" ? step(qsTr("Now the loan"), qsTr("Your WBTC is in. Now borrow USDC against it."),
                                            [{ name: "op-borrow", label: qsTr("Click Borrow USDC"), side: "above" }])
                 : step(qsTr("Lend"), qsTr("Your USDC goes to borrowers, who pay you for it."), [{ name: "op-supply", label: qsTr("Click Lend USDC"), side: "above" }])
        if (!P.useAll && Units.isZero(P.amount)) {
            var raw = want === "supply_collateral" ? String(tokenBalance(root.privateId, tokenId("WBTC")) || "0") : want === "borrow" ? suggestedBorrow() : suggestedSave()
            var sym = want === "supply_collateral" ? "WBTC" : "USDC", has = !Units.isZero(raw)
            var use = has ? qsTr("Use %1").arg(amountOf(raw, sym)) : "", fill = function() { positionPage.fill(raw) }
            var box = [{ name: "positionAmountBox", label: qsTr("How much"), side: "right" }]
            if (want === "supply_collateral") return step(qsTr("How much WBTC"), qsTr("All of it. More collateral means more room before any is sold."), box, use, fill)
            if (want === "borrow") return step(qsTr("How much USDC"), has ? qsTr("We suggest %1: WBTC could fall about %2% before any is sold.").arg(amountOf(raw, "USDC")).arg(root.fallPct)
                                                                         : qsTr("Waiting for the WBTC price…"), box, use, fill)
            return step(qsTr("How much to lend"), qsTr("The faucet's USDC. What you borrowed stays yours to spend."), box, use, fill)
        }
        var go = [{ name: "positionGo", label: qsTr("Click to dive"), side: "above" }]
        if (want === "supply_collateral") return step(qsTr("Dive"), qsTr("Your computer proves it, %1. The network sees WBTC come in, not whose.").arg(root.diveTime), go)
        if (want === "borrow") return step(qsTr("Watch the water, then dive"), qsTr("The dashed line shows where your debt would take the water; the top line is where some WBTC would be sold. Proving takes %1.").arg(root.diveTime), go)
        return step(qsTr("Dive"), qsTr("Your computer proves it, %1. The market sees new lending, not whose.").arg(root.diveTime), go)
    }
    // On screen unless a dive or the tour is.
    function showCoach() {
        var s = root.coachStep
        if (s && root.dive === null && !tour.visible) coach.show(s)
        else coach.visible = false
    }
    onCoachStepChanged: showCoach()
    onDiveChanged: showCoach()

    function startTour() {
        root.coachGoal = ""
        tour.steps = Tour.steps
        tour.start()
    }

    // Markets and their details; with `portfolio`, every account's positions and balances too.
    function refreshMarkets(portfolio) {
        call("markets", {}, function(list) {
            // With token names set, only markets whose two tokens are both named are shown.
            if (Object.keys(root.tokenNames).length > 0)
                list = list.filter(function(m) { return !!root.tokenNames[m.loan_asset] && !!root.tokenNames[m.collateral_asset] })
            // With a market list set, only those (replaced markets stay on chain).
            if (root.marketIds.length > 0)
                list = list.filter(function(m) { return root.marketIds.indexOf(m.market) >= 0 })
            root.markets = list
            root.refreshing = false
            list.forEach(function(m) {
                call("market", { market: m.market }, function(d) {
                    var all = Object.assign({}, root.marketDetails)
                    all[m.market] = d
                    root.marketDetails = all
                }, function() {})
                if (portfolio) root.accounts.forEach(function(a) { refreshPosition(m.market, a.account) })
            })
        }, function(e) { root.refreshing = false; if (e.code === "network") root.online = false; root.quietly(e) })
    }

    function refreshPosition(market, account) {
        var key = market + "|" + account
        call("positions", { market: market, owner: account }, function(r) {
            var all = Object.assign({}, root.positions); all[key] = r; root.positions = all
        }, function() {})
        call("balances", { market: market, account: account }, function(r) {
            var all = Object.assign({}, root.balances); all[key] = r; root.balances = all
        }, function() {})
    }

    // ---- lookups ------------------------------------------------------------------------

    function isTerminal(state) { return ["done", "failed", "interrupted", "unknown"].indexOf(state) >= 0 }

    function marketById(id) {
        for (var i = 0; i < root.markets.length; i++) if (root.markets[i].market === id) return root.markets[i]
        return null
    }

    function tokenSymbol(id) {
        if (!id) return ""
        return root.tokenNames[id] || Units.shortId(id).slice(0, 6)
    }

    function marketName(m) { return m ? tokenSymbol(m.loan_asset) + " / " + tokenSymbol(m.collateral_asset) : "" }

    // Tokens seen in any market, with their decimals.
    readonly property var tokens: {
        var seen = {}, out = []
        root.markets.forEach(function(m) {
            [[m.loan_asset, m.loan_decimals], [m.collateral_asset, m.collateral_decimals]].forEach(function(t) {
                if (!seen[t[0]]) { seen[t[0]] = true; out.push({ id: t[0], decimals: t[1] || 0, symbol: root.tokenSymbol(t[0]) }) }
            })
        })
        return out
    }

    function tokenDecimals(id) {
        for (var i = 0; i < root.tokens.length; i++) if (root.tokens[i].id === id) return root.tokens[i].decimals
        return 0
    }

    // An account's balance of a token (base units), from any market that has it; null if unknown.
    function tokenBalance(account, token) {
        for (var i = 0; i < root.markets.length; i++) {
            var b = root.balances[root.markets[i].market + "|" + account]
            if (!b) continue
            if (b.loan_asset === token) return b.loan
            if (b.collateral_asset === token) return b.collateral
        }
        return null
    }

    // What can pay `token` into `market` from `account`: the token balance, or for a screened market
    // the screened lots (approved and market lots). null: unknown.
    function payable(market, account, token) {
        var b = root.balances[market + "|" + account]
        if (!b || !b.screened) return tokenBalance(account, token)
        if (b.loan_asset === token) return b.loan_screened
        if (b.collateral_asset === token) return b.collateral_screened
        return null
    }

    function accountPositions(market, account) { return root.positions[market + "|" + account] || [] }

    function hasAnything(p) { return !Units.isZero(p.supply_shares) || !Units.isZero(p.collateral) || !Units.isZero(p.borrow_shares) }

    // Positions worth showing: anything with shares, collateral or debt.
    readonly property var openPositions: {
        var out = []
        root.markets.forEach(function(m) {
            root.accounts.forEach(function(a) {
                (root.positions[m.market + "|" + a.account] || []).forEach(function(p) {
                    if (root.hasAnything(p)) out.push({ market: m, account: a.account, position: p })
                })
            })
        })
        return out
    }

    // Markets → Position: act in `marketId`, optionally as `account`, starting on `op`.
    function openPosition(marketId, account, op) {
        if (marketId) root.openMarket = marketId
        if (account && Units.isPrivate(account)) root.actAs = account
        root.presetOp = op || ""
        root.tab = 2
    }

    // "Everyday · Main", "Private · 2": accounts as people read them (labels win).
    function accountName(id) {
        var priv = Units.isPrivate(id), n = 0
        for (var i = 0; i < root.accounts.length; i++) {
            var a = root.accounts[i]
            if (Units.isPrivate(a.account) !== priv) continue
            n++
            if (a.account === id) {
                var label = a.labels && a.labels.length > 0 ? a.labels[0] : (n === 1 ? qsTr("Main") : String(n))
                return (priv ? qsTr("Private") : qsTr("Everyday")) + " · " + label
            }
        }
        return (priv ? qsTr("Private") : qsTr("Everyday")) + " · " + Units.shortId(id)
    }

    // The network's state, for the status pill and the seabed: "calm" | "behind" | "offline" |
    // "stalled" | "problem".
    readonly property string weather: {
        if (!root.online) return "offline"
        if (!root.info) return "calm"
        if (root.info.lending_problem || root.info.privacy_circuit_problem) return "problem"
        if (root.stalled) return "stalled"
        return root.info.last_block - root.info.synced_block > 30 ? "behind" : "calm"
    }
    readonly property string weatherText: ({
        calm: qsTr("Up to date"), behind: qsTr("Catching up"), offline: qsTr("No connection"),
        stalled: qsTr("Network stalled"), problem: qsTr("Rough water")
    })[root.weather]

    readonly property var runningJobs: root.jobs.filter(function(j) { return !root.isTerminal(j.state) })
    readonly property var divingJob: {
        for (var i = 0; i < root.jobs.length; i++) if (root.jobs[i].state === "proving") return root.jobs[i]
        return null
    }

    // `quiet`: the caller shows "Copied" itself.
    function copy(text, quiet) {
        clipboard.text = text
        clipboard.selectAll()
        clipboard.copy()
        if (!quiet) toast.showInfo(qsTr("Copied"), Units.shortId(text))
    }

    function notify(title, message) { toast.showInfo(title, message) }
    function openStatus() { statusPop.open() }
    function closeStatus() { statusPop.close() }

    TextEdit { id: clipboard; visible: false }


        // ---- layout -------------------------------------------------------------------------

    Rectangle { anchors.fill: parent; color: Tide.sky }

    // Opening an existing wallet.
    Item {
        anchors.fill: parent
        visible: root.screen === "loading"
        Kit.Water { level: 0.2 }
        Column {
            anchors.centerIn: parent
            spacing: 14
            LogosSpinner { anchors.horizontalCenter: parent.horizontalCenter; width: 40; height: 40; ringColor: "#FFFFFF" }
            LogosText {
                anchors.horizontalCenter: parent.horizontalCenter
                text: root.backendReady ? qsTr("Opening your wallet…") : qsTr("Connecting to Bosphorus…")
                font.pixelSize: 17; font.weight: Font.DemiBold
            }
            LogosText {
                anchors.horizontalCenter: parent.horizontalCenter
                visible: root.backendReady
                text: qsTr("The first start also gets the prover ready.")
                color: Tide.text2
            }
        }
    }

    Pages.WelcomePage { anchors.fill: parent; app: root; visible: root.screen === "welcome" }

    // The app: a header with the nav pill and the status pill, then the screen.
    Item {
        anchors.fill: parent
        visible: root.screen === "main"

        StackLayout {
            anchors.fill: parent
            currentIndex: root.tab
            Pages.WalletPage { app: root; topInset: header.height }
            Pages.MarketsPage { id: marketsPage; app: root; topInset: header.height }
            Pages.PositionPage { id: positionPage; app: root; topInset: header.height; active: root.tab === 2 }
            Pages.ActivityPage { app: root; topInset: header.height }
        }

        RowLayout {
            id: header
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.leftMargin: 28
            anchors.rightMargin: 28
            height: 70
            spacing: 16
            RowLayout {
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                spacing: 10
                Image { source: "../icons/bosphorus.png"; sourceSize: Qt.size(56, 56); Layout.preferredWidth: 28; Layout.preferredHeight: 28; smooth: true; mipmap: true }
                LogosText { text: "Bosphorus"; font.pixelSize: 17; font.weight: Font.ExtraBold; Layout.fillWidth: true }
            }
            Rectangle {
                objectName: "nav"
                implicitWidth: navRow.implicitWidth + 8
                implicitHeight: 42
                radius: 21
                color: Tide.glass
                Row {
                    id: navRow
                    anchors.centerIn: parent
                    spacing: 4
                    Repeater {
                        model: [qsTr("Wallet"), qsTr("Markets"), qsTr("Position"), qsTr("Activity")]
                        delegate: Rectangle {
                            objectName: ["navWallet", "navMarkets", "navPosition", "navActivity"][index]
                            readonly property bool on: root.tab === index
                            readonly property int badge: index === 3 ? root.runningJobs.length : 0
                            height: 34
                            width: navLabel.implicitWidth + 32 + (badge > 0 ? 26 : 0)
                            radius: 17
                            color: on ? "#FFFFFF" : "transparent"
                            Row {
                                anchors.centerIn: parent
                                spacing: 8
                                LogosText { id: navLabel; text: modelData; font.pixelSize: 14; font.weight: on ? Font.Bold : Font.DemiBold; color: on ? Tide.sky : Tide.text2; anchors.verticalCenter: parent.verticalCenter }
                                Rectangle {
                                    visible: badge > 0
                                    width: 18; height: 18; radius: 9
                                    color: Tide.warn
                                    anchors.verticalCenter: parent.verticalCenter
                                    LogosText { anchors.centerIn: parent; text: badge; font.pixelSize: 11; font.weight: Font.ExtraBold; color: Tide.sky }
                                }
                            }
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: root.tab = index }
                        }
                    }
                }
            }
            Item {
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                implicitHeight: 32
                Rectangle {
                    id: statusPill
                    objectName: "statusPill"
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    height: 32
                    width: statusRow.implicitWidth + 24
                    radius: 16
                    color: Tide.glass
                    Row {
                        id: statusRow
                        anchors.centerIn: parent
                        spacing: 8
                        Rectangle {
                            width: 8; height: 8; radius: 4
                            anchors.verticalCenter: parent.verticalCenter
                            color: root.weather === "calm" ? Tide.good : root.weather === "behind" ? Tide.info : root.weather === "stalled" ? Tide.warn : Tide.bad
                        }
                        LogosText {
                            text: root.weatherText
                            font.pixelSize: 13
                            color: "#E6FFFFFF"
                        }
                    }
                    MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: statusPop.opened ? statusPop.close() : statusPop.open() }
                    // What the pill means, and what to do about it.
                    Popup {
                        id: statusPop
                        y: parent.height + 10
                        x: parent.width - width
                        width: 300
                        padding: 16
                        background: Rectangle { color: "#13233D"; border.color: "#24FFFFFF"; radius: 16 }
                        contentItem: ColumnLayout {
                            spacing: 8
                            LogosText { text: root.weatherText; font.pixelSize: 15; font.weight: Font.ExtraBold }
                            LogosText {
                                Layout.fillWidth: true
                                wrapMode: Text.Wrap
                                font.pixelSize: 13
                                color: "#D9FFFFFF"
                                textFormat: Text.PlainText
                                text: ({
                                    calm: qsTr("Your balances are current."),
                                    behind: qsTr("The network has moved on since your private balances were last checked. Catch up to see them as they are now."),
                                    offline: root.lastOnlineAt > 0 ? qsTr("Bosphorus can't reach the network. You're seeing balances from %1 ago. Moving money is paused until you're back.").arg(Units.ago(root.now - root.lastOnlineAt))
                                                                   : qsTr("Bosphorus can't reach the network. Moving money is paused until you're back."),
                                    stalled: qsTr("The network answers but hasn't made a block for a while. Anything you send waits until it starts again."),
                                    problem: qsTr("Something is wrong with this network: ") + String((root.info && (root.info.lending_problem || root.info.privacy_circuit_problem)) || "")
                                })[root.weather]
                            }
                            Kit.BigButton {
                                visible: root.weather === "offline" || root.weather === "behind"
                                Layout.topMargin: 4
                                buttonHeight: 36
                                fontSize: 13.5
                                enabled: root.weather === "offline" || !root.syncing
                                text: root.weather === "offline" ? qsTr("Try again") : root.syncing ? qsTr("Catching up…") : qsTr("Catch up")
                                onClicked: {
                                    if (root.weather === "offline") root.refreshInfo(function() { if (root.online) root.refreshAll() })
                                    else root.syncWallet()
                                    statusPop.close()
                                }
                            }
                        }
                    }
                }
            }
        }

        Pages.DiveOverlay { anchors.fill: parent; app: root; visible: root.dive !== null; z: 5 }
    }

    Kit.TourOverlay {
        id: tour
        objectName: "tour"
        anchors.fill: parent
        z: 8
        visible: false
        scope: root
        onStepShown: function(step) {
            if (!step) return
            if (step.setup === "position") { root.openPosition(root.marketFor("WBTC") || (root.markets.length > 0 ? root.markets[0].market : ""), root.privateId, ""); positionPage.close() }
            root.showSellLine = step.setup === "position"
            root.tab = step.tab
        }
        onFinished: function(skipped) { root.showSellLine = false; root.saveUi({ tourDone: true }); root.tab = 0 }
    }

    Kit.TourOverlay {
        id: coach
        objectName: "coach"
        anchors.fill: parent
        z: 7
        visible: false
        coach: true
        scope: root
        bottomInset: toast.shown ? toast.height + 2 * Theme.spacing.xlarge : 0
        onFinished: function(skipped) { root.coachGoal = "" }
    }

    LogosToast {
        id: toast
        objectName: "toast"
        z: 10
        width: Math.min(parent.width - 2 * Theme.spacing.xxlarge, 560)
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Theme.spacing.xlarge
        shown: false
        closable: true

        function showError(e) {
            severity = LogosNotice.Error
            show(qsTr("That didn’t work"), root.explain(e))
        }
        function showInfo(title, message) {
            severity = LogosNotice.Success
            show(title, message)
        }
    }
}
