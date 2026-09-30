# pi CLI invocation contract — findings for RiceSwap's AI research tier

> Wayfinder ticket: [Research: pi CLI invocation contract for the AI research tier](https://github.com/ouanred249-art/dots-swetcher/issues/23) (map #22)
> All facts verified against pi 0.87.1 (`~/.npm-global/bin/pi`, package `@earendil-works/pi-coding-agent`):
> `pi --help`, `pi auth --help`, the package's own `docs/json.md`, its `dist/` sources
> (`main.js`, `modes/print-mode.js`, `modes/json-event.js`, `core/session-manager.d.ts`), the local
> `~/.pi/agent/settings.json`, and three live smoke runs on this machine.

## TL;DR contract

```
pi --offline -p --mode json --no-session --no-context-files --no-approve \
   --tools read,grep,find,ls --model anthropic/claude-sonnet-4-5 --thinking off \
   -- "<research brief ending with the required JSON schema>"
```

Run with `current_dir` set to the clone cache directory, stdin closed, stdout drained
incrementally, and a hard SIGTERM timeout. Parse the JSONL envelope, take the **last
assistant `message_end`**, extract its text block, parse the embedded JSON object.
Cache the result keyed by `(repo, commit)` so each shell is researched once.

---

## 1. Probe (no tokens consumed)

Two probes, mirroring the `src/tools.rs` `Tool`/`ToolStatus` pattern:

1. **Presence + version** — add `Tool::Pi { name: "pi", version_args: ["--version"] }`.
   `pi --version` prints `0.87.1`, exit 0, pure local, <1 s.
2. **Auth readiness** — `pi auth check --provider <P> --json` (help: "Print credentials or
   check provider readiness"; requires at least one of `--provider`/`--model`):
   - ready → stdout `{"status":"ready","provider":"opencode","authType":"api_key"}`, exit 0
   - not ready → `{"status":"not_ready","provider":"openai","reason":"credentials_not_configured"}`, exit 1
   - No token spend, no completion request. Verified: `anthropic`, `kios`, `b-ai`, `opencode`
     report ready; `openai`, `google`, `github-copilot` report `not_ready` on this machine.

**Trap: "ready" ≠ usable.** The default model here is `opencode/nemotron-3-ultra-free`
(`~/.pi/agent/settings.json`: `defaultProvider: "opencode"`, `defaultModel:
"nemotron-3-ultra-free"`, `defaultThinkingLevel: "high"`). Its key passes `auth check`
but every request is refused at request time: `403 FreeTierError — OpenCode's free tier
can only be used from within OpenCode` (observed in smoke run 1). Provider validity is
only provable by one real (tiny) completion — see the retry chain in §5.

`pi --list-models [search]` (exit 0, offline-capable) is a cheap way to verify a
provider/model pair exists before invoking.

## 2. Invocation semantics

- `--mode json` **implies print**: `resolveAppMode()` in `dist/main.js` routes
  `mode === "json"` straight to `runPrintMode`; `-p` is redundant but keep it for
  readability. Non-interactive also auto-triggers when stdin/stdout are not TTYs.
- `--no-session` — ephemeral run: nothing written under `~/.pi/sessions`. The session
  **header record is still emitted** as line 1 (observed), so the parser is unchanged.
- Model selection: `--model provider/id` (with optional `:thinking` suffix) or
  `--provider`/`--model` separately; falls back to `settings.json` defaults.
  RiceSwap should **always pass `--model` explicitly** — user defaults here are a broken
  free tier.
- `--offline` (or `PI_OFFLINE=1`) skips startup network (catalog refresh); model catalog
  is local (`~/.pi/agent/models.json`, `models-store.json`). Smoke runs confirmed
  completions work with `--offline`.
- `--thinking off` — cuts latency on non-reasoning models; default on this box is `high`.

### Sandbox / read-only enforcement (from `pi --help`)

Built-in tools are `read, bash, edit, write` on by default; `grep, find, ls` exist but are
off by default. The help's own example is exactly our use case:

```
# Read-only mode (no file modifications possible)
pi --tools read,grep,find,ls -p "Review the code in src/"
```

So the authority set is:

| flag | effect | needed because |
|---|---|---|
| `--tools read,grep,find,ls` | allowlist; `bash`/`edit`/`write` become unavailable — model *cannot* mutate anything or exec | research only reads the clone |
| `--no-context-files` (`-nc`) | don't load `AGENTS.md`/`CLAUDE.md` found in cwd | a cloned rice's own agent files must not hijack the brief (prompt injection + determinism) |
| `--no-approve` (`-na`) | ignore project-local (untrusted) config without prompting | clone dirs appear in `trust.json`-like trust checks |
| `--no-session` | no transcript persisted | hygiene |
| `--offline` | no startup network | determinism |

Plus process-level: `current_dir = <clone cache>`, env unchanged otherwise. There is no
chroot flag in pi; the tool allowlist is the enforcement, cwd is convention.

## 3. Output envelope (`--mode json`) — pinned by docs + smoke test

pi ships a canonical spec at `<package>/docs/json.md`; a live run confirmed it byte-for-shape.
Strict **JSONL**: one JSON object per LF-terminated line; stdout is reserved for JSONL,
all diagnostics go to stderr. Do **not** use line-splitting that also honors U+2028/2029
(docs call out Node `readline`); split on `\n` only. **Drain stdout continuously** — a
stalled reader wedges pi when the pipe fills.

Record sequence for a one-shot run:

```json
{"type":"session","version":3,"id":"<uuid>","timestamp":"…","cwd":"/path"}   // header (emitted even with --no-session)
{"type":"agent_start"}
{"type":"turn_start"}
{"type":"message_start","message":{...}} / {"type":"message_end","message":{...}}   // system, user, then assistant
{"type":"message_update","usage":{...},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"…"}}
{"type":"turn_end","message":{...},"toolResults":[...]}
{"type":"agent_end","messages":[...],"willRetry":false}
{"type":"agent_settled"}
```

- `message_update` is delta-only streaming (`text_delta`, `toolcall_*`, …) with a
  cumulative `usage` — use for live progress, ignore for the final answer.
- **`message_end` with `message.role === "assistant"` is authoritative** ("This is the
  authoritative final message"). `message.content` is an array of blocks
  (`text`/`thinking`/`toolCall`); concatenate the `text` blocks.
- `message.usage`: `{input, output, cacheRead, cacheWrite, totalTokens, cost:{…,total}, reasoning?}` — log it.
- `message.stopReason`: `"stop"` on success; `"error"`/`"aborted"` carry
  `errorMessage` (observed: `"stopReason":"error","errorMessage":"403: {\"type\":\"FreeTierError\",…}"`).
- `agent_settled` is the true end of automatic work (after any `auto_retry_*` /
  `compaction_*` events, which can appear between `agent_end` and settling).
- Tool use appears as `tool_execution_start/update/end` records (correlated by
  `toolCallId`) — useful to assert pi actually read the clone.

**Critical exit-code trap:** a provider-level request error (403 etc.) still exits **0**
with a well-formed JSONL stream — `runPrintMode` only forces exit 1 in *text* mode.
In json mode, success is `stopReason === "stop"` on the final assistant message, not
the process exit code. A *startup* error (bad model name) is the opposite: exit **1**,
empty stdout, `Error: Model "…" not found…` on stderr (observed).

### Rust extraction recipe

1. Collect stdout, split on `\n`, `serde_json::from_str` each non-empty line
   (skip unparseable lines with a warning — forward-compatible).
2. Keep the last `message_end` whose `message.role == "assistant"`; require
   `stopReason == "stop"`.
3. Concatenate its `content[].text` blocks → strip markdown fences if the model added
   any → scan for the first balanced `{ … }` → deserialize into `ResearchFindings`.
4. Treat `turn_end.toolResults` / `tool_execution_*` as diagnostics evidence.

## 4. Prompt brief + required findings schema

User prompt (single argument after `--`), template owned by RiceSwap:

```text
You are researching an unknown desktop rice to make RiceSwap able to switch it.
The repo is already cloned at <cwd>. Only read files inside it (docs, config, sources).

Failing symptoms observed on the live system: <symptoms list>.

Answer these specific questions from the repo's docs/config:
1. Which appid does it register Hyprland GlobalShortcuts / layer-shell namespaces under?
2. Which env vars pick config/wallpaper paths, and what values does RiceSwap need to set?
3. Exact start and stop commands for the shell (incl. the binary or entry script).
4. Which keybind namespace/chord scheme does it use ($mod, dispatcher names)?
5. Where are wallpapers stored / how are they picked?

Return ONLY one JSON object, no prose, no code fences, matching exactly:
{
  "schema": "riceswap.research.v1",
  "shell": "<hyprland|quickshell|waybar|other>",
  "appid": "<GlobalShortcuts appid or null>",
  "env": { "<VAR>": "<value or template>" },
  "dispatcher_namespaces": { "<name>": "<ns>" },
  "start_cmd": ["<argv…>"],
  "stop_cmd": ["<argv…>"],
  "keybind_namespace": "<e.g. $mod or SUPER>",
  "wallpaper_dir": "<path or null>",
  "confidence": 0.0,
  "evidence": [{ "file": "<repo-relative path>", "quote": "<short excerpt>" }],
  "unknowns": ["<question you could not answer>"]
}
Use null / [] / "" rather than inventing values; put every gap in "unknowns".
```

Rust parses this into `ResearchFindings` (fields exactly as above; `confidence: f32`).
The schema version string lets the parser reject stale shapes. `--append-system-prompt
<file>` is available to move the output discipline into the system prompt if a model
keeps chattering, but keeping the schema in the user message is version-drift-robust.

## 5. Failure matrix

| # | Symptom | Detection | Handling |
|---|---|---|---|
| F1 | `pi` not on PATH | probe: spawn error, `available:false` | **Degrade to engine+adapt only**, emit diagnostics "AI research tier disabled: pi not installed" (Q3 floor) |
| F2 | auth `not_ready` (exit 1, `status:"not_ready"`) | auth-check parse | same degradation, diagnostic names the provider + `pi auth` hint |
| F3 | bad/unresolvable model at startup | exit 1, empty stdout, stderr `Error: Model … not found` | config error → diagnostic, no retry |
| F4 | provider refuses at request time (free-tier 403) with exit 0 | final assistant `stopReason:"error"` + `errorMessage` | retry chain next; if exhausted → F1-style degradation carrying `errorMessage` |
| F5 | hang — no JSONL line within T1 (~45 s: healthy runs emit header+`agent_start` in seconds; observed b-ai/kios free runs produced **zero bytes** for 90–120 s) | no first stdout line by T1 → `kill -TERM` | SIGTERM makes pi clean up children and die with **143** (SIGHUP → 129, from `print-mode.js`) |
| F6 | stall mid-stream | no new record for T2 or no `agent_settled` before total budget T3 (~300 s) | SIGTERM, partial JSONL to diagnostics log |
| F7 | malformed/unparseable final JSON | step 3 of the recipe fails | one cheap repair attempt (`pi -p --no-tools` "extract the JSON from this text: <transcript>") else fail to diagnostics; never write a corrupt cache entry |
| F8 | unauthenticated env mid-run (key revoked) | like F4 | same |

Retry chain (cheap, bounded): configured `--model` → `anthropic/claude-sonnet-4-5` →
give up to floor. One attempt per model, one total run budget.

## 6. Cost, latency, caching

- Startup: `--version` <1 s; auth-check ~1–2 s; full json run overhead observed at
  **11.8 s** for a one-token reply (this machine). Budget 30–120 s wall for a real
  tool-using research pass; T3=300 s cap.
- Free tiers on this box are unreliable (two observed hangs, one 403); `anthropic` is the
  dependable default. Many catalog models cost 0; every record carries `usage`/`cost`
  anyway — log the final `message_end.usage.cost.total` per research run.
- **Cache once per shell**: key `(repo_url, commit_sha)` → store `ResearchFindings` JSON
  (plus `model`, `usage`, timestamp) under the clone-cache dir. Hit on identical commit →
  skip pi entirely; commit change invalidates naturally. Only cache parses that succeed
  (F7 never writes). This feeds the write-back recipe flow: engine+adapt consume the
  cached findings, pi is never in the interactive switch path.

## 7. Existing-code alignment

- `Tool::Pi` fits `src/tools.rs` as-is: `version_args() = ["--version"]`, and auth-check
  results map onto `ToolStatus { available, exit_code, version: status, error }`.
- A new `src/research.rs` module would own the invocation, JSONL scan, and
  `ResearchFindings`, using the same `first_line` stderr-trim discipline for diagnostics.

## Smoke-test log (this machine)

1. `pi --offline -p --mode json --no-session --no-context-files --no-tools --thinking off "…{"ok":true}"` — default (opencode free) model: 11.8 s, exit 0, full JSONL captured, final assistant `stopReason:"error"`, 403 FreeTierError. Header emitted despite `--no-session`.
2. same with `--model b-ai/deepseek-v4-flash` — 120 s, **zero output**, killed via timeout(124). Endpoint reachable by curl (HTTP 200), so this is provider latency, not config.
3. same with `--model kios/grok-4.6-free` — 90 s, **zero output**, killed (124).
4. `--model nosuchprovider/nosuchmodel` — exit 1, stdout empty, stderr `Error: Model "nosuchprovider/nosuchmodel" not found. Use --list-models to see available models.` in <1 s.
