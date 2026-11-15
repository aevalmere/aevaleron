# Lane brief: region-text

Branch `rust/region-text`, cut from `origin/rust-rewrite` at `ff1765bd`.

## Goal

"Copy text from screen": the person drags a rectangle over anything on screen (a picture, a video frame, a dialog whose text cannot be selected) and the text inside it is on the clipboard.

- Entry points: the action `copyRegionText` with its own shortcut (unset by default, set in Settings, Shortcuts, under "Ask about an area"), and the tray item `Copy text from screen…` under `Ask about an area…`.
- The area picker is reused. Opened for this action its line reads "Drag over the text to copy it", its name for screen readers is "Choose an area of the screen to copy its text", and the keyboard help says "Enter copies the text".
- Reading prefers exact text. When one window lies under the whole rectangle and UI Automation gives text for the lines or elements inside it, that text is used in reading order. Otherwise the rectangle is read with Windows OCR, the engine Find uses.
- The result is on the clipboard and a small note beside the rectangle says "Copied 42 words" with the first few words, then goes away by itself. The note never takes the keyboard. When nothing readable is there it says "No text found here" and the clipboard is as it was.
- Escape closes the picker and changes nothing.

## Privacy

Nothing is sent to a model and nothing is written to disk. The text goes from the desktop helper (UI Automation or Windows OCR, both on this PC) to the clipboard and is forgotten; the last outcome kept for the smoke run holds the count line, the source and the time, never the text. The module `src-tauri/src/region_text/` names no model, question, Find, task, history or settings code and no file API, and the unit test `nothing_here_reaches_a_model_or_the_disk` reads the three source files and fails if one of them ever does. The note's page calls one channel, `region:note-ready`, and the ipc table lets its window call nothing else (`the_copy_text_note_calls_only_its_own_channel`).

## Files

