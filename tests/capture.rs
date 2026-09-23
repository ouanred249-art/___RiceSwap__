//! Ticket #13: the snapshot pipeline through the operation surface — what
//! `detect` proposes on a fixture fake `$HOME` (allowlisted dirs,
//! config-referenced dirs including lua-sourced refs, `pacman -Qo`-resolvable
//! binaries, asset dirs, Downloads wallpapers) and what `snapshot` writes
//! (mirrored files, the locked manifest, the grim screenshot, the hardware
//! `source =` line exactly once, collision refusal with `--force` overwrite,
//! fork semantics while a profile is active, and hand-added `optional`
//! entries that load correctly).

mod common;

use common::{Sandbox, image_fixture, manifest_toml};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::symlink;

/// The live Hyprland config of the detection fixture: an allowlisted
/// `source =` reference, non-path `exec-once` commands, a path-valued
/// `exec-once`, and a `bind`-embedded `exec` — every reference shape the
/// format-agnostic scan has to understand.
const HYPRLAND_CONF: &str = "\
# my live rice
source = ~/.config/hypr/conf.d/apps.conf
exec-once = waybar
exec-once = ags
exec-once = not-a-real-binary
bind = SUPER, Q, exec, kitty
exec-once = ~/.config/ricekit/launch.sh
";

/// Everything the detection fixture proposes, laid out under the fake `$HOME:
/// allowlisted config dirs, a dir referenced only through the Hyprland config
/// (reachable only via a lua `source` chain), asset dirs, and Downloads
/// wallpapers — plus the `pacman -Qo` owner fixture behind the package split.
fn detection_fixture(sandbox: &Sandbox) {
    sandbox.write_home(".config/hypr/hyprland.conf", HYPRLAND_CONF);
    sandbox.write_home(".config/waybar/config.jsonc", "{}\n");
    sandbox.write_home(".config/kitty/kitty.conf", "font_size 12\n");
    sandbox.write_home(
        ".config/hypr/conf.d/apps.conf",
        "source = ~/.config/hypr/conf.d/lua-init.lua\nexec-once = swaync\n",
    );
    sandbox.write_home(
        ".config/hypr/conf.d/lua-init.lua",
        "-- lua-sourced config\ndofile(os.getenv(\"HOME\") .. \"/.config/lua-rico/init.lua\")\n",
    );
    sandbox.write_home(".config/lua-rico/init.lua", "-- rice lua module\n");
    sandbox.write_home(".config/ricekit/launch.sh", "#!/bin/sh\ntrue\n");
    sandbox.write_home(".config/random/rice.conf", "not a rice dir\n");

    sandbox.write_home(".local/share/fonts/Inter-Regular.ttf", "font bytes");
    sandbox.write_home(
        ".local/share/icons/Tokyo-Night/index.theme",
        "[Icon Theme]\n",
    );
    sandbox.write_home("Downloads/dawn.png", image_fixture());
    sandbox.write_home("Downloads/peak.jpg", image_fixture());
    sandbox.write_home("Downloads/notes.txt", "not a wallpaper\n");

    sandbox.own("waybar", "waybar", false);
    sandbox.own("kitty", "kitty", false);
    sandbox.own("ags", "ags", true);
    sandbox.own("swaync", "swaync", false);
}

/// A small rice for the snapshot tests: four config dirs, one font, and the
/// owner fixture the manifest's package split is asserted against.
fn snapshot_fixture(sandbox: &Sandbox) {
    sandbox.write_home(
        ".config/hypr/hyprland.conf",
        "monitor=DP-1,2560x1440@144,0x0,1\nexec-once = waybar\nbind = SUPER, Q, exec, kitty\n",
    );
    sandbox.write_home(
        ".config/waybar/config.jsonc",
        "{ \"on-click\": \"pavucontrol\" }\n",
    );
    sandbox.write_home(".config/kitty/kitty.conf", "font_size 12\n");
    sandbox.write_home(".config/matugen/config.toml", "template = default\n");
    sandbox.write_home(".local/share/fonts/Inter-Regular.ttf", "font bytes");
    sandbox.own("waybar", "waybar", false);
    sandbox.own("kitty", "kitty", false);
    sandbox.own("matugen", "matugen", false);
}

