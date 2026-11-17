# Lane note: ask-do

Branch `rust/ask-do`, cut from `rust-rewrite` at `0c3f8e86`. Dated 2026-11-17.

## Goal

On the owner's word, the main line is just Ask and Do. Find leaves the app completely (it lives on
`rust/find`), and the bar decides between asking and doing from what is typed, locally and with no
added latency.

## What changed

### Find, removed

- Rust: `find.rs`, `find/act.rs`, `host/find.rs` and the `find_act` probe are gone, with `Kind::Find`,
  the Find model defaults, the `findOnScreen` hotkey and tray item, `bar_mode` / `find_open` and their
  hooks, the output `view` and `view:set`, `settings:set-find` and `settings:save-find-model`, the
  `find:*` and `bar:mode` channels, the Find answer-menu row, the desktop client's Find-only calls
  (`find_text`, `scroll_to_text`, `select_text`, `selection`, `menu_items`, `menu_invoke`) and their
  `desktop_ops` and privacy rows.
- The lens draws only the agent's control frame and its chip: hits, the current-hit ring, the why chip
  and the loupe are gone, and so are `backdrop::crop`, `crop_url`, `LOUPE_QUALITY`,
  `capture::crop_dip`, `Rgb::crop` and `lens:loupe`.
- Settings: the Find page, the Find model picker and the Find shortcut row.
- Bar: the `/` and `//` prefixes, Ctrl+F, Ctrl+3, live Find, the scope button, the guess card.
- Output: the Find results view and its styles, "Find this on screen instead".
- Tour: the Find step. It is now three steps: the key card, the ask step, the token step.
- Settings from an older install (`findModel`, `findModelChosen`, `findScope`, `findRelatedMode`,
  `findRelated`, `findOcr`, `lastMode`, `shortcuts.findOnScreen`, `nativeModels.*.find`) are dropped
  on load (`store::migrate`); a test covers it.
- `bar:context` no longer carries the window's text (up to 16000 characters per bar open). Only Find's
  guess read it; Ask still reads the same prefetched text in Rust.

### Ask or Do

- `renderer/intent.js`: two intents. `?` forces Ask, `>` forces Do. A clear order (Do score at least
  0.6 and 0.15 over Ask) is Do; anything else is Ask, and a line with some task in it (Do score at
  least 0.2) carries `offerDo`. Every old Find phrase maps to ask ("find", "where is", "locate",
  "look for", "show me where") or do ("search for", "jump to", "highlight"). The examples are a
  question and a task per app.
- Bar: Enter on a clear Do sends `agent:start {task}`, the same call a `>` line sends, so
  `agent/gate.rs` and `auto.rs` are unchanged and never see the guess. Enter on anything else sends
  `query:submit {question, offerDo}`. Alt+Enter does the other one. The token shows Do for a clear
  task, and a quieter glyph (`data-confidence="low"`) for the middle. Ctrl+1 is Ask, Ctrl+2 is Do,
  in the token's tooltip, its menu, its description and Alt+Down's menu.
- `SubmitOpts.inferred` is now `offer_do`, carried as `offerDo` in `query:done`. The answer then shows
  "Do this instead", which sends `agent:start {task: question}` from that window (allowed by the
  existing own-transcript rule in `ipc::authorize`). An answer that reads as steps already has Do it,
  so it does not get both.
- A task's trail shows "Just answer" until its first act: it stops the task and sends the same words
  as `query:submit`, which now answers in the calling output window when an output sends it.
- The text entry point for the wake lane: **`windows::submit_text(app, text) -> bool`**. It opens the
  bar and emits `bar:submit {text}`; bar.js runs it exactly as typed plus Enter (prefixes, the guess,
  a task waiting on a reply). False when the bar cannot open (no overlay, or the HUD).

## Files

