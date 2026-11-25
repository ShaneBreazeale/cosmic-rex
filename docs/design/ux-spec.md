# cosmic-rex UX specification (2026-11-25)

Unofficial Rex client for COSMIC. Companion files: `backend-gaps.md`, `roadmap.md`, `mockups.html`.

Evidence tags: **[V]** read in source at the pinned revisions (libcosmic `009fb9d`, rex-rune `bd77ede`, local COSMIC defaults file, official docs text), **[I]** inferred, **[U]** unknown until tried. "Verified" means the source says so; nothing here was built or run. Native Rex was studied through its docs text only (actions, settings, hosts); no screenshots were consulted, and no branding, artwork or wording is reused.

## 1. Goals

1. From a Linux desk, know within a glance which agent on which box needs the operator, and answer it without breaking concentration elsewhere.
2. Never send input to the wrong machine, the wrong prompt, or a pane that is not live.
3. Coexist with the Mac and iOS apps on the same sessions: observing is free of side effects; anything other devices will notice is a deliberate act.
4. Feel like a COSMIC app: theme, density, radius, tiling and keyboard conventions come from the desktop.

Non-goals for now: server administration, Lua/keymap editing, floating layers, file transfer, theme editing.

## 2. Operator and contexts

One operator, expert with terminals, runs 3 to 10 agents over 2 to 4 boxes (a Mac that also runs the native app, a Linux server, this laptop). No colleagues share the client.

| Context | What is scarce | Design consequence |
| --- | --- | --- |
| A. Working in another app or workspace | Attention | Applet count and one calm notification; no approval outside the window |
| B. Triage sweep: several agents waiting | Time per item | One queue across boxes, one key to the next item, answer in place |
| C. Deep work in one session | Screen space, keystroke fidelity | Chrome recedes; every key reaches the terminal unless it has Ctrl+Shift or Ctrl+Alt |
| D. Tiled at half or third width | Width | Narrow layout that keeps identity and the inbox count |
| E. Battery, Wi-Fi, suspend/resume | Trust in what is shown | Honest connection state; never a frozen screen that looks live |

## 3. Task analysis

Frequency: H many times an hour, M daily, L rare. Criticality: consequence of an error or a miss.

