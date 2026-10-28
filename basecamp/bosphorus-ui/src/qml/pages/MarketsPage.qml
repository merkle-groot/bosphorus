import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Logos.Theme
import Logos.Controls
import "../kit" as Kit
import "../kit/units.js" as Units
import "../kit/tide.js" as Tide

// Markets v2 (docs/design/wallet-v2/HANDOFF.md, "Markets"): the list above the surface, one row per
// market; the chosen one's details below it. A market whose programs don't match this build
// (`code_problem`) is "Turned off for your safety"; a price older than the market's own limit is
// "Old price".
Item {
    id: page
    property var app
    property int topInset: 70
    property bool details: false

    readonly property string selected: app.openMarket !== "" ? app.openMarket : (app.markets.length > 0 ? app.markets[0].market : "")
    readonly property var cur: app.marketById(selected)

    function info(m) {
        var d = page.app.marketDetails[m.market] || null
        var feed = d && d.feed && !d.feed.error ? d.feed : null
        var util = parseFloat(m.utilization) || 0
        var loanDec = m.loan_decimals || 0
        return {
            loan: page.app.tokenSymbol(m.loan_asset), coll: page.app.tokenSymbol(m.collateral_asset),
            loanDec: loanDec, collDec: m.collateral_decimals || 0,
            feed: feed, util: util, off: !!(d && d.code_problem),
            stale: feed !== null && feed.age_ms > m.max_price_age_ms, full: util >= 0.9,
            free: Units.format(Units.sub(m.supplied, m.borrowed), loanDec, 2)
        }
    }
    function pct(wad, places) { var x = parseFloat(wad); return isNaN(x) ? "–" : (x * 100).toFixed(places === undefined ? 2 : places) }
    readonly property var c: cur ? info(cur) : null

    component Disc: Rectangle {
        property string symbol: ""
        readonly property var look: Tide.look(symbol)
        radius: width / 2
        color: look[1]
        LogosText { anchors.centerIn: parent; text: parent.look[0]; font.pixelSize: Math.round(parent.width * 0.45); font.weight: Font.ExtraBold; color: parent.look[2] }
    }

    // Two overlapping token badges.
    component Pair: Item {
        property string a: ""
        property string b: ""
        property int size: 30
        implicitWidth: size * 1.7
        implicitHeight: size
        Disc { x: parent.size * 0.7; width: parent.size; height: parent.size; symbol: parent.b; border.width: 2; border.color: Tide.sky }
        Disc { width: parent.size; height: parent.size; symbol: parent.a; border.width: 2; border.color: Tide.sky }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.topMargin: page.topInset
        spacing: 0

        // Above the surface: the list.
        Item {
            Layout.fillWidth: true
            Layout.preferredHeight: Math.min(listCol.implicitHeight + 34, page.height * 0.5)
            Flickable {
                anchors.fill: parent
                anchors.leftMargin: 32
                anchors.rightMargin: 32
                contentHeight: listCol.implicitHeight
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                ColumnLayout {
                    id: listCol
                    width: Math.min(parent.width, 1000)
                    x: (parent.width - width) / 2
                    spacing: 6
                    RowLayout {
                        spacing: 14
                        Layout.bottomMargin: 6
                        LogosText { text: qsTr("Markets"); font.pixelSize: 30; font.weight: Font.ExtraBold; font.letterSpacing: -0.9 }
                        LogosText {
                            Layout.alignment: Qt.AlignBaseline
                            text: page.app.markets.length === 0 ? (page.app.refreshing ? qsTr("Looking for markets…") : qsTr("No markets under this lending program yet."))
                                                                : qsTr("Lend a token to earn, or borrow one against another.")
                            font.pixelSize: 14; color: "#BFFFFFFF"
                        }
                    }
                    // Columns: 1.7fr 1fr 1fr 1.5fr 1.2fr, gap 16, inside 16 px of padding.
                    component Cols: Item {
                        id: cols
                        default property alias cells: holder.data
                        Layout.fillWidth: true
                        readonly property real unit: (width - 32 - 4 * 16) / 6.4
                        function cx(i) { return 16 + [0, 1.7, 2.7, 3.7, 5.2][i] * unit + i * 16 }
                        function cw(i) { return [1.7, 1, 1, 1.5, 1.2][i] * unit }
                        Item { id: holder; anchors.fill: parent }
                    }
                    Cols {
                        visible: page.app.markets.length > 0
                        implicitHeight: 18
                        id: head
                        Repeater {
                            model: [qsTr("Market"), qsTr("Savers earn"), qsTr("Borrowers pay"), qsTr("Lent out"), qsTr("Free to borrow")]
                            delegate: LogosText {
                                x: head.cx(index); width: head.cw(index)
                                horizontalAlignment: index === 4 ? Text.AlignRight : Text.AlignLeft
                                text: modelData; font.pixelSize: 12; font.weight: Font.DemiBold; color: "#99FFFFFF"
                            }
                        }
                    }
                    Repeater {
                        model: page.app.markets
                        delegate: Rectangle {
                            id: mrow
                            objectName: "marketRow"
                            readonly property var m: modelData
                            readonly property var i: page.info(m)
                            readonly property bool on: m.market === page.selected
                            Layout.fillWidth: true
                            implicitHeight: 62
                            radius: 16
                            color: on ? "#16243D" : "transparent"
                            border.width: on ? 1.5 : 1
                            border.color: on ? "#FFFFFF" : "#1AFFFFFF"
                            opacity: i.off ? 0.7 : 1
                            Cols {
                                id: rc
                                anchors.fill: parent
                                Row {
                                    x: rc.cx(0); width: rc.cw(0)
                                    anchors.verticalCenter: parent.verticalCenter
                                    spacing: 12
                                    Pair { a: mrow.i.loan; b: mrow.i.coll; anchors.verticalCenter: parent.verticalCenter }
                                    Column {
                                        anchors.verticalCenter: parent.verticalCenter
                                        LogosText { text: mrow.i.loan + " · " + mrow.i.coll; font.pixelSize: 16; font.weight: Font.ExtraBold }
                                        LogosText { text: qsTr("Borrow %1 with %2").arg(mrow.i.loan).arg(mrow.i.coll); font.pixelSize: 12; color: "#B3FFFFFF" }
                                    }
                                }
                                LogosText { x: rc.cx(1); anchors.verticalCenter: parent.verticalCenter; text: mrow.i.off ? "–" : page.pct(mrow.m.supply_apr) + "%"; font.pixelSize: 18; font.weight: Font.Bold; color: Tide.good }
                                LogosText { x: rc.cx(2); anchors.verticalCenter: parent.verticalCenter; text: mrow.i.off ? "–" : page.pct(mrow.m.borrow_apr) + "%"; font.pixelSize: 18; font.weight: Font.Bold }
                                Row {
                                    x: rc.cx(3)
                                    anchors.verticalCenter: parent.verticalCenter
                                    spacing: 10
                                    Rectangle {
                                        width: Math.min(120, rc.cw(3) - 50); height: 12; radius: 6
                                        anchors.verticalCenter: parent.verticalCenter
                                        color: "#1AFFFFFF"
                                        Rectangle { width: parent.width * Math.min(1, mrow.i.util); height: parent.height; radius: 6; color: mrow.i.full ? Tide.warn : "#2F7FE0" }
                                    }
                                    LogosText { text: Math.round(mrow.i.util * 100) + "%"; font.weight: Font.DemiBold; anchors.verticalCenter: parent.verticalCenter }
                                }
                                Column {
                                    x: rc.cx(4); width: rc.cw(4)
                                    anchors.verticalCenter: parent.verticalCenter
                                    LogosText { width: parent.width; horizontalAlignment: Text.AlignRight; text: mrow.i.free + " " + mrow.i.loan; font.weight: Font.Bold; elide: Text.ElideLeft }
                                    LogosText {
                                        visible: mrow.i.off || mrow.i.stale || !!mrow.m.cached
                                        width: parent.width; horizontalAlignment: Text.AlignRight
                                        text: mrow.i.off ? qsTr("Turned off") : mrow.i.stale ? qsTr("Old price") : qsTr("Last seen")
                                        font.pixelSize: 12; font.weight: Font.Bold; color: mrow.i.off ? Tide.bad : "#FFC56B"
                                    }
                                }
                            }
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: page.app.openMarket = mrow.m.market }
                        }
                    }
                }
            }
        }

        // Below the surface: the chosen market.
        Rectangle {
            Layout.fillWidth: true
            Layout.fillHeight: true
            color: Tide.waterDeep
            Kit.Waves { width: parent.width; color: Tide.waterDeep }
            Flickable {
                anchors.fill: parent
                anchors.leftMargin: 32
                anchors.rightMargin: 32
                anchors.topMargin: 20
                contentHeight: detail.implicitHeight + 24
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                visible: page.c !== null
                RowLayout {
                    id: detail
                    width: Math.min(parent.width, 1000)
                    x: (parent.width - width) / 2
                    spacing: 36

                    // Left: the pair, its rates and how much is lent out.
                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.preferredWidth: 1.15
                        Layout.alignment: Qt.AlignTop
                        spacing: 18
                        RowLayout {
                            spacing: 14
                            Pair { a: page.c ? page.c.loan : ""; b: page.c ? page.c.coll : ""; size: 44 }
                            ColumnLayout {
                                spacing: 0
                                LogosText { text: page.c ? page.c.loan + " · " + page.c.coll : ""; font.pixelSize: 34; font.weight: Font.ExtraBold; font.letterSpacing: -1 }
                                LogosText { text: page.c ? qsTr("Savers lend %1. Borrowers put up %2 as collateral.").arg(page.c.loan).arg(page.c.coll) : ""; font.pixelSize: 14; color: "#D1FFFFFF" }
                            }
                        }
                        Rectangle {
                            visible: page.c !== null && page.c.off
                            Layout.fillWidth: true
                            implicitHeight: offCol.implicitHeight + 28
                            radius: 14
                            color: Tide.badTint
                            ColumnLayout {
                                id: offCol
                                anchors.fill: parent; anchors.margins: 14; spacing: 4
                                LogosText { text: qsTr("Turned off for your safety"); font.weight: Font.ExtraBold }
                                LogosText { Layout.fillWidth: true; wrapMode: Text.Wrap; font.pixelSize: 14; color: "#FFE0DC"; text: qsTr("This market's program isn't the one Bosphorus expects. Keep your money out until it's fixed.") }
                            }
                        }
                        GridLayout {
                            Layout.fillWidth: true
                            columns: 2
                            columnSpacing: 12
                            Repeater {
                                model: page.c ? [[qsTr("Savers earn"), page.pct(page.cur.supply_apr), Tide.good, qsTr("a year, on %1").arg(page.c.loan)],
                                                 [qsTr("Borrowers pay"), page.pct(page.cur.borrow_apr), "#FFFFFF", qsTr("a year, in %1").arg(page.c.loan)]] : []
                                delegate: Rectangle {
                                    Layout.fillWidth: true
                                    Layout.preferredWidth: 1
                                    implicitHeight: tile.implicitHeight + 32
                                    radius: 18
                                    color: "#4D000A1E"
                                    ColumnLayout {
                                        id: tile
                                        anchors.fill: parent; anchors.margins: 16; anchors.leftMargin: 18; spacing: 2
                                        LogosText { text: modelData[0]; font.pixelSize: 13; color: "#CCFFFFFF" }
                                        LogosText { text: modelData[1] + "%"; font.pixelSize: 40; font.weight: Font.ExtraBold; font.letterSpacing: -1.2; color: modelData[2] }
                                        LogosText { text: modelData[3]; font.pixelSize: 13; color: "#CCFFFFFF" }
                                    }
                                }
                            }
                        }
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 8
                            RowLayout {
                                Layout.fillWidth: true
                                LogosText { text: page.c ? qsTr("%1% lent out").arg(Math.round(page.c.util * 100)) : ""; font.pixelSize: 14; font.weight: Font.Bold; Layout.fillWidth: true }
                                LogosText { text: page.c ? qsTr("<b>%1 %2</b> free to borrow").arg(page.c.free).arg(page.c.loan) : ""; textFormat: Text.StyledText; font.pixelSize: 14; color: "#D9FFFFFF" }
                            }
                            // The tank.
                            Rectangle {
                                Layout.fillWidth: true
                                implicitHeight: 44
                                radius: 14
                                color: "#59000A1E"
                                border.width: 1.5
                                border.color: "#40FFFFFF"
                                clip: true
                                Rectangle {
                                    x: 1.5; y: 1.5
                                    height: parent.height - 3
                                    width: page.c ? (parent.width - 3) * Math.min(1, page.c.util) : 0
                                    radius: 12.5
                                    color: page.c && page.c.full ? Tide.warn : "#2F7FE0"
                                }
                            }
                            LogosText {
                                Layout.fillWidth: true
                                wrapMode: Text.Wrap
                                font.pixelSize: 12
                                color: page.c && page.c.full ? "#FFD08A" : "#BFFFFFFF"
                                text: !page.c ? "" : page.c.full ? qsTr("Nearly full. Savers may have to wait to take money back.")
                                    : qsTr("Plenty free: savers can take their money back any time.")
                            }
                        }
                        LogosText {
                            visible: !!(page.cur && page.cur.cached)
                            Layout.fillWidth: true
                            wrapMode: Text.Wrap
                            text: qsTr("No connection: these are the numbers last seen.")
                            font.pixelSize: 13; color: "#FFD08A"
                        }
                    }

                    // Right: the limit, the price and the way in.
                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.preferredWidth: 1
                        Layout.alignment: Qt.AlignTop
                        spacing: 16
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 8
                            readonly property real lltv: page.cur ? parseFloat(page.cur.lltv) || 0 : 0
                            LogosText { text: qsTr("Borrowing limit"); font.weight: Font.Bold }
                            Item {
                                id: bar
                                Layout.fillWidth: true
                                implicitHeight: 12
                                readonly property real lltv: parent.lltv
                                Rectangle { anchors.fill: parent; radius: 6; color: "#59000A1E" }
                                Rectangle { width: parent.width * parent.lltv; height: parent.height; radius: 6; color: "#33FFFFFF" }
                                Rectangle { x: parent.width * parent.lltv - 1.5; y: -3; width: 3; height: parent.height + 6; radius: 1.5; color: Tide.warn }
                            }
                            Item {
                                Layout.fillWidth: true
                                implicitHeight: 18
                                LogosText { text: "0%"; font.pixelSize: 12; font.weight: Font.Bold }
                                LogosText {
                                    x: Math.min(parent.width - width, Math.max(0, parent.width * bar.lltv - width / 2))
                                    text: qsTr("Sell line · %1%").arg(page.cur ? page.pct(page.cur.lltv, 0) : "–")
                                    font.pixelSize: 12; font.weight: Font.Bold; color: Tide.warn
                                }
                            }
                            LogosText {
                                Layout.fillWidth: true
                                wrapMode: Text.Wrap
                                font.pixelSize: 13; color: "#D9FFFFFF"
                                text: page.c ? qsTr("Borrow up to %1% of what your %2 is worth. Go past the line and some %2 is sold to pay back the loan.").arg(page.pct(page.cur.lltv, 0)).arg(page.c.coll) : ""
                            }
                        }
                        // The price.
                        Rectangle {
                            readonly property bool stale: page.c !== null && page.c.stale
                            Layout.fillWidth: true
                            implicitHeight: priceCol.implicitHeight + 28
                            radius: 16
                            color: stale ? "#4A3208" : "#4D000A1E"
                            ColumnLayout {
                                id: priceCol
                                anchors.fill: parent; anchors.margins: 14; anchors.leftMargin: 16; anchors.rightMargin: 16
                                spacing: 4
                                RowLayout {
                                    Layout.fillWidth: true
                                    LogosText { text: page.c ? qsTr("%1 price").arg(page.c.coll) : ""; Layout.fillWidth: true }
                                    LogosText {
                                        text: page.c && page.c.feed ? Units.price(page.c.feed.price, page.c.loanDec, page.c.collDec) + " " + page.c.loan : "–"
                                        font.pixelSize: 17; font.weight: Font.ExtraBold
                                    }
                                }
                                LogosText {
                                    text: page.c && page.c.feed ? qsTr("Checked %1 ago").arg(Units.ago(page.c.feed.age_ms))
                                        : page.cur && page.cur.cached ? qsTr("Can't check now") : qsTr("Not checked yet")
                                    font.pixelSize: 13; font.weight: Font.Bold
                                    color: page.c && page.c.feed && !page.c.stale ? Tide.good : Tide.warn
                                }
                                LogosText {
                                    visible: parent.parent.stale
                                    Layout.fillWidth: true
                                    wrapMode: Text.Wrap
                                    font.pixelSize: 13; color: "#FFEBC7"
                                    text: qsTr("Too old to trust. Borrowing and taking collateral out wait for a fresh price; saving, taking savings back and paying back still work.")
                                }
                            }
                        }
                        GridLayout {
                            visible: page.c !== null && !page.c.off
                            Layout.fillWidth: true
                            columns: 2
                            columnSpacing: 10
                            Kit.BigButton { Layout.fillWidth: true; Layout.preferredWidth: 1; buttonHeight: 52; outline: true; text: page.c ? qsTr("Save %1").arg(page.c.loan) : ""; onClicked: page.app.openPosition(page.selected, "", "supply") }
                            Kit.BigButton { Layout.fillWidth: true; Layout.preferredWidth: 1; buttonHeight: 52; text: page.c ? qsTr("Borrow %1").arg(page.c.loan) : ""; onClicked: page.app.openPosition(page.selected, "", "borrow") }
                        }
                        LogosText {
                            text: page.details ? qsTr("Hide technical details") : qsTr("Technical details")
                            font.pixelSize: 13; font.underline: true; color: "#D9FFFFFF"
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: page.details = !page.details }
                        }
                        GridLayout {
                            visible: page.details && page.cur !== null
                            Layout.fillWidth: true
                            columns: 2
                            columnSpacing: 12
                            rowSpacing: 4
                            Repeater {
                                model: page.cur ? [
                                    ["market", page.cur.market], ["lltv", page.cur.lltv], ["utilisation", page.pct(page.cur.utilization, 2) + "%"],
                                    ["oracle", page.c && page.c.feed ? page.c.feed.price + " (1e36)" : "–"], ["loan", page.cur.loan_asset], ["collateral", page.cur.collateral_asset]
                                ] : []
                                delegate: Item {
                                    Layout.columnSpan: 2
                                    Layout.fillWidth: true
                                    implicitHeight: Math.max(k.implicitHeight, v.implicitHeight)
                                    LogosText { id: k; width: 80; text: modelData[0]; font.family: Theme.typography.mono; font.pixelSize: 11; color: "#99FFFFFF" }
                                    LogosText { id: v; x: 88; width: parent.width - 88; text: modelData[1]; textFormat: Text.PlainText; wrapMode: Text.WrapAnywhere; font.family: Theme.typography.mono; font.pixelSize: 11; color: "#D9FFFFFF" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
