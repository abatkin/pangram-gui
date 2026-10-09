import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Dialog {
    id: root
    title: qsTr("Settings")
    modal: true
    width: Math.min(580, (parent ? parent.width : 580) - 32)
    height: Math.min(implicitHeight, (parent ? parent.height : 900) - 32)

    signal deleteAllRequested
    signal usageRequested

    onAboutToShow: priceField.text = backend.usdPerCredit.toString()
    onOpened: keyField.forceActiveFocus()

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        ScrollView {
            id: scroll
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            clip: true

            ColumnLayout {
                // Leave room for the scrollbar when the content scrolls.
                width: scroll.availableWidth - (scroll.ScrollBar.vertical.visible ? scroll.ScrollBar.vertical.width + 6 : 0)
                spacing: 12

                GroupBox {
                    Layout.fillWidth: true
                    title: qsTr("Pangram API key")

                    ColumnLayout {
                        width: parent.width
                        spacing: 8

                        Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            wrapMode: Text.Wrap
                            text: !backend.hasApiKey ? qsTr("No API key is set.") : backend.keyPersisted ? qsTr("A key is stored in the system keyring.") : qsTr("A key is set for this session only.")
                        }
                        Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            visible: backend.keyNote.length > 0
                            wrapMode: Text.Wrap
                            color: window.warningColor
                            text: backend.keyNote
                        }
                        TextField {
                            id: keyField
                            Layout.fillWidth: true
                            echoMode: TextInput.Password
                            placeholderText: backend.hasApiKey ? qsTr("Paste a new key to replace it") : qsTr("Paste your API key")
                            Accessible.name: qsTr("API key")
                            inputMethodHints: Qt.ImhSensitiveData | Qt.ImhNoPredictiveText
                            onAccepted: saveKey.clicked()
                        }
                        CheckBox {
                            id: rememberBox
                            text: qsTr("Remember the key in the system keyring")
                            checked: backend.rememberKey
                        }
                        RowLayout {
                            Button {
                                id: saveKey
                                text: qsTr("Save key")
                                enabled: keyField.text.trim().length > 0
                                onClicked: {
                                    backend.setApiKey(keyField.text, rememberBox.checked)
                                    keyField.clear()
                                }
                            }
                            Button {
                                text: qsTr("Forget key")
                                enabled: backend.hasApiKey
                                onClicked: backend.forgetApiKey()
                            }
                        }
                    }
                }

                GroupBox {
                    Layout.fillWidth: true
                    title: qsTr("Model")

                    ColumnLayout {
                        width: parent.width
                        spacing: 8
                        RowLayout {
                            Layout.fillWidth: true
                            ComboBox {
                                Layout.fillWidth: true
                                model: backend.models
                                enabled: backend.models.length > 0
                                currentIndex: backend.models.indexOf(backend.selectedModel)
                                displayText: backend.models.length > 0 ? currentText : backend.modelsLoading ? qsTr("Loading…") : qsTr("No models")
                                onActivated: index => backend.selectModel(backend.models[index])
                                Accessible.name: qsTr("Model")
                            }
                            Button {
                                text: qsTr("Refresh")
                                enabled: backend.hasApiKey && !backend.modelsLoading
                                onClicked: backend.refreshModels()
                            }
                        }
                        Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            visible: backend.modelsError.length > 0
                            wrapMode: Text.Wrap
                            color: window.errorColor
                            text: backend.modelsError
                        }
                        Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            wrapMode: Text.Wrap
                            opacity: 0.75
                            text: qsTr("“default” follows Pangram's current default model. The list comes from your account.")
                        }
                    }
                }

                GroupBox {
                    Layout.fillWidth: true
                    title: qsTr("History")

                    ColumnLayout {
                        width: parent.width
                        spacing: 8
                        CheckBox {
                            text: qsTr("Save scans to local history")
                            checked: backend.saveHistory
                            onToggled: backend.setHistoryEnabled(checked)
                        }
                        Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            wrapMode: Text.Wrap
                            opacity: 0.75
                            text: qsTr("When off, new scans are kept only until you quit. Scans already saved stay until you delete them.")
                        }
                        RowLayout {
                            Label {
                                textFormat: Text.PlainText
                                Layout.fillWidth: true
                                text: backend.savedCount === 1 ? qsTr("1 saved scan") : qsTr("%1 saved scans").arg(backend.savedCount)
                            }
                            Button {
                                text: qsTr("Delete all…")
                                enabled: window.history.length > 0
                                onClicked: root.deleteAllRequested()
                            }
                        }
                        Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            wrapMode: Text.Wrap
                            opacity: 0.75
                            text: qsTr("Deleting local scans doesn't delete copies held by Pangram.")
                        }
                        Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            Layout.topMargin: 4
                            wrapMode: Text.Wrap
                            text: qsTr("History is a SQLite database at:")
                        }
                        TextEdit {
                            Layout.fillWidth: true
                            readOnly: true
                            selectByMouse: true
                            wrapMode: TextEdit.WrapAnywhere
                            color: palette.windowText
                            selectionColor: palette.highlight
                            selectedTextColor: palette.highlightedText
                            font.family: "monospace"
                            text: backend.historyPath
                            Accessible.name: qsTr("History file location")
                        }
                    }
                }

                GroupBox {
                    Layout.fillWidth: true
                    title: qsTr("Window")

                    ColumnLayout {
                        width: parent.width
                        spacing: 8
                        CheckBox {
                            id: trayBox
                            text: qsTr("Minimize to the system tray")
                            enabled: trayIcon.available || checked
                            checked: backend.minimizeToTray
                            onToggled: backend.setTrayEnabled(checked)
                        }
                        Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            wrapMode: Text.Wrap
                            opacity: 0.75
                            text: trayIcon.available
                                  ? qsTr("Closing or minimizing the window hides it in the tray and off the taskbar; scans keep running. Click the tray icon to bring it back. Quit from the tray menu or with Ctrl+Q.")
                                  : qsTr("No system tray is available on this desktop.")
                        }
                    }
                }

                GroupBox {
                    Layout.fillWidth: true
                    title: qsTr("Costs")

                    ColumnLayout {
                        width: parent.width
                        spacing: 8
                        RowLayout {
                            Label {
                                textFormat: Text.PlainText
                                text: qsTr("Price per credit (USD)")
                            }
                            TextField {
                                id: priceField
                                Layout.preferredWidth: 100
                                inputMethodHints: Qt.ImhFormattedNumbersOnly
                                validator: DoubleValidator {
                                    bottom: 0
                                    top: 100
                                    decimals: 4
                                    notation: DoubleValidator.StandardNotation
                                    locale: "C"
                                }
                                onEditingFinished: {
                                    const v = Number(text)
                                    if (acceptableInput && v !== backend.usdPerCredit)
                                        backend.saveUsdPerCredit(v)
                                }
                                Accessible.name: qsTr("Price per credit in US dollars")
                            }
                            Button {
                                text: qsTr("Reset")
                                onClicked: {
                                    priceField.text = "0.05"
                                    backend.saveUsdPerCredit(0.05)
                                }
                            }
                        }
                        Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            wrapMode: Text.Wrap
                            opacity: 0.75
                            text: qsTr("Pangram 4 bills one credit per 100 words, rounded up per scan; Pangram's published API price is $0.05 per credit. Pangram doesn't report charges through the API, so costs here are estimates. A new price applies to later scans.")
                        }
                        Button {
                            text: qsTr("Usage and cost…")
                            onClicked: root.usageRequested()
                        }
                    }
                }
            }
        }

        ButtonRow {
            Button {
                text: qsTr("Close")
                onClicked: root.close()
            }
        }
    }
}
