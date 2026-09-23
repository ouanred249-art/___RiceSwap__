import QtQuick
import Quickshell
import Quickshell.Hyprland
import Quickshell.Io
import Quickshell.Wayland
import "views"

// RiceSwap's entire GUI: one right-edge slide-in PanelWindow with a
// stacked view stack (push/pop), Super+R toggle via GlobalShortcut with
// an IpcHandler fallback, and theme-following via Theme.qml.
//
// Autostart (documented in gui/README.md):
//     exec-once = qs -c riceswap
// Keybind:
//     bind = $mainMod, R, global, quickshell:riceswap-toggle
// IPC fallback:
//     qs ipc call riceswap toggle
ShellRoot {
    id: shellRoot

    // The window stays mapped; open only slides the content in and out.
    property bool open: false

    // The stacked view stack: [] = root view; entries are {name, arg}.
    property var navStack: []

    // The root view flips to Profiles the moment init writes
    // initialized: true into state.json — that is the onboarding gate.
    readonly property string rootView: rsBackend.initialized ? "profiles" : "onboarding"
    readonly property string currentView: navStack.length > 0 ? navStack[navStack.length - 1].name : rootView
    readonly property var currentArg: navStack.length > 0 ? navStack[navStack.length - 1].arg : null

    // How the views reach everything else: one explicit property each.
    readonly property var backend: rsBackend
    readonly property var theme: rsTheme

    function toggle() {
        open = !open;
    }

    function close() {
        open = false;
    }

    function push(name, arg) {
        navStack = navStack.concat([{
            name: name,
            arg: arg
        }]);
    }

    function pop() {
        if (navStack.length > 0)
            navStack = navStack.slice(0, navStack.length - 1);
    }

    function popToRoot() {
        navStack = [];
    }

    onOpenChanged: {
        if (open) {
            if (rsBackend.initialized)
                rsBackend.runList();
            Qt.callLater(function () {
                content.forceActiveFocus();
            });
        }
    }

    Backend {
        id: rsBackend
    }

    Theme {
        id: rsTheme
        activeProfile: rsBackend.activeProfile
    }

    // Super+R: bind = $mainMod, R, global, quickshell:riceswap-toggle
    // (Hyprland's global shortcuts protocol; appid defaults to quickshell).
    GlobalShortcut {
        name: "riceswap-toggle"
        onPressed: shellRoot.toggle()
    }

    // Scripting / fallback path: qs ipc call riceswap toggle
    IpcHandler {
        target: "riceswap"

        function toggle(): void {
            shellRoot.toggle();
        }
    }

    PanelWindow {
        id: panel

        anchors {
            top: true
            bottom: true
            right: true
        }
        implicitWidth: 420
        color: "transparent"
        exclusionMode: ExclusionMode.Ignore
        aboveWindows: true
        focusable: shellRoot.open
        WlrLayershell.layer: WlrLayer.Top
        WlrLayershell.namespace: "riceswap"

        // Only the panel content is clickable. While the content has slid
        // out of the window, its rect no longer intersects the surface, so
        // the closed panel is fully inert and clicks pass through.
        mask: Region {
            item: content
        }

        Rectangle {
            id: content

            width: panel.implicitWidth
            height: panel.height
            x: shellRoot.open ? 0 : panel.implicitWidth
            color: rsTheme.background
            focus: shellRoot.open

            Behavior on x {
                NumberAnimation {
                    duration: 240
                    easing.type: Easing.OutCubic
                }
            }

            Keys.onEscapePressed: event => {
                if (shellRoot.navStack.length > 0)
                    shellRoot.pop();
                else
                    shellRoot.close();
                event.accepted = true;
            }

            // Hairline separating the panel from the desktop behind it.
            Rectangle {
                anchors.left: parent.left
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                width: 1
                color: rsTheme.border
            }

            OnboardingView {
                shell: shellRoot
                theme: rsTheme
                width: content.width
                height: content.height
                active: shellRoot.currentView === "onboarding"
            }

            ProfilesView {
                shell: shellRoot
                theme: rsTheme
                width: content.width
                height: content.height
                active: shellRoot.currentView === "profiles"
            }

            InfoView {
                shell: shellRoot
                theme: rsTheme
                arg: shellRoot.currentView === "info" && shellRoot.currentArg ? String(shellRoot.currentArg) : ""
                width: content.width
                height: content.height
                active: shellRoot.currentView === "info"
            }

            SnapshotView {
                shell: shellRoot
                theme: rsTheme
                width: content.width
                height: content.height
                active: shellRoot.currentView === "snapshot"
            }

            SwitchView {
                shell: shellRoot
                theme: rsTheme
                arg: shellRoot.currentView === "switch" && shellRoot.currentArg ? String(shellRoot.currentArg) : ""
                width: content.width
                height: content.height
                active: shellRoot.currentView === "switch"
            }
        }
    }
}