/// The `$HOME`-relative paths a profile captured from [`snapshot_fixture`],
/// in the order the manifest reports them.
fn expected_paths() -> Value {
    json!([
        ".config/hypr",
        ".config/kitty",
        ".config/matugen",
        ".config/waybar",
        ".local/share/fonts",
    ])
}

/// Class: pre-flight detection. Allowlisted dirs and dirs referenced from
/// the Hyprland config both land in the proposal — a lua `source` chain
/// included — while an unrelated dir under `~/.config` stays out of it.
#[test]
fn detect_proposes_allowlisted_and_config_referenced_dirs() {
    let sandbox = Sandbox::new();
    detection_fixture(&sandbox);

    let data = sandbox.run(&["detect"]).assert_ok();

    for expected in [
        ".config/hypr",
        ".config/waybar",
        ".config/kitty",
        ".config/ricekit",
        ".config/lua-rico",
    ] {
        assert!(
            data["config_dirs"]
                .as_array()
                .is_some_and(|dirs| dirs.iter().any(|dir| dir == expected)),
            "config_dirs must propose {expected}: {}",
            data["config_dirs"]
        );
    }
    assert!(
        !data["config_dirs"]
            .as_array()
            .is_some_and(|dirs| dirs.iter().any(|dir| dir == ".config/random")),
        "a dir that is neither allowlisted nor referenced must stay out: {}",
        data["config_dirs"]
    );
    for tool in ["pacman", "yay", "paru", "hyprctl", "grim"] {
        assert_eq!(
            data["tools"][tool]["exit_code"],
            json!(0),
            "{tool} was not probed"
        );
    }
}

/// Class: pre-flight detection. Binary references in the detected configs
/// resolve through the `pacman -Qo` stub, and `pacman -Qm` splits them into
/// official and AUR packages; a binary nobody owns is dropped.
#[test]
fn detect_resolves_binary_references_into_an_official_aur_split() {
    let sandbox = Sandbox::new();
    detection_fixture(&sandbox);

    let data = sandbox.run(&["detect"]).assert_ok();

    assert_eq!(
        data["packages"]["official"],
        json!(["kitty", "swaync", "waybar"]),
        "unowned `not-a-real-binary` never reaches the proposal: {}",
        data["packages"]
    );
    assert_eq!(data["packages"]["aur"], json!(["ags"]));
    assert!(
        sandbox.log().iter().any(|line| {
            line.starts_with("pacman -Qo ")
                && (line.ends_with("waybar") || line.ends_with("/waybar"))
        }),
        "binaries resolve via `pacman -Qo`: {:?}",
        sandbox.log()
    );
    assert!(
        sandbox.log_contains("pacman -Qm"),
        "the AUR split queries foreign packages: {:?}",
        sandbox.log()
    );
}

/// Class: pre-flight detection. Fonts and icons are proposed from the
/// standard asset locations, and image files in `~/Downloads` come back as
/// wallpaper-import candidates (non-images excluded).
#[test]
fn detect_proposes_asset_dirs_and_downloads_wallpapers() {
    let sandbox = Sandbox::new();
    detection_fixture(&sandbox);

    let data = sandbox.run(&["detect"]).assert_ok();

    assert_eq!(
        data["assets"],
        json!([".local/share/fonts", ".local/share/icons"]),
        "an absent themes dir is not proposed"
    );
    assert_eq!(
        data["wallpapers"],
        json!([
            sandbox
                .home()
                .join("Downloads/dawn.png")
                .display()
                .to_string(),
            sandbox
                .home()
                .join("Downloads/peak.jpg")
                .display()
                .to_string(),
        ]),
        "Downloads images are wallpaper-import candidates"
    );
    assert!(
        !data["wallpapers"]
            .as_array()
            .is_some_and(|wallpapers| wallpapers
                .iter()
                .any(|path| path.to_string().contains("notes.txt"))),
        "a non-image never becomes a wallpaper candidate: {}",
        data["wallpapers"]
    );
}

