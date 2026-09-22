//! The first-run bootstrap and the wallpaper layer through the operation
//! surface: `init` creates the shared layers idempotently, lifts the hardware
//! configuration out of a live Hyprland config into the shared hardware file,
//! and installs the four bundled defaults; `wallpaper-import` moves an image
//! into the shared layer and refuses a non-image through the envelope.

mod common;

use common::{Sandbox, image_fixture};
use serde_json::json;
use std::fs;
use std::path::Path;

/// A live Hyprland config exercising every hardware construct the lift
/// handles: `monitor=` lines, GPU `env =` lines, an `input { ... }` block, and
/// rice-specific lines — theming env and app bindings — that must stay in
/// place.
const HYPRLAND_CONF: &str = "\
# my live rice config
monitor=DP-1,2560x1440@144,0x0,1
monitor=HDMI-A-1,1920x1080@60,2560x0,1
env = LIBVA_DRIVER_NAME,nvidia
env = GBM_BACKEND,nvidia-drm
env = XCURSOR_SIZE,24
env = GTK_THEME,Adwaita-dark
input {
    kb_layout = us
    follow_mouse = 1
}
exec-once = waybar
bind = SUPER, Q, exec, kitty
";

/// The four bundled defaults, by name — their presence in the wallpaper layer
/// is what makes a wallpaper-less profile still look put-together.
const BUNDLED: &[&str] = &[
    "default-dawn.png",
    "default-day.png",
    "default-dusk.png",
    "default-night.png",
];

/// Class: first-run bootstrap. A fresh fake `$HOME` gains every shared layer,
/// the shared hardware file with its permanent symlink, and `state.json`
/// agrees that RiceSwap is initialized.
#[test]
fn init_on_a_fresh_home_creates_every_shared_layer() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["init"]);
    let data = run.assert_ok();

    assert_eq!(data["initialized"], json!(true));
    let created = data["created"].as_array().expect("created is a list");
    assert!(
        !created.is_empty(),
        "a first run must report what it created: {created:?}"
    );

    assert!(
        sandbox.profiles_dir().is_dir(),
        "the profile layer is there"
    );
    assert!(
        sandbox.wallpapers_dir().is_dir(),
        "the wallpaper layer is there"
    );
    assert!(
        sandbox.hardware_file().is_file(),
        "the shared hardware file exists even with no live config"
    );
    assert_eq!(
        fs::read_link(sandbox.hardware_link()).ok(),
        Some(sandbox.hardware_file()),
        "the permanent symlink points at the shared hardware file"
    );
    assert_eq!(
        sandbox.state()["initialized"],
        json!(true),
        "state.json mirrors the bootstrap"
    );

    let progress = run.progress();
    assert!(
        progress.len() >= 3,
        "bootstrap streams progress: {progress:?}"
    );
    run.assert_no_panic();
}

/// Class: first-run bootstrap. The second run is a no-op: it creates nothing,
/// and every file and symlink under the fake `$HOME` keeps its exact content —
/// hardware extraction included.
#[test]
fn init_is_idempotent_a_second_run_changes_nothing() {
    let sandbox = Sandbox::new();
    sandbox.write_home(".config/hypr/hyprland.conf", HYPRLAND_CONF);

    let first = sandbox.run(&["init"]).assert_ok();
    assert!(
        !first["created"].as_array().is_some_and(Vec::is_empty),
        "the first run reports what it created: {first:?}"
    );
    let after_first = sandbox.home_tree();

    let second = sandbox.run(&["init"]).assert_ok();
    assert_eq!(
        second["created"],
        json!([]),
        "a second run creates nothing: {second:?}"
    );
    assert_eq!(
        sandbox.home_tree(),
        after_first,
        "a second run changes nothing under the fake $HOME"
    );

    let hardware = fs::read_to_string(sandbox.hardware_file()).expect("read hardware file");
    assert_eq!(
        hardware.matches("monitor=DP-1").count(),
        1,
        "re-running must not duplicate the lifted blocks"
    );
}

