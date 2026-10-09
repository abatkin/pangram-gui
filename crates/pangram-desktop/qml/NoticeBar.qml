import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Inline message for errors and confirmations. Info messages hide themselves.
Rectangle {
    id: root
    visible: false
    implicitHeight: visible ? row.implicitHeight + 12 : 0
    // Opaque: the tint is blended into the window colour.
    color: Qt.tint(palette.window, Qt.alpha(accent, 0.15))

    property string level: "info"
    property string message: ""
    readonly property color accent: level === "error" ? window.errorColor
                                    : level === "warning" ? window.warningColor
                                    : palette.highlight

    Accessible.role: Accessible.AlertMessage
    Accessible.name: message

    function show(l, m) {
        level = l
        message = m
        visible = true
        if (l === "info")
            hideTimer.restart()
        else
            hideTimer.stop()
    }

    Timer {
        id: hideTimer
        interval: 6000
        onTriggered: root.visible = false
    }

    Rectangle {
        width: 4
        height: parent.height
        color: root.accent
    }

    RowLayout {
        id: row
        anchors.fill: parent
        anchors.leftMargin: 14
        anchors.rightMargin: 6
        spacing: 8

        Label {
            Layout.fillWidth: true
            text: root.message
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Button {
            text: qsTr("Copy")
            flat: true
            visible: root.level !== "info"
            onClicked: window.copyText(root.message)
        }
        ToolButton {
            icon.name: "window-close"
            text: qsTr("Dismiss")
            display: AbstractButton.IconOnly
            onClicked: root.visible = false
            Accessible.name: qsTr("Dismiss message")
        }
    }
}