/// Class: profile write. `snapshot` mirrors the selected paths preserving the
/// `$HOME` layout, writes the manifest against the locked schema (package
/// split, `pkill` default stop, auto-filled rice_info, no `optional` flag),
/// and captures the screenshot through the `grim` stub.
#[test]
fn snapshot_writes_the_profile_with_mirrored_files_manifest_and_screenshot() {
    let sandbox = Sandbox::new();
    snapshot_fixture(&sandbox);

    let run = sandbox.run(&["snapshot", "demo"]);
    let data = run.assert_ok();

    assert_eq!(data["manifest_written"], json!(true));
    assert_eq!(data["forked"], json!(false));
    assert_eq!(data["checked_paths"], expected_paths());
    assert_eq!(
        data["checked_packages"]["official"],
        json!(["kitty", "matugen", "waybar"])
    );
    assert_eq!(data["checked_packages"]["aur"], json!([]));

    let profile = sandbox.profile_dir("demo");
    let live = fs::read_to_string(sandbox.home().join(".config/hypr/hyprland.conf"))
        .expect("read the live config");
    let captured = fs::read_to_string(profile.join(".config/hypr/hyprland.conf"))
        .expect("the Hyprland config was mirrored");
    assert!(
        captured.contains("monitor=DP-1,2560x1440@144,0x0,1"),
        "the mirrored config keeps the live content:\n{captured}"
    );
    assert_eq!(
        captured.trim_end(),
        format!("{live}\nsource = ~/.config/hypr/riceswap/hardware.conf").trim_end(),
        "the captured config is the live one plus the hardware source line"
    );
    for mirrored in [
        ".config/waybar/config.jsonc",
        ".config/kitty/kitty.conf",
        ".config/matugen/config.toml",
        ".local/share/fonts/Inter-Regular.ttf",
    ] {
        assert!(
            profile.join(mirrored).is_file(),
            "{mirrored} was not mirrored into the profile"
        );
    }

    let screenshot = profile.join("screenshot.png");
    assert!(screenshot.is_file(), "grim did not write the screenshot");
    assert_eq!(
        data["screenshot"],
        json!(screenshot.display().to_string()),
        "the envelope names the captured screenshot"
    );
    assert!(
        sandbox.log_contains(&format!("grim {}", screenshot.display())),
        "the screenshot is captured via the grim stub: {:?}",
        sandbox.log()
    );

    let info = sandbox.run(&["info", "demo"]).assert_ok();
    let manifest = &info["manifest"];
    assert_eq!(manifest["manifest_version"], json!(1));
    assert_eq!(manifest["profile"]["name"], json!("demo"));
    assert_eq!(manifest["profile"]["screenshot"], json!("screenshot.png"));
    assert_eq!(
        manifest["packages"]["official"],
        json!(["kitty", "matugen", "waybar"])
    );
    assert_eq!(manifest["packages"]["aur"], json!([]));
    assert_eq!(manifest["services"].as_array().expect("services").len(), 1);
    assert_eq!(manifest["services"][0]["name"], json!("waybar"));
    assert_eq!(manifest["services"][0]["start"], json!("waybar"));
    assert_eq!(manifest["services"][0]["stop"], json!("pkill waybar"));
    let manifest_paths: Vec<String> = manifest["files"]
        .as_array()
        .expect("files is a list")
        .iter()
        .map(|entry| {
            entry["path"]
                .as_str()
                .expect("path is a string")
                .to_string()
        })
        .collect();
    assert_eq!(json!(manifest_paths), expected_paths());
    assert_eq!(manifest["rice_info"]["bar"], json!("waybar"));
    assert_eq!(manifest["rice_info"]["terminal"], json!("kitty"));
    assert_eq!(manifest["rice_info"]["colors"], json!("matugen"));

    assert!(
        run.progress().len() >= 4,
        "snapshot streams progress: {:?}",
        run.progress()
    );
    let steps: Vec<u64> = run
        .progress()
        .iter()
        .map(|line| line["step"].as_u64().expect("step is a number"))
        .collect();
    assert!(
        steps.windows(2).all(|pair| pair[0] < pair[1]),
        "steps advance monotonically: {steps:?}"
    );
}

