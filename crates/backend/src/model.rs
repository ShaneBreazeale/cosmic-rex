//! What a frontend draws: plain data derived from the core's model.
use crate::Frame;
use rex_rune::{
    connection::{
        DEFAULT_TAB_LABEL, HostModel,
        activity::{BlockedKind, ProgramState, StatusRecord},
    },
    layout,
};

/// In order of urgency.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentState {
    #[default]
    Idle,
    Working,
    Done,
    Error,
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitingFor {
    Permission,
    Question,
    Auth,
}

/// A program's own report about itself (OSC 7501). The text is untrusted terminal output: it is
/// already stripped and bounded, and must still only be shown as plain text.
#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub session: String,
    pub window: String,
    pub block: String,
    pub state: AgentState,
    pub waiting_for: Option<WaitingFor>,
    pub app: String,
    pub message: Option<String>,
    /// Session and tab, so a program cannot pose as one in another terminal.
    pub place: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: String,
    pub label: String,
    pub state: AgentState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub id: String,
    pub label: String,
}

/// Whose size a pane has. Everyone viewing it sees that size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    /// This client's.
    Here,
    /// Another client's: typing here must not take it.
    Elsewhere,
    #[default]
    Unclaimed,
}

#[derive(Clone)]
pub struct PaneView {
    pub block: String,
    /// x, y, width, height as fractions of the window.
    pub rect: [f64; 4],
    pub frame: Option<Frame>,
    pub status: String,
    pub focused: bool,
    pub title: String,
    pub size: Size,
    /// Output and input are both flowing. A frame without this is only the last one seen.
    pub live: bool,
    pub state: AgentState,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub connected: bool,
    pub status: String,
    pub notice: Option<String>,
    pub sessions: Vec<Session>,
    pub session: Option<String>,
    pub tabs: Vec<Tab>,
    pub tab: Option<String>,
    pub panes: Vec<PaneView>,
    /// Most urgent first.
    pub agents: Vec<Agent>,
}

impl Snapshot {
    pub fn focused(&self) -> Option<&str> {
        self.panes.iter().find(|p| p.focused).map(|p| p.block.as_str())
    }
    pub fn waiting(&self) -> usize {
        self.agents.iter().filter(|a| a.state == AgentState::Blocked).count()
    }

    pub(crate) fn new(m: &HostModel) -> Self {
        let agents = agents(m);
        let sessions = m
            .sessions
            .iter()
            .map(|s| Session {
                id: s.session_id.clone(),
                label: plain(&s.label, 60),
                state: agents
                    .iter()
                    .filter(|a| a.session == s.session_id)
                    .map(|a| a.state)
                    .max()
                    .unwrap_or_default(),
            })
            .collect();
        let window = m.window();
        let focused = window.and_then(|w| w.focused_block_id.as_deref());
        let panes = window.map_or_else(Vec::new, |w| layout::placements(w, None).0);
        Self {
            connected: m.connected,
            status: plain(&m.status, 300),
            notice: m.notice.as_deref().map(|n| plain(n, 300)),
            sessions,
            session: m.selected.clone(),
            tabs: m
                .view()
                .into_iter()
                .flat_map(|v| &v.windows)
                .filter_map(|w| {
                    Some(Tab { id: w.window_id.clone()?, label: tab_label(m, w) })
                })
                .collect(),
            tab: window.and_then(|w| w.window_id.clone()),
            panes: panes
                .into_iter()
                .map(|p| {
                    let (frame, status, live) = match m.terminals.get(&p.block) {
                        Some(c) => {
                            let t = c.model.lock().expect("terminal model");
                            (
                                t.frame.clone().map(Frame),
                                plain(&t.status, 300),
                                m.connected && t.connected && t.control_connected,
                            )
                        }
                        None => (None, "Not a terminal, or not connected yet".into(), false),
                    };
                    PaneView {
                        focused: focused == Some(p.block.as_str()),
                        title: pane_title(m, &p.block),
                        size: size(m, m.selected.as_deref().unwrap_or_default(), &p.block),
                        state: agents
                            .iter()
                            .find(|a| a.block == p.block)
                            .map(|a| a.state)
                            .unwrap_or_default(),
                        live,
                        block: p.block,
                        rect: p.rect,
                        frame,
                        status,
                    }
                })
                .collect(),
            agents,
        }
    }
}

