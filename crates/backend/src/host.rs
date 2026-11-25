//! One connected Rex server.
use crate::{Size, Snapshot, input::Input, model::size, predict::Predictor};
use rex_rune::{
    connection::{
        Command, Connection, DEFAULT_TAB_LABEL, HostCommand, HostConnection, perform::Outcome,
    },
    layout,
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

pub struct Host {
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
}

fn next_serial() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Host {
    /// Connect in the background and keep reconnecting. Never starts or stops a server, and
    /// never creates or destroys a session.
    pub fn connect(endpoint: String, session: Option<String>) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Outgoing;
    use tokio::sync::mpsc::UnboundedReceiver;

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
    ) -> (Host, flume::Receiver<HostCommand>, UnboundedReceiver<Outgoing>) {
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
        let host = Host {
            serial: next_serial(),
            core,
            input,
            terminals: Mutex::default(),
            navigation: Mutex::new(0),
            asked: Mutex::default(),
            predictors: Mutex::default(),
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