/// Class: hardware layer. The captured Hyprland config gains the shared
/// hardware `source =` line exactly once — appended when the live config
/// does not have it yet, not appended again when it does, and never
/// duplicated by a forced re-snapshot.
#[test]
fn the_hardware_source_line_lands_exactly_once_across_resnapshots() {
    let sandbox = Sandbox::new();
    snapshot_fixture(&sandbox);
    let source_line = "source = ~/.config/hypr/riceswap/hardware.conf";
    let captured = sandbox
        .profile_dir("demo")
        .join(".config/hypr/hyprland.conf");

    sandbox.run(&["snapshot", "demo"]).assert_ok();
    let content = fs::read_to_string(&captured).expect("read the captured config");
    assert_eq!(
        content.matches(source_line).count(),
        1,
        "the source line lands exactly once:\n{content}"
    );

    sandbox.run(&["snapshot", "demo", "--force"]).assert_ok();
    let content = fs::read_to_string(&captured).expect("read the captured config");
    assert_eq!(
        content.matches(source_line).count(),
        1,
        "a re-snapshot must not duplicate the source line:\n{content}"
    );

    // A live config that already sources the hardware layer (after `init`)
    // contributes its line verbatim: copied once, never appended again.
    sandbox.write_home(
        ".config/hypr/hyprland.conf",
        format!("exec-once = waybar\n{source_line}\n"),
    );
    sandbox
        .run(&["snapshot", "from-live", "--force"])
        .assert_ok();
    let from_live = fs::read_to_string(
        sandbox
            .profile_dir("from-live")
            .join(".config/hypr/hyprland.conf"),
    )
    .expect("read the second captured config");
    assert_eq!(
        from_live.matches(source_line).count(),
        1,
        "an already-sourced live config is captured as-is:\n{from_live}"
    );
}

/// Class: collisions. A taken name refuses through a failed envelope that
/// names the profile and the flag; `--force` overwrites the files, packages,
/// and services while keeping the name and directory — and the active
/// profile is never overwritten in place, force included.
#[test]
fn a_name_collision_refuses_unless_forced_and_force_keeps_name_and_dir() {
    let sandbox = Sandbox::new();
    sandbox.write_home(".config/hypr/hyprland.conf", "exec-once = waybar\n");
    sandbox.own("waybar", "waybar", false);
    sandbox.run(&["snapshot", "demo"]).assert_ok();

    // The desktop changes: the next capture has different services to record.
    sandbox.write_home(".config/hypr/hyprland.conf", "exec-once = swaync\n");
    sandbox.own("swaync", "swaync", false);

    let message = sandbox.run(&["snapshot", "demo"]).assert_failed();
    assert!(message.contains("demo"), "names the offender: {message}");
    assert!(
        message.contains("--force"),
        "says how to overwrite: {message}"
    );
    assert!(
        fs::read_to_string(
            sandbox
                .profile_dir("demo")
                .join(".config/hypr/hyprland.conf")
        )
        .expect("read the existing profile")
        .contains("waybar"),
        "a refused collision overwrites nothing"
    );

    let data = sandbox.run(&["snapshot", "demo", "--force"]).assert_ok();
    assert_eq!(data["profile"], json!("demo"));
    let mut names: Vec<String> = fs::read_dir(sandbox.profiles_dir())
        .expect("read the profile store")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["demo"],
        "force keeps the name and directory; nothing is renamed aside"
    );

    let info = sandbox.run(&["info", "demo"]).assert_ok();
    assert_eq!(info["manifest"]["profile"]["name"], json!("demo"));
    assert_eq!(
        info["manifest"]["packages"]["official"],
        json!(["swaync"]),
        "force overwrites the package list"
    );
    assert_eq!(info["manifest"]["services"][0]["name"], json!("swaync"));
    assert_eq!(
        info["manifest"]["services"][0]["stop"],
        json!("pkill swaync"),
        "force overwrites the services"
    );
    assert!(
        !fs::read_to_string(
            sandbox
                .profile_dir("demo")
                .join(".config/hypr/hyprland.conf")
        )
        .expect("read the overwritten profile")
        .contains("waybar"),
        "force overwrites the mirrored files"
    );

    sandbox.activate("demo");
    let message = sandbox
        .run(&["snapshot", "demo", "--force"])
        .assert_failed();
    assert!(
        message.contains("active") && message.contains("fork"),
        "the active profile is never mutated in place: {message}"
    );
}

