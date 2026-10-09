import QtQuick
import QtQuick.Layouts

// Right-aligned dialog buttons, placed inside the dialog's content so they line up with it.
RowLayout {
    default property alias buttons: inner.data
    Layout.fillWidth: true
    Layout.topMargin: 8
    spacing: 8

    Item { Layout.fillWidth: true }
    RowLayout {
        id: inner
        spacing: 8
    }
}
