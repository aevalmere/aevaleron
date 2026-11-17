# Lane agents-note: the agents notification sits and behaves like Aevalsistant's

Branch `rust/agents-note`, from `rust-rewrite` at `0c3f8e86`.

## Goal

The owner (2026-11-17): "refine the ai assistant part to just be a notification at the top of the
screen where aevalsistant had it. shouldnt be very distinguishable from aevalsistant."

Then, after a first version on Aevalsistant's paper: "yes. aevaleron glass. no icon. it just needs
to like notificaiton and behave like aevalsistant yea? and the multiple branches and shit too, can
track time for each too." So the place and behaviour are Aevalsistant's, the material is
Aevaleron's thin glass, there is no avatar, and the session tree shows every session with its
subagents, each with its own time.

Before: a thin glass card that grew out of the bar (under it, or above a bar at the bottom of the
screen), with a close button, "Turn on / Not now" buttons on the app's offer, an "Alt+Tab" text
chip, rows that could not be clicked when their window was unknown, and "N more".

After: a 420 px card of Aevaleron's thin glass 16 px below the top of the work area of the display
under the pointer, centred, dropping in from above the top edge and lifting away, as Aevalsistant's
does. A small status dot (no avatar), Alt and Tab keycaps, the session tree with connectors and a
time on every row, "N more in the tray menu", hover that holds the countdown and opens the whole
message, click to jump, right-click to put away, the same queue and timings. It follows the glass
setting, the light and dark tone and the text size like every other Aevaleron window, and no longer
depends on the bar or the on-screen button.

In short: place and behaviour match Aevalsistant; the material is Aevaleron's by the owner's word;
there is no avatar.

## Aevalsistant's notification, from its source

Read from a clone of `aevalmere/aevalsistant` at `b71c3d1` (read only, not built or run). Paths are
in that repo.

