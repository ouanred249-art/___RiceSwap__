# Acceptance test — the destination's definition of done

`run.sh` creates a throwaway system user, builds a donor-graft fixture from the
repo's reconcile test fixtures, launches Hyprland on a spare VT for that user,
and runs one `riceswap install` end-to-end. It then sabotages the installed
profile twice (dead QML → `invariant-dead-names`, missing shell binary →
`shell-not-alive`) and verifies each failure rolls back and preserves the old
desktop. A final re-install proves `profile-exists` idempotency.

Exit code 0 = **RiceSwap is a true rice installer.**

## Usage

```bash
# Attended (will prompt for pkexec/sudo password)
accept/run.sh

# CI-ish (installs a scoped passwordless sudoers drop-in for the throwaway user)
accept/run.sh --ci-ish

# Clean up a stranded test user from an interrupted run
accept/run.sh --cleanup-only riceswap-test-<timestamp>
```

## Prerequisites

- Root or `sudo` access (user creation/deletion)
- A spare VT (the script finds one automatically, tty2–tty12)
- Hyprland installed
- `qs` (quickshell) installed — for the shell-alive verification

## Golden-file tests (no VT, no root)

```bash
accept/test-golden.sh
```

Runs the desktop-free half: fixture assembly, scratch-HOME install, reconcile
facts, idempotent refusal, verdict shape assertions. These can run in CI.

## Evidence

Each run archives its NDJSON envelopes, fixture hashes, and the test user's
final `state.json` + profile tree under `accept/artifacts/<timestamp>/`.
The directory is gitignored.
