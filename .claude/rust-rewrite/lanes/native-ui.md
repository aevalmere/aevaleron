# Lane: native-ui

Branch `rust/native-ui`, cut from `rust/next`; `origin/rust/cli-providers` merged in. Goal: no Tauri and no WebView2. The app is one native Rust process with Slint 1.19 pages on the software renderer, driven by our own Win32 platform layer. The liquid glass is dropped for now. PARITY.md is the contract.

## Stages

1. **Boot (done; CI green at a795bec7).**
   - `crate::shell` replaces Tauri: AppHandle, Manager, Emitter, Window, events, async runtime, the window builder shim, and the Win32 platform with its event loop.
   - Tray on tray-icon 0.26, hotkeys on global-hotkey 0.10, clipboard and opener on Win32, toasts still on tauri-winrt-notification.
   - The bar (`ui/bar.rs` over `ui/bar.slint`) ports overlay.js, bar.js and voice.js.
   - The answer window (`ui/output.rs`, `ui/markdown.rs`) ports output.js and trail.js.
   - Installer is plain NSIS (`installer.nsi`), built with `cargo build --release` plus makensis.
2. **Every other surface (done in code).**
   - Settings, every section (`ui/settings/*`), with the Slint attribution in About.
   - History, the agents card, the region-text note, the voice pill, region picker, lens and buddy (`ui/cards.rs`, `ui/overlays.rs`).
   - The confirm card, including Allow similar, lives in the answer window's trail.
3. **Cleanup and measurement (done).**
   - The CLI provider picker and per-job providers are native (Settings > Providers).
   - The local gates pass: check, clippy, fmt, and test (656 tests).
   - Measurements come from the runner's smoke (`harness::footprint`) and the install check.

## How it fits

- **Windows.**
  - The bar, pill, region, lens and buddy are layered windows: UpdateLayeredWindow with per-pixel alpha, so clear pixels pass clicks through and no click-through flag or mouse forwarding is needed.
  - Answers, Settings, History, the agents card and the note are opaque: WS_THICKFRAME with no non-client area, DWM rounded corners and a dark frame.
  - Stage MARGIN is 0, so the OS window is the glass.
