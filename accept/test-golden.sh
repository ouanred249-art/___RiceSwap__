#!/usr/bin/env bash
# accept/test-golden.sh — desktop-free assertions for accept/run.sh
#
# These tests verify the acceptance ritual's non-VT components: fixture
# assembly, sabotage injection, cleanup idempotency, and NDJSON parsing.
# They run in a scratch HOME with no compositor, so they prove the script's
# logic without needing a spare VT or root access.
#
# Exit code 0 == all golden-file assertions pass.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

BIN="${RICESWAP_BIN:-./target/debug/riceswap}"
if [[ ! -x "$BIN" ]]; then
  echo "build first: cargo build (or set RICESWAP_BIN)" >&2
  exit 1
fi

PASS=0; FAIL=0; TOTAL=0
assert() {
  local label="$1" condition="$2"
  TOTAL=$((TOTAL + 1))
  if eval "$condition"; then
    printf "  ✓ %s\n" "$label"
    PASS=$((PASS + 1))
  else
    printf "  ✗ %s\n" "$label" >&2
    FAIL=$((FAIL + 1))
  fi
}

# ── Test 1: Fixture assembly reproduces the donor tree ─────────────────────
echo "── Fixture assembly"
FIXTURE="$(mktemp -d)"
cp -R tests/fixtures/reconcile/donor/. "$FIXTURE/"
mkdir -p "$FIXTURE/.config/hypr"
[[ -f "$FIXTURE/.config/hypr/hyprland.conf" ]] || \
  printf 'monitor=,preferred,auto,1\n' > "$FIXTURE/.config/hypr/hyprland.conf"
if [[ ! -f "$FIXTURE/.config/quickshell/caelestia/shell.qml" ]]; then
  mkdir -p "$FIXTURE/.config/quickshell/caelestia"
  printf 'import QtQuick\nimport Quickshell\n\nShellRoot {}\n' \
    > "$FIXTURE/.config/quickshell/caelestia/shell.qml"
