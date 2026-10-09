import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Estimated API spend grouped by day, week or month. A separate, resizable window.
ApplicationWindow {
    id: root
    title: qsTr("Usage and cost — Pangram")
    width: 680
    height: 520
    minimumWidth: 540
    minimumHeight: 320
    flags: Qt.Dialog

    readonly property var rows: JSON.parse(backend.usageJson.length > 0 ? backend.usageJson : "[]")
    readonly property var totals: rows.reduce((t, r) => ({
        scans: t.scans + r.scans, words: t.words + r.words,
        credits: t.credits + r.credits, cost: t.cost + r.costUsd
    }), { scans: 0, words: 0, credits: 0, cost: 0 })
    // Relative widths of: period, scans, words, credits, cost. They sum to 1.
    readonly property var columnWeights: [0.31, 0.14, 0.18, 0.17, 0.20]
    // Header, rows and total share one width, which excludes the list's scrollbar when shown.
    readonly property real rowWidth: list.width - (list.contentHeight > list.height ? vbar.width : 0)
    readonly property int cellPadding: 10
    // Exposed for UI smoke tests.
    readonly property alias content: content

    function openWindow() {
        backend.requestUsage(periodBox.currentValue)
        show()
        raise()
        requestActivate()
    }

    function periodLabel(key) {
        const p = key.split("-").map(Number)
        const d = new Date(p[0], p[1] - 1, p.length > 2 ? p[2] : 1)
        switch (periodBox.currentValue) {
        case "day": return d.toLocaleDateString(Qt.locale(), "ddd, MMM d, yyyy")
        case "week": return qsTr("Week of %1").arg(d.toLocaleDateString(Qt.locale(), "MMM d, yyyy"))
        default: return d.toLocaleDateString(Qt.locale(), "MMMM yyyy")
        }
    }

    function number(n) {
        return Number(n).toLocaleString(Qt.locale(), "f", 0)
    }

    function columnX(index) {
        let x = 0
        for (let i = 0; i < index; i++)
            x += columnWeights[i]
        return x * rowWidth
    }

    FontMetrics { id: metrics }

    // One table row: cells are placed explicitly so every row lines up exactly.
    component TableRow: Item {
        id: tableRow
        property var cells: []
        property bool header: false
        width: root.rowWidth
        implicitHeight: metrics.height + 12

        Repeater {
            model: 5
            Label {
                textFormat: Text.PlainText
                required property int index
                x: root.columnX(index) + root.cellPadding
                width: root.columnWeights[index] * root.rowWidth - 2 * root.cellPadding
                anchors.verticalCenter: parent.verticalCenter
                horizontalAlignment: index === 0 ? Text.AlignLeft : Text.AlignRight
                elide: Text.ElideRight
                font.bold: tableRow.header
                text: tableRow.cells[index] ?? ""
            }
        }
    }

    Shortcut {
        sequences: [StandardKey.Cancel]
        onActivated: root.close()
    }

    Pane {
        id: content
        anchors.fill: parent
        padding: 16

        ColumnLayout {
            anchors.fill: parent
            spacing: 10

            RowLayout {
                Layout.fillWidth: true
                Label { text: qsTr("Group by") }
                ComboBox {
                    id: periodBox
                    textRole: "text"
                    valueRole: "value"
                    model: [
                        { value: "day", text: qsTr("Day") },
                        { value: "week", text: qsTr("Week") },
                        { value: "month", text: qsTr("Month") }
                    ]
                    currentIndex: 2
                    onActivated: backend.requestUsage(currentValue)
                    Accessible.name: qsTr("Group by")
                }
                Item { Layout.fillWidth: true }
            }

            Frame {
                Layout.fillWidth: true
                Layout.fillHeight: true
                padding: 1

                ColumnLayout {
                    anchors.fill: parent
                    spacing: 0

                    TableRow {
                        header: true
                        cells: [qsTr("Period"), qsTr("Scans"), qsTr("Words"), qsTr("Credits"), qsTr("Est. cost")]
                    }
                    Rectangle {
                        Layout.fillWidth: true
                        implicitHeight: 1
                        color: palette.mid
                    }
                    ListView {
                        id: list
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        model: root.rows
                        boundsBehavior: Flickable.StopAtBounds
                        keyNavigationEnabled: true
                        activeFocusOnTab: true
                        ScrollBar.vertical: ScrollBar { id: vbar }
                        Accessible.role: Accessible.Table
                        Accessible.name: qsTr("Usage by period")
                        delegate: TableRow {
                            required property var modelData
                            required property int index
                            cells: [root.periodLabel(modelData.period), root.number(modelData.scans),
                                    root.number(modelData.words), root.number(modelData.credits),
                                    window.formatUsd(modelData.costUsd)]
                            Accessible.name: cells.join(", ")
                            Rectangle {
                                anchors.fill: parent
                                z: -1
                                color: parent.ListView.isCurrentItem && list.activeFocus ? palette.highlight
                                     : index % 2 === 1 ? palette.alternateBase : "transparent"
                                opacity: parent.ListView.isCurrentItem && list.activeFocus ? 0.3 : 1
                            }
                        }
                        Label {
                            textFormat: Text.PlainText
                            anchors.centerIn: parent
                            visible: list.count === 0
                            opacity: 0.7
                            text: qsTr("No completed scans recorded yet.")
                        }
                    }
                    Rectangle {
                        Layout.fillWidth: true
                        implicitHeight: 1
                        color: palette.mid
                    }
                    TableRow {
                        header: true
                        cells: [qsTr("Total"), root.number(root.totals.scans), root.number(root.totals.words),
                                root.number(root.totals.credits), window.formatUsd(root.totals.cost)]
                    }
                }
            }

            Label {
                textFormat: Text.PlainText
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                opacity: 0.75
                text: qsTr("Costs are estimates: Pangram 4 bills one credit per 100 words, rounded up per scan, and each scan records the price per credit set at the time (now %1). Your Pangram dashboard is authoritative. Usage records hold no text and are kept when you delete scans.")
                      .arg(window.formatUsd(backend.usdPerCredit))
            }

            ButtonRow {
                Button {
                    text: qsTr("Clear usage records…")
                    enabled: root.rows.length > 0
                    onClicked: clearDialog.open()
                }
                Button {
                    text: qsTr("Close")
                    onClicked: root.close()
                }
            }
        }
    }

    Dialog {
        id: clearDialog
        anchors.centerIn: Overlay.overlay
        modal: true
        title: qsTr("Clear usage records?")
        ColumnLayout {
            anchors.fill: parent
            Label {
                textFormat: Text.PlainText
                Layout.preferredWidth: Math.min(implicitWidth, 380)
                wrapMode: Text.Wrap
                text: qsTr("The cost summary will start again from zero. Scan history isn't affected.")
            }
            ButtonRow {
                Button {
                    text: qsTr("Clear")
                    onClicked: {
                        backend.clearUsage()
                        clearDialog.close()
                    }
                }
                Button {
                    text: qsTr("Cancel")
                    onClicked: clearDialog.close()
                }
            }
        }
    }
}
