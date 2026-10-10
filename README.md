# gray-repguard

A repetition guard for tool calls. `tool/before` hashes each call's
`(name, canonicalized-args)` — object keys sorted recursively, so key order
can't dodge the match — and keeps a sliding window of hashes per session.
When the same call appears `REPGUARD_MAX_REPEATS` times inside
`REPGUARD_WINDOW_SECS`, `tool/after` appends a *"you are looping, change
approach"* nudge to that call's result.

Port of the idea in hermes' `agent/repetition_guard.py`, applied to tool
calls rather than response fragments.

## Config

| env                    | default | meaning                                  |
|------------------------|---------|------------------------------------------|
| `REPGUARD_MAX_REPEATS` | `3`     | identical calls in window that nudge     |
| `REPGUARD_WINDOW_SECS` | `600`   | sliding window size                      |

## Commands

- `/repguard` — config and this session's hottest repeat count

## Wire

`plugin/manifest`, `tool/before`, `tool/after`, `command/run`,
`plugin/shutdown`. Protocol 2.0. State is in-memory per sidecar (per
session); no files, no capabilities.

## Install

```sh
gray plugin install repguard
```

## Limits

Always allows the call — it annotates, never blocks. Sessions are keyed by
`session.id` (falls back to `cwd`); the window resets when the sidecar
restarts.