/// Class: hardware extraction. `monitor=`, GPU `env =`, and the `input`
/// block are lifted into the shared hardware file; the live config keeps its
/// rice-specific lines and gains the `source =` line through which every
/// profile inherits the hardware setup.
#[test]
fn init_lifts_hardware_blocks_into_the_shared_hardware_file() {
    let sandbox = Sandbox::new();
    sandbox.write_home(".config/hypr/hyprland.conf", HYPRLAND_CONF);
    sandbox.run(&["init"]).assert_ok();

    let hardware = fs::read_to_string(sandbox.hardware_file()).expect("read hardware file");
    for lifted in [
        "monitor=DP-1,2560x1440@144,0x0,1",
        "monitor=HDMI-A-1,1920x1080@60,2560x0,1",
        "env = LIBVA_DRIVER_NAME,nvidia",
        "env = GBM_BACKEND,nvidia-drm",
        "input {",
        "kb_layout = us",
        "follow_mouse = 1",
    ] {
        assert!(
            hardware.contains(lifted),
            "the shared hardware file must contain {lifted:?}:\n{hardware}"
        );
    }
    for stayed in ["exec-once = waybar", "bind = SUPER"] {
        assert!(
            !hardware.contains(stayed),
            "{stayed:?} is not hardware and must not be lifted:\n{hardware}"
        );
    }
    for stayed in ["XCURSOR_SIZE", "GTK_THEME"] {
        assert!(
            !hardware.contains(stayed),
            "{stayed:?} is rice-specific theming, not GPU env; it must stay in \
             hyprland.conf:\n{hardware}"
        );
    }

    let live = fs::read_to_string(sandbox.hyprland_config()).expect("read live config");
    for lifted in ["monitor=DP-1", "env = LIBVA_DRIVER_NAME", "kb_layout = us"] {
        assert!(
            !live.contains(lifted),
            "{lifted:?} was lifted out of the live config:\n{live}"
        );
    }
    for stayed in [
        "exec-once = waybar",
        "bind = SUPER, Q",
        "env = XCURSOR_SIZE,24",
        "env = GTK_THEME,Adwaita-dark",
    ] {
        assert!(
            live.contains(stayed),
            "the live config keeps {stayed:?}:\n{live}"
        );
    }
    assert!(
        live.contains("source = ~/.config/hypr/riceswap/hardware.conf"),
        "the live config sources the hardware layer through the permanent \
         symlink:\n{live}"
    );
    assert_eq!(
        fs::read_link(sandbox.hardware_link()).ok(),
        Some(sandbox.hardware_file()),
        "the permanent symlink under the Hyprland config dir points at the \
         shared hardware file"
    );
}

/// Class: hardware extraction. A live config with nothing hardware in it is
/// still connected to the shared hardware layer: the `source =` line lands
/// exactly once, and a re-run does not append it again.
#[test]
fn a_live_config_without_hardware_directives_still_sources_the_hardware_layer() {
    let sandbox = Sandbox::new();
    sandbox.write_home(
        ".config/hypr/hyprland.conf",
        "exec-once = waybar\nbind = SUPER, Q, exec, kitty\n",
    );
    let source_line = "source = ~/.config/hypr/riceswap/hardware.conf";

    sandbox.run(&["init"]).assert_ok();
    let live = fs::read_to_string(sandbox.hyprland_config()).expect("read live config");
    assert!(
        live.contains(source_line),
        "even with nothing to lift, the live config must source the hardware \
         layer:\n{live}"
    );
    assert!(
        live.contains("exec-once = waybar"),
        "the untouched lines stay:\n{live}"
    );
    assert_eq!(
        live.matches(source_line).count(),
        1,
        "the source line lands exactly once:\n{live}"
    );
    assert!(
        sandbox.hardware_file().is_file(),
        "the shared hardware file exists for the source line to reach"
    );

    sandbox.run(&["init"]).assert_ok();
    let live = fs::read_to_string(sandbox.hyprland_config()).expect("read live config");
    assert_eq!(
        live.matches(source_line).count(),
        1,
        "a re-run must not append the source line twice:\n{live}"
    );
}

/// Class: bundled wallpapers. `init` leaves exactly the four bundled defaults
/// in the shared wallpaper layer, each a real PNG.
#[test]
fn init_installs_the_four_bundled_default_wallpapers() {
    let sandbox = Sandbox::new();
    sandbox.run(&["init"]).assert_ok();

    let mut names: Vec<String> = fs::read_dir(sandbox.wallpapers_dir())
        .expect("read the wallpaper layer")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(names, BUNDLED, "exactly the four bundled defaults");

    for name in BUNDLED {
        let bytes = fs::read(sandbox.wallpapers_dir().join(name)).expect("read bundled default");
        assert!(!bytes.is_empty(), "{name} is empty");
        assert_eq!(
            &bytes[..8],
            b"\x89PNG\r\n\x1a\n",
            "{name} is a real PNG, not a placeholder"
        );
    }
}

