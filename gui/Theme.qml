import QtQuick
import Quickshell
import Quickshell.Io

// The panel palette.
//
// Follows the active profile's colors when (and only when) a profile is
// active AND its palette file parses; otherwise the bundled neutral-dark
// fallback. Two judgment calls, recorded in gui/README.md:
//
// - The watched palette file is pywal's documented `~/.cache/wal/colors.json`
//   ({special:{background,foreground}, colors:{color0..15}}). matugen, the
//   other candidate named by the spec, has no canonical stable-path colors
//   file — its output is template-defined — so it cannot be a contract.
// - The neutral-dark fallback constants are placeholder asset values, the
//   spec calls the fallback palette "an asset to source during
//   implementation".
QtObject {
    id: theme

    // Bound by shell.qml from state.json: "" when no profile is active.
    property string activeProfile: ""

    readonly property string home: {
        const value = Quickshell.env("HOME");
        return value ? value : "";
    }

    // Bundled neutral-dark fallback (placeholder asset).
    readonly property color fallbackBackground: "#ff141419"
    readonly property color fallbackSurface: "#ff1c1c23"
    readonly property color fallbackSurfaceAlt: "#ff24242d"
    readonly property color fallbackBorder: "#ff33333f"
    readonly property color fallbackForeground: "#ffe8e8ef"
    readonly property color fallbackMuted: "#ff8f8f9c"
    readonly property color fallbackAccent: "#ff89b4fa"
    readonly property color fallbackDanger: "#fff47184"
    readonly property color fallbackSuccess: "#ff9ece6a"

    // ------------------------------------------------------------------
    // pywal palette, watched reactively
    // ------------------------------------------------------------------

    property int paletteRev: 0

    property FileView paletteFile: FileView {
        id: paletteView
        path: theme.home + "/.cache/wal/colors.json"
        preload: true
        printErrors: false
        watchChanges: true
        onFileChanged: paletteView.reload()
    }

    property Connections paletteConn: Connections {
        target: paletteView
        function onDataChanged() {
            theme.paletteRev++;
        }
    }

    readonly property var palette: {
        const revision = theme.paletteRev;
        const raw = paletteView.text();
        if (!raw)
            return null;
        try {
            const doc = JSON.parse(raw);
            if (!doc || !doc.special || !doc.special.background || !doc.special.foreground)
                return null;
            if (!doc.colors || !doc.colors.color0 || !doc.colors.color1 || !doc.colors.color2 || !doc.colors.color4)
                return null;
            return doc;
        } catch (error) {
            return null;
        }
    }

    // The theming gate: active profile exists AND the palette parses.
    readonly property bool usePalette: activeProfile !== "" && palette !== null

    // ------------------------------------------------------------------
    // The resolved palette
    // ------------------------------------------------------------------

    readonly property color background: usePalette ? palette.special.background : fallbackBackground
    readonly property color foreground: usePalette ? palette.special.foreground : fallbackForeground

    // Surfaces/borders/muted are derived from background/foreground so any
    // palette stays coherent instead of picking arbitrary ANSI slots.
    readonly property color surface: usePalette ? Qt.lighter(background, 1.14) : fallbackSurface
    readonly property color surfaceAlt: usePalette ? Qt.lighter(background, 1.32) : fallbackSurfaceAlt
    readonly property color border: usePalette ? Qt.rgba(foreground.r, foreground.g, foreground.b, 0.16) : fallbackBorder
    readonly property color muted: usePalette ? Qt.rgba(foreground.r, foreground.g, foreground.b, 0.62) : fallbackMuted

    // Accents do take ANSI slots: pywal orders them by ANSI role
    // (1 red, 2 green, 4 blue), which is the closest thing to a canonical
    // accent/success/danger a pywal document has.
    readonly property color accent: usePalette ? palette.colors.color4 : fallbackAccent
    readonly property color danger: usePalette ? palette.colors.color1 : fallbackDanger
    readonly property color success: usePalette ? palette.colors.color2 : fallbackSuccess
}
