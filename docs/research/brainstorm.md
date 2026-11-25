# Rex on Linux: direction brainstorm (2026-11-25)

Companion to `landscape.md` and `protocol.md`. Nothing here was built, run or tested against a server.

Tags: **[V]** verified by me today in a primary source (official docs text, a clone, or a fetched upstream page), **[I]** inferred, **[U]** unknown until tested. "Verified" means "the source says so", never "it works".

## 0. Short version

- The stated goal is *controlling agents remotely*, not *owning a terminal emulator*. Those have very different costs. A terminal client is roughly 90% of the work and the part most certain to be superseded.
- Two paths need no new code and were under-weighted in the docs: install rex-rune's `.deb`, and pipe the vendor's own `rex events --json` into `notify-send`.
- If a gap remains after that, the only thing worth building is a small control-plane-only "agent inbox" (daemon + COSMIC applet). It needs no Ghostty, no Zig and no data plane.
- A new libcosmic terminal client is the worst value per hour of everything listed.

## 1. What "control AI agents remotely" needs

| # | Need | Protocol surface | Already done by |
| --- | --- | --- | --- |
| 1 | Know when an agent is blocked, failed or done, without watching | `program_status_changed` events; agents emit them through Rex's hooks for Claude Code and Codex (OpenCode: skill only, no hooks) [V docs] | Mac app; rex-rune Activity + desktop notifications [V README claim]; rexboard; shrew inbox |
| 2 | See why (question text, recent screen) | `com.superlogical.terminal.format` (text/html/vt) | Any terminal client; `rex capture` [V docs] |
| 3 | Answer: approve, deny, type a reply | paste via `block.event` + `client.key`; `rex send` / `rex send-key` [V docs] | Any terminal client; rexboard |
| 4 | Full terminal when the answer needs judgement | data plane + Ghostty snapshot | rex-rune and five others on Linux; Mac/iOS apps |
| 5 | Start, stop, restart agents | `session.create`, `new_window`, `rex run`, `rex new` | `rex` CLI. rex-rune: "attachment never creates or destroys a session" [V]; whether an explicit create exists there is [U] |
| 6 | Across several hosts at once | one connection per host | rex-rune saved hosts + tailnet discovery [V claim]; rexboard |
| 7 | Away from the desk | push to phone | **Not yet**: vendor lists mobile push and "a unified priority inbox view" as future work [V docs, program-status page] |
| 8 | Not disturbing the Mac/iOS views | ownership rules | See 4.3 |

Observations:

- Needs 1-3 and 6 are control-plane only. Need 4 is the only one that requires terminal emulation, and it is the one with the most existing implementations.
- Need 7 is the one nothing covers today, and it is not a Linux-desktop problem at all.
- The user already holds the best answer to "remote": the iOS app. The Linux gap is narrower than "a Rex client": it is "at my Linux desk, tell me which agent needs me and let me answer fast".

## 2. Directions

Effort is for one person, to "useful daily", not to polish.

