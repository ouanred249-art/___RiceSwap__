hl.bind("CTRL+SUPER+ALT+Slash", hl.dsp.exec_cmd("xdg-open ~/.config/hypr/custom/keybinds.lua"), {description = "Edit user keybinds"} )

-- Region screenshot on CTRL+K (same as SUPER+SHIFT+S)
hl.bind("CTRL + K", hl.dsp.global("caelestia:screenshotClip"), { description = "Utilities: Screen snip" })

-- (scroll-overview removed 2026-09-05: plugin unloaded, all custom binds reverted.
--  SUPER+G is back to stock = widget overlay. HyprGlass untouched.)
