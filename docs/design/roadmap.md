# cosmic-rex roadmap (2026-11-25)

Small slices, each shippable alone, ordered by risk removed per unit of work. References: `ux-spec.md` (sections), `backend-gaps.md` (G numbers).

Every slice ends with the same gate: `--demo` fixtures extended to show the new states without a server, the hazard table in `ux-spec.md` 5 re-read, and the usability check below done and written down in two lines. The checks are sized for one developer: a self-test with a fixture, or a five-minute test with one other person.

Standing rule for all slices: no slice may make attaching create, stop or resize anything.

## Slice 1. Several boxes

Makes more than one machine usable. Backend: G2.

- Saved boxes in a config file: label, monogram, tint, endpoint, connect on start. `--endpoint` keeps working as a one-off box.
- Sidebar becomes boxes containing sessions (custom `nav_bar()`), with link state per box as shape and word. Boxes connect in the background and observe; only the session on screen gets terminals.
- Identity: monogram chip in the header breadcrumb, in each pane header, and the window title.
- "Add box" as a plain form (address or `user@host`, label, monogram, tint) and "Remove".
- Ctrl+Alt+1…9 selects a box.

Accept when:
- Two boxes are listed and connect independently; one failing does not delay the other.
- Selecting a session on box B never sends anything to box A. A test asserts keys are addressed by box and pane.
- With one box unreachable its row says so in words and its sessions cannot be opened.
- Sizing behaviour is no worse than today, and the release note states the known hazard G1.

Usability check: show a screenshot for 2 s to one person (or yourself after a break) and ask "which machine receives typing?". Pass at 5 of 5 across five fixtures, including two boxes with similar session names.

## Slice 2. Truthful panes

Backend: G1, G3. This removes the two silent hazards (H5, H6).

- Size policy `Polite` as default: typing does not take size from a connected device.
- Size chip in the pane header; owner's grid drawn scaled or letterboxed; "Fit to this window" and "Keep their size" popover; Ctrl+Shift+S.
- Paused treatment: hatch overlay, plate with time, hidden cursor, input off with the "not sent" note; "screen reloaded" on return.
- Typed `Link` and `PaneLink` replace status prose in the view code.
- Re-check all boxes after resume from suspend.

