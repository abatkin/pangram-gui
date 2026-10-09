import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// New check, history search, recent scans, settings.
Pane {
    id: root
    padding: 8

    property var items: []

    signal newCheckRequested()
    signal openRequested(string id)
    signal deleteRequested(string id, string title)
    signal settingsRequested()
    signal usageRequested()

    function summaryLine(item) {
        let s = window.formatTime(item.createdAt)
        if (item.state === "completed") {
            if (item.predictionShort)
                s += " · " + item.predictionShort
            if (item.fractionAi !== null && item.fractionAi !== undefined)
                s += " · " + qsTr("%1 AI").arg(window.percent(item.fractionAi))
        } else {
            s += " · " + window.stateText(item.state)
        }
        if (!item.saved)
            s += " · " + qsTr("not saved")
        return s
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 8

        Button {
            Layout.fillWidth: true
            text: qsTr("New check")
            icon.name: "document-new"
            onClicked: root.newCheckRequested()
            ToolTip.visible: hovered
            ToolTip.delay: 600
            ToolTip.text: qsTr("Start a new draft (Ctrl+N)")
        }

        TextField {
            id: search
            Layout.fillWidth: true
            placeholderText: qsTr("Search history")
            Accessible.name: qsTr("Search history")
            onTextChanged: searchTimer.restart()
            Keys.onDownPressed: list.forceActiveFocus()
            Timer {
                id: searchTimer
                interval: 250
                onTriggered: backend.searchHistory(search.text)
            }
        }

        Label {
            textFormat: Text.PlainText
            Layout.fillWidth: true
            visible: !backend.saveHistory || !backend.historyDiskAvailable
            wrapMode: Text.Wrap
            opacity: 0.8
            font.pointSize: Qt.application.font.pointSize * 0.9
            text: !backend.historyDiskAvailable
                  ? qsTr("History can't be saved; scans are kept until you quit.")
                  : qsTr("Saving is off; new scans are kept until you quit.")
        }

        ListView {
            id: list
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: root.items
            keyNavigationEnabled: true
            activeFocusOnTab: true
            currentIndex: -1
            boundsBehavior: Flickable.StopAtBounds
            ScrollBar.vertical: ScrollBar {}
            Accessible.role: Accessible.List
            Accessible.name: qsTr("Recent scans")

            Keys.onReturnPressed: if (currentItem) root.openRequested(currentItem.modelData.id)
            Keys.onEnterPressed: if (currentItem) root.openRequested(currentItem.modelData.id)
            Keys.onDeletePressed: if (currentItem) root.deleteRequested(currentItem.modelData.id, currentItem.modelData.title)

            delegate: ItemDelegate {
                id: entry
                required property var modelData
                required property int index
                width: ListView.view.width
                highlighted: modelData.id === backend.currentId && window.mode === "scan"
                Accessible.name: modelData.title + ", " + root.summaryLine(modelData)

                onClicked: {
                    list.currentIndex = index
                    root.openRequested(modelData.id)
                }

                contentItem: ColumnLayout {
                    spacing: 2
                    Label {
                        Layout.fillWidth: true
                        text: entry.modelData.title
                        textFormat: Text.PlainText
                        elide: Text.ElideRight
                        font.bold: true
                    }
                    Label {
                        textFormat: Text.PlainText
                        Layout.fillWidth: true
                        text: root.summaryLine(entry.modelData)
                        elide: Text.ElideRight
                        opacity: 0.75
                        font.pointSize: Qt.application.font.pointSize * 0.9
                        color: entry.modelData.state === "failed" || entry.modelData.state === "submissionUnknown"
                               ? window.errorColor : palette.windowText
                    }
                }

                TapHandler {
                    acceptedButtons: Qt.RightButton
                    onTapped: contextMenu.popup()
                }
                Menu {
                    id: contextMenu
                    MenuItem {
                        text: qsTr("Open")
                        onTriggered: root.openRequested(entry.modelData.id)
                    }
                    MenuItem {
                        text: qsTr("Delete…")
                        onTriggered: root.deleteRequested(entry.modelData.id, entry.modelData.title)
                    }
                }
            }

            Label {
                textFormat: Text.PlainText
                anchors.centerIn: parent
                width: parent.width - 16
                visible: list.count === 0
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                opacity: 0.7
                text: search.text.length > 0 ? qsTr("No matching scans.") : qsTr("No scans yet.")
            }
        }

        Button {
            Layout.fillWidth: true
            text: qsTr("Usage and cost")
            icon.name: "view-statistics"
            onClicked: root.usageRequested()
        }

        Button {
            Layout.fillWidth: true
            text: qsTr("Settings")
            icon.name: "configure"
            onClicked: root.settingsRequested()
        }
    }
}
