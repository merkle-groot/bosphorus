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
// down") or restore → "You're afloat" with the first Everyday address. The water rises a little
// at each step.
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

    Kit.Water {
        level: ({ start: 0.2, phrase: 0.14, restore: 0.14, ready: 0.34 })[page.step] || 0.2
        Behavior on height { NumberAnimation { duration: 1000; easing.type: Easing.OutCubic } }
        LogosText {
            visible: page.step === "ready"
            x: 32; y: 18
            text: qsTr("↓ Private accounts live down here. Only you can see them.")
            font.pixelSize: 13; font.weight: Font.DemiBold
            color: "#E6FFFFFF"
        }
    }

    LogosText { x: 32; y: 22; text: "Bosphorus"; font.pixelSize: 17; font.weight: Font.ExtraBold }

    LogosNotice {
        anchors.horizontalCenter: parent.horizontalCenter
        y: 64
        width: Math.min(parent.width - 48, 540)
        severity: LogosNotice.Error
        message: page.app.openError
        shown: message.length > 0
    }

    // Start.
    ColumnLayout {
        visible: page.step === "start"
        anchors.horizontalCenter: parent.horizontalCenter
        y: Math.max(90, parent.height * 0.16)
        width: Math.min(parent.width - 48, 540)
        spacing: 22
        LogosText {
            Layout.alignment: Qt.AlignHCenter
            text: "bosphorus"
            font.pixelSize: 64; font.weight: Font.ExtraBold; font.letterSpacing: -64 * 0.045
        }
        LogosText {
            Layout.alignment: Qt.AlignHCenter
            Layout.maximumWidth: 440
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
            text: qsTr("Save and borrow against your crypto. We'll always show you how high the water is, so you know how safe you are.")
            font.pixelSize: 19
            color: "#D1FFFFFF"
        }
        Row {
            Layout.alignment: Qt.AlignHCenter
            spacing: 10
            Kit.BigButton {
                objectName: "createWallet"
                text: page.app.busy ? qsTr("Creating…") : qsTr("Create a wallet")
                enabled: !page.app.busy
                onClicked: { page.revealed = false; page.app.createWallet("", "") }
            }
            Kit.BigButton { outline: true; text: qsTr("I have a recovery phrase"); onClicked: page.app.welcomeStep = "restore" }
        }
        LogosText {
            Layout.alignment: Qt.AlignHCenter
            text: page.advanced ? qsTr("Hide network settings") : qsTr("Network settings")
            font.pixelSize: 14; font.underline: true
            color: Tide.text2
            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: page.advanced = !page.advanced }
        }
        Rectangle {
            visible: page.advanced
            Layout.alignment: Qt.AlignHCenter
            Layout.preferredWidth: 440
            implicitHeight: netCol.implicitHeight + 32
            radius: 16
            color: "#D90B1424"
            border.color: "#26FFFFFF"
            ColumnLayout {
                id: netCol
                anchors.fill: parent
                anchors.margins: 16
                spacing: 12
                LogosText { text: qsTr("Network address"); font.pixelSize: 13; color: Tide.text3 }
                LogosTextField { Layout.fillWidth: true; text: page.app.sequencerUrl; onTextChanged: page.app.sequencerUrl = text }
                LogosText { text: qsTr("Lending program"); font.pixelSize: 13; color: Tide.text3 }
                LogosTextField { Layout.fillWidth: true; text: page.app.lendingId; onTextChanged: page.app.lendingId = text }
            }
        }
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
            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: { page.app.phrase = ""; page.app.welcomeStep = "start" } }
        }
        LogosText {
            text: qsTr("Your %1 lifebuoy words").arg(page.words.length)
            font.pixelSize: 40; font.weight: Font.ExtraBold; font.letterSpacing: -40 * 0.035
        }
        LogosText {
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            text: qsTr("If you lose this computer, these words bring your money back. Anyone who sees them can take it. Write them on paper. We'll show them once.")
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
        LogosCheckbox { id: ack; enabled: page.revealed; text: qsTr("I've written them down somewhere safe") }
        Kit.BigButton {
            objectName: "phraseDone"
            Layout.fillWidth: true
            text: qsTr("Continue")
            enabled: page.revealed && ack.checked && !page.app.busy
            onClicked: { page.app.phrase = ""; page.app.openWallet(true) }
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
            onClicked: page.app.createWallet(phraseArea.text.trim().replace(/\s+/g, " "), fromBlock.text.trim())
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
            text: qsTr("Everyday money floats on top, where anyone can see it. To add some, send LEZ to this address from any wallet.")
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
