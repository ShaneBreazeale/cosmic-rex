# Rex client landscape (2026-11-25)

Target machine: Pop!_OS 24.04 (noble, glibc 2.39), COSMIC on Wayland, cargo 1.98, Tailscale present. Zig is **not** installed.

Evidence: **[R]** read README/docs, **[C]** read or measured the clone, **[V]** vendor statement. **Nothing here has been built or run by us**; feature lists are the projects' own claims.

## Official status

- Rex (Superlogical) is proprietary beta software. Servers: macOS, Linux, FreeBSD, Windows. Apps: macOS to all testers; iOS and Windows to enrolled testers.
- "We have a Rex client for Linux that isn't part of testing yet." [V, /rex/docs/installation] No date found. Any client we write is potentially superseded.
- Linux install commands on the docs site are shown after a Discord beta login. We did not bypass that or run a server.
- The wire API is not on the docs site, but each server publishes it (`/llms.txt`, `/api/help`). It is beta and moves fast: third-party captures show API 7 (2026-10-30/31), 13 (early Nov), 21 (2026-11-24).
- No stability promise or third-party client policy was found.

## Linux-capable clients

| Project | Stack | Licence | Own Rust LOC | API | Last commit | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| [shuv1337/rex-rune](https://github.com/shuv1337/rex-rune) | Rust, Rune (wgpu/winit), ghostty-vt, rexrs | MIT | ~152k in src/ (550k with vendored) | 10-21 | 2026-11-25 | 94 stars, 23 forks, 473 commits since 2026-11-01, signed releases, v0.12.0 ships x86_64 `.deb`, needs glibc 2.39 [R,C]. Activity list, desktop notifications, SSH + Tailscale discovery. **No outside PRs**; issues/fork branches get harvested. No COSMIC testing documented. Opens: #179/#180 high-RTT input latency. |
| [willothy/rex-linux](https://github.com/willothy/rex-linux) | Rust, GPUI 0.2.2, libghostty-rs | **none found** | 11k | 15 | 2026-11-15 | Mac-like layout, multi-host, client.keymap [R] |
| [Goooooooooody/openrex](https://github.com/Goooooooooody/openrex) | Rust, gpui, ghostty-vt 0.8 | MIT/Apache-2.0 | 18k | 13 | 2026-11-13 | Very broad feature claims; explicitly invites forks; looks dormant since a single burst |
| [Adityabhaskar685/rex-gpui](https://github.com/Adityabhaskar685/rex-gpui) | Rust, GPUI | MIT | 9k | 7 | 2026-11-18 | Headless `--check` mode |
| [moejay/rex-client-linux-temp](https://github.com/moejay/rex-client-linux-temp) | Rust, GTK4 | MIT | 14k | 13 | 2026-11-18 | Alpha; says it will be archived when official client ships; no copy/IME/reconnect |
| [teddytennant/shrew](https://github.com/teddytennant/shrew) | Rust core, GTK4/libadwaita Linux app, UniFFI mobile | MIT | ~8k core crates + app | 8 | 2026-11-03 | UI-agnostic proto/client/term/core split; "needs you" inbox |
| [Unayung/orex](https://github.com/Unayung/orex) | C++ | custom/unclear | n/a | - | 2026-11-19 | Omarchy-oriented |
| [nivox/rex-attach](https://github.com/nivox/rex-attach) | Rust TUI (ratatui) | **none found** | 11k | 10-11 | 2026-11-24 | Local Unix socket only; run over SSH |

No libcosmic/iced client found (searches: "rex cosmic client terminal", "libcosmic rex", and COSMIC greps of the clones). GitHub search is not exhaustive.

Not Linux desktop, but useful references: joshgermon/windex (WinUI + Rust, best protocol writeup), maosuyun2009/flex (Windows GPUI on shrew-core), Nam0101/rexc (Android, Rust core), leizhiyuan/rexpocket (Android), mLupine/rexctl (Python CLI/MCP, GPL-3.0: do not copy code into MIT work).

Agent-oversight layers: vanducng/rexboard (Rust TUI, cross-host attention board and answer, MIT, 50k LOC, uses the `rex` CLI), captjt/rex-inbox (Rust hub + Mac app, MIT, requires macOS).

## Reusable Rust building blocks

| Need | Candidate | Caveat |
| --- | --- | --- |
| Typed control API | [a10y/rexrs](https://github.com/a10y/rexrs) (MIT/Apache-2.0 declared, no licence files; 2 commits), or rex-rune's vendored copy regenerated at API 21 with generator and tests | Upstream stale since 2026-10-29; control only, no data plane |
| Data plane/reconnect | shrew-proto + shrew-client (MIT, ~1.5k LOC, API 8); openrex-proto (MIT/Apache, ~1.5k LOC, API 13) | Need checks against API 21 |
| Terminal state | `ghostty-vt` 0.8.0 (used by rex-rune, openrex, rex-attach) or libghostty-rs (shrew, rex-linux) | Live terminals must decode Ghostty snapshots; needs Zig 0.17 to build. alacritty_terminal/vte cannot decode a snapshot. Screen previews do not need this: `terminal.format` works over the control channel. |
| OSC 7501 | nlopes/osc7501-rs, buildkite/osc7501 | Client normally needs no parser: the server emits normalized events |

## Assessment

Supersession risk covers inboxes too: the vendor's program-status page lists mobile push and "a unified priority inbox view" as future plans.

The premise "no native Linux client exists" holds for official, publicly testable releases only. rex-rune already covers the core need and publishes a `.deb` for this distro's glibc. Options:

1. Use rex-rune now; contribute issues/forks for gaps.
2. Fork rex-rune or openrex for COSMIC integration.
3. Build a new libcosmic client on rexrs + shrew/openrex data plane + ghostty-vt. The unfilled niche is COSMIC-native integration.
4. Build only an agent-control layer (COSMIC applet/inbox) over control-plane events, using rex-rune for terminals. Needs no terminal emulation.
5. No new client code: the official CLI (`rex events --json`, `wait`, `capture`, `send`, `send-key`, `-S`) over SSH into `notify-send`, plus server-side Lua `rex.on` / `rex.exec`. Cheapest; the vendor absorbs API churn.

See `brainstorm.md` for the ranking and tests.
