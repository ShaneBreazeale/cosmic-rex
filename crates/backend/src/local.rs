//! Shells on this computer, run by this app with no server. They end when the app does.
//!
//! One worker thread owns every terminal (Ghostty's are not `Send`). Each pane has a reader
//! thread, whose bounded channel pushes back on a flooding program rather than buffering it, and
//! a writer thread, so a program that stops reading stalls only its own pane.
use crate::{
    AgentState, Frame, Key, KeyAction, Mouse, PaneView, Pointer, Select, Session, Size, Snapshot,
    Split, Tab, Wake, host::selection_step, plain,
};
use ghostty_vt::{
    Terminal,
    key, mouse, paste,
    screen::Screen,
    terminal::{Event, Mode, Options, ScrollViewport},
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use rex_rune::terminal::{RENDER_HOLD_LIMIT, Renderer, graphics, selection};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Draw at most this often while a program writes.
const FRAME: Duration = Duration::from_millis(8);
/// What Ghostty keeps by default.
const SCROLLBACK_BYTES: usize = 10_000_000;
const FIRST_SIZE: (u16, u16) = (80, 24);
const FIRST_CELL: (u16, u16) = (8, 18);

/// A tab's panes: each split halves its space.
#[derive(Debug, PartialEq)]
enum Node {
    Leaf(String),
    Split { across: bool, parts: Box<(Node, Node)> },
}

impl Node {
    fn leaves<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Node::Leaf(b) => out.push(b),
            Node::Split { parts, .. } => {
                parts.0.leaves(out);
                parts.1.leaves(out);
            }
        }
    }
    fn first(&self) -> &str {
        match self {
            Node::Leaf(b) => b,
            Node::Split { parts, .. } => parts.0.first(),
        }
    }
    fn place(&self, [x, y, w, h]: [f64; 4], out: &mut Vec<(String, [f64; 4])>) {
        match self {
            Node::Leaf(b) => out.push((b.clone(), [x, y, w, h])),
            Node::Split { across: true, parts } => {
                parts.0.place([x, y, w / 2.0, h], out);
                parts.1.place([x + w / 2.0, y, w / 2.0, h], out);
            }
            Node::Split { across: false, parts } => {
                parts.0.place([x, y, w, h / 2.0], out);
                parts.1.place([x, y + h / 2.0, w, h / 2.0], out);
            }
        }
    }
    /// Put `new` after `at`, sharing its space.
    fn split(&mut self, at: &str, new: String, across: bool) -> bool {
        match self {
            Node::Leaf(b) if b == at => {
                let old = std::mem::replace(self, Node::Leaf(String::new()));
                *self = Node::Split { across, parts: Box::new((old, Node::Leaf(new))) };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { parts, .. } => {
                parts.0.split(at, new.clone(), across) || parts.1.split(at, new, across)
            }
        }
    }
    /// This tree without `block`: its sibling takes its space. `None` when nothing is left.
    fn without(self, block: &str) -> Option<Node> {
        match self {
            Node::Leaf(b) if b == block => None,
            Node::Leaf(_) => Some(self),
            Node::Split { across, parts } => {
                let (a, b) = *parts;
                match (a.without(block), b.without(block)) {
                    (Some(a), Some(b)) => Some(Node::Split { across, parts: Box::new((a, b)) }),
                    (one, None) | (None, one) => one,
                }
            }
        }
    }
}

struct Window {
    id: String,
    /// Given by the person; otherwise the focused pane's title is shown.
    label: Option<String>,
    tree: Node,
    focused: String,
    zoomed: Option<String>,
}

impl Window {
    fn leaves(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.tree.leaves(&mut out);
        out
    }
}

struct LocalSession {
    id: String,
    label: String,
    windows: Vec<Window>,
    active: usize,
}

#[derive(Default)]
struct PaneInfo {
    frame: Option<Frame>,
    title: String,
    pid: Option<u32>,
    size: Option<(u16, u16)>,
}

#[derive(Default)]
struct State {
    sessions: Vec<LocalSession>,
    selected: Option<String>,
    panes: HashMap<String, PaneInfo>,
    notice: Option<String>,
    visible: bool,
    /// The first session has been opened: closing every one later leaves none.
    started: bool,
    next: u64,
}

