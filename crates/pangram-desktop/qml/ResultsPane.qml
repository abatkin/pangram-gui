import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Summary, proportions, section inspection. Empty until a completed scan is shown.
Pane {
    id: root
    padding: 0

    signal sectionActivated(int index)

    // Exposed for UI smoke tests.
    readonly property alias sectionListView: sectionList

    readonly property var result: window.mode === "scan" ? window.currentResult : null
    readonly property var section: result && window.selectedSection >= 0 && window.selectedSection < result.sections.length
                                   ? result.sections[window.selectedSection] : null

    function score(v) {
        return (v === null || v === undefined) ? qsTr("n/a") : Number(v).toFixed(2)
    }

    Label {
        textFormat: Text.PlainText
        anchors.centerIn: parent
        width: parent.width - 32
        visible: root.result === null
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
        opacity: 0.7
        text: qsTr("Results appear here after a scan completes.")
    }

    Flickable {
        id: flick
        anchors.fill: parent
        visible: root.result !== null
        clip: true
        contentWidth: width
        contentHeight: column.implicitHeight + 32
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar {}

        ColumnLayout {
            id: column
            x: 16
            y: 16
            width: flick.width - 32
            spacing: 12

            // Summary
            Label {
                textFormat: Text.PlainText
                Layout.fillWidth: true
                text: root.result ? (root.result.headline || root.result.predictionShort || qsTr("Result")) : ""
                font.pointSize: Qt.application.font.pointSize * 1.5
                font.bold: true
                wrapMode: Text.Wrap
            }
            Label {
                textFormat: Text.PlainText
                Layout.fillWidth: true
                visible: text.length > 0
                text: root.result ? (root.result.prediction || "") : ""
                wrapMode: Text.Wrap
            }

            // Proportions bar
            Rectangle {
                id: bar
                Layout.fillWidth: true
                Layout.preferredHeight: 14
                radius: 3
                color: "transparent"
                border.color: palette.mid
                clip: true
                Accessible.role: Accessible.StaticText
                Accessible.name: root.result
                    ? qsTr("AI-generated %1, AI-assisted %2, human-written %3")
                        .arg(window.percent(root.result.fractionAi))
                        .arg(window.percent(root.result.fractionAiAssisted))
                        .arg(window.percent(root.result.fractionHuman))
                    : ""
                Row {
                    anchors.fill: parent
                    anchors.margins: 1
                    Repeater {
                        model: root.result ? [
                            { f: root.result.fractionAi, c: window.aiColor },
                            { f: root.result.fractionAiAssisted, c: window.assistedColor },
                            { f: root.result.fractionHuman, c: window.humanColor }
                        ] : []
                        Rectangle {
                            required property var modelData
                            height: parent.height
                            width: Math.max(0, (modelData.f || 0) * bar.width - 2)
                            color: modelData.c
                        }
                    }
                }
            }

            // Legend with text labels alongside colors
            GridLayout {
                Layout.fillWidth: true
                columns: 3
                columnSpacing: 8
                rowSpacing: 4
                Repeater {
                    model: root.result ? [
                        { name: qsTr("AI-generated"), f: root.result.fractionAi, n: root.result.numAiSegments, c: window.aiColor },
                        { name: qsTr("AI-assisted"), f: root.result.fractionAiAssisted, n: root.result.numAiAssistedSegments, c: window.assistedColor },
                        { name: qsTr("Human-written"), f: root.result.fractionHuman, n: root.result.numHumanSegments, c: window.humanColor }
                    ] : []
                    delegate: RowLayout {
                        id: legendRow
                        required property var modelData
                        Layout.columnSpan: 3
                        spacing: 8
                        Rectangle {
                            Layout.preferredWidth: 14
                            Layout.preferredHeight: 14
                            radius: 2
                            color: legendRow.modelData.c
                            border.color: palette.mid
                        }
                        Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            text: legendRow.modelData.name
                        }
                        Label {
                            textFormat: Text.PlainText
                            text: window.percent(legendRow.modelData.f)
                                  + (legendRow.modelData.n !== null && legendRow.modelData.n !== undefined
                                     ? "  " + (legendRow.modelData.n === 1 ? qsTr("(1 section)")
                                                         : qsTr("(%1 sections)").arg(legendRow.modelData.n)) : "")
                            font.bold: true
                        }
                    }
                }
            }
            Label {
                textFormat: Text.PlainText
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                opacity: 0.75
                font.pointSize: Qt.application.font.pointSize * 0.9
                text: qsTr("Percentages are the share of the text in each category, not the probability that the whole document is AI-written.")
            }

            Label {
                textFormat: Text.PlainText
                Layout.fillWidth: true
                visible: root.result !== null && !!root.result.highlightNote
                wrapMode: Text.Wrap
                color: window.warningColor
                text: root.result ? (root.result.highlightNote || "") : ""
            }

            Button {
                text: qsTr("Copy summary")
                onClicked: window.copyText(backend.summaryText())
            }

            GroupBox {
                Layout.fillWidth: true
                title: qsTr("Scan details")

                GridLayout {
                    width: parent.width
                    columns: 2
                    columnSpacing: 12
                    rowSpacing: 2

                    Label { text: qsTr("Scanned"); opacity: 0.75 }
                    Label {
                        textFormat: Text.PlainText
                        Layout.fillWidth: true
                        wrapMode: Text.Wrap
                        text: window.currentScan ? window.formatTime(window.currentScan.createdAt) : ""
                    }

                    Label { text: qsTr("Model"); opacity: 0.75 }
                    Label {
                        textFormat: Text.PlainText
                        Layout.fillWidth: true
                        wrapMode: Text.Wrap
                        text: {
                            const sc = window.currentScan
                            if (!sc)
                                return ""
                            return sc.returnedVersion ? qsTr("%1 (version %2)").arg(sc.requestedModel).arg(sc.returnedVersion)
                                                      : sc.requestedModel
                        }
                    }

                    Label {
                        textFormat: Text.PlainText
                        text: qsTr("Billed")
                        opacity: 0.75
                        visible: billedLabel.visible
                    }
                    Label {
                        textFormat: Text.PlainText
                        id: billedLabel
                        Layout.fillWidth: true
                        wrapMode: Text.Wrap
                        readonly property var sc: window.currentScan
                        visible: !!sc && sc.credits !== null && sc.credits !== undefined
                        text: visible ? qsTr("%1 words · %2 · %3 (estimated)")
                                         .arg(sc.billedWords)
                                         .arg(sc.credits === 1 ? qsTr("1 credit") : qsTr("%1 credits").arg(sc.credits))
                                         .arg(window.formatUsd(sc.costUsd))
                                      : ""
                    }

                    Label {
                        textFormat: Text.PlainText
                        Layout.columnSpan: 2
                        Layout.fillWidth: true
                        Layout.topMargin: 4
                        visible: text.length > 0
                        wrapMode: Text.Wrap
                        text: window.currentScan && window.currentScan.normalizationNote ? window.currentScan.normalizationNote : ""
                    }

                    Button {
                        Layout.columnSpan: 2
                        Layout.topMargin: 4
                        text: qsTr("Raw request and response…")
                        onClicked: rawDialog.open()
                    }
                }
            }

            // Selected section details
            GroupBox {
                Layout.fillWidth: true
                title: root.section ? qsTr("Section %1").arg(root.section.index + 1) : qsTr("Selected section")

                ColumnLayout {
                    width: parent.width
                    spacing: 6

                    Label {
                        textFormat: Text.PlainText
                        Layout.fillWidth: true
                        visible: root.section === null
                        wrapMode: Text.Wrap
                        opacity: 0.75
                        text: qsTr("Click in the text or choose a section below to see its label and confidence.")
                    }

                    RowLayout {
                        visible: root.section !== null
                        spacing: 8
                        Rectangle {
                            Layout.preferredWidth: 14
                            Layout.preferredHeight: 14
                            radius: 2
                            color: root.section ? window.classColor(root.section.class) : "transparent"
                            border.color: palette.mid
                        }
                        Label {
                            textFormat: Text.PlainText
                            text: root.section ? root.section.label : ""
                            font.bold: true
                        }
                    }

                    GridLayout {
                        Layout.fillWidth: true
                        visible: root.section !== null
                        columns: 2
                        columnSpacing: 12
                        rowSpacing: 2

                        Label { text: qsTr("Confidence"); opacity: 0.75 }
                        Label { text: root.section ? (root.section.confidence || qsTr("n/a")) : "" }

                        Label { text: qsTr("AI assistance score"); opacity: 0.75 }
                        Label { text: root.section ? root.score(root.section.aiAssistanceScore) : "" }

                        Label {
                            textFormat: Text.PlainText
                            text: qsTr("Humanized")
                            opacity: 0.75
                            visible: root.section !== null && root.section.isHumanized !== null
                        }
                        Label {
                            textFormat: Text.PlainText
                            visible: root.section !== null && root.section.isHumanized !== null
                            text: root.section ? (root.section.isHumanized ? qsTr("Yes") : qsTr("No")) : ""
                        }

                        Label {
                            textFormat: Text.PlainText
                            text: qsTr("Humanizer score")
                            opacity: 0.75
                            visible: root.section !== null && root.section.humanizerScore !== null
                        }
                        Label {
                            textFormat: Text.PlainText
                            visible: root.section !== null && root.section.humanizerScore !== null
                            text: root.section ? root.score(root.section.humanizerScore) : ""
                        }

                        Label {
                            textFormat: Text.PlainText
                            text: qsTr("Words / tokens")
                            opacity: 0.75
                            visible: root.section !== null && (root.section.wordCount !== null || root.section.tokenLength !== null)
                        }
                        Label {
                            textFormat: Text.PlainText
                            visible: root.section !== null && (root.section.wordCount !== null || root.section.tokenLength !== null)
                            text: root.section ? (root.section.wordCount ?? "–") + " / " + (root.section.tokenLength ?? "–") : ""
                        }

                        Repeater {
                            model: root.section ? root.section.details : []
                            delegate: Label {
                                required property var modelData
                                required property int index
                                Layout.fillWidth: index % 2 === 1
                                wrapMode: Text.Wrap
                                textFormat: Text.PlainText
                                text: modelData[0] + ": " + modelData[1]
                                Layout.columnSpan: 2
                            }
                        }
                    }

                    // Section text is shown here when it can't be highlighted in place.
                    Label {
                        Layout.fillWidth: true
                        visible: root.section !== null && root.result !== null && !root.result.highlightsValid
                        wrapMode: Text.Wrap
                        textFormat: Text.PlainText
                        text: root.section ? root.section.text : ""
                    }
                }
            }

            Label {
                textFormat: Text.PlainText
                text: root.result ? qsTr("Sections (%1)").arg(root.result.sections.length) : ""
                font.bold: true
                visible: root.result !== null && root.result.sections.length > 0
            }

            ListView {
                id: sectionList
                Layout.fillWidth: true
                Layout.preferredHeight: contentHeight
                interactive: false
                keyNavigationEnabled: true
                activeFocusOnTab: true
                model: root.result ? root.result.sections : []
                currentIndex: window.selectedSection
                Accessible.role: Accessible.List
                Accessible.name: qsTr("Sections")

                onCurrentIndexChanged: {
                    if (currentIndex !== window.selectedSection && currentIndex >= 0)
                        root.sectionActivated(currentIndex)
                    if (currentItem) {
                        const y = column.y + sectionList.y + currentItem.y
                        if (y < flick.contentY)
                            flick.contentY = y
                        else if (y + currentItem.height > flick.contentY + flick.height)
                            flick.contentY = y + currentItem.height - flick.height
                    }
                }

                delegate: ItemDelegate {
                    id: sectionDelegate
                    required property var modelData
                    required property int index
                    width: ListView.view.width
                    highlighted: ListView.isCurrentItem
                    onClicked: root.sectionActivated(index)
                    Accessible.name: qsTr("Section %1: %2, %3 confidence")
                        .arg(index + 1).arg(modelData.label).arg(modelData.confidence || qsTr("unknown"))

                    contentItem: RowLayout {
                        spacing: 8
                        Rectangle {
                            Layout.preferredWidth: 6
                            Layout.fillHeight: true
                            radius: 2
                            color: window.classColor(sectionDelegate.modelData.class)
                        }
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2
                            Label {
                                textFormat: Text.PlainText
                                Layout.fillWidth: true
                                elide: Text.ElideRight
                                font.bold: true
                                text: (sectionDelegate.index + 1) + ". " + sectionDelegate.modelData.label
                                      + (sectionDelegate.modelData.confidence ? " · " + sectionDelegate.modelData.confidence : "")
                            }
                            Label {
                                Layout.fillWidth: true
                                text: sectionDelegate.modelData.text.replace(/\s+/g, " ").trim()
                                textFormat: Text.PlainText
                                elide: Text.ElideRight
                                maximumLineCount: 2
                                wrapMode: Text.Wrap
                                opacity: 0.8
                            }
                        }
                    }
                }
            }
        }
    }
}
