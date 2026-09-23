import QtQuick

// Base type for every stacked view in the panel.
//
// Provides the context every view needs (shell for navigation + backend,
// theme for colors), the slide/fade transition between views, and the
// active gate: an inactive view is disabled, so only the current view ever
// takes input even though all views stay instantiated (which is what lets
// every view keep its Connections to the backend alive).
//
// Concrete views declare their children normally; they land in `body`.
Item {
    id: root

    property var shell
    property var theme
    property bool active: false

    default property alias content: body.data

    opacity: active ? 1 : 0
    x: active ? 0 : 16
    enabled: active

    Behavior on opacity {
        NumberAnimation {
            duration: 160
            easing.type: Easing.OutCubic
        }
    }

    Behavior on x {
        NumberAnimation {
            duration: 160
            easing.type: Easing.OutCubic
        }
    }

    Rectangle {
        id: body
        anchors.fill: parent
        color: "transparent"
    }
}
