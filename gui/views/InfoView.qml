import QtQuick

// Read-only manifest view, fed by the real `info <name>` operation: every
// section of profile.toml rendered as-is — profile table, packages,
// services, files, and the open rice_info table.
PanelView {
    id: view

    property string arg: ""
    property var manifest: null
    property string infoError: ""
    property bool loading: false

    readonly property var profileMeta: manifest !== null && manifest.profile ? manifest.profile : null
    readonly property var packageInfo: manifest !== null && manifest.packages ? manifest.packages : null
    readonly property var services: manifest !== null && manifest.services ? manifest.services : []
    readonly property var files: manifest !== null && manifest.files ? manifest.files : []
    readonly property var riceInfo: manifest !== null && manifest.rice_info ? manifest.rice_info : ({})

    function load() {
        infoError = "";
        manifest = null;
        if (shell.backend.runInfo(arg)) {
            loading = true;
        } else {
            infoError = "another operation is already running";
        }
    }

    onEnabledChanged: {
        if (enabled && arg !== "")
            load();
    }

    Connections {
        target: view.shell ? view.shell.backend : null
        function onOperationFinished(operation, ok, envelope) {
            if (operation !== "info")
                return;
            view.loading = false;
            if (!ok) {
                view.infoError = envelope && envelope.data && envelope.data.error ? String(envelope.data.error) : "cannot read this manifest";
                view.manifest = null;
                return;
            }
            view.infoError = "";
            view.manifest = envelope.data && envelope.data.manifest ? envelope.data.manifest : null;
        }
    }

    function display(value) {
        if (value === null || value === undefined)
            return "—";
        if (typeof value === "string")
            return value.length > 0 ? value : "—";
        return JSON.stringify(value);
    }

    // Reusable label/value pair for the manifest's scalar fields.
    component Field: Column {
        property string label: ""
        property string value: ""
        width: fieldColumn.width
        spacing: 2

        Text {
            text: parent.label
            color: view.theme.muted
            font.pixelSize: 11
        }

        Text {
            width: parent.width
            wrapMode: Text.WordWrap
            text: parent.value
            color: view.theme.foreground
            font.pixelSize: 13
        }

        Item {
            width: 1
            height: 6
        }
    }

    component Heading: Text {
        text: ""
        color: view.theme.accent
        font.pixelSize: 13
        font.bold: true
        topPadding: 10
        bottomPadding: 4
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

            Rectangle {
                id: backBtn
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                width: 30
                height: 28
                radius: 6
                color: backHover.hovered ? view.theme.surfaceAlt : "transparent"

                Text {
                    anchors.centerIn: parent
                    text: "←"
                    color: view.theme.foreground
                    font.pixelSize: 15
                }

                HoverHandler {
                    id: backHover
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: view.shell.pop()
                }
            }

            Text {
                anchors.left: backBtn.right
                anchors.leftMargin: 10
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                elide: Text.ElideRight
                text: view.arg.length > 0 ? view.arg : "Profile"
                color: view.theme.foreground
                font.pixelSize: 20
                font.bold: true
            }
        }

        Text {
            width: parent.width
            visible: view.loading
            text: "Reading manifest…"
            color: view.theme.muted
            font.pixelSize: 12
        }

        Text {
            width: parent.width
            visible: view.infoError !== ""
            wrapMode: Text.WordWrap
            text: view.infoError
            color: view.theme.danger
            font.pixelSize: 12
        }
    }

    // ------------------------------------------------------------------
    // Manifest body (read-only)
    // ------------------------------------------------------------------

    Flickable {
        anchors.top: header.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.leftMargin: 18
        anchors.rightMargin: 18
        anchors.bottomMargin: 18
        contentWidth: width
        contentHeight: fieldColumn.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        visible: view.manifest !== null

        Column {
            id: fieldColumn
            width: parent.width
            spacing: 2

            Heading {
                text: "Profile"
            }

            Field {
                label: "Name"
                value: view.profileMeta ? view.display(view.profileMeta.name) : "—"
            }

            Field {
                label: "Description"
                value: view.profileMeta ? view.display(view.profileMeta.description) : "—"
            }

            Field {
                label: "Created"
                value: view.profileMeta ? view.display(view.profileMeta.created_at) : "—"
            }

            Field {
                label: "Updated"
                value: view.profileMeta ? view.display(view.profileMeta.updated_at) : "—"
            }

            Field {
                label: "Screenshot"
                value: view.profileMeta ? view.display(view.profileMeta.screenshot) : "—"
            }

            Field {
                visible: view.profileMeta !== null && view.profileMeta.source_url !== null && view.profileMeta.source_url !== undefined
                label: "Source"
                value: {
                    if (!view.profileMeta)
                        return "—";
                    const url = view.profileMeta.source_url ? String(view.profileMeta.source_url) : "";
                    const commit = view.profileMeta.source_commit ? " @ " + String(view.profileMeta.source_commit) : "";
                    return (url.length > 0 ? url : "—") + commit;
                }
            }

            Heading {
                text: "Packages (" + (view.packageInfo ? (view.packageInfo.official.length + view.packageInfo.aur.length) : 0) + ")"
            }

            Field {
                label: "Official"
                value: view.packageInfo && view.packageInfo.official.length > 0 ? view.packageInfo.official.join(", ") : "none"
            }

            Field {
                label: "AUR"
                value: view.packageInfo && view.packageInfo.aur.length > 0 ? view.packageInfo.aur.join(", ") : "none"
            }

            Heading {
                visible: view.services.length > 0
                text: "Services (" + view.services.length + ")"
            }

            Repeater {
                model: view.services
                delegate: Column {
                    width: fieldColumn.width
                    spacing: 2

                    Text {
                        text: modelData.name
                        color: view.theme.foreground
                        font.pixelSize: 13
                        font.bold: true
                    }

                    Text {
                        width: parent.width
                        wrapMode: Text.WrapAnywhere
                        text: "start: " + modelData.start
                        color: view.theme.muted
                        font.pixelSize: 11
                        font.family: "monospace"
                    }

                    Text {
                        width: parent.width
                        wrapMode: Text.WrapAnywhere
                        text: "stop:  " + modelData.stop
                        color: view.theme.muted
                        font.pixelSize: 11
                        font.family: "monospace"
                    }

                    Item {
                        width: 1
                        height: 6
                    }
                }
            }

            Heading {
                visible: view.files.length > 0
                text: "Files (" + view.files.length + ")"
            }

            Repeater {
                model: view.files
                delegate: Text {
                    width: fieldColumn.width
                    wrapMode: Text.WrapAnywhere
                    text: modelData.path + (modelData.optional ? "  (optional)" : "")
                    color: view.theme.foreground
                    font.pixelSize: 12
                    font.family: "monospace"
                }
            }

            Heading {
                visible: Object.keys(view.riceInfo).length > 0
                text: "Rice info"
            }

            Repeater {
                model: Object.keys(view.riceInfo)
                delegate: Item {
                    width: fieldColumn.width
                    height: riceRow.implicitHeight + 4

                    Row {
                        id: riceRow
                        spacing: 8

                        Text {
                            width: 96
                            text: modelData
                            color: view.theme.muted
                            font.pixelSize: 12
                        }

                        Text {
                            width: fieldColumn.width - 104
                            wrapMode: Text.WordWrap
                            text: view.display(view.riceInfo[modelData])
                            color: view.theme.foreground
                            font.pixelSize: 12
                        }
                    }
                }
            }

            Item {
                width: 1
                height: 16
            }
        }
    }
}
