-- Glass-style blur tuning (saneAspect style, adapted to 0.56.2)
-- After hyprland-git install, uncomment the variant block for video effects
-- Variants: kawase, frost, ripple, drops, water, fluid_jar, prism,
-- heat_shimmer, acrylic (liquid glass), aurora, haze
hl.config({
    decoration = {
        rounding = 18,            -- corner radius (px)
        rounding_power = 2.5,     -- 2 = circle-ish, 4 = squircle
        blur = {
            enabled = true,
            passes = 1,           -- blur repetitions: keep 1 for minimal blur
            size = 3,             -- blur kernel size; 3=clear glass, 5=medium, 10=heavy
            brightness = 1.0,     -- neutral: shows true wallpaper hue (0.9 darkened/yellowed it)
            contrast = 1.0,      -- neutral: contrast boost caused grainy look
            noise = 0,         -- 0=clean glass, 0.05=video texture
            vibrancy = 0.35,       -- higher = blurred backdrop keeps wallpaper color
            vibrancy_darkness = 0.0,  -- how much vibrancy affects dark areas
            xray = true,          -- true = blur shows through transparent term
            new_optimizations = true,
            popups = true,        -- video has popups blurred (was false)
            special = false,      -- blur special workspaces (GPU heavy on 4K)
            ignore_opacity = true, -- let blur ignore window opacity (key for terminals!)
            -- === VIDEO VARIANTS (hyprland-git only, post Aug 22) ===
            -- Uncomment after hyprland-git install:
            -- variant = "acrylic", -- liquid-glass like video. Try: frost, haze, aurora
            -- acrylic = {
            --     refraction = 24,   -- 0-48 lens displacement
            --     bulb = 48,         -- 4-256 curved edge width
            --     clarity = 0.5,     -- 0-1 sharp backdrop (0.5 = vaxry demo)
            --     aberration = 0.025,-- 0-0.25 chromatic separation
            --     -- tint = "rgba(0addeeff)", -- optical absorption tint
            -- },
            -- glass = {
            --     refraction = 20,
            --     size = 40,
            --     roughness = 1.0,
            -- }, 
        },
    },
})

-- HyprGlass Liquid Glass (saneAspect video recipe: "I read hyprglass source so you don't have to")
-- Video rules applied: glass REPLACES Hyprland blur (no stacking), shadows stay on,
-- blur_strength never below 0.35 (half-res fast path), edge_thickness is the visibility knob.
 if hl.plugin.hyprglass then
    local hg = hl.plugin.hyprglass

    hg.config({
        default_theme = "dark",
        default_preset = "glass",
        tint_color = 0x00000020,

        brightness = 1.0,
        dark = { brightness = 0.95, adaptive_dim = 0.0 },
        light = { adaptive_boost = 0.5 },

        layers = { enabled = true },
    })

    -- Layer surfaces: every preset named here MUST exist below.
    -- (Missing presets + light theme on dark wallpapers = big milky sheets.)
    hg.layer("waybar", { preset = "clear", mask_threshold = 0.05 })
    hg.layer("swaync", { preset = "clear" })
    hg.layer("quickshell:bezel", { preset = "clear", mask_threshold = 0.3 })
    hg.layer("debug-panel", { exclude = true })

    -- Presets - video recipe (author runs theme=light, preset=glass, exaggerated for camera)
    -- THE video look: readable backdrop + strong prism rim + wavy liquid edge
    hg.preset("glass", {
        glass_opacity = 0.5,
        blur_strength = 0.35,
        refraction_strength = 1.0,
        chromatic_aberration = 0.6,
        fresnel_strength = 0.8,
        specular_strength = 0.9,
        edge_thickness = 0.10,
        lens_distortion = 0.6,
        tint_color = 0x00000020,
        dark = { brightness = 0.9, adaptive_dim = 0.4 },
        light = { brightness = 1.1, adaptive_boost = 0.3 },
    })

    hg.preset("clear", {
        glass_opacity = 0.35,
        blur_strength = 0.35,
        refraction_strength = 1.0,
        chromatic_aberration = 0.4,
        fresnel_strength = 0.45,
        specular_strength = 0.4,
        edge_thickness = 0.10,
        lens_distortion = 0.5,
        dark = { brightness = 0.92, adaptive_dim = 0.0 },
        light = { brightness = 1.05 },
    })

    -- Frost for terminals (video: blur_strength >= 0.35 stays on the fast path)
    hg.preset("frost", {
        glass_opacity = 0.45,
        blur_strength = 0.5,
        refraction_strength = 0.7,
        chromatic_aberration = 0.3,
        fresnel_strength = 0.5,
        specular_strength = 0.45,
        edge_thickness = 0.1,
        lens_distortion = 0.4,
        tint_color = 0x00000020,
        dark = { brightness = 0.9, adaptive_dim = 0.6 },
        light = { brightness = 1.05, adaptive_boost = 0.3 },
    })

    hg.preset("contrasted", {
        inherits = "high_contrast"
,
        contrast = 1.2,
        adaptive_dim = 1.5,
        dark = { tint_color = 0x02142aa9 },
    })
end

