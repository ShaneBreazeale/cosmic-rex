//! One box: a connected Rex server, or shells on this computer.
use crate::{Size, Snapshot, input::Input, model::size, predict::Predictor};
use rex_rune::{
    connection::{
        Command, Connection, DEFAULT_TAB_LABEL, HostCommand, HostConnection, HumanClose,
        perform::Outcome,
    },
    layout,
    terminal::selection,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Further down than any scrollback, so the view returns to the live screen.
const TO_BOTTOM: isize = 1 << 30;

/// A coalescing "something changed" signal, and an identity that changes when it is replaced.
#[derive(Clone)]
pub struct Wake {
    pub id: String,
    pub signal: flume::Receiver<()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Press,
    Repeat,
    Release,
}

/// A key as the layout produced it. The server encodes it for the program.
#[derive(Clone, Debug, PartialEq)]
pub struct Key {
    /// Ghostty's physical key name, such as `key_a`.
    pub name: String,
    /// `shift+ctrl+alt+super`, in that order.
    pub mods: String,
    pub consumed_mods: &'static str,
    pub text: String,
    pub unshifted: u32,
    pub action: KeyAction,
}

impl Key {
    /// The one character this types, if it is plain typing.
    fn plain(&self) -> Option<char> {
        let mut chars = self.text.chars();
        let c = chars.next().filter(|c| *c == ' ' || c.is_ascii_graphic())?;
        (chars.next().is_none() && matches!(self.mods.as_str(), "" | "shift")).then_some(c)
    }
    fn is_modifier(&self) -> bool {
        ["shift_", "control_", "alt_", "meta_", "super_", "caps_lock", "num_lock", "fn"]
            .iter()
            .any(|name| self.name.starts_with(name))
    }
    pub(crate) fn payload(&self) -> Value {
        json!({
            "key": self.name,
            "mods": self.mods,
            "consumed_mods": self.consumed_mods,
            "text": self.text,
            "unshifted_codepoint": self.unshifted,
            "composing": false,
            "action": match self.action {
                KeyAction::Press => "press",
                KeyAction::Repeat => "repeat",
                KeyAction::Release => "release",
            },
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    Right,
    Down,
}

/// A step of a pointer selection in one pane. Selections are this client's own: nothing reaches
/// the server or the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Select {
    /// Pressed: 1 starts at a cell, 2 takes a word, 3 or more a line.
    Press(u32),
    Drag,
    Release,
    Clear,
    /// The whole screen and scrollback.
    All,
}

/// Where the pointer is over a pane's grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointer {
    /// Column and row of the frame, clamped to it.
    pub cell: (u16, u16),
    /// Pixels from the grid's top-left corner; outside the grid while dragging past its edge.
    pub position: (f32, f32),
    pub cell_width: f32,
    /// Pixels of grid on screen, so a drag past the edge scrolls.
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// A mouse report for a program that asked for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouse {
    Press(Button),
    /// Moved with this button held.
    Drag(Button),
    Release(Button),
}

/// A box's sessions and terminals: a Rex server's, or this app's own shells on this computer.
pub struct Host(Kind);

enum Kind {
    Rex(Remote),
    Local(crate::local::Local),
}

/// Calls the method of the same name on whichever kind of box this is.
macro_rules! both {
    ($($(#[$doc:meta])* fn $name:ident(&self $(, $arg:ident: $ty:ty)*) $(-> $ret:ty)?;)*) => {$(
        $(#[$doc])*
        pub fn $name(&self $(, $arg: $ty)*) $(-> $ret)? {
            match &self.0 {
                Kind::Rex(h) => h.$name($($arg),*),
                Kind::Local(h) => h.$name($($arg),*),
            }
        }
    )*};
}

impl Host {
    /// Connect in the background and keep reconnecting; for [`crate::boxes::LOCAL`], run shells
    /// here instead. Never starts or stops a server, and never creates or destroys a session on
    /// one.
    pub fn connect(endpoint: String, session: Option<String>) -> Self {
        if endpoint == crate::boxes::LOCAL {
            return Self(Kind::Local(crate::local::Local::start(next_serial())));
        }
        Self(Kind::Rex(Remote::connect(endpoint, session)))
    }
    /// Its shells run in this app and end with it; there is no server.
    pub fn is_local(&self) -> bool {
        matches!(self.0, Kind::Local(_))
    }
    /// For reading and answering a program's question, in any session on this box. Only a Rex
    /// server reads programs' questions.
    pub fn asker(&self) -> Option<crate::Asker> {
        match &self.0 {
            Kind::Rex(h) => Some(h.asker()),
            Kind::Local(_) => None,
        }
    }
    /// A new session with one shell. Only on this computer; a Rex server's sessions are made
    /// elsewhere.
    pub fn new_session(&self) {
        if let Kind::Local(h) = &self.0 {
            h.new_session();
        }
    }
    /// A new session running `program` (its argv), named `label`. Only on this computer.
    pub fn run(&self, label: &str, program: Vec<String>) {
        if let Kind::Local(h) = &self.0 {
            h.run(label, program);
        }
    }
    /// Take this pane's size, on the person's explicit request: every client viewing it re-wraps.
    /// Local panes always have this window's size.
    pub fn fit_here(&self, session: &str, block: &str) {
        if let Kind::Rex(h) = &self.0 {
            h.fit_here(session, block);
        }
    }
    pub fn reconnect(&self) {
        if let Kind::Rex(h) = &self.0 {
            h.reconnect();
        }
    }
    both! {
        fn snapshot(&self) -> Snapshot;
        fn wakes(&self) -> Vec<Wake>;
        /// Typing. Dropped, never queued, if the pane is not live.
        fn key(&self, block: &str, key: &Key);
        fn paste(&self, block: &str, text: String);
        /// Report the pointer to a program that tracks the mouse.
        fn mouse(&self, block: &str, mouse: Mouse, cell: (u16, u16), mods: &str);
        /// One step of a selection in this pane, made by this client on its own copy of the
        /// screen. Its text comes with the first frame whose `selection_revision` is the one
        /// returned.
        fn selection(&self, block: &str, step: Select, at: Pointer) -> Option<u64>;
        /// Empties a live pane's screen and scrollback, keeping the current line. The program
        /// keeps running.
        fn clear(&self, block: &str);
        /// Draw this box's terminals, or only keep its sessions and statuses current.
        fn set_visible(&self, visible: bool);
        /// Local viewport lines; negative is up.
        fn scroll(&self, block: &str, lines: isize);
        /// Wheel lines for a program that tracks the mouse; negative is up.
        fn wheel(&self, block: &str, x: u16, y: u16, lines: i32);
        /// This window's size for a pane it sizes. Ignored for any other.
        fn resize(&self, block: &str, cols: u16, rows: u16, cell: (u16, u16));
        fn focus(&self, block: &str);
        fn select(&self, session: String);
        fn open(&self, session: String, window: Option<String>);
        fn zoom(&self, session: &str, block: &str);
        /// A new tab, starting in `block`'s directory.
        fn new_tab(&self, session: &str, block: &str);
        /// Ends every program in the tab. Only on a confirmed, explicit request; sent once.
        fn close_tab(&self, session: &str, window: &str);
        /// Ends the program in one pane. Only on a confirmed, explicit request; sent once.
        fn close_pane(&self, session: &str, block: &str);
        /// Names a tab; an empty name gives it its default title back.
        fn rename_tab(&self, session: &str, window: &str, label: &str);
        /// Ends every program in every tab of the session. Only on a confirmed, explicit request.
        fn close_session(&self, session: &str);
        /// Names a session. An empty name keeps the old one.
        fn rename_session(&self, session: &str, label: &str);
        /// Moves a session to `slot` in the box's list: 0 is the top, the list's length the end.
        fn move_session(&self, session: &str, slot: usize);
        fn split(&self, session: &str, block: &str, split: Split);
    }
}

/// One step of a pointer selection as the core's selection controller takes it, numbered after
/// the pane's last one. `None` when it continues a gesture that has ended.
pub(crate) fn selection_step(
    selections: &Mutex<HashMap<String, (u64, Option<selection::Input>)>>,
    block: &str,
    step: Select,
    at: Pointer,
    generation: u64,
    cols: u16,
) -> Option<selection::Input> {
    let mut selections = selections.lock().expect("selections");
    let (revision, last) = selections.entry(block.to_owned()).or_default();
    let phase = match step {
        Select::Press(clicks) => selection::Phase::Down(clicks),
        Select::Drag => selection::Phase::Move,
        Select::Release => selection::Phase::Up,
        Select::Clear => selection::Phase::Clear,
        Select::All => selection::Phase::All,
    };
    // A release ends the gesture where it last was; one with no gesture does nothing.
    let input = match (step, *last) {
        (Select::Release | Select::Drag, None) => return None,
        (Select::Release, Some(mut input)) => {
            input.phase = phase;
            input
        }
        _ => {
            // Ghostty picks the half of a cell from a whole-pixel cell width.
            let width = at.cell_width.ceil().max(1.0);
            selection::Input {
                generation,
                revision: 0,
                phase,
                at: at.cell,
                position: (f64::from(at.position.0 * width / at.cell_width.max(1.0)), f64::from(at.position.1)),
                geometry: selection::Geometry {
                    columns: u32::from(cols),
                    cell_width: width as u32,
                    padding_left: 0,
                    screen_height: at.height.ceil().max(1.0) as u32,
                },
            }
        }
    };
    *revision += 1;
    let input = selection::Input { revision: *revision, ..input };
    *last = matches!(step, Select::Press(_) | Select::Drag).then_some(input);
    Some(input)
}

struct Remote {
    /// Distinguishes this connection's wakes from an earlier one to the same box.
    serial: u64,
    core: HostConnection,
    input: Input,
    /// Live panes as of the last snapshot.
    terminals: Mutex<HashMap<String, Connection>>,
    navigation: Mutex<u64>,
    /// Unclaimed panes this client has asked to size, once each.
    asked: Mutex<HashSet<String>>,
    predictors: Mutex<HashMap<String, Predictor>>,
    /// Each pane's last selection step and its revision, so a release or clear can follow it.
    selections: Mutex<HashMap<String, (u64, Option<selection::Input>)>>,
}

fn next_serial() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Remote {
    fn connect(endpoint: String, session: Option<String>) -> Self {
        let core = HostConnection::spawn(endpoint.clone(), session, None, None, true, None);
        let input = Input::spawn(endpoint, core.model.clone());
        Self {
            serial: next_serial(),
            core,
            input,
            terminals: Mutex::default(),
            navigation: Mutex::new(0),
            asked: Mutex::default(),
            predictors: Mutex::default(),
            selections: Mutex::default(),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        // Built under the core's lock, without copying its model: this runs on every screen
        // update, and a model holds every session, layout and status on the box.
        let (mut snapshot, terminals, performs) = {
            let mut live = self.core.model.lock().expect("host model");
            // Queues the core hands over once. Requests from other clients are refused below;
            // the rest has no consumer yet.
            let performs = std::mem::take(&mut live.performs);
            live.client_actions.clear();
            live.prompt_events.clear();
            live.notices.clear();
            live.bells.clear();
            live.clipboard = None;
            (Snapshot::new(&live), live.terminals.clone(), performs)
        };
        for p in performs {
            self.send(HostCommand::PerformReply {
                epoch: p.epoch,
                call_id: p.request.call_id,
                outcome: Outcome::error("refused", "Remote control is off in this client"),
            });
        }
        // A pane is live only if typing reaches it too.
        let typing = self.input.state.live();
        let round_trip = self.input.state.round_trip();
        let mut predictors = self.predictors.lock().expect("predictors");
        predictors.retain(|block, _| snapshot.panes.iter().any(|p| &p.block == block));
        for pane in &mut snapshot.panes {
            pane.live &= typing;
            if let (Some(frame), Some(p)) = (&pane.frame, predictors.get_mut(&pane.block)) {
                p.observe(frame, Instant::now(), round_trip);
                if pane.live {
                    pane.guess = p.show(frame, round_trip);
                } else {
                    p.reset();
                }
            }
        }
        drop(predictors);
        if snapshot.connected && snapshot.notice.is_none() {
            snapshot.notice = self.input.take_notice();
        }
        *self.terminals.lock().expect("terminals") = terminals;
        snapshot
    }

    pub fn wakes(&self) -> Vec<Wake> {
        let mut all = vec![Wake { id: format!("{}:host", self.serial), signal: self.core.wake.clone() }];
        all.push(Wake { id: format!("{}:input", self.serial), signal: self.input.wake.clone() });
        all.extend(self.terminals.lock().expect("terminals").iter().map(|(block, c)| Wake {
            id: format!("{}:{block}:{:p}", self.serial, Arc::as_ptr(&c.model)),
            signal: c.wake.clone(),
        }));
        all
    }

    fn send(&self, command: HostCommand) {
        // Fails only when the worker has stopped, which the snapshot's status shows.
        let _ = self.core.commands.send(command);
    }

    fn terminal(&self, block: &str, command: Command) {
        if let Some(c) = self.terminals.lock().expect("terminals").get(block) {
            let _ = c.commands.try_send(command);
        }
    }

    /// Send input to a live pane, without ever taking its size from another client. What is
    /// typed at a dead screen is dropped, never queued for later.
    fn type_into(&self, block: &str, method: &'static str, mut payload: Value, claims: bool) -> bool {
        let (session, size) = {
            let m = self.core.model.lock().expect("host model");
            let Some(session) = m.selected.clone() else { return false };
            let live = m.connected
                && m.terminals.get(block).is_some_and(|c| {
                    let t = c.model.lock().expect("terminal model");
                    t.connected && t.control_connected
                });
            if !live {
                return false;
            }
            let size = size(&m, &session, block);
            (session, size)
        };
        payload["session_id"] = json!(session);
        payload["block_id"] = json!(block);
        if !self.input.send(method, payload) {
            return false;
        }
        if !claims {
            return true;
        }
        self.scroll(block, TO_BOTTOM);
        // Using a pane nobody has sized gives it this window's size. `request` cannot take one
        // that another client claims first.
        let mut asked = self.asked.lock().expect("asked");
        if size != Size::Unclaimed {
            asked.remove(block);
        } else if asked.insert(block.to_owned()) {
            self.send(HostCommand::Request {
                method: "session.own_block".into(),
                payload: json!({"session_id": session, "block_ids": [block], "mode": "request"}),
            });
        }
        true
    }

    pub fn key(&self, block: &str, key: &Key) {
        let press = key.action != KeyAction::Release;
        if self.type_into(block, "client.key", json!({"key": key.payload()}), press) && press {
            self.predict(block, |p| match key.plain() {
                Some(c) => p.typed(c, Instant::now()),
                None if key.is_modifier() => {}
                None => p.reset(),
            });
        }
    }
    pub fn paste(&self, block: &str, text: String) {
        let event = json!({"event": {"type": "paste", "payload": {"text": text}}});
        if self.type_into(block, "block.event", event, true) {
            self.predict(block, Predictor::reset);
        }
    }
    /// Report the pointer to a program that tracks the mouse. Only a press asks for an
    /// unclaimed pane's size, as typing does; nothing takes one another client sized.
    pub fn mouse(&self, block: &str, mouse: Mouse, cell: (u16, u16), mods: &str) {
        let (action, button, claims) = match mouse {
            Mouse::Press(b) => ("press", b, true),
            Mouse::Drag(b) => ("motion", b, false),
            Mouse::Release(b) => ("release", b, false),
        };
        let button = match button {
            Button::Left => "left",
            Button::Middle => "middle",
            Button::Right => "right",
        };
        let event = json!({"event": {"type": "mouse", "payload": {
            "action": action, "button": button, "x": cell.0, "y": cell.1, "mods": mods}}});
        self.type_into(block, "block.event", event, claims);
    }
    /// One step of a selection in this pane, made by this client on its own copy of the screen.
    /// Its text comes with the first frame whose `selection_revision` is the one returned.
    pub fn selection(&self, block: &str, step: Select, at: Pointer) -> Option<u64> {
        let (generation, cols) = {
            let terminals = self.terminals.lock().expect("terminals");
            let t = terminals.get(block)?.model.lock().expect("terminal model");
            (t.restore_generation, t.frame.as_ref().map_or(1, |f| f.cols))
        };
        let input = selection_step(&self.selections, block, step, at, generation, cols)?;
        let revision = input.revision;
        self.terminal(block, Command::Selection(input));
        Some(revision)
    }
    /// Clears a live pane's screen and scrollback for every device, keeping the current line.
    /// The program keeps running. Not input: it takes no size. Dropped if the pane is not live.
    pub fn clear(&self, block: &str) {
        let live = {
            let m = self.core.model.lock().expect("host model");
            m.connected
                && m.terminals.get(block).is_some_and(|c| {
                    let t = c.model.lock().expect("terminal model");
                    t.connected && t.control_connected
                })
        };
        if live && self.input.state.live() {
            self.terminal(block, Command::Clear);
        }
    }
    fn predict(&self, block: &str, act: impl FnOnce(&mut Predictor)) {
        act(self.predictors.lock().expect("predictors").entry(block.to_owned()).or_default());
    }
    /// For reading and answering a program's question, in any session on this box.
    pub fn asker(&self) -> crate::Asker {
        crate::Asker(self.input.sender())
    }

    /// Take this pane's size, on the person's explicit request: every client viewing it re-wraps.
    pub fn fit_here(&self, session: &str, block: &str) {
        self.send(HostCommand::Claim {
            session: session.to_owned(),
            blocks: vec![block.to_owned()],
        });
    }
    /// Draw this box's terminals, or only keep its sessions and statuses current.
    pub fn set_visible(&self, visible: bool) {
        self.send(HostCommand::Visibility(visible));
    }
    pub fn reconnect(&self) {
        self.send(HostCommand::Reconnect);
    }
    /// Local viewport lines; negative is up.
    pub fn scroll(&self, block: &str, lines: isize) {
        self.terminal(block, Command::Scroll(lines));
    }
    /// Wheel lines for a program that tracks the mouse; negative is up. One event per line.
    pub fn wheel(&self, block: &str, x: u16, y: u16, lines: i32) {
        for _ in 0..lines.unsigned_abs().min(8) {
            let event = json!({"event": {"type": "wheel", "payload": {
                "x": x, "y": y, "delta_x": 0, "delta_y": lines.signum()}}});
            self.type_into(block, "block.event", event, false);
        }
    }
    /// Report this client's size for a pane it owns. Ignored for any other: the server applies a
    /// resize at once to a pane nobody owns, so merely looking at one would re-wrap it.
    pub fn resize(&self, block: &str, cols: u16, rows: u16, cell: (u16, u16)) {
        let ours = {
            let m = self.core.model.lock().expect("host model");
            m.selected.as_deref().is_some_and(|s| size(&m, s, block) == Size::Here)
        };
        if ours {
            self.terminal(block, Command::Resize(cols, rows, cell.0, cell.1));
        }
    }
    pub fn focus(&self, block: &str) {
        if let Some(c) = self.terminals.lock().expect("terminals").get(block) {
            let _ = c.commands.focus();
        }
    }
    pub fn select(&self, session: String) {
        self.send(HostCommand::Select(session));
    }
    pub fn open(&self, session: String, window: Option<String>) {
        let id = {
            let mut n = self.navigation.lock().expect("navigation");
            *n += 1;
            *n
        };
        self.send(HostCommand::Navigate {
            id,
            deadline: Instant::now() + Duration::from_secs(8),
            session,
            window,
        });
    }
    pub fn zoom(&self, session: &str, block: &str) {
        self.send(HostCommand::Request {
            method: "session.zoom_block".into(),
            payload: json!({"session_id": session, "block_id": block}),
        });
    }
    /// A new tab, starting in `block`'s directory.
    pub fn new_tab(&self, session: &str, block: &str) {
        self.send(HostCommand::NewTerminal {
            method: "session.new_window".into(),
            payload: json!({
                "session_id": session,
                "window_label": DEFAULT_TAB_LABEL,
                "layout": layout::shell_layout(None, None),
                "focus": true,
            }),
            cwd_from: block.to_owned(),
        });
    }
    /// Ends every program in the tab. Only on a confirmed, explicit request; sent once.
    pub fn close_tab(&self, session: &str, window: &str) {
        self.send(HostCommand::Close {
            kind: HumanClose::Tab,
            payload: json!({"session_id": session, "window_id": window}),
        });
    }
    /// Ends the program in one pane. Only on a confirmed, explicit request; sent once.
    pub fn close_pane(&self, session: &str, block: &str) {
        self.send(HostCommand::Close {
            kind: HumanClose::Pane,
            payload: json!({"session_id": session, "block_id": block}),
        });
    }
    /// Names a tab for every device; an empty name gives it its default title back.
    pub fn rename_tab(&self, session: &str, window: &str, label: &str) {
        self.send(HostCommand::Request {
            method: "session.set_window_label".into(),
            payload: json!({"session_id": session, "window_id": window, "label": label}),
        });
    }
    /// Ends every program in every tab of the session. Only on a confirmed, explicit request.
    pub fn close_session(&self, session: &str) {
        self.send(HostCommand::Close {
            kind: HumanClose::Session,
            payload: json!({"session_id": session}),
        });
    }
    /// Names a session for every device. The server keeps the old name for an empty one.
    pub fn rename_session(&self, session: &str, label: &str) {
        self.send(HostCommand::Request {
            method: "session.set_label".into(),
            payload: json!({"session_id": session, "label": label}),
        });
    }
    /// Moves a session to `slot` in the box's list, as last seen: 0 is the top, the list's
    /// length the end. The order is the server's, so every device sees it.
    pub fn move_session(&self, session: &str, slot: usize) {
        let sessions = self.snapshot().sessions;
        let order: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        let Some(from) = order.iter().position(|id| *id == session) else { return };
        let Some(before) = move_before(&order, from, slot) else { return };
        let mut payload = json!({"session_id": session});
        if let Some(before) = before {
            payload["before_session_id"] = json!(before);
        }
        self.send(HostCommand::Request { method: "session.move".into(), payload });
    }
    pub fn split(&self, session: &str, block: &str, split: Split) {
        self.send(HostCommand::NewTerminal {
            method: "session.new_split".into(),
            payload: json!({
                "session_id": session,
                "anchor_id": block,
                "direction": match split { Split::Right => "horizontal", Split::Down => "vertical" },
                "side": "after",
                "layout": layout::shell_layout(None, None),
                "focus": true,
            }),
            cwd_from: block.to_owned(),
        });
    }
}

/// Where `session.move` should put the session at `from` so it lands at `slot`: before the
/// session given, or at the end for `Some(None)`. `None` when it would not move.
fn move_before<'a>(order: &[&'a str], from: usize, slot: usize) -> Option<Option<&'a str>> {
    if from >= order.len() || slot > order.len() || slot == from || slot == from + 1 {
        return None;
    }
    Some(order.get(slot).copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Outgoing;
    use tokio::sync::mpsc::UnboundedReceiver;

    #[test]
    fn moving_a_session_names_the_one_it_goes_before() {
        let order = ["a", "b", "c"];
        assert_eq!(move_before(&order, 2, 0), Some(Some("a")));
        assert_eq!(move_before(&order, 0, 2), Some(Some("c")));
        assert_eq!(move_before(&order, 0, 3), Some(None));
        // Its own slot or the one just after it: it stays where it is.
        assert_eq!(move_before(&order, 1, 1), None);
        assert_eq!(move_before(&order, 1, 2), None);
        assert_eq!(move_before(&order, 2, 3), None);
        assert_eq!(move_before(&order, 0, 4), None);
        assert_eq!(move_before(&order, 3, 0), None);
    }

    #[test]
    fn key_payload_matches_the_documented_shape() {
        let key = Key {
            name: "key_a".into(),
            mods: "shift".into(),
            consumed_mods: "shift",
            text: "A".into(),
            unshifted: 97,
            action: KeyAction::Press,
        };
        assert_eq!(
            key.payload(),
            json!({"key": "key_a", "mods": "shift", "consumed_mods": "shift", "text": "A",
                   "unshifted_codepoint": 97, "composing": false, "action": "press"})
        );
    }

    #[test]
    fn an_unreachable_server_reports_disconnected_and_shuts_down() {
        let host = Host::connect("unix:///nonexistent/rex-test.sock".into(), None);
        let wake = host.wakes().remove(0).signal;
        let _ = wake.recv_timeout(Duration::from_secs(5));
        let s = host.snapshot();
        assert!(!s.connected && s.panes.is_empty());
        drop(host);
        assert!(crate::shutdown(Duration::from_secs(10)));
    }

    /// A host with no worker, so every command it would act on can be read.
    fn intercepted(
        owner: Option<&str>,
        live: bool,
    ) -> (Remote, flume::Receiver<HostCommand>, UnboundedReceiver<Outgoing>) {
        use rex_rune::connection::{HostModel, TerminalSender};
        let (commands, sent) = flume::unbounded();
        let (_, wake) = flume::bounded(1);
        let mut m = HostModel {
            connected: true,
            client_id: "client:me".into(),
            selected: Some("session:a".into()),
            ..Default::default()
        };
        let view = json!({
            "revision": 1, "session_id": "session:a", "label": "a", "active_window_id": "window:1",
            "windows": [{"window_id": "window:1", "focused_block_id": "block:1", "layers": [{
                "layer_id": "layer:1", "kind": "tiled", "bounds": [0.0, 0.0, 1.0, 1.0],
                "layout": {"rect": [0.0, 0.0, 1.0, 1.0], "block_id": "block:1"},
                "blocks": [{"block_id": "block:1", "owner": owner}]}]}]
        });
        m.views.insert("session:a".into(), serde_json::from_value(view).unwrap());
        let pane = rex_rune::connection::Model {
            connected: live,
            control_connected: live,
            owner: owner == Some("client:me"),
            ..Default::default()
        };
        m.terminals.insert(
            "block:1".into(),
            Connection {
                model: Arc::new(Mutex::new(pane)),
                commands: TerminalSender::new(commands.clone(), "session:a".into(), "block:1".into()),
                wake: wake.clone(),
            },
        );
        let core = HostConnection { model: Arc::new(Mutex::new(m)), commands, wake };
        let (input, typed) = Input::intercepted(true);
        let host = Remote {
            serial: next_serial(),
            core,
            input,
            terminals: Mutex::default(),
            navigation: Mutex::new(0),
            asked: Mutex::default(),
            predictors: Mutex::default(),
            selections: Mutex::default(),
        };
        host.snapshot();
        (host, sent, typed)
    }

    fn press() -> Key {
        Key {
            name: "key_a".into(),
            mods: String::new(),
            consumed_mods: "",
            text: "a".into(),
            unshifted: 97,
            action: KeyAction::Press,
        }
    }

    #[test]
    fn typing_never_takes_a_pane_another_client_owns() {
        let (host, sent, mut typed) = intercepted(Some("client:mac"), true);
        for _ in 0..100 {
            host.key("block:1", &press());
        }
        host.paste("block:1", "text".into());
        host.wheel("block:1", 1, 1, -3);
        // The core, which claims before it types, is asked only to scroll its own view.
        for command in sent.try_iter() {
            assert!(matches!(command, HostCommand::Terminal { command: Command::Scroll(_), .. }));
        }
        let mut methods = Vec::new();
        while let Ok(Outgoing { method, payload, .. }) = typed.try_recv() {
            assert_eq!(payload["session_id"], "session:a");
            assert_eq!(payload["block_id"], "block:1");
            methods.push(method);
        }
        assert_eq!(methods.len(), 104);
        assert!(methods[..100].iter().all(|m| *m == "client.key"));
        assert!(methods[100..].iter().all(|m| *m == "block.event"));
    }

    #[test]
    fn looking_at_a_pane_never_resizes_it() {
        for owner in [Some("client:mac"), None] {
            let (host, sent, _typed) = intercepted(owner, true);
            host.resize("block:1", 100, 30, (8, 18));
            assert!(sent.try_recv().is_err());
        }
        let (host, sent, _typed) = intercepted(Some("client:me"), true);
        host.resize("block:1", 100, 30, (8, 18));
        assert!(matches!(
            sent.try_recv().unwrap(),
            HostCommand::Terminal { command: Command::Resize(100, 30, 8, 18), .. }
        ));
    }

    #[test]
    fn typing_in_an_unclaimed_pane_asks_for_it_once_and_politely() {
        let (host, sent, mut typed) = intercepted(None, true);
        host.wheel("block:1", 1, 1, 1);
        assert!(sent.try_recv().is_err(), "the wheel asks for nothing");
        for _ in 0..5 {
            host.key("block:1", &press());
        }
        let asked: Vec<Value> = sent
            .try_iter()
            .filter_map(|c| match c {
                HostCommand::Request { method, payload } if method == "session.own_block" => Some(payload),
                HostCommand::Terminal { command: Command::Scroll(_), .. } => None,
                _ => panic!("unexpected command"),
            })
            .collect();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0]["mode"], "request");
        assert!(typed.try_recv().is_ok());
    }

    fn pointer(x: u16, y: u16) -> Pointer {
        Pointer { cell: (x, y), position: (f32::from(x) * 8.0, f32::from(y) * 18.0), cell_width: 8.0, height: 360.0 }
    }

    #[test]
    fn selecting_stays_on_this_client_and_counts_its_steps() {
        let (host, sent, mut typed) = intercepted(Some("client:mac"), true);
        assert_eq!(host.selection("block:1", Select::Release, pointer(0, 0)), None, "no gesture to end");
        assert_eq!(host.selection("block:1", Select::Press(2), pointer(3, 1)), Some(1));
        assert_eq!(host.selection("block:1", Select::Drag, pointer(9, 2)), Some(2));
        assert_eq!(host.selection("block:1", Select::Release, pointer(40, 9)), Some(3));
        assert_eq!(host.selection("block:1", Select::Release, pointer(0, 0)), None, "it has ended");
        assert_eq!(host.selection("block:1", Select::All, pointer(0, 0)), Some(4));
        assert_eq!(host.selection("block:9", Select::Press(1), pointer(0, 0)), None, "no such pane");
        let steps: Vec<_> = sent
            .try_iter()
            .map(|c| match c {
                HostCommand::Terminal { block, command: Command::Selection(i), .. } => {
                    assert_eq!(block, "block:1");
                    (i.revision, i.at)
                }
                _ => panic!("a selection asks the server for nothing"),
            })
            .collect();
        // A release ends where the drag last was, not where the pointer let go.
        assert_eq!(steps, [(1, (3, 1)), (2, (9, 2)), (3, (9, 2)), (4, (0, 0))]);
        assert!(typed.try_recv().is_err());
    }

    #[test]
    fn mouse_reports_never_take_a_pane_and_only_a_press_asks_for_one() {
        let (host, sent, mut typed) = intercepted(Some("client:mac"), true);
        host.mouse("block:1", Mouse::Press(Button::Left), (2, 3), "ctrl");
        host.mouse("block:1", Mouse::Drag(Button::Left), (4, 3), "");
        host.mouse("block:1", Mouse::Release(Button::Left), (4, 3), "");
        let mut actions = Vec::new();
        while let Ok(Outgoing { method, payload, .. }) = typed.try_recv() {
            assert_eq!(method, "block.event");
            assert_eq!(payload["event"]["type"], "mouse");
            actions.push(payload["event"]["payload"]["action"].as_str().unwrap().to_owned());
        }
        assert_eq!(actions, ["press", "motion", "release"]);
        assert!(sent.try_iter().all(|c| matches!(c, HostCommand::Terminal { command: Command::Scroll(_), .. })));

        let (host, sent, _typed) = intercepted(None, true);
        host.mouse("block:1", Mouse::Drag(Button::Left), (4, 3), "");
        assert!(sent.try_recv().is_err(), "motion asks for nothing");
        host.mouse("block:1", Mouse::Press(Button::Left), (4, 3), "");
        assert!(sent.try_iter().any(|c| matches!(c, HostCommand::Request { method, .. } if method == "session.own_block")));
    }

    #[test]
    fn clearing_reaches_only_a_live_pane() {
        let (host, sent, _typed) = intercepted(None, false);
        host.clear("block:1");
        host.mouse("block:1", Mouse::Press(Button::Left), (0, 0), "");
        assert!(sent.try_recv().is_err());
        let (host, sent, _typed) = intercepted(Some("client:mac"), true);
        host.clear("block:1");
        assert!(matches!(sent.try_recv().unwrap(), HostCommand::Terminal { command: Command::Clear, .. }));
    }

    #[test]
    fn input_to_a_dead_pane_is_dropped_not_queued() {
        for owner in [Some("client:me"), Some("client:mac"), None] {
            let (host, sent, mut typed) = intercepted(owner, false);
            host.key("block:1", &press());
            host.paste("block:1", "rm -rf x".into());
            assert!(sent.try_recv().is_err() && typed.try_recv().is_err());
        }
        let (mut host, sent, _) = intercepted(None, true);
        let (down, mut typed) = Input::intercepted(false);
        host.input = down;
        host.key("block:1", &press());
        assert!(sent.try_recv().is_err() && typed.try_recv().is_err());
        assert!(!host.snapshot().panes[0].live, "and the pane says so");
    }
}
