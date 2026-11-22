# Lane note: native-helper

Branch `rust/native-helper`, cut from `origin/rust/next` at `2c2bb4dd`. Dated 2026-11-21.

## Goal

No `powershell.exe` child. The desktop helper (lib/desktop/host.cs, about 3,600 lines of C#
compiled through Add-Type inside a long-lived powershell.exe, about 79 MB) now runs in process as
Rust, with the same ops, request and reply JSON, error codes and coordinates (physical
virtual-screen pixels). Nothing above `desktop/client.rs` changed.

## What changed

- `src-tauri/src/desktop/host/` is the port:
  - `mod.rs`: the worker loop (one request at a time, in order, each on its own MTA thread
    bounded by its timeout), the cancel flags, args parsing, the own-window registry
    (`config`), `ping` (now with `inProcess` and `opMs`, the last and slowest time of each op).
  - `wins.rs`: top-level windows, monitors, focus, close, the own-window, elevation and
    secure-desktop refusals (windows-sys).
  - `uia.rs`: the cached `IUIAutomation` with its conditions, walker and three cache requests,
    one element's properties (raw VARIANT and SAFEARRAY reads), refs (30000, deduped by runtime
    id), the generation counter (a hand-rolled `IUIAutomationStructureChangedEventHandler`, six
    windows at most), readTree with its limits and the 300/700 ms first-read retries, getText,
    findText, docText, scrollToText, selectText, selection, elementAt, focused, resolve.
  - `input.rs`: SendInput with the "AEVB" extra-info mark, chords, the keyboard and point guards,
    click, hover, drag, type, key, scroll, scrollTo, invoke, setValue, restoreCursor.
  - `menus.rs`: menuItems and menuInvoke, Win32 menu handles first, UI Automation otherwise.
  - `pixels.rs`: capture (GDI through `capture::gdi`, Lanczos3 resize, PNG data URL) and ocr
    (Windows.Media.Ocr over a SoftwareBitmap, the 2x enlargement under a 1400 px long edge).
  - `watch.rs`: the low-level mouse and keyboard hooks on their own thread with a message loop,
    a drain thread, the 100 ms / 50 ms throttles, injected events ignored.
- `client.rs`: the spawn, the bootstrap line, the job object, `taskkill` and the stdout reader
  are gone. A "helper" is now a worker thread fed over a channel. HELPER_DOWN and the
  three-deaths rule mean "the worker panicked or stopped answering; the next call starts a
  fresh one". `cancel()` sets every queued and running request's flag directly. `status().pid`
  is the app's own pid while the worker runs.
- `protocol.rs`: the bootstrap line, `ps_quote`, `valid_sha`, `request_line`, `parse_line`,
  `START_TIMEOUT` and their tests are gone.
- `selfcheck.rs`: a new must, `desktopInProcess`, and a `desktopOpMs` log line at the end.
- CI: the "Prebuild desktop helper" step and its `CI_STEPS` row, and the capture step's "no
  bundled desktop/ folder" warning, are gone from rust.yml; release.yml's Tauri prebuild step is
  gone. The Smoke and Desktop selfcheck steps now poll the app's child processes every second
  (`children.ps1`, written by the gate helper step) and fail on `powershell.exe -Command -` or on
  any child outside the allowlist (msedgewebview2, notepad, powershell, conhost, aevaleron, cmd,
  werfault, explorer). The list goes to `ci-logs/.../smoke-children.log` and
  `desktop-children.log`.
- `tauri.conf.json`: the `desktop-*.dll` resource is gone.
- `Cargo.toml`, agent lane blocks: windows gains Foundation_Collections, Globalization,
  Graphics_Imaging, Media_Ocr, Storage_Streams, Win32_System_Com, Win32_System_Ole,
  Win32_System_Variant and loses Win32_System_JobObjects; windows-sys gains Win32_Graphics_Dwm,
  Win32_System_Ole, Win32_System_StationsAndDesktops, Win32_System_SystemInformation.
- Left on disk, untouched: `lib/desktop/*` (Electron), `main.js`, `tools/build-desktop.js` (the
  Electron build and update-check.yml still use it).

## Known differences from host.cs

- getText's reading order sorts on a total key. host.cs's comparator put a span with no rect in
  index order against every other span, which is not a total order (Rust's sort can panic on
  one); here a span with no rect takes the line key of the span before it.
- A worker that times out a request leaves that request's thread running, as host.cs did; the
  request can no longer send input. A worker that stops answering entirely cannot be killed the
  way a process could; it is left behind and a new one starts.
- `ready.pid` and `ping.pid` are the app's own pid.

## Verified

rust.yml green at `7a417c8c` (run 45031541330): fmt, clippy, unit tests (639, also run locally
with tools' rust-check.ps1), installer build, smoke, capture check, desktop selfcheck.

- Desktop selfcheck: every must passes, including the new `desktopInProcess` (ping's pid is the
  app's). desktopReadyMs=4 (was a powershell.exe start), uiaTreeMs=1095 (a cold first read of
  Notepad, including the retry wait), ocrMs=121. `desktopOpMs`: readTree 30 ms warm, getText 30,
  type 56, setValue 11, elementAt 3, key 16, listWindows/click/focusWindow under 1 ms.
- Child processes: the smoke run's children were msedgewebview2, notepad, a second Aevaleron
  (the platform probe), and powershell.exe only in the `-File` (test forms) and `-Command Add-Type`
  (voice probe's speech) forms. No `-Command -` helper; nothing outside the allowlist. The
  desktop selfcheck's children: msedgewebview2 and notepad.

## Not verified

- On CI the selfcheck and smoke drive Notepad, a WinForms test form and the agent probes, so
  readTree, getText, setValue, type, key, click (invoke), elementAt, focusWindow, ocr, capture and
  the input watch are exercised there. menuItems, menuInvoke, findText, docText, scrollToText,
  selectText, selection, drag and closeWindow have unit coverage of their pure parts only;
  nothing on the main line calls the Find-only ones any more (see ask-do.md).
- An elevated target, a locked session and a UAC prompt (the refusals) are not reachable on a
  runner.
- A worker that panics and restarts (HELPER_DOWN, three deaths) is covered by the client's state
  tests, not by a real panic in an op.