/// Class: fork semantics. Snapshotting while a profile is active captures
/// the live desktop — symlinks dereferenced — into a *new* profile; the
/// active profile's directory and manifest stay byte-identical, and the
/// `current` symlink keeps pointing where it did.
#[test]
fn snapshot_while_a_profile_is_active_forks_and_leaves_the_active_profile_untouched() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("old", &manifest_toml("old"));
    let old_manifest = sandbox.profile_manifest("old");
    let old_config = ".local/share/riceswap/profiles/old/.config/hypr/hyprland.conf";
    sandbox.write_home(old_config, "exec-once = waybar\n");

    // Activation managed `~/.config/hypr` as a symlink into the active
    // profile — the layout a real desktop has while a profile is live.
    let live_hypr = sandbox.home().join(".config/hypr");
    fs::create_dir_all(sandbox.home().join(".config")).expect("create .config");
    symlink(sandbox.profile_dir("old").join(".config/hypr"), &live_hypr)
        .expect("activate the Hyprland config dir");
    sandbox.own("waybar", "waybar", false);
    sandbox.activate("old");

    let data = sandbox.run(&["snapshot", "fork"]).assert_ok();
    assert_eq!(
        data["forked"],
        json!(true),
        "snapshotting over an active profile forks it"
    );

    let forked = fs::read_to_string(
        sandbox
            .profile_dir("fork")
            .join(".config/hypr/hyprland.conf"),
    )
    .expect("the fork captured the live config through the symlink");
    assert!(
        forked.contains("exec-once = waybar"),
        "the fork mirrors the live desktop:\n{forked}"
    );
    assert_eq!(
        forked
            .matches("source = ~/.config/hypr/riceswap/hardware.conf")
            .count(),
        1,
        "the fork's config gets the hardware source line:\n{forked}"
    );

    assert_eq!(
        fs::read_to_string(
            sandbox
                .profile_dir("old")
                .join(".config/hypr/hyprland.conf")
        )
        .expect("read the active profile's config"),
        "exec-once = waybar\n",
        "the active profile is never mutated in place"
    );
    assert_eq!(
        sandbox.profile_manifest("old"),
        old_manifest,
        "the active profile's manifest is byte-identical"
    );
    assert_eq!(
        fs::read_link(&live_hypr).ok(),
        Some(sandbox.profile_dir("old").join(".config/hypr")),
        "the live symlink still points at the active profile"
    );
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("old")),
        "the fork never re-activates anything"
    );
    assert_eq!(sandbox.state()["active_profile"], json!("old"));
}

/// Class: manifest schema. What `snapshot` writes never carries
/// `optional = true` — but a hand-edited or hand-appended entry with the
/// flag loads through `info` as-is, because the flag is the user's alone.
#[test]
fn a_snapshot_manifest_never_generates_optional_but_hand_added_entries_load() {
    let sandbox = Sandbox::new();
    snapshot_fixture(&sandbox);
    sandbox.run(&["snapshot", "demo"]).assert_ok();

    let written = sandbox.profile_manifest("demo");
    assert!(
        !written.contains("optional = true"),
        "snapshot never generates optional = true:\n{written}"
    );
    let info = sandbox.run(&["info", "demo"]).assert_ok();
    let files = info["manifest"]["files"]
        .as_array()
        .expect("files is a list");
    assert!(!files.is_empty(), "the fixture captured files: {files:?}");
    for file in files {
        assert_eq!(file["optional"], json!(false), "generated entry: {file}");
    }

    // Hand-add an entry — flag and all — after the generated manifest.
    let hand_edited = format!("{written}\n[[files]]\npath = \".zshrc\"\noptional = true\n");
    fs::write(
        sandbox.profile_dir("demo").join("profile.toml"),
        hand_edited,
    )
    .expect("hand-edit the manifest");
    let info = sandbox.run(&["info", "demo"]).assert_ok();
    let files = info["manifest"]["files"]
        .as_array()
        .expect("files is a list");
    let last = files.last().expect("the hand-added entry survived");
    assert_eq!(last["path"], json!(".zshrc"));
    assert_eq!(
        last["optional"],
        json!(true),
        "a hand-added optional entry loads correctly: {last}"
    );
}