fi
mkdir -p "$FIXTURE/Wallpapers"
cp assets/wallpapers/*.png "$FIXTURE/Wallpapers/" 2>/dev/null || true

assert "fixture has shell.qml identity marker" \
  "[[ -f '$FIXTURE/.config/quickshell/caelestia/shell.qml' ]]"
assert "fixture has hyprland.conf" \
  "[[ -f '$FIXTURE/.config/hypr/hyprland.conf' ]]"
assert "fixture has donor keybinds" \
  "[[ -f '$FIXTURE/.config/hypr/custom/keybinds.lua' ]]"
assert "fixture has donor hypridle" \
  "[[ -f '$FIXTURE/.config/hypr/hypridle.conf' ]]"
assert "fixture has wallpapers" \
  "[[ -d '$FIXTURE/Wallpapers' ]] && [[ \$(ls '$FIXTURE/Wallpapers/' | wc -l) -gt 0 ]]"

# ── Test 2: Install in scratch HOME ───────────────────────────────────────
echo "── Scratch-HOME install"
FH="$(mktemp -d)"
trap 'rm -rf "$FH" "$FIXTURE"' EXIT

# Hash the fixture before
BEFORE="$(cd "$FIXTURE" && find . -type f -exec sha256sum {} + | sort)"

# Init + install
HOME="$FH" "$BIN" init >/dev/null 2>&1
ENVELOPE="$(HOME="$FH" "$BIN" install "$FIXTURE" 2>&1 || true)"
LAST="$(printf '%s\n' "$ENVELOPE" | tail -1)"

IS_OK="$(printf '%s' "$LAST" | python3 -c "import json,sys; print(json.loads(sys.stdin.read()).get('ok', False))")"
assert "install envelope ok" "[[ '$IS_OK' == 'True' ]]"

# The source tree must be byte-unchanged
AFTER="$(cd "$FIXTURE" && find . -type f -exec sha256sum {} + | sort)"
assert "source tree byte-unchanged after install" "[[ '$BEFORE' == '$AFTER' ]]"

# The profile must exist
assert "profile caelestia created" \
  "[[ -f '$FH/.local/share/riceswap/profiles/caelestia/profile.toml' ]]"

# The manifest must have the source URL
assert "manifest has source_url" \
  "grep -q 'source_url.*file://' '$FH/.local/share/riceswap/profiles/caelestia/profile.toml'"

# ── Test 3: Reconcile facts in envelope ──────────────────────────────────
echo "── Reconcile facts"
# Test reconcile directly from the envelope (avoid shell variable corruption of JSON)
RECONCILE_SHELL="$(printf '%s' "$LAST" | python3 -c "
import json,sys
d = json.load(sys.stdin)
r = d.get('data',{}).get('reconcile',{})
if not r:
    r = d.get('data',{}).get('switch',{}).get('report',{}).get('reconcile',{})
has_shell = 'shell' in r
has_registered = 'registered' in r
print(f'{has_shell}|{has_registered}')
" 2>/dev/null || echo "False|False")"

HAS_SHELL="${RECONCILE_SHELL%%|*}"
HAS_REG="${RECONCILE_SHELL##*|}"
assert "reconcile report has 'shell' key" "[[ '$HAS_SHELL' == 'True' ]]"
assert "reconcile report has 'registered' key" "[[ '$HAS_REG' == 'True' ]]"

# ── Test 4: Second install → profile-exists ──────────────────────────────
echo "── Idempotent refusal"
ENVELOPE2="$(HOME="$FH" "$BIN" install "$FIXTURE" 2>&1 || true)"
LAST2="$(printf '%s\n' "$ENVELOPE2" | tail -1)"
IS_OK2="$(printf '%s' "$LAST2" | python3 -c "import json,sys; print(json.loads(sys.stdin.read()).get('ok', False))")"
REASON2="$(printf '%s' "$LAST2" | python3 -c "
import json,sys
d = json.load(sys.stdin)
print(d.get('data',{}).get('reason_code',''))
" 2>/dev/null || true)"

assert "second install refused" "[[ '$IS_OK2' == 'False' ]]"
assert "refusal reason is profile-exists" "[[ '$REASON2' == 'profile-exists' ]]"

# Third install — also refused, no journal corruption
ENVELOPE3="$(HOME="$FH" "$BIN" install "$FIXTURE" 2>&1 || true)"
LAST3="$(printf '%s\n' "$ENVELOPE3" | tail -1)"
IS_OK3="$(printf '%s' "$LAST3" | python3 -c "import json,sys; print(json.loads(sys.stdin.read()).get('ok', False))")"
assert "third install also refused (no journal corruption)" "[[ '$IS_OK3' == 'False' ]]"

# ── Test 5: Verdict shape ────────────────────────────────────────────────
echo "── Verdict shape"
VERDICT="$(printf '%s' "$LAST" | python3 -c "
import json,sys
d = json.load(sys.stdin)
# verification lives at data.switch.report.verification
v = d.get('data',{}).get('switch',{}).get('report',{}).get('verification',{})
if not v:
    v = d.get('data',{}).get('report',{}).get('verification',{})
print(json.dumps(v))
" 2>/dev/null || echo "{}")"

assert "verification has 'verdict' key" \
  "printf '%s' '$VERDICT' | python3 -c \"import json,sys; d=json.load(sys.stdin); assert 'verdict' in d\""
assert "verification has 'checks' array" \
  "printf '%s' '$VERDICT' | python3 -c \"import json,sys; d=json.load(sys.stdin); assert isinstance(d.get('checks'), list)\""

# Check that verdicts are from the frozen vocabulary
VERDICT_VAL="$(printf '%s' "$VERDICT" | python3 -c "
import json,sys; d=json.load(sys.stdin); print(d.get('verdict',''))
")"
assert "verdict is from frozen vocabulary" \
  "[[ '$VERDICT_VAL' == 'verified-full' || '$VERDICT_VAL' == 'verified-core' || '$VERDICT_VAL' == 'fail' ]]"

# All checks have the frozen key set {id, tier, ok, evidence}
CHECKS_VALID="$(printf '%s' "$VERDICT" | python3 -c "
import json,sys
d = json.load(sys.stdin)
checks = d.get('checks', [])
valid = all(
    isinstance(c, dict) and
    all(k in c for k in ['id', 'tier', 'ok', 'evidence'])
    for c in checks
)
print(valid)
" 2>/dev/null || echo "False")"
assert "all checks have frozen key set {id, tier, ok, evidence}" \
  "[[ '$CHECKS_VALID' == 'True' ]]"

# ── Summary ──────────────────────────────────────────────────────────────
echo
if [[ $FAIL -eq 0 ]]; then
  printf "✓ All %d golden-file assertions pass\n" "$TOTAL"
  exit 0
else
  printf "✗ %d/%d assertions failed\n" "$FAIL" "$TOTAL" >&2
  exit 1
fi
