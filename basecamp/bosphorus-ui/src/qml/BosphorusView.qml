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
import "pages" as Pages

Item {
    id: root
    objectName: "bosphorusRoot"

    readonly property var backend: typeof logos !== "undefined" && logos.module ? logos.module("bosphorus_ui") : null
    property bool backendReady: false

    // "loading" → "welcome" (no wallet: start | phrase | restore, then ready) → "main".
    property string screen: "loading"
    property string welcomeStep: "start"
    property string phrase: ""
    property bool busy: false
    property string openError: ""
    // A restored wallet looks through the chain for its money once it opens.
    property bool restoring: false
    property bool restoreSyncing: false

    // From the core's settings.json (status.settings); the defaults are the LEZ testnet and the
    // lending program deployed there (DEPLOYMENTS.md).
    property string sequencerUrl: "https://testnet.lez.logos.co"
    property string lendingId: "Public/8mnAx1h5Utbi1XCMUaHdACn6qM87Sek9NgKvdy2ewRKF"
    property var tokenNames: ({})
    property var prefill: ({})

    // Chain and wallet state, replaced (never mutated) so bindings see every change.
    property var info: null
    property var prover: null
    property var accounts: []
    property var pool: null            // `pool`: the screened pool and this wallet's lots; null without one
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
    readonly property string diveTime: root.gpu ? qsTr("about a minute") : qsTr("about 10 minutes")

    // Navigation: Wallet, Markets, Position, Activity.
    property int tab: 0
    // Dev: a tab change re-reads settings.json, so a test can change the prefill between steps.
    onTabChanged: if (root.screen === "main") reloadSettings()
    property string openMarket: ""     // the market Markets shows and Position acts in
    property string actAs: ""          // the account Position acts as
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
        onTriggered: { root.refreshInfo(); if (root.online) root.refreshMarkets(false) }
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
        root.tokenNames = s.tokens || {}
        root.prefill = s.prefill || {}
    }

    function bootstrap() {
        watch(backend.status(), function(raw) {
            var s = JSON.parse(String(raw))
            applySettings(s.settings)
            if (s.open) { root.prover = s.open_result ? s.open_result.prover : null; enterMain() }
            else if (s.wallet_exists) openWallet(false)
            else { root.screen = "welcome"; root.welcomeStep = "start" }
        }, function(e) { root.openError = e; root.screen = "welcome" })
    }

    // Re-reads settings.json (token names, dev prefill) without reopening.
    function reloadSettings() {
        watch(backend.status(), function(raw) { applySettings(JSON.parse(String(raw)).settings) }, function(e) {})
    }

    function createWallet(mnemonic, syncFrom) {
        var p = { sequencer: root.sequencerUrl }
        if (mnemonic) { p.mnemonic = mnemonic; if (syncFrom) p.sync_from = parseInt(syncFrom) }
        root.busy = true
        root.openError = ""
        root.restoring = !!mnemonic
        call("create_wallet", p, function(result) {
            root.busy = false
            if (result.mnemonic) { root.phrase = result.mnemonic; root.welcomeStep = "phrase" }
            else openWallet(true)
        }, function(e) { root.busy = false; root.openError = explain(e) })
    }

    // `fresh`: a wallet just made or restored, which lands on "You're afloat" first.
    function openWallet(fresh) {
        root.busy = true
        root.openError = ""
        if (!fresh) root.screen = "loading"
        call("open", { lending: root.lendingId }, function(result) {
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
            if (root.actAs === "" && r.length > 0) root.actAs = r[0].account
            refreshMarkets(true)
            refreshPool()
        }, function(e) { root.refreshing = false; root.quietly(e) })
    }

    function refreshPool() {
        call("pool", {}, function(r) { root.pool = r }, function(e) { if (e.code === "no_pool") root.pool = null; else root.quietly(e) })
    }

    // Markets and their details; with `portfolio`, every account's positions and balances too.
    function refreshMarkets(portfolio) {
        call("markets", {}, function(list) {
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
        if (account) root.actAs = account
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
            Pages.MarketsPage { app: root; topInset: header.height }
            Pages.PositionPage { app: root; topInset: header.height; active: root.tab === 2 }
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
