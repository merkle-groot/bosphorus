// Loads the real view (basecamp/bosphorus-ui/src/qml) against a fake core module, walks its
// screens and saves a screenshot of each to $PROBE_OUT (run.sh). No Basecamp, no clicks.
import QtQuick
import QtQuick.Window
import "fake.js" as Fake

Window {
    id: probe
    width: 1330
    visible: true
    color: "black"

    // PROBE_SCREEN: "main" (an open wallet, default), "setup" (no wallet), "phrase".
    readonly property string mode: Qt.application.arguments.indexOf("setup") >= 0 ? "setup" : "main"
    readonly property bool stale: Qt.application.arguments.indexOf("stale") >= 0
    // "onboard": a fresh wallet with a faucet: Get started, the claims, every tour step.
    readonly property bool onboard: Qt.application.arguments.indexOf("onboard") >= 0
    // "offline": the sequencer can't be reached. "short": Basecamp's window height (720).
    readonly property bool offline: Qt.application.arguments.indexOf("offline") >= 0
    height: Qt.application.arguments.indexOf("short") >= 0 ? 720 : 820
    readonly property string out: Qt.application.arguments[Qt.application.arguments.length - 1]

    property var logos: ({
        module: function(n) { return fake },
        isViewModuleReady: function(n) { return true },
        watch: function(reply, ok, fail) { ok(reply) }
    })

    QtObject {
        id: fake
        property int next: 1
        property var answers: ({})
        function status() {
            return JSON.stringify({ wallet_dir: "/tmp/w", wallet_exists: probe.mode === "main" && !probe.onboard, wallet_unconfirmed: false, open: probe.mode === "main" && !probe.onboard,
                                    open_result: { prover: { gpu: true } }, settings: Fake.settings(probe.onboard) })
        }
        function start(method, params) {
            var t = "t" + (next++)
            var r = Fake.answer(method, JSON.parse(params), { stale: probe.stale, offline: probe.offline, onboard: probe.onboard })
            var a = answers; a[t] = r && r.__error ? { id: null, error: r.__error } : { id: null, result: r }; answers = a
            return JSON.stringify({ ticket: t })
        }
        function take(t) {
            var r = answers[t]
            return JSON.stringify(r ? { state: "done", response: r } : { state: "unknown" })
        }
        function events() { return JSON.stringify({ events: [], lagged: 0 }) }
    }

    Loader {
        id: view
        anchors.fill: parent
        source: "../../bosphorus-ui/src/qml/BosphorusView.qml"
        onStatusChanged: if (status === Loader.Error) { console.log("PROBE: view failed to load"); Qt.exit(1) }
    }

    // Steps: [what to do, screenshot name].
    function tourStep(i) { return [function() { var t = probe.find(view.item, "tour"); if (i === 0) view.item.startTour(); else t.go(i) }, "tour-" + (i + 1)] }
    // Onboarding v2: Welcome (the wallet made and its WBTC claimed in the background), the
    // phrase, arriving, then the next-step bar through borrow and save, and the tour.
    property var onboardSteps: [
        [function() {}, "onboard-welcome"],
        [function() { view.item.newWallet() }, "onboard-phrase"],
        [function() { view.item.starterQueue = ["WBTC"]; view.item.starterStarted = Date.now() - 42000; view.item.welcomeStep = "arriving" }, "onboard-arriving"],
        [function() { view.item.starterQueue = [] }, "onboard-arrived"],
        [function() { view.item.enterMain() }, "onboard-bar-borrow"],
        [function() { view.item.startCoach("borrow") }, "coach-borrow-1-nav"],
        [function() { view.item.tab = 1 }, "coach-borrow-2-row"],
        [function() { view.item.openMarket = Fake.MARKET2 }, "coach-borrow-3-button"],
        [function() { view.item.openPosition(Fake.MARKET2, Fake.PUB, "borrow") }, "coach-borrow-4-private"],
        [function() { view.item.actAs = Fake.PRIV }, "coach-borrow-5-add"],
        [function() { probe.find(view.item, "positionPage").open(2) }, "coach-borrow-6-amount"],
        [function() { probe.find(view.item, "coach").step.act() }, "coach-borrow-7-go"],
        [function() { probe.find(view.item, "positionPage").submit() }, "coach-borrow-8-dive"],
        [function() { view.item.dive = null; view.item.refreshAll() }, "coach-borrow-9-op"],
        [function() { probe.find(view.item, "positionPage").open(4) }, "coach-borrow-10-amount"],
        [function() { probe.find(view.item, "coach").step.act() }, "coach-borrow-11-go"],
        [function() { probe.find(view.item, "positionPage").submit() }, "coach-borrow-12-dive"],
        [function() { view.item.dive = null; view.item.refreshAll() }, "coach-borrow-13-done"],
        [function() {}, "coach-borrow-14-next"],
        [function() { probe.find(view.item, "coach").step.act() }, "coach-save-1-switch"],
        [function() { view.item.tab = 0 }, "coach-save-1-nav"],
        [function() { view.item.tab = 1 }, "coach-save-2-row"],
        [function() { view.item.openMarket = Fake.MARKET }, "coach-save-3-button"],
        [function() { view.item.openPosition(Fake.MARKET, "", "supply") }, "coach-save-4-amount"],
        [function() { probe.find(view.item, "coach").step.act() }, "coach-save-5-go"],
        [function() { probe.find(view.item, "positionPage").submit() }, "coach-save-6-dive"],
        [function() { view.item.dive = null; view.item.refreshAll() }, "coach-save-7-done"],
        [function() { probe.find(view.item, "coach").step.act() }, "onboard-bar-done"],
        tourStep(0), tourStep(1), tourStep(2), tourStep(3), tourStep(4), tourStep(5), tourStep(6), tourStep(7), tourStep(8),
        [function() { probe.find(view.item, "tour").go(9) }, "onboard-after-tour"]
    ]
    property var steps: probe.onboard ? onboardSteps : probe.mode === "setup" ? [[function() {}, "welcome-start"],
                                                  [function() { view.item.createWallet("", "") }, "welcome-phrase"],
                                                  [function() { view.item.welcomeStep = "restore" }, "welcome-restore"],
                                                  [function() { view.item.accounts = Fake.answer("accounts", {}, {}); view.item.welcomeStep = "ready" }, "welcome-ready"]]
        : [
        [function() { view.item.tab = 0 }, "wallet"],
        [function() { var w = probe.find(view.item, "walletPage"); w.open("dive", "above", w.aboveRows[0]); probe.find(view.item, "moveAmount").text = "25000" }, "wallet-dive-sheet"],
        [function() { var w = probe.find(view.item, "walletPage"); w.open("send", "above", w.aboveRows[1]); probe.find(view.item, "moveAmount").text = "5" }, "wallet-send-over"],
        [function() { var w = probe.find(view.item, "walletPage"); w.openBelow("surface", w.belowRows[w.belowRows.length - 1]); probe.find(view.item, "moveAmount").text = "1000" }, "wallet-surface-sheet"],
        [function() { probe.closePopups() }, "wallet-screened"],
        [function() { var w = probe.find(view.item, "walletPage"); w.open("dive", "above", w.aboveLez); probe.find(view.item, "moveAmount").text = "1000000" }, "wallet-dive-screened"],
        [function() { var w = probe.find(view.item, "walletPage"); w.open("surface", "below", w.belowScreened[1], w.belowOf(w.belowScreened[1].token)); probe.find(view.item, "moveAmount").text = "2500" }, "wallet-surface-screened"],
        [function() { var w = probe.find(view.item, "walletPage"); w.open("takeback", "below", w.belowScreened[2]) }, "wallet-takeback"],
        [function() { var w = probe.find(view.item, "walletPage"); var a = w.belowScreened[1]; var p = Object.assign({}, a, { raw: "1500000000", screened: undefined }); w.open("send", "below", p, { plain: p, approved: a, pending: [] }) }, "wallet-send-source"],
        [function() { probe.closePopups(); view.item.lastSyncAt = Date.now() - 600000; view.item.info = Object.assign({}, view.item.info, { synced_block: 900 }) }, "wallet-behind"],
        [function() { view.item.openStatus() }, "wallet-status-popover"],
        [function() { probe.closePopups(); view.item.info = Object.assign({}, view.item.info, { synced_block: view.item.info.last_block }) }, "wallet-clean"],
        [function() { view.item.tab = 1 }, "markets"],
        [function() { view.item.openPosition(Fake.MARKET, Fake.PRIV, ""); probe.find(view.item, "positionPage").close() }, "position-overview"],
        [function() { probe.find(view.item, "positionMarket").clicked() }, "position-market-menu"],
        [function() { view.item.openPosition(Fake.MARKET, Fake.PRIV, "borrow") }, "position-private"],
        [function() { view.item.prefill = { amount: "2500" } }, "position-borrow-quote"],
        [function() { view.item.prefill = { amount: "90000" } }, "position-spill"],
        [function() { view.item.prefill = {}; view.item.openPosition(Fake.MARKET, Fake.PRIV, "repay"); probe.find(view.item, "positionPage").useAll = true }, "position-repay-all"],
        [function() { view.item.openPosition(Fake.MARKET, Fake.PUB, "supply"); view.item.prefill = { amount: "1000" } }, "position-public-save"],
        [function() { view.item.openPosition(Fake.MARKET, Fake.PUB, "supply"); view.item.prefill = { amount: "999999" } }, "position-short"],
        [function() { probe.land("supply", Fake.PUB, false) }, "position-landed-in"],
        [function() { probe.land("borrow", Fake.PRIV, true) }, "position-landed-out"],
        [function() { view.item.tab = 3 }, "activity"],
        [function() { view.item.selectedJob = "1793023410000-0000" }, "activity-interrupted"],
        [function() { view.item.selectedJob = "1793023420000-0000" }, "activity-dropped"],
        [function() { view.item.selectedJob = "1793023430000-0000" }, "activity-rejected"],
        [function() { view.item.dive = { job: "1793023400000-0000", title: "Pay back 1,000 USDC on Private · Main", seen: "Some position paid back 1,000 USDC." } }, "dive"],
        [function() { view.item.dive = { title: "Moving 5 LEZ to Private · Main", started: Date.now() - 30000, seen: "5 LEZ left your Everyday account. Not where it went.",
                                         error: { code: "unconfirmed", message: "tx … went out, but the network has made almost no blocks since" } } }, "dive-transfer-unsure"],
        [function() { view.item.dive = null; view.item.tab = 0 }, "wallet-after"],
        [function() { var ms = view.item.markets.slice(); ms[1] = Object.assign({}, ms[1], { pool: "Public/HyWkqC3kJ4N6ewpYoHgvBmKpfp9oWRrZBj25dg4G6HCb" }); view.item.markets = ms; view.item.openMarket = ms[1].market; view.item.tab = 1 }, "markets-screened"]
    ]
    property int step: 0
    // "frames": the Welcome intro, a frame every 200 ms for 5 s (setup mode), then exit.
    readonly property bool frames: Qt.application.arguments.indexOf("frames") >= 0
    Timer {
        interval: 200; repeat: true; running: probe.frames && view.status === Loader.Ready
        property int n: 0
        onTriggered: {
            var name = "intro-" + (n < 10 ? "0" : "") + n
            probe.contentItem.grabToImage(function(r) { r.saveToFile(probe.out + "/" + name + ".png") })
            if (++n > 35) Qt.exit(0)
        }
    }

    // The first item under `item` with this objectName (Popups' content included).
    function find(item, name) {
        if (!item) return null
        if (item.objectName === name) return item
        var kids = (item.children || []).concat(item.contentItem && item.contentItem !== item ? [item.contentItem] : [])
        for (var i = 0; i < kids.length; i++) { var f = find(kids[i], name); if (f) return f }
        if (item.data) for (var j = 0; j < item.data.length; j++) if (item.data[j] && item.data[j].contentItem) { var g = find(item.data[j].contentItem, name); if (g) return g }
        return null
    }
    // Fakes an op landing on the position on screen (no job): the effects play as after a real one.
    function land(op, account, priv) {
        view.item.openPosition(Fake.MARKET, account, op)
        var p = find(view.item, "positionPage")
        p.close()
        p.landed = { op: op, symbol: op === "supply_collateral" ? "WETH" : "USDC", loanSym: "USDC", loanDec: 6, collDec: 18, amountText: "1,000",
                     key: p.posKey, before: { saved: "0", coll: p.coll, debt: "0" }, account: account, to: "", isPrivate: priv, apr: "0.18" }
        p.play()
    }
    function closePopups() { var w = find(view.item, "walletPage"); if (w) w.closeSheet(); view.item.closeStatus() }

    Timer {
        interval: 1500
        running: view.status === Loader.Ready && !probe.frames
        repeat: true
        onTriggered: {
            if (probe.step >= probe.steps.length) { Qt.exit(0); return }
            var s = probe.steps[probe.step++]
            s[0]()
            var name = s[1]
            later.name = name
            later.restart()
        }
    }
    Timer {
        id: later
        property string name: ""
        interval: 1200
        onTriggered: probe.contentItem.grabToImage(function(r) { r.saveToFile(probe.out + "/" + name + ".png"); console.log("PROBE: saved " + name) })
    }
}