New Rust: `src-tauri/src/region_text/mod.rs` (the flow: start, pick, read, copy, say), `assemble.rs` (pieces with rectangles to text; pure), `note.rs` (the note's window and words), `src-tauri/src/selftest/probes/region_text.rs`.
New pages: `renderer/region-note.html`, `renderer/region-note.js`, `renderer/region-note.css`.
New tests: `tools/ui-shots/lanes/region-text.mjs`, `tools/smoke-labels/region-text.txt`.

Shared files, each edit small and marked "region-text lane" where the file marks lanes:

- `src-tauri/src/region.rs`: `select_text` and `page_script` added after `open_window`; `open_window` takes its initialization script from `page_script`. `pick`, `select` and the start of the Ask capture are untouched.
- `src-tauri/src/store/mod.rs` (`ACTIONS` 5 to 6, no default chord), `hotkeys.rs` (the action's name), `windows/mod.rs` (`on_hotkey` arm), `tray.rs` (the item and its menu arm), `lib.rs` (`mod region_text`).
- `src-tauri/src/ipc.rs`, `preload.js`: `region:note-ready` (invoke) and `region:note` (listen), appended to both lists; caller `Note` for the window `region-note`. `desktop/channels.rs`: the route arm.
- `src-tauri/src/selftest/probes/mod.rs` (lane count 11 to 12, a Notepad lane), `probes/platform.rs` (the tray order the smoke run expects).
- `renderer/region.html` (the hint's first words got an id), `renderer/region.js` (the text mode's three strings), `settings/settings.html` (the shortcut row).
- `tools/ui-shots/lanes.mjs`, `tools/ui-shots/bridge.test.mjs` (lane count 12 to 13).

`find.rs` and `host/find.rs` are not touched. Find's reading is called as it is: `Desktop::get_text` and `Desktop::ocr`.

## Decisions

- Which reader. Five points of the rectangle (the middle and just inside each corner) are asked which window is there. When all five name one window, `get_text` is asked for that window's text inside the rectangle; a piece counts when at least half of it lies inside. When the points name different windows, when the window gives no text there, or when it refuses (an elevated app, one of Aevaleron's own windows, a tree too slow to walk), the rectangle is read by OCR.
- UI Automation gives whole lines and whole elements. A rectangle that covers half of a line takes the line or leaves it; it never cuts a word. OCR works by words, so there a rectangle cuts where the person drew.
- Reading order: pieces whose bands overlap by half make one line, left to right, joined by one space (none between two Chinese or Japanese characters); lines run top to bottom. A step between two lines that is half as large again as the smallest step is kept as one empty line. Evenly spaced rows keep no empty lines, however far apart.
- Hyphens. A word is joined across a line break only when all of this holds: the line ends in a hyphen after two or more letters, the next line starts with two or more small letters, that word is not "and", "or", "to" (or "und", "oder", "et"), the line reaches the right edge of the block, and no empty line follows. The hyphen is dropped. A compound that happens to break at its own hyphen ("well-" / "known") loses the hyphen; that is the cost of the rule.
- The clipboard gets CRLF line ends, no trailing spaces, no leading or trailing empty lines.
- The note: a window of its own (`region-note`), never focusable, clicks pass through, left out of screenshots, built hidden when the picker opens and kept hidden afterwards. Under the rectangle, above it when there is no room, inside its bottom edge when the rectangle fills the screen. 2.2 s for a copy, 3.2 s for "No text found here", 6 s for an error. A read that takes over 0.7 s shows "Reading the text…" meanwhile.
- Focus. The picker has to take the keyboard (Escape, arrows). When it closes, the window that had the keyboard before gets it back if Windows left it with one of Aevaleron's windows or with nothing.
- Errors say what happened and what to do: "Windows cannot read text in pictures yet. Add a language with text recognition in Windows Settings, then try again."; "The text could not be read. Nothing was copied. Try again."; "The text could not be copied. Another app is using the clipboard. Try again."
- No typed command in the bar. The bar has no commands of that kind: its prefixes (`/`, `?`, `>`) only pick Ask, Find or Do for the words that follow. Adding the first command word was outside this lane.
- One copy at a time: the action is ignored while a pick or a read is under way, and while a task's HUD is up, as the Ask pick is.

## For the private-apps lane

This path does not go through `region::pick`. Its start is `region_text::run` in `src-tauri/src/region_text/mod.rs`; a comment there marks where the gate call belongs, before the picker opens. `region.rs`'s capture path was left as it was, so a gate call at the start of `pick` merges cleanly.

## Smoke labels

`tools/smoke-labels/region-text.txt`. The probe runs with the smoke's Notepad, between `llm` and `voice`, and puts the clipboard back.

- `regionTextFront` (logged): Notepad had the keyboard when the probe began.
- `regionTextHint`: the action opens the picker and its line and name say the text will be copied.
- `regionTextEscape`: a real Escape closes it; the clipboard is as it was, nothing was read, no note shows, the keyboard is where it was.
- `regionTextNotepad`: the whole action (the picker driven by a drag dispatched in its page, as the capture check does) over the fixture's three lines puts exactly those lines on the clipboard.
- `regionTextExact`: that read came from UI Automation.
- `regionTextReadMs` (logged): the source and the time.
- `regionTextNote`: the note shows "Copied 11 words" and the first words, and Notepad still has the keyboard.
- `regionTextNoteLeaves`: the note goes away by itself.
- `regionTextPicture`: the step after the picker, over a window that shows two lines of text as a picture (no text for UI Automation), puts both lines on the clipboard through OCR.
- `regionTextNothing`: over the blank part of that picture the clipboard is as it was and the note says "No text found here".

The last two print `skipped (no OCR language)` on a PC where Windows has no OCR language.

## Page scenes

`tools/ui-shots/lanes/region-text.mjs`: `region-text-picker` (the picker's line, name and keyboard help), `region-text-note-copied`, `-none`, `-reading`, `-error` (one scene per kind, so each is audited), `region-text-note-long` (the longest first words stay inside; a note with a time fades; the newest note wins), `region-text-shortcut` (the Settings row unset, recording, set).

## Not verified

- Nothing was run on a desktop with a person at it. The runner has one 1024x720 display at 100%: two displays, other scales and a rectangle that spans two windows are covered by unit tests of the arithmetic only.
- Exact text was proved against Notepad only. Browsers, Office and Electron apps give UI Automation text in other shapes (nested elements, long documents); the assembly rules have unit tests for those shapes but no live run.
- Whether a screen reader says the result. The note has `role="status"`, and the words also go through the app's own announcer, which speaks only when an Aevaleron window has the focus. After a copy the focus is back in the person's app, so Narrator may say nothing.
- The shortcut was not pressed by a real keyboard in the smoke run; the probe starts the action through the same function the shortcut and the tray item call.
- OCR accuracy on small or low-contrast text is Windows' own and was not measured.

## For the owner to decide

- When exact text is found, buttons and tab names in the rectangle are left out: the helper's text reader covers text, edit fields, documents, list rows, links, headers, tree rows and menu items. A rectangle over a dialog's message and its buttons copies the message only. Reading such a rectangle by OCR as well, and merging, would add them at the cost of two reads and possible doubles.
- The note does not follow the text-size setting (neither does the dictation pill).
- The hyphen rule drops the hyphen of a compound broken at a line end. The other choice is never to join.
- A typed bar command (for example "copy text") can be added once the bar has command words.