Rust: `lens.rs`, `backdrop.rs`, `capture.rs`, `capture/encode.rs`, `store/mod.rs`, `hotkeys.rs`,
`tray.rs`, `state.rs`, `buddy.rs`, `ipc.rs`, `lib.rs`, `region.rs`, `privacy.rs`, `windows/mod.rs`,
`windows/outputs.rs`, `host/{mod,ask,agent,menu,settings,desktop_ops}.rs`, `desktop/{client,selfcheck}.rs`,
`llm/{catalogue,provider,settings,client,wire,errors,fake}.rs`, `agent/{run,seams}.rs`,
`region_text/mod.rs`, `selftest/probes/{bar,capture,sheet,platform,private_apps,mod}.rs`.
Pages: `preload.js`, `renderer/{intent,bar,overlay,output,trail,lens,voice}.js`,
`renderer/{index,output,lens}.html`, `renderer/{overlay,lens}.css`, `renderer/icons.svg`,
`settings/settings.{html,js}`.
Tools: `tools/smoke-labels/{bar,capture,private-apps}.txt`, `tools/fixtures/llm/errors.json`,
`tools/ui-shots/{kit,lanes,bridge.test}.mjs`, `tools/ui-shots/lanes/{bar,capture,settings,sheet}.mjs`.
Deleted: the Find files above, `tools/fixtures/llm/find.jsonl`, `tools/smoke-labels/find-act.txt`,
`tools/ui-shots/lanes/find-act.mjs`.

## Shared counts changed

- `selftest::probes::LANES`: 14 to 13.
- ui-shots `LANES` (`bridge.test.mjs` `laneFiles`): 15 to 14.
- `store::ACTIONS`: 6 to 5.
- `outputs::RESENT`: 7 to 6.
- overlay-state test `ELECTRON`: 21 to 19; `settings_get` test `ELECTRON`: 28 to 25.
- `llm::settings::KINDS`: 4 to 3.
- Smoke labels: removed `lensWhyClear`, `privateAppsFind` and the find-act lane's; added
  `intentOffersDo` and `textEntryDecides`.
- `LISTEN_CHANNELS`: minus `find:hits`, `find:state`, `view:set`, `lens:loupe`; plus `bar:submit`.

## Decisions the owner may want to reverse

- Keys: Ctrl+1 Ask, Ctrl+2 Do. Ctrl+3 and Ctrl+F now do nothing.
- A clear task runs on Enter with no card first. The gate still asks before anything risky.
- Alt+Enter does the other one (it used to mean "run the guessed task").
- `/` is plain text now, so "/etc/hosts" and "/inv" are just typed.
- Kept the C# helper ops in `lib/desktop/host.cs` (findText, scrollToText, selectText, menuItems,
  menuInvoke); only their Rust callers went. Deleting them means rebuilding the helper.
- The viewfinder icon stays in `icons.svg` so the sprite's slots do not move.
- "Just answer" goes away at the first act of any kind, reads included.
- The commit trailer is `Co-Authored-By: Claude Opus 5.5`, the one the session asked for.

## For ARCHITECTURE.md

The bar has two intents, Ask and Do, decided by `renderer/intent.js` on every keystroke with no model
call. `?` and `>` force one; otherwise a clear task becomes Do and runs on Enter through `agent:start`,
the same path as `>`, so the executor's gate never sees the guess. Everything else is asked. A line
that could be either is asked with `offerDo`, and its answer offers "Do this instead". A task offers
"Just answer" until it first acts. Anything that hears or reads a line for the person (the wake word)
goes in through `windows::submit_text`, which runs it as typed plus Enter. Find was removed on
2026-11-17 and lives on `rust/find`. The lens now draws only the agent's frame.

## Not verified

- Nothing here has run on this PC: there is no Rust toolchain, and the app is never started here.
  CI is the compiler and the only runner.
- The classifier's speed and table are checked in the smoke (`intentFast`, `intentTable`,
  `intentOffersDo`); locally the table passed under Node, worst case 0.02 ms. `barOpenMs` is logged,
  not gated; it is only expected to be a little lower now that `bar:context` is smaller.
- `textEntryDecides` runs with the key cleared, so a task is refused with `needs-key` before anything
  happens. A real task started by `submit_text` is not exercised.
- "Do this instead" and "Just answer" are covered by ui-shots against the mock bridge, not end to end
  against Rust.