| # | Task | Freq | Crit | Design response |
| --- | --- | --- | --- | --- |
| T1 | Notice that an agent is blocked | H | High (agents idle; a missed approval stalls a run) | Attention chip, sidebar shapes, applet, notification (10, 18) |
| T2 | Find out what it wants and where | H | High | Inbox row: state, box, session, tab, waited time, its own words marked as such |
| T3 | Answer it | H | **Highest** (approves commands on a real machine) | Answer sheet with stale guard (10.1) |
| T4 | Type in a terminal | H | **Highest** (wrong box) | Box identity at the locus of typing (8); input paused when not live (9) |
| T5 | Switch session, tab or box | H | Medium | Quick switcher (12), sidebar, stable order |
| T6 | Glance at progress of working agents | M | Low | Sidebar shapes; "Working (n)" line in the inbox |
| T7 | Recover from a dropped link | M | High | States in 9; nothing replayed |
| T8 | Start a tab, split or session | M | Medium (visible to other devices) | Explicit commands only; target box named |
| T9 | Fit a pane to this window | M | Medium (re-wraps the Mac's view) | Size chip and a choice (11) |
| T10 | Close a pane, tab or session | L | High (ends programs) | Destructive policy (15) |
| T11 | Add or remove a box | L | High (grants full account control; list is shared) | Add-box flow (13) |

T3 and T4 dominate risk; T1, T2 and T5 dominate frequency. The layout spends its permanent pixels on those five and puts everything else behind the switcher.

## 4. Situation awareness

| Level | The operator must know | Carried by |
| --- | --- | --- |
| Perceive | Which box, session, tab and pane has the keyboard | Header breadcrumb, pane header chip, focus ring, window title |
| Perceive | Is this pane live, and whose size is it | Pane header: link state, size chip; offline treatment |
| Perceive | How many agents need me, where | Attention chip, sidebar shapes, applet |
| Comprehend | What a blocked agent is asking, and that the text is the program's claim, not the app's | Inbox row and answer sheet with "the program says" framing and a screen excerpt |
| Comprehend | Why a box is not connected and whether it is retrying | Box row state word, reason, countdown |
| Comprehend | Who else is looking at this session | Size chip names the owning device; session tooltip lists devices |
| Project | What happens if I type now | Input paused states say so before the keystroke, not after |
| Project | What other devices will see if I fit, close or switch | Consequence sentence in the popover or dialog |
| Project | What I will find when a box returns | "As of 14:02" on stale rows; "screen reloaded, nothing you typed was sent" on return |

## 5. Hazards

| # | Hazard | Designed out by | Residual risk |
| --- | --- | --- | --- |
| H1 | Typing into the wrong box | Identity in four places, one of them touching the pane (8). The app never moves the keyboard target by itself: notifications, inbox changes and reconnects do not navigate. Keys are addressed to the pane the operator last chose. | Another device can change the shared active tab; see 6.3 |
| H2 | Stale approval: the answer lands on a different prompt | Reply bound to box, pane, record id and record revision; any change disarms the sheet. 300 ms inert after open or change. Repeated Enter ignored (10.1). | Programs that change prompt without reporting status |
| H3 | Blocked agent not noticed | Persistent chip with count, sidebar shape on box and session even when collapsed, applet, one notification per transition, waited time shown. Nothing auto-dismisses a blocked item. | Agents that never report (OpenCode without hooks [V docs]) |
| H4 | Accidental close | No single key ends a program. Dialog names box and what ends, default is Keep; undo where the server offers it (15). Closing the window ends nothing and never asks. | |
| H5 | Resizing the Mac unintentionally | Panes show their owner's grid; typing never takes size from a connected device; "Fit to this window" is explicit and says who is affected (11). | **Today the pinned core takes size on the first key press [V]**; see `backend-gaps.md` G1 |
| H6 | Disconnected pane mistaken for live | Pattern overlay plus plate plus hidden cursor plus paused input; state shown before typing; all boxes re-checked on resume from suspend (9). | Half-open links until the first unanswered request |
| H7 | Program text spoofing the app | Status text is plain, bounded, stripped [V backend `plain`], always inside a quoted frame with its source. Client words and program words never share a line. Program text never picks an icon, colour, sound or button. | |
| H8 | Believing a reply arrived when it did not | Three outcomes shown: confirmed by a status change, sent but unconfirmed, not sent. No retry button; nothing replayed. | |

## 6. Information architecture

```
Boxes (saved, ordered by the operator)            Inbox (cuts across everything)
 └ Sessions (server order, shared)                 one row per reporting pane,
    └ Tabs (Rex windows; active tab is shared)     grouped by urgency, each row
       └ Panes (Rex blocks; layout is the server's) names its box > session > tab
```

Three concepts carry the whole design:

1. **A box is a place with a face.** Each box has a label, a two-letter monogram and one of six identity tints. The monogram is on every surface that can receive input or cause an effect on that box.
2. **Attention is one queue.** Agents are not a fifth level of the tree. They are an index into it, sorted by who has waited longest.
3. **Shown means true, and effects are chosen.** A pane says whether it is live and whose size it has. Anything other devices will notice (fit, switch tab, split, close) happens only on a direct command.

The word "box" is the operator's; "host" stays an internal term.

### 6.1 Sidebar: boxes and sessions
- Order is spatial memory: boxes in operator order (drag to reorder), sessions in server order. **Nothing is re-sorted by status**; urgency lives in shapes and in the inbox.
- Box row: monogram chip, label, link state (shape and word when not online; quiet when online). Collapsible; a collapsed box shows its worst session shape and a count.
- Session row: status shape for its worst pane, label, count when more than one pane needs attention. The session with keyboard focus has the accent selection pill.
- A box that is offline keeps its last sessions, dimmed, under "as of 14:02". They cannot be opened.
- Footer: "Add box".
- Build: override `Application::nav_bar()` [V `src/app/mod.rs:390`] with a `scrollable` column of `button::custom` rows. The stock `nav_bar` model has text, icon, indent, divider and closable per row [V segmented_button entity builder] and no slot for a trailing shape or second line, so it is not enough.

### 6.2 Tabs and panes
- Tabs: `widget::tab_bar::horizontal` [V], one per Rex window, each with a status shape for unfocused tabs. Tab order and the active tab are the server's.
- Panes: the existing custom `Grid` widget, with a 28 px header per pane added (custom drawing inside `Grid`): monogram chip, title, size chip, status shape. Headers are shown even for a single pane, because the header is where identity touches the typing.

### 6.3 The shared active tab
The active tab belongs to the session, not to this window. Rule: **follow when idle, hold when typing.** If another device switches tab within 1.5 s of the operator's last key press, this window stays put and shows "iPhone switched this session to ‹logs›. Show". Otherwise it follows and says "Switched by iPhone" for 4 s with "Go back". Whether the core can keep rendering a tab that is not the server's active one is [U]; fallback if not: follow, and discard key presses for 400 ms after a switch this window did not ask for, with a visible "key not sent" note.

## 7. Main window

### 7.1 Wide, 1100 px and up (laptop full screen)
```
┌────────────────────────────────────────────────────────────────────────────────────────────────┐
│ [≡]            [ST] studio › api-refactor › tests  ▾            [◆ 2 need you]  [⋯]   – □ ×  │
├────────────────┬───────────────────────────────────────────────────────┬───────────────────────┤
│ [ST] studio    │ [ tests ] [◔ server ] [  shell ]                 [+]  │ Inbox             [×] │
│ ▌◆ api-refac 2 │┌ [ST] claude-code ─────── ⊡ Sized by MacBook 142×38 ◆┐│ NEEDS YOU             │
│  ◔ billing-mig ││                                                     ││ ◆ Approval     4 min  │
│  ○ docs-site   ││   terminal, letterboxed at the owner's grid         ││  [ST] api-refactor ›  │
│ [FG] forge     ││                                                     ││       tests           │
│  ✕ infra       │└─────────────────────────────────────────────────────┘│  ▏Run cargo test?     │
│  ✓ nightly     │┌ [ST] zsh ──── ⤢ Fits here ┐┌ [ST] tail ── ⤢ Fits h. ┐│ ◆ Question     1 min  │
│ [LP] laptop    ││                           ││                        ││  [FG] infra › plan    │
│  ○ scratch     ││                           ││                        ││ FAILED                │
│ [AT] attic  ⦸  │└───────────────────────────┘└────────────────────────┘│ ✕ Error   [FG] infra  │
│  as of 14:02   │                                                       │ FINISHED              │
│  ○ backups     │                                                       │ ✓ Done    [FG] nightly│
│ [+ Add box]    │                                                       │ Working (3)         ▸ │
└────────────────┴───────────────────────────────────────────────────────┴───────────────────────┘
  sidebar 280        content (tabs 40, pane header 28, gap 8)              context drawer 344
```
The drawer sits beside content (`core.window.context_is_overlay = false` [V field, `src/core.rs:34`]) so the terminal is never covered at this width.

### 7.2 Medium, 648 to 1099 px (half of a large display)
```
┌──────────────────────────────────────────────────────────────┐
│ [≡]   [ST] studio › api-refactor › tests ▾   [◆ 2] [⋯] – □ × │
├───────────────┬──────────────────────────────────────────────┤
│ [ST] studio   │ [ tests ] [◔ server ] [ shell ]         [+]  │
│ ▌◆ api-refac 2│┌ [ST] claude-code ──── ⊡ MacBook 142×38  ◆ ┐ │
│  ◔ billing-mig││                                           │ │
│ [FG] forge    │└───────────────────────────────────────────┘ │
│  ✕ infra      │┌ [ST] zsh ── ⤢ ┐┌ [ST] tail ───────── ⤢ ┐   │
│ [AT] attic ⦸  │└───────────────┘└───────────────────────┘   │
└───────────────┴──────────────────────────────────────────────┘
```
The inbox drawer overlays the content from the right (libcosmic default [V]). The chip shortens to shape and number; its accessible name stays "2 agents need you".

### 7.3 Narrow, under 648 px (tiled third, half of the laptop)
libcosmic sets `is_condensed` when width is under (280 + 8 + 360) × scale and turns the nav bar into a toggled overlay [V `src/core.rs:243-251`].
```
┌──────────────────────────────────────┐
│ [≡] [ST] api-refactor ▾  [◆ 2] – □ × │   breadcrumb collapses to monogram + session
├──────────────────────────────────────┤
│ tests ▾            pane 1 of 3  ‹ ›  │   tabs become a dropdown; one pane at a time
│┌ [ST] claude-code ─ ⊡ MacBook 142×38┐│
││ terminal at the owner's grid,      ││   scaled down to a 9 px floor,
││ then panned to follow the cursor   ││   then horizontal pan
│└────────────────────────────────────┘│
│ ◆ Approval · api-refactor › tests  ▸ │   one-line inbox strip when anything waits
└──────────────────────────────────────┘
```
- "One pane at a time" is local presentation. It does not call the server's zoom, which other devices would see.
- The sidebar and the inbox open as full-height overlays; Escape or the scrim closes them.
- Under 420 px the window controls stay, the overflow menu absorbs everything but the monogram and the chip.

### 7.4 Header bar
`header_start`: nav toggle (libcosmic). `header_center`: breadcrumb button; pressing it opens the quick switcher. `header_end`: attention chip, overflow menu (New tab, Split, New session…, Reconnect box, Settings). No toolbar of icons: commands live in the switcher.

## 8. Box identity

| Where | What | Why there |
| --- | --- | --- |
| Pane header, left | Monogram chip in the box tint, then program or title | Adjacent to the cursor: the last thing seen before typing |
| Header centre | `[ST] studio › api-refactor › tests` | One fixed place to verify the whole path |
| Sidebar | Box row above its sessions | Spatial map |
| Window title | `api-refactor · studio · cosmic-rex` | Alt-Tab, workspace overview, tiling stack titles |

- Monogram: two characters derived from the label, editable, unique among saved boxes (the add flow enforces it). It is text, so identity survives monochrome and colour blindness.
- Tint: six fixed hues chosen to stay apart under common colour-vision deficiencies, used only on the chip and a 2 px rule on top of the pane header. Never used for status. Accent is reserved for "where the keyboard is"; warning, destructive and success for agent state.
- When the keyboard target changes box, the chip in the newly focused pane header holds a 2 px accent outline for 600 ms. No modal, no delay.
- Optional per box, later: "Careful box" adds a hatched band to the pane header and asks before multi-line paste.

## 9. Connection states

| State | Box row | Pane | Input |
| --- | --- | --- | --- |
| Online | Label only | Normal | Sent |
| Slow (round trip over 150 ms, or a sent key unanswered for 2 s) | "slow 240 ms" | Header shows "slow" | Sent; no local echo is faked |
| Connecting | Dashed ring, "Connecting… 3 s" | Skeleton with box identity | Off |
| Resuming (link lost under 60 s; server keeps the client [V protocol]) | Dashed ring, "Resuming…" | Last screen, **diagonal hatch overlay**, plate "Paused · last screen from 14:02:10", cursor hidden, focus ring dashed | Off, not queued |
| Reconnecting | Dashed ring, "Retrying in 8 s · Retry now" | Same as resuming | Off |
| Needs you | Diamond, "Check host key…" or "Sign in…" | Plate with the one action | Off |
| Offline by choice | Slashed circle, "Disconnected" | Sessions hidden behind "Connect" | Off |
| Cannot connect | Cross, reason in words (refused, not admitted, no server at socket, version too old) | Error state (14) | Off |
| Newer server than this client knows | "Watching only" | Normal, banner | Off for replies and closes; typing allowed is [U], default off |

Rules:
- A key pressed while input is off is dropped and answered by a one-line note on the plate, "Not sent: studio is paused" (at most once per 3 s). It is never queued or replayed [V protocol].
- On return: the screen is replaced from the server, the plate says "Back. Screen reloaded. Nothing typed while paused was sent." for 4 s, focus stays where it was.
- On resume from suspend every box goes to "Checking…" before anything is shown as live.
- One banner at most, in the content area for the focused box (`widget::warning` [V]); other boxes speak through their sidebar row.

## 10. Attention: the inbox

- **Item**: one pane's most urgent record, keyed by box, pane and record id.
- **Sections, in order**: Needs you (blocked; longest wait first), Failed, Finished (done and not yet seen), then one collapsed line "Working (n)". Idle is not listed.
- **Count** on the chip and applet = Needs you + Failed. Finished shows as a small check beside the chip, never as a number.
- **Row**: shape and state word ("Approval", "Question", "Sign-in", "Needs input", "Error", "Done"); monogram and `session › tab`; waited time; up to two lines of the program's message behind a quote bar; the program's self-reported name in caption text.
- **Seen**: a Finished or Failed item clears when its pane has had keyboard focus for 1 s or receives a key, or by "Dismiss". A blocked item leaves only when the program's state changes.
- **Offline boxes**: their items stay, dimmed, "as of 14:02", and cannot be answered.
- **Keys**: Ctrl+Shift+A opens and focuses the list; arrows move; Enter opens the answer sheet (blocked) or goes to the pane (others); Shift+Enter always goes to the pane; Ctrl+Shift+G goes straight to the pane that has waited longest without opening anything.
- **Empty**: "Nothing needs you." plus, when no program has ever reported on any box, one line on how agents report status.
- Build: context drawer [V] with a `scrollable` column of `button::custom` cards (as today's `agents.rs`), section labels as `text::caption_heading`.

### 10.1 Answer sheet

Opened only by the operator, from an inbox row. `Application::dialog()` with `widget::dialog` [V], max width 560.

```
┌ Approval request ─────────────────────────────────────────┐
│ [ST] studio › api-refactor › tests                        │  client words: where
│ Program reports itself as "claude-code" · waiting 4 min   │  client words: who claims
│ ┌ The program says ─────────────────────────────────────┐ │
│ ▏ Run cargo test --workspace?                           │ │  program words, quoted
│ └───────────────────────────────────────────────────────┘ │
│ ┌ Screen, last 12 lines · live ─────────────────────────┐ │
│ │ ...                                                   │ │  plain text excerpt
│ │ ❯ 1. Yes   2. Yes, and don't ask again   3. No        │ │
│ └───────────────────────────────────────────────────────┘ │
│ Send key:  [ 1 ] [ 2 ] [ 3 ] [ y ] [ n ] [ Enter ] [ Esc ]│  labelled by what is sent
│ Reply:     [______________________________] [ Send ]      │  paste, then Enter
│ Goes to studio. Sent once; never repeated.                │
│                         [ Cancel ]   [ Open terminal ]    │  default: Open terminal
└───────────────────────────────────────────────────────────┘
```

- **Buttons say what is sent, not what it means.** The client cannot know that "1" means yes. There is no "Approve" button. The screen excerpt is what gives the keys meaning, so the key row is disabled until the excerpt has loaded.
- **Stale guard.** The sheet holds a ticket: box, pane, record id, record revision, connection epoch. Before any send the backend compares the ticket with the current record. On mismatch nothing is sent; the sheet shows "This changed while you were reading" with the new state and re-arms after 300 ms if it is still blocked.
- **Accident guards.** Controls are inert for 300 ms after the sheet opens or its content changes; key repeat is ignored; Enter in the reply field sends only when the field is non-empty; outside the field Enter means Open terminal. Clicking the scrim does nothing; Escape cancels.
- **Outcomes.** "Sent. Waiting for the program…" until its status changes, then the sheet closes with a toast "claude-code is working again". After 5 s without a change: "Sent, but the program has not reacted. Open the terminal to check." No resend from that state.
- **Sign-in requests** have no key row or reply field: "Sign-in is answered in the terminal".
- **Sizing.** The sheet never takes size and never changes the shared active tab; that is its advantage over navigating.
- Until the excerpt exists (`backend-gaps.md` G5) the sheet is the top half plus "Open terminal" only.

## 11. Ownership and size

A pane has one size, the owner's. The size chip in every pane header states it:

| Chip | Meaning | Presentation |
| --- | --- | --- |
| `⤢ Fits here` | This window owns the size | Fills the pane |
| `⊡ Sized by MacBook · 142×38` | Another connected device owns it | Owner's grid, scaled down to a 9 px font floor, then letterboxed or panned; margins in the container colour |
| `⊡ Unclaimed · 120×40` | No owner | As above until the first key press, which claims it in `request` mode (takes only what nobody owns [V docs]) |

- Chip press opens a `popover` [V]: "This pane is sized for MacBook. Fitting it here re-wraps it on MacBook and iPhone." with **Fit to this window** and **Keep their size**. Ctrl+Shift+S does the same for the focused pane. "Give size back" appears when this window owns a pane another device is waiting for.
- Typing never takes size from a connected device. Selection, scrolling and wheel never claim (already so in the core [V]).
- The choice is remembered per session until the owner changes; a setting offers "Always ask / Fit when I type / Never fit".
- Window or tile resizes on a pane this window owns are sent after a 150 ms settle, so dragging a tile edge does not spray resizes at other devices.

## 12. Quick switcher

One overlay, Ctrl+Shift+K (also the breadcrumb, and Ctrl+Shift+P opening it in command mode). `popover` with `modal(true)`, `Position::Center` [V], 560 wide, containing `search_input` [V] and a result column.

```
┌──────────────────────────────────────────────────────┐
│ 🔍 inf|                                              │
├──────────────────────────────────────────────────────┤
│ NEEDS YOU                                            │
│ ◆ [FG] infra › plan           Question · 1 min    ↵  │
│ SESSIONS AND TABS                                    │
│ ✕ [FG] infra                  3 tabs                 │
│ ○ [FG] infra › shell                                 │
│ COMMANDS                                             │
│   New session on…   Reconnect forge   Add box…       │
└──────────────────────────────────────────────────────┘
```
- Empty query: Needs you first, then recent panes (most recent first, current excluded), so Ctrl+Shift+K, Enter is "back to where I was".
- Matching: case-insensitive subsequence over box label, monogram, session, tab, program; a leading `>` limits to commands, `@` to boxes.
- Every result row starts with shape and monogram, so the target box is read before Enter.
- Enter goes; Ctrl+Enter on a blocked row opens its answer sheet. Offline targets are listed dimmed and cannot be chosen.

## 13. Adding a box

A dialog in three steps; the header always shows "Step n of 3".

1. **Where is it?** Four choices, each with a factual status line:
   - *This computer*: "Server socket found" or "No server is running here. cosmic-rex never starts one."
   - *From your other devices*: boxes already saved through the Mac or iOS app, read from the home server's host list [V `hosts.list`]. Fastest path, listed first when available.
   - *Tailscale*: machines from the local `tailscale status` [V rex-rune `tailnet::status`]. Listing is local; a machine is contacted only when chosen.
   - *SSH or address*: `user@host`, or an `https://` address. SSH uses the system's keys and config.
2. **Check.** A read-only check (hello and `server.status` only [V rex-rune `Probe`]) shows: reachable or not and why, server version and whether this client supports it, the account, session count. An unfamiliar SSH host key shows its fingerprint for explicit confirmation; a changed key is refused. Above the continue button: **"Connecting gives this app everything your account can do on this box."**
3. **Name it.** Label, monogram (unique), tint, "Connect when the app starts". Add.

After Add the box appears connected, observing only: sessions and statuses are listed, no session is attached, nothing is created or sized. Removing a box that came from the shared list asks "Remove from this device only / Remove everywhere" and defaults to this device.

## 14. Empty, loading, error, offline

| Situation | Shown |
| --- | --- |
| No boxes | Centred: "Add a box to begin", the four choices from step 1 as cards, one line on what a box is |
| Box connected, no sessions | "No sessions on forge." with **New session on forge…** (explicit creation) |
| Session selected, panes loading | Pane frames from the layout at once, headers filled, bodies blank in the terminal background; "Loading screen…" only after 400 ms. No shimmer |
| Connecting for more than 2 s | Elapsed seconds and the address; after 8 s "Details" with the last error |
| Pane is not a terminal | Header plus "This pane type is not supported yet" |
| Cannot connect | Title in words, cause, what still works, one primary action (Retry, Check host key…, Edit box…) |
| Everything offline | Sidebar as of last contact; content: "No box is reachable. Retrying studio in 8 s, forge in 20 s." |
| Inbox with nothing | "Nothing needs you." |

## 15. Destructive and shared actions

| Class | Examples | Policy |
| --- | --- | --- |
| Local only | Close window, collapse, disconnect a box, local one-pane view | Immediate, no question. Closing the app ends nothing on any box |
| Seen by other devices, reversible | Switch tab, split, new tab, zoom, fit size, rename | Immediate on explicit command; a toast names the box; Undo where it exists |
| Ends a program | Close pane or tab with a running program, close session | Dialog titled with the box: "Close tab 'tests' on studio?", lists what will end, default **Keep running**, destructive button is `button::destructive`, never the Enter default, inert 300 ms. A pane at an idle shell prompt closes at once with an "Undo" toast when the server supports undo (API 20 [V protocol]) |
| Server level | Stop or install a server | Not offered |

Never available from a notification, the applet, or while the box is not online. Creation is always explicit and names its target box before it runs.

## 16. Keyboard

Principles: a focused terminal receives every key except chords with **Ctrl+Shift** or **Ctrl+Alt** (and Ctrl+Tab, which legacy terminals cannot encode). Never plain Ctrl+letter (readline, vim, tmux), never Alt+letter (readline word motion, TUI menus), never Super (the compositor), never bare function keys (htop, mc), never Escape.

| Keys | Action | Collision check |
| --- | --- | --- |
| Ctrl+Shift+K | Quick switcher | Plain Ctrl+K is readline kill-line and nano cut, so the native Cmd+K maps to the shifted form |
| Ctrl+Shift+P | Switcher in command mode | |
| Ctrl+Shift+A | Inbox | Existing binding in `grid.rs` [V] |
| Ctrl+Shift+G | Go to the agent that has waited longest | |
| Ctrl+Alt+1…9 | Box n, last focused pane there | COSMIC defaults bind no Ctrl+Alt chord [V local defaults file]; VT switching uses Ctrl+Alt+F-keys, not digits |
| Ctrl+Shift+[ / ] | Previous / next session in sidebar order | |
| Ctrl+Tab, Ctrl+Shift+Tab | Next / previous tab | Same as cosmic-term [V clone]. Ctrl+PageUp/Down left to vim |
| Ctrl+Shift+1…9 | Tab n | Same as cosmic-term [V] |
| Ctrl+Alt+arrows | Focus pane in that direction | Free in COSMIC defaults [V]; Alt+arrows stay with programs |
| Ctrl+Shift+T / D / E | New tab / split right / split down | Existing [V] |
| Ctrl+Shift+Enter | Zoom pane (shared; toast says so) | Existing [V] |
| Ctrl+Shift+S | Fit pane here / give size back | |
| Ctrl+Shift+W | Close pane (policy in 15) | |
| Ctrl+Shift+C / V / F | Copy / paste / find | Convention |
| Ctrl+Shift+R | Reconnect the focused box now | |
| Ctrl+Shift+B | Sidebar | |
| Ctrl+F6, Ctrl+Shift+F6 | Cycle regions: panes, tabs, sidebar, header, inbox | Bare F6 belongs to htop and mc |
| Ctrl+, | Settings | cosmic-term [V] |

Programs using the kitty keyboard protocol can see Ctrl+Shift chords; all bindings will be rebindable and "Send next key to the terminal" is a switcher command. COSMIC's own Super chords (focus, move, workspaces, Super+Q close [V]) are untouched; Super+Q needs no confirmation because closing ends nothing.

**Focus.** On window activation, focus returns to the last focused pane, never to a chrome control. Inside chrome regions Tab and arrows work as usual; Escape returns to the pane. While focus is in chrome, the pane's focus ring changes from solid accent to a dashed neutral outline and its cursor is hollow: typing will not reach a terminal, and it shows. Overlays trap focus; closing one returns focus to where it came from. Order within the window: pane grid, tab bar, sidebar, header (toggle, breadcrumb, chip, menu), inbox.

## 17. Accessibility

- **Status is shape + word + colour**, in that order of reliance:

| State | Shape (12 px, 1.5 px stroke) | Word | Colour token |
| --- | --- | --- | --- |
| Blocked | Filled diamond ◆ | Approval / Question / Sign-in / Needs input | `warning` |
| Error | Cross ✕ | Error | `destructive` |
| Done, unseen | Check ✓ | Done | `success` |
| Working | Three-quarter ring ◔, static | Working | `on_bg` at 70% |
| Idle | Small hollow circle ○ | Idle | `on_bg` at 45% |
| Link lost | Dashed ring | Paused / Reconnecting | `on_bg` at 70% |
| Disconnected | Slashed circle ⦸ | Disconnected | `on_bg` at 45% |

  Each differs in outline, so the set survives greyscale. Words appear wherever width allows and always in accessible names.
- **Contrast**: text at least 4.5:1, shapes and focus ring at least 3:1 against their container. Use the `*_text_color()` variants on containers and `on_*` on fills [V cosmic-theme]. Unfocused panes are not dimmed (it costs contrast). With `theme.is_high_contrast` [V] chips gain a 1 px border and the focus ring goes to 3 px.
- **Paused panes** use a hatch pattern and a text plate, not a colour shift alone.
- **Reduced motion**: no reduced-motion token was found in libcosmic at this revision [V by search]; provide an app setting and honour the desktop portal value if one exists [U]. When on: all durations zero, no pulse.
- **Screen reader**: enable libcosmic's `a11y` feature (off in `crates/cosmic/Cargo.toml` today because default features are disabled [V]). Buttons take `.name()` and `.description()` [V]. Names are composed by the client: "studio, api-refactor, tests: approval request, waiting 4 minutes". Program text is read after the word "says". New blocked items should be announced once; whether libcosmic exposes a live region is [U]. The terminal grid has no accessible text yet; this is a known gap, stated in Settings.
- **Targets**: at least 32 px high for pointer targets, 40 px in the sidebar and inbox; the size chip and close buttons included.
- **Text**: chrome follows the desktop font and scale. Terminal font size is its own setting (Ctrl+= / Ctrl+-, as cosmic-term [V]).

## 18. Notifications and applet

| Event (only for changes observed live, never for states read on connect [V core `live`]) | Window focused, pane visible | Window focused, pane not visible | Window not focused |
| --- | --- | --- | --- |
| Blocked | Shapes update | Chip count, toast naming the box | Desktop notification |
| Error | Shapes update | Chip count | Desktop notification |
| Done | Shapes update | Check beside chip | Notification if enabled (default off) |
| Box needs you (host key, sign-in) | Banner | Banner | Notification |
| Box lost for over 60 s | Sidebar | Sidebar | None by default |

- Text is the client's: "studio · api-refactor › tests" as title, "An agent is waiting for approval" as body. The program's own message is included only if "Show agent text in notifications" is on (default off: lock screens and screen sharing), escaped and cut at 120 characters.
- One notification per pane per transition; at most one per pane per 30 s; more than three in 10 s collapse into "4 agents need you on 2 boxes".
- The only action is the default one: open the window on that inbox row. No approve, deny or close. Whether COSMIC's notification daemon delivers action clicks is [U]; without them the notification is informative and the applet is the way in.
- A notification is withdrawn when its item leaves the inbox (if the daemon supports closing by id [U]).
- No sounds by default.

**Panel applet** (separate small binary, `cosmic::applet::run`, feature `applet` [V]): the diamond with the count, or a quiet outline when nothing waits; paused boxes add the dashed ring. Popup (`popup_container` [V]): the top of the inbox, each row with monogram and waited time; pressing a row raises the main window on that row; footer "Open cosmic-rex". It reads state from the running app over the session bus and opens no connection of its own, so it adds no client to any box. App not running: "Not running · Open".

## 19. Visual and motion specification

All values come from the active COSMIC theme; numbers below are the Standard-density, Round defaults [V cosmic-theme] and change with the desktop.

| Use | Token | Default |
| --- | --- | --- |
| Hairline gaps, shape to label | `space_xxxs` | 4 |
| Inside chips, between rows | `space_xxs` | 8 |
| Card padding, pane gap (8 in Compact) | `space_xs` | 12 |
| Sidebar and drawer padding | `space_s` | 16 |
| Dialog padding | `space_m` | 24 |
| Empty-state spacing | `space_l` | 32 |
| Monogram chip, key buttons | `radius_xs` | 4 |
| Pane cards, inbox cards, rows | `radius_s` | 8 |
| Dialogs, switcher | `radius_m` | 16 |
| Attention chip, size chip (pills) | `radius_xl` | 160 |

| Text | libcosmic | Size / line |
| --- | --- | --- |
| Empty-state title | `text::title3` | 24 / 36 |
| Dialog title | `text::title4` | 20 / 30 |
| Box label, card title | `text::heading` | 14 / 21 bold |
| Rows, body | `text::body` | 14 / 21 |
| Section labels | `text::caption_heading` | 12 / 17 semibold |
| Paths, times, chips | `text::caption` | 12 / 17 |
| Program text, excerpts | `text::monotext` | 14 / 20 |

Colour roles: window `background`; sidebar and drawer `primary` container; cards and rows `primary` component; pane surround `background`; terminal colours come from the server and are never tinted. Accent only for keyboard location (focus ring 2 px, selected session pill, primary buttons). `warning`, `destructive`, `success` only for agent and link state. Box tints are six fixed values outside the theme, used on 20 × 20 chips and 2 px rules only.

Fixed sizes: sidebar 280 (libcosmic's), drawer 344 (libcosmic's [V]), pane header 28, tab bar 40, status shape 12, monogram chip 20 × 20 with 11 px semibold caps, focus ring 2.

Depth: three planes only. Ground (terminal cards on background), side surfaces (sidebar, drawer), overlays (switcher, dialogs, popovers) with the theme's shadow and a 40% scrim. No blur or translucency: the target is a modest integrated GPU.

Motion:

| What | Duration | Curve |
| --- | --- | --- |
| Hover, press, focus ring | 100 ms | linear |
| Popover, switcher, dialog in/out (opacity, 8 px rise) | 150 ms | `anim::smootherstep` [V] |
| Sidebar and drawer slide | 200 ms (matches libcosmic toggler [V]) | smootherstep |
| Identity outline on box change | 600 ms hold, 150 ms fade | |
| Toast | 5 s (`toaster` Short [V]); 15 s when it carries Undo | |

Terminal content never animates. Nothing loops: the working shape is static, because a spinner would force continuous redraws on every visible row. The window repaints only on a backend wake or input, as today [V `main.rs`].
