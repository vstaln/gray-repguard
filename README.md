# gray-repguard

A repetition guard for tool calls. `tool/before` hashes each call's
`(name, canonicalized-args)` — object keys sorted recursively, so key order
can't dodge the match — and counts how many times the same call has run
*in a row*. When the run reaches `REPGUARD_MAX_REPEATS`, `tool/after`
appends a *"you are looping, change approach"* nudge to that call's
result. A different call — or a gap longer than `REPGUARD_WINDOW_SECS` —
resets the run to 1, so identical calls interleaved with other work count
as iteration, not a loop.

Port of the idea in hermes' `agent/repetition_guard.py`, applied to tool
calls rather than response fragments. Complements gray-core's
`SIGNATURE_ABORT_REPEATS` abort: repguard is the softer UX-level nudge.

## Config

| env                    | default | meaning                                  |
|------------------------|---------|------------------------------------------|
| `REPGUARD_MAX_REPEATS` | `3`     | consecutive identical calls that nudge   |
| `REPGUARD_WINDOW_SECS` | `600`   | sliding window / run-staleness cutoff    |

## Commands

- `/repguard` — config, this session's tracked calls and current run

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
