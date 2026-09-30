# Install a foreign rice

**What it is:** the destination feature — one command takes a stranger's dotfiles (git URL or local dir) through acquire → identify → claim → materialize → reconcile → switch → **verify** (with rollback on failure).

## Sub-features
- `install <git-url>` / `install <local-dir>`, `--shell <name>` when the tree holds several quickshell shells.
- Identity refusals: `not-a-quickshell-rice`, `ambiguous-identity` (without `--shell`, names candidates), `profile-exists` (after a COMPLETED install), source neither dir nor URL; `git-missing` / `clone-failed` on the URL path.
- Resume: an install that died mid-way is re-made by plain re-invocation (journal claim, written only after the exists-check — a refusal leaves `state.json` untouched). Note the asymmetry: a re-run over a FAILED-VERIFICATION install does not refuse — it `resumed:true` re-drives the switch; refusal-by-`profile-exists` proves only against a completed profile.
- Cache: full clone keyed `sources/<slug>/<sha>`; second run reuses; no re-clone. Local paths read in place, `file://` source_url, sha-if-git-else-null.
- The donor's dead dispatcher names are reconciled before the first switch reads them; reconcile facts ride the envelope.
- Research tier: unknown shell → one `pi` consultation; skips on ANY of four arms — built-in recipe present (known shell), profile `adapt.toml`, user-tier `recipes/<shell>.toml` file present (delete it to refresh), OR the static derivation was already confident (`Skip::Confident`). Write-back only ever creates `recipes/<shell>.toml`, never clobbers, never touches `adapt.toml`. Research can NEVER fail an install: its failure is a warning + `data.research.failure`, and the install continues on the engine's proposals.
- Verification (#39/#40): the embedded switch ends with the same Tier C(+F) pass as plain switch; `fail` ⇒ rollback (flip `current` back first; a FIRST install has no previous — `current` stays on the activated profile with that said in warnings).

## How to get to it (user POV)
Terminal (future: panel): "install this rice" → `riceswap install https://.../dots`.

## Driving it with the CLI (scratch HOME)
Stage a donor tree OUTSIDE $HOME, scrub the seat env, and stub `pi` so the research tier fails-fast into its documented skip instead of making a REAL, NETWORKED, AUTHENTICATED LLM call with the user's own key (live 2026-09-29: an unstubbed drive really called pi; a 401 is harmless, a 200 spends real money on a sandbox test). A stub that exits 127 on a PATH PREFIX is enough — research treats an unusable pi as its own evidence:
```bash
FH=$(mktemp -d); TREE=$(mktemp -d); STUB=$(mktemp -d)
printf '#!/bin/sh\necho "stub: pi absent" >&2\nexit 127\n' > $STUB/pi; chmod +x $STUB/pi
mkdir -p $TREE/.config/quickshell/demo $TREE/.config/hypr
printf 'import QtQuick\nShellRoot {}\n' > $TREE/.config/quickshell/demo/shell.qml
printf 'monitor=,preferred,auto,1\n' > $TREE/.config/hypr/hyprland.conf
R="./target/debug/riceswap"; S="env -u HYPRLAND_INSTANCE_SIGNATURE -u WAYLAND_DISPLAY -u DBUS_SESSION_BUS_ADDRESS"
( cd $TREE && find . -type f -exec sha256sum {} + | sort ) > /tmp/tree-before.sha
HOME=$FH PATH=$STUB:$PATH $S $R init
HOME=$FH PATH=$STUB:$PATH $S $R install "$TREE"   # pass 1
HOME=$FH PATH=$STUB:$PATH $S $R install "$TREE"   # pass 2: resumed or profile-exists
( cd $TREE && find . -type f -exec sha256sum {} + | sort ) > /tmp/tree-after.sha
diff /tmp/tree-before.sha /tmp/tree-after.sha && echo "source untouched: PROVEN"
```

## Observable end state that proves it
- envelope `data.reconcile` (full Report: `registered`, `dead_names`, `proposals`, `resolutions`, `files_written`, `backups`, `layers`), `data.research` (`asked`, `budgets_seconds`, `failure` or `recipe{written,path}`), and — the part older maps omit — the verification facts nested under **`data.switch`**: `{completed_steps, report.verification{verdict,checks[]}, reason_code, phase:"verify", next, facts}` on a failed install (top-level `data.phase` reads `switch`; the frozen `reason_code` is NOT at `data.reason_code` here). `demo` registers nothing, so `quickshell:*` donor dispatches must appear as proposals/dead names — the conservative engine asserts, it does not silently rewrite.
- The source tree byte-unchanged (hash before/after — "read where it stands").
- A completed install refused on the next pass with `reason_code:"profile-exists"`, and the refusal leaves no claim a third run would resume (drive three times; the frozen chain is exactly what `accept/test-golden.sh` pins).
- Without `git` (bare PATH): `reason_code:"git-missing"`, zero dirs created under `sources/`.

## Gotchas
- NEVER point `install` at the real HOME store; and scrub the seat env (SKILL.md): an unscrubbed drive with a live compositor can make a fake shell's `qs -c <name>` CONNECT TO THE REAL SESSION as a client (observed 2026-09-29) — and whether such a stub survives the 700 ms shell-alive grace then varies run to run, so `ok:false shell-not-alive + rollback` vs `ok:true verified-core` on the same fake fixture are BOTH live possibilities on a desktop host. Assert the envelope SHAPE, not a fixed verdict, unless the seat is scrubbed.
- The `[[shell]]` start/stop command derivation from recipes is still open work (recorded on #37) — today's install starts shells by convention, so a fixture whose shell binary is missing will exercise the failure/rollback path, which is valid evidence for that branch.
