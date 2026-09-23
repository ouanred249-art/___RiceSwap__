import QtQuick
import QtQuick.Effects

// One profile card: 16:9 screenshot thumbnail (grayscale, colour on hover,
// full colour when active), name, rice_info summary line, package-count
// chip, Active badge, and the ⋯ menu trigger. The menu itself is rendered
// by ProfilesView so it can overlay the list; the card only reports what
// was pressed and where the button was.
Rectangle {
    id: card

    property var shell
    property var theme
    property string profileName: ""
    property var manifest: null
    property bool isActive: false
    property bool menuOpen: false

    // Bumped when a refreshed grim run rewrote this card's screenshot, so
    // the Image URL changes and Qt reloads the file instead of the cache.
    property int shotRevision: 0

    // The ProfilesView instance, used to place the ⋯ menu overlay.
    property var viewRoot

    signal activateMenu(string name, real x, real y)
    signal cardPressed()

    readonly property var profileMeta: manifest !== null && manifest.profile ? manifest.profile : ({})
    readonly property var riceInfo: manifest !== null && manifest.rice_info ? manifest.rice_info : ({})
    readonly property var packageInfo: manifest !== null && manifest.packages ? manifest.packages : ({
        official: [],
        aur: []
    })
    readonly property int packageCount: {
        const official = packageInfo.official ? packageInfo.official.length : 0;
        const aur = packageInfo.aur ? packageInfo.aur.length : 0;
        return official + aur;
    }

    // The "bar · terminal · colors" summary line, in the order the spec
    // shows it, plus any extra keys the open rice_info table carries.
    readonly property string riceLine: {
        const ordered = ["bar", "terminal", "colors"];
        const parts = [];
        for (const key of ordered) {
            if (riceInfo[key] !== undefined && riceInfo[key] !== null && String(riceInfo[key]).length > 0)
                parts.push(String(riceInfo[key]));
        }
        for (const key of Object.keys(riceInfo)) {
            if (ordered.indexOf(key) === -1 && riceInfo[key] !== undefined && riceInfo[key] !== null && String(riceInfo[key]).length > 0)
                parts.push(String(riceInfo[key]));
        }
        return parts.join(" · ");
    }

    readonly property string shotName: profileMeta.screenshot ? String(profileMeta.screenshot) : "screenshot.png"
    readonly property string shotPath: shell.backend.home + "/.local/share/riceswap/profiles/" + profileName + "/" + shotName

    Connections {
        target: card.shell ? card.shell.backend : null
        function onScreenshotRefreshed(target, ok, message) {
            if (target !== card.shotPath)
                return;
            if (ok)
                card.shotRevision++;
        }
    }

    radius: 10
    clip: true
    color: card.theme.surface
    border.width: 1
    border.color: hover.hovered || card.menuOpen ? card.theme.border : Qt.rgba(0, 0, 0, 0)

    // 16:9 thumbnail + text area + padding.
    height: Math.round(width * 9 / 16) + 10 + infoArea.implicitHeight + 12

    Behavior on border.color {
        ColorAnimation {
            duration: 120
        }
    }

    HoverHandler {
        id: hover
    }

    // Whole-card press: closes an open menu (declared first so the ⋯
    // button and everything else above it keep their clicks; the menu
    // overlay itself lives in ProfilesView, above all of this).
    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onClicked: card.cardPressed()
    }

    // ------------------------------------------------------------------
    // Thumbnail (16:9)
    // ------------------------------------------------------------------

    Item {
        id: thumbWrap
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        height: card.width * 9 / 16

        // Backing plate: visible under the image while it loads, and the
        // "no screenshot" backdrop when the file is missing entirely.
        Rectangle {
            anchors.fill: parent
            color: card.theme.surfaceAlt
        }

        Text {
            anchors.centerIn: parent
            visible: thumb.status === Image.Error || thumb.status === Image.Null
            text: "No screenshot"
            color: card.theme.muted
            font.pixelSize: 12
        }

        Image {
            id: thumb
            anchors.fill: parent
            source: card.shotPath ? card.shell.backend.fileUrl(card.shotPath) + "?rev=" + card.shotRevision : ""
            fillMode: Image.PreserveAspectCrop
            asynchronous: true

            // Grayscale at rest, full colour on hover or when active.
            layer.enabled: true
            layer.effect: MultiEffect {
                saturation: card.isActive || hover.hovered ? 0 : -1

                Behavior on saturation {
                    NumberAnimation {
                        duration: 200
                        easing.type: Easing.OutCubic
                    }
                }
            }
        }

        // ⋯ menu trigger, top-right of the thumbnail.
        Rectangle {
            id: menuBtn
            anchors.top: parent.top
            anchors.right: parent.right
            anchors.margins: 8
            width: 30
            height: 30
            radius: 7
            color: card.menuOpen ? card.theme.surfaceAlt : Qt.rgba(0, 0, 0, 0.45)
            border.width: 1
            border.color: card.theme.border

            Text {
                anchors.centerIn: parent
                text: "⋯"
                color: card.theme.foreground
                font.pixelSize: 16
            }

            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: {
                    if (card.viewRoot) {
                        const point = menuBtn.mapToItem(card.viewRoot, menuBtn.width, menuBtn.height);
                        card.activateMenu(card.profileName, point.x, point.y);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Text area
    // ------------------------------------------------------------------

    Column {
        id: infoArea
        anchors.top: thumbWrap.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.margins: 12
        anchors.topMargin: 10
        spacing: 5

        Item {
            width: parent.width
            height: Math.max(nameText.implicitHeight, activeBadge.height)

            Text {
                id: nameText
                anchors.left: parent.left
                anchors.right: activeBadge.visible ? activeBadge.left : parent.right
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                elide: Text.ElideRight
                text: card.profileName
                color: card.theme.foreground
                font.pixelSize: 15
                font.bold: true
            }

            Rectangle {
                id: activeBadge
                visible: card.isActive
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                width: badgeText.implicitWidth + 16
                height: 22
                radius: 11
                color: card.theme.accent

                Text {
                    id: badgeText
                    anchors.centerIn: parent
                    text: "Active"
                    color: card.theme.background
                    font.pixelSize: 11
                    font.bold: true
                }
            }
        }

        Text {
            width: parent.width
            elide: Text.ElideRight
            text: card.riceLine.length > 0 ? card.riceLine : (card.profileMeta.description ? String(card.profileMeta.description) : "no rice_info")
            color: card.theme.muted
            font.pixelSize: 12
        }

        Item {
            width: parent.width
            height: 24

            Rectangle {
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                width: pkgText.implicitWidth + 16
                height: 22
                radius: 11
                color: card.theme.surfaceAlt
                border.width: 1
                border.color: card.theme.border

                Text {
                    id: pkgText
                    anchors.centerIn: parent
                    text: card.packageCount + (card.packageCount === 1 ? " package" : " packages")
                    color: card.theme.muted
                    font.pixelSize: 11
                }
            }
        }
    }
}
