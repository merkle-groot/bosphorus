import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Effects
import Logos.Theme
import Logos.Controls
import "../kit" as Kit
import "../kit/units.js" as Units
import "../kit/tide.js" as Tide

// TWelcome: start → a new wallet's recovery phrase (blurred until revealed, gated by "written
// down") → arriving (the starter WBTC sinking while it's proven) or restore → "You're afloat"
// with the first Everyday address. The water rises a little at each step.
Item {
    id: page
    property var app
    readonly property string step: app.welcomeStep
    readonly property var words: String(app.phrase).split(/\s+/).filter(function(w) { return w.length > 0 })
    property bool revealed: false
    property bool advanced: false
    readonly property string firstPublic: {
        for (var i = 0; i < app.accounts.length; i++) if (!Units.isPrivate(app.accounts[i].account)) return app.accounts[i].account
        return ""
    }

    // The intro on the start screen (kit/LogoIntro.qml): the lockup flashes in front and centre,
    // then settles top left. Once per session.
    property bool introPlayed: false
    readonly property bool introShown: intro.done
    Timer {
        interval: 250
        running: page.visible && page.step === "start" && !page.introPlayed
        onTriggered: { page.introPlayed = true; intro.play() }
    }

    Kit.Water {
        id: water
        level: ({ start: page.introShown ? 0.45 : 0.2, phrase: 0.08, restore: 0.14, arriving: 0.66, ready: 0.34 })[page.step] || 0.2
        Behavior on height { NumberAnimation { duration: 1000; easing.type: Easing.OutCubic } }
        Repeater {
            model: page.step === "start" ? [0.22, 0.55, 0.8] : []
            // Soft at the sides; the depth gradient over it fades it downwards.
            delegate: Rectangle {
                x: water.width * modelData; y: 0
                width: 170; height: water.height
                rotation: 6; transformOrigin: Item.Top
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0; color: "#00FFFFFF" }
                    GradientStop { position: 0.5; color: "#1AFFFFFF" }
                    GradientStop { position: 1; color: "#00FFFFFF" }
                }
                SequentialAnimation on opacity {
                    loops: Animation.Infinite
                    NumberAnimation { from: 0.4; to: 1; duration: 3000 + index * 700; easing.type: Easing.InOutSine }
                    NumberAnimation { from: 1; to: 0.4; duration: 3000 + index * 700; easing.type: Easing.InOutSine }
                }
            }
        }
        // The start screen's sea (and arriving's): deeper towards the bottom, a few soft rays of light.
        Rectangle {
            visible: page.step === "start" || page.step === "arriving"
            anchors.fill: parent
            gradient: Gradient { GradientStop { position: 0; color: "#000A5BB8" } GradientStop { position: 1; color: "#E6031F4A" } }
        }
        // Light just under the surface.
        Rectangle {
            visible: page.step === "start"
            width: parent.width; height: 70
            gradient: Gradient { GradientStop { position: 0; color: "#406FB6FF" } GradientStop { position: 1; color: "#006FB6FF" } }
        }
        LogosText {
            visible: page.step === "ready"
            x: 32; y: 18
            text: qsTr("↓ Private accounts live down here. Only you can see them.")
            font.pixelSize: 13; font.weight: Font.DemiBold
            color: "#E6FFFFFF"
        }
    }

    Row {
        visible: page.step !== "start"
        x: 32; y: 22
        spacing: 10
        Image { source: "../../icons/bosphorus.png"; sourceSize: Qt.size(56, 56); width: 28; height: 28; smooth: true; mipmap: true; anchors.verticalCenter: parent.verticalCenter }
        LogosText { text: "Bosphorus"; font.pixelSize: 17; font.weight: Font.ExtraBold; anchors.verticalCenter: parent.verticalCenter }
    }

    LogosNotice {
        anchors.horizontalCenter: parent.horizontalCenter
        y: 64
        width: Math.min(parent.width - 48, 540)
        severity: LogosNotice.Error
        message: page.app.openError
        shown: message.length > 0
    }

    // Start (the "surface line"): what floats is said above the water, "privately." is under it,
    // and Create a wallet bobs on the surface. The intro's lockup settles top left.
    Item {
        id: shore
        visible: page.step === "start"
        anchors.fill: parent
        readonly property real surface: page.height * (1 - 0.45)
        readonly property real inset: Math.max(40, page.width * 0.07)
        readonly property real big: Math.max(40, Math.min(84, page.width * 0.056))
        // 0 → 1 once the intro is done and the sea has risen.
        property real shown: 0
        Behavior on shown { NumberAnimation { duration: 900; easing.type: Easing.OutCubic } }
        Timer { interval: 300; running: page.introShown && shore.visible; onTriggered: shore.shown = 1 }
        // One slow clock for the swell: the buoy's bob, the drift under water.
        Item { id: drift; property real t: 0
               NumberAnimation on t { from: 0; to: Math.PI * 2; duration: 6000; loops: Animation.Infinite; running: shore.visible } }

        LogosText {
            x: shore.inset
            y: shore.surface - height - shore.big * 0.55 + 18 * (1 - shore.shown)
            width: page.width - shore.inset * 2
            opacity: shore.shown
            text: qsTr("Lend and borrow\nagainst your crypto,")
            font.pixelSize: shore.big; font.weight: Font.ExtraBold; font.letterSpacing: -shore.big * 0.03
            lineHeight: 0.98
        }

        // Below the surface: soft, tinted, drifting a little. The softness is faint offset copies
        // (no shader effect, so it draws on any renderer).
        Item {
            x: shore.inset + 4 * Math.sin(drift.t)
            y: shore.surface + shore.big * 0.45
            opacity: shore.shown
            Repeater {
                model: [[0, 0, 0.22], [-3, 0, 0.13], [3, 0, 0.13], [0, -3, 0.13], [0, 3, 0.13], [-6, 2, 0.08], [6, -2, 0.08], [2, 6, 0.08], [-2, -6, 0.08], [-9, 0, 0.05], [9, 0, 0.05], [0, 9, 0.05], [0, -9, 0.05]]
                delegate: LogosText {
                    x: modelData[0] * (1 + 0.3 * Math.sin(drift.t * 1.3)); y: modelData[1]
                    opacity: modelData[2]
                    text: qsTr("privately.")
                    font.pixelSize: shore.big; font.weight: Font.ExtraBold; font.letterSpacing: -shore.big * 0.03
                    color: "#6AA6F5"
                }
            }
        }

        // The buoy: Create a wallet, riding the surface, with rings spreading from it.
        Item {
            id: buoy
            readonly property real cx: Math.min(page.width - 150, Math.max(shore.inset + 520, page.width * 0.79))
            x: cx - width / 2
            y: shore.surface - height * 0.62 + 3 * Math.sin(drift.t * 2) + 30 * (1 - shore.shown)
            width: createBtn.width; height: createBtn.height
            opacity: shore.shown
            rotation: 0.8 * Math.sin(drift.t * 2 + 1)
            Repeater {
                model: 3
                delegate: Rectangle {
                    id: ring
                    property real k: 0
                    anchors.horizontalCenter: parent.horizontalCenter
                    y: parent.height * 0.62 - height / 2 + 4
                    width: parent.width * (1.05 + 0.7 * k); height: 22 * (1 + 0.9 * k)
                    radius: height / 2
                    color: "transparent"
                    border.width: 1.5; border.color: "#9CCBFF"
                    opacity: 0.55 * (1 - k)
                    SequentialAnimation on k {
                        loops: Animation.Infinite; running: shore.visible
                        PauseAnimation { duration: index * 1200 }
                        NumberAnimation { from: 0; to: 1; duration: 3600; easing.type: Easing.OutSine }
                        PauseAnimation { duration: (2 - index) * 1200 }
                    }
                }
            }
            // Its glow: soft rings behind it.
            Repeater {
                model: [[14, 0.10], [8, 0.16], [3, 0.24]]
                delegate: Rectangle {
                    x: -modelData[0]; y: -modelData[0]
                    width: createBtn.width + 2 * modelData[0]; height: createBtn.height + 2 * modelData[0]
                    radius: height / 2
                    color: Qt.rgba(0.56, 0.77, 1, modelData[1])
                }
            }
            Kit.BigButton {
                id: createBtn
                objectName: "createWallet"
                buttonHeight: 58; fontSize: 18
                text: page.app.busy ? qsTr("Creating…") : qsTr("Create a wallet")
                enabled: !page.app.busy && shore.shown > 0.5
                onClicked: { page.revealed = false; page.app.newWallet() }
            }
        }

        Row {
            x: shore.inset
            y: Math.min(page.height - 80, shore.surface + shore.big * 2.5)
            spacing: 22
            opacity: shore.shown
            Kit.BigButton { outline: true; buttonHeight: 50; text: qsTr("I have a recovery phrase"); enabled: shore.shown > 0.5; onClicked: page.app.welcomeStep = "restore" }
            LogosText {
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Network settings")
                font.pixelSize: 13; font.underline: true
                color: "#B3FFFFFF"
                MouseArea { anchors.fill: parent; anchors.margins: -6; cursorShape: Qt.PointingHandCursor; onClicked: netPop.opened ? netPop.close() : netPop.open() }
                Popup {
                    id: netPop
                    y: -height - 12
                    width: 440
                    padding: 16
                    background: Rectangle { radius: 16; color: "#F20B1424"; border.color: "#26FFFFFF" }
                    contentItem: ColumnLayout {
                        spacing: 12
                        LogosText { text: qsTr("Network address"); font.pixelSize: 13; color: Tide.text3 }
                        LogosTextField { Layout.fillWidth: true; text: page.app.sequencerUrl; onTextChanged: page.app.sequencerUrl = text }
                        LogosText { text: qsTr("Lending program"); font.pixelSize: 13; color: Tide.text3 }
                        LogosTextField { Layout.fillWidth: true; text: page.app.lendingId; onTextChanged: page.app.lendingId = text }
                    }
                }
            }
        }
    }

    Kit.LogoIntro {
        id: intro
        anchors.fill: parent
        visible: page.step === "start"
    }

    // A new wallet's words.
    ColumnLayout {
        visible: page.step === "phrase"
        anchors.horizontalCenter: parent.horizontalCenter
        y: 70
        width: Math.min(parent.width - 48, 640)
        spacing: 18
        LogosText {
            text: qsTr("‹ Back"); font.weight: Font.DemiBold
            // The wallet stays (unconfirmed): its starter keeps proving.
            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: page.app.welcomeStep = "start" }
        }
        LogosText {
            text: qsTr("Your %1 words").arg(page.words.length)
            font.pixelSize: 40; font.weight: Font.ExtraBold; font.letterSpacing: -40 * 0.035
        }
        LogosText {
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            text: qsTr("Write these words on paper and keep them secret. You need them to get your money back if you lose this computer. Anyone who has them can take your money.")
            font.pixelSize: 16
            color: "#D1FFFFFF"
        }
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: grid.implicitHeight + 32
            radius: 18
            color: "#D10B1424"
            border.color: "#26FFFFFF"
            GridLayout {
                id: grid
                anchors.fill: parent
                anchors.margins: 16
                columns: 4
                columnSpacing: 8
                rowSpacing: 8
                layer.enabled: !page.revealed
                layer.effect: MultiEffect { blurEnabled: true; blur: 1.0; blurMax: 24 }
                // Basecamp's renderer blurs less than the probe's: faded too, nothing is readable.
                opacity: page.revealed ? 1 : 0.15
                Repeater {
                    model: page.words
                    delegate: Rectangle {
                        Layout.fillWidth: true
                        implicitHeight: 36
                        radius: 10
                        color: "#12FFFFFF"
                        Row {
                            anchors.verticalCenter: parent.verticalCenter
                            x: 10
                            spacing: 8
                            LogosText { width: 18; horizontalAlignment: Text.AlignRight; text: index + 1; font.pixelSize: 11; color: Tide.text4; anchors.baseline: word.baseline }
                            LogosText { id: word; text: modelData; textFormat: Text.PlainText; font.family: Theme.typography.mono; font.pixelSize: 15 }
                        }
                    }
                }
            }
            Kit.BigButton {
                visible: !page.revealed
                anchors.centerIn: parent
                buttonHeight: 46
                fontSize: 15
                text: qsTr("Show words · no one watching?")
                onClicked: page.revealed = true
            }
        }
        LogosText {
            visible: page.app.starterRunning
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            text: qsTr("Meanwhile, your computer is fetching some test WBTC for you, straight into your private account.")
            font.pixelSize: 14
            color: Tide.text2
        }
        LogosCheckbox { id: ack; enabled: page.revealed; text: qsTr("I've written them down somewhere safe") }
        Kit.BigButton {
            objectName: "phraseDone"
            Layout.fillWidth: true
            text: qsTr("Continue")
            enabled: page.revealed && ack.checked && !page.app.busy
            onClicked: page.app.confirmWallet()
        }
    }

    // Restore.
    ColumnLayout {
        visible: page.step === "restore"
        anchors.horizontalCenter: parent.horizontalCenter
        y: 70
        width: Math.min(parent.width - 48, 540)
        spacing: 18
        LogosText {
            text: qsTr("‹ Back"); font.weight: Font.DemiBold
            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: page.app.welcomeStep = "start" }
        }
        LogosText { text: qsTr("Bring your wallet back"); font.pixelSize: 40; font.weight: Font.ExtraBold; font.letterSpacing: -40 * 0.035 }
        LogosText { text: qsTr("Type your words in order, with spaces between them."); font.pixelSize: 15; color: "#D1FFFFFF" }
        LogosTextArea {
            id: phraseArea
            Layout.fillWidth: true
            Layout.preferredHeight: 110
            placeholderText: "harbor violet tunnel …"
            wrapMode: TextEdit.Wrap
            font.family: Theme.typography.mono
        }
        LogosTextField {
            id: fromBlock
            Layout.fillWidth: true
            placeholderText: qsTr("Look from block (optional; empty looks through the whole chain, which is slow)")
        }
        Kit.BigButton {
            Layout.fillWidth: true
            text: page.app.busy ? qsTr("Restoring…") : qsTr("Restore")
            enabled: !page.app.busy && phraseArea.text.trim().split(/\s+/).length >= 12
            onClicked: page.app.restoreWallet(phraseArea.text.trim().replace(/\s+/g, " "), fromBlock.text.trim())
        }
    }

    // Arriving: the starter WBTC is being proven on this computer. Most of the screen is water:
    // the coin sinks from the surface as the proof runs (by time; a claim isn't a job, so there are
    // no stages to show), and lands at the bottom, in the Private account. Above, what the network
    // sees; below, the clock and what comes next. The wallet opens once it has landed (or at once,
    // by choice).
    Item {
        id: arriving
        anchors.fill: parent
        visible: page.step === "arriving"
        property real now: Date.now()
        onVisibleChanged: now = Date.now()
        readonly property bool failed: !page.app.starterRunning && page.app.starterFailed
        readonly property bool landed: !page.app.starterRunning && !page.app.starterFailed
        readonly property int expectS: page.app.gpu ? 90 : 600
        readonly property bool late: !landed && !failed && secs > expectS * 1.4
        readonly property int secs: Math.max(0, Math.round((now - page.app.starterStarted) / 1000))
        // How far down the coin is: by time, never at the bottom before it lands.
        readonly property real depth: landed ? 1 : failed ? 0 : Math.min(0.9, secs / expectS * 0.9)
        readonly property string clock: Math.floor(secs / 60) + ":" + ("0" + secs % 60).slice(-2)
        readonly property string drop: {
            var id = page.app.tokenId("WBTC"), d = page.app.dropOf(id)
            return d ? Units.format(d.amount, page.app.tokenDecimals(id), 4) + " " : ""
        }
        readonly property real surface: water.y
        readonly property int margin: Math.max(32, Math.round(width * 0.06))
        Timer { interval: 1000; repeat: true; running: arriving.visible && !arriving.landed; onTriggered: arriving.now = Date.now() }

        // Above the surface: what's happening, and what the network sees.
        Column {
            x: arriving.margin
            width: Math.min(560, arriving.width * 0.5)
            y: arriving.surface - height - 34
            spacing: 10
            LogosText {
                text: arriving.landed ? qsTr("Your WBTC is here.") : arriving.failed ? qsTr("That didn't land.") : qsTr("Getting your WBTC")
                font.pixelSize: 44; font.weight: Font.ExtraBold; font.letterSpacing: -44 * 0.035
            }
            LogosText {
                width: parent.width
                wrapMode: Text.Wrap
                font.pixelSize: 15
                color: "#D1FFFFFF"
                text: arriving.landed ? qsTr("%1test WBTC, out of sight in your Private account. Next: borrow some USDC against it.").arg(arriving.drop)
                    : arriving.failed ? qsTr("The network didn't take the claim, so nothing moved. Trying again starts a new one.")
                    : qsTr("Your computer is proving the claim, so nobody learns it was you.")
            }
        }
        Column {
            x: arriving.width - width - arriving.margin
            y: arriving.surface - height - 38
            width: Math.min(300, arriving.width * 0.28)
            spacing: 4
            LogosText { text: qsTr("Seen from above"); font.pixelSize: 13; font.weight: Font.Bold }
            LogosText {
                width: parent.width; wrapMode: Text.Wrap; font.pixelSize: 13; color: Tide.text2
                text: arriving.landed ? qsTr("Someone claimed %1test WBTC. Not who, and not where it went.").arg(arriving.drop)
                                      : qsTr("Nothing yet: the proof is made here, before anything is sent.")
            }
        }

        // The coin, on its way down a faint line from the surface to the Private account.
        Item {
            id: shaft
            x: Math.round(arriving.width * 0.66); y: arriving.surface + 24
            width: 64; height: arriving.height - y - 64
            Rectangle { x: parent.width / 2; width: 1; height: parent.height; color: "#33FFFFFF" }
            Rectangle { x: parent.width / 2; width: 1; height: coin.y + coin.height / 2; color: "#8CFFFFFF" }
            Rectangle {
                id: coin
                width: 64; height: 64; radius: 32
                y: (shaft.height - height) * arriving.depth
                Behavior on y { enabled: arriving.visible; NumberAnimation { duration: 1000; easing.type: Easing.InOutSine } }
                color: Tide.look("WBTC")[1]
                border.width: 4; border.color: "#33FFFFFF"
                LogosText { anchors.centerIn: parent; text: Tide.look("WBTC")[0]; font.pixelSize: 30; font.weight: Font.ExtraBold; color: Tide.look("WBTC")[2] }
                // Bobbing while it sinks.
                SequentialAnimation on rotation {
                    running: arriving.visible && !arriving.landed && !arriving.failed
                    loops: Animation.Infinite
                    NumberAnimation { from: -6; to: 6; duration: 1600; easing.type: Easing.InOutSine }
                    NumberAnimation { from: 6; to: -6; duration: 1600; easing.type: Easing.InOutSine }
                }
                Rectangle {
                    visible: arriving.landed
                    anchors.right: parent.right; anchors.bottom: parent.bottom; anchors.margins: -4
                    width: 24; height: 24; radius: 12; color: Tide.good
                    LogosText { anchors.centerIn: parent; text: "✓"; font.pixelSize: 14; font.weight: Font.ExtraBold; color: Tide.sky }
                }
            }
            LogosText {
                anchors.left: parent.right; anchors.leftMargin: 14
                y: coin.y + (coin.height - height) / 2
                text: arriving.landed ? qsTr("In Private · Main") : arriving.failed ? "" : qsTr("Sinking out of sight")
                font.pixelSize: 13; font.weight: Font.Bold; color: Tide.text2
            }
            LogosText {
                visible: !arriving.landed
                anchors.left: parent.right; anchors.leftMargin: 14; anchors.bottom: parent.bottom
                text: qsTr("Private · Main"); font.pixelSize: 12; color: Tide.text4
            }
        }

        // Below: the clock, what comes next, the way out.
        Column {
            x: arriving.margin
            y: arriving.surface + 40
            width: shaft.x - arriving.margin - 48
            spacing: 26
            Row {
                visible: !arriving.landed && !arriving.failed
                spacing: 18
                LogosText { text: arriving.clock; font.pixelSize: 40; font.weight: Font.ExtraBold; font.family: Theme.typography.mono; anchors.verticalCenter: parent.verticalCenter }
                LogosText {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 300; wrapMode: Text.Wrap; font.pixelSize: 14; color: Tide.text2
                    text: arriving.late ? qsTr("Taking longer than usual. It's still proving on this computer; nothing has gone out yet.")
                                        : page.app.gpu ? qsTr("Usually a minute or two on the graphics card, then a few seconds to land.")
                                                       : qsTr("Usually about 10 minutes on this computer's processor.")
                }
            }
            Column {
                width: parent.width
                spacing: 12
                LogosText { text: qsTr("What comes next"); font.pixelSize: 12; font.weight: Font.Bold; font.capitalization: Font.AllUppercase; color: Tide.text3 }
                Repeater {
                    model: [
                        [qsTr("Borrow against it"), qsTr("Put the WBTC up as collateral and borrow up to 80% of its value in USDC. It stays yours.")],
                        [qsTr("Mind the water line"), qsTr("If WBTC falls far enough, some is sold to pay the loan back. Borrow less and the water stays low.")],
                        [qsTr("Lend what you don't need"), qsTr("USDC lent to a market earns what its borrowers pay, and nobody sees it's yours.")]
                    ]
                    delegate: Row {
                        spacing: 14
                        Rectangle {
                            width: 26; height: 26; radius: 13; color: "#26FFFFFF"
                            LogosText { anchors.centerIn: parent; text: index + 1; font.pixelSize: 13; font.weight: Font.ExtraBold }
                        }
                        Column {
                            width: arriving.width * 0.5 - 40 - arriving.margin
                            spacing: 2
                            LogosText { text: modelData[0]; font.pixelSize: 16; font.weight: Font.ExtraBold }
                            LogosText { width: parent.width; wrapMode: Text.Wrap; text: modelData[1]; font.pixelSize: 14; color: "#D9FFFFFF" }
                        }
                    }
                }
            }
            Row {
                spacing: 10
                Kit.BigButton {
                    objectName: "arrivingRetry"
                    visible: arriving.failed
                    text: qsTr("Try again")
                    onClicked: page.app.startStarter()
                }
                Kit.BigButton {
                    objectName: "arrivingOpen"
                    outline: !arriving.landed
                    text: arriving.landed || arriving.failed ? qsTr("Open my wallet") : qsTr("Open it now; it'll land there")
                    onClicked: page.app.enterMain()
                }
            }
        }
    }

    // Afloat.
    ColumnLayout {
        visible: page.step === "ready"
        anchors.horizontalCenter: parent.horizontalCenter
        y: Math.max(90, parent.height * 0.14)
        width: Math.min(parent.width - 48, 600)
        spacing: 18
        LogosText { Layout.alignment: Qt.AlignHCenter; text: qsTr("You're afloat."); font.pixelSize: 52; font.weight: Font.ExtraBold; font.letterSpacing: -52 * 0.045 }
        LogosText {
            Layout.alignment: Qt.AlignHCenter
            Layout.maximumWidth: 480
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
            text: page.app.faucetId !== "" ? qsTr("Everyday money floats on top, where anyone can see it. Next: free test money, and a short tour.")
                                          : qsTr("Everyday money floats on top, where anyone can see it. To add some, send LEZ to this address from any wallet.")
            font.pixelSize: 17
            color: "#D9FFFFFF"
        }
        Rectangle {
            Layout.alignment: Qt.AlignHCenter
            implicitHeight: 54
            implicitWidth: addr.implicitWidth + copyBtn.implicitWidth + 30
            radius: 27
            color: "#1AFFFFFF"
            border.color: "#33FFFFFF"
            LogosText { id: addr; x: 18; anchors.verticalCenter: parent.verticalCenter; text: page.firstPublic; textFormat: Text.PlainText; font.family: Theme.typography.mono; font.pixelSize: 13 }
            Kit.BigButton {
                id: copyBtn
                anchors.right: parent.right; anchors.rightMargin: 6; anchors.verticalCenter: parent.verticalCenter
                buttonHeight: 40; fontSize: 13.5
                property bool copied: false
                text: copied ? qsTr("Copied") : qsTr("Copy")
                onClicked: { page.app.copy(page.firstPublic); copied = true }
            }
        }
        LogosText {
            visible: page.app.restoring
            Layout.alignment: Qt.AlignHCenter
            Layout.maximumWidth: 480
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
            font.pixelSize: 14
            color: "#D9FFFFFF"
            text: {
                var s = page.app.info && page.app.info.syncing
                var where = s ? qsTr(" (block %1 of %2)").arg(s.synced_block).arg(s.target_block) : ""
                return (page.app.restoreSyncing ? qsTr("Looking through the chain for your private money%1. You can open the wallet meanwhile.").arg(where)
                                                : qsTr("Done looking.")) + " "
                     + qsTr("Had more than one account? Add them again in Wallet, in the same order, then press Update: they come back with their money.")
            }
        }
        Kit.BigButton { Layout.alignment: Qt.AlignHCenter; text: qsTr("Open my wallet"); onClicked: page.app.enterMain() }
    }
}
