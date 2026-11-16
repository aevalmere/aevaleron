# Lane brief: allow-similar

Branch `rust/allow-similar`, cut from `rust-rewrite` at `ec8f2aa4`.

## Goal

The owner asked for it on 2026-11-15: "do a allow similar ones button where it like allows doing the same thing i already confirmed was safe".

The card that asks before a step now has a third button, **Allow similar**, between Skip and Allow. It allows the step, and the same step runs without a card whenever it comes up again in the same task. The trail row of such a step says "Not asked again: you allowed this same step earlier in this task".

## What counts as the same step

`similar_key` in `src-tauri/src/agent/run.rs`. Two flagged steps are the same when all of these match:

- the tool (click, double click, key, type, open and so on),
- the app,
- the control it lands on, or the one with keyboard focus: its role and its name,
- what it types or opens, in full,
- the sentence on the card,
- the gate's reason for asking.

The reason is part of it on purpose: an "OK" button under a dialog that warns about deleting is flagged for another reason than the "OK" that was allowed, so it asks again.

## What is never offered

The button is left off, and nothing is remembered, for:

- everything automatic mode always asks about (`auto::always_ask`): money, a password or a sign-in, a change to this PC or an account, a command line, and anything the gate could not read,
- a drag,
- closing a window.

## Decisions the owner may want to reverse

- **This task only.** The memory is in the run's state and goes when the task ends. Nothing is stored. A list that survives across tasks is a bigger promise and was not built.
- **Exactly the same step, not a family of them.** "Delete" on thirty files with thirty names asks thirty times if the file's name is the control's name. Loosening it (same button name in the same app, whatever it lands on) is one line in `similar_key`, and makes one yes cover more than the person saw.
- **In automatic mode it skips the second check.** The person's own yes for this exact step outranks the reviewer model.
- **It is offered in "approve every action" mode too.** The person chose that mode, and the button is their own way out of it for one repeated step.
- **Send, delete and post are allowed as similar.** Only the list above is held back. Pressing the same Send button again in the same app for the same reason will not ask.

## Files

- `src-tauri/src/agent/run.rs`: `similar_key`, the run state's `similar` and `similar_yes`, `Agent::answer`, the flow in the step function, `ask_confirm`'s `similar`, three tests.
- `src-tauri/src/host/agent.rs`: `agent:confirm` reads `similar`.
- `renderer/trail.js`: the button, its settle and its answer; the row note.
- `renderer/overlay.css`: the card's buttons may wrap.
- `tools/ui-shots/lanes/sheet.mjs`: check `confirmAllowSimilar`.

## For ARCHITECTURE.md at the merge

`agent:confirm-request` carries `similar: true` when the step may be allowed as similar. `agent:confirm {id, allow, similar}`. A trail row carries `similar: true` when it ran on an earlier Allow similar.

## Not verified

- Nothing has run on a real desktop: the button was pressed in the page harness, and the flow ran against the test desktop in unit tests.
- No smoke probe in the installed app.
- How the three buttons sit on a narrow card was not looked at; they wrap if they do not fit.
