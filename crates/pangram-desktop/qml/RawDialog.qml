import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// The current scan's request and responses as exchanged with Pangram, formatted as JSON.
Dialog {
    id: root
    title: qsTr("Raw request and response")
    modal: true
    width: Math.min(900, (parent ? parent.width : 900) - 32)
    height: Math.min(720, (parent ? parent.height : 720) - 32)

    property var details: ({})
    readonly property var pages: [
        { name: qsTr("Request"), key: "request",
          note: qsTr("The body is exactly what was sent. The API key is never shown.") },
        { name: qsTr("Submission response"), key: "submitResponse",
          note: qsTr("Pangram's reply to the submission: the task ID and any notices.") },
        { name: qsTr("Result"), key: "response",
          note: qsTr("The final polling response, which contains the analysis.") }
    ]
    readonly property string currentText: details[pages[tabs.currentIndex].key] || ""

    onAboutToShow: {
        const raw = backend.rawDetails()
        details = raw.length > 0 ? JSON.parse(raw) : {}
        tabs.currentIndex = details.response && details.response !== "No final result yet." ? 2 : 0
    }
    onOpened: view.forceActiveFocus()

    ColumnLayout {
        anchors.fill: parent
        spacing: 8

        TabBar {
            id: tabs
            Layout.fillWidth: true
            Repeater {
                model: root.pages
                TabButton {
                    required property var modelData
                    text: modelData.name
                    width: implicitWidth
                }
            }
        }

        Label {
            textFormat: Text.PlainText
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            opacity: 0.75
            text: root.pages[tabs.currentIndex].note
        }

        Flickable {
            id: flick
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            contentWidth: width
            contentHeight: view.implicitHeight
            boundsBehavior: Flickable.StopAtBounds
            ScrollBar.vertical: ScrollBar {}

            TextArea.flickable: TextArea {
                id: view
                readOnly: true
                selectByMouse: true
                selectByKeyboard: true
                textFormat: TextEdit.PlainText
                wrapMode: TextEdit.WrapAtWordBoundaryOrAnywhere
                font.family: "monospace"
                text: root.currentText
                Accessible.name: root.pages[tabs.currentIndex].name
            }
        }

        ButtonRow {
            Button {
                text: qsTr("Copy")
                onClicked: window.copyText(root.currentText)
            }
            Button {
                text: qsTr("Close")
                onClicked: root.close()
            }
        }
    }
}
