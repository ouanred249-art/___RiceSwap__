-- Terminal glass look (matches video: transparent kitty/foot + blur behind)
-- Base file hyprland/rules.lua disables blur for ALL windows (no_blur=true).
-- These custom rules re-enable blur + opacity for terminals only.
-- Later rules win, so these override the global no_blur.

-- Video rule: HyprGlass REPLACES Hyprland blur (it sets no_blur on glass windows itself).
-- Do NOT re-enable Hyprland blur on glass windows (they won't stack + causes peekaboo bug).
-- Base file already sets no_blur=true globally: leave it. Opacity stays so text floats over glass.

-- Glass opacity: more transparent so wallpaper shows through on any wallpaper
hl.window_rule({ match = { class = "^(kitty)$" }, opacity = "0.78 0.82" })
hl.window_rule({ match = { class = "^(foot)$" }, opacity = "0.78 0.82" })
hl.window_rule({ match = { class = "^(com.mitchellh.ghostty)$" }, opacity = "0.78 0.82" })
hl.window_rule({ match = { class = "^(Alacritty)$" }, opacity = "0.78 0.82" })

-- Keep terminals nice: no shadow pop on tiled is already handled,
-- but ensure rounding matches decoration
hl.window_rule({ match = { class = "^(kitty)$" }, rounding = 12 })
hl.window_rule({ match = { class = "^(foot)$" }, rounding = 12 })

-- Liquid Glass: floating windows get glass opacity
-- (tiled terminals above already have 0.82/0.85)
hl.window_rule({ match = { float = 1 }, opacity = "0.88 0.90" })

-- HyprGlass per-window tuning
if hl.plugin and hl.plugin.hyprglass then
    hl.window_rule({ match = { class = "^(mpv)$" }, tag = "+hyprglass_disabled" })
    hl.window_rule({ match = { fullscreen = 1 }, tag = "+hyprglass_disabled" })
    hl.window_rule({ match = { class = "^(kitty|foot|com.mitchellh.ghostty|Alacritty)$" }, tag = "+hyprglass_preset_frost" })
end