impl State {
    fn id(&mut self, kind: &str) -> String {
        self.next += 1;
        format!("local-{kind}-{}", self.next)
    }
    fn session(&self, id: &str) -> Option<&LocalSession> {
        self.sessions.iter().find(|s| s.id == id)
    }
    fn session_mut(&mut self, id: &str) -> Option<&mut LocalSession> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }
    /// The session and tab holding `block`.
    fn place_of(&self, block: &str) -> Option<(usize, usize)> {
        self.sessions.iter().enumerate().find_map(|(si, s)| {
            let wi = s.windows.iter().position(|w| w.leaves().contains(&block))?;
            Some((si, wi))
        })
    }
    fn title(&self, block: &str) -> String {
        self.panes.get(block).map(|p| p.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(shell_name)
    }
    /// Where a shell opened beside `block` should start.
    fn cwd(&self, block: &str) -> Option<PathBuf> {
        let pid = self.panes.get(block)?.pid?;
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
    fn size(&self, block: &str) -> (u16, u16) {
        self.panes.get(block).and_then(|p| p.size).unwrap_or(FIRST_SIZE)
    }
    /// Take `block` out of the layout, and its tab and session if they are left empty.
    fn remove_block(&mut self, block: &str) {
        self.panes.remove(block);
        let Some((si, wi)) = self.place_of(block) else { return };
        let session = &mut self.sessions[si];
        let window = &mut session.windows[wi];
        let tree = std::mem::replace(&mut window.tree, Node::Leaf(String::new()));
        match tree.without(block) {
            Some(tree) => {
                if window.focused == block {
                    window.focused = tree.first().to_owned();
                }
                if window.zoomed.as_deref() == Some(block) {
                    window.zoomed = None;
                }
                window.tree = tree;
            }
            None => self.remove_window(si, wi),
        }
    }
    fn remove_window(&mut self, si: usize, wi: usize) {
        let session = &mut self.sessions[si];
        session.windows.remove(wi);
        if session.windows.is_empty() {
            self.remove_session(si);
        } else if session.active >= wi && session.active > 0 {
            session.active -= 1;
        }
    }
    fn remove_session(&mut self, si: usize) {
        let gone = self.sessions.remove(si);
        if self.selected.as_ref() == Some(&gone.id) {
            let next = self.sessions.get(si).or_else(|| self.sessions.last());
            self.selected = next.map(|s| s.id.clone());
        }
    }
    fn unique_label(&self) -> String {
        (1..)
            .map(|n| if n == 1 { "Shell".to_owned() } else { format!("Shell {n}") })
            .find(|l| self.sessions.iter().all(|s| &s.label != l))
            .expect("unbounded")
    }
}

/// The shell's own name, such as `zsh`.
fn shell_name() -> String {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        let shell = shell();
        shell.rsplit('/').next().unwrap_or("shell").to_owned()
    })
    .clone()
}

