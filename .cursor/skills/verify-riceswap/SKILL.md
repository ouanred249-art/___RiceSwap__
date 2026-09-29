---
name: verify-riceswap
description: Drive and prove the RiceSwap CLI (rust backend, NDJSON envelope) the way a user/panel really uses it — launch-isolated, evidence-captured. Use when verifying a feature of riceswap, after touching src/ or recipes/, or when asked to prove install/switch/snapshot behavior. NOT for the Quickshell panel UI itself.
---

# Verify RiceSwap

RiceSwap is a Rust CLI backend (`riceswap <op>`, NDJSON on stdout — one progress line per step, then one final envelope object `{"ok":…,"warnings":[…],"data":…}`) plus a Quickshell GUI panel that shells out to it. The surface this skill drives is the **CLI as the panel calls it**. The panel itself (a layer-shell window on a live Hyprland seat) is out of scope here — see `features/` notes for what that costs.

## Isolation model (read first, it is the whole safety story)

Everything the tool touches is rooted at `$HOME` (`~/.local/share/riceswap/{profiles,state.json,sources,wallpapers}`, links into `~/.config/hypr` and `~/.config/quickshell`). **Every verification run MUST use a scratch HOME:**

```bash
FH=$(mktemp -d)            # scratch $HOME — the instance is fully isolated by this
```

- Two instances side by side: yes — one per scratch HOME; they never collide.
- The **real** `$HOME` is off-limits: never run `switch`, `install`, or `init` with the user's actual HOME. `snapshot` on a real HOME is READ-mostly but writes a profile; treat it the same — scratch HOME only.
- A scratch HOME has no Hyprland session, so `switch` will fail at the `hyprctl reload` / shell-start steps *on purpose* — the envelope still reports the earlier phases. That is valid evidence; it is not a broken app.
- `install` against a git URL in tests must use the sandbox `git` stub or a local-path fixture; never let a verification run hit the network.

## Launch

No server, no port. "Launch" = build once, then each drive is one short-lived subprocess:

```bash
cargo build 2>&1 | tail -1          # "Finished … " means the binary is current
BIN=./target/debug/riceswap
```

Ready check: `$BIN list` prints an `{"ok":true,…}` envelope line. Teardown: nothing to kill if you only ran CLI ops — see Cleanup for the case where an op started daemons.

## Doctor

One read-only run that answers "is this instance worth driving?":

```bash
FH=$(mktemp -d); HOME=$FH cargo run --quiet -- detect
```

Healthy = final envelope `"ok":true` and a `data.tools` map where `hyprctl`/`grim`/`pkexec` statuses reflect the *host* (in CI they fail; that is expected and is itself the degrade path under test). Run this first whenever behavior looks off.

## Drive

`HOME=$FH $BIN <op> [args…]`, capture stdout. Parse the LAST line as the envelope (`ok` is the truth — exit code 0 does not mean success, and a refused op also exits 0 with `ok:false` only where the contract says so; assert on the JSON). Ops: `detect | list | init | snapshot <name> [--force] [--only <paths>] | plan <target> | switch <target> | install <source> [--shell <name>] | info <name> | delete <name> [--force] | diff <a> <b> | wallpaper-import <file>`.

Stable handles, in this order: envelope `data` keys (documented in each command's module doc), then `warnings` strings (they name the tool/profile/path), then on-disk state under `$FH/.local/share/riceswap/`.

## Evidence & proof standards

Write every run to `$FH/proof/` (survives; name it in your report) plus a copy under `.scratch/verify-riceswap/<UTC-timestamp>/<feature>/` in the repo when the finding should outlive the temp dir.

- Exercise the real user path (the CLI as the panel invokes it). No internal setters, no test-only endpoints — the backend has none.
- Capture the ACTION and the RESULTING STATE: envelope JSON + the side-effect files (`profiles/<name>/profile.toml`, `state.json`, `backups/` hashes, symlinked `~/.config` layout) — not just the final screen.
- Verify side effects, not exit codes: `ok:false` with a `reason_code` IS a correct outcome for refusal tests.
- `install`/`switch` write into the scratch profile store; after the run, `find $FH/.local/share/riceswap -newer <marker>` proves what actually landed.
- Dry-run analogue: `riceswap plan <target>` performs the diff without touching anything — prove that by observing the store is untouched (byte-identical `state.json`, no new files), don't trust the name.
- Mocks only where production already isolates: package transactions go through `pkexec`/`paccache` — in a scratch HOME with no agent, they refuse; that refusal is real behavior, not a mock.

## Cleanup

```bash
rm -rf "$FH"                        # the whole instance including proof — copy proof out first if it must survive
```

Never kill by process name. If an op started daemons under the scratch HOME (services whose `start` lines exec real binaries), kill only the pids the run recorded (`$FH/.local/share/riceswap/state.json` `operation` field / your own spawn log). Cleanup removes the instance, never the evidence you copied to `.scratch/verify-riceswap/`.

## Helpers

`scripts/verify.sh <feature> [args…]` — builds, opens a scratch HOME, runs the mapped feature's drive sequence from `features/<feature>.md`, prints the envelope, archives evidence to `.scratch/verify-riceswap/<ts>/<feature>/`, cleans the instance. Invocation shown in the file it drives; start there.

## features/

The maintained map: one file per user-facing feature, each with the user-POV path, the drive sequence, and the observable end state that proves it. A proof that drives one convenient entry point is incomplete when the map lists others.
