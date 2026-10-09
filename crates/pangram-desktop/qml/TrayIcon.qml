import QtQuick
import Qt.labs.platform as Platform

// System tray icon, shown while "Minimize to the system tray" is on. Uses QSystemTrayIcon
// (StatusNotifierItem on Linux), so `available` is false without a tray host.
Item {
    id: root

    readonly property alias available: tray.available
    // ID of the displayed scan while it runs, to notice when it finishes.
    property string runningId: ""

    Platform.SystemTrayIcon {
        id: tray

        visible: backend.minimizeToTray
        // The installed theme icon; the embedded copy covers uninstalled builds.
        icon.name: "net.batkin.pangram-desktop"
        icon.source: "qrc:/net/batkin/pangram/app-icon.svg"
        tooltip: backend.busy ? qsTr("Pangram: scan in progress") : qsTr("Pangram")

        onActivated: reason => {
            if (reason === Platform.SystemTrayIcon.Trigger)
                window.toggleFromTray()
        }
        onMessageClicked: window.showFromTray()

        menu: Platform.Menu {
            // With QApplication this is a widget QMenu, and `visible` (default true) pops it up
            // at the window's corner when the tray icon appears. The tray only exports its items.
            visible: false

            Platform.MenuItem {
                text: window.visible ? qsTr("Hide Pangram") : qsTr("Show Pangram")
                onTriggered: window.visible ? window.hideToTray() : window.showFromTray()
            }
            Platform.MenuItem {
                text: qsTr("New check")
                onTriggered: {
                    window.showFromTray()
                    window.newCheck()
                }
            }
            Platform.MenuSeparator {}
            Platform.MenuItem {
                text: qsTr("Quit")
                onTriggered: window.quit()
            }
        }
    }

    // Tells the user about a scan that finished while the window was hidden. The message holds no
    // document text or title, since notification daemons keep a history.
    Connections {
        target: backend
        function onCurrentRevisionChanged() {
            const scan = window.currentScan
            const running = !!scan && (scan.state === "submitting" || scan.state === "polling")
            const finished = !!scan && !running && scan.id === root.runningId
            root.runningId = running ? scan.id : ""
            if (!finished || window.visible || !tray.visible || !tray.supportsMessages)
                return
            if (scan.state === "completed" && scan.result)
                tray.showMessage(qsTr("Scan complete"),
                                 qsTr("AI-generated %1, AI-assisted %2, human-written %3")
                                     .arg(window.percent(scan.result.fractionAi))
                                     .arg(window.percent(scan.result.fractionAiAssisted))
                                     .arg(window.percent(scan.result.fractionHuman)))
            else
                tray.showMessage(qsTr("Scan didn't complete"),
                                 qsTr("%1. Open Pangram for details.").arg(window.stateText(scan.state)),
                                 Platform.SystemTrayIcon.Warning)
        }
    }
}