fn shell() -> String {
    static SHELL: OnceLock<String> = OnceLock::new();
    SHELL
        .get_or_init(|| {
            std::env::var("SHELL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| CommandBuilder::new_default_prog().get_shell())
        })
        .clone()
}

const TERM_ENV: [(&str, &str); 3] =
    [("TERM", "xterm-256color"), ("COLORTERM", "truecolor"), ("TERM_PROGRAM", "cosmic-rex")];

enum Op {
    /// A shell, or `program` (its argv) in its place.
    Spawn { block: String, cwd: Option<PathBuf>, size: (u16, u16), program: Option<Vec<String>> },
    Key(String, Key),
    Paste(String, String),
    Mouse(String, mouse::Action, mouse::Button, (u16, u16), String),
    Wheel(String, (u16, u16), i32),
    Selection(String, selection::Input),
    Scroll(String, isize),
    Clear(String),
    Resize(String, (u16, u16), (u16, u16)),
    Kill(String),
    /// Shown again: draw what changed meanwhile.
    Render,
    Stop,
}

/// From a pane's reader thread, in the order the program wrote.
enum Output {
    Bytes(String, Vec<u8>),
    Ended(String),
}

pub(crate) struct Local {
    serial: u64,
    state: Arc<Mutex<State>>,
    ops: flume::Sender<Op>,
    notify: flume::Sender<()>,
    wake: flume::Receiver<()>,
    selections: Mutex<HashMap<String, (u64, Option<selection::Input>)>>,
    worker: Option<JoinHandle<()>>,
}

impl Local {
    pub(crate) fn start(serial: u64) -> Self {
        let state = Arc::new(Mutex::new(State { visible: true, ..Default::default() }));
        let (notify, wake) = flume::bounded(1);
        let (ops, commands) = flume::unbounded();
        let worker = {
            let (state, notify) = (state.clone(), notify.clone());
            std::thread::Builder::new()
                .name("local shells".into())
                .spawn(move || run(&state, &notify, &commands))
                .expect("thread")
        };
        Self { serial, state, ops, notify, wake, selections: Mutex::default(), worker: Some(worker) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("local state")
    }
    fn send(&self, op: Op) {
        let _ = self.ops.send(op);
    }
    fn changed(&self) {
        let _ = self.notify.try_send(());
    }
    /// A pane that is running and drawn. Input to any other is dropped.
    fn live(&self, block: &str) -> bool {
        self.lock().panes.get(block).is_some_and(|p| p.frame.is_some())
    }

    /// The first session opens when the box is first shown, not before.
    fn start_if_shown(&self, st: &mut State) {
        if st.visible && !st.started {
            st.started = true;
            self.open_session(st);
        }
    }
    fn open_session(&self, st: &mut State) {
        self.open_program(st, None, None);
    }
    /// A session with one pane running `program`, or a shell.
    fn open_program(&self, st: &mut State, label: Option<&str>, program: Option<Vec<String>>) {
        let (session, window, block) = (st.id("session"), st.id("window"), st.id("block"));
        let label = match label {
            Some(l) if st.sessions.iter().all(|s| s.label != l) => l.to_owned(),
            _ => st.unique_label(),
        };
        st.sessions.push(LocalSession {
            id: session.clone(),
            label,
            windows: vec![Window {
                id: window,
                label: None,
                tree: Node::Leaf(block.clone()),
                focused: block.clone(),
                zoomed: None,
            }],
            active: 0,
        });
        st.selected = Some(session);
        st.notice = None;
        st.panes.insert(block.clone(), PaneInfo { size: Some(FIRST_SIZE), ..Default::default() });
        self.send(Op::Spawn { block, cwd: None, size: FIRST_SIZE, program });
    }
    fn spawn(&self, st: &mut State, block: String, cwd: Option<PathBuf>, size: (u16, u16)) {
        st.notice = None;
        st.panes.insert(block.clone(), PaneInfo { size: Some(size), ..Default::default() });
        self.send(Op::Spawn { block, cwd, size, program: None });
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        let mut st = self.lock();
        self.start_if_shown(&mut st);
        let st = &*st;
        let current = st.selected.as_deref().and_then(|id| st.session(id));
        let window = current.and_then(|s| s.windows.get(s.active));
        let mut placed = Vec::new();
        if let Some(w) = window {
            match &w.zoomed {
                Some(z) => placed.push((z.clone(), [0.0, 0.0, 1.0, 1.0])),
                None => w.tree.place([0.0, 0.0, 1.0, 1.0], &mut placed),
            }
        }
        let tab_label = |w: &Window| w.label.clone().unwrap_or_else(|| st.title(&w.focused));
        Snapshot {
            connected: true,
            tab_panes: window.map_or(0, |w| w.leaves().len()),
            status: String::new(),
            problem: None,
            notice: st.notice.clone(),
            sessions: st
                .sessions
                .iter()
                .map(|s| Session {
                    id: s.id.clone(),
                    label: s.label.clone(),
                    state: AgentState::Idle,
                    about: s.windows.get(s.active).map(|w| st.title(&w.focused)).unwrap_or_default(),
                })
                .collect(),
            session: current.map(|s| s.id.clone()),
            tabs: current
                .into_iter()
                .flat_map(|s| &s.windows)
                .map(|w| Tab { id: w.id.clone(), label: tab_label(w) })
                .collect(),
            tab: window.map(|w| w.id.clone()),
            panes: placed
                .into_iter()
                .map(|(block, rect)| {
                    let frame = st.panes.get(&block).and_then(|p| p.frame.clone());
                    let w = window.expect("placed panes have a window");
                    PaneView {
                        focused: w.focused == block,
                        title: st.title(&block),
                        size: Size::Here,
                        live: frame.is_some(),
                        state: AgentState::Idle,
                        guess: String::new(),
                        zoomed: w.zoomed.as_ref() == Some(&block),
                        status: String::new(),
                        rect,
                        frame,
                        block,
                    }
                })
                .collect(),
            agents: Vec::new(),
        }
    }

    pub(crate) fn wakes(&self) -> Vec<Wake> {
        vec![Wake { id: format!("{}:local", self.serial), signal: self.wake.clone() }]
    }

    pub(crate) fn key(&self, block: &str, key: &Key) {
        if self.live(block) {
            self.send(Op::Key(block.to_owned(), key.clone()));
        }
    }
    pub(crate) fn paste(&self, block: &str, text: String) {
        if self.live(block) {
            self.send(Op::Paste(block.to_owned(), text));
        }
    }
    pub(crate) fn mouse(&self, block: &str, report: Mouse, cell: (u16, u16), mods: &str) {
        let (action, button) = match report {
            Mouse::Press(b) => (mouse::Action::Press, b),
            Mouse::Drag(b) => (mouse::Action::Motion, b),
            Mouse::Release(b) => (mouse::Action::Release, b),
        };
        let button = match button {
            crate::Button::Left => mouse::Button::Left,
            crate::Button::Middle => mouse::Button::Middle,
            crate::Button::Right => mouse::Button::Right,
        };
        if self.live(block) {
            self.send(Op::Mouse(block.to_owned(), action, button, cell, mods.to_owned()));
        }
    }
    pub(crate) fn wheel(&self, block: &str, x: u16, y: u16, lines: i32) {
        if self.live(block) {
            self.send(Op::Wheel(block.to_owned(), (x, y), lines));
        }
    }
    pub(crate) fn selection(&self, block: &str, step: Select, at: Pointer) -> Option<u64> {
        let cols = self.lock().panes.get(block)?.frame.as_ref()?.cols();
        let input = selection_step(&self.selections, block, step, at, 0, cols)?;
        self.send(Op::Selection(block.to_owned(), input));
        Some(input.revision)
    }
    pub(crate) fn clear(&self, block: &str) {
        if self.live(block) {
            self.send(Op::Clear(block.to_owned()));
        }
    }
    pub(crate) fn scroll(&self, block: &str, lines: isize) {
        self.send(Op::Scroll(block.to_owned(), lines));
    }
    /// Every local pane is sized by this window.
    pub(crate) fn resize(&self, block: &str, cols: u16, rows: u16, cell: (u16, u16)) {
        let mut st = self.lock();
        let Some(pane) = st.panes.get_mut(block) else { return };
        pane.size = Some((cols, rows));
        drop(st);
        self.send(Op::Resize(block.to_owned(), (cols, rows), cell));
    }
    pub(crate) fn set_visible(&self, visible: bool) {
        let mut st = self.lock();
        st.visible = visible;
        if visible {
            self.start_if_shown(&mut st);
            self.send(Op::Render);
        }
    }

    pub(crate) fn focus(&self, block: &str) {
        let mut st = self.lock();
        if let Some((si, wi)) = st.place_of(block) {
            st.sessions[si].windows[wi].focused = block.to_owned();
        }
        drop(st);
        self.changed();
    }
    pub(crate) fn select(&self, session: String) {
        self.open(session, None);
    }
    pub(crate) fn open(&self, session: String, window: Option<String>) {
        let mut st = self.lock();
        let Some(s) = st.session_mut(&session) else { return };
        if let Some(at) = window.and_then(|w| s.windows.iter().position(|x| x.id == w)) {
            s.active = at;
        }
        st.selected = Some(session);
        drop(st);
        self.changed();
    }
    pub(crate) fn zoom(&self, _session: &str, block: &str) {
        let mut st = self.lock();
        if let Some((si, wi)) = st.place_of(block) {
            let w = &mut st.sessions[si].windows[wi];
            w.zoomed = if w.zoomed.as_deref() == Some(block) { None } else { Some(block.to_owned()) };
            w.focused = block.to_owned();
        }
        drop(st);
        self.changed();
    }
    /// A new session with one shell, in the home folder.
    pub(crate) fn new_session(&self) {
        let mut st = self.lock();
        st.started = true;
        self.open_session(&mut st);
        drop(st);
        self.changed();
    }
    /// A new session named `label` (if free) whose one pane runs `program`, shown at once. Its
    /// pane closes when the program ends, as a shell's does.
    pub(crate) fn run(&self, label: &str, program: Vec<String>) {
        let mut st = self.lock();
        st.started = true;
        self.open_program(&mut st, Some(label), Some(program));
        drop(st);
        self.changed();
    }
    /// A new tab after the current one, starting in `block`'s folder.
    pub(crate) fn new_tab(&self, session: &str, block: &str) {
        let mut st = self.lock();
        let (window, new) = (st.id("window"), st.id("block"));
        let (cwd, size) = (st.cwd(block), st.size(block));
        let Some(s) = st.session_mut(session) else { return };
        let at = (s.active + 1).min(s.windows.len());
        s.windows.insert(at, Window {
            id: window,
            label: None,
            tree: Node::Leaf(new.clone()),
            focused: new.clone(),
            zoomed: None,
        });
        s.active = at;
        self.spawn(&mut st, new, cwd, size);
        drop(st);
        self.changed();
    }
    pub(crate) fn split(&self, _session: &str, block: &str, split: Split) {
        let mut st = self.lock();
        let Some((si, wi)) = st.place_of(block) else { return };
        let new = st.id("block");
        let (cwd, (cols, rows)) = (st.cwd(block), st.size(block));
        let across = split == Split::Right;
        let size = if across { ((cols / 2).max(1), rows) } else { (cols, (rows / 2).max(1)) };
        let w = &mut st.sessions[si].windows[wi];
        w.tree.split(block, new.clone(), across);
        w.focused = new.clone();
        w.zoomed = None;
        self.spawn(&mut st, new, cwd, size);
        drop(st);
        self.changed();
    }
    /// Ends every shell in the tab. Only on a confirmed, explicit request.
    pub(crate) fn close_tab(&self, session: &str, window: &str) {
        let mut st = self.lock();
        let Some(si) = st.sessions.iter().position(|s| s.id == session) else { return };
        let Some(wi) = st.sessions[si].windows.iter().position(|w| w.id == window) else { return };
        let blocks: Vec<String> =
            st.sessions[si].windows[wi].leaves().into_iter().map(str::to_owned).collect();
        st.remove_window(si, wi);
        self.end(&mut st, blocks);
    }
    /// Ends the shell in one pane. Only on a confirmed, explicit request.
    pub(crate) fn close_pane(&self, _session: &str, block: &str) {
        let mut st = self.lock();
        st.remove_block(block);
        self.end(&mut st, vec![block.to_owned()]);
    }
    /// Ends every shell in the session. Only on a confirmed, explicit request.
    pub(crate) fn close_session(&self, session: &str) {
        let mut st = self.lock();
        let Some(si) = st.sessions.iter().position(|s| s.id == session) else { return };
        let blocks: Vec<String> = st.sessions[si]
            .windows
            .iter()
            .flat_map(|w| w.leaves())
            .map(str::to_owned)
            .collect();
        st.remove_session(si);
        self.end(&mut st, blocks);
    }
    fn end(&self, st: &mut State, blocks: Vec<String>) {
        for block in blocks {
            st.panes.remove(&block);
            self.send(Op::Kill(block));
        }
        self.changed();
    }
    /// An empty name gives the tab its default title back.
    pub(crate) fn rename_tab(&self, session: &str, window: &str, label: &str) {
        let mut st = self.lock();
        if let Some(w) = st.session_mut(session).and_then(|s| s.windows.iter_mut().find(|w| w.id == window)) {
            let label = plain(label.trim(), 60);
            w.label = (!label.is_empty()).then_some(label);
        }
        drop(st);
        self.changed();
    }
    /// An empty name keeps the old one.
    pub(crate) fn rename_session(&self, session: &str, label: &str) {
        let label = plain(label.trim(), 60);
        let mut st = self.lock();
        if let Some(s) = st.session_mut(session).filter(|_| !label.is_empty()) {
            s.label = label;
        }
        drop(st);
        self.changed();
    }
    /// Moves a session to `slot`: 0 is the top, the list's length the end.
    pub(crate) fn move_session(&self, session: &str, slot: usize) {
        let mut st = self.lock();
        let Some(from) = st.sessions.iter().position(|s| s.id == session) else { return };
        if slot > st.sessions.len() || slot == from || slot == from + 1 {
            return;
        }
        let moved = st.sessions.remove(from);
        st.sessions.insert(if slot > from { slot - 1 } else { slot }, moved);
        drop(st);
        self.changed();
    }
}

impl Drop for Local {
    fn drop(&mut self) {
        self.send(Op::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Pane {
    terminal: Terminal,
    renderer: Renderer,
    selection: selection::Controller,
    keys: key::Encoder,
    mice: mouse::Encoder,
    master: Box<dyn MasterPty + Send>,
    input: flume::Sender<Vec<u8>>,
    child: Box<dyn Child + Send + Sync>,
    cell: (u16, u16),
    dirty: bool,
    held_since: Option<Instant>,
}

impl Pane {
    fn write(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            let _ = self.input.send(bytes);
        }
    }
    /// End the shell and reap it without waiting here.
    fn end(mut self) {
        let _ = self.child.kill();
        let mut child = self.child;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

fn spawn(
    block: &str,
    cwd: Option<PathBuf>,
    (cols, rows): (u16, u16),
    program: Option<Vec<String>>,
    output: &flume::Sender<Output>,
) -> Result<(Pane, Option<u32>), String> {
    let pair = native_pty_system()
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("{e:#}"))?;
    let dir = cwd.or_else(|| std::env::var_os("HOME").map(PathBuf::from)).unwrap_or_else(|| "/".into());
    let mut command = match program {
        Some(argv) => CommandBuilder::from_argv(argv.into_iter().map(Into::into).collect()),
        None => CommandBuilder::new(shell()),
    };
    command.cwd(&dir);
    for (key, value) in TERM_ENV {
        command.env(key, value);
    }
    let child = pair.slave.spawn_command(command).map_err(|e| format!("{e:#}"))?;
    drop(pair.slave);
    let pid = child.process_id();
    let mut reader = pair.master.try_clone_reader().map_err(|e| format!("{e:#}"))?;
    let mut writer = pair.master.take_writer().map_err(|e| format!("{e:#}"))?;
    let (input, typed) = flume::unbounded::<Vec<u8>>();
    std::thread::spawn(move || {
        for bytes in typed {
            if writer.write_all(&bytes).and_then(|()| writer.flush()).is_err() {
                break;
            }
        }
    });
    let (output, name) = (output.clone(), block.to_owned());
    std::thread::spawn(move || {
        let mut buffer = vec![0; 64 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if output.send(Output::Bytes(name.clone(), buffer[..n].to_vec())).is_err() {
                        return;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = output.send(Output::Ended(name));
    });
    let ghostty = |e: ghostty_vt::Error| e.to_string();
    let mut terminal =
        Terminal::new(Options { cols, rows, max_scrollback: SCROLLBACK_BYTES }).map_err(ghostty)?;
    graphics::install_decoder().map_err(|e| format!("{e:#}"))?;
    graphics::configure(&mut terminal).map_err(|e| format!("{e:#}"))?;
    let pane = Pane {
        terminal,
        renderer: Renderer::new().map_err(|e| format!("{e:#}"))?,
        selection: selection::Controller::new().map_err(|e| format!("{e:#}"))?,
        keys: key::Encoder::new().map_err(ghostty)?,
        mice: mouse::Encoder::new().map_err(ghostty)?,
        master: pair.master,
        input,
        child,
        cell: FIRST_CELL,
        dirty: true,
        held_since: None,
    };
    Ok((pane, pid))
}

fn run(state: &Mutex<State>, notify: &flume::Sender<()>, ops: &flume::Receiver<Op>) {
    // Bounded: a program writing faster than this draws waits in its write.
    let (output, written) = flume::bounded::<Output>(64);
    let mut panes: HashMap<String, Pane> = HashMap::new();
    let mut drawn = Instant::now();
    loop {
        let visible = state.lock().expect("local state").visible;
        let busy = panes.values().any(|p| p.dirty || p.selection.autoscrolling(&p.terminal));
        let wait = if visible && busy {
            (drawn + FRAME).saturating_duration_since(Instant::now()).max(Duration::from_millis(1))
        } else {
            Duration::from_secs(3600)
        };
        let mut stop = false;
        // Titles and closed panes show even when nothing is drawn.
        let mut touched = false;
        let first = flume::Selector::new()
            .recv(ops, |op| op.ok().map(Ok))
            .recv(&written, |out| out.ok().map(Err))
            .wait_timeout(wait);
        let mut next = first.ok().flatten();
        // Everything waiting, but not so much output that drawing stalls.
        let mut budget = 256;
        while let Some(event) = next.take() {
            match event {
                Ok(Op::Stop) => stop = true,
                Ok(op) => touched |= command(op, &mut panes, state, &output),
                Err(out) => touched |= take_output(out, &mut panes, state),
            }
            budget -= 1;
            if budget == 0 || stop {
                break;
            }
            next = ops.try_recv().map(Ok).ok().or_else(|| written.try_recv().map(Err).ok());
        }
        if stop || ops.is_disconnected() {
            for (_, pane) in panes.drain() {
                pane.end();
            }
            return;
        }
        let visible = state.lock().expect("local state").visible;
        if visible && Instant::now() >= drawn + FRAME {
            let mut changed = false;
            for (block, pane) in &mut panes {
                changed |= draw(block, pane, state);
            }
            if changed {
                drawn = Instant::now();
                touched = true;
            }
        }
        if touched {
            let _ = notify.try_send(());
        }
    }
}

/// Act on one request. True if the layout changed.
fn command(op: Op, panes: &mut HashMap<String, Pane>, state: &Mutex<State>, output: &flume::Sender<Output>) -> bool {
    match op {
        Op::Spawn { block, cwd, size, program } => {
            // Closed before it started.
            if !state.lock().expect("local state").panes.contains_key(&block) {
                return false;
            }
            match spawn(&block, cwd, size, program, output) {
                Ok((pane, pid)) => {
                    if let Some(info) = state.lock().expect("local state").panes.get_mut(&block) {
                        info.pid = pid;
                    }
                    panes.insert(block, pane);
                }
                Err(error) => {
                    let mut st = state.lock().expect("local state");
                    st.remove_block(&block);
                    st.notice = Some(plain(&format!("Could not start a shell: {error}"), 300));
                    return true;
                }
            }
        }
        Op::Kill(block) => {
            if let Some(pane) = panes.remove(&block) {
                pane.end();
            }
        }
        Op::Render => panes.values_mut().for_each(|p| p.dirty = true),
        Op::Stop => {}
        Op::Key(block, k) => {
            let Some(pane) = panes.get_mut(&block) else { return false };
            let Some(event) = key_event(&k) else { return false };
            let mut bytes = Vec::new();
            pane.keys.set_options_from_terminal(&pane.terminal);
            if pane.keys.encode_to_vec(&event, &mut bytes).is_ok() && !bytes.is_empty() {
                pane.write(bytes);
                if k.action != KeyAction::Release {
                    pane.terminal.scroll_viewport(ScrollViewport::Bottom);
                    let _ = pane.selection.after_scroll(&pane.terminal);
                    pane.dirty = true;
                }
            }
        }
        Op::Paste(block, text) => {
            let Some(pane) = panes.get_mut(&block) else { return false };
            let bracketed = pane.terminal.mode(Mode::BRACKETED_PASTE).unwrap_or(false);
            let mut data = text.into_bytes();
            let mut buffer = vec![0; data.len() + 32];
            if let Ok(n) = paste::encode(&mut data, bracketed, &mut buffer) {
                buffer.truncate(n);
                pane.write(buffer);
                pane.terminal.scroll_viewport(ScrollViewport::Bottom);
                pane.dirty = true;
            }
        }
        Op::Mouse(block, action, button, cell, mods) => {
            let Some(pane) = panes.get_mut(&block) else { return false };
            let mut bytes = Vec::new();
            if mouse_report(pane, action, Some(button), cell, &mods, &mut bytes) {
                pane.write(bytes);
            }
        }
        Op::Wheel(block, cell, lines) => {
            let Some(pane) = panes.get_mut(&block) else { return false };
            let button = if lines < 0 { mouse::Button::Four } else { mouse::Button::Five };
            for _ in 0..lines.unsigned_abs().min(8) {
                let mut bytes = Vec::new();
                if mouse_report(pane, mouse::Action::Press, Some(button), cell, "", &mut bytes) {
                    pane.write(bytes);
                }
            }
        }
        Op::Selection(block, input) => {
            let Some(pane) = panes.get_mut(&block) else { return false };
            if pane.selection.apply(&pane.terminal, input).is_ok() {
                pane.dirty = true;
            }
        }
        Op::Scroll(block, lines) => {
            let Some(pane) = panes.get_mut(&block) else { return false };
            pane.terminal.scroll_viewport(ScrollViewport::Delta(lines));
            let _ = pane.selection.after_scroll(&pane.terminal);
            pane.dirty = true;
        }
        Op::Clear(block) => {
            let Some(pane) = panes.get_mut(&block) else { return false };
            // Scroll the screen up to the cursor's line, then drop the scrollback: what remains
            // is the line being typed. The program is not told.
            let primary = pane.terminal.active_screen().is_ok_and(|s| s == Screen::Primary);
            let up = pane.terminal.cursor_y().unwrap_or(0);
            let bytes = if primary && up > 0 { format!("\x1b[{up}S\x1b[{up}A\x1b[3J") } else { "\x1b[3J".into() };
            pane.terminal.vt_write(bytes.as_bytes());
            pane.terminal.scroll_viewport(ScrollViewport::Bottom);
            pane.dirty = true;
        }
        Op::Resize(block, (cols, rows), cell) => {
            let Some(pane) = panes.get_mut(&block) else { return false };
            let cell = (cell.0.max(1), cell.1.max(1));
            let same = pane.terminal.cols().ok() == Some(cols) && pane.terminal.rows().ok() == Some(rows);
            if same && pane.cell == cell || cols == 0 || rows == 0 {
                return false;
            }
            pane.cell = cell;
            let _ = pane.terminal.resize(cols, rows, u32::from(cell.0), u32::from(cell.1));
            let _ = pane.master.resize(PtySize {
                rows,
                cols,
                pixel_width: cols.saturating_mul(cell.0),
                pixel_height: rows.saturating_mul(cell.1),
            });
            pane.dirty = true;
        }
    }
    false
}

/// Feed a program's output to its terminal. True if a title or the layout changed.
fn take_output(out: Output, panes: &mut HashMap<String, Pane>, state: &Mutex<State>) -> bool {
    match out {
        Output::Bytes(block, bytes) => {
            let Some(pane) = panes.get_mut(&block) else { return false };
            pane.terminal.vt_write(&bytes);
            pane.dirty = true;
            let events: Vec<Event> = pane.terminal.events().collect();
            let mut titled = false;
            for event in events {
                match event {
                    // Answers to the program's own questions, such as where the cursor is.
                    Event::PtyWrite(answer) => pane.write(answer),
                    Event::TitleChanged(title) => {
                        if let Some(info) = state.lock().expect("local state").panes.get_mut(&block) {
                            info.title = plain(&title, 120);
                            titled = true;
                        }
                    }
                    // Clipboard writes and notifications from programs are not passed on.
                    _ => {}
                }
            }
            titled
        }
        // The shell exited: its pane closes, as in Rex.
        Output::Ended(block) => {
            let Some(pane) = panes.remove(&block) else { return false };
            pane.end();
            state.lock().expect("local state").remove_block(&block);
            true
        }
    }
}

/// Draw `pane` if it changed and is not mid synchronized update. True if a frame was published.
fn draw(block: &str, pane: &mut Pane, state: &Mutex<State>) -> bool {
    let autoscroll = pane.selection.autoscrolling(&pane.terminal);
    if !pane.dirty && !autoscroll {
        return false;
    }
    if pane.terminal.mode(Mode::SYNC_OUTPUT).unwrap_or(false) {
        let since = *pane.held_since.get_or_insert_with(Instant::now);
        if since.elapsed() < RENDER_HOLD_LIMIT {
            return false;
        }
        let _ = pane.terminal.set_mode(Mode::SYNC_OUTPUT, false);
    }
    pane.held_since = None;
    if autoscroll {
        let _ = pane.selection.tick(&mut pane.terminal);
    }
    let Ok(mut frame) = pane.renderer.frame(&pane.terminal) else { return false };
    frame.selection_revision = pane.selection.revision;
    pane.dirty = false;
    match state.lock().expect("local state").panes.get_mut(block) {
        Some(info) => {
            info.frame = Some(Frame(Arc::new(frame)));
            true
        }
        None => false,
    }
}

fn mouse_report(
    pane: &mut Pane,
    action: mouse::Action,
    button: Option<mouse::Button>,
    cell: (u16, u16),
    mods: &str,
    bytes: &mut Vec<u8>,
) -> bool {
    let (cols, rows) = (pane.terminal.cols().unwrap_or(1), pane.terminal.rows().unwrap_or(1));
    let (width, height) = (u32::from(pane.cell.0), u32::from(pane.cell.1));
    let Ok(mut event) = mouse::Event::new() else { return false };
    event.set_action(action).set_button(button).set_mods(parse_mods(mods)).set_position(mouse::Position {
        x: (f32::from(cell.0) + 0.5) * width as f32,
        y: (f32::from(cell.1) + 0.5) * height as f32,
    });
    pane.mice
        .set_options_from_terminal(&pane.terminal)
        .set_size(mouse::EncoderSize {
            screen_width: u32::from(cols) * width,
            screen_height: u32::from(rows) * height,
            cell_width: width,
            cell_height: height,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        })
        .set_any_button_pressed(action == mouse::Action::Motion);
    pane.mice.encode_to_vec(&event, bytes).is_ok() && !bytes.is_empty()
}

fn parse_mods(mods: &str) -> key::Mods {
    mods.split('+').fold(key::Mods::empty(), |all, m| {
        all | match m {
            "shift" => key::Mods::SHIFT,
            "ctrl" => key::Mods::CTRL,
            "alt" => key::Mods::ALT,
            "super" => key::Mods::SUPER,
            _ => key::Mods::empty(),
        }
    })
}

/// Ghostty's key for its physical key name, such as `key_a` or `arrow_up`.
fn key_code(name: &str) -> Option<key::Key> {
    static NAMES: OnceLock<HashMap<String, key::Key>> = OnceLock::new();
    NAMES
        .get_or_init(|| {
            (0..512)
                .filter_map(|i| key::Key::try_from(i).ok())
                .filter(|k| *k != key::Key::Unidentified)
                .map(|k| (key_name(k), k))
                .collect()
        })
        .get(name)
        .copied()
}

/// `A` -> `key_a`, `Digit1` -> `digit_1`, `ArrowUp` -> `arrow_up`, `F12` -> `f12`.
fn key_name(k: key::Key) -> String {
    let debug = format!("{k:?}");
    if debug.len() == 1 {
        return format!("key_{}", debug.to_ascii_lowercase());
    }
    if let Some(n) = debug.strip_prefix('F')
        && !n.is_empty()
        && n.bytes().all(|b| b.is_ascii_digit())
    {
        return format!("f{n}");
    }
    let mut out = String::with_capacity(debug.len() + 4);
    let mut previous: Option<char> = None;
    for ch in debug.chars() {
        if previous.is_some_and(|p| {
            (ch.is_ascii_uppercase() && (p.is_ascii_lowercase() || p.is_ascii_digit()))
                || (ch.is_ascii_digit() && p.is_ascii_alphabetic())
        }) {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
        previous = Some(ch);
    }
    out
}

fn key_event(k: &Key) -> Option<key::Event> {
    let code = key_code(&k.name)?;
    let mut event = key::Event::new().ok()?;
    // Control characters are the encoder's to make, from the key and its modifiers.
    let text = Some(k.text.clone()).filter(|t| !t.is_empty() && !t.chars().any(char::is_control));
    event
        .set_action(match k.action {
            KeyAction::Press => key::Action::Press,
            KeyAction::Repeat => key::Action::Repeat,
            KeyAction::Release => key::Action::Release,
        })
        .set_key(code)
        .set_mods(parse_mods(&k.mods))
        .set_consumed_mods(parse_mods(k.consumed_mods))
        .set_utf8(text);
    if let Some(c) = char::from_u32(k.unshifted).filter(|c| *c != '\0') {
        event.set_unshifted_codepoint(c);
    }
    Some(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(name: &str, mods: &str, text: &str) -> Key {
        Key {
            name: name.into(),
            mods: mods.into(),
            consumed_mods: "",
            text: text.into(),
            unshifted: text.chars().next().map_or(0, |c| c as u32),
            action: KeyAction::Press,
        }
    }

    fn encode(k: &Key) -> Vec<u8> {
        let terminal = Terminal::new(Options { cols: 80, rows: 24, max_scrollback: 0 }).unwrap();
        let mut encoder = key::Encoder::new().unwrap();
        encoder.set_options_from_terminal(&terminal);
        let mut out = Vec::new();
        encoder.encode_to_vec(&key_event(k).unwrap(), &mut out).unwrap();
        out
    }

    #[test]
    fn key_names_match_the_frontends() {
        for (name, key) in [
            ("key_a", key::Key::A),
            ("digit_1", key::Key::Digit1),
            ("arrow_up", key::Key::ArrowUp),
            ("numpad_0", key::Key::Numpad0),
            ("f12", key::Key::F12),
            ("bracket_left", key::Key::BracketLeft),
            ("page_up", key::Key::PageUp),
            ("enter", key::Key::Enter),
            ("escape", key::Key::Escape),
        ] {
            assert_eq!(key_code(name), Some(key), "{name}");
        }
        assert_eq!(key_code("unidentified"), None);
    }

    #[test]
    fn keys_encode_as_a_terminal_would() {
        assert_eq!(encode(&press("key_a", "", "a")), b"a");
        assert_eq!(encode(&press("key_c", "ctrl", "")), b"\x03");
        // The frontend sends Enter's text as \n; the program gets a carriage return.
        assert_eq!(encode(&press("enter", "", "\n")), b"\r");
        assert_eq!(encode(&press("arrow_up", "", "")), b"\x1b[A");
        assert_eq!(encode(&press("backspace", "", "")), b"\x7f");
    }

    #[test]
    fn a_tab_closes_up_around_a_closed_pane() {
        let mut tree = Node::Leaf("a".into());
        assert!(tree.split("a", "b".into(), true));
        assert!(tree.split("b", "c".into(), false));
        let mut placed = Vec::new();
        tree.place([0.0, 0.0, 1.0, 1.0], &mut placed);
        assert_eq!(placed[0], ("a".into(), [0.0, 0.0, 0.5, 1.0]));
        assert_eq!(placed[2], ("c".into(), [0.5, 0.5, 0.5, 0.5]));
        let tree = tree.without("b").unwrap();
        let mut placed = Vec::new();
        tree.place([0.0, 0.0, 1.0, 1.0], &mut placed);
        assert_eq!(placed, [("a".into(), [0.0, 0.0, 0.5, 1.0]), ("c".into(), [0.5, 0.0, 0.5, 1.0])]);
        assert_eq!(Node::Leaf("a".into()).without("a"), None);
    }

    /// Waits for the box to change, then returns its snapshot.
    fn next(local: &Local) -> Snapshot {
        let _ = local.wake.recv_timeout(Duration::from_secs(5));
        local.snapshot()
    }

    /// The text of the pane's screen.
    fn screen(s: &Snapshot) -> String {
        let Some(frame) = s.panes.first().and_then(|p| p.frame.as_ref()) else { return String::new() };
        (0..frame.rows())
            .map(|y| frame.row(usize::from(y)).map(|c| c.text()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_shell_runs_types_and_ends_with_its_pane() {
        let local = Local::start(u64::MAX);
        let mut s = local.snapshot();
        assert_eq!(s.sessions.len(), 1, "shown boxes open one session");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !s.panes.first().is_some_and(|p| p.live) && Instant::now() < deadline {
            s = next(&local);
        }
        let block = s.panes[0].block.clone();
        assert!(s.panes[0].live && s.panes[0].size == Size::Here);
        local.paste(&block, "echo cosmic-$((6*7))\n".into());
        let deadline = Instant::now() + Duration::from_secs(10);
        while !screen(&s).contains("cosmic-42") && Instant::now() < deadline {
            s = next(&local);
        }
        assert!(screen(&s).contains("cosmic-42"), "{}", screen(&s));

        let session = s.session.clone().unwrap();
        local.split(&session, &block, Split::Right);
        let s = local.snapshot();
        assert_eq!(s.tab_panes, 2);
        assert!(s.panes[1].focused && s.panes[0].rect[2] == 0.5);

        local.close_pane(&session, &s.panes[1].block);
        assert_eq!(local.snapshot().tab_panes, 1);
        // Exiting the shell closes its pane, its tab and its session. Typed, since a shell with
        // bracketed paste on keeps a pasted line for editing.
        for c in "exit".chars() {
            local.key(&block, &press(&format!("key_{c}"), "", &c.to_string()));
        }
        local.key(&block, &press("enter", "", "\n"));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut s = local.snapshot();
        while !s.sessions.is_empty() && Instant::now() < deadline {
            s = next(&local);
        }
        assert!(s.sessions.is_empty() && s.panes.is_empty());
        assert!(local.snapshot().sessions.is_empty(), "and none is opened again by itself");
    }

    #[test]
    fn input_to_a_pane_that_is_not_running_is_dropped() {
        let local = Local::start(u64::MAX - 1);
        local.set_visible(false);
        assert!(local.snapshot().sessions.is_empty(), "a hidden box opens nothing");
        local.key("local-block-9", &press("key_a", "", "a"));
        local.paste("local-block-9", "rm -rf x\n".into());
        assert!(local.ops.is_empty());
    }
}
