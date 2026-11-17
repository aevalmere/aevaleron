# Lane agents-note: the agents notification looks and sits like Aevalsistant's

Branch `rust/agents-note`, from `rust-rewrite` at `0c3f8e86`.

## Goal

The owner (2026-11-17): "refine the ai assistant part to just be a notification at the top of the
screen where aevalsistant had it. shouldnt be very distinguishable from aevalsistant."

Before: a thin glass card in Aevaleron's look that grew out of the bar (under it, or above a bar at
the bottom of the screen), with a close button, "Turn on / Not now" buttons on the app's offer, an
"Alt+Tab" text chip, rows that could not be clicked when their window was unknown, and "N more".

After: Aevalsistant's notification, redrawn in the page: a 420 px paper card 16 px below the top of
the work area of the display under the pointer, centred, dropping in from above the top edge and
lifting away. Same palette, type, avatar circle with a status dot, Alt and Tab keycaps, session list
with connectors, "N more in the tray menu", hover that opens the whole message, click to jump,
right-click to put away, the same queue and timings. It no longer depends on the bar or the
on-screen button at all.

## Aevalsistant's notification, from its source

Read from a clone of `aevalmere/aevalsistant` at `b71c3d1` (read only, not built or run). Paths are
in that repo.

| Fact | Aevalsistant | Source | Aevaleron now |
|---|---|---|---|
| Window | Layered popup, WS_EX_TOOLWINDOW, NOACTIVATE, TOPMOST; WDA_EXCLUDEFROMCAPTURE | src/Toast.cs:630-639 | Same flags (`win::tool_window`, `flags::adopt`, `set_focusable(false)`, `exclude_window`); a transparent WebView instead of a layered bitmap |
| Which display | The monitor under the pointer when it appears (MonitorFromPoint, nearest); kept while it shows | Toast.cs:692-699, 662 | Display under the pointer (`screen::matching_px` of the cursor), kept until it goes (`Up::spot`) |
| Where | Window pinned to the top of the work area (rcWork), so under a top taskbar; centred on the work area; card 16 px down | Toast.cs:701-703, 709-715, 690; docs/DESIGN.md:37 | Same (`place_px`): window x = work.x + (work.w - 464 s)/2, y = work.y; card at (22, 16) in it |
| DPI | Per-monitor v2; every length times GetDpiForMonitor/96 | app.manifest:18-19; Toast.cs:698 | Lengths are DIPs in the page; the window is sized in that display's pixels (`spot.scale`). Unit test covers 100, 125, 150% and a display left of the primary |
| Size | Card 420 wide, head 64 tall, 22 px margin round it for the shadow; window as tall as the card fully open | Toast.cs:48, 81-84, 709-719 | Same: window 464 DIPs wide, height 16 + card closed + open growth + 22 |
| Corner radius | 16 | Toast.cs:48 | 16 |
| Fill and edge | Paper #FFF6F1, opaque; 1 px PaperEdge #F0DDD3 at alpha 200, SlateMist #C9D5DA while hovered | Toast.cs:127-133; src/Theme.cs:14-20 | Same colours; the edge is drawn inside the card instead of centred on its outline |
| Shadow | #5A3A3A, two passes: 6 px down blur 9 at 17%, 1 px down blur 1.5 at 10% (three box blurs) | Toast.cs:432-433, 477-521; Theme.cs:24 | CSS `0 6px 11px` at 17% and `0 1px 3px` at 10% (the box blurs' spread as a CSS blur) |
| Font | Segoe UI Variable Text, else Segoe UI; semibold cut for weight | Theme.cs:30-65 | `"Segoe UI Variable Text", "Segoe UI"`, weight 600 |
| Title | 13.5 px semibold Ink #3E3438, then "·  host" 12.5 px Slate #586C7D 5 px after; the host shrinks to a 64 px stub before the title | Toast.cs:174-182, 399-416 | Same sizes and colours; flex shrink plus a 64 px min-width stub |
| Detail | 12 px Slate, one line with an ellipsis; 66 px from the card's left; title 12 px from the top, detail at 32 | Toast.cs:176-183, 191 | Same |
| Avatar | 40 px circle 12 px in, Aevalsistant's own picture, PaperEdge 1.2 px ring | Toast.cs:136-149; assets/avatar.png | Same circle and ring, but a drawn sleepy face (Aevalsistant's tray glyph shapes, Theme.cs:88-145) instead of the picture. See decisions |
| Status dot | 10 px on the avatar's lower right, 2 px paper ring; needs you Blush #E9A9AE, finished SlateSoft #9DB0B9; none for the app's own messages | Toast.cs:151-157 | Same |
| Keycaps | "Alt" "Tab", 14 px from the right, 4 px apart, 20 tall, radius 5, PaperDeep fill, 1.5 px PaperEdge lip, 10.5 px semibold Slate, width max(text + 12, 24); only while Alt+Tab would jump | Toast.cs:159-166, 452-475; src/TrayApp.cs:244 | Same; shown only when the window exists and is not already in front (`can_jump`) |
| Session list | Hairline, 6 px, 28 px rows (24 for a child), 8 px; dot under the avatar's centre (8 px; child 6 px), name 12.5 px Ink (child 12), host 11.5 px, status right-aligned 11.5 px, Ink for "needs you"; working is a 1.4 px Slate ring; hover tints the row PaperDeep, radius 8 | Toast.cs:310-361; docs/DESIGN.md:50 | Same |
| Children | SlateMist lines from 7 px under a session's dot, arm to each child, rounded elbow (radius 4) on the last; the head's own children hang from 4 px under the avatar | Toast.cs:363-394 | Same, drawn with CSS borders |
| How many | 6 rows counting children, then "N more in the tray menu" | Toast.cs:50; TrayApp.cs:247-250 | Same text and count |
| Subagent row | Named by its type or "subagent"; "running 4m"; a click focuses its session's window | TrayApp.cs:257-261 | Same (was "Subagent" and not clickable) |
| Texts | "<project> finished", "is waiting", "stopped early", "needs you"; "Ready for your next prompt.", "Its background agents are done.", "Claude is asking for permission." | src/Core.cs:243-320 | Already the same (`sessions.rs`) |
| Enter | From fully above the top edge down to its place over 360 ms, cubic-bezier(0.16, 1, 0.3, 1); opacity reaches full by 55% of that | Toast.cs:582, 591, 672-676, 828-833; docs/DESIGN.md:12-14 | Same (`drop` and `show` animations; `--from` = card height + 38) |
| Leave | 10 px up and fade over 220 ms, cubic-bezier(0.4, 0, 0.2, 1) | Toast.cs:583, 591, 687 | Same (`lift`) |
| Reduced motion | With Windows animations off: fade in 180 ms, out 160 ms, in place | Toast.cs:591, 661; src/Native.cs:204-205 | `prefers-reduced-motion`, which WebView2 takes from the same Windows setting |
| How long | Finished 8 s, needs you 10 s, app messages 5 s; Short x0.6, Long x1.75; never under 5 s; counts only while the person used the keyboard or mouse in the last 20 s and not while hovered | src/Core.cs:582-587; Toast.cs:592, 660, 868-876 | Same (`card_ms`, `MIN_CARD_MS`, `AWAY_AFTER_MS`). The app's offer keeps 20 s |
| Hover | Hand cursor; edge to SlateMist; row tint; holds the countdown; after the pointer rests 150 ms the card opens over 240 ms to show up to 8 lines (16 px each), closes over 200 ms | Toast.cs:605, 591, 897-935; docs/DESIGN.md:50 | Same. The first line's ellipsis cross-fades to the full text instead of fading word by word |
| Click | Head (or Alt+Tab): clears the queue, hides, focuses that session's newest window. A row: same for that row. List margins: nothing | Toast.cs:908-911; TrayApp.cs:277-294 | Same. The app's offer opens Settings, Agents instead |
| Right click | Dismisses (clears the queue) | Toast.cs:912-914; TrayApp.cs:62 | Same (was a close button) |
| Clicks off the card | Pass through where the bitmap is transparent | Toast.cs:775-776 (per-pixel alpha) | The window lets the mouse through everywhere off the card (`agents:pass`, `flags::set_click_through` with the forwarding poll) |
| Queue | A new one replaces any from the same session and goes on top; a new prompt or the session ending withdraws that session's and the one under comes back; sessions gone in the sweep go too; expiring, clicking or dismissing clears all | TrayApp.cs:171, 201-230, 236-254, 269-274, 332-334 | Same (`enqueue`, `withdraw`, `show_top`, sweep). Was: newest replaced everything, nothing came back |
| Showing a new one while up | Content swaps in place, no animation, countdown restarts | Toast.cs:657-670 | Same |
| Reply text | Waits 300 ms after Stop for the transcript to be written, then reads it | TrayApp.cs:211-226 | Same (`SNIPPET_WAIT_MS`); was read at once |
| Alt+Tab | Swallowed only while a notification with a window to go to shows, and only when that window is not already in front | Toast.cs:948-1004; TrayApp.cs:184-191 | Same check added to `agents::alt_tab` (hook itself is in `voice/ptt.rs`, unchanged) |
| Sound | done.wav or needs-you.wav as it appears; none for app messages or during full screen, busy or presentation | src/Sound.cs:21-54 | Already the same (same WAVs, `do_not_disturb`) |