Accept when:
- With a second client owning a pane, typing 100 characters here causes zero `own_block` calls (asserted on a fake server or the core's test fixtures).
- Pulling the network shows the paused state within 2 s of the first unanswered request, and keys pressed meanwhile never arrive after reconnect.
- "Fit" re-wraps once, after the popover, and the chip then reads "Fits here".

Usability check: greyscale screenshot of four panes (live mine, live theirs, paused, loading). A viewer labels all four correctly without colour. Then the lid test: suspend for two minutes, open, and note whether anything looked live before it was.

If the core change for G1 is refused upstream and a carried patch is unwanted, ship the chip and overlay anyway and put a one-time warning on first key press into a pane owned by another device.

## Slice 3. Inbox across boxes

Backend: G4.

- Drawer lists Needs you, Failed, Finished, and "Working (n)" for all boxes; rows carry monogram, place, waited time and quoted program text.
- Chip in the header with the count; shapes on session and box rows, including collapsed boxes.
- Seen and dismiss rules. Ctrl+Shift+A opens; Ctrl+Shift+G goes to the longest-waiting pane.
- Drawer beside the content at wide widths, over it below 1100 px.

Accept when:
- A blocked agent on a box whose session is not open appears within one status event, on the right box.
- Order is stable under updates: a row does not move while the pointer or selection is on it.
- Items from a paused box are dimmed, dated and not actionable.
- A hostile status string (control characters, direction overrides, 2 KB) renders as bounded plain text inside the quote frame (fixture).

Usability check: with a fixture of seven agents on three boxes, time "find and reach the agent that has waited longest" from a cold window. Target: under 3 s with one chord. Then ask what the quoted line is; the answer should be "what the program said", not "what the app says".

## Slice 4. Quick switcher

Frontend only.

- Ctrl+Shift+K overlay: needs-you rows, recent panes, sessions, tabs, boxes, a few commands. Breadcrumb opens it.
- Recent-pane history kept locally.

Accept when: any session on any connected box is reachable in at most four keystrokes after the chord; Ctrl+Shift+K then Enter returns to the previous pane; no key typed in the switcher reaches a terminal (test on the grid's input path).

Usability check: ten scripted jumps among three boxes; count wrong-target arrivals. Target zero, with no look at the sidebar.

## Slice 5. Answer in place

Backend: G5. Depends on slices 2 and 3.

- Answer sheet: source lines, quoted program text, screen excerpt, key row, reply field, default "Open terminal".
- Ticket check before every send; inert period; outcomes confirmed, unconfirmed, stale, not sent.
- Sign-in requests offer only "Open terminal".

Accept when:
- If the record changes between open and send (fixture flips it at 200 ms), nothing is sent and the sheet says what changed.
- A held Enter sends at most once. A dropped link during send reports "not confirmed" and nothing is sent on reconnect.
- Answering does not change the shared active tab and makes no ownership call.

Usability check: stale-prompt drill. A fixture replaces prompt A with prompt B while the sheet is open, ten times at random delays. Count answers that reach B. Target zero. Then a think-aloud pass on the key row: the viewer should say the buttons "send 1" or "send y", not "approve".

Stop and reconsider if `terminal.format` or unattached sends prove unavailable: ship the sheet as a preview with "Open terminal" and move on.

## Slice 6. Add-box flow

Backend: G6.

- Three-step dialog: source (this computer, other devices' list, Tailscale, SSH or address), read-only check, naming.
- Host-key confirmation with fingerprint; changed key refused; the full-control sentence before continuing.
- Shared list: adopt a box saved on the Mac; removal asks this device or everywhere.

Accept when: a box can be added by each of the four routes without the terminal; no machine is contacted before it is chosen; a duplicate endpoint or monogram is refused with the reason; cancel at any step leaves no trace.

Usability check: cognitive walkthrough of each route against four questions (will they know what to do, see how, connect the control with the goal, see progress). Then ask after step 2: "what can this app do on that machine once you continue?"

## Slice 7. Notifications

Backend: G8.

- Desktop notifications for blocked, error, box needs you, with client-written text, per the table in `ux-spec.md` 18. Rate limit and storm collapse. Withdraw when resolved if the daemon allows.
- Settings: per-state on or off, "show agent text", none while the pane is visible.

Accept when: one transition gives one notification; ten agents blocking within 5 s give one summary; program text is absent by default; nothing is notified for states read on connect.

Usability check: a working day with it on. Tally notifications that were useful, redundant or missed. Change defaults if redundant exceeds useful. Also try the default action on COSMIC and record whether clicks arrive ([U] today).

## Slice 8. Narrow and tiled

Frontend only.

- Under 648 px: overlay sidebar, monogram-and-session breadcrumb, tab dropdown, one pane at a time (local), inbox strip.
- Panes not sized by this window: scale to the 9 px floor, then pan with the cursor.
- 150 ms settle before sending a new size for panes this window owns.

Accept when: at 455 px and 640 px nothing clips or overlaps, identity and the count stay visible, and dragging a tile edge for 3 s sends at most one resize after release.

Usability check: tile the window to a third beside a browser and repeat the slice 1 identity test and the slice 3 timing. Results should not get worse by more than half.

## Slice 9. Closing and creating safely

Backend: G7.

- Close pane, tab, session with the dialog from `ux-spec.md` 15; undo toast for idle shells; new session with the target box named; rename.
- All of these disabled, with the reason, when the box is not online.

Accept when: no path closes a running program in fewer than two deliberate actions; Enter on the dialog keeps it running; the dialog title carries the box label; undo restores the pane on the same connection only.

Usability check: slip test. Ask for "close the shell tab on forge" while focus is on studio, five times. Count wrong-box closes confirmed. Target zero.

## Slice 10. Accessibility pass

- Enable libcosmic `a11y`; names and descriptions for every control; announce new blocked items if a live region exists.
- High-contrast adjustments; reduced-motion setting; keyboard-only run through every flow with Ctrl+F6 region cycling.

Accept when: every flow in slices 1 to 9 completes without a pointer; every shape has a word in its accessible name; contrast measured at 4.5:1 for text and 3:1 for shapes in COSMIC light, dark and both high-contrast themes.

Usability check: fifteen minutes with Orca on: add a box, find a blocked agent, open its answer sheet. Write down what was spoken at each step and fix the three worst.

## Slice 11. Panel applet

- Separate binary with count and shape; popup with the top of the inbox; raises the window on a row. Reads the app over the session bus; opens no connection.

Accept when: the applet adds no client to any box (checked with the server's client list); count matches the chip within one second; with the app closed it says so.

Usability check: a week of context A. Note each time the count was noticed late or the popup was opened only to be closed; decide whether the applet stays.

## Slice 12. Shared active tab, follow when idle

Backend: G9. Last because it depends on the least certain core behaviour.

Accept when: a tab switch from another device during typing never redirects a key press; when idle the window follows within one event and offers "Go back".

Usability check: with the phone app, switch tabs while typing a long command here, ten times. Count characters that reached the wrong pane. Target zero.

## Later, unordered

Per-box "careful" mode and multi-line paste confirmation, copy mode and find, rebinding UI, mute and mark-unread per tab, port forwarding, file drop, floating layers.

## Checks that can end or reshape the plan

- The vendor ships its Linux client or the unified inbox it has announced (`brainstorm.md`): compare before building slices 5, 7 and 11.
- G1 cannot be solved: the client stays a hazard to the Mac view and should say so on every launch until it is.
- Draw time with three boxes observing and one session open exceeds a frame on the target laptop: reduce background work before adding surface.