- **Channels.** Pages hear the same channels with the same JSON (`Page::on_event`) and call the same `ipc` channels through `ui::bridge` → `ipc::invoke`, keeping the allowlist, the caller rules and the panic guard. The core did not change shape.
- **Probes.** The smoke asks pages through `Page::probe(key)`; `selftest::Cx::probe` replaces eval.
- **Colours.** DESIGN palette via the `Theme` global (`ui/theme.slint`). `shell::sys::theme_background` uses the same base (#1A2331 dark, #FAF7F0 light).

## PARITY

**Checked by unit tests or ported 1:1:**
- KEY-*: the reconcile logic, unchanged, on global-hotkey.
- TRAY-01..08: the template, diff, icon sizes, the double-click rule and the listening mark.
- IPC allowlists and caller rules (the tests pass).
- ASK, MODEL, FAIL, AGENT and STORE: untouched core.
- Intent guessing: `ui/intent.rs` is a port of intent.js, with its tests.

**At risk or changed on purpose:**
- **GLASS-01..09:** the backdrop and liquid glass are gone (brief: "remove liquid glass for now"). The tier is always solid, and the contrast and loupe checks are not run.
- **RKEY (bar):**
  - The caret position is read from Slint's caret point, not a character offset. "Backspace at the start" and "Up only from the first row, Down only from the last" use caret x and y.
  - Voice words and early keys are put at the end of the field, not at the caret.
  - Tab to the notice's button marks it, and Enter or Space presses it; it is not real keyboard focus.
- **Bar morphs:** the grow-from-orb and fold-into-orb animations are not ported. `bar:folded` goes after one painted frame (34 ms).
- **Output:**
  - The context menu sends `selection: ""`.
  - Inline bold and code inside a paragraph are flattened.
  - Wide tables and code blocks wrap instead of scrolling sideways.
  - There is no taskbar title text.
- **History:** the reply preview drops markdown marks; the old page showed them raw.
- **Settings job pickers:** they are their own in-place lists, without the shared Combo's arrow-key and type-ahead navigation.
- **PKG-04/05:** names follow the Aevaleron rebrand: `Aevaleron.exe` in `%LOCALAPPDATA%\Aevaleron`, data in `%APPDATA%\aevaleron`. The install is an in-place upgrade over the Tauri one, and there is no WebView2 bootstrapper.
- **Removed smoke labels** (each commented `# native-ui:` in tools/smoke-labels): navigationBlocked, readingMeasure, scrollThumb, globMorphMs, glassLoaded, snapCost, glassCost, backdropIdle, buddyReload and marginClickThrough. `selftest.eval` became `selftest.probe`.
- **New smoke labels:** nativeWindows, processCount, noWebView2 (a must), idlePrivateMB, idleCpuPct and hotkeyToFocusMs.

## Interfaces changed outside the lane

- **desktop/ (native-helper lane), minimal:**
  - `win::place` takes `&crate::shell::WebviewWindow`.
  - `mod.rs` no longer calls `backdrop::init`.
  - `channels.rs` and `client.rs` have path swaps.
  - `checks.rs`: the fixture webviews became native solid windows, the region drag uses SendInput, and the glass checks are gone.
- **llm/:** one line in `client.rs` (`map_or(true, ..)` → `is_none_or`, for clippy 1.100). The cli-providers lane's own llm/ files came in by merge.
- **ipc:** `aeval_invoke` (a Tauri command) became `pub async fn invoke(window, channel, payload)`. `bridge_script` and BRIDGE_JS are gone; `overlay:set-ignore` and `glass:passthrough` are no-ops; `glass:frame` left both listen lists (preload.js too); `models:list` may be called by outputs.
- **native-helper lane merge:** this branch drops tauri.conf.json and the WebView2 allowlist entries entirely, so in children.ps1 the msedgewebview2 allowance becomes unneeded.

## Measurements (runner)

CI run 45041909944 on a795bec7: every step green (fmt, clippy, 656 unit tests, release build, installer, smoke, capture, desktop selfcheck, install check).

| Measure | Target | Value | Where |
|---|---|---|---|
| Installer | report | 8525 KB | install check |
| Exe | report | 28387 KB | install check |
| Idle private memory, fresh install | ≤ 40 MB | 38.9 MB | install check |
| Idle working set | report | 130.1 MB (it includes shared, mapped pages) | install check |
| Idle CPU | ≈ 0 | 0.23 % (install), 0.00 % (smoke) | both |
| Process count | 1 | 1 (install); 2 in the smoke | both |
| Hotkey to focused input | ≤ 30 ms | 12 ms | smoke |
| Second launch hands off | report | 19 ms | install check |
| Threads at idle | report | 18 | install check |
| Upgrade over the Tauri build | in place | ok, from 1.1.1 | install check |
| WebView2 | none | none loaded (noWebView2=ok) | smoke |

- The smoke's processCount of 2 is the PowerShell desktop helper child. That goes away when the native-helper lane (in-process Rust) merges.
- The smoke's idle private memory (85.4 MB) is measured after every probe lane has opened its windows. Hidden windows free their pixels, but the heap does not shrink back. The fresh-install figure is the idle target.
- The step that brought memory under 40 MB was drawing straight into each window's DIB instead of into a second buffer (c3e962ab). The first run measured 42.1 MB.

## Unverified on a real desktop

- Nothing here was run on a desktop; only the CI runner ran the exe. Unverified there:
  - how it looks at 125/150 % scaling and on a second monitor;
  - IME composition in the bar;
  - screen-reader output (announcements go through `a11y:announce`; Slint's own accessibility tree is not wired to UIA here);
  - the drag and resize feel of the opaque windows;
  - the tray icon after an Explorer restart;
  - Focus Assist and toasts.
- Hotkey to focused input is measured on the runner only.

## Open items

- Port the bar's morph animations, and bring the glass back once the native look is settled.
- Read the TextInput's character caret and selection when Slint exposes them: it fixes the bar's caret-at-start rule and the output's context-menu selection.
- Route the Settings job pickers through the shared Combo; add the CLI rows to the settings `text` probe.
- Delete `renderer/`, `settings/` and `preload.js` once no test reads them; ipc tests still compare against preload.js.
