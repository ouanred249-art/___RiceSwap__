import QtQuick

// The Profiles home: cards rendered from the real `list` operation, the
// Active badge driven by state.json (backend.activeProfile, watched — it
// stays truthful without a re-list), the ⋯ menu overlay, the in-panel
// delete confirmation running the real `delete` operation, and the empty
// state that leads into the snapshot flow.
PanelView {
    id: view

    property var profiles: []
    property var listWarnings: []
    property string listError: ""
    property bool loading: false

    // Which card's ⋯ menu is open, plus the button coordinates (view
    // space) the overlay is placed at.
    property string openMenu: ""
    property real menuX: 0
    property real menuY: 0

    property string confirmTarget: ""
    property string confirmError: ""
    property bool confirming: false

    readonly property string activeBadge: shell.backend.activeProfile

    function refresh() {
        if (shell.backend.runList())
            loading = true;
    }

    function findProfile(name) {
        for (var i = 0; i < profiles.length; i++) {
            if (profiles[i].name === name)
                return profiles[i];
        }
        return null;
    }

    function shotPathFor(entry) {
        const shot = entry.manifest && entry.manifest.profile && entry.manifest.profile.screenshot ? String(entry.manifest.profile.screenshot) : "";
        const file = shot.length > 0 ? shot : "screenshot.png";
        return shell.backend.home + "/.local/share/riceswap/profiles/" + entry.name + "/" + file;
    }

    function handleMenu(name, action) {
        openMenu = "";
        if (action === "switch") {
            // Navigation only: the switch flow itself lands in ticket #19.
            shell.push("switch", name);
        } else if (action === "info") {
            shell.push("info", name);
        } else if (action === "refresh") {
            const entry = findProfile(name);
            if (entry)
                shell.backend.refreshScreenshot(shotPathFor(entry));
        } else if (action === "delete") {
            confirmTarget = name;
            confirmError = "";
        }
    }

    Component.onCompleted: refresh()

    onEnabledChanged: {
        if (enabled) {
            refresh();
        } else {
            openMenu = "";
            confirmTarget = "";
            confirming = false;
        }
    }

    Connections {
        target: view.shell ? view.shell.backend : null
        function onOperationFinished(operation, ok, envelope) {
            if (operation === "list") {
                view.loading = false;
                if (!ok) {
                    view.listError = envelope && envelope.data && envelope.data.error ? String(envelope.data.error) : "cannot read the profile store";
                    return;
                }
                view.listError = "";
                view.profiles = envelope.data && envelope.data.profiles ? envelope.data.profiles : [];
                view.listWarnings = envelope.warnings ? envelope.warnings : [];
            } else if (operation === "delete") {
                view.confirming = false;
                if (!ok) {
                    // e.g. the active-profile refusal — shown in the dialog.
                    view.confirmError = envelope && envelope.data && envelope.data.error ? String(envelope.data.error) : "delete failed";
                    return;
                }
                view.confirmTarget = "";
                view.confirmError = "";
                view.refresh();
            }
        }
    }

    // ------------------------------------------------------------------
    // Header
    // ------------------------------------------------------------------

    Column {
        id: header
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.margins: 18
        spacing: 6

        Item {
            width: parent.width
            height: 30

            Text {
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                text: "Profiles"
                color: view.theme.foreground
                font.pixelSize: 20
                font.bold: true
            }

            Rectangle {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                width: 28
                height: 28
                radius: 6
                color: closeHover.hovered ? view.theme.surfaceAlt : "transparent"

                Text {
                    anchors.centerIn: parent
                    text: "×"
                    color: view.theme.muted
                    font.pixelSize: 16
                }

                HoverHandler {
                    id: closeHover
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: view.shell.close()
                }
            }
        }

        Text {
            width: parent.width
            visible: view.loading && view.profiles.length === 0
            text: "Loading the profile store…"
            color: view.theme.muted
            font.pixelSize: 12
        }

        Text {
            width: parent.width
            visible: view.listError !== ""
            wrapMode: Text.WordWrap
            text: view.listError
            color: view.theme.danger
            font.pixelSize: 12
        }

        Text {
            width: parent.width
            visible: view.listError !== ""
            text: "Retry"
            color: view.theme.accent
            font.pixelSize: 12
            font.bold: true

            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: view.refresh()
            }
        }

        Text {
            width: parent.width
            visible: view.shell.backend.shotError !== ""
            wrapMode: Text.WordWrap
            text: "Screenshot refresh failed: " + view.shell.backend.shotError
            color: view.theme.danger
            font.pixelSize: 11
        }

        Repeater {
            model: view.listWarnings
            delegate: Text {
                width: header.width
                wrapMode: Text.WordWrap
                text: modelData
                color: view.theme.muted
                font.pixelSize: 11
            }
        }
    }

    // ------------------------------------------------------------------
    // Card list
    // ------------------------------------------------------------------

    Flickable {
        id: list
        anchors.top: header.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.leftMargin: 16
        anchors.rightMargin: 16
        anchors.bottomMargin: 16
        contentWidth: width
        contentHeight: cards.implicitHeight + 8
        clip: true
        boundsBehavior: Flickable.StopAtBounds

        // Clicking the empty part of the list closes an open ⋯ menu.
        MouseArea {
            anchors.fill: parent
            z: 0
            enabled: view.openMenu !== ""
            onClicked: view.openMenu = ""
        }

        Column {
            id: cards
            z: 1
            width: list.width
            spacing: 12

            Repeater {
                model: view.profiles

                delegate: ProfileCard {
                    width: cards.width
                    shell: view.shell
                    theme: view.theme
                    viewRoot: view
                    profileName: modelData.name
                    manifest: modelData.manifest
                    isActive: view.activeBadge === modelData.name
                    menuOpen: view.openMenu === modelData.name

                    onActivateMenu: (name, x, y) => {
                        if (view.openMenu === name) {
                            view.openMenu = "";
                        } else {
                            view.openMenu = name;
                            view.menuX = x;
                            view.menuY = y;
                        }
                    }
                    onCardPressed: view.openMenu = ""
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Empty state → snapshot flow (stubbed to ticket #18)
    // ------------------------------------------------------------------

    Column {
        anchors.centerIn: parent
        visible: view.profiles.length === 0 && !view.loading && view.listError === ""
        spacing: 14
        z: 2

        Text {
            width: view.width - 72
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            text: "No profiles yet — snapshot your current desktop"
            color: view.theme.foreground
            font.pixelSize: 15
        }

        Rectangle {
            width: snapshotLabel.implicitWidth + 36
            height: 42
            radius: 9
            anchors.horizontalCenter: parent.horizontalCenter
            color: view.theme.surface
            border.width: 1
            border.color: view.theme.accent

            Text {
                id: snapshotLabel
                anchors.centerIn: parent
                text: "Open snapshot flow"
                color: view.theme.accent
                font.pixelSize: 14
                font.bold: true
            }

            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: view.shell.push("snapshot", null)
            }
        }
    }

    // ------------------------------------------------------------------
    // ⋯ menu overlay (rendered above the list so it can never be clipped
    // by the Flickable)
    // ------------------------------------------------------------------

    Rectangle {
        id: menu
        visible: view.openMenu !== ""
        z: 5
        width: 196
        height: menuColumn.implicitHeight + 12
        x: Math.max(8, Math.min(view.menuX - width + 30, view.width - width - 8))
        y: Math.max(8, Math.min(view.menuY + 4, view.height - height - 8))
        radius: 8
        color: view.theme.surfaceAlt
        border.width: 1
        border.color: view.theme.border

        Column {
            id: menuColumn
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: 6
            spacing: 0

            Repeater {
                model: [
                    {
                        label: "Switch",
                        action: "switch",
                        danger: false
                    },
                    {
                        label: "Info",
                        action: "info",
                        danger: false
                    },
                    {
                        label: "Refresh screenshot",
                        action: "refresh",
                        danger: false
                    },
                    {
                        label: "Delete",
                        action: "delete",
                        danger: true
                    }
                ]

                delegate: Rectangle {
                    width: menuColumn.width
                    height: 32
                    radius: 5
                    color: rowHover.hovered ? (modelData.danger ? Qt.rgba(1, 0.3, 0.3, 0.16) : Qt.rgba(1, 1, 1, 0.08)) : "transparent"

                    Text {
                        anchors.left: parent.left
                        anchors.leftMargin: 10
                        anchors.verticalCenter: parent.verticalCenter
                        text: modelData.label
                        color: modelData.danger ? view.theme.danger : view.theme.foreground
                        font.pixelSize: 13
                    }

                    HoverHandler {
                        id: rowHover
                    }

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: view.handleMenu(view.openMenu, modelData.action)
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // In-panel delete confirmation (real `delete`, never --force)
    // ------------------------------------------------------------------

    Rectangle {
        id: confirm
        visible: view.confirmTarget !== ""
        z: 10
        anchors.fill: parent
        color: Qt.rgba(0, 0, 0, 0.55)

        // Modal: every click lands here, not on the list behind.
        MouseArea {
            anchors.fill: parent
        }

        Rectangle {
            anchors.centerIn: parent
            width: parent.width - 44
            height: confirmColumn.implicitHeight + 32
            radius: 10
            color: view.theme.surface
            border.width: 1
            border.color: view.theme.border

            Column {
                id: confirmColumn
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.margins: 16
                spacing: 10

                Text {
                    text: "Delete this profile?"
                    color: view.theme.foreground
                    font.pixelSize: 16
                    font.bold: true
                }

                Text {
                    width: parent.width
                    wrapMode: Text.WordWrap
                    text: "“" + view.confirmTarget + "” will be removed from the profile store. This cannot be undone."
                    color: view.theme.muted
                    font.pixelSize: 13
                }

                Text {
                    width: parent.width
                    visible: view.confirmError !== ""
                    wrapMode: Text.WordWrap
                    text: view.confirmError
                    color: view.theme.danger
                    font.pixelSize: 12
                }

                Row {
                    spacing: 10

                    Rectangle {
                        width: cancelLabel.implicitWidth + 28
                        height: 38
                        radius: 8
                        color: view.theme.surfaceAlt
                        border.width: 1
                        border.color: view.theme.border

                        Text {
                            id: cancelLabel
                            anchors.centerIn: parent
                            text: "Cancel"
                            color: view.theme.foreground
                            font.pixelSize: 14
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                view.confirmTarget = "";
                                view.confirmError = "";
                            }
                        }
                    }

                    Rectangle {
                        width: deleteLabel.implicitWidth + 28
                        height: 38
                        radius: 8
                        color: view.confirming ? view.theme.surfaceAlt : view.theme.danger

                        Text {
                            id: deleteLabel
                            anchors.centerIn: parent
                            text: view.confirming ? "Deleting…" : "Delete"
                            color: view.confirming ? view.theme.muted : "#ffffffff"
                            font.pixelSize: 14
                            font.bold: true
                        }

                        MouseArea {
                            anchors.fill: parent
                            enabled: !view.confirming
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                if (view.shell.backend.runDelete(view.confirmTarget)) {
                                    view.confirming = true;
                                    view.confirmError = "";
                                } else {
                                    view.confirmError = "another operation is already running";
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