/// Class: first-run bootstrap. No packaged GUI config on this machine (a test
/// sandbox is not a packaged install): `init` still succeeds, and the absence
/// is not even worth a warning.
#[test]
fn an_absent_packaged_gui_config_does_not_fail_init() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["init"]);

    run.assert_ok();
    run.assert_no_panic();
    assert!(
        !run.warnings()
            .iter()
            .any(|warning| warning.contains("quickshell")),
        "an absent packaged GUI config is expected, not warned about: {:?}",
        run.warnings()
    );
}

/// Class: asset import. An image is moved — not copied — into the shared
/// wallpaper layer, and the envelope reports both ends of the move.
#[test]
fn wallpaper_import_moves_an_image_into_the_shared_layer() {
    let sandbox = Sandbox::new();
    let source = sandbox.write_home("Downloads/forest.png", image_fixture());

    let run = sandbox.run(&["wallpaper-import", source.to_str().expect("utf-8 path")]);
    let data = run.assert_ok();

    assert_eq!(data["source"], json!(source.display().to_string()));
    let imported = sandbox.wallpapers_dir().join("forest.png");
    assert_eq!(
        data["imported_to"],
        json!(imported.display().to_string()),
        "the envelope names where the image landed"
    );
    assert!(
        !source.exists(),
        "the source image was moved out of Downloads"
    );
    assert_eq!(
        fs::read(&imported).expect("read the imported image"),
        image_fixture(),
        "the image arrived with its bytes intact"
    );
    assert!(
        run.progress().len() >= 2,
        "import streams progress: {:?}",
        run.progress()
    );
}

/// Class: asset import. A non-image refuses through the failed envelope, with
/// a message naming the file — and nothing is created or moved.
#[test]
fn wallpaper_import_refuses_a_non_image_through_the_envelope() {
    let sandbox = Sandbox::new();
    let notes = sandbox.write_home("Downloads/notes.txt", "just some notes");

    let run = sandbox.run(&["wallpaper-import", notes.to_str().expect("utf-8 path")]);
    let message = run.assert_failed();

    assert!(
        message.contains("notes.txt"),
        "the refusal must name the file: {message}"
    );
    assert!(
        message.contains("image"),
        "the refusal must say why: {message}"
    );
    assert!(notes.exists(), "a refused file stays where it was");
    assert!(
        !sandbox.wallpapers_dir().exists(),
        "a refused import never touches the wallpaper layer"
    );
    run.assert_no_panic();
}

/// Class: asset import. An image extension is not enough: a file whose
/// contents are not an image is refused even when its name claims otherwise.
#[test]
fn wallpaper_import_refuses_a_non_image_wearing_an_image_extension() {
    let sandbox = Sandbox::new();
    let fake = sandbox.write_home("Downloads/fake.png", "#!/bin/sh\necho not an image\n");

    let run = sandbox.run(&["wallpaper-import", fake.to_str().expect("utf-8 path")]);
    let message = run.assert_failed();

    assert!(
        message.contains("fake.png"),
        "the refusal must name the file: {message}"
    );
    assert!(
        message.contains("image"),
        "the refusal must say why: {message}"
    );
    assert!(
        fake.exists(),
        "a payload wearing an image extension is never moved"
    );
    assert!(
        !sandbox.wallpapers_dir().exists(),
        "a refused import never touches the wallpaper layer"
    );
    run.assert_no_panic();
}

/// Class: asset import. A path that does not exist is a failed envelope naming
/// it, not a crash and not a silent success.
#[test]
fn wallpaper_import_refuses_a_path_that_does_not_exist() {
    let sandbox = Sandbox::new();
    let missing = sandbox.home().join("Downloads/ghost.png");

    let run = sandbox.run(&["wallpaper-import", missing.to_str().expect("utf-8 path")]);
    let message = run.assert_failed();

    assert!(
        message.contains("ghost.png"),
        "the refusal must name the path: {message}"
    );
    run.assert_no_panic();
}

/// Class: shared layers. The wallpaper layer lives exactly where the spec
/// pins it, so a profile referencing a path there survives any switch.
#[test]
fn the_wallpaper_layer_lives_in_the_shared_data_directory() {
    let sandbox = Sandbox::new();
    let source = sandbox.write_home("wall.jpg", image_fixture());
    let data = sandbox
        .run(&["wallpaper-import", source.to_str().expect("utf-8 path")])
        .assert_ok();

    let imported = data["imported_to"].as_str().expect("path string");
    let imported = Path::new(imported);
    assert!(
        imported.starts_with(sandbox.data_dir().join("wallpapers")),
        "imports land in ~/.local/share/riceswap/wallpapers, got {}",
        imported.display()
    );
}
