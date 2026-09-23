import QtQuick

// The snapshot flow, fully in-panel: one scrolling page with a name field on
// top and grouped collapsible sections (Configs / Packages / Fonts & assets)
// of pre-checked toggle chips fed by the real `detect` operation. Create
// Profile runs `snapshot` with the confirmed selections and progress; a name
// collision renders as an inline error under the field; Downloads
// wallpaper-import candidates are offered inline; success returns to
// Profiles showing the new card. Zero terminal usage — everything is a
// subprocess the backend object spawns.
PanelView {
    id: view

    // The real `detect` results.
    property var detectData: null
    property bool detecting: false
    property string detectError: ""

    // Confirmed selections. Each entry is a chip label; a chip present in
    // the array is checked, absent is unchecked. Labels carry a group
    // prefix so the arrays never collide across groups.
    property var selectedConfigs: []
    property var selectedPackages: []
    property var selectedAssets: []

    // Collapsible sections: a group collapsed after first expand stays
    // collapsed; a fresh detect re-opens everything.
    property bool configsOpen: true
    property bool packagesOpen: true
    property bool assetsOpen: true

    // The profile name and its inline error.
    property string profileName: ""
    property string nameError: ""

    // Creating state and result.
    readonly property bool creating: shell !== null && shell.backend.busy && shell.backend.runningOp === "snapshot"
    property var snapshotWarnings: []
    property string snapshotError: ""

    // Wallpaper-import candidates from detect's `wallpapers` list, each
    // with an "Import" button.
    readonly property var wallpaperCandidates: view.detectData && view.detectData.wallpapers ? view.detectData.wallpapers : []
    property var importedWallpapers: []

    function resetForm() {
        view.selectedConfigs = [];
        view.selectedPackages = [];
        view.selectedAssets = [];
        view.nameError = "";
        view.snapshotError = "";
        view.snapshotWarnings = [];
    }

    function detect() {
        if (view.shell.backend.runDetect())
            view.detecting = true;
    }

    function chipChecked(group, label) {
        if (group === "configs")
            return view.selectedConfigs.indexOf(label) >= 0;
        if (group === "packages")
            return view.selectedPackages.indexOf(label) >= 0;
        return view.selectedAssets.indexOf(label) >= 0;
    }

    function toggleChip(group, label) {
        var list = group === "configs" ? view.selectedConfigs : group === "packages" ? view.selectedPackages : view.selectedAssets;
        var index = list.indexOf(label);
        if (index >= 0)
            list.splice(index, 1);
        else
            list.push(label);
    }

    // All chip labels in the right shape for the `snapshot` operation's
    // confirmed selections. detect's `config_dirs` are home-relative paths
    // like ".config/waybar"; `packages` is the official/aur split; `assets`
    // are home-relative like ".fonts", ".icons". The snapshot operation
    // mirrors the home-relative selection it was given, so we hand it
    // exactly what detect reported.
    function confirmedSelections() {
        return {
            configs: view.selectedConfigs.slice(),
            packages: view.selectedPackages.slice(),
            assets: view.selectedAssets.slice()
        };
    }

    function createProfile() {
        var name = view.profileName.trim();
        if (name.length === 0) {
            view.nameError = "give the profile a name";
            return;
        }
        if (!/^[A-Za-z0-9_-]+$/.test(name)) {
            view.nameError = "profile names may only contain letters, numbers, `-`, and `_`";
            return;
        }
        view.nameError = "";
        if (view.shell.backend.runSnapshot(name)) {
            // success is handled by the Connections below, which pops back to
            // Profiles on the new card.
        }
    }

    Component.onCompleted: {
        view.resetForm();
        view.detect();
    }

    Connections {
        target: view.shell ? view.shell.backend : null
        function onOperationFinished(operation, ok, envelope) {
            if (operation === "detect") {
                view.detecting = false;
                if (ok && envelope.data) {
                    view.detectData = envelope.data;
                    view.detectError = "";
                    // Pre-check every chip: the whole point is a confirmed
                    // selection the user can un-tick, not a blank list.
                    view.selectedConfigs = envelope.data.config_dirs ? envelope.data.config_dirs.slice() : [];
                    var official = envelope.data.packages && envelope.data.packages.official ? envelope.data.packages.official : [];
                    var aur = envelope.data.packages && envelope.data.packages.aur ? envelope.data.packages.aur : [];
                    view.selectedPackages = official.concat(aur);
                    view.selectedAssets = envelope.data.assets ? envelope.data.assets.slice() : [];
                } else {
                    view.detectData = null;
                    view.detectError = envelope && envelope.data && envelope.data.error ? String(envelope.data.error) : "detection failed";
                }
            } else if (operation === "snapshot") {
                view.snapshotWarnings = ok && envelope.warnings ? envelope.warnings : [];
                if (!ok) {
                    view.snapshotError = envelope && envelope.data && envelope.data.error ? String(envelope.data.error) : "snapshot failed";
                    return;
                }
                // Success: the new card is already in the store. Pop the
                // snapshot view; Profiles (the root when uninitialized or
                // pushed onto the stack) re-lists on activation and shows it.
                view.snapshotError = "";
                view.shell.pop();
            } else if (operation === "wallpaper-import") {
                if (ok && envelope.data && envelope.data.source)
                    view.importedWallpapers = view.importedWallpapers.concat([envelope.data.source]);
            }
        }
    }

    Flickable {
        anchors.fill: parent
        anchors.margins: 18
        contentWidth: width
        contentHeight: column.implicitHeight + 16
        clip: true
        boundsBehavior: Flickable.StopAtBounds

        Column {
            id: column
            width: parent.width
            spacing: 14

            // Back + title row.
            Item {
                width: column.width
                height: 30

                Text {
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    text: "New snapshot"
                    color: view.theme.foreground
                    font.pixelSize: 20
                    font.bold: true
                }

                Text {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    text: "← Profiles"
                    color: view.theme.accent
                    font.pixelSize: 13
                    font.bold: true

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: view.shell.pop()
                    }
                }
            }

            // ----------------------------------------------------------
            // Name field
            // ----------------------------------------------------------
            Text {
                text: "Profile name"
                color: view.theme.foreground
                font.pixelSize: 14
                font.bold: true
            }

            Rectangle {
                width: column.width
                height: 44
                radius: 9
                color: view.theme.surface
                border.width: 1
                border.color: view.nameError !== "" ? view.theme.danger : view.theme.border

                TextInput {
                    anchors.fill: parent
                    anchors.leftMargin: 12
                    anchors.rightMargin: 12
                    verticalAlignment: TextInput.AlignVCenter
                    text: view.profileName
                    onTextChanged: view.profileName = text
                    color: view.theme.foreground
                    font.pixelSize: 14
                    placeholderText: "my-rice"
                    placeholderTextColor: view.theme.muted
                }
            }

            Text {
                width: parent.width
                visible: view.nameError !== ""
                wrapMode: Text.WordWrap
                text: view.nameError
                color: view.theme.danger
                font.pixelSize: 12
            }

            // ----------------------------------------------------------
            // Grouped sections: Configs / Packages / Fonts & assets
            // ----------------------------------------------------------

            // A reusable collapsible section header + chip grid.
            component SectionHeader: Item {
                id: sec
                property string title: ""
                property bool open: true
                property int count: 0
                property int checked: 0

                width: column.width
                height: 34

                Text {
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    text: (sec.open ? "▾" : "▸") + "  " + sec.title + "  (" + sec.checked + "/" + sec.count + ")"
                    color: view.theme.foreground
                    font.pixelSize: 13
                    font.bold: true
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: sec.open = !sec.open
                }
            }

            // Configs
            SectionHeader {
                title: "Configs"
                count: view.detectData ? view.detectData.config_dirs.length : 0
                checked: view.selectedConfigs.length
                property bool open: view.configsOpen
                onOpenChanged: view.configsOpen = open
            }

            Repeater {
                visible: view.configsOpen
                model: view.detectData ? view.detectData.config_dirs : []

                delegate: Rectangle {
                    width: chipLabel.length * 8 + 34
                    height: 28
                    radius: 6
                    color: view.chipChecked("configs", modelData) ? view.theme.accent : view.theme.surfaceAlt
                    border.width: 1
                    border.color: view.chipChecked("configs", modelData) ? view.theme.accent : view.theme.border

                    Text {
                        id: chipLabel
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.left: parent.left
                        anchors.leftMargin: 8
                        text: modelData
                        color: view.chipChecked("configs", modelData) ? "#ffffffff" : view.theme.muted
                        font.pixelSize: 12
                    }

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: view.toggleChip("configs", modelData)
                    }
                }
            }

            // Packages
            SectionHeader {
                title: "Packages"
                count: view.detectData ? (view.detectData.packages.official.length + view.detectData.packages.aur.length) : 0
                checked: view.selectedPackages.length
                property bool open: view.packagesOpen
                onOpenChanged: view.packagesOpen = open
            }

            Repeater {
                visible: view.packagesOpen
                model: view.detectData ? view.detectData.packages.official.concat(view.detectData.packages.aur) : []

                delegate: Rectangle {
                    width: chipLabel.length * 8 + 34
                    height: 28
                    radius: 6
                    color: view.chipChecked("packages", modelData) ? view.theme.accent : view.theme.surfaceAlt
                    border.width: 1
                    border.color: view.chipChecked("packages", modelData) ? view.theme.accent : view.theme.border

                    Text {
                        id: chipLabel
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.left: parent.left
                        anchors.leftMargin: 8
                        text: modelData
                        color: view.chipChecked("packages", modelData) ? "#ffffffff" : view.theme.muted
                        font.pixelSize: 12
                    }

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: view.toggleChip("packages", modelData)
                    }
                }
            }

            // Fonts & assets
            SectionHeader {
                title: "Fonts & assets"
                count: view.detectData ? view.detectData.assets.length : 0
                checked: view.selectedAssets.length
                property bool open: view.assetsOpen
                onOpenChanged: view.assetsOpen = open
            }

            Repeater {
                visible: view.assetsOpen
                model: view.detectData ? view.detectData.assets : []

                delegate: Rectangle {
                    width: chipLabel.length * 8 + 34
                    height: 28
                    radius: 6
                    color: view.chipChecked("assets", modelData) ? view.theme.accent : view.theme.surfaceAlt
                    border.width: 1
                    border.color: view.chipChecked("assets", modelData) ? view.theme.accent : view.theme.border

                    Text {
                        id: chipLabel
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.left: parent.left
                        anchors.leftMargin: 8
                        text: modelData
                        color: view.chipChecked("assets", modelData) ? "#ffffffff" : view.theme.muted
                        font.pixelSize: 12
                    }

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: view.toggleChip("assets", modelData)
                    }
                }
            }

            // ----------------------------------------------------------
            // Wallpaper-import candidates (Downloads)
            // ----------------------------------------------------------
            Column {
                width: column.width
                spacing: 6
                visible: view.wallpaperCandidates.length > 0

                Text {
                    text: "Import wallpapers from Downloads"
                    color: view.theme.foreground
                    font.pixelSize: 13
                    font.bold: true
                }

                Repeater {
                    model: view.wallpaperCandidates
                    delegate: Row {
                        spacing: 8
                        width: column.width

                        Text {
                            width: column.width - 110
                            elide: Text.ElideRight
                            text: modelData.split("/").pop()
                            color: view.theme.muted
                            font.pixelSize: 12
                            verticalAlignment: Text.AlignVCenter
                        }

                        Rectangle {
                            width: 64
                            height: 26
                            radius: 6
                            color: view.importedWallpapers.indexOf(modelData) >= 0 ? view.theme.success : view.theme.surfaceAlt
                            border.width: 1
                            border.color: view.importedWallpapers.indexOf(modelData) >= 0 ? view.theme.success : view.theme.border

                            Text {
                                anchors.centerIn: parent
                                text: view.importedWallpapers.indexOf(modelData) >= 0 ? "Imported" : "Import"
                                color: view.importedWallpapers.indexOf(modelData) >= 0 ? "#ffffffff" : view.theme.foreground
                                font.pixelSize: 11
                                font.bold: true
                            }

                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                enabled: view.importedWallpapers.indexOf(modelData) < 0
                                onClicked: view.shell.backend.runWallpaperImport(modelData)
                            }
                        }
                    }
                }
            }

            // ----------------------------------------------------------
            // Create button + progress + errors
            // ----------------------------------------------------------
            Rectangle {
                width: column.width
                height: 46
                radius: 9
                color: view.theme.surface
                border.width: 1
                border.color: view.creating ? view.theme.border : view.theme.accent

                Text {
                    anchors.centerIn: parent
                    text: view.creating ? "Snapshotting…" : "Create profile"
                    color: view.creating ? view.theme.muted : view.theme.accent
                    font.pixelSize: 15
                    font.bold: true
                }

                MouseArea {
                    anchors.fill: parent
                    enabled: !view.creating
                    cursorShape: Qt.PointingHandCursor
                    onClicked: view.createProfile()
                }
            }

            Column {
                width: column.width
                visible: view.creating
                spacing: 6

                Text {
                    width: parent.width
                    wrapMode: Text.WordWrap
                    text: view.shell.backend.progressMessage
                    color: view.theme.foreground
                    font.pixelSize: 12
                }

                Rectangle {
                    width: parent.width
                    height: 4
                    radius: 2
                    color: view.theme.surfaceAlt

                    Rectangle {
                        width: Math.min(parent.width, parent.width * Math.min(view.shell.backend.progressStep, 8) / 8)
                        height: parent.height
                        radius: 2
                        color: view.theme.accent

                        Behavior on width {
                            NumberAnimation {
                                duration: 220
                                easing.type: Easing.OutCubic
                            }
                        }
                    }
                }
            }

            Text {
                width: parent.width
                visible: view.snapshotError !== ""
                wrapMode: Text.WordWrap
                text: view.snapshotError
                color: view.theme.danger
                font.pixelSize: 12
            }

            Repeater {
                model: view.snapshotWarnings
                delegate: Text {
                    width: column.width
                    wrapMode: Text.WordWrap
                    text: modelData
                    color: view.theme.muted
                    font.pixelSize: 11
                }
            }

            // detect error
            Text {
                width: parent.width
                visible: view.detectError !== ""
                wrapMode: Text.WordWrap
                text: view.detectError + "  —  retry"
                color: view.theme.danger
                font.pixelSize: 12
            }
        }
    }
}