| Fact | Aevalsistant | Source | Aevaleron now |
|---|---|---|---|
| Window | Layered popup, WS_EX_TOOLWINDOW, NOACTIVATE, TOPMOST; WDA_EXCLUDEFROMCAPTURE | src/Toast.cs:630-639 | Same flags (`win::tool_window`, `flags::adopt`, `set_focusable(false)`, `exclude_window`); a transparent WebView instead of a layered bitmap |
| Which display | The monitor under the pointer when it appears (MonitorFromPoint, nearest); kept while it shows | Toast.cs:692-699, 662 | Same (`screen::matching_px` of the cursor, kept in `Up::spot`) |
| Where | Window pinned to the top of the work area (rcWork), so under a top taskbar; centred on the work area; card 16 px down | Toast.cs:701-703, 709-715, 690; docs/DESIGN.md:37 | Same (`place_px`): window x = work.x + (work.w - 464 s)/2, y = work.y; card at (22, 16) in it |
| DPI | Per-monitor v2; every length times GetDpiForMonitor/96 | app.manifest:18-19; Toast.cs:698 | Lengths are DIPs in the page; the window is sized in that display's pixels (`spot.scale`). Unit test covers 100, 125, 150% and a display left of the primary |
| Size | Card 420 wide, 22 px margin round it for the shadow; window as tall as the card fully open | Toast.cs:48, 81-84, 709-719 | Same: window 464 DIPs wide, height 16 + card closed + open growth + 22 |
| Material | Paper #FFF6F1, opaque, 1 px edge, warm shadow, radius 16 | Toast.cs:48, 127-133, 432-433; src/Theme.cs:14-24 | Aevaleron's thin glass (`glass thin`): its blur, rim, shadow and 28 px sheet radius; a little denser while hovered. Follows the glass setting and the light and dark tone. Material is Aevaleron's by the owner's word |
| Type | Segoe UI Variable Text; title 13.5 px semibold, "·  host" 12.5 px; detail 12 px | Theme.cs:30-65; Toast.cs:174-183 | Aevaleron's `--font-text`, row and caption sizes and label colours, scaled by the text size setting. Same layout: the host shrinks to a 64 px stub before the title does |
| Avatar | 40 px circle 12 px in, Aevalsistant's own picture | Toast.cs:136-149; assets/avatar.png | None, by the owner's word |
| Status dot | 10 px on the avatar; needs you Blush, finished SlateSoft; none for the app's own messages | Toast.cs:151-157 | An 8 px dot left of the title: needs you `--danger`, finished `--accent-bright`; none for the app's own messages |
| Keycaps | "Alt" "Tab" at the right of the first line, 4 px apart; only while Alt+Tab would jump | Toast.cs:159-166, 452-475; src/TrayApp.cs:244 | Same place and rule (`can_jump`: the window exists and is not already in front), drawn as Aevaleron keycaps |
| Session list | Hairline, 28 px rows (24 for a child); dot per row (8 px, child 6 px), name, host, status right-aligned, strong for "needs you"; working is a ring; hover tints the row | Toast.cs:310-361; docs/DESIGN.md:50 | Same layout in Aevaleron tokens |
| Children | Lines from 7 px under a session's dot, an arm to each child, a rounded elbow on the last; the head's own children hang from under the avatar | Toast.cs:363-394 | Same, drawn with CSS; the head's children hang from under its dot |
| How many | 6 rows counting children, then "N more in the tray menu" | Toast.cs:50; TrayApp.cs:247-250 | 12 rows counting children (`MAX_ROWS`), room for about five sessions with subagents, then the same "N more in the tray menu" |
| Times | Each row's status as it was when the notification appeared | TrayApp.cs:247-261 | Each row has its own time ("working 12m", "running 4m", "done 3m ago"; blank under a minute). They tick while the notification is up: every 5 s Rust rebuilds the rows and sends them only when a text changed, and the content swaps in place |
| Subagent row | Named by its type or "subagent"; "running 4m"; a click focuses its session's window | TrayApp.cs:257-261 | Same (was "Subagent" and not clickable) |
| Texts | "<project> finished", "is waiting", "stopped early", "needs you"; "Ready for your next prompt." and the rest | src/Core.cs:243-320 | Already the same (`sessions.rs`) |
| Enter | From fully above the top edge down to its place over 360 ms, cubic-bezier(0.16, 1, 0.3, 1); opaque by 55% of that | Toast.cs:582, 591, 672-676, 828-833; docs/DESIGN.md:12-14 | Same (`drop` and `show`; `--from` = card height + 38) |
| Leave | 10 px up and fade over 220 ms, cubic-bezier(0.4, 0, 0.2, 1) | Toast.cs:583, 591, 687 | Same (`lift`) |
| Reduced motion | With Windows animations off: fade in 180 ms, out 160 ms, in place | Toast.cs:591, 661; src/Native.cs:204-205 | Same, on Aevaleron's reduced motion setting or `prefers-reduced-motion` |
| How long | Finished 8 s, needs you 10 s, app messages 5 s; Short x0.6, Long x1.75; never under 5 s; counts only while the person used the keyboard or mouse in the last 20 s and not while hovered | src/Core.cs:582-587; Toast.cs:592, 660, 868-876 | Same (`card_ms`, `MIN_CARD_MS`, `AWAY_AFTER_MS`). The app's offer keeps 20 s |
| Hover | Hand cursor; holds the countdown; after the pointer rests 150 ms the card opens over 240 ms to show up to 8 lines, closes over 200 ms | Toast.cs:605, 591, 897-935; docs/DESIGN.md:50 | Same. The first line's ellipsis cross-fades to the full text |
| Click | Head (or Alt+Tab): clears the queue, hides, focuses that session's newest window. A row: same for that row | Toast.cs:908-911; TrayApp.cs:277-294 | Same. The app's offer opens Settings, Agents instead |
| Right click | Dismisses (clears the queue) | Toast.cs:912-914; TrayApp.cs:62 | Same (was a close button) |
| Clicks off the card | Pass through where the bitmap is transparent | Toast.cs:775-776 | The window lets the mouse through everywhere off the card (`agents:pass`, `flags::set_click_through` with the forwarding poll) |
| Queue | A new one replaces any from the same session and goes on top; a new prompt or the session ending withdraws that session's and the one under comes back; sessions gone in the sweep go too; expiring, clicking or dismissing clears all | TrayApp.cs:171, 201-230, 236-254, 269-274, 332-334 | Same (`enqueue`, `withdraw`, `show_top`, sweep). Was: newest replaced everything, nothing came back |
| Showing a new one while up | Content swaps in place, no animation, countdown restarts | Toast.cs:657-670 | Same |
| Reply text | Waits 300 ms after Stop for the transcript to be written | TrayApp.cs:211-226 | Same (`SNIPPET_WAIT_MS`) |
| Alt+Tab | Swallowed only while a notification with a window to go to shows, and only when that window is not already in front | Toast.cs:948-1004; TrayApp.cs:184-191 | Same check in `agents::alt_tab` (the hook itself is in `voice/ptt.rs`, unchanged) |
| Sound | done.wav or needs-you.wav as it appears; none for app messages or during full screen, busy or presentation | src/Sound.cs:21-54 | Already the same |

## What changed

- `renderer/agents.html`, `agents.css`, `agents.js`: rewritten as above, on `glass.css` and
  `glass.js`. The card is `glass thin`; `painted()` uses `AevalGlass.painted`, and the glass follows
  the window after each resize with `AevalGlass.track`. Colours, type and radii are glass.css
  tokens; the only raw values are Aevalsistant's lengths and timings.
