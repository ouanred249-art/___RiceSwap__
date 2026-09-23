import QtQuick
import Quickshell
import Quickshell.Io

// The panel's whole backend surface: the watched `state.json` document
// (frozen contract: {initialized, active_profile, operation, last_result})
// and one subprocess per operation over the frozen NDJSON stream
// (progress lines, then exactly one {ok, warnings, data} envelope).
//
// The backend binary must be on PATH as `riceswap`; every operation is a
// fresh process (no daemon), per the spec's Language & split decision.
QtObject {
    id: rsBackend

    readonly property string home: {
        const value = Quickshell.env("HOME");
        return value ? value : "";
    }
    readonly property string statePath: home + "/.local/share/riceswap/state.json"

    // ------------------------------------------------------------------
    // state.json, watched reactively (FileView + watchChanges)
    // ------------------------------------------------------------------

    // Bumped on every completed read. The parse bindings below read it
    // first, so each FileView reload re-evaluates them with fresh text.
    property int stateRev: 0

    property FileView stateFile: FileView {
        id: stateView
        path: rsBackend.statePath
        preload: true
        printErrors: false
        watchChanges: true
        onFileChanged: stateView.reload()
    }

    Connections {
        target: stateView
        function onDataChanged() {
            rsBackend.stateRev++;
        }
    }

    readonly property var stateDoc: {
        const revision = rsBackend.stateRev;
        const raw = stateView.text();
        if (!raw)
            return null;
        try {
            return JSON.parse(raw);
        } catch (error) {
            return null;
        }
    }

    // The onboarding gate: only `initialized === true` counts. A missing or
    // unparsable state.json is therefore "not initialized", by contract.
    readonly property bool initialized: stateDoc !== null && stateDoc.initialized === true

    // The Active badge source. The contract writes null when nothing is
    // active; the QML reads "" for that case.
    readonly property string activeProfile: {
        const doc = rsBackend.stateDoc;
        return doc !== null && typeof doc.active_profile === "string" ? doc.active_profile : "";
    }

    // The running-operation object {name, target, started_at, step} or null.
    // Consumed shape for the progress work later tickets build on; every key
    // is pinned by tests/contracts.rs.
    readonly property var runningOperation: {
        const doc = rsBackend.stateDoc;
        return doc !== null && doc.operation ? doc.operation : null;
    }

    // ------------------------------------------------------------------
    // The operation runner: one Process, sequential operations
    // ------------------------------------------------------------------

    property bool busy: false
    property string runningOp: ""
    property string progressMessage: ""
    property int progressStep: 0
    property var lastEnvelope: null
    property var pendingEnvelope: null

    // Emitted once per operation with the final envelope (fabricated as a
    // failure when the process died before reporting one).
    signal operationFinished(string operation, bool ok, var envelope)

    property Process opProcess: Process {
        stdout: SplitParser {
            onRead: line => rsBackend.handleLine(line)
        }
        onExited: (exitCode, exitStatus) => rsBackend.handleExit(exitCode)
    }

    function handleLine(line) {
        if (!line)
            return;
        var parsed;
        try {
            parsed = JSON.parse(line);
        } catch (error) {
            return;
        }
        if (parsed && typeof parsed.ok === "boolean") {
            pendingEnvelope = parsed;
        } else if (parsed && parsed.progress) {
            progressMessage = parsed.progress.message ? String(parsed.progress.message) : "";
            progressStep = parsed.progress.step ? parsed.progress.step : 0;
        }
    }

    function handleExit(exitCode) {
        // The backend rewrites state.json before it exits; re-read now
        // instead of waiting for the watcher to catch up.
        stateView.reload();
        var envelope = pendingEnvelope;
        if (envelope === null || envelope === undefined) {
            envelope = ({
                ok: false,
                warnings: [],
                data: {
                    error: "the operation exited with code " + exitCode + " before reporting a result"
                }
            });
        }
        pendingEnvelope = null;
        const operation = runningOp;
        busy = false;
        runningOp = "";
        lastEnvelope = envelope;
        operationFinished(operation, envelope.ok, envelope);
    }

    // Starts one operation. Returns false when another one is running, so
    // callers can surface "busy" instead of silently dropping the action.
    function run(operation, args) {
        if (busy)
            return false;
        busy = true;
        runningOp = operation;
        progressMessage = "";
        progressStep = 0;
        pendingEnvelope = null;
        opProcess.command = ["riceswap"].concat(args);
        opProcess.running = true;
        return true;
    }

    function runInit() {
        return run("init", ["init"]);
    }

    function runList() {
        return run("list", ["list"]);
    }

    function runInfo(name) {
        return run("info", ["info", name]);
    }

    // Never force: the panel offers no path that deletes the active
    // profile; the backend's refusal message is shown inline instead.
    function runDelete(name) {
        return run("delete", ["delete", name]);
    }

    // ------------------------------------------------------------------
    // Refresh screenshot: grim, run directly from the panel
    // ------------------------------------------------------------------

    // The ten-operation surface is frozen and has no capture operation, so
    // Refresh screenshot spawns `grim <profile>/<screenshot>` itself — the
    // exact command `snapshot` uses — and the card cache-busts its image on
    // success. Judgment call recorded in gui/README.md and the ticket report.
    property bool shotBusy: false
    property string shotTarget: ""
    property string shotError: ""

    signal screenshotRefreshed(string target, bool ok, string message)

    property Process shotProcess: Process {
        stderr: StdioCollector {}
        onExited: (exitCode, exitStatus) => rsBackend.handleShotExit(exitCode)
    }

    function refreshScreenshot(target) {
        if (shotBusy || !target)
            return false;
        shotTarget = target;
        shotError = "";
        shotProcess.command = ["grim", target];
        shotProcess.running = true;
        shotBusy = true;
        return true;
    }

    function handleShotExit(exitCode) {
        shotBusy = false;
        if (exitCode === 0) {
            shotError = "";
            screenshotRefreshed(shotTarget, true, "");
            return;
        }
        const text = shotProcess.stderr && shotProcess.stderr.text ? String(shotProcess.stderr.text) : "";
        const lines = text.split("\n").filter(function (line) {
            return line.trim().length > 0;
        });
        const message = lines.length > 0 ? lines[0].trim() : "grim exited with code " + exitCode;
        shotError = message;
        screenshotRefreshed(shotTarget, false, message);
    }

    // ------------------------------------------------------------------
    // Helpers for the views
    // ------------------------------------------------------------------

    // A properly encoded file:// URL for an absolute path (profile names
    // may contain spaces or `#`, which a raw concatenation would break).
    function fileUrl(path) {
        return "file://" + encodeURI(path).replace(/#/g, "%23").replace(/\?/g, "%3F");
    }
}