fn pane_title(m: &HostModel, block: &str) -> String {
    let title = m.title(block).or(m.programs.get(block).map(String::as_str));
    plain(title.unwrap_or(DEFAULT_TAB_LABEL), 60)
}

pub(crate) fn size(m: &HostModel, session: &str, block: &str) -> Size {
    let owner = m
        .views
        .get(session)
        .into_iter()
        .flat_map(|v| &v.windows)
        .flat_map(|w| &w.layers)
        .flat_map(|l| &l.blocks)
        .find(|b| b.block_id.as_deref() == Some(block))
        .and_then(|b| b.owner.as_deref());
    match owner {
        None | Some("") => Size::Unclaimed,
        Some(id) if id == m.client_id => Size::Here,
        Some(_) => Size::Elsewhere,
    }
}

fn state(s: ProgramState) -> AgentState {
    match s {
        ProgramState::Idle => AgentState::Idle,
        ProgramState::Working => AgentState::Working,
        ProgramState::Done => AgentState::Done,
        ProgramState::Error => AgentState::Error,
        ProgramState::Blocked => AgentState::Blocked,
    }
}

fn tab_label(m: &HostModel, w: &rexrs::api::session::Window) -> String {
    let label = w.label.as_deref().unwrap_or_default();
    let title = w.focused_block_id.as_deref().and_then(|b| m.title(b));
    let text = if label.is_empty() || label == DEFAULT_TAB_LABEL {
        title.unwrap_or(DEFAULT_TAB_LABEL)
    } else {
        label
    };
    plain(text, 40)
}

fn agents(m: &HostModel) -> Vec<Agent> {
    let mut all = Vec::new();
    for (session, view) in &m.views {
        // The session list follows renames; a cached view can lag behind.
        let label = m
            .sessions
            .iter()
            .find(|s| &s.session_id == session)
            .map_or_else(|| view.label.clone().unwrap_or_default(), |s| s.label.clone());
        for w in &view.windows {
            let Some(window) = &w.window_id else { continue };
            for block in w.layers.iter().flat_map(|l| &l.blocks).filter_map(|b| b.block_id.as_ref())
            {
                let Some(r) = m
                    .activity
                    .get(block)
                    .and_then(|records| records.values().max_by_key(|r| state(r.state)))
                else {
                    continue;
                };
                all.push(agent(r, session, window, block, &label, &tab_label(m, w)));
            }
        }
    }
    all.sort_by(|a, b| b.state.cmp(&a.state).then_with(|| a.place.cmp(&b.place)));
    all
}

fn agent(
    r: &StatusRecord,
    session: &str,
    window: &str,
    block: &str,
    session_label: &str,
    tab: &str,
) -> Agent {
    Agent {
        session: session.to_owned(),
        window: window.to_owned(),
        block: block.to_owned(),
        state: state(r.state),
        waiting_for: r.kind.map(|k| match k {
            BlockedKind::Permission => WaitingFor::Permission,
            BlockedKind::Question => WaitingFor::Question,
            BlockedKind::Auth => WaitingFor::Auth,
        }),
        app: plain(r.app.as_deref().unwrap_or("program"), 32),
        message: r.msg.as_deref().or(r.title.as_deref()).map(|t| plain(t, 200)),
        place: format!("{} / {tab}", plain(session_label, 40)),
    }
}

