#!/bin/bash
# Switch Hyprland blur variant live (requires hyprland-git post Aug 22)
# Usage: blur-variant.sh <kawase|frost|ripple|drops|water|fluid_jar|prism|heat_shimmer|acrylic|aurora|haze>
# Example: blur-variant.sh acrylic
VARIANT=${1:-acrylic}
case "$VARIANT" in
  kawase|frost|ripple|drops|water|fluid_jar|prism|heat_shimmer|acrylic|aurora|haze) ;;
  *) echo "Unknown variant: $VARIANT"; echo "Valid: kawase frost ripple drops water fluid_jar prism heat_shimmer acrylic aurora haze"; exit 1 ;;
esac
# Lua runtime config (hyprland-git uses Lua, not `keyword`)
hyprctl eval "hl.config({ decoration = { blur = { variant = \"$VARIANT\" } } })" 2>&1
echo "-> blur variant set to $VARIANT"
echo "Tip: animated ones (drops/water/fluid_jar/ripple) hammer old iGPUs. Use kawase/frost/acrylic/haze for daily."
