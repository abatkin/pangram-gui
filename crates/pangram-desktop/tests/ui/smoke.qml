// UI smoke test, loaded into the main window by debug builds via --qml-script.
// Run through scripts/ui-smoke.sh, which starts the mock API and isolates settings/history.
import QtQuick
import QtTest
import QtQuick.Controls

Item {
    id: smoke

    property int step: 0
    property int ticks: 0
    property var failures: []
    property string firstId: ""
    property string failedId: ""
    property int historyBefore: 0
    property var notices: []
    property var finishedScans: []

    Connections {
        target: backend
        function onNotice(level, message) { smoke.notices.push(message) }
        function onScanFinished(state, ai, assisted, human) { smoke.finishedScans.push({ state, ai, human }) }
    }
    readonly property string outDir: {
        const a = Qt.application.arguments.find(x => x.startsWith("--smoke-out="))
        return a ? a.substring("--smoke-out=".length) : ""
    }
    readonly property string sample: "Hello “world”.  This has  double spaces.\tAnd a tab! Emoji 😀 here? "
        + "<b>not bold</b> & \"quotes\".\n\nSecond paragraph: 日本語です。 Café crème. Final sentence here.\n"
        + "Last line without punctuation"

    TestEvent { id: keys }

    // Renders the whole window, including the header and open dialogs (popups live in the overlay).
    // Created only when screenshots are requested, and not live: a live recursive capture
    // re-renders the window every frame, and resizing with one present crashes Mesa's radeonsi
    // driver (observed with Mesa 26.2 on Wayland).
    Loader {
        id: captureLoader
        active: smoke.outDir.length > 0
        sourceComponent: ShaderEffectSource {
            width: window.width
            height: window.height
            sourceItem: Overlay.overlay.parent
            recursive: true
            live: false
            opacity: 0.01
        }
    }

    function type(text) {
        for (const c of text)
            keys.keyClickChar(c, Qt.NoModifier, -1)
    }

    function log(m) { console.warn("SMOKE: " + m) }
    function check(cond, what) {
        if (cond) {
            log("ok   " + what)
        } else {
            failures.push(what)
            log("FAIL " + what)
        }
    }
    function plain(s) { return s.replace(/[\u2028\u2029]/g, "\n") }
    // Waits for popup animations to settle, then captures the window.
    function shot(name, next) {
        if (outDir.length === 0) {
            step = next
            return
        }
        step = -1
        shotTimer.name = name
        shotTimer.next = next
        shotTimer.start()
    }
    // Captures a separate window's content item.
    function shotItem(item, name, next) {
        if (outDir.length === 0) {
            step = next
            return
        }
        step = -1
        shotTimer.item = item
        shotTimer.name = name
        shotTimer.next = next
        shotTimer.start()
    }
    Timer {
        id: shotTimer
        property Item item: null
        property string name
        property int next
        interval: 500
        onTriggered: {
            if (!item)
                captureLoader.item.scheduleUpdate()
            grabTimer.start()
        }
    }
    Timer {
        id: grabTimer
        interval: 100
        onTriggered: (shotTimer.item || captureLoader.item).grabToImage(r => {
            shotTimer.item = null
            r.saveToFile(smoke.outDir + "/" + shotTimer.name + ".png")
            smoke.step = shotTimer.next
        })
    }
    property bool finished: false
    function fitsWindow(item) {
        const p = item.mapToItem(null, 0, 0)
        return item.visible && p.x >= 0 && p.x + item.width <= window.width + 0.5
    }

    // A title that would render as a 400 px image if treated as markup. It must stay under the
    // 80-character title limit, hence a short system image path (fedora-logos).
    readonly property string htmlTitle: '<img src="file:///usr/share/pixmaps/fedora-logo.png" height="400">x'

    function finish() {
        if (finished)
            return
        finished = true
        step = 99
        log(failures.length === 0 ? "PASSED" : "FAILED: " + failures.join("; "))
        Qt.exit(failures.length === 0 ? 0 : 1)
    }

    Timer {
        interval: 150
        repeat: true
        running: true
        // Key simulation can spin a nested event loop; don't re-enter a step from inside it.
        property bool inStep: false
        onTriggered: {
            if (inStep)
                return
            inStep = true
            try {
                smoke.tick()
            } finally {
                inStep = false
            }
        }
    }

    function tick() {
        {
            smoke.ticks++
            if (smoke.ticks > 400) {
                smoke.check(false, "timed out at step " + smoke.step)
                smoke.finish()
                return
            }
            const scan = window.currentScan
            switch (smoke.step) {
            case 0:
                if (backend.ready) {
                    smoke.log("platform: " + Qt.platform.pluginName)
                    if (Qt.application.arguments.indexOf("--smoke-dark") >= 0) {
                        // Approximates Breeze Dark for screenshot review of dark mode.
                        const p = window.palette
                        p.window = "#202326"; p.windowText = "#fcfcfc"; p.base = "#141618"
                        p.alternateBase = "#1d1f22"; p.text = "#fcfcfc"; p.button = "#292c30"
                        p.buttonText = "#fcfcfc"; p.highlight = "#3daee9"; p.highlightedText = "#fcfcfc"
                        p.mid = "#4d4d4d"; p.midlight = "#3a3d41"; p.dark = "#101010"; p.light = "#3f4246"
                        p.placeholderText = "#a1a9b1"
                        smoke.check(window.dark, "dark palette detected")
                    }
                    smoke.check(settingsDialog.visible, "settings open on first run without a key")
                    backend.setApiKey("mock-key", false)
                    smoke.step = 1
                }
                break
            case 1:
                if (backend.selectedModel.length > 0 && !backend.modelsLoading) {
                    smoke.check(backend.models.length === 2 && backend.selectedModel === "default", "models loaded, default chosen")
                    smoke.shot("01-settings", 2)
                }
                break
            case 2:
                settingsDialog.close()
                documentPane.setDraft("", smoke.sample)
                smoke.check(documentPane.draftEditor.background.color === palette.base, "editable text uses the input background")
                smoke.shot("02-draft", 3)
                break
            case 3:
                window.analyze()
                smoke.check(window.mode === "scan", "analyze switches to scan view")
                smoke.check(documentPane.draftText.length === 0, "draft cleared after submission")
                smoke.step = 4
                break
            case 4:
                if (scan && scan.state === "polling")
                    smoke.shot("03-polling", 5)
                break
            case 5:
                if (scan && scan.state === "completed" && window.currentResult) {
                    const r = window.currentResult
                    const ed = documentPane.resultEditor
                    smoke.firstId = scan.id
                    smoke.check(r.highlightsValid, "highlights valid")
                    smoke.check(documentPane.positionsReliable, "rendered length matches (" + ed.length + " vs " + r.displayLen + ")")
                    smoke.check(!!scan.normalizationNote && scan.normalizationNote.indexOf("changed some characters") >= 0,
                                "normalization explained: " + scan.normalizationNote)
                    smoke.check(scan.credits === 1 && scan.billedWords > 0 && Math.abs(scan.costUsd - 0.05) < 1e-9,
                                "charge recorded: " + scan.billedWords + " words, " + scan.credits + " credits")
                    smoke.check(smoke.notices.some(n => n.indexOf("mock Pangram API") >= 0), "server notice shown")
                    smoke.check(window.title === "Pangram", "window title stays fixed")
                    smoke.check(r.sections.length >= 4, "sections returned: " + r.sections.length)
                    smoke.check(smoke.finishedScans.length === 1 && smoke.finishedScans[0].state === "completed"
                                && smoke.finishedScans[0].ai === r.fractionAi, "completion signalled with the result")
                    smoke.check(plain(ed.getText(0, ed.length)) === r.displayText, "rendered text is literal")
                    let mapped = true, selected = true
                    for (let i = 0; i < r.sections.length; i++) {
                        const s = r.sections[i]
                        if (backend.sectionAt(s.start) !== i)
                            mapped = false
                        documentPane.selectSection(i)
                        if (window.selectedSection !== i || plain(ed.selectedText) !== r.displayText.substring(s.start, s.end))
                            selected = false
                    }
                    smoke.check(mapped, "sectionAt maps every section start")
                    smoke.check(selected, "selecting each section selects exactly its text")
                    documentPane.selectSection(1)
                    smoke.check(documentPane.find("emoji", false) && ed.selectedText === "Emoji", "find in result")
                    smoke.check(backend.summaryText().indexOf("Share of text") >= 0, "summary text")
                    // Clipboard: a selection spanning several highlights pastes as the plain text.
                    const sink = Qt.createQmlObject("import QtQuick; TextEdit { visible: false; textFormat: TextEdit.PlainText }", smoke)
                    const a = r.sections[0].start, b = r.sections[3].end
                    ed.select(a, b)
                    ed.copy()
                    sink.paste()
                    smoke.check(plain(sink.text) === r.displayText.substring(a, b), "copy across highlights pastes plain text")
                    window.copyText("marker 😀 <b>")
                    sink.text = ""
                    sink.paste()
                    smoke.check(sink.text === "marker 😀 <b>", "copy buttons place text on the clipboard")
                    sink.destroy()
                    documentPane.selectSection(2)
                    smoke.shot("04-result", 50)
                }
                break
            case 50: {
                rawDialog.open()
                const d = rawDialog.details
                smoke.check(d.request.indexOf('"public_dashboard_link": false') >= 0 && d.request.indexOf("mock-key") < 0,
                            "raw request shown without the key")
                smoke.check(d.submitResponse.indexOf("task_id") >= 0 && d.response.indexOf("STAGE_SUCCESS") >= 0,
                            "raw responses shown")
                smoke.shot("10-raw", 51)
                break
            }
            case 51:
                rawDialog.close()
                usageWindow.openWindow()
                smoke.step = 52
                break
            case 52:
                if (usageWindow.rows.length > 0) {
                    smoke.check(usageWindow.totals.scans === 1 && usageWindow.totals.credits === 1, "usage summary totals")
                    smoke.shotItem(usageWindow.content, "11-usage", 520)
                }
                break
            case 520:
                // Narrow the window: every column must still fit inside the table.
                usageWindow.width = usageWindow.minimumWidth
                smoke.step = 521
                break
            case 521:
                smoke.shotItem(usageWindow.content, "11-usage-narrow", 53)
                break
            case 53:
                smoke.check(usageWindow.rowWidth > 0 && usageWindow.columnX(5) <= usageWindow.rowWidth + 0.5,
                            "usage columns fit the table")
                usageWindow.close()
                settingsDialog.open()
                smoke.shot("12-settings-full", 54)
                break
            case 54:
                settingsDialog.close()
                backend.setTrayEnabled(true)
                smoke.step = 55
                break
            // Tray: a tray host exists only on a live desktop, so offscreen this drives the window
            // functions directly. Hiding the only window must not quit the application.
            case 55:
                if (backend.minimizeToTray) {
                    smoke.log("tray available: " + trayIcon.available)
                    if (trayIcon.available)
                        window.close()
                    else
                        window.hideToTray()
                    smoke.step = 56
                }
                break
            case 56:
                smoke.check(!window.visible, "window hides to the tray")
                window.showFromTray()
                smoke.step = 57
                break
            case 57:
                smoke.check(window.visible && window.visibility === Window.Windowed, "window returns from the tray")
                backend.setTrayEnabled(false)
                smoke.step = 58
                break
            case 58:
                if (!backend.minimizeToTray) {
                    smoke.check(!window.trayActive, "tray setting turns off")
                    smoke.step = 6
                }
                break
            case 6:
                window.width = 820
                smoke.step = 7
                break
            case 7:
                smoke.check(!window.wide, "narrow layout")
                smoke.shot("05-narrow", 8)
                break
            case 8:
                window.width = 1280
                smoke.step = 80
                break
            case 80:
                window.editAndRescan()
                smoke.check(window.mode === "draft" && documentPane.draftText === smoke.sample, "edit and rescan copies input")
                smoke.check(window.draftSourceId === smoke.firstId, "rescan remembers its source")
                smoke.shot("06-rescan", 81)
                break
            case 81:
                window.width = window.minimumWidth
                smoke.step = 82
                break
            case 82:
                smoke.check(window.historyVisible && smoke.fitsWindow(documentPane.checkButton),
                            "Check for AI stays visible at the minimum width")
                smoke.shot("06b-narrow-draft", 9)
                break
            case 9:
                documentPane.setDraft(smoke.htmlTitle, "This should FAIL.")
                window.analyze()
                smoke.step = 10
                break
            case 10:
                if (scan && scan.state === "failed") {
                    smoke.failedId = scan.id
                    smoke.check(scan.error.indexOf("mock failure") >= 0, "failure message shown")
                    const last = smoke.finishedScans[smoke.finishedScans.length - 1]
                    smoke.check(last.state === "failed" && last.ai === -1, "failure signalled")
                    smoke.check(documentPane.scanToolbar.height < 120, "markup in a title is shown as text ("
                                + documentPane.scanToolbar.height + " px toolbar)")
                    smoke.check(smoke.fitsWindow(documentPane.rescanButton), "Edit and rescan stays visible at the minimum width")
                    smoke.shot("07-failed", 11)
                }
                break
            case 11:
                window.width = 1280
                smoke.historyBefore = window.history.length
                window.openScan(smoke.firstId)
                smoke.step = 12
                break
            case 12:
                if (scan && scan.id === smoke.firstId && window.currentResult) {
                    smoke.check(!backend.busy, "reopening history does not start a scan")
                    smoke.check(window.history.length === smoke.historyBefore, "history unchanged by reopening")
                    backend.deleteScan(smoke.failedId)
                    smoke.step = 13
                }
                break
            case 13:
                if (window.history.length === smoke.historyBefore - 1) {
                    smoke.check(!window.history.some(h => h.id === smoke.failedId), "deleted scan removed")
                    smoke.step = 20
                }
                break
            // Keyboard-only flow.
            case 20:
                window.requestActivate()
                smoke.step = 200
                break
            case 200:
                keys.keySequence("Ctrl+N")
                keys.keyClickChar("x", Qt.NoModifier, -1)
                const typed = documentPane.draftText === "x"
                documentPane.clearDraft()
                if (window.mode !== "draft" || !typed) {
                    // On a live desktop the compositor may refuse keyboard focus to a test window.
                    smoke.log("SKIP keyboard checks: key events are not reaching the window")
                    smoke.step = 22
                    break
                }
                smoke.check(documentPane.draftEditor.activeFocus, "Ctrl+N opens a focused draft")
                smoke.type("Typed by keyboard. More words follow here! Last one.")
                smoke.check(documentPane.draftText === "Typed by keyboard. More words follow here! Last one.", "typing into the draft")
                keys.keyClick(Qt.Key_Tab, Qt.NoModifier, -1)
                smoke.check(!documentPane.draftEditor.activeFocus && documentPane.draftText.indexOf("\t") < 0,
                            "Tab leaves the editor without inserting a tab")
                documentPane.focusEditor()
                keys.keySequence("Ctrl+Return")
                smoke.check(window.mode === "scan", "Ctrl+Enter submits")
                smoke.step = 21
                break
            case 21:
                if (scan && scan.state === "completed" && window.currentResult && scan.inputText.indexOf("Typed by keyboard") === 0) {
                    keys.keySequence("Ctrl+F")
                    smoke.check(documentPane.findBarVisible, "Ctrl+F opens find")
                    smoke.type("more words")
                    keys.keyClick(Qt.Key_Return, Qt.NoModifier, -1)
                    smoke.check(documentPane.resultEditor.selectedText === "More words", "find selects the match")
                    keys.keyClick(Qt.Key_Escape, Qt.NoModifier, -1)
                    smoke.check(!documentPane.findBarVisible && documentPane.resultEditor.activeFocus, "Escape closes find")
                    // Tab from the text to the section list, then inspect with arrow keys.
                    const list = resultsPane.sectionListView
                    for (let i = 0; i < 12 && !list.activeFocus; i++)
                        keys.keyClick(Qt.Key_Tab, Qt.NoModifier, -1)
                    smoke.check(list.activeFocus, "Tab reaches the section list")
                    const before = window.selectedSection
                    keys.keyClick(Qt.Key_Down, Qt.NoModifier, -1)
                    smoke.check(window.selectedSection === before + 1 && window.selectedSection >= 0, "arrow keys select sections")
                    const s = window.currentResult.sections[window.selectedSection]
                    smoke.check(plain(documentPane.resultEditor.selectedText) === window.currentResult.displayText.substring(s.start, s.end),
                                "keyboard section selection highlights its text")
                    smoke.shot("09-keyboard", 22)
                }
                break
            case 22:
                {
                    // Long document: ~100k characters, hundreds of sections.
                    let long = ""
                    for (let i = 0; long.length < 100000; i++)
                        long += "Sentence number " + i + " talks about “things” 😀 at some length. "
                              + (i % 7 === 0 ? "\n\n" : "")
                    window.showDraft()
                    documentPane.setDraft("Long document", long)
                    window.analyze()
                    smoke.step = 14
                }
                break
            case 14:
                if (scan && scan.title === "Long document" && scan.state === "completed" && window.currentResult) {
                    const r = window.currentResult
                    let t = Date.now()
                    backend.resultHtml("#ff0000", "#00ff00", "#0000ff")
                    const htmlMs = Date.now() - t
                    t = Date.now()
                    for (let i = 0; i < 1000; i++)
                        backend.sectionAt(Math.floor(i * r.displayLen / 1000))
                    const lookupMs = Date.now() - t
                    t = Date.now()
                    documentPane.selectSection(r.sections.length - 1)
                    const selectMs = Date.now() - t
                    smoke.log("long document: " + r.displayLen + " chars, " + r.sections.length + " sections; html "
                              + htmlMs + " ms, 1000 lookups " + lookupMs + " ms, select last " + selectMs + " ms")
                    smoke.check(documentPane.positionsReliable, "long document positions match")
                    smoke.check(htmlMs < 500 && lookupMs < 200 && selectMs < 500, "long document stays responsive")
                    smoke.shot("08-long", 15)
                }
                break
            case 15:
                smoke.finish()
                smoke.step = 99
                break
            }
        }
    }
}