/// Terminal text made safe to show in a label: no control, direction or invisible characters,
/// and bounded.
pub fn plain(text: &str, max: usize) -> String {
    let mut out: String = text
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c,
                    '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}'
                    | '\u{2060}'..='\u{2069}' | '\u{feff}')
        })
        .take(max + 1)
        .collect();
    if out.chars().count() > max {
        out.pop();
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Contract with the core: built from server JSON, so a core upgrade that changes parsing,
    /// layout or status handling fails here rather than in a frontend.
    fn model() -> HostModel {
        let block = |id: &str| json!({"block_id": id, "creator_name": "com.superlogical.terminal"});
        let view = json!({
            "revision": 1, "session_id": "session:a", "label": "stale", "active_window_id": "window:1",
            "windows": [{
                "window_id": "window:1", "label": "Shell", "focused_block_id": "block:2",
                "layers": [{
                    "layer_id": "layer:1", "kind": "tiled", "bounds": [0.0, 0.0, 1.0, 1.0],
                    "layout": {"rect": [0.0, 0.0, 1.0, 1.0], "split": {
                        "split_id": "split:1", "direction": "horizontal", "ratio": 0.25,
                        "before": {"rect": [0.0, 0.0, 0.25, 1.0], "block_id": "block:1"},
                        "after": {"rect": [0.25, 0.0, 0.75, 1.0], "block_id": "block:2"}}},
                    "blocks": [block("block:1"), block("block:2")]
                }]
            }]
        });
        let mut m = HostModel {
            connected: true,
            selected: Some("session:a".into()),
            sessions: vec![
                serde_json::from_value(json!({"session_id": "session:a", "label": "work"})).unwrap(),
                serde_json::from_value(json!({"session_id": "session:b", "label": "quiet"})).unwrap(),
            ],
            ..Default::default()
        };
        m.views.insert("session:a".into(), serde_json::from_value(view).unwrap());
        let record = |v| StatusRecord::parse(&v);
        m.activity.entry("block:1".into()).or_default().insert(
            String::new(),
            record(json!({"state": "working", "app": "cargo"})),
        );
        let two = m.activity.entry("block:2".into()).or_default();
        two.insert(String::new(), record(json!({"state": "idle", "app": "claude-code"})));
        two.insert(
            "worker".into(),
            record(json!({"state": "blocked", "kind": "permission", "app": "claude-code",
                          "msg": "Run tests\u{202e}?"})),
        );
        m
    }

    #[test]
    fn snapshot_places_panes_and_marks_focus() {
        let s = Snapshot::new(&model());
        let placed: Vec<_> =
            s.panes.iter().map(|p| (p.block.as_str(), p.rect, p.focused)).collect();
        assert_eq!(
            placed,
            [
                ("block:1", [0.0, 0.0, 0.25, 1.0], false),
                ("block:2", [0.25, 0.0, 0.75, 1.0], true)
            ]
        );
        assert_eq!(s.focused(), Some("block:2"));
        assert_eq!(s.tab.as_deref(), Some("window:1"));
    }

    #[test]
    fn agents_sort_by_urgency_and_take_a_blocks_worst_record() {
        let s = Snapshot::new(&model());
        assert_eq!(s.waiting(), 1);
        assert_eq!(s.agents[0].state, AgentState::Blocked);
        assert_eq!(s.agents[0].waiting_for, Some(WaitingFor::Permission));
        assert_eq!(s.agents[0].message.as_deref(), Some("Run tests?"));
        assert_eq!(s.agents[0].place, "work / Shell");
        assert_eq!(s.agents[1].app, "cargo");
        assert_eq!(s.sessions[0].state, AgentState::Blocked);
        assert_eq!(s.sessions[1].state, AgentState::Idle);
    }

    #[test]
    fn size_follows_the_owner_and_a_dead_link_is_never_live() {
        let mut m = model();
        m.client_id = "client:me".into();
        for (owner, want) in [
            (None, Size::Unclaimed),
            (Some("client:me"), Size::Here),
            (Some("client:mac"), Size::Elsewhere),
        ] {
            let view = m.views.get_mut("session:a").unwrap();
            view.windows[0].layers[0].blocks[1].owner = owner.map(str::to_owned);
            assert_eq!(Snapshot::new(&m).panes[1].size, want);
        }
        assert!(Snapshot::new(&m).panes.iter().all(|p| !p.live));
    }

    #[test]
    fn unknown_states_never_alarm() {
        let mut m = model();
        m.activity.clear();
        m.activity
            .entry("block:1".into())
            .or_default()
            .insert(String::new(), StatusRecord::parse(&json!({"state": "from-the-future"})));
        assert_eq!(Snapshot::new(&m).waiting(), 0);
    }

    #[test]
    fn plain_strips_and_bounds() {
        assert_eq!(plain("a\u{1b}[31m\u{202e}b\nc", 20), "a[31mbc");
        assert_eq!(plain("abcdef", 3), "abc…");
        assert_eq!(plain("abc", 3), "abc");
    }
}
