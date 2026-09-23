import QtQuick

// First-run onboarding: shown while state.json does not say initialized.
// Explains what setup does, runs the real `init` operation with live
// progress from its NDJSON stream, and lands on Profiles — the root view
// binding flips the moment `init` writes initialized: true.
PanelView {
    id: view

    property string setupError: ""
    property var setupWarnings: []
    readonly property bool settingUp: shell !== null && shell.backend.busy && shell.backend.runningOp === "init"

    Connections {
        target: view.shell ? view.shell.backend : null
        function onOperationFinished(operation, ok, envelope) {
            if (operation !== "init")
                return;
            view.setupError = ok ? "" : (envelope && envelope.data && envelope.data.error ? String(envelope.data.error) : "setup failed");
            view.setupWarnings = ok && envelope.warnings ? envelope.warnings : [];
        }
    }

    Flickable {
        anchors.fill: parent
        anchors.margins: 26
        contentHeight: column.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds

        Column {
            id: column
            width: parent.width
            spacing: 14

            Text {
                text: "RiceSwap"
                color: view.theme.foreground
                font.pixelSize: 26
                font.bold: true
            }

            Text {
                width: parent.width
                wrapMode: Text.WordWrap
                lineHeight: 1.3
                text: "RiceSwap snapshots riced-up desktops into profiles and switches between them. Nothing is set up yet — setup prepares the shared pieces every profile builds on."
                color: view.theme.muted
                font.pixelSize: 13
            }

            Text {
                text: "What setup does"
                color: view.theme.foreground
                font.pixelSize: 14
                font.bold: true
            }

            Repeater {
                model: [
                    {
                        title: "Shared layers",
                        detail: "creates the profile store and the shared wallpapers layer under ~/.local/share/riceswap"
                    },
                    {
                        title: "Hardware extraction",
                        detail: "lifts monitors, input, and GPU settings out of hyprland.conf into one shared hardware file every profile inherits"
                    },
                    {
                        title: "Bundled wallpapers",
                        detail: "installs four default wallpapers so a fresh install starts with choices"
                    },
                    {
                        title: "Panel config",
                        detail: "copies the packaged panel configuration into your own config, where you can customize it"
                    }
                ]
                delegate: Column {
                    width: column.width
                    spacing: 2

                    Text {
                        text: "✓  " + modelData.title
                        color: view.theme.accent
                        font.pixelSize: 13
                        font.bold: true
                    }

                    Text {
                        width: parent.width
                        wrapMode: Text.WordWrap
                        text: modelData.detail
                        color: view.theme.muted
                        font.pixelSize: 12
                    }
                }
            }

            // Live progress straight off the init NDJSON stream.
            Column {
                width: column.width
                visible: view.settingUp
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
                        width: Math.min(parent.width, parent.width * Math.min(view.shell.backend.progressStep, 5) / 5)
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

            Rectangle {
                width: column.width
                height: 46
                radius: 9
                color: view.theme.surface
                border.width: 1
                border.color: view.settingUp ? view.theme.border : view.theme.accent

                Text {
                    anchors.centerIn: parent
                    text: view.settingUp ? "Setting up…" : "Set Up"
                    color: view.settingUp ? view.theme.muted : view.theme.accent
                    font.pixelSize: 15
                    font.bold: true
                }

                MouseArea {
                    anchors.fill: parent
                    enabled: !view.settingUp
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        if (view.shell.backend.runInit())
                            view.setupError = "";
                        else
                            view.setupError = "another operation is already running";
                    }
                }
            }

            Text {
                width: parent.width
                visible: view.setupError !== ""
                wrapMode: Text.WordWrap
                text: view.setupError
                color: view.theme.danger
                font.pixelSize: 12
            }

            Repeater {
                model: view.setupWarnings
                delegate: Text {
                    width: column.width
                    wrapMode: Text.WordWrap
                    text: modelData
                    color: view.theme.muted
                    font.pixelSize: 11
                }
            }

            Text {
                width: parent.width
                wrapMode: Text.WordWrap
                text: "Open the panel any time with Super+R."
                color: view.theme.muted
                font.pixelSize: 11
                opacity: 0.8
            }
        }
    }
}
