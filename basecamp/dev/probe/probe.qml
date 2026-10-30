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
            return JSON.stringify({ wallet_dir: "/tmp/w", wallet_exists: probe.mode === "main", open: probe.mode === "main",
                                    open_result: { prover: { gpu: true } }, settings: Fake.settings() })
        }
        function start(method, params) {
            var t = "t" + (next++)
            var r = Fake.answer(method, JSON.parse(params), { stale: probe.stale, offline: probe.offline })
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
    property var steps: probe.mode === "setup" ? [[function() {}, "welcome-start"],
                                                  [function() { view.item.createWallet("", "") }, "welcome-phrase"],
                                                  [function() { view.item.welcomeStep = "restore" }, "welcome-restore"],
                                                  [function() { view.item.accounts = Fake.answer("accounts", {}, {}); view.item.welcomeStep = "ready" }, "welcome-ready"]]
        : [
        [function() { view.item.tab = 0 }, "wallet"],
        [function() { var w = probe.find(view.item, "walletPage"); w.open("dive", "above", w.aboveRows[0]); probe.find(view.item, "moveAmount").text = "25000" }, "wallet-dive-sheet"],
        [function() { var w = probe.find(view.item, "walletPage"); w.open("send", "above", w.aboveRows[1]); probe.find(view.item, "moveAmount").text = "5" }, "wallet-send-over"],
        [function() { var w = probe.find(view.item, "walletPage"); w.open("surface", "below", w.belowRows[w.belowRows.length - 1]); probe.find(view.item, "moveAmount").text = "1000" }, "wallet-surface-sheet"],
        [function() { probe.closePopups() }, "wallet-screened"],
        [function() { var w = probe.find(view.item, "walletPage"); w.open("dive", "above", w.aboveRows[w.aboveRows.length - 1]); probe.find(view.item, "moveAmount").text = "1000000" }, "wallet-dive-screened"],
        [function() { var w = probe.find(view.item, "walletPage"); w.open("surface", "below", w.belowScreened[1]); probe.find(view.item, "moveAmount").text = "2500" }, "wallet-surface-screened"],
        [function() { var w = probe.find(view.item, "walletPage"); w.open("takeback", "below", w.belowScreened[2]) }, "wallet-takeback"],
        [function() { probe.closePopups(); view.item.lastSyncAt = Date.now() - 600000; view.item.info = Object.assign({}, view.item.info, { synced_block: 900 }) }, "wallet-behind"],
        [function() { view.item.openStatus() }, "wallet-status-popover"],
        [function() { probe.closePopups(); view.item.info = Object.assign({}, view.item.info, { synced_block: view.item.info.last_block }) }, "wallet-clean"],
        [function() { view.item.tab = 1 }, "markets"],
        [function() { view.item.openPosition(Fake.MARKET, Fake.PRIV, "borrow") }, "position-private"],
        [function() { view.item.prefill = { amount: "2500" } }, "position-borrow-quote"],
        [function() { view.item.prefill = { amount: "90000" } }, "position-spill"],
        [function() { view.item.openPosition(Fake.MARKET, Fake.PUB, "supply"); view.item.prefill = { amount: "1000" } }, "position-public-save"],
        [function() { view.item.openPosition(Fake.MARKET, Fake.PUB, "supply"); view.item.prefill = { amount: "999999" } }, "position-short"],
        [function() { probe.land("supply", Fake.PUB, false) }, "position-landed-in"],
        [function() { probe.land("borrow", Fake.PRIV, true) }, "position-landed-out"],
        [function() { view.item.tab = 3 }, "activity"],
        [function() { view.item.selectedJob = "1793023410000-0000" }, "activity-interrupted"],
        [function() { view.item.selectedJob = "1793023420000-0000" }, "activity-dropped"],
        [function() { view.item.selectedJob = "1793023430000-0000" }, "activity-rejected"],
        [function() { view.item.dive = { job: "1793023400000-0000", title: "Pay back 1,000 TKB on Private · Main", seen: "Some position paid back 1,000 TKB." } }, "dive"],
        [function() { view.item.dive = { title: "Moving 5 LEZ to Private · Main", started: Date.now() - 30000, seen: "5 LEZ left your Everyday account. Not where it went.",
                                         error: { code: "unconfirmed", message: "tx … went out, but the network has made almost no blocks since" } } }, "dive-transfer-unsure"],
        [function() { view.item.dive = null; view.item.tab = 0 }, "wallet-after"]
    ]
    property int step: 0

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
        p.landed = { op: op, symbol: op === "supply_collateral" ? "TKA" : "TKB", loanSym: "TKB", loanDec: 6, collDec: 6, amountText: "1,000",
                     key: p.posKey, before: { saved: "0", coll: p.coll, debt: "0" }, account: account, to: "", isPrivate: priv, apr: "0.18" }
        p.play()
    }
    function closePopups() { var w = find(view.item, "walletPage"); if (w) w.closeSheet(); view.item.closeStatus() }

    Timer {
        interval: 1500
        running: view.status === Loader.Ready
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
