import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Draft editor, or the selected scan: highlighted analyzed text when complete, status otherwise.
Pane {
    id: root
    padding: 0

    readonly property alias draftText: editor.text
    readonly property alias draftTitle: titleField.text
    readonly property var scan: window.currentScan
    readonly property var result: window.currentResult
    readonly property bool scanMode: window.mode === "scan"
    readonly property bool showingResult: scanMode && result !== null
    // Qt positions match our UTF-16 offsets only if the rendered document has the expected length.
    property bool positionsReliable: true
    property bool selectingSection: false
    // Exposed for UI smoke tests.
    readonly property alias resultEditor: resultView
    readonly property alias draftEditor: editor
    readonly property alias findBarVisible: findBar.visible
    readonly property alias checkButton: checkButton
    readonly property alias rescanButton: rescanButton
    readonly property alias scanToolbar: scanBar

    readonly property bool canAnalyze: editor.text.trim().length > 0 && backend.hasApiKey
                                       && backend.selectedModel.length > 0 && !backend.busy

    function clearDraft() {
        editor.clear()
        titleField.clear()
    }

    function setDraft(title, text) {
        titleField.text = title
        editor.text = text
        editor.cursorPosition = 0
    }

    function focusEditor() {
        editor.forceActiveFocus()
    }

    function openFind() {
        findBar.open()
    }

    function wordCount(text) {
        const m = text.match(/\S+/g)
        return m ? m.length : 0
    }

    function ensureVisible(flick, item, pos) {
        const r = item.positionToRectangle(pos)
        const margin = 24
        if (r.y < flick.contentY)
            flick.contentY = Math.max(0, r.y - margin)
        else if (r.y + r.height > flick.contentY + flick.height)
            flick.contentY = Math.min(Math.max(0, flick.contentHeight - flick.height),
                                      r.y + r.height - flick.height + margin)
    }

    function selectSection(index) {
        window.selectedSection = index
        if (!showingResult || !positionsReliable || index < 0)
            return
        const s = result.sections[index]
        if (s.start === null || s.end === null)
            return
        selectingSection = true
        resultView.select(s.start, s.end)
        selectingSection = false
        ensureVisible(resultFlick, resultView, s.start)
    }

    // Searches the visible text; returns false if nothing matched.
    function find(query, backwards) {
        if (query.length === 0)
            return true
        let item, flick, haystack
        if (!scanMode) {
            item = editor; flick = editorFlick; haystack = editor.text
        } else if (showingResult && positionsReliable) {
            item = resultView; flick = resultFlick; haystack = result.displayText
        } else if (scanMode && scan) {
            item = inputView; flick = inputFlick; haystack = inputView.text
        } else {
            return false
        }
        const re = new RegExp(query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi")
        let match = null
        if (!backwards) {
            re.lastIndex = item.selectionEnd
            match = re.exec(haystack)
            if (!match) {
                re.lastIndex = 0
                match = re.exec(haystack)
            }
        } else {
            let m, last = null
            while ((m = re.exec(haystack)) !== null) {
                if (m.index < item.selectionStart)
                    match = m
                last = m
            }
            if (!match)
                match = last
        }
        if (!match)
            return false
        item.select(match.index, match.index + match[0].length)
        ensureVisible(flick, item, match.index)
        return true
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Draft toolbar: title, model, analyze. When narrow the title gets its own row and the
        // model picker shrinks, so Check for AI always stays visible.
        ToolBar {
            id: draftBar
            Layout.fillWidth: true
            visible: !root.scanMode
            position: ToolBar.Header
            topPadding: 4
            bottomPadding: 4
            leftPadding: 8
            rightPadding: 8

            readonly property bool narrow: availableWidth < 220 + draftControls.implicitWidth

            GridLayout {
                width: parent.width
                columns: draftBar.narrow ? 1 : 2
                columnSpacing: 8
                rowSpacing: 4

                TextField {
                    id: titleField
                    Layout.fillWidth: true
                    Layout.minimumWidth: 120
                    placeholderText: qsTr("Title (optional)")
                    Accessible.name: qsTr("Title")
                }
                RowLayout {
                    id: draftControls
                    Layout.fillWidth: draftBar.narrow
                    spacing: 8

                    Label {
                        textFormat: Text.PlainText
                        text: qsTr("Model")
                        visible: modelBox.visible
                    }
                    ComboBox {
                        id: modelBox
                        Layout.fillWidth: draftBar.narrow
                        Layout.minimumWidth: 80
                        model: backend.models
                        visible: backend.models.length > 0
                        currentIndex: backend.models.indexOf(backend.selectedModel)
                        onActivated: index => backend.selectModel(backend.models[index])
                        Accessible.name: qsTr("Model")
                    }
                    Item {
                        Layout.fillWidth: draftBar.narrow && !modelBox.visible
                    }
                    Button {
                        id: checkButton
                        text: qsTr("Check for AI")
                        highlighted: true
                        enabled: root.canAnalyze
                        onClicked: window.analyze()
                        ToolTip.visible: hovered
                        ToolTip.delay: 600
                        ToolTip.text: !backend.hasApiKey ? qsTr("Add an API key in Settings first")
                                     : backend.busy ? qsTr("Wait for the current scan to finish")
                                     : backend.selectedModel.length === 0 ? qsTr("No model available")
                                     : qsTr("Analyze this text (Ctrl+Enter)")
                    }
                }
            }
        }

        // Scan toolbar: title, state and actions. The actions wrap onto more rows when narrow.
        ToolBar {
            id: scanBar
            Layout.fillWidth: true
            visible: root.scanMode
            position: ToolBar.Header
            topPadding: 4
            bottomPadding: 4
            leftPadding: 8
            rightPadding: 8

            readonly property real actionsWidth: {
                let w = -scanActions.spacing
                for (const c of scanActions.children)
                    if (c.visible)
                        w += c.implicitWidth + scanActions.spacing
                return Math.max(0, w)
            }
            readonly property bool narrow: availableWidth < 200 + stateLabel.implicitWidth + actionsWidth + 16

            GridLayout {
                width: parent.width
                columns: scanBar.narrow ? 1 : 2
                columnSpacing: 8
                rowSpacing: 4

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    Label {
                        textFormat: Text.PlainText
                        Layout.fillWidth: true
                        text: root.scan ? root.scan.title : qsTr("Loading…")
                        font.bold: true
                        elide: Text.ElideRight
                    }
                    Label {
                        id: stateLabel
                        textFormat: Text.PlainText
                        visible: root.scan !== null
                        text: root.scan ? window.stateText(root.scan.state) : ""
                        color: root.scan && (root.scan.state === "failed" || root.scan.state === "submissionUnknown")
                               ? window.errorColor : palette.windowText
                    }
                }

                Flow {
                    id: scanActions
                    Layout.fillWidth: scanBar.narrow
                    Layout.preferredWidth: scanBar.narrow ? -1 : scanBar.actionsWidth
                    Layout.preferredHeight: implicitHeight
                    spacing: 8

                    Button {
                        text: qsTr("Stop polling")
                        visible: root.scan !== null && root.scan.state === "polling"
                        onClicked: backend.stopPolling(root.scan.id)
                        ToolTip.visible: hovered
                        ToolTip.delay: 600
                        ToolTip.text: qsTr("Stops checking for the result. Pangram may still finish (and bill) the scan.")
                    }
                    Button {
                        text: qsTr("Resume")
                        visible: root.scan !== null && root.scan.state === "paused" && !!root.scan.taskId
                        enabled: !backend.busy
                        onClicked: backend.resumePolling(root.scan.id)
                    }
                    Button {
                        text: qsTr("Copy text")
                        visible: root.scan !== null
                        onClicked: window.copyText(root.result ? root.result.displayText : root.scan.inputText)
                        ToolTip.visible: hovered
                        ToolTip.delay: 600
                        ToolTip.text: root.result ? qsTr("Copy the analyzed text") : qsTr("Copy the submitted text")
                    }
                    Button {
                        text: qsTr("Raw data")
                        visible: root.scan !== null
                        onClicked: rawDialog.open()
                        ToolTip.visible: hovered
                        ToolTip.delay: 600
                        ToolTip.text: qsTr("Show the request sent to Pangram and its responses")
                    }
                    Button {
                        id: rescanButton
                        text: qsTr("Edit and rescan")
                        visible: root.scan !== null
                        onClicked: window.editAndRescan()
                        ToolTip.visible: hovered
                        ToolTip.delay: 600
                        ToolTip.text: qsTr("Copy the submitted text into a new draft. This result stays in history.")
                    }
                }
            }
        }

        FindBar {
            id: findBar
            Layout.fillWidth: true
            onFindRequested: (query, backwards) => found = root.find(query, backwards)
            onClosed: {
                if (root.scanMode && root.showingResult)
                    resultView.forceActiveFocus()
                else if (!root.scanMode)
                    editor.forceActiveFocus()
            }
        }

        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: !root.scanMode ? 0 : root.showingResult ? 1 : 2

            // 0: draft editor
            Flickable {
                id: editorFlick
                clip: true
                contentWidth: width
                contentHeight: editor.implicitHeight
                boundsBehavior: Flickable.StopAtBounds
                ScrollBar.vertical: ScrollBar {}

                TextArea.flickable: TextArea {
                    id: editor
                    textFormat: TextEdit.PlainText
                    wrapMode: TextEdit.Wrap
                    selectByMouse: true
                    persistentSelection: true
                    placeholderText: qsTr("Paste or type the text to check, then press Check for AI (Ctrl+Enter).\n\nNothing is sent to Pangram until you ask.")
                    Accessible.name: qsTr("Text to check")
                    // Editable text gets the normal input background; read-only views keep the
                    // window colour. Attached to the Flickable, this fills the whole viewport.
                    background: Rectangle {
                        color: palette.base
                        border.width: 1
                        border.color: editor.activeFocus ? palette.highlight : palette.mid
                    }
                    // Tab moves focus rather than inserting a tab character.
                    Keys.onPressed: event => {
                        if (event.key === Qt.Key_Tab && event.modifiers === Qt.NoModifier) {
                            nextItemInFocusChain(true).forceActiveFocus(Qt.TabFocusReason)
                            event.accepted = true
                        } else if (event.key === Qt.Key_Backtab) {
                            nextItemInFocusChain(false).forceActiveFocus(Qt.BacktabFocusReason)
                            event.accepted = true
                        }
                    }
                }
            }

            // 1: highlighted, read-only analyzed text
            Flickable {
                id: resultFlick
                clip: true
                contentWidth: width
                contentHeight: resultView.implicitHeight
                boundsBehavior: Flickable.StopAtBounds
                ScrollBar.vertical: ScrollBar {}

                TextArea.flickable: TextArea {
                    id: resultView
                    readOnly: true
                    textFormat: TextEdit.RichText
                    wrapMode: TextEdit.Wrap
                    selectByMouse: true
                    selectByKeyboard: true
                    persistentSelection: true
                    activeFocusOnTab: true
                    Accessible.name: qsTr("Analyzed text")
                    Accessible.description: qsTr("Highlighted by section. Select a section in the results list to inspect it.")
                    // Depends on currentRevision so it re-renders when the scan changes.
                    text: root.showingResult && backend.currentRevision >= 0
                          ? backend.resultHtml(window.aiColor.toString(), window.assistedColor.toString(),
                                               window.humanColor.toString())
                          : ""
                    onTextChanged: {
                        // RichText `text` is never empty; skip the transient empty document.
                        if (!root.showingResult || length === 0)
                            return
                        root.positionsReliable = !root.result || length === root.result.displayLen
                        if (!root.positionsReliable)
                            console.warn("Rendered text length", length, "differs from analyzed text length",
                                         root.result.displayLen, "- section mapping disabled")
                    }
                    onCursorPositionChanged: {
                        if (root.selectingSection || !root.positionsReliable)
                            return
                        const pos = selectionStart !== selectionEnd ? selectionStart : cursorPosition
                        let index = backend.sectionAt(pos)
                        if (index < 0 && pos > 0)
                            index = backend.sectionAt(pos - 1)
                        if (index >= 0)
                            window.selectedSection = index
                    }
                    Keys.onPressed: event => {
                        if (event.key === Qt.Key_Tab && event.modifiers === Qt.NoModifier) {
                            nextItemInFocusChain(true).forceActiveFocus(Qt.TabFocusReason)
                            event.accepted = true
                        }
                    }
                }
            }

            // 2: scan in progress, failed, paused or loading
            ColumnLayout {
                spacing: 12

                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.margins: 16
                    spacing: 8

                    RowLayout {
                        spacing: 8
                        BusyIndicator {
                            running: visible
                            visible: root.scan === null || root.scan.state === "submitting" || root.scan.state === "polling"
                            Layout.preferredWidth: 32
                            Layout.preferredHeight: 32
                        }
                        Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            font.pointSize: Qt.application.font.pointSize * 1.2
                            wrapMode: Text.Wrap
                            text: {
                                if (!root.scan)
                                    return qsTr("Loading…")
                                switch (root.scan.state) {
                                case "submitting": return qsTr("Submitting to Pangram…")
                                case "polling": return backend.progress.length > 0 ? backend.progress : qsTr("Waiting for Pangram…")
                                case "failed": return qsTr("The scan failed.")
                                case "paused": return qsTr("Polling is paused.")
                                case "submissionUnknown": return qsTr("It's not known whether Pangram received this scan.")
                                case "completed": return qsTr("The result could not be displayed.")
                                }
                                return ""
                            }
                        }
                    }
                    Label {
                        textFormat: Text.PlainText
                        Layout.fillWidth: true
                        visible: text.length > 0
                        wrapMode: Text.Wrap
                        color: root.scan && root.scan.state === "paused" ? palette.windowText : window.errorColor
                        text: root.scan ? (root.scan.error || root.scan.resultError || "") : ""
                    }
                    Label {
                        textFormat: Text.PlainText
                        Layout.fillWidth: true
                        visible: root.scan !== null && root.scan.state === "polling"
                        wrapMode: Text.Wrap
                        opacity: 0.75
                        text: qsTr("You can keep working; the result will appear in history when it's ready.")
                    }
                }

                Label {
                    textFormat: Text.PlainText
                    Layout.leftMargin: 16
                    text: qsTr("Submitted text")
                    font.bold: true
                    visible: root.scan !== null
                }

                Flickable {
                    id: inputFlick
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    visible: root.scan !== null
                    clip: true
                    contentWidth: width
                    contentHeight: inputView.implicitHeight
                    boundsBehavior: Flickable.StopAtBounds
                    ScrollBar.vertical: ScrollBar {}

                    TextArea.flickable: TextArea {
                        id: inputView
                        readOnly: true
                        textFormat: TextEdit.PlainText
                        wrapMode: TextEdit.Wrap
                        selectByMouse: true
                        selectByKeyboard: true
                        persistentSelection: true
                        text: root.scan && !root.showingResult ? root.scan.inputText : ""
                        Accessible.name: qsTr("Submitted text")
                    }
                }
            }
        }

        // Draft status line: size and an estimate of what the scan will cost.
        Frame {
            Layout.fillWidth: true
            visible: !root.scanMode
            padding: 4
            leftPadding: 10
            rightPadding: 10

            Label {
                textFormat: Text.PlainText
                id: draftStatus
                width: parent.width
                elide: Text.ElideRight
                opacity: 0.8
                readonly property int words: root.wordCount(editor.text)
                readonly property int credits: backend.estimateCredits(words, backend.selectedModel)
                text: {
                    let s = qsTr("%1 characters · %2 words").arg(editor.text.length).arg(words)
                    if (words > 0)
                        s += " · " + (credits === 1 ? qsTr("about 1 credit") : qsTr("about %1 credits").arg(credits))
                             + " (" + window.formatUsd(credits * backend.usdPerCredit) + ")"
                    if (window.draftSourceId.length > 0)
                        s += " · " + qsTr("editing a previous scan")
                    return s
                }
                HoverHandler { id: statusHover }
                ToolTip.visible: statusHover.hovered && words > 0
                ToolTip.delay: 600
                ToolTip.text: qsTr("Estimate: Pangram bills per 100 words, rounded up, and counts words a little differently. The actual charge is shown with the result.")
            }
        }
    }
}
