import QtQuick

// Destination for stack entries whose flow lands in a later ticket:
// Switch Confirm / Progress / Result (ticket #19) and the Snapshot
// checklist (ticket #18). This ticket only wires the navigation to them.
PanelView {
    id: view

    property string title: ""
    property string subtitle: ""
    property string note: ""

    Column {
        anchors.centerIn: parent
        width: parent.width - 56
        spacing: 12

        Text {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            text: view.title
            color: view.theme.foreground
            font.pixelSize: 20
            font.bold: true
        }

        Text {
            width: parent.width
            visible: view.subtitle !== ""
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            text: view.subtitle
            color: view.theme.muted
            font.pixelSize: 13
        }

        Text {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            lineHeight: 1.3
            text: view.note
            color: view.theme.muted
            font.pixelSize: 12
        }

        Rectangle {
            width: backLabel.implicitWidth + 32
            height: 40
            radius: 8
            anchors.horizontalCenter: parent.horizontalCenter
            color: view.theme.surface
            border.width: 1
            border.color: view.theme.accent

            Text {
                id: backLabel
                anchors.centerIn: parent
                text: "Back"
                color: view.theme.accent
                font.pixelSize: 14
                font.bold: true
            }

            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: view.shell.pop()
            }
        }
    }
}
