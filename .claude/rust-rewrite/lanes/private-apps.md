# Lane brief: private-apps

Branch `rust/private-apps`, cut from `rust-rewrite` at `ff1765bd`.

## Goal

A list of apps Aevaleron never reads. While the window in front belongs to a listed app, the app takes no screenshot of it, reads no text from it through UI Automation, runs no text recognition on it and sends nothing from it to a model. It is a privacy promise, so it holds on every path: one gate, asked by every reader.

What the person sees:

- The bar still opens. A typed question is answered without the screen, and the bar says one line: "1Password is on your private apps list, so the screen was not read." with a button to the list.
- Find says the same line and finds nothing. The area picker does not start and the lens draws no loupe; the bar says the line.
- The agent refuses a step that would read or act on a listed app and writes "Refused: ..." in its trail.
- "What was sent" on the answer says "Nothing from the screen." and the line.
- Settings, Privacy, Private apps: the list, Remove on each app, add by program name, and "Add the app that was in front".

## Files

New: `src-tauri/src/privacy.rs` (the list, the gate, the bar's line, the `privacy:*` channels, unit tests), `renderer/private-apps-settings.js`, `src-tauri/src/selftest/probes/private_apps.rs`, `tools/smoke-labels/private-apps.txt`, `tools/ui-shots/lanes/private-apps.mjs`.

Gate calls, each a few lines marked "private apps":

- `capture.rs` `grab`: the one source of screen pixels (Ask's screenshot, a picked area, the lens's loupe, the glass backdrop).
- `desktop/client.rs` `call`: the one door to the desktop helper. `check_op` before an op is sent (and before the helper is started), `check_reply` on what a read answers. This covers Ask's window text and document, Find's reads, every read and action of the agent, and the pointer buddy. `window_shape` blanks the title of a listed app's window and marks it `private: true`.
- `host/ask.rs` `submit_to`: with a listed app in front the question goes alone, the capture is not counted as failed, and `query:done.sent.private` carries the line.
- `find.rs` `take_snapshot` and `refresh`: the gate call, and the line as Find's status with no "Ask" button. Nothing else in the file changed.
- `region.rs` `pick`: the gate call; the bar opens and says the line. Nothing else changed.
- `lens.rs` `loupe`, `backdrop.rs` `tick` (glass goes solid with reason `private-app`), `agents/chats.rs` `sample` (a listed chat app's window is treated as gone).
- `agent/run.rs` and `agent/describe.rs`: a step whose target is in a listed app fails with a row in the trail; a refused observation is a note, and the model is told "(not read: ...)".

Shared edits: `lib.rs` (`mod privacy;`), `store/mod.rs` (`try_get`), `windows/flags.rs` (`remembered_foreground`), `windows/mod.rs` (the bar re-checks when it takes focus), `host/mod.rs` (`overlay:state.privateNotice`), `ipc.rs` and `preload.js` (`// private apps lane` blocks: `privacy:get`, `privacy:add`, `privacy:remove`, Settings only), `renderer/overlay.js` (the notice), `renderer/output.js` (the sent panel line), `settings/settings.html`, `settings/settings.css`, `settings/settings.js` (the glass reason's text), `selftest/probes/mod.rs` (12 lanes, private-apps last, a Notepad lane), `tools/ui-shots/lanes.mjs` and `bridge.test.mjs` (13 lane files).

`lib/desktop/host.cs` is not changed: the gate is on the Rust side of the helper.

## Decisions

- **Storage.** `privateApps` in aevaleron.json: a list of exe names in lower case ending in `.exe`. A missing key reads as the starting list; a stored list, even an empty one, is the person's own. Nothing is added to `store::defaults()`, so an install that never opens the section follows later changes to the starting list.
- **Starting list.** `1password.exe`, `bitwarden.exe`, `dashlane.exe`, `keepass.exe`, `keepassxc.exe`, `protonpass.exe`, `sechealthui.exe` (Windows Security). LastPass is left out: the name its Windows app runs under could not be confirmed. No banking app is on it: none has a name known with confidence.
- **Matching.** By the file name of the process that owns the window, without regard to case, with or without `.exe`, a path cut down to its file name. A Store app's process is the one that owns the `Windows.UI.Core.CoreWindow` child, not ApplicationFrameHost. A process that cannot be opened (one running as administrator) is named from the process list.
- **"The window in front".** The foreground window; while one of Aevaleron's own windows has the foreground, the first ordinary window under it. This is the desktop helper's rule (host.cs `UserForeground`), so both sides mean the same window.
- **Whole screen.** With a listed app in front every screenshot is refused. No window is blacked out.
- **The helper is pinned.** A read of "the window in front" is sent with the handle that was checked, and the answer's window is checked again, so the helper cannot read a different window a moment later.
- **Actions too.** The agent's clicks, hovers, scrolls and drags at a point on a listed app, and typing or keys while one is in front, are refused. That also closes copying from the app and reading the clipboard. Ops that name only an element ref are not checked again: a ref comes from a read that was checked, and adding an app restarts the helper so no older ref survives.
- **Glass.** The backdrop is a capture, so the glass is solid while a listed app is in front (reason `private-app`, explained in Settings, Appearance) and comes back when it leaves.
- **One sentence.** Composed once in Rust (`Refused::message`). The bar gets it as `overlay:state.privateNotice`, the answer as `sent.private`, Find as its status, the agent as the step's error.
- **Settings.** `privacy:add` takes `{name}` or `{front: true}`; `privacy:add` and `privacy:remove` answer with the whole section's state. The app that was in front is the one `flags::remember_foreground` noted when the bar last opened.

## Smoke labels

`privateAppsCapture`, `privateAppsFind`, `privateAppsAsk`, `privateAppsBar`, `privateAppsAgain`, `privateAppsRestored`. Logged, not gated: `privateAppsAdded`, `privateAppsMs` (what one ask of the gate costs, and how long the text read took to come back).

The probe puts the smoke's Notepad on the list, brings it to the front, shows each reader refusing, takes it off, shows the screenshot and the text read working again, and puts the stored list back, also when the probe panics.

## Not verified

- No real password manager was run: the runner has none. The gate is proven on Notepad; the starting list's names were not checked against running apps.
- The agent's refusal is covered by unit tests of the op table and the trail text, not by a smoke run of a whole task.
- A Store app (the CoreWindow rule) and an elevated app (the process list fallback) were not exercised on the runner.
- More than one display: the gate looks at the one window in front, whichever display it is on.

## For the owner to decide

- **Only the window in front counts.** A listed app that is visible beside or behind the window in front is still inside a whole-screen screenshot. The brief asked for the front window; refusing whenever any listed app is visible on screen would be stricter and is a small change in `privacy::check`.
- **The starting list.** Dashlane and Proton Pass are on it; LastPass and banking apps are not (see above).
- **The agent cannot act on a listed app at all**, not only not read it. If typing into a password manager on request should be allowed, the action half of `op_reads` is the place.
- **The glass goes solid** over a listed app. The alternative is glass made from the last frame before the app came to the front, which would keep the look but hold an old picture of the screen.
- **Not gated:** the agent's `clipboard_read` tool (text the person copied themselves), and the window titles the agents lane shows in its cards (they stay on this PC).
- **Adding an app restarts the desktop helper**, which ends a running agent step with "desktop helper stopped". Rare, and safe.
