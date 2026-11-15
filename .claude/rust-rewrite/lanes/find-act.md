# Lane brief: find-act

Branch `rust/find-act`, cut from `rust-rewrite`.

## Goal

Find could only show where a match is. Now Enter on the selected match acts on it, so Find does more than an app's own Ctrl+F:

1. Enter presses a button, link, menu item or tick box, selects a tab, list or tree item, gives a text field the keyboard, selects words in a document, or clicks words that only text recognition could read. Each row of the results ends in the one word for what Enter will do: Press, Focus, Select or Click.
2. Before anything is touched the bar and the results fold away and the app gets the foreground back. A match that moved is found again, once. One that is gone is said in the bar and nothing is done.
3. A window's menu commands are found by name even though they are not on screen ("word wrap" in Notepad finds Format, Word Wrap). They are listed after what is on screen, marked as menu commands, and Enter runs them.

A plain typed word still never starts Find by itself. How on-screen text is matched and ranked is unchanged.

## Files

- `src-tauri/src/find/act.rs` (new): which action a hit gets (`element_action`, `plan_span`, `plan_doc`), the menu bounds and parsing (`menu_commands`), the refusals and their sentences, and `perform`, the one desktop call that carries an action out.
- `src-tauri/src/find.rs`: spans and document matches carry what they are (`Span.act`, `Hit.via`); menu commands as a fifth group of hits; `plan_act` (the guard, the one re-read, `refind`); `rows()` for the smoke run; each row's `act` word and `counts.menu` in what the pages are sent.
- `src-tauri/src/host/find.rs`: `find_act` and the `find:act` route. `src-tauri/src/ipc.rs`, `preload.js`: the channel.
- `lib/desktop/host.cs`: spans say their role, patterns and the button or link they sit in; new ops `selectText`, `selection`, `menuItems`, `menuInvoke`; `invoke` action `focusNear`; `click` takes `inWindow`. `src-tauri/src/desktop/client.rs`, `src-tauri/src/host/desktop_ops.rs`: the same ops on the Rust side.
- `renderer/bar.js`, `renderer/trail.js`, `renderer/overlay.css`: Enter, the arrows and F3 in the bar; the action word and the menu mark in the results; Enter on a row.
- `README.md`: the key table.
- Tests: unit tests in `find/act.rs` and `find.rs`; `src-tauri/src/selftest/probes/find_act.rs` with `tools/smoke-labels/find-act.txt`; `tools/ui-shots/lanes/find-act.mjs`.

## Decisions

Each is the owner's to reverse.

- **Keys.** Enter in Find acts on the selected match (it went to the next match before). Down and Up, and F3 and Shift+F3, go to the next and previous match. Shift+Enter (previous) and Ctrl+Enter (explain the match) are as they were. Enter anywhere outside Find is unchanged. While Find has matches, Up and Down no longer recall earlier entries in the bar; they do again once the field is empty or Find has nothing.
- **Acting is the person's own Enter and nothing else.** `find:act` carries the list's generation, the row and its words. Rust acts only when all three are what it has selected right now; otherwise it says "The matches changed just now" and does nothing. The agent's permission system is not involved, and the agent cannot reach this path: the channel is open to the bar and the results window only.
- **Items are selected, not opened.** A list item, tab, tree item or radio button is selected through SelectionItem even when it can be invoked, because invoking a file in a list opens it. A tick box is toggled. A menu item that holds a submenu is opened. Everything else goes Invoke, Toggle, SelectionItem, ExpandCollapse. A real click is used only when no pattern applies, or the element refused the pattern it claimed.
- **A document match inside a link or button presses it.** Plain words in a document are scrolled into view and selected through the TextPattern range, with no click.
- **Text recognition hits get one click at their middle**, after the window has been read again, and only if the window under that point is the hit's own (or another of the same app). Otherwise nothing is clicked and the bar says another window covers the spot.
- **Re-finding never guesses.** After the one re-read the hit is the same one only if as many hits say the same words as before. With fewer or more, which one it was cannot be told and it counts as gone.
- **Classic menus run without being drawn.** For a window with a Win32 menu the commands are read from the menu handle (nothing opens), and Enter sends the application what opening the menus would send (`WM_INITMENU`, `WM_INITMENUPOPUP`), checks the item is still the one listed and is available, then posts its command. The brief said "expands its parents and invokes it"; this does the same thing with no menu flashing on screen. Menus that only exist in UI Automation are run the brief's way (expand each parent, invoke the leaf), but see the limit below.
- **Menu bounds.** Four levels, 400 commands, 300 ms in the helper, and Find stops waiting after 2.5 s. A cut read adds one line under the count ("This app's menus took too long to read, so menu commands are not listed." and three like it). The menu is read once per window, after the on-screen matches have been shown, so it never holds a match back. Menu matching starts at two letters.
- **Own windows.** Find never reads Aevaleron's windows, the helper refuses any ref or point in them, and a refusal is said in the bar.
- **Private apps.** The start of Find's read path (`take_snapshot`, `refresh` up to the snapshot) is untouched, so the private-apps lane can put its gate at Find's entry. The menu read starts after the snapshot, from `refresh`; a gate there covers it too if it stops the snapshot.
- **In passing.** `getText` in the helper threw when a window's text passed `maxChars` (it read a variable that was not set yet). Fixed.

