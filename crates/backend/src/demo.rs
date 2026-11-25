//! Local synthetic terminals through the real Ghostty engine, for looking at and timing the
//! renderer without a server.
use crate::{Frame, PaneView};
use ghostty_vt::{Terminal, terminal::Options};
use rex_rune::terminal::Renderer;
use std::{sync::Arc, time::Duration};

struct Source {
    block: &'static str,
    rect: [f64; 4],
    terminal: Terminal,
    renderer: Renderer,
    frame: Frame,
}

const CARGO: &str = "$ cargo test --workspace\r\n\
    \x1b[1;32m   Compiling\x1b[0m invoice v0.4.1\r\n\
    \x1b[1;32m    Finished\x1b[0m test profile in 8.42s\r\n\
    \x1b[1;32m     Running\x1b[0m unittests src/lib.rs\r\n\r\n\
    test routes::missing_id_is_404 ... \x1b[32mok\x1b[0m\r\n\
    test routes::totals_round ... \x1b[32mok\x1b[0m\r\n\
    test store::migrates ... ";

pub struct Demo {
    sources: Vec<Source>,
    ticks: u64,
    build: Duration,
}

impl Default for Demo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo {
    pub fn new() -> Self {
        let make = |block, rect, text: &str| {
            let mut terminal =
                Terminal::new(Options { cols: 80, rows: 24, max_scrollback: 1 << 20 })
                    .expect("terminal");
            terminal.vt_write(text.as_bytes());
            let mut renderer = Renderer::new().expect("renderer");
            let frame = Frame(Arc::new(renderer.frame(&terminal).expect("frame")));
            Source { block, rect, terminal, renderer, frame }
        };
        let styles = "\x1b[1mbold\x1b[0m \x1b[3mitalic\x1b[0m \x1b[4munderline\x1b[0m \
            \x1b[9mstrike\x1b[0m \x1b[7minverse\x1b[0m \x1b[2mfaint\x1b[0m\r\n\
            \x1b[38;2;255;120;0mtrue colour\x1b[0m \x1b[44;97m background \x1b[0m\r\n\
            |日本語|wide|\r\n|abcdef|ascii|\r\n\
            ┌──────┬──────┐\r\n│ box  │ draw │\r\n└──────┴──────┘\r\n\
            => != -> :: |> fi fl\r\n$ ";
        Self {
            sources: vec![
                make("demo:styles", [0.0, 0.0, 0.4, 1.0], styles),
                make("demo:busy", [0.4, 0.0, 0.6, 1.0], CARGO),
            ],
            ticks: 0,
            build: Duration::ZERO,
        }
    }

    /// Full-screen coloured output every tick: worse than any agent, about a busy `cat`.
    pub fn tick(&mut self) {
        self.ticks += 1;
        let busy = &mut self.sources[1];
        let mut out = String::new();
        for i in 0..busy.frame.rows() as u64 {
            let n = self.ticks * 7 + i;
            out.push_str(&format!("\x1b[38;5;{}m{n:08} ", 16 + n % 216));
            out.push_str(&"the quick brown fox jumps over the lazy dog ".repeat(3));
            out.truncate(out.len().saturating_sub(1));
            out.push_str("\x1b[0m\r\n");
        }
        busy.terminal.vt_write(out.as_bytes());
        let start = std::time::Instant::now();
        busy.frame = Frame(Arc::new(busy.renderer.frame(&busy.terminal).expect("frame")));
        self.build += start.elapsed();
    }

    pub fn resize(&mut self, block: &str, cols: u16, rows: u16) {
        for s in self.sources.iter_mut().filter(|s| s.block == block) {
            if s.terminal.resize(cols, rows, 8, 18).is_ok() {
                s.frame = Frame(Arc::new(s.renderer.frame(&s.terminal).expect("frame")));
            }
        }
    }

    pub fn panes(&self) -> Vec<PaneView> {
        self.sources
            .iter()
            .enumerate()
            .map(|(i, s)| PaneView {
                block: s.block.into(),
                rect: s.rect,
                frame: Some(s.frame.clone()),
                status: String::new(),
                focused: i == 0,
                title: ["claude-code", "cargo test"][i].into(),
                size: [crate::Size::Elsewhere, crate::Size::Here][i],
                live: true,
                guess: String::new(),
                state: [crate::AgentState::Blocked, crate::AgentState::Working][i],
            })
            .collect()
    }

