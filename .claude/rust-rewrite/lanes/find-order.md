# Lane brief: find-order

Branch `rust/find-order`, cut from `rust-rewrite` at `ec8f2aa4`.

## Goal

The owner on 2026-11-15, about Find: "the behavior is good. its just like the stuff it finds is completely random". The keys and what Enter does are untouched. This lane changes what Find lists and in what order.

## What was wrong

- **Whole-screen scope marked words nobody can see.** It read up to six windows on the display and never asked which of them is behind another. With maximized windows stacked on a laptop, the five behind the front one gave matches too, and their marks were drawn on the front window, on words that are not the query. The owner's `findScope` is `screen`.
- **The first match was a tab or a title.** Matches of every window's controls came first, then every window's document. In a browser the tab title and the address bar came before the page.
- **A phrase lit up its words everywhere.** `invoice total` marked every `invoice` and every `total` in any line holding both, even with the phrase itself on screen.

## What changed

- `screen_targets` (find.rs): the windows are taken front to back, as the helper lists them. A window that one window in front hides whole is not read and does not count against the limit of six.
- A piece of text hidden whole by windows in front is dropped when the screen is read (`buried`). A match whose middle is under a window in front is dropped (`Snapshot::hides`): exact, document and related matches alike.
- The helper's `listWindows` says `through` for a window that does not count as hiding anything: one the mouse passes through, one that never takes focus, one kept on top.
- `State::hits`: the front window's matches first; in each window the document's matches before its controls'.
- `match_literal`: the words of a phrase found apart are held back, like near matches, and shown only when the phrase is nowhere. They come in the order they stand in the line.

## Decisions the owner may want to reverse

- **A window kept on top hides nothing.** An overlay is always on top and may be see-through and screen-sized; if it counted, Find could list nothing at all. The cost: words under a small always-on-top window (picture in picture, a sticky note) are still marked.
- **The document before the controls.** In an app whose page is not a document to Windows, everything is still in reading order from the top, so a title bar match can still come first there.
- **A match is hidden by its middle point.** A word half under another window's edge may stay or go.
- **The scope setting is left as he set it.** Whole screen is slower than this window and reads more; it now reads only what shows.

## Files

- `src-tauri/src/find.rs`: `Rect::holds`, `Rect::contains`, `under_window`, `buried`, `screen_targets`, `Snapshot::above`, `Snapshot::hides`, `Snapshot::depth`, the order in `State::hits`, `set_doc`, `add_related`, `rematch`, `take_snapshot`, `match_literal`; five tests.
- `lib/desktop/host.cs`: `through` in `WinInfo`.

## For ARCHITECTURE.md at the merge

`listWindows` rows carry `through: bool`. A whole-screen Find reads the windows that show, front to back, and lists nothing that a window in front covers.

## Not verified

- Nothing ran on a real desktop with overlapping windows: the rules are unit tested with made-up rectangles. No smoke probe covers whole-screen scope.
- A window that is only partly behind another is hidden by rectangles, not by what is drawn: a window with rounded corners or a see-through part still counts as covering its whole rectangle.
- Matches inside one window that another part of the same window covers (a dialog over a page, an open dropdown) are still listed.
- Text read by OCR has estimated places; nothing here makes those more exact.