## Limits

- **Menus built only when opened are not listed.** Windows 11 Notepad, WinUI, WPF and most Electron apps give UI Automation no menu items until a menu has been opened once. Find lists what is there and nothing else; it does not open menus to look.
- **Ribbons are not read as menu commands.** A ribbon button that is on screen is an ordinary match and can be pressed; one on another tab is not found.

## Smoke labels

`tools/smoke-labels/find-act.txt`, printed by `selftest/probes/find_act.rs` in the smoke's Notepad, before the voice lane types into it:

| Label | What it proves |
|---|---|
| `findActWord` | The row for a word in the document says Select. |
| `findActStale` | `find:act` naming another list is refused as `changed`, nothing is selected and the bar stays open. |
| `findActSelect` | Enter in the bar selects the word in Notepad, read back with the helper's `selection` op. |
| `findActFolds` | After it the bar is folded, Find is closed and Notepad is the foreground window. |
| `findActPress` | Enter on the menu bar's View opens its menu (a menu window of Notepad's, or the menu's words appearing in the window). The menu is then closed. |
| `findActMenuCommand` | "select all" lists Edit, Select All as a menu row after the on-screen rows, and Enter runs it: the whole text is selected. Prints `skipped` with the reason on a Notepad that lists no menu commands. |
| `probe.find-act` | The probe ran to its end. |

Logged, not gated: `findActNotepad`, `findActRow`, `findActEnter` (key or page, and the time), `findActPressRow`, `findActMenuClosed`, `findActMenuRead` (source, count, limit, time), `findActMenuRow`, `findActTextKept`.

## ui-shots

`find-act-rows` (the action words; also at 200% text), `find-act-menu` (menu-command rows), `find-act-gone` (the notice in the bar). Checks: `findRowsSayTheAction`, `findActionWordIsQuiet`, `findRowEnterActs`, `findRowClickOnlyGoesThere`, `findMenuRowsComeLast`, `findBarCountsMenus`, `findArrowsStep`, `findEnterActs`, `findGoneIsSaid`, `enterOutsideFindAsks`.

## For ARCHITECTURE.md at the merge

`find:act {gen, index, match}` (bar and output windows) answers `{status: "done", act, via}` or `{status, message}` with status `no-hit`, `changed`, `gone`, `disabled`, `no-place`, `own-window`, `elevated`, `secure`, `covered` or `failed`. `find:hits` rows carry `act` ("Press", "Focus", "Select", "Click" or ""), and a row of kind `menu` is a menu command. `find:state.counts` carries `menu`. Helper ops added: `selectText`, `selection`, `menuItems`, `menuInvoke`; helper spans carry `role`, `pat`, `enabled`, `line`, `via`.
