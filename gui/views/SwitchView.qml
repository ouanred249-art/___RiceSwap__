import QtQuick

// The switch flow: three stacked sub-states on one view.
//
//  1. Confirm — the real `plan <target>` diff: "Will install N / remove N /
//     stop N services / start N" with expandable sections naming the actual
//     packages. Blocked target paths surface with a Snapshot-first chain
//     that pushes the snapshot view and returns with a fresh plan on the
//     way back. Cancel aborts before anything runs.
//  2. Progress — running the real `switch <target>`: a step list fed by
//     the live NDJSON progress stream (✓ done / ● running / ○ upcoming),
//     inline warnings as they arrive, and a Cancel that sends SIGTERM —
//     the backend stops at the next step boundary, never mid-step.
//  3. Result — the final envelope: the success report, or the failure and
//     recovery screen built from `completed_steps` + `resume_hint` with a
//     one-click "switch to restore".
//
// Reopening the panel mid-switch lands here in the progress state, resumed
// from state.json's running operation (backend.runningOperation).
PanelView {
    id: view

    property string arg: ""

    // confirm | progress | result
    property string stage: "confirm"

    // ------------------------------------------------------------------
    // Confirm state (fed by the real `plan` operation)
    // ------------------------------------------------------------------
    property var planData: null
    property bool planning: false
    property string planError: ""
    property var planWarnings: []
    property bool packagesOpen: false
    property bool servicesOpen: false
    property bool configsOpen: false

    readonly property var planBlocked: planData !== null && planData.blocked_paths ? planData.blocked_paths : []

    readonly property var planInstall: {
        const install = planData && planData.package_diff && planData.package_diff.install ? planData.package_diff.install : {};
        const official = install.official ? install.official : [];
        const aur = install.aur ? install.aur : [];
        return official.concat(aur);
    }
    readonly property var planRemove: {
        const remove = planData && planData.package_diff && planData.package_diff.remove ? planData.package_diff.remove : {};
        const official = remove.official ? remove.official : [];
        const aur = remove.aur ? remove.aur : [];
        return official.concat(aur);
    }
    readonly property var planServicesStop: {
        const stop = planData && planData.service_changes && planData.service_changes.stop ? planData.service_changes.stop : [];
        return stop;
    }
    readonly property var planServicesStart: {
        const start = planData && planData.service_changes && planData.service_changes.start ? planData.service_changes.start : [];
        return start;
    }
    readonly property var planLink: planData && planData.symlink_changes ? planData.symlink_changes.link : []
    readonly property var planUnlink: planData && planData.symlink_changes ? planData.symlink_changes.unlink : []

    function planCounts() {
        const install = planInstall.length;
        const remove = planRemove.length;
        const stop = planServicesStop.length;
        const start = planServicesStart.length;
        const parts = [];
        if (install > 0)
            parts.push("install " + install + " package" + (install === 1 ? "" : "s"));
        if (remove > 0)
            parts.push("remove " + remove + " package" + (remove === 1 ? "" : "s"));
        if (stop > 0)
            parts.push("stop " + stop + " service" + (stop === 1 ? "" : "s"));
        if (start > 0)
            parts.push("start " + start + " service" + (start === 1 ? "" : "s"));
        if (planLink.length > 0)
            parts.push("link " + planLink.length + " config path" + (planLink.length === 1 ? "" : "s"));
        if (planUnlink.length > 0)
            parts.push("unlink " + planUnlink.length + " config path" + (planUnlink.length === 1 ? "" : "s"));
        return parts;
    }

    function refreshPlan() {
        planError = "";
        planData = null;
        if (shell.backend.runPlan(arg))
            planning = true;
    }

    // ------------------------------------------------------------------
    // Progress state. The step list mirrors the switch operation's own
    // progress stream one-to-one (state.json's step index and the live
    // NDJSON progress step both advance per these messages):
    //
    //   1 verify the target profile
    //   2 compute the switch plan
    //   3 activate the profile (flip `current`)
    //   4 stop the old services
    //   5 link the managed config paths
    //   6 apply the package changes (install-first, then remove)
    //   7 reload Hyprland
    //   8 start the new services
    //
    // The GUI pre-runs steps 1–2 as the separate `plan` operation, so the
    // list starts at step 3; live progress lines map onto it directly.
    // ------------------------------------------------------------------
    readonly property var switchSteps: [
        "Verify the target profile",
        "Compute the switch plan",
        "Activate the profile",
        "Stop old services",
        "Link managed config paths",
        "Apply package changes",
        "Reload Hyprland",
        "Start new services"
    ]
    property var switchWarnings: []
    property bool cancelling: false

    // The live per-step progress message, straight from the NDJSON stream
    // (the backend's own progressMessage, which only mirrors switch lines
    // while a switch is the running operation).
    readonly property string progressMessage: shell.backend.runningOp === "switch" ? shell.backend.progressMessage : ""

    // The running operation seen in state.json. While it is a switch of
    // this target, its step index is authoritative for the step list —
    // that is how a panel reopened mid-switch resumes the view.
    readonly property var runningOp: shell.backend.runningOperation

    readonly property bool opRunning: runningOp !== null && runningOp.name === "switch" && runningOp.target === view.arg

    // The running step: the live NDJSON stream while the panel is open,
    // state.json's step after a resume. Both advance the same way.
    readonly property var opStep: {
        if (shell.backend.busy && shell.backend.runningOp === "switch")
            return shell.backend.progressStep;
        return opRunning ? (runningOp.step ? runningOp.step : 0) : 0;
    }

    // State for step index i (0-based): done / active / todo. While a
    // switch is running the running step is active and everything before
    // it is done; after the operation finishes every step is done.
    function stepState(i) {
        const running = view.stage === "progress";
        if (!running)
            return "done";
        const current = view.opStep;
        if (i + 1 < current)
            return "done";
        if (i + 1 === current)
            return "active";
        return "todo";
    }

    // ------------------------------------------------------------------
    // Result state
    // ------------------------------------------------------------------
    property bool switchOk: false
    property var switchData: null
    property var switchResultWarnings: []

    function showResult(envelope) {
        switchOk = envelope.ok;
        switchData = envelope.data ? envelope.data : null;
        switchResultWarnings = envelope.warnings ? envelope.warnings : [];
        cancelling = false;
        stage = "result";
    }

    // ------------------------------------------------------------------
    // Lifecycle
    // ------------------------------------------------------------------
    onEnabledChanged: {
        if (enabled) {
            // A switch may be running with the panel closed: resume from
            // state.json instead of re-planning (re-planning a target that
            // is now active is a no-op diff and would look wrong).
            const op = runningOp;
            if (op !== null && op.name === "switch" && op.target === view.arg) {
                stage = "progress";
                switchWarnings = [];
                cancelling = false;
                return;
            }
            // Confirming for the first time, or coming back from a
            // snapshot-first chain (the store changed, so re-plan).
            if (stage === "confirm" && !planning)
                refreshPlan();
        }
    }

    function startSwitch() {
        if (shell.backend.runSwitch(arg)) {
            switchWarnings = [];
            cancelling = false;
            stage = "progress";
        }
    }

    function cancelSwitch() {
        // SIGTERM: the backend stops at the next step boundary and reports
        // an honest "cancelled" envelope with completed_steps + resume_hint.
        cancelling = true;
        shell.backend.cancelSwitch();
    }

    function snapshotFirst() {
        // Chain into the snapshot flow; SnapshotView re-detects on open and
        // this view re-plans when it comes back active.
        shell.push("snapshot", null);
    }

    function finishSwitch() {
        // Success: the switch view is done. Pop the whole stack; the
        // Profiles home re-lists and re-badges the now-active profile.
        shell.popToRoot();
    }

    Connections {
        target: view.shell ? view.shell.backend : null
        function onOperationStarted(operation) {
            // Clear the progress warnings when a switch starts; the plan
            // warnings are separate (planWarnings) and survive.
            if (operation === "switch")
                view.switchWarnings = [];
        }
        function onOperationWarning(operation, message) {
            // Warnings stream at the moment they happen — the kept-package
            // note on the package step, the service note on start-services —
            // so the progress list renders them inline, not at the end.
            if (operation === "switch")
                view.switchWarnings = view.switchWarnings.concat([message]);
        }
        function onOperationFinished(operation, ok, envelope) {
            if (operation === "plan") {
                view.planning = false;
                if (!ok) {
                    view.planError = envelope && envelope.data && envelope.data.error ? String(envelope.data.error) : "cannot plan this switch";
                    view.planData = null;
                    return;
                }
                view.planWarnings = envelope.warnings ? envelope.warnings : [];
                view.planData = envelope.data ? envelope.data : null;
            } else if (operation === "switch") {
                view.showResult(envelope);
            }
        }
    }


    // ------------------------------------------------------------------
    // Header: back + title
    // ------------------------------------------------------------------
    Column {
        id: header
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.margins: 18
        anchors.bottomMargin: 0
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
                visible: view.stage === "confirm" || (view.stage === "result" && !view.switchOk)
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
                anchors.leftMargin: backBtn.visible ? 10 : 0
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                elide: Text.ElideRight
                text: view.stage === "progress" ? "Switching to " + view.arg : view.stage === "result" ? "Switch " + (view.switchOk ? "done" : "failed") : "Switch to " + view.arg
                color: view.theme.foreground
                font.pixelSize: 20
                font.bold: true
            }
        }
    }

    // ------------------------------------------------------------------
    // Confirm
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
        contentHeight: confirmColumn.implicitHeight + 8
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        visible: view.stage === "confirm"

        Column {
            id: confirmColumn
            width: parent.width
            spacing: 12

            Text {
                width: parent.width
                visible: view.planning && view.planData === null
                text: "Computing the switch plan…"
                color: view.theme.muted
                font.pixelSize: 12
            }

            Text {
                width: parent.width
                visible: view.planError !== ""
                wrapMode: Text.WordWrap
                text: view.planError
                color: view.theme.danger
                font.pixelSize: 12
            }

            // The one-line summary: "Will install 6 / remove 2 / …"
            Item {
                width: confirmColumn.width
                height: summaryText.implicitHeight + (summarySubText.implicitHeight > 0 ? summarySubText.implicitHeight + 4 : 0)
                visible: view.planData !== null && view.planBlocked.length === 0

                Text {
                    id: summaryText
                    width: parent.width
                    wrapMode: Text.WordWrap
                    text: {
                        const counts = view.planCounts();
                        return counts.length > 0 ? "Will " + counts.join(" / ") : "No changes — the desktop already matches this profile";
                    }
                    color: view.theme.foreground
                    font.pixelSize: 14
                    font.bold: true
                }

                Text {
                    id: summarySubText
                    width: parent.width
                    y: summaryText.implicitHeight + 4
                    wrapMode: Text.WordWrap
                    text: view.planWarnings.length > 0 ? view.planWarnings.join(" · ") : ""
                    color: view.theme.muted
                    font.pixelSize: 11
                }
            }

            // Expandable package detail.
            Column {
                width: confirmColumn.width
                visible: view.planData !== null && (view.planInstall.length > 0 || view.planRemove.length > 0)
                spacing: 4

                Text {
                    text: (view.packagesOpen ? "▾" : "▸") + "  Packages  (" + view.planInstall.length + " in / " + view.planRemove.length + " out)"
                    color: view.theme.foreground
                    font.pixelSize: 13
                    font.bold: true

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: view.packagesOpen = !view.packagesOpen
                    }
                }

                Column {
                    width: parent.width
                    visible: view.packagesOpen
                    spacing: 2

                    Repeater {
                        model: view.planInstall
                        delegate: Text {
                            text: "+  " + modelData
                            color: view.theme.success
                            font.pixelSize: 12
                            font.family: "monospace"
                        }
                    }
                    Repeater {
                        model: view.planRemove
                        delegate: Text {
                            text: "−  " + modelData
                            color: view.theme.danger
                            font.pixelSize: 12
                            font.family: "monospace"
                        }
                    }
                }
            }

            // Expandable services detail.
            Column {
                width: confirmColumn.width
                visible: view.planData !== null && (view.planServicesStop.length > 0 || view.planServicesStart.length > 0)
                spacing: 4

                Text {
                    text: (view.servicesOpen ? "▾" : "▸") + "  Services  (" + view.planServicesStop.length + " stop / " + view.planServicesStart.length + " start)"
                    color: view.theme.foreground
                    font.pixelSize: 13
                    font.bold: true

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: view.servicesOpen = !view.servicesOpen
                    }
                }

                Column {
                    width: parent.width
                    visible: view.servicesOpen
                    spacing: 2

                    Repeater {
                        model: view.planServicesStop
                        delegate: Text {
                            text: "stop   " + modelData
                            color: view.theme.muted
                            font.pixelSize: 12
                            font.family: "monospace"
                        }
                    }
                    Repeater {
                        model: view.planServicesStart
                        delegate: Text {
                            text: "start  " + modelData
                            color: view.theme.success
                            font.pixelSize: 12
                            font.family: "monospace"
                        }
                    }
                }
            }

            // Expandable config paths detail.
            Column {
                width: confirmColumn.width
                visible: view.planData !== null && (view.planLink.length > 0 || view.planUnlink.length > 0)
                spacing: 4

                Text {
                    text: (view.configsOpen ? "▾" : "▸") + "  Configs  (" + view.planLink.length + " link / " + view.planUnlink.length + " unlink)"
                    color: view.theme.foreground
                    font.pixelSize: 13
                    font.bold: true

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: view.configsOpen = !view.configsOpen
                    }
                }

                Column {
                    width: parent.width
                    visible: view.configsOpen
                    spacing: 2

                    Repeater {
                        model: view.planLink
                        delegate: Text {
                            text: "link    " + modelData
                            color: view.theme.foreground
                            font.pixelSize: 12
                            font.family: "monospace"
                        }
                    }
                    Repeater {
                        model: view.planUnlink
                        delegate: Text {
                            text: "unlink  " + modelData
                            color: view.theme.muted
                            font.pixelSize: 12
                            font.family: "monospace"
                        }
                    }
                }
            }

            // Blocked paths → Snapshot first chain.
            Column {
                width: confirmColumn.width
                visible: view.planBlocked.length > 0
                spacing: 8

                Text {
                    width: parent.width
                    wrapMode: Text.WordWrap
                    text: "These target paths hold real files that would be overwritten:"
                    color: view.theme.danger
                    font.pixelSize: 13
                    font.bold: true
                }

                Repeater {
                    model: view.planBlocked
                    delegate: Text {
                        text: "•  " + modelData
                        color: view.theme.danger
                        font.pixelSize: 12
                        font.family: "monospace"
                    }
                }

                Text {
                    width: parent.width
                    wrapMode: Text.WordWrap
                    text: "Snapshot your current desktop first to adopt them into a profile, then come back and switch."
                    color: view.theme.muted
                    font.pixelSize: 12
                }

                Row {
                    spacing: 10

                    Rectangle {
                        width: snapLabel.implicitWidth + 28
                        height: 38
                        radius: 8
                        color: view.theme.surface
                        border.width: 1
                        border.color: view.theme.accent

                        Text {
                            id: snapLabel
                            anchors.centerIn: parent
                            text: "Snapshot first"
                            color: view.theme.accent
                            font.pixelSize: 13
                            font.bold: true
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: view.snapshotFirst()
                        }
                    }

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
                            font.pixelSize: 13
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: view.shell.pop()
                        }
                    }
                }
            }

            // Switch now / Cancel — only when nothing blocks the switch.
            Row {
                width: confirmColumn.width
                visible: view.planData !== null && view.planBlocked.length === 0 && !view.planning
                spacing: 10

                Rectangle {
                    width: switchLabel.implicitWidth + 32
                    height: 44
                    radius: 9
                    color: view.theme.accent

                    Text {
                        id: switchLabel
                        anchors.centerIn: parent
                        text: "Switch now"
                        color: "#ffffffff"
                        font.pixelSize: 15
                        font.bold: true
                    }

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: view.startSwitch()
                    }
                }

                Rectangle {
                    width: cancelAllLabel.implicitWidth + 32
                    height: 44
                    radius: 9
                    color: view.theme.surfaceAlt
                    border.width: 1
                    border.color: view.theme.border

                    Text {
                        id: cancelAllLabel
                        anchors.centerIn: parent
                        text: "Cancel"
                        color: view.theme.foreground
                        font.pixelSize: 14
                    }

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: view.shell.pop()
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Progress — live step list from state.json / the NDJSON stream
    // ------------------------------------------------------------------
    Item {
        anchors.top: header.bottom
        anchors.bottom: parent.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.margins: 18
        visible: view.stage === "progress"

        Column {
            anchors.fill: parent
            spacing: 10

            // The running-operation line: which step of the switch sequence
            // is current. The live step index drives it; the per-step
            // progress message is shown under the list.
            Text {
                width: parent.width
                wrapMode: Text.WordWrap
                text: {
                    if (view.cancelling)
                        return "Cancelling — stops at the next step boundary…";
                    return "step " + view.opStep + " of " + view.switchSteps.length + " — switch in progress";
                }
                color: view.theme.foreground
                font.pixelSize: 13
                font.bold: true
            }

            // The step list: ✓ done / ● running / ○ upcoming.
            Column {
                width: parent.width
                spacing: 6

                Repeater {
                    model: view.switchSteps.length

                    delegate: Row {
                        spacing: 8
                        width: parent.width

                        Text {
                            text: {
                                const state = view.stepState(index);
                                return state === "done" ? "✓" : state === "active" ? "●" : "○";
                            }
                            color: view.stepState(index) === "done" ? view.theme.success : view.stepState(index) === "active" ? view.theme.accent : view.theme.muted
                            font.pixelSize: 13
                            font.bold: true
                        }

                        Text {
                            text: view.switchSteps[index]
                            color: view.stepState(index) === "todo" ? view.theme.muted : view.theme.foreground
                            font.pixelSize: 13
                        }
                    }
                }

                // The in-step detail line: which package/service the live
                // progress message is currently working on.
                Text {
                    width: parent.width
                    visible: view.progressMessage !== ""
                    wrapMode: Text.WordWrap
                    text: view.progressMessage
                    color: view.theme.muted
                    font.pixelSize: 11
                }
            }

            // Inline warnings as they arrive.
            Repeater {
                model: view.switchWarnings
                delegate: Text {
                    width: parent.width
                    wrapMode: Text.WordWrap
                    text: "⚠ " + modelData
                    color: view.theme.muted
                    font.pixelSize: 11
                }
            }

            Item {
                width: 1
                height: parent ? 16 : 0
            }

            // Cancel: SIGTERM at a step boundary.
            Rectangle {
                width: cancelNowLabel.implicitWidth + 32
                height: 40
                radius: 8
                color: view.theme.surfaceAlt
                border.width: 1
                border.color: view.theme.border
                visible: !view.cancelling

                Text {
                    id: cancelNowLabel
                    anchors.centerIn: parent
                    text: view.cancelling ? "Cancelling…" : "Cancel switch"
                    color: view.theme.foreground
                    font.pixelSize: 13
                    font.bold: true
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: view.cancelSwitch()
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Result — success report or recovery screen
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
        contentHeight: resultColumn.implicitHeight + 8
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        visible: view.stage === "result"

        Column {
            id: resultColumn
            width: parent.width
            spacing: 10

            // ---- Success report ----
            Column {
                width: resultColumn.width
                spacing: 6
                visible: view.switchOk

                Text {
                    text: "Switched to " + view.arg
                    color: view.theme.success
                    font.pixelSize: 16
                    font.bold: true
                }

                Text {
                    width: parent.width
                    visible: view.switchData !== null && view.switchData.completed_steps !== undefined
                    wrapMode: Text.WordWrap
                    text: view.switchData !== null && view.switchData.completed_steps !== undefined ? "completed steps: " + view.switchData.completed_steps : ""
                    color: view.theme.muted
                    font.pixelSize: 11
                }

                Repeater {
                    model: view.switchResultWarnings
                    visible: view.switchResultWarnings.length > 0
                    delegate: Text {
                        width: resultColumn.width
                        wrapMode: Text.WordWrap
                        text: "⚠ " + modelData
                        color: view.theme.muted
                        font.pixelSize: 11
                    }
                }

                Rectangle {
                    width: okLabel.implicitWidth + 40
                    height: 44
                    radius: 9
                    color: view.theme.accent

                    Text {
                        id: okLabel
                        anchors.centerIn: parent
                        text: "Done"
                        color: "#ffffffff"
                        font.pixelSize: 14
                        font.bold: true
                    }

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: view.finishSwitch()
                    }
                }
            }

            // ---- Failure / recovery screen ----
            Column {
                width: resultColumn.width
                spacing: 8
                visible: !view.switchOk

                Text {
                    text: "The switch to " + view.arg + " did not finish"
                    color: view.theme.danger
                    font.pixelSize: 16
                    font.bold: true
                }

                Text {
                    width: parent.width
                    wrapMode: Text.WordWrap
                    text: view.switchData && view.switchData.error ? String(view.switchData.error) : "the operation reported a failure"
                    color: view.theme.foreground
                    font.pixelSize: 13
                }

                Text {
                    width: parent.width
                    visible: view.switchData && view.switchData.completed_steps !== undefined
                    wrapMode: Text.WordWrap
                    text: view.switchData && view.switchData.completed_steps !== undefined ? "steps completed before stopping: " + view.switchData.completed_steps : ""
                    color: view.theme.muted
                    font.pixelSize: 11
                }

                Repeater {
                    model: view.switchResultWarnings
                    visible: view.switchResultWarnings.length > 0
                    delegate: Text {
                        width: resultColumn.width
                        wrapMode: Text.WordWrap
                        text: "⚠ " + modelData
                        color: view.theme.muted
                        font.pixelSize: 11
                    }
                }

                // The one-click restore from resume_hint ("switch to `X` to restore").
                Row {
                    spacing: 10

                    Rectangle {
                        visible: view.switchData && view.switchData.resume_hint
                        width: restoreLabel.implicitWidth + 32
                        height: 44
                        radius: 9
                        color: view.theme.accent

                        Text {
                            id: restoreLabel
                            anchors.centerIn: parent
                            text: view.switchData && view.switchData.resume_hint ? String(view.switchData.resume_hint) : "Switch to restore"
                            color: "#ffffffff"
                            font.pixelSize: 14
                            font.bold: true
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: view.startSwitch()
                        }
                    }

                    Rectangle {
                        width: closeLabel.implicitWidth + 32
                        height: 44
                        radius: 9
                        color: view.theme.surfaceAlt
                        border.width: 1
                        border.color: view.theme.border

                        Text {
                            id: closeLabel
                            anchors.centerIn: parent
                            text: "Close"
                            color: view.theme.foreground
                            font.pixelSize: 14
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: view.shell.popToRoot()
                        }
                    }
                }
            }
        }
    }
}