### A. Adopt rex-rune as shipped
- **Value**: a full client today; activity list, notifications, SSH and Tailscale hosts, signed `.deb` for glibc 2.39 [V README/docs].
- **Effort**: under an hour. No Zig needed for the binary.
- **Reused**: everything.
- **Risk**: single-maintainer project tracking a proprietary beta at 470 commits a month; untested on COSMIC ("not every desktop, compositor or input method" [V]); open high-RTT input latency issues (#179/#180, per landscape, not rechecked).
- **Kills it**: broken clipboard, IME or scaling on COSMIC; latency over the user's real link; maintainer stops tracking the API.

### B. Zero-code glue on the vendor CLI and Lua
- **What**: `ssh host rex events --json | jq | notify-send`; `rex capture`, `rex send`, `rex send-key`, `rex wait`, `rex hosts`, `-S/REX_SERVER` for remote targets [V docs]. Server-side Lua `rex.on(...)` handlers plus `rex.exec` can call any command, for example a push service [V docs: bell-to-notification example].
- **Value**: alerts and scripted answers in an evening. The vendor maintains the CLI, so **API churn is absorbed for free**.
- **Effort**: hours.
- **Risk**: `rex events` without `-s` covers session-level events; block events (where program status lives) are documented under `--session` [V], so whether one stream covers all sessions is [U]. Running through SSH needs nothing installed locally; a local `rex` means installing the proprietary package on the client.
- **Kills it**: status events not reachable from the CLI stream; unacceptable UX for answering.

### C. Agent inbox: Rust daemon + COSMIC applet (control plane only)
- **Value**: panel badge ("2 blocked"), popover with per-agent state and screen excerpt, guarded quick reply, "open in terminal" handoff to rex-rune. Multi-host.
- **Effort**: 1 week to a CLI/daemon that is useful; 2-3 weeks with applet.
- **Reused**: typed API from rexrs (rex-rune's regenerated API 21 copy, MIT repo); ideas from rexboard and shrew's inbox; or wrap the `rex` CLI and reuse nothing.
- **Risk**: vendor's planned "unified priority inbox" [V]; rex-rune's Activity view may already be enough; API churn (small surface: about 8 methods and 6 events).
- **Kills it**: A+B already satisfy the user; COSMIC notifications cannot carry actions and the applet adds little; the official Linux client ships with an inbox.

### D. Contribute to rex-rune
- **Reality**: no outside PRs [V README]. Path is issues and fork branches that maintainers "harvest, cherry-pick or rewrite" with credit. The harvest record shows a fork's cross-session activity sidebar was **skipped/deferred** on product grounds [V `docs/fork-harvest-2026-11.md`].
- **Value**: COSMIC bug reports are cheap and likely welcome. Feature work has no merge guarantee and no say over design.
- **Kills it**: wanting ownership of direction.

### E. Fork rex-rune (or openrex, rex-linux)
- **Value**: full client you control.
- **Effort**: low to start, permanent to maintain. About 152k lines of own Rust moving daily; a fork diverges within weeks. Needs Zig 0.17 to build [V building.md].
- **Limits**: Rune (wgpu/winit) is not libcosmic, so "COSMIC-native" in a fork means D-Bus-level integration only, which direction C delivers without the fork.
- **Licence**: rex-rune MIT, openrex MIT/Apache: fine. rex-linux and rex-attach have no licence: use only, do not fork or copy.
- **Kills it**: upstream velocity; nothing the user needs requires changing the client's internals.

### F. New libcosmic terminal client
- **Value**: theme-matched native client.
- **Effort**: months. Needs a GPU text grid widget in iced, ghostty-vt render state to that widget, key encoding inputs, selection, IME, the data plane with resume, layout tree, ownership. cosmic-term's widget is built on `alacritty_terminal` [I, from memory], which cannot ingest Ghostty snapshots, so it is not reusable as is.
- **Risk**: superseded by the official client before it reaches parity with rex-rune; Zig toolchain; snapshot format coupling to the server's Ghostty version.
- **Kills it**: almost anything. Only justified as a learning project, and should be labelled as one.

### G. rex-attach inside cosmic-term over SSH
- **Value**: COSMIC theming, clipboard, IME and tiling for free, because the native terminal provides them. Local-socket TUI, so it runs on the server.
- **Risk**: unlicensed, API 10-11; nested-terminal fidelity [U].
- **Role**: fallback terminal if rex-rune misbehaves on COSMIC.

### H. Wait, or settle elsewhere
- Official Linux client exists internally, no date [V docs]. Asking for an ETA and a third-party client policy in the beta Discord costs one message and could collapse this whole document.
- Leaving Rex means giving up the Mac and iOS clients the user values, to solve a Linux-only gap. That trade looks bad unless A fails outright.

### Ranking
1. **A + B together** (this week, no code)
2. **C** (only if a concrete gap survives 1)
3. **G** (fallback terminal)
4. **D** (as a by-product: file COSMIC findings)
5. **E**
6. **F**

Where I disagree with `landscape.md`'s Assessment: "the unfilled niche is COSMIC-native integration" describes a niche, not a need. Most COSMIC integration is either free (tiling, theming of an applet) or unproven to matter. And its option list omits B entirely.

## 3. COSMIC specifics

| Feature | Feasible? | Evidence | Justifies native work? |
| --- | --- | --- | --- |
| Panel applet | Yes | [V libcosmic book]: separate process run with `cosmic::applet::run`, `applet` feature, desktop entry `X-CosmicApplet=true`, popups forwarded to the compositor. Template repo exists. Building against today's libcosmic on cargo 1.98: [U] | **Yes**, the strongest case: a persistent badge |
| Notifications | Basic: yes | cosmic-notifications is a layer-shell daemon [V repo description]. Action buttons, default-action click, body markup handling: **[U]**, nothing found | Only if actions work; otherwise notify-send from B is equal |
| Global shortcuts | Not through the portal | `cosmic.portal` on master lists Access, FileChooser, RemoteDesktop, Screenshot, Settings, ScreenCast: **no GlobalShortcuts, no Notification** [V fetched file, read through a summarising tool] | Workaround: COSMIC custom shortcut that runs a command such as `rexwatch next` [I, unverified on this machine] |
| Tiling | Free | The compositor tiles any toplevel [I] | No |
| Theming | Free for a libcosmic applet [I]; rex-rune has its own palettes | | Weak. Mapping the COSMIC palette to a Rex terminal theme is an idea, not a need |
| Clipboard | Unclear for rex-rune | rex-rune uses `arboard` with `wayland-data-control` [V Cargo.toml]. Whether cosmic-comp exposes data-control by default: [U] (I recall it being opt-in; unverified) | Test in T1 before assuming a problem |
| IME, dead keys | Unclear | rex-rune claims both, tested on "private fixtures" [V]; COSMIC text-input behaviour [U] | Test in T1 |
| Fractional scale, decorations | Unclear | [U] | Test in T1 |
| Tray (StatusNotifierItem) | Probably | COSMIC ships a status-area applet [I, from memory] | A portable alternative to a COSMIC-only applet |

Net: the applet is real and verified in principle. Everything else is either free, unverified, or a test of rex-rune rather than a reason to write a client.

## 4. Architecture sketch for C

### 4.1 Shape
```
rexwatchd (user service)                  cosmic-applet-rexwatch
  host manager ── per host: transport ──┐   badge, popover, reply box
  state model (agents, status, age)     │   talks to daemon over D-Bus
  policy (allowlist, rate limit, guard) │
  D-Bus: org.freedesktop.Notifications ─┘  rexwatch (CLI): list, peek, reply, next
```
- Daemon separate from applet: notifications survive panel restarts, UI is replaceable (tray, launcher plugin, TUI), and all privileged logic sits in one small auditable place.
- Start as one binary with `rexwatch tail`; split when the applet arrives.

### 4.2 Crates
- `tokio`, `tokio-tungstenite` (rustls), `serde`, `serde_json`. WebSocket over a `UnixStream` via `client_async`.
- Typed API: vendor rex-rune's regenerated rexrs (API 21, with generator). Upstream rexrs declares MIT/Apache in `Cargo.toml` but ships no licence file [V]; keep attribution. For the first spike, `serde_json::Value` is enough and removes the dependency.
- `zbus` for notifications (action signals) and the daemon's own interface; `libcosmic` (git) with `applet` for the UI.
- Not needed: ghostty-vt, Zig, wgpu, russh, any data-plane code.
- Alternative backend: shell out to `rex` and parse `--json`. Slower and needs the proprietary binary reachable, but immune to wire changes. Keep the transport behind a trait so both are possible.

### 4.3 Ownership and resize with the Mac and iOS clients
- A client that only calls `session.attach_all` **cannot** own blocks or open the data plane; `own_block` fails with `bad_request` [V official docs]. So the inbox can never resize or re-theme what the Mac or phone is showing. This is a structural guarantee, not a convention.
- Rules a full client would need [V docs]: a block follows its owner's size; `take` wins immediately, `request` only takes unowned blocks and queues; on owner loss the block passes to the most recent claimant, takes before requests; a dropped owner gets blocks back on reconnect unless ownership changed.
- Handoff etiquette for "open in terminal": let rex-rune own while focused. Check in T1 what it does to the Mac view and whether it gives ownership back.
- Screen excerpts: `terminal.format` is a block method; whether it works without `session.attach` is [U]. If attach is required, attach without claiming.
- `session.view` records focus history, `session.attach` does not (API 19-21, third-party observation). A monitor must avoid polluting the shared focus state.

### 4.4 Connection and reconnect
- Hello with kind `cli` or `agent`, not `app`: kinds are app, cli, agent, other [V help.json], and API 17+ routes script prompts to `app` clients, which would stall scripts if unanswered [I from protocol.md].
- Pin `api_min_revision`; on an unknown higher revision keep watching but disable replies.
- Abrupt loss: resume with the token inside 60 s. Otherwise, and after every resume, **rebuild state from scratch** (`session.list`, status per block), since gap events are lost.
- Never replay input. A reply that did not get a response is reported as "not confirmed", not retried.
- Backoff with jitter; reconnect on resume from suspend [I].

### 4.5 SSH or Tailscale
- SSH: spawn the system `ssh -N -L <local.sock>:<remote.sock>` with `ExitOnForwardFailure` and keepalives. Reuses the user's keys, agent and config; identity is the SSH login; nothing new listens. Vendor's current recommendation for personal servers [V].
- Tailscale: plain HTTPS/WSS to `rex.<tailnet>.ts.net` through the system tailscaled; no embedded tsnet. Identity is the tailnet login.
- Support both behind one endpoint type; default to SSH. Never offer plain HTTP with `--allow-anonymous`.

### 4.6 Safety
One connection can do everything the server account can, and status text comes from whatever runs in the terminal.
- **Method allowlist in the daemon**: observe, list, read status, format, paste, key. No create, close, kill, upload or `client.perform` unless the user turns each on.
- **Stale-reply guard**: a reply is bound to (host, block, status `id`, state). Re-read status immediately before sending; if it changed, refuse and show the new state. Otherwise "y" can land in a different prompt than the one displayed.
- **No one-click approve from a notification** by default. Notification actions are "Open" and "Dismiss". Approval happens in the popover, after showing the screen excerpt and the source (host, session, tab, program).
- **Untrusted text**: plain text only; strip control and bidi characters; escape `<` and `&` (notification bodies may be parsed as markup); truncate; never linkify; never use status text to choose an icon, sound or command.
- **Rate-limit and dedupe** per block; collapse storms into one summary.
- Screen excerpts are also untrusted: request `text`, not `html` or `vt`.
- Logs hold ids and states, not prompt contents.

## 5. Ideas to prototype, separate from the core

1. **Phone push now**: a server-side Lua handler on program-status events calling `rex.exec` with a push tool. Fills need 7 until the vendor ships push. Shelf life is short by design.
2. **Preview without Ghostty**: `terminal.format` with `vt` output fed to a plain VT parser for a static excerpt. If it works, the "must decode Ghostty snapshots" rule applies only to live terminals [I].
3. **Launcher instead of applet**: a command for COSMIC's launcher ("rx" lists blocked agents) if it supports external plugins [U].
4. **Attention metrics**: time blocked per agent, a daily "minutes agents waited on me" figure.
5. **API drift alarm**: save each server's `/api/help` per build and diff on connect.
6. **Supervisor hook**: expose the inbox over MCP so one agent can triage others. rexctl does this in Python under GPL-3.0: reference only.
7. **Window-per-block client**: each Rex block as a native toplevel, tiled by COSMIC. Conflicts with server-owned layout; interesting, probably wrong.
8. **Upstream goodwill**: publish the T1 COSMIC findings as rex-rune issues.

## 6. Recommendation

Do A and B this week. Build C only if a specific, named gap survives. Do not start F.

### First week: kill-tests, ordered by what they falsify

| # | Test | Cost | Falsifies |
| --- | --- | --- | --- |
| T0 | Ask in the beta Discord: Linux client ETA, third-party client policy, inbox/push timing | 5 min | The whole project |
| T1 | Install rex-rune `.deb`; connect over SSH and over Tailscale. Check typing latency on the real link, copy/paste, dead keys/IME, fractional scale, tiling, notifications, and what happens to the Mac/iOS view | 1 h | "I need to build a terminal client" |
| T2 | `ssh host rex events --json` (with and without `-s`, with `--all`) while an agent blocks; pipe to `notify-send` | 1 h | "Alerts need protocol code"; also whether status events are reachable from the CLI |
| T3 | `notify-send -A` action round trip on COSMIC; a custom shortcut bound to a script | 30 min | The value of native notifications and shortcuts |
| T4 | Live two days on A+B. Write down each moment it was not enough | 2 days | The need for C. **Decision gate** |
| T5 | Build the applet template; show a badge from a fake JSON file | half day | Applet feasibility and build pain on this machine |
| T6 | Rust spike: hello, `attach_all`, print status events, over `ssh -L` | 1 day | protocol.md's control notes at the server's real API revision, including the unresolved `msg`/`title` event question |
| T7 | Guarded reply from the CLI | half day | The quick-answer UX and the stale guard |

Stop conditions: T0 gives a near date; T4 produces no list; T5 or T6 costs more than twice the estimate.

### Decisions only the user can make
1. Is the aim a daily-driver terminal on Linux, or fast oversight and answering? This picks between "adopt" and "build C".
2. Is depending on a single-maintainer third-party client acceptable for daily work?
3. Tool or learning project? Being superseded matters for the first and not for the second.
4. Install the proprietary `rex` CLI on the Linux desktop, or only reach it through SSH?
5. SSH forward, Tailscale listener, or both, and which hosts.
6. May a desktop notification ever approve an agent action, or must approval always follow a look at the screen?
7. Which agents: Claude Code and Codex report status through hooks; OpenCode does not.
8. Publish as open source (licence choice), or keep private.
9. What result would make "settle on another project" the answer, given it costs the Mac and iOS clients.

## 7. Corrections and gaps in the existing docs

1. **landscape.md Assessment omits the zero-code path.** The official `rex` CLI has `events --json`, `wait`, `send`, `send-key`, `capture`, `hosts` and `-S/REX_SERVER`, and server Lua has event handlers plus `rex.exec` [V docs]. This is the cheapest route to option 4's value and is insulated from API churn.
2. **Supersession risk is understated.** The vendor's program-status page names "push notifying our mobile applications, a unified priority inbox view" as planned [V]. So the inbox direction is exposed too, not only a terminal client.
3. **"Must decode Ghostty snapshots" is too broad.** True for a live terminal on the data plane. `terminal.format` (protocol.md section 6) returns screen content on the control plane, so previews and an inbox need no Ghostty or Zig.
4. **protocol.md hello example uses `kind: app` without comment.** Kinds are app, cli, agent, other [V help.json]. A monitor should not claim `app`, given the API 17 prompt routing the same file describes.
5. **protocol.md section 3 under-describes ownership.** Missing: the claim queue (take ahead of request, most recent first), return of blocks on reconnect, and that owning "gives no other rights" [V docs]. These decide how a Linux client coexists with the Mac and phone.
6. **"SSH, vendor-recommended today" needs scoping.** True for personal servers; for the login server the docs recommend "something like Tailscale" [V].
7. **rexboard "uses the `rex` CLI"**: a quick grep of the clone did not confirm it. Unverified, not shown wrong.
8. **rex-rune "Activity list"** is the project's claim; its own harvest record shows a cross-session activity sidebar was deferred. What the shipped Activity view covers across unopened sessions needs T1.

## 8. Reviewer follow-up (main session, same day)

- Items 1-6 of section 7 were folded into `landscape.md` / `protocol.md`.
- Item 7 resolved: rexboard does use the `rex` CLI (`src/rex.rs:87`: "no rex CLI in REXBOARD_REX, REX_BIN_DIR, PATH, or the Rex app bundle").
- Direction B: the docs say `rex events --all` follows "every server event", so one stream per host probably carries status events. T2 still has to confirm it. The docs also recommend Lua event handlers over pipelines.
- "Single maintainer" is close but not exact: rex-rune's log has six names including dependabot; `shuv1337` + `shuv` (apparently one person) have 426 of 473 commits.