    pub fn report(&self, wall: Duration, shaped: u64) {
        let f = &self.sources[1].frame;
        println!(
            "wall {:.1}s, {} updates ({:.1}/s), grid {}x{}, rows shaped {shaped}, frame build {:.2} ms each",
            wall.as_secs_f64(),
            self.ticks,
            self.ticks as f64 / wall.as_secs_f64(),
            f.cols(),
            f.rows(),
            self.build.as_secs_f64() * 1000.0 / self.ticks.max(1) as f64,
        );
    }
}

/// Invented boxes, sessions and agents.
pub fn sample() -> (Vec<crate::BoxConfig>, Vec<(String, crate::Snapshot)>) {
    use crate::{Agent, AgentState as S, BoxConfig, Session, Snapshot, Tab, WaitingFor};
    let config = |n: u8, label: &str, monogram: &str| BoxConfig {
        id: format!("box-{n}"),
        label: label.into(),
        monogram: monogram.into(),
        tint: n - 1,
        endpoint: format!("ssh://demo@{label}"),
        connect_on_start: false,
    };
    let session = |id: &str, state| Session {
        id: id.into(),
        label: id.into(),
        state,
        about: "zsh ~/src".into(),
    };
    let agent = |session: &str, tab: &str, state, waiting_for, app: &str, message: &str| Agent {
        session: session.into(),
        window: tab.into(),
        block: format!("{session}:{tab}"),
        state,
        waiting_for,
        app: app.into(),
        message: Some(message.into()),
        place: format!("{session} / {tab}"),
        since: std::time::Instant::now()
            .checked_sub(Duration::from_secs(60 * (2 + message.len() as u64 % 9))),
        progress: (state == S::Working).then_some(35),
    };
    let tab = |id: &str| Tab { id: id.into(), label: id.into() };
    let studio = Snapshot {
        connected: true,
        sessions: vec![
            session("api-refactor", S::Blocked),
            session("billing-migration", S::Working),
            session("docs-site", S::Idle),
        ],
        session: Some("api-refactor".into()),
        tabs: vec![tab("tests"), tab("server"), tab("shell")],
        tab: Some("tests".into()),
        agents: vec![
            agent("api-refactor", "tests", S::Blocked, Some(WaitingFor::Permission), "claude-code", "Run cargo test --workspace?"),
            agent("billing-migration", "run", S::Working, None, "codex", "Updating the invoice schema"),
        ],
        ..Default::default()
    };
    let forge = Snapshot {
        connected: true,
        sessions: vec![session("infra", S::Blocked), session("nightly-evals", S::Done)],
        session: Some("infra".into()),
        agents: vec![
            agent("infra", "plan", S::Blocked, Some(WaitingFor::Question), "codex", "Two modules define the same bucket. Which one should own it?"),
            agent("infra", "apply", S::Error, None, "terraform", "Provider returned 403 for staging"),
            agent("nightly-evals", "run", S::Done, None, "claude-code", "412 of 412 cases passed"),
        ],
        ..Default::default()
    };
    let attic = Snapshot {
        status: "Connection refused".into(),
        sessions: vec![session("backups", S::Idle)],
        ..Default::default()
    };
    (
        vec![config(1, "studio", "ST"), config(2, "forge", "FG"), config(3, "attic", "AT")],
        vec![("box-1".into(), studio), ("box-2".into(), forge), ("box-3".into(), attic)],
    )
}

/// An invented question to look at.
pub fn question(agent: &crate::Agent) -> crate::Question {
    let screen = [
        "● Updated src/routes/invoice.rs",
        "",
        "  Run command",
        "    cargo test --workspace",
        "",
        "  Do you want to proceed?",
        "  ❯ 1. Yes",
        "    2. Yes, and do not ask again for cargo test",
        "    3. No, and say what to do instead",
    ];
    crate::Question {
        waiting_for: agent.waiting_for,
        app: agent.app.clone(),
        message: agent.message.clone(),
        screen: screen.map(str::to_owned).to_vec(),
        ticket: crate::Ticket::sample(),
    }
}