- `src-tauri/src/agents/mod.rs`: the bar placement (`spot`, `place_card`, `place_alone`, `from`) is
  gone; `place_px` puts the window at the top centre of the work area of the display under the
  pointer. `Showing` became `Pending` (a queue) plus `Up` (the window's countdown and display).
  `enqueue`, `fill_snippet`, `show_top`, `withdraw`, `take_down` follow Aevalsistant's
  Enqueue/ShowTop/Withdraw/Hide. Action buttons and `act` are gone; the offer is a plain info
  notification whose click opens Settings at `sec-watch-agents`. The window is adopted by
  `windows::flags` so it can be click-through with forwarded moves, and is marked not focusable.
  `MAX_ROWS` is 12. `send_card` keeps the last payload sent (`Up::sent`), and the 5 s tick calls
  `refresh_card` so the row times move while the notification is up.
- `src-tauri/src/agents/native.rs`: `root()` (GetAncestor GA_ROOT) for the Alt+Tab check.
- `src-tauri/src/agents/ipc.rs`: `agents:pass` added, `agents:act` removed.
- `settings/settings.html` (#pg-watch only), `renderer/agents-settings.js`: "card" is
  "notification"; the section is "Notifications"; the note says where it appears and that
  right-click puts it away; the Aevalsistant line says both apps would show the same notification
  in the same place. None of it describes the material.
- `tools/ui-shots/lanes/watch.mjs`: scenes for the notification (placement in the window, glass and
  no icon, each row's own time, parts, pass-through, hover opening, row clicks, right-click, lift
  and `agents:hidden`, the offer opening Settings), a tree of five sessions with subagents on a
  white desktop checked for the light tone (`watch-card-tree`), and a static check that the page
  loads the glass and has no buttons or icon.

Shared files touched (small): `src-tauri/src/ipc.rs` (the agents block of the invoke list:
`agents:act` became `agents:pass`), `preload.js` (same swap in the agents block). No shared counts
changed.

## Decisions the owner may want to reverse

1. **Aevaleron's glass, no avatar.** The owner's call after seeing the paper version. The first
   version drew Aevalsistant's paper card and a sleepy face; both are gone. The dot left of the
   title is the only mark of state.
2. **12 rows, not 6.** Room for about five sessions with a couple of subagents each before "N more
   in the tray menu".
3. **Times tick while up.** Rust resends the rows every 5 s when a text changed; nothing moves
   between those beats, and a time under a minute is blank.
4. **The offer has no buttons.** "Watch your coding agents?" says "Click to turn it on in
   Settings." and opens Settings, Agents on a click; right-click or waiting puts it away. It still
   waits 20 s, since it shows only once.
5. **Clicks pass through the shadow.** Aevalsistant's window takes clicks on any pixel with some
   alpha and does nothing with them. Here everything off the card goes through.
6. **A queue with only a reply still being read hides the window.** In Aevalsistant, withdrawing the
   top notification while the only other one is waiting 300 ms for its reply leaves the old one on
   screen (TrayApp.cs:238-239). That looks like a bug; here the window goes and the waiting one
   comes up when its reply is in.
7. **No detection of a running Aevalsistant.** The Settings line keys off Aevalsistant being
   installed, worded "If it is running, quit it...". With both running, two notifications land in
   the same spot.

## Where the page differs on purpose or cannot match

- The look is Aevaleron's (glass, tokens, no avatar), so it is distinguishable from Aevalsistant's
  at a glance; the place, width, motion and behaviour are the same.
- Opening on hover cross-fades the first line's ellipsis into the full text and reveals lines
  through a growing clip; Aevalsistant fades each line in as it clears the card's edge.
- Aevalsistant paces its drop with DwmFlush on its own thread; this uses CSS animations.

## For ARCHITECTURE.md at the merge

The agents notification (`agents/mod.rs`, `renderer/agents.*`) sits and behaves like
Aevalsistant's (its `src/Toast.cs`), in Aevaleron's thin glass with no avatar: a 420 DIP card in a
464 DIP transparent window at the top centre of the work area of the display under the pointer when
it appears (`place_px`, kept in `Up::spot` until it goes), independent of the bar. It drops in (360
ms) and lifts out (220 ms) with CSS; the 10.2 pair is `agents:hide` answered by `agents:hidden`
(`agents::take_down`, wait 600 ms). The window is adopted by `windows::flags` and lets the mouse
through off the card: the page reports `agents:pass {ignore}` from real or forwarded moves.
Notifications queue as Aevalsistant's do (`enqueue`, `withdraw`, `show_top`): one per session,
newest on top, withdrawn on a new prompt or session end, all cleared on expiry, click or
right-click. The session tree lists up to 12 rows, each with its own time, refreshed every 5 s while
up and sent only on a change.

## Not verified

- Nothing here was seen on a real screen. Run on CI only: the `rust` workflow (unit tests including
  `place_px` and the 12-row cap, clippy, smoke) and `ui-shots` (scenes `watch-card-done`,
  `watch-card-open`, `watch-card-needs-you`, `watch-card-offer`, `watch-card-tree`, each also at
  150%).
- Times ticking on a real notification: the 5 s refresh is covered by reading the code; the shots
  draw one payload each.
- Not compared side by side with a running Aevalsistant; the values come from its source.
- Pass-through with forwarded moves on the real window, the drop on a second display, at 125% and
  150%, and with a top taskbar: covered by arithmetic tests and the shared forwarding code, not by a
  probe on a real desktop.
- The Alt+Tab check against the window already in front.