## What changed

- `renderer/agents.html`, `agents.css`, `agents.js`: rewritten as above. The page no longer loads
  `glass.css` or `glass.js` (no glass, no backdrop frames for this window); `painted()` is a local
  copy of the two-frame wait. All lengths are Aevalsistant's, in DIPs.
- `src-tauri/src/agents/mod.rs`: the bar placement (`spot`, `place_card`, `place_alone`, `from`) is
  gone; `place_px` puts the window at the top centre of the work area of the display under the
  pointer. `Showing` became `Pending` (a queue) plus `Up` (the window's countdown and display).
  `enqueue`, `fill_snippet`, `show_top`, `withdraw`, `take_down` follow Aevalsistant's
  Enqueue/ShowTop/Withdraw/Hide. Action buttons and `act` are gone; the offer is a plain info
  notification whose click opens Settings at `sec-watch-agents`. The window is adopted by
  `windows::flags` so it can be click-through with forwarded moves, and is marked not focusable
  there too. Test for the card under the bar replaced by one for `place_px`.
- `src-tauri/src/agents/native.rs`: `root()` (GetAncestor GA_ROOT) for the Alt+Tab check.
- `src-tauri/src/agents/ipc.rs`: `agents:pass` added, `agents:act` removed.
- `settings/settings.html` (#pg-watch only), `renderer/agents-settings.js`: "card" is "notification";
  the section is "Notifications"; the note says where it appears and that right-click puts it away;
  the Aevalsistant line says both apps would show the same notification in the same place.
- `tools/ui-shots/lanes/watch.mjs`: scenes for the new notification (placement inside the window,
  colours, font, parts, pass-through, hover opening, row clicks, right-click, lift and
  `agents:hidden`, the offer opening Settings) and a static check that the page has no buttons and
  no glass.

Shared files touched (small): `src-tauri/src/ipc.rs` (the agents block of the invoke list:
`agents:act` became `agents:pass`; the Agents page script list no longer includes glass.js),
`preload.js` (same swap in the agents block). No shared counts changed.

## Decisions the owner may want to reverse

1. **The avatar is a drawn face, not Aevalsistant's picture.** The brief said not to copy
   Aevalsistant into this repo, so the 40 px circle holds a vector of Aevalsistant's own tray glyph
   (sleepy face, slate hair, maroon clip) on PaperDeep. To make the two truly identical, put
   Aevalsistant's `assets/avatar.png` in `renderer/` and swap the `<svg>` in `agents.html` for an
   `<img>`; `img-src 'self'` must then go back into that page's CSP.
2. **Aevalsistant's palette, not Aevaleron's.** The notification is warm paper in light and dark
   mode alike, as Aevalsistant's is. It no longer follows Aevaleron's glass slider or text size.
3. **The offer has no buttons.** "Watch your coding agents?" now says "Click to turn it on in
   Settings." and opens Settings, Agents on a click; right-click or waiting puts it away. It still
   waits 20 s rather than an info message's 5 s, since it shows only once.
4. **Clicks pass through the shadow.** Aevalsistant's window takes clicks on any pixel with some
   alpha, the shadow included, and does nothing with them. Here everything off the card's box goes
   through to the app underneath.
5. **A queue with only a reply still being read hides the window.** In Aevalsistant, withdrawing the
   top notification while the only other one is waiting 300 ms for its reply leaves the old one on
   screen (TrayApp.cs:238-239 returns without hiding). That looks like a bug; here the window goes
   and the waiting one comes up when its reply is in.
6. **No detection of a running Aevalsistant.** The Settings line still keys off Aevalsistant being
   installed (`%LOCALAPPDATA%\Aevalsistant\Aevalsistant.exe`), now worded "If it is running, quit
   it...". With both running, two near-identical notifications land in the same spot.

## Where the page cannot match exactly

- Text is drawn by Chromium (DirectWrite, greyscale on a transparent window) where Aevalsistant uses
  GDI+ ClearType; glyph shapes and widths differ slightly, so ellipses can fall a word apart.
- The hairline is a CSS border inside the card; GDI+ centres its pen on the outline (half a pixel
  outside).
- The shadow is a CSS gaussian standing in for three box blurs; same offset, colour and strength.
- Opening on hover cross-fades the first line's ellipsis into the full text and reveals lines
  through a growing clip; Aevalsistant fades each line in as it clears the card's edge.
- Aevalsistant paces its drop with DwmFlush on its own thread; this uses CSS animations on the
  compositor.

## For ARCHITECTURE.md at the merge

The agents notification (`agents/mod.rs`, `renderer/agents.*`) is drawn to match Aevalsistant's
(its `src/Toast.cs`): a 420 DIP paper card in a 464 DIP transparent window at the top centre of the
work area of the display under the pointer when it appears (`place_px`, kept in `Up::spot` until it
goes), independent of the bar. It drops in (360 ms) and lifts out (220 ms) with CSS; the 10.2 pair is
now `agents:hide` answered by `agents:hidden` (`agents::take_down`, wait 600 ms). The window is
adopted by `windows::flags` and lets the mouse through off the card: the page reports
`agents:pass {ignore}` from real or forwarded moves. Notifications queue as Aevalsistant's do
(`enqueue`, `withdraw`, `show_top`): one per session, newest on top, withdrawn on a new prompt or
session end, all cleared on expiry, click or right-click. The page loads no glass.

## Not verified

- Nothing here was seen on a real screen. Run on CI only: the `rust` workflow (unit tests including
  `place_px`, clippy, smoke) and `ui-shots` (scenes `watch-card-done`, `watch-card-open`,
  `watch-card-needs-you`, `watch-card-offer`, each also at 150%).
- Not compared side by side with a running Aevalsistant; the values come from its source.
- Pass-through with forwarded moves on the real window, the drop on a second display, at 125% and
  150%, and with a top taskbar: covered by arithmetic tests and the shared forwarding code, not by a
  probe on a real desktop.
- The Alt+Tab check against the window already in front.
