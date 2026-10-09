import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ToolBar {
    id: root
    visible: false
    topPadding: 4
    bottomPadding: 4

    property bool found: true

    signal findRequested(string query, bool backwards)
    signal closed()

    function open() {
        visible = true
        field.forceActiveFocus()
        field.selectAll()
    }

    function close() {
        visible = false
        closed()
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 8
        spacing: 6

        Label { text: qsTr("Find") }
        TextField {
            id: field
            Layout.fillWidth: true
            Accessible.name: qsTr("Find text")
            placeholderText: qsTr("Search the document")
            onTextChanged: {
                root.found = true
                if (text.length > 0)
                    root.findRequested(text, false)
            }
            Keys.onReturnPressed: event => root.findRequested(text, (event.modifiers & Qt.ShiftModifier) !== 0)
            Keys.onEnterPressed: event => root.findRequested(text, (event.modifiers & Qt.ShiftModifier) !== 0)
            Keys.onEscapePressed: root.close()
        }
        Label {
            textFormat: Text.PlainText
            visible: !root.found
            text: qsTr("Not found")
            color: window.errorColor
        }
        ToolButton {
            text: qsTr("Previous")
            icon.name: "go-up"
            enabled: field.text.length > 0
            onClicked: root.findRequested(field.text, true)
        }
        ToolButton {
            text: qsTr("Next")
            icon.name: "go-down"
            enabled: field.text.length > 0
            onClicked: root.findRequested(field.text, false)
        }
        ToolButton {
            text: qsTr("Close")
            icon.name: "window-close"
            display: AbstractButton.IconOnly
            onClicked: root.close()
            Accessible.name: qsTr("Close find bar")
        }
    }
}
