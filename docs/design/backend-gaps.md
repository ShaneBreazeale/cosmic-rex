# What rex-backend must expose for the design (2026-11-25)

Reference: `ux-spec.md`. Today `rex-backend` wraps one `Host` and a `Snapshot` (sessions, tabs, panes, agents). The design needs several boxes, truthful link and size state, and guarded replies.

Marks:
- **[core]** rex-rune at `bd77ede` already has it; the work is a wrapper in `crates/backend`. Verified by reading the named item, not by running it.
- **[core?]** the core appears to have it; behaviour not confirmed.
- **[new]** not in the core; rex-backend must add it (often through the core's raw `HostCommand::Request`, whose result handling is [U]).
- **[patch]** needs a change inside the core (upstream request or a carried patch).

Rules kept from the current crate: no rex-rune type in the public API; all program text passes through `plain`; nothing here creates, stops or resizes unless the method name says so.

## Priority order

| # | Gap | Hazard it closes | Mark |
| --- | --- | --- | --- |
| G1 | Input without taking size; explicit fit and give back | H5 resizing the Mac | **[patch]** |
| G2 | Several boxes: store, registry, per-box link state | T5, H1, H6 | [core] + [new] |
| G3 | Pane truth: live or not, owner, grid size | H5, H6 | [core] + [new] |
| G4 | Inbox across boxes with stable keys, revisions, ages, seen | H2, H3 | [core] + [new] |
| G5 | Guarded reply and screen excerpt | H2, H8 | [new] |
| G6 | Add-box: check, Tailscale list, SSH questions, shared host list | T11 | [core] |
| G7 | Explicit create, close, undo, rename | T8, T10 | [core] |
| G8 | Events for notifications; applet feed | T1 | [core] + [new] |
| G9 | Local tab pinning (follow when idle) | H1 | [core?] / [patch] |

## G1. Size ownership (do this before more people-facing work)

> **Update (main session, same day): solved without a core patch.** `Host::type_into` in `crates/backend/src/host.rs` now sends all input on our own pipelined, observe-only connection (`input.rs`), which cannot claim. An unclaimed pane is asked for with `request` on first key. Verified live against API 18 (2026-11-25), as is `attach_all` being enough to type, so in-place answers (G5) are possible. `disown_block` ("give size back") is still missing.

Finding [V `src/connection.rs` near line 3444]: for every key press, paste and mouse press on a pane this client does not own, the core first sends `session.own_block` with `mode: "take"`. So today the first keystroke in cosmic-rex re-wraps the pane on the Mac and phone. `Host::resize` is then honoured because this client is the owner.

Needed:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizePolicy {
    /// Typing never claims a pane that a connected client owns; an unowned pane is claimed
    /// with `request`. Default.
    Polite,
    /// The core's behaviour today: the first key press takes.
    TakeOnInput,
    /// Never claim, even when unowned.
    Never,
}

impl Host {
    pub fn set_size_policy(&self, policy: SizePolicy);
    /// `session.own_block` mode `take`, then the pending resize. Explicit user act only.
    pub fn fit_here(&self, session: &str, block: &str);
    /// `session.disown_block`.
    pub fn give_size_back(&self, session: &str, block: &str);
}
```

- **[patch]** the automatic claim needs a switch in the core's input path (one condition on the `!owner && !passive` branch) and a `request` mode. `Command::TakeControl` [core] already gives an explicit take per pane; `HostCommand::Claim` [core] takes for a list of panes.
- **[new]** `session.disown_block` is not called anywhere in the core [V by search].
- Until the patch lands, the frontend can only warn; it cannot type without taking. That is why this is first.

## G2. Boxes

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BoxId(pub String);            // local, stable, never an endpoint

#[derive(Clone, Debug, PartialEq)]
pub enum Endpoint {
    Local { socket: std::path::PathBuf },
    Ssh { user: Option<String>, host: String, port: Option<u16>, socket: Option<String> },
    Url(String),                         // https:// or wss:// only; plain http is refused
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoxConfig {
    pub id: BoxId,
    pub label: String,                   // unique, case-insensitive
    pub monogram: String,                // two characters, unique
    pub tint: u8,                        // 0..6, index into the frontend's identity palette
    pub endpoint: Endpoint,
    pub connect_on_start: bool,
    pub shared_id: Option<String>,       // id in the home server's host list, if linked
}

pub struct Boxes { /* owns one Host per connected box */ }

impl Boxes {
    pub fn load(path: &std::path::Path) -> Result<Self, StoreError>;
    pub fn configs(&self) -> Vec<BoxConfig>;
    pub fn add(&mut self, new: BoxConfig) -> Result<(), StoreError>;      // validates uniqueness
    pub fn update(&mut self, config: BoxConfig) -> Result<(), StoreError>;
    pub fn remove(&mut self, id: &BoxId) -> Result<(), StoreError>;       // local only
    pub fn reorder(&mut self, order: &[BoxId]) -> Result<(), StoreError>;
    pub fn connect(&mut self, id: &BoxId);      // observe: session list and statuses only
    pub fn disconnect(&mut self, id: &BoxId);   // clean close, so the server drops us at once
    pub fn retry_now(&self, id: &BoxId);
    pub fn host(&self, id: &BoxId) -> Option<&Host>;
    pub fn overview(&self) -> Overview;         // cheap; no terminal frames
    pub fn wakes(&self) -> Vec<Wake>;
    /// Call on resume from suspend: every link is treated as unknown until it answers.
    pub fn recheck_all(&self);
}

pub struct Overview {
    pub boxes: Vec<BoxSummary>,
    pub inbox: Vec<Attention>,                  // see G4
}

pub struct BoxSummary {
    pub id: BoxId,
    pub link: Link,
    pub sessions: Vec<Session>,                 // last known when not online
    pub as_of: Option<std::time::SystemTime>,   // None while online
    pub server: Option<ServerFacts>,
}

pub struct ServerFacts { pub version: String, pub api: u32, pub account: Option<String>, pub replies_allowed: bool }

#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Off,
    Connecting { since: std::time::Instant },
    Online { round_trip: Option<std::time::Duration> },
    /// Inside the server's 60 s keep window; the same client id will come back.
    Resuming { since: std::time::Instant },
    Reconnecting { attempt: u32, next_try: std::time::Instant, why: LinkError },
    NeedsYou(LinkQuestion),
    Failed(LinkError),
}

pub enum LinkError { Refused, NotAdmitted, NoServer, TooOld { server: u32, min: u32 }, Tls, Timeout, Other(String) }
pub enum LinkQuestion { UnknownHostKey { fingerprint: String }, ChangedHostKey, SshAuth }
```

- [core] one `HostConnection` per endpoint with background reconnect; `HostModel.connected`, `status`, `notice`, `connection_epoch`, `ssh_problem` (`ssh::Problem::{NoServer, UnknownKey, ChangedKey, AuthFailed}`), `server_info`, `protocol.api_revision`; `HostCommand::Reconnect`; `settings::validate_host`; `visible: bool` on spawn and `HostCommand::Visibility`.
- [core?] connecting without attaching a session: `spawn(endpoint, None, None, None, false, None)` plus the core's status sweep. What it attaches by default on first connect needs reading or a trial.
- [new] the store (the core's `settings::Settings` carries rex-rune's whole UI settings; keep our own small file under the COSMIC config directory), `BoxId`, monogram and tint, typed `Link` (the core exposes a status string and a bool; `Resuming` versus `Reconnecting`, attempt count and next retry time are internal to its worker), round-trip time, last-known sessions with `as_of`, `recheck_all`.
- `Session` gains `box_id: BoxId` and `needing: u8` (panes blocked or failed).

## G3. Pane truth

```rust
pub struct PaneView {
    pub block: String,
    pub rect: [f64; 4],
    pub frame: Option<Frame>,
    pub focused: bool,
    pub title: String,                 // plain, bounded
    pub live: PaneLink,
    pub size: PaneSize,
    pub state: AgentState,             // for the header shape
}

pub enum PaneLink { Live, Loading, Paused { since: std::time::SystemTime }, Closed, Unsupported }

pub struct PaneSize {
    pub owner: Owner,
    pub cols: u16,
    pub rows: u16,
    /// What this window would ask for; lets the UI say "142×38 → 96×30".
    pub wanted: Option<(u16, u16)>,
}

pub enum Owner { Me, Other { client: String, label: String, kind: String }, Nobody }

impl Host {
    /// Devices attached to a session, for "who else is looking".
    pub fn clients(&self, session: &str) -> Vec<ClientInfo>;
}
pub struct ClientInfo { pub id: String, pub label: String, pub kind: String, pub platform: String }
```

- [core] `terminal::Model.connected`, `control_connected`, `owner: bool`, `status`, `notice`; the session view carries each block's `owner` client id; `HostModel.titles`; frame `cols`/`rows`.
- [new] owner label needs `client.list` or `session.list_clients` (neither is called by the core [V by search]); `Paused.since`; `wanted`.
- Replace `PaneView.status: String` with `PaneLink`: the frontend must not parse prose to decide whether to paint the paused overlay.

## G4. Inbox

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AttentionKey { pub box_id: BoxId, pub block: String, pub record: String }

pub struct Attention {
    pub key: AttentionKey,
    /// Bumped by the backend whenever this record's state, kind or text changes.
    pub revision: u64,
    pub epoch: u64,                    // connection epoch it was observed on
    pub session: String,
    pub window: String,
    pub state: AgentState,
    pub waiting_for: Option<WaitingFor>,
    pub app: String,                   // program's claim; plain, 32
    pub message: Option<String>,       // program's words; plain, 200
    pub place: Place,                  // client-composed labels, each plain
    pub since: Option<std::time::SystemTime>,   // None when read back, not watched
    pub progress: Option<u8>,
    pub observed_live: bool,
    pub seen: bool,
    pub stale: bool,                   // box not online: last known
}
pub struct Place { pub box_label: String, pub monogram: String, pub session: String, pub tab: String }

impl Boxes {
    pub fn mark_seen(&self, key: &AttentionKey);
    pub fn dismiss(&self, key: &AttentionKey);      // done and error only
}
```

- [core] `HostModel.activity` (block → record id → `StatusRecord` with `state`, `kind`, `app`, `title`, `msg`, `progress`, `live`, `age`), status sweep for unopened sessions, urgency order (already used by `model.rs`).
- [new] cross-box merge and ordering (blocked by longest wait, then error, then done), `revision`, `seen`, `stale`, record id in the key (today `agents()` keeps only the worst record per block and drops its id).
- Open question carried from `protocol.md`: whether `msg`/`title` reach clients on the user's server build.

## G5. Guarded reply and excerpt

```rust
pub struct ReplyTicket { pub key: AttentionKey, pub revision: u64, pub epoch: u64 }

pub enum Reply { Text(String), Key(Key) }     // Text is pasted, then Enter is sent

pub enum ReplyOutcome {
    /// The record changed after the send.
    Confirmed { now: AgentState },
    /// Sent; the server accepted it; no status change yet.
    SentUnconfirmed,
    /// Nothing was sent: the record or connection no longer matches the ticket.
    Stale { now: Option<Attention> },
    NotSent(LinkError),
}

impl Boxes {
    pub fn ticket(&self, key: &AttentionKey) -> Option<ReplyTicket>;
    /// Compares the ticket with the live record under the model lock, then sends once.
    /// Never retried, never replayed after a reconnect.
    pub fn reply(&self, ticket: &ReplyTicket, reply: Reply) -> flume::Receiver<ReplyOutcome>;
    /// Plain text of the last `lines` screen lines. Read-only; claims nothing.
    pub fn excerpt(&self, key: &AttentionKey, lines: u16) -> flume::Receiver<Result<Excerpt, LinkError>>;
}
pub struct Excerpt { pub lines: Vec<String>, pub cols: u16, pub taken: std::time::SystemTime }
```

- [new] all of it. The core can send keys and pastes to a block it has a terminal connection for [core `Command::Key`, `Paste`], which implies attaching the session and, at this revision, taking size (G1). A reply path that uses `block.event` paste and `client.key` directly on the control connection, with no data stream and no claim, is the goal; whether those calls work for a session this client observes but has not attached is [U].
- [new] `com.superlogical.terminal.format` (text) for the excerpt is not called by the core [V by search]. Whether it needs `session.attach` is [U].
- The check-then-send is not atomic with the server. The guard narrows the window to one round trip; it does not close it. The sheet's wording must not claim more.

## G6. Adding a box

```rust
pub fn local_candidate() -> Option<Endpoint>;                    // socket exists; starts nothing
pub fn tailnet_machines() -> Result<Vec<Machine>, String>;       // local `tailscale status --json`
pub struct Machine { pub name: String, pub os: String, pub online: bool, pub tailscale_ssh: bool }
pub fn check(endpoint: &Endpoint) -> flume::Receiver<CheckResult>;   // hello + server.status only
pub enum CheckResult { Rex(ServerFacts, u32 /* sessions */), Nothing, Unknown(String), Needs(LinkQuestion) }
impl Boxes {
    pub fn answer(&self, id: &BoxId, answer: LinkAnswer);        // TrustHostKey, Cancel
    pub fn shared_hosts(&self) -> Vec<SharedBox>;                // from the home server's list
    pub fn share(&self, id: &BoxId) -> Result<(), StoreError>;   // hosts.put
    pub fn unshare(&self, id: &BoxId) -> Result<(), StoreError>; // hosts.remove: every device
}
```

- [core] `tailnet::status()`, `tailnet::Machine`, `tailnet::Probe::{Rex, Nothing, Unknown}`; `ssh::Target::parse`, `ssh::Tunnel`, `ssh::Problem`, strict host-key checking; `hosts::SharedHost`, `HostCommand::{WatchSharedHosts, PutSharedHost, RemoveSharedHost}`, `hosts::reconcile`, `HostModel.shared`; `transport::local_endpoint()`.
- [core?] the entry point that runs a probe for one machine and the host-key confirmation call live under rex-rune's `ui` feature or its worker; which parts compile with `default-features = false` needs checking.
- Not wrapped on purpose: `ssh::SetupOffer::{Start, Install}`. This client never starts or installs a server.

## G7. Explicit changes

```rust
impl Host {
    pub fn new_session(&self, label: Option<String>);
    pub fn close(&self, target: CloseTarget) -> flume::Receiver<CloseOutcome>;   // pane, tab, session
    pub fn what_ends(&self, target: &CloseTarget) -> Vec<RunningProgram>;        // for the dialog
    pub fn undo_close(&self);                                                    // same epoch only
    pub fn rename(&self, target: RenameTarget, label: String);
    pub fn focus_direction(&self, session: &str, direction: Direction);
}
```

- [core] `HostCommand::{CreateSession, Close { kind: HumanClose, .. }, UndoClose { epoch }, NewTerminal, Request}`; `HostModel.programs`, `foreground`, `executables` for "what ends".
- Existing `new_tab`, `split`, `zoom`, `open`, `select` stay.

## G8. Notifications and applet feed

```rust
pub enum Notice {
    Became { item: Attention },          // only when observed_live
    Cleared { key: AttentionKey },
    BoxNeedsYou { id: BoxId, question: LinkQuestion },
    BoxLost { id: BoxId },
}
impl Boxes { pub fn notices(&self) -> flume::Receiver<Notice>; }   // rate-limited and deduped here
```

- [core] `StatusRecord.live` separates watched changes from read-back state; `HostModel.notices` and `bells` exist and are cleared unused by `Host::snapshot` today.
- [new] rate limit and storm collapse (policy belongs in one place, not per frontend); sending desktop notifications (the core's sender is behind its `ui` feature, so cosmic-rex needs its own `org.freedesktop.Notifications` call, a new dependency such as `zbus`); a small session-bus interface on the app for the applet (`Overview.inbox` summary, "raise on key").

## G9. Local tab pinning

Needed for "follow when idle, hold when typing" (`ux-spec.md` 6.3): render a tab that is not the server's active one, and learn which client changed the active tab.

- [core?] `HostCommand::FetchView`, `PrefetchViews` and `background_views` suggest the core can hold layouts for more than the active view. Rendering a non-active window's panes is not confirmed.
- [U] whether the server says who changed focus. Without it the notice reads "switched from another device".
- Fallback needs nothing new: follow, and drop key presses for 400 ms after an unrequested switch.

## What does not need the backend

Layout breakpoints, the switcher's ranking, the paused overlay, key bindings, box tints, the answer sheet's inert timers and all wording are frontend work over the data above.
