import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import net.batkin.pangram

ApplicationWindow {
    id: window

    width: 1280
    height: 820
    minimumWidth: 560
    minimumHeight: 420
    visible: true
    title: "Pangram"

    // Results move below the document on narrower windows.
    readonly property bool wide: width >= 1050
    property bool historyVisible: true
    // "draft": editable text; "scan": a submitted scan (read-only).
    property string mode: "draft"
    property string draftSourceId: ""
    property int selectedSection: -1

    // Closing or minimizing hides the window to the tray (off the taskbar) instead of quitting.
    readonly property bool trayActive: backend.minimizeToTray && trayIcon.available
    // Set by quit(), so the close it causes isn't turned into hiding.
    property bool quitting: false
    // How to show the window again when it comes back from the tray.
    property int restoreVisibility: Window.Windowed

    readonly property var currentScan: backend.currentJson.length > 0 ? JSON.parse(backend.currentJson) : null
    readonly property var currentResult: currentScan && currentScan.result ? currentScan.result : null
    readonly property var history: JSON.parse(backend.historyJson.length > 0 ? backend.historyJson : "[]")

    readonly property bool dark: palette.window.hslLightness < 0.5
    readonly property color aiColor: dark ? "#8c3b20" : "#ffb59a"
    readonly property color assistedColor: dark ? "#6f5815" : "#ffe08a"
    readonly property color humanColor: dark ? "#25543b" : "#cdeccf"
    readonly property color errorColor: dark ? "#ff8a7a" : "#b3261e"
    readonly property color warningColor: dark ? "#f0c060" : "#8a5a00"

    function classColor(cls) {
        if (cls === "ai") return aiColor
        if (cls === "assisted") return assistedColor
        if (cls === "human") return humanColor
        return "transparent"
    }

    function stateText(state) {
        switch (state) {
        case "submitting": return qsTr("Submitting")
        case "polling": return qsTr("Analyzing")
        case "completed": return qsTr("Completed")
        case "failed": return qsTr("Failed")
        case "paused": return qsTr("Paused")
        case "submissionUnknown": return qsTr("Outcome unknown")
        }
        return state
    }

    function percent(f) {
        return (f === null || f === undefined) ? qsTr("n/a") : Math.round(f * 100) + "%"
    }

    function formatUsd(v) {
        return "$" + Number(v || 0).toFixed(2)
    }

    function formatTime(ms) {
        return new Date(ms).toLocaleString(Qt.locale(), Locale.ShortFormat)
    }

    function copyText(text) {
        clipboardHelper.text = text
        clipboardHelper.selectAll()
        clipboardHelper.copy()
        clipboardHelper.text = ""
    }

    function showNotice(level, message) {
        noticeBar.show(level, message)
    }

    function analyze() {
        if (mode !== "draft" || !documentPane.canAnalyze)
            return
        const id = backend.analyze(documentPane.draftTitle, documentPane.draftText, draftSourceId)
        if (id.length > 0) {
            documentPane.clearDraft()
            draftSourceId = ""
            selectedSection = -1
            mode = "scan"
        }
    }

    function openScan(id) {
        selectedSection = -1
        mode = "scan"
        backend.openScan(id)
    }

    function hasDraft() {
        return documentPane.draftText.trim().length > 0
    }

    function startEmptyDraft() {
        documentPane.clearDraft()
        draftSourceId = ""
        mode = "draft"
        documentPane.focusEditor()
    }

    function newCheck() {
        if (hasDraft())
            draftDialog.ask("new")
        else
            startEmptyDraft()
    }

    function editAndRescan() {
        if (!currentScan)
            return
        if (hasDraft())
            draftDialog.ask("rescan")
        else
            loadRescanDraft()
    }

    function loadRescanDraft() {
        documentPane.setDraft(currentScan.title, currentScan.inputText)
        draftSourceId = currentScan.id
        mode = "draft"
        documentPane.focusEditor()
    }

    function showDraft() {
        mode = "draft"
        documentPane.focusEditor()
    }

    function hideToTray() {
        usageWindow.close()
        window.hide()
    }

    function showFromTray() {
        if (restoreVisibility === Window.Maximized)
            window.showMaximized()
        else if (restoreVisibility === Window.FullScreen)
            window.showFullScreen()
        else
            window.showNormal()
        window.raise()
        window.requestActivate()
    }

    function toggleFromTray() {
        if (window.visible && window.active)
            hideToTray()
        else
            showFromTray()
    }

    function quit() {
        quitting = true
        Qt.quit()
    }

    onClosing: close => {
        if (trayActive && !quitting) {
            close.accepted = false
            hideToTray()
        }
    }

    // Wayland doesn't tell clients about minimizing, so there only closing hides to the tray.
    onVisibilityChanged: v => {
        if (v === Window.Windowed || v === Window.Maximized || v === Window.FullScreen)
            restoreVisibility = v
        else if (v === Window.Minimized && trayActive)
            Qt.callLater(hideToTray)
    }

    Backend {
        id: backend
        onNotice: (level, message) => window.showNotice(level, message)
        onCurrentCleared: {
            if (window.mode === "scan")
                window.mode = "draft"
        }
        onReadyChanged: {
            if (ready && !hasApiKey)
                settingsDialog.open()
        }
        onCurrentRevisionChanged: {
            if (!window.currentResult || window.selectedSection >= window.currentResult.sections.length)
                window.selectedSection = -1
        }
    }

    Component.onCompleted: backend.initialize()

    Shortcut {
        sequences: ["Ctrl+Return", "Ctrl+Enter"]
        context: Qt.ApplicationShortcut
        onActivated: window.analyze()
    }
    Shortcut {
        sequences: [StandardKey.New]
        context: Qt.ApplicationShortcut
        onActivated: window.newCheck()
    }
    Shortcut {
        sequences: [StandardKey.Find]
        context: Qt.ApplicationShortcut
        onActivated: documentPane.openFind()
    }
    Shortcut {
        sequences: ["F9"]
        context: Qt.ApplicationShortcut
        onActivated: window.historyVisible = !window.historyVisible
    }
    Shortcut {
        sequences: [StandardKey.Quit]
        context: Qt.ApplicationShortcut
        onActivated: window.quit()
    }
    Shortcut {
        sequences: [StandardKey.Preferences, "Ctrl+,"]
        context: Qt.ApplicationShortcut
        onActivated: settingsDialog.open()
    }

    header: ToolBar {
        RowLayout {
            anchors.fill: parent
            spacing: 4

            ToolButton {
                text: qsTr("History")
                icon.name: "sidebar-show-symbolic"
                checkable: true
                checked: window.historyVisible
                onToggled: window.historyVisible = checked
                ToolTip.visible: hovered
                ToolTip.text: qsTr("Show or hide history (F9)")
                ToolTip.delay: 600
            }
            ToolButton {
                visible: !window.historyVisible
                text: qsTr("New check")
                icon.name: "document-new"
                onClicked: window.newCheck()
            }
            Item { Layout.fillWidth: true }
            BusyIndicator {
                visible: backend.busy
                running: visible
                Layout.preferredHeight: 24
                Layout.preferredWidth: 24
                Accessible.name: qsTr("Scan in progress")
            }
            Label {
                textFormat: Text.PlainText
                visible: backend.busy
                text: qsTr("Scan in progress")
            }
            ToolButton {
                visible: !window.historyVisible
                text: qsTr("Settings")
                icon.name: "configure"
                onClicked: settingsDialog.open()
            }
        }
    }

    ColumnLayout {
        id: mainLayout
        anchors.fill: parent
        spacing: 0

        NoticeBar {
            id: noticeBar
            Layout.fillWidth: true
        }

        SplitView {
            id: outerSplit
            Layout.fillWidth: true
            Layout.fillHeight: true
            orientation: Qt.Horizontal
            handle: splitHandle

            HistoryPane {
                id: historyPane
                visible: window.historyVisible
                SplitView.preferredWidth: 270
                SplitView.minimumWidth: 200
                SplitView.maximumWidth: 480
                items: window.history
                onNewCheckRequested: window.newCheck()
                onOpenRequested: id => window.openScan(id)
                onSettingsRequested: settingsDialog.open()
                onUsageRequested: usageWindow.openWindow()
                onDeleteRequested: (id, title) => deleteDialog.ask(id, title)
            }

            SplitView {
                id: innerSplit
                SplitView.fillWidth: true
                orientation: window.wide ? Qt.Horizontal : Qt.Vertical
                handle: splitHandle

                DocumentPane {
                    id: documentPane
                    SplitView.fillWidth: true
                    SplitView.fillHeight: true
                    SplitView.minimumWidth: 300
                    SplitView.minimumHeight: 180
                }

                ResultsPane {
                    id: resultsPane
                    SplitView.preferredWidth: 380
                    SplitView.preferredHeight: 300
                    SplitView.minimumWidth: 260
                    SplitView.minimumHeight: 140
                    onSectionActivated: index => documentPane.selectSection(index)
                }
            }
        }
    }

    // A thin, theme-coloured divider with a wider grab area.
    Component {
        id: splitHandle
        Rectangle {
            id: handleItem
            implicitWidth: 7
            implicitHeight: 7
            color: window.palette.window
            Rectangle {
                readonly property bool horizontal: handleItem.width < handleItem.height
                anchors.centerIn: parent
                width: horizontal ? 1 : parent.width
                height: horizontal ? parent.height : 1
                color: handleItem.SplitHandle.pressed || handleItem.SplitHandle.hovered
                       ? window.palette.highlight : window.palette.mid
            }
        }
    }

    SettingsDialog {
        id: settingsDialog
        anchors.centerIn: Overlay.overlay
        onDeleteAllRequested: deleteAllDialog.open()
        onUsageRequested: usageWindow.openWindow()
    }

    TrayIcon {
        id: trayIcon
    }

    UsageWindow {
        id: usageWindow
        transientParent: window
    }

    RawDialog {
        id: rawDialog
        anchors.centerIn: Overlay.overlay
    }

    Dialog {
        id: draftDialog
        property string action: ""
        function ask(a) {
            action = a
            open()
        }
        anchors.centerIn: Overlay.overlay
        modal: true
        title: action === "rescan" ? qsTr("Replace draft?") : qsTr("Unsubmitted draft")
        ColumnLayout {
            anchors.fill: parent
            Label {
                textFormat: Text.PlainText
                Layout.preferredWidth: Math.min(implicitWidth, 440)
                wrapMode: Text.Wrap
                text: draftDialog.action === "rescan"
                      ? qsTr("Editing this scan replaces the text in your current draft, which hasn't been checked.")
                      : qsTr("Your current draft hasn't been checked. Keep editing it, or discard it and start a new check?")
            }
            ButtonRow {
                Button {
                    text: draftDialog.action === "rescan" ? qsTr("Replace draft") : qsTr("Discard draft")
                    onClicked: {
                        draftDialog.close()
                        if (draftDialog.action === "rescan")
                            window.loadRescanDraft()
                        else
                            window.startEmptyDraft()
                    }
                }
                Button {
                    text: qsTr("Show draft")
                    onClicked: {
                        draftDialog.close()
                        window.showDraft()
                    }
                }
                Button {
                    text: qsTr("Cancel")
                    onClicked: draftDialog.close()
                }
            }
        }
    }

    Dialog {
        id: deleteDialog
        property string scanId: ""
        property string scanTitle: ""
        function ask(id, t) {
            scanId = id
            scanTitle = t
            open()
        }
        anchors.centerIn: Overlay.overlay
        modal: true
        title: qsTr("Delete scan?")
        ColumnLayout {
            anchors.fill: parent
            Label {
                textFormat: Text.PlainText
                Layout.preferredWidth: Math.min(implicitWidth, 440)
                wrapMode: Text.Wrap
                text: qsTr("Delete “%1” from local history? This doesn't delete Pangram's copy.").arg(deleteDialog.scanTitle)
            }
            ButtonRow {
                Button {
                    text: qsTr("Delete")
                    onClicked: {
                        backend.deleteScan(deleteDialog.scanId)
                        deleteDialog.close()
                    }
                }
                Button {
                    text: qsTr("Cancel")
                    onClicked: deleteDialog.close()
                }
            }
        }
    }

    Dialog {
        id: deleteAllDialog
        anchors.centerIn: Overlay.overlay
        modal: true
        title: qsTr("Delete all local history?")
        ColumnLayout {
            anchors.fill: parent
            Label {
                textFormat: Text.PlainText
                Layout.preferredWidth: Math.min(implicitWidth, 440)
                wrapMode: Text.Wrap
                text: qsTr("All scans stored on this computer will be deleted, including any still in progress. This doesn't delete Pangram's copies. The cost summary is kept.")
            }
            ButtonRow {
                Button {
                    text: qsTr("Delete all")
                    onClicked: {
                        backend.deleteAllScans()
                        deleteAllDialog.close()
                    }
                }
                Button {
                    text: qsTr("Cancel")
                    onClicked: deleteAllDialog.close()
                }
            }
        }
    }

    // Development aid (debug builds only): `--qml-script=/path/file.qml` loads a QML file that can
    // drive this window, e.g. tests/ui/smoke.qml.
    Loader {
        source: {
            if (!backend.developerBuild())
                return ""
            const arg = Qt.application.arguments.find(a => a.startsWith("--qml-script="))
            return arg ? "file://" + arg.substring("--qml-script=".length) : ""
        }
    }

    // QML has no clipboard API; a hidden text edit performs the copy.
    TextEdit {
        id: clipboardHelper
        visible: false
        textFormat: TextEdit.PlainText
    }
}
