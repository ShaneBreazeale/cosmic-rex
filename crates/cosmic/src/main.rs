//! Rex for COSMIC: a libcosmic frontend over `rex-backend`.
mod claim;
mod grid;
mod keys;
mod reorder;
mod ui;

use cosmic::{
    Application, ApplicationExt as _, Core, Element,
    app::{Settings, Task, context_drawer},
    iced::{Alignment, Length, Size, Subscription, clipboard},
    widget::{self, nav_bar},
};
use futures_util::StreamExt as _;
use grid::{Action, Cache, Grid, Identity, Metrics, Shortcut, Tab};
use rex_backend::{
    Agent, AgentState, AnswerKey, BoxConfig, Boxes, Host, Outcome, PaneView, Question, Reply, Snapshot, Split, Wake,
    demo::Demo,
};
use std::{
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};

/// How long a confirmed tab close hides the tab while the server drops it. If it is still there
/// after this, it shows again as it is.
/// The terminal font sizes allowed, and one step of the View menu's.
const FONT_MIN: f32 = 6.0;
const FONT_MAX: f32 = 72.0;
const FONT_STEP: f32 = 1.0;
const CLOSE_WAIT: Duration = Duration::from_secs(5);

#[derive(clap::Parser, Clone)]
#[command(version, about = "Rex for COSMIC (unofficial)")]
struct Args {
    /// Open this server for this run only, without saving it: ssh://user@host, https://host,
    /// or unix:///path
    #[arg(long)]
    endpoint: Option<String>,
    /// Session to open there, by ID or exact label
    #[arg(long, requires = "endpoint")]
    session: Option<String>,
    /// Terminal font size in pixels
    #[arg(long, default_value_t = 14.0)]
    font_size: f32,
    /// Draw local synthetic terminals; connects to nothing
    #[arg(long)]
    demo: bool,
    /// With --demo: flood a pane with changing colours at 60 updates a second, to time drawing
    #[arg(long, requires = "demo")]
    stress: bool,
    /// Exit after this many seconds (with --stress, print draw statistics)
    #[arg(long)]
    bench: Option<u64>,
    /// Wait for the server to echo typing instead of showing it at once
    #[arg(long)]
    no_local_echo: bool,
    /// With --demo: open an answer sheet
    #[arg(long, requires = "demo")]
    ask: bool,
    /// Start with the inbox open
    #[arg(long)]
    inbox: bool,
    /// Start with the sidebar closed
    #[arg(long)]
    quiet: bool,
    /// With --bench: save this window (only) as a PPM image before exiting
    #[arg(long, requires = "bench")]
    screenshot: Option<std::path::PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use clap::Parser as _;
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args = Args::parse();
    if !args.font_size.is_finite() || !(FONT_MIN..=FONT_MAX).contains(&args.font_size) {
        return Err("--font-size must be between 6 and 72".into());
    }
    rex_backend::init();
    rex_backend::set_local_echo(!args.no_local_echo);
    let settings = Settings::default().size(Size::new(1280.0, 800.0));
    cosmic::app::run::<App>(settings, args)?;
    // Let host workers close their control connections, so the server drops this client at once.
    rex_backend::shutdown(Duration::from_secs(2));
    Ok(())
}

#[derive(Clone, Debug)]
enum Message {
    /// Something changed on this box: take a new snapshot of it.
    Wake(String),
    Grid(Action),
    SelectBox(String),
    OpenSession(String, String),
    Tab(String),
    /// A tab's menu opened: its commands act on this tab.
    TabMenu(String),
    /// The pointer came onto this tab.
    HoverTab(String),
    /// The pointer left this tab. Clears only this one: entering the next may come first.
    LeaveTab(String),
    TabCommand(ui::TabCommand),
    PaneCommand(ui::PaneCommand),
    View(ui::ViewCommand),
    AskCloseTab(String),
    CloseTab { id: String, session: String, window: String },
    ClosePane { id: String, session: String, block: String },
    RenameDraft(String),
    RenameTab,
    /// The close question has been readable for a moment.
    DialogArmed(u64),
    Surface(cosmic::surface::Action<Message>),
    /// A session's menu opened, on this box: its commands act on this session.
    SessionMenu(String, String),
    SessionCommand(ui::SessionCommand),
    /// Put the session at this slot in its box's list: 0 is the top.
    MoveSession { id: String, session: String, slot: usize },
    CloseSession { id: String, session: String },
    RenameSession,
    OpenAgent(String, String, String),
    Pasted(String, String, Option<String>),
    ToggleInbox,
    AddBox,
    Draft(Field, String),
    SaveBox,
    CancelDialog,
    AskRemove(String),
    EditBox(String),
    Remove(String),
    Fit(String, String, String),
    Ask(String, Box<Agent>),
    Asked(u64, Result<Question, String>),
    AnswerArmed(u64),
    AnswerDraft(String),
    AnswerKey(AnswerKey),
    AnswerLine,
    Answered(u64, Outcome),
    OpenAsked,
    Connect(String),
    Disconnect(String),
    /// Waiting times on screen have moved on.
    Minute,
    /// The window's screen pixels per layout pixel, from the compositor.
    Scale(f32),
    DemoTick,
    BenchDone,
    Captured(cosmic::iced::window::Screenshot),
}

#[derive(Clone, Copy, Debug)]
enum Field {
    Label,
    Address,
}

/// What has the keyboard instead of the terminals.
enum Dialog {
    /// Adding a box, or editing the one with this id.
    Add { label: String, address: String, error: Option<String>, editing: Option<String> },
    Remove(BoxConfig),
    Fit { config: BoxConfig, session: String, block: String },
    Answer(Box<Answer>),
    /// Ends programs, so it asks, and Close stays inert until `armed`.
    CloseTab { config: BoxConfig, session: String, window: String, label: String, number: u64, armed: bool },
    Rename { config: BoxConfig, session: String, window: String, label: String },
    /// Ends the pane's program, so it asks, and Close stays inert until `armed`.
    ClosePane { config: BoxConfig, session: String, block: String, label: String, number: u64, armed: bool },
    /// Ends every tab in the session, so it asks, and Close stays inert until `armed`.
    CloseSession { config: BoxConfig, session: String, label: String, tabs: Option<usize>, number: u64, armed: bool },
    RenameSession { config: BoxConfig, session: String, label: String },
}

/// An open answer sheet. Results carry its number, so a late one can't land in another sheet.
struct Answer {
    number: u64,
    config: BoxConfig,
    agent: Agent,
    question: Option<Result<Question, String>>,
    /// The question has been readable for a moment.
    armed: bool,
    reply: String,
    sending: bool,
    outcome: Option<String>,
}

struct App {
    core: Core,
    args: Args,
    /// Only tells libcosmic that there is a sidebar; `nav_bar` draws it.
    nav: nav_bar::Model,
    boxes: Boxes,
    now: Vec<(String, Snapshot)>,
    /// The box on screen. Only this one draws terminals.
    current: Option<String>,
    dialog: Option<Dialog>,
    /// The box list could not be read or written.
    problem: Option<String>,
    demo: Option<Demo>,
    cache: Cache,
    metrics: Metrics,
    /// The terminal font size asked for: `--font-size` until the View menu changes it.
    font_size: f32,
    title: String,
    /// Answer sheets and close questions opened so far.
    sheets: u64,
    /// The tab whose right-click menu opened last.
    menu_tab: Option<String>,
    hovered_tab: Option<String>,
    /// The box and session whose right-click menu opened last.
    menu_session: Option<(String, String)>,
    /// Tabs whose close was confirmed and sent, by box, until the server drops them: their
    /// programs have ended, so they are not drawn as tabs or as paused screens meanwhile.
    closing: Vec<(String, String, Instant)>,
    /// Panes whose close was confirmed and sent, by box, until the server drops them.
    closing_panes: Vec<(String, String, Instant)>,
    /// The pane whose right-click menu opened last, on the box on screen.
    menu_pane: Option<String>,
    /// A selection just made, by box, pane and revision: its text goes to the primary
    /// selection once a frame carries it.
    primary: Option<(String, String, u64)>,
    /// Why the last copy did not happen.
    copy_problem: Option<String>,
    started: Instant,
}

impl App {
    fn sheet(&mut self) -> Option<&mut Answer> {
        match &mut self.dialog {
            Some(Dialog::Answer(a)) => Some(a),
            _ => None,
        }
    }

    fn send_answer(&mut self, reply: Reply) -> Task<Message> {
        let Some(a) = self.sheet().filter(|a| a.armed && !a.sending && a.outcome.is_none()) else {
            return Task::none();
        };
        let Some(Ok(question)) = &a.question else { return Task::none() };
        let (number, ticket, id) = (a.number, question.ticket.clone(), a.config.id.clone());
        a.sending = true;
        let Some(asker) = self.boxes.host(&id).map(Host::asker) else {
            return self.update(Message::Answered(number, Outcome::NotSent("The box is disconnected".into())));
        };
        Task::perform(asker.answer(ticket, reply), move |o| {
            cosmic::Action::App(Message::Answered(number, o))
        })
    }

    fn snapshot(&self, id: &str) -> Option<&Snapshot> {
        self.now.iter().find(|(b, _)| b == id).map(|(_, s)| s)
    }
    fn here(&self) -> Option<(&BoxConfig, &Snapshot)> {
        let id = self.current.as_deref()?;
        Some((self.boxes.config(id)?, self.snapshot(id)?))
    }
    /// A tab of the box on screen whose close is on its way.
    fn closing(&self, window: &str) -> bool {
        let Some(id) = self.current.as_deref() else { return false };
        self.closing.iter().any(|(b, w, sent)| b == id && w == window && sent.elapsed() < CLOSE_WAIT)
    }
    fn host(&self) -> Option<&Host> {
        self.boxes.host(self.current.as_deref()?)
    }
    fn pane(&self, block: &str) -> Option<&PaneView> {
        self.here()?.1.panes.iter().find(|p| p.block == block)
    }

    /// Put this pane's selection on the clipboard. Nothing is selected: nothing happens.
    fn copy(&mut self, block: &str) -> Task<Message> {
        let Some(frame) = self.pane(block).and_then(|p| p.frame.clone()) else { return Task::none() };
        self.copy_problem = frame.selection_error().map(|e| format!("Not copied: {e}"));
        frame.selected_text().map_or_else(Task::none, |text| clipboard::write(text.to_owned()))
    }

    /// The selection just made, once a frame of it has arrived, as the primary selection.
    fn offer_primary(&mut self) -> Task<Message> {
        let Some((id, block, revision)) = self.primary.clone() else { return Task::none() };
        let Some(frame) = self
            .snapshot(&id)
            .and_then(|s| s.panes.iter().find(|p| p.block == block))
            .and_then(|p| p.frame.clone())
        else {
            self.primary = None;
            return Task::none();
        };
        if frame.selection_revision() < revision {
            return Task::none();
        }
        self.primary = None;
        frame.selected_text().map_or_else(Task::none, |text| clipboard::write_primary(text.to_owned()))
    }

    /// Take new snapshots of every box.
    fn refresh(&mut self) -> Task<Message> {
        if self.demo.is_some() {
            return Task::none();
        }
        self.now = self.boxes.snapshots();
        let keep: Vec<String> =
            self.here().map_or_else(Vec::new, |(_, s)| s.panes.iter().map(|p| p.block.clone()).collect());
        self.cache.retain(|block| keep.iter().any(|b| b == block));
        // Forget a close once the server has dropped the tab, or has had long enough to.
        let now = &self.now;
        self.closing.retain(|(id, window, sent)| {
            sent.elapsed() < CLOSE_WAIT
                && now.iter().any(|(b, s)| b == id && s.tabs.iter().any(|t| &t.id == window))
        });
        self.closing_panes.retain(|(id, block, sent)| {
            sent.elapsed() < CLOSE_WAIT
                && now.iter().any(|(b, s)| b == id && s.panes.iter().any(|p| &p.block == block))
        });
        self.retitle()
    }

    /// Session, then box: what Alt-Tab and the workspace overview show.
    fn retitle(&mut self) -> Task<Message> {
        let title = self.here().map_or_else(
            || "cosmic-rex".to_owned(),
            |(c, s)| match s.sessions.iter().find(|x| Some(&x.id) == s.session.as_ref()) {
                Some(session) => format!("{} · {} · cosmic-rex", session.label, c.label),
                None => format!("{} · cosmic-rex", c.label),
            },
        );
        if title == self.title {
            return Task::none();
        }
        let Some(window) = self.core.main_window_id() else { return Task::none() };
        self.title.clone_from(&title);
        self.set_window_title(title, window)
    }

    fn show_box(&mut self, id: &str) -> Task<Message> {
        if self.current.as_deref() != Some(id) {
            if let Some(host) = self.host() {
                host.set_visible(false);
            }
            self.current = Some(id.to_owned());
            if let Some(host) = self.host() {
                host.set_visible(true);
            }
        }
        self.retitle()
    }

    fn panes(&self) -> Vec<PaneView> {
        match &self.demo {
            Some(demo) => demo.panes(),
            None => self.here().map_or_else(Vec::new, |(_, s)| s.panes.clone()),
        }
    }

    fn waiting(&self) -> usize {
        self.now
            .iter()
            .flat_map(|(_, s)| &s.agents)
            .filter(|a| matches!(a.state, AgentState::Blocked | AgentState::Error))
            .count()
    }

    /// Every box's agents as one queue.
    fn inbox(&self) -> Vec<(&BoxConfig, &Snapshot, &Agent)> {
        let mut all: Vec<_> = self
            .now
            .iter()
            .filter_map(|(id, s)| Some((self.boxes.config(id)?, s)))
            .flat_map(|(c, s)| s.agents.iter().map(move |a| (c, s, a)))
            .collect();
        all.sort_by(|a, b| Agent::by_urgency(a.2, b.2));
        all
    }

    fn ask_fit(&mut self, block: String) {
        if let Some((config, now)) = self.here()
            && let Some(session) = now.session.clone()
        {
            self.dialog = Some(Dialog::Fit { config: config.clone(), session, block });
        }
    }

    fn shortcut(&mut self, shortcut: Shortcut) -> Task<Message> {
        self.shortcut_on(shortcut, None)
    }

    /// On `pane` if given, else on the pane with the keyboard.
    fn shortcut_on(&mut self, shortcut: Shortcut, pane: Option<&str>) -> Task<Message> {
        match shortcut {
            Shortcut::Agents => return self.update(Message::ToggleInbox),
            Shortcut::FontBigger => return self.update(Message::View(ui::ViewCommand::Bigger)),
            Shortcut::FontSmaller => return self.update(Message::View(ui::ViewCommand::Smaller)),
            Shortcut::FontReset => return self.update(Message::View(ui::ViewCommand::Reset)),
            _ => {}
        }
        let Some((config, now)) = self.here() else { return Task::none() };
        let (Some(host), Some(session), Some(block)) =
            (self.host(), now.session.as_deref(), pane.or(now.focused()))
        else {
            return Task::none();
        };
        match shortcut {
            Shortcut::Copy => {
                let block = block.to_owned();
                return self.copy(&block);
            }
            Shortcut::Paste => {
                // Bound to the box and pane focused now, not whichever is when the read returns.
                let (id, block) = (config.id.clone(), block.to_owned());
                return clipboard::read().map(move |t| {
                    cosmic::Action::App(Message::Pasted(id.clone(), block.clone(), t))
                });
            }
            Shortcut::Fit => {
                let block = block.to_owned();
                self.ask_fit(block);
            }
            Shortcut::Zoom => host.zoom(session, block),
            Shortcut::ClosePane => {
                // The session's only pane is closed with its tab or the session, never here.
                if now.tab_panes > 1 || now.tabs.len() > 1 {
                    let label = now.panes.iter().find(|p| p.block == block).map_or_else(String::new, |p| p.title.clone());
                    let (config, session, block) = (config.clone(), session.to_owned(), block.to_owned());
                    self.sheets += 1;
                    let number = self.sheets;
                    self.dialog = Some(Dialog::ClosePane { config, session, block, label, number, armed: false });
                    // Long enough that a click meant for the button can't land on Close.
                    return Task::perform(rex_backend::pause(Duration::from_millis(300)), move |()| {
                        cosmic::Action::App(Message::DialogArmed(number))
                    });
                }
            }
            Shortcut::NewTab => host.new_tab(session, block),
            Shortcut::SplitRight => host.split(session, block, Split::Right),
            Shortcut::SplitDown => host.split(session, block, Split::Down),
            Shortcut::Agents | Shortcut::FontBigger | Shortcut::FontSmaller | Shortcut::FontReset => {}
        }
        Task::none()
    }
}

/// The backend coalesces wakes, so a slow UI never queues up stale frames.
struct Wakes(String, Wake);
impl Hash for Wakes {
    fn hash<H: Hasher>(&self, h: &mut H) {
        (&self.0, &self.1.id).hash(h);
    }
}
fn wakes(w: &Wakes) -> cosmic::iced::futures::stream::BoxStream<'static, Message> {
    let id = w.0.clone();
    w.1.signal.clone().into_stream().map(move |()| Message::Wake(id.clone())).boxed()
}

impl Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = Args;
    type Message = Message;
    const APP_ID: &'static str = "io.github.cosmic_rex.CosmicRex";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, args: Args) -> (Self, Task<Message>) {
        let mut problem = None;
        let (mut boxes, now) = if args.demo {
            let (configs, now) = rex_backend::demo::sample();
            (Boxes::temporary(configs), now)
        } else if let Some(endpoint) = &args.endpoint {
            let configs = match BoxConfig::new("", endpoint, &[]) {
                Ok(config) => vec![config],
                Err(error) => {
                    problem = Some(error);
                    Vec::new()
                }
            };
            (Boxes::temporary(configs), Vec::new())
        } else {
            let loaded = Boxes::default_path()
                .ok_or_else(|| "No home folder to keep the box list in".to_owned())
                .and_then(|path| Boxes::load(&path));
            let boxes = loaded.unwrap_or_else(|error| {
                problem = Some(error);
                Boxes::temporary(Vec::new())
            });
            (boxes, Vec::new())
        };
        let first = boxes.configs().first().map(|c| c.id.clone());
        if !args.demo {
            if let Some(id) = first.as_deref().filter(|_| args.session.is_some()) {
                boxes.connect(id, args.session.clone());
            }
            boxes.connect_on_start();
            for c in boxes.configs().iter().skip(1) {
                if let Some(host) = boxes.host(&c.id) {
                    host.set_visible(false);
                }
            }
        }
        let mut app = Self {
            metrics: Metrics::measure(args.font_size, 1.0),
            font_size: args.font_size,
            demo: args.demo.then(Demo::new),
            nav: nav_bar::Model::default(),
            boxes,
            now,
            current: first,
            dialog: None,
            problem,
            cache: Cache::default(),
            title: String::new(),
            sheets: 0,
            menu_tab: None,
            hovered_tab: None,
            menu_session: None,
            closing: Vec::new(),
            closing_panes: Vec::new(),
            menu_pane: None,
            primary: None,
            copy_problem: None,
            started: Instant::now(),
            core,
            args,
        };
        app.core.window.context_is_overlay = false;
        app.core.window.show_context = app.args.inbox;
        if app.args.quiet && !app.args.demo {
            app.core.nav_bar_set_toggled(false);
        }
        let mut task = app.refresh();
        if let Some(id) = app.core.main_window_id() {
            task = Task::batch([task, cosmic::iced::window::scale_factor(id).map(|f| cosmic::Action::App(Message::Scale(f)))]);
        }
        if app.args.ask
            && let Some((id, agent)) = app.inbox().first().map(|(c, _, a)| (c.id.clone(), (*a).clone()))
        {
            task = app.update(Message::Ask(id, Box::new(agent)));
        }
        (app, task)
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(&self.nav)
    }

    fn nav_bar(&self) -> Option<Element<'_, cosmic::Action<Message>>> {
        if !self.core.nav_bar_active() {
            return None;
        }
        let rows = self
            .configs()
            .map(|config| ui::BoxRow { now: self.snapshot(&config.id), config })
            .collect();
        Some(ui::sidebar(rows, self.current.as_deref()).map(cosmic::Action::App))
    }

    // In the start slot: text in libcosmic's centre slot, which is laid out twice, isn't drawn.
    fn header_start(&self) -> Vec<Element<'_, Message>> {
        let view = ui::view_menu(ui::ViewMenu {
            bigger: self.font_size < FONT_MAX,
            smaller: self.font_size > FONT_MIN,
            reset: self.font_size != self.args.font_size,
        });
        let Some((config, now)) = self.here() else { return vec![view] };
        let session = now.sessions.iter().find(|s| Some(&s.id) == now.session.as_ref());
        let mut out = vec![
            view,
            widget::space::horizontal().width(8).into(),
            ui::monogram(config),
            widget::space::horizontal().width(4).into(),
            ui::place(session.map(|s| s.label.as_str()), &config.label),
        ];
        // Tabs live in the title bar, as pills, whether or not the sidebar is open.
        if !now.tabs.is_empty() {
            out.push(widget::space::horizontal().width(12).into());
            out.push(ui::tabs(
                (now.tabs.iter())
                    .filter(|t| !self.closing(&t.id))
                    .map(|t| ui::TabItem { id: &t.id, label: &t.label, active: now.tab.as_ref() == Some(&t.id) })
                    .collect(),
                self.hovered_tab.as_deref(),
                self.can_open_tab().then_some(Message::Grid(Action::Shortcut(Shortcut::NewTab))),
            ));
        }
        out
    }

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        let waiting = self.waiting();
        let button = if waiting == 0 {
            widget::button::standard("Inbox")
        } else {
            widget::button::standard(format!("◆ {waiting} need you"))
        };
        vec![button.on_press(Message::ToggleInbox).into()]
    }

    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Message>> {
        self.core.window.show_context.then(|| {
            context_drawer::context_drawer(ui::inbox(self.inbox()), Message::ToggleInbox)
                .title("Inbox")
        })
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        let cancel = || widget::button::standard("Cancel").on_press(Message::CancelDialog);
        Some(match self.dialog.as_ref()? {
            Dialog::Add { label, address, error, editing } => {
                let mut form = widget::column::with_capacity(4)
                    .spacing(12)
                    .push(
                        widget::text_input("user@host, https://… or unix:///…", address.as_str())
                            .label("Address")
                            .on_input(|v| Message::Draft(Field::Address, v))
                            .on_submit(|_| Message::SaveBox),
                    )
                    .push(
                        widget::text_input("Taken from the address", label.as_str())
                            .label("Name")
                            .on_input(|v| Message::Draft(Field::Label, v))
                            .on_submit(|_| Message::SaveBox),
                    );
                if let Some(error) = error {
                    form = form.push(widget::text::body(error.as_str()));
                }
                let (title, action) =
                    if editing.is_some() { ("Edit box", "Save") } else { ("Add a box", "Add box") };
                widget::dialog()
                    .title(title)
                    .body("Connecting gives this app everything your account can do on that machine. It starts, stops and resizes nothing by itself.")
                    .control(form)
                    .primary_action(widget::button::suggested(action).on_press(Message::SaveBox))
                    .secondary_action(cancel())
                    .into()
            }
            Dialog::Answer(a) => ui::answer(ui::Sheet {
                config: &a.config,
                agent: &a.agent,
                question: a.question.as_ref().map(|q| q.as_ref().map_err(String::as_str)),
                reply: &a.reply,
                outcome: a.outcome.as_deref().or(a.sending.then_some("Sending…")),
                armed: a.armed && !a.sending && a.outcome.is_none(),
            }),
            Dialog::CloseTab { config, session, window, label, armed, .. } => widget::dialog()
                .title(format!("Close tab \u{2018}{label}\u{2019} on {}?", config.label))
                .body(format!(
                    "Ends every program running in this tab on {}, for every device viewing it. Closing this window instead ends nothing.",
                    config.label
                ))
                .primary_action(widget::button::suggested("Keep running").on_press(Message::CancelDialog))
                .secondary_action(widget::button::destructive("Close tab").on_press_maybe(armed.then(|| {
                    Message::CloseTab { id: config.id.clone(), session: session.clone(), window: window.clone() }
                })))
                .into(),
            Dialog::ClosePane { config, session, block, label, armed, .. } => widget::dialog()
                .title(format!("Close pane \u{2018}{label}\u{2019} on {}?", config.label))
                .body(format!(
                    "Ends the program running in this pane on {}, for every device viewing it. The tab's other panes keep running.",
                    config.label
                ))
                .primary_action(widget::button::suggested("Keep running").on_press(Message::CancelDialog))
                .secondary_action(widget::button::destructive("Close pane").on_press_maybe(armed.then(|| {
                    Message::ClosePane { id: config.id.clone(), session: session.clone(), block: block.clone() }
                })))
                .into(),
            Dialog::Rename { config, label, .. } => widget::dialog()
                .title("Rename tab")
                .body(format!("Every device viewing this session on {} sees the new name.", config.label))
                .control(
                    widget::text_input("Leave empty for its default name", label.as_str())
                        .label("Name")
                        .on_input(Message::RenameDraft)
                        .on_submit(|_| Message::RenameTab),
                )
                .primary_action(widget::button::suggested("Rename").on_press(Message::RenameTab))
                .secondary_action(cancel())
                .into(),
            Dialog::CloseSession { config, session, label, tabs, armed, .. } => widget::dialog()
                .title(format!("Close session \u{2018}{label}\u{2019} on {}?", config.label))
                .body(format!(
                    "Ends every program in {} on {}, for every device viewing it. Closing this window instead ends nothing.",
                    match tabs {
                        Some(1) => "its one tab".to_owned(),
                        Some(n) => format!("its {n} tabs"),
                        None => "all its tabs".to_owned(),
                    },
                    config.label
                ))
                .primary_action(widget::button::suggested("Keep running").on_press(Message::CancelDialog))
                .secondary_action(widget::button::destructive("Close session").on_press_maybe(
                    armed.then(|| Message::CloseSession { id: config.id.clone(), session: session.clone() }),
                ))
                .into(),
            Dialog::RenameSession { config, label, .. } => widget::dialog()
                .title("Rename session")
                .body(format!("Every device viewing {} sees the new name.", config.label))
                .control(
                    widget::text_input("Session name", label.as_str())
                        .label("Name")
                        .on_input(Message::RenameDraft)
                        .on_submit(|_| Message::RenameSession),
                )
                .primary_action(
                    widget::button::suggested("Rename")
                        .on_press_maybe((!label.trim().is_empty()).then_some(Message::RenameSession)),
                )
                .secondary_action(cancel())
                .into(),
            Dialog::Remove(config) => widget::dialog()
                .title(format!("Remove {}?", config.label))
                .body("It is only forgotten here. Everything running on it keeps running.")
                .primary_action(
                    widget::button::destructive("Remove")
                        .on_press(Message::Remove(config.id.clone())),
                )
                .secondary_action(cancel())
                .into(),
            Dialog::Fit { config, session, block } => widget::dialog()
                .title("Fit this pane here?")
                .body(format!(
                    "It re-wraps for every device viewing this pane on {}, including a Mac or phone.",
                    config.label
                ))
                .primary_action(widget::button::suggested("Fit here").on_press(Message::Fit(
                    config.id.clone(),
                    session.clone(),
                    block.clone(),
                )))
                .secondary_action(cancel())
                .into(),
        })
    }

    fn on_escape(&mut self) -> Task<Message> {
        self.dialog = None;
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut all: Vec<_> = self
            .boxes
            .wakes()
            .into_iter()
            .map(|(id, w)| Subscription::run_with(Wakes(id, w), wakes))
            .collect();
        // A guess the server never confirms must not stay on screen.
        if let Some(id) = &self.current
            && self.panes().iter().any(|p| !p.guess.is_empty())
        {
            let id = id.clone();
            all.push(
                cosmic::iced::time::every(Duration::from_millis(200))
                    .with(id)
                    .map(|(id, _)| Message::Wake(id)),
            );
        }
        all.push(cosmic::iced::time::every(Duration::from_secs(30)).map(|_| Message::Minute));
        all.push(cosmic::iced::event::listen_with(|event, _, _| match event {
            cosmic::iced::Event::Window(cosmic::iced::window::Event::Rescaled(f)) => Some(Message::Scale(f)),
            _ => None,
        }));
        if self.args.stress {
            all.push(cosmic::iced::time::every(Duration::from_millis(16)).map(|_| Message::DemoTick));
        }
        if let Some(seconds) = self.args.bench {
            all.push(
                cosmic::iced::time::every(Duration::from_secs(seconds)).map(|_| Message::BenchDone),
            );
        }
        Subscription::batch(all)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Wake(id) => {
                let (Some(new), Some(at)) =
                    (self.boxes.snapshot(&id), self.now.iter().position(|(b, _)| *b == id))
                else {
                    return self.refresh();
                };
                self.now[at].1 = new;
                // Only the box on screen has a title and a glyph cache to keep in step.
                if self.current.as_ref() == Some(&id) {
                    let panes = &self.now[at].1.panes;
                    self.cache.retain(|block| panes.iter().any(|p| p.block == block));
                    let primary = self.offer_primary();
                    return Task::batch([self.retitle(), primary]);
                }
            }
            Message::Grid(Action::Shortcut(s)) => return self.shortcut(s),
            Message::Grid(Action::Fit(block)) => self.ask_fit(block),
            Message::Grid(Action::Pane(block, s)) => return self.shortcut_on(s, Some(&block)),
            Message::Grid(Action::Resize(block, cols, rows)) => {
                let cell = self.metrics.cell;
                if let Some(demo) = &mut self.demo {
                    demo.resize(&block, cols, rows);
                }
                if let Some(host) = self.host() {
                    host.resize(&block, cols, rows, (cell.width.round() as u16, cell.height.round() as u16));
                }
            }
            Message::Grid(action) => {
                let (Some(host), Some((_, now))) = (self.host(), self.here()) else {
                    return Task::none();
                };
                match action {
                    Action::Focus(block) => host.focus(&block),
                    Action::Key(key) => {
                        if let Some(block) = now.focused() {
                            host.key(block, &key);
                            // Draw what was typed now, not when the server next speaks.
                            if key.action != rex_backend::KeyAction::Release {
                                return self.current.clone().map_or_else(Task::none, |id| {
                                    self.update(Message::Wake(id))
                                });
                            }
                        }
                    }
                    Action::Scroll(block, n) => host.scroll(&block, n),
                    Action::Wheel(block, x, y, lines) => host.wheel(&block, x, y, lines),
                    Action::Select(block, step, at) => {
                        let revision = host.selection(&block, step, at);
                        if let (rex_backend::Select::Release, Some(revision), Some(id)) =
                            (step, revision, self.current.clone())
                        {
                            self.primary = Some((id, block, revision));
                        }
                    }
                    Action::Mouse(block, report, cell, mods) => host.mouse(&block, report, cell, &mods),
                    Action::Menu(block) => self.menu_pane = Some(block),
                    Action::PastePrimary(block) => {
                        let Some(id) = self.current.clone() else { return Task::none() };
                        return clipboard::read_primary()
                            .map(move |t| cosmic::Action::App(Message::Pasted(id.clone(), block.clone(), t)));
                    }
                    Action::Resize(..) | Action::Shortcut(_) | Action::Fit(_) | Action::Pane(..) => {}
                }
            }
            Message::Pasted(id, block, text) => {
                if let (Some(host), Some(text)) =
                    (self.boxes.host(&id), text.filter(|t| !t.is_empty()))
                {
                    host.paste(&block, text);
                }
            }
            Message::SelectBox(id) => return self.show_box(&id),
            Message::OpenSession(id, session) => {
                if let Some(host) = self.boxes.host(&id) {
                    host.select(session);
                }
                return self.show_box(&id);
            }
            Message::Tab(window) => {
                if let (Some(host), Some((_, now))) = (self.host(), self.here())
                    && let Some(session) = now.session.clone()
                {
                    host.open(session, Some(window));
                }
            }
            Message::TabMenu(window) => self.menu_tab = Some(window),
            Message::HoverTab(window) => self.hovered_tab = Some(window),
            Message::LeaveTab(window) => {
                if self.hovered_tab.as_ref() == Some(&window) {
                    self.hovered_tab = None;
                }
            }
            Message::TabCommand(command) => {
                let Some(window) = self.menu_tab.clone() else { return Task::none() };
                match command {
                    ui::TabCommand::New => return self.shortcut(Shortcut::NewTab),
                    ui::TabCommand::Close => return self.update(Message::AskCloseTab(window)),
                    ui::TabCommand::Rename => {
                        if let Some((config, now)) = self.here()
                            && let (Some(session), Some(tab)) =
                                (now.session.clone(), now.tabs.iter().find(|t| t.id == window))
                        {
                            let (config, label) = (config.clone(), tab.label.clone());
                            self.dialog = Some(Dialog::Rename { config, session, window, label });
                        }
                    }
                }
            }
            Message::View(command) => {
                let size = match command {
                    ui::ViewCommand::Bigger => self.font_size + FONT_STEP,
                    ui::ViewCommand::Smaller => self.font_size - FONT_STEP,
                    ui::ViewCommand::Reset => self.args.font_size,
                }
                .clamp(FONT_MIN, FONT_MAX);
                if size != self.font_size {
                    self.font_size = size;
                    self.metrics = Metrics::measure(size, self.metrics.scale);
                }
            }
            Message::PaneCommand(command) => {
                let Some(now) = self.here().map(|(_, s)| s) else { return Task::none() };
                let Some(block) = self
                    .menu_pane
                    .clone()
                    .filter(|b| now.panes.iter().any(|p| &p.block == b))
                    .or_else(|| now.focused().map(str::to_owned))
                else {
                    return Task::none();
                };
                let tab = now.tab.clone();
                let on = |app: &mut Self, s| app.shortcut_on(s, Some(&block));
                match command {
                    ui::PaneCommand::Copy => return self.copy(&block),
                    ui::PaneCommand::Paste => return on(self, Shortcut::Paste),
                    ui::PaneCommand::SelectAll => {
                        if let Some(host) = self.host() {
                            let nowhere = rex_backend::Pointer { cell: (0, 0), position: (0.0, 0.0), cell_width: 1.0, height: 1.0 };
                            host.selection(&block, rex_backend::Select::All, nowhere);
                        }
                    }
                    ui::PaneCommand::Clear => {
                        if let Some(host) = self.host() {
                            host.clear(&block);
                        }
                    }
                    ui::PaneCommand::SplitRight => return on(self, Shortcut::SplitRight),
                    ui::PaneCommand::SplitDown => return on(self, Shortcut::SplitDown),
                    ui::PaneCommand::Zoom => return on(self, Shortcut::Zoom),
                    ui::PaneCommand::ClosePane => return on(self, Shortcut::ClosePane),
                    ui::PaneCommand::CloseTab => {
                        if let Some(window) = tab {
                            return self.update(Message::AskCloseTab(window));
                        }
                    }
                }
            }
            Message::AskCloseTab(window) => {
                let Some((config, now)) = self.here() else { return Task::none() };
                let (Some(session), Some(tab)) =
                    (now.session.clone(), now.tabs.iter().find(|t| t.id == window))
                else {
                    return Task::none();
                };
                let (config, label) = (config.clone(), tab.label.clone());
                self.sheets += 1;
                let number = self.sheets;
                self.dialog = Some(Dialog::CloseTab { config, session, window, label, number, armed: false });
                // Long enough that a click meant for the menu can't land on Close.
                return Task::perform(rex_backend::pause(Duration::from_millis(300)), move |()| {
                    cosmic::Action::App(Message::DialogArmed(number))
                });
            }
            Message::DialogArmed(n) => {
                if let Some(
                    Dialog::CloseTab { number, armed, .. }
                    | Dialog::CloseSession { number, armed, .. }
                    | Dialog::ClosePane { number, armed, .. },
                ) = &mut self.dialog
                    && *number == n
                {
                    *armed = true;
                }
            }
            Message::SessionMenu(id, session) => self.menu_session = Some((id, session)),
            Message::SessionCommand(command) => {
                let Some((id, session)) = self.menu_session.clone() else { return Task::none() };
                let (Some(config), Some(now)) = (self.boxes.config(&id), self.snapshot(&id)) else {
                    return Task::none();
                };
                let Some(index) = now.sessions.iter().position(|s| s.id == session) else {
                    return Task::none();
                };
                let label = now.sessions[index].label.clone();
                match command {
                    ui::SessionCommand::Open => return self.update(Message::OpenSession(id, session)),
                    ui::SessionCommand::MoveUp if index > 0 => {
                        return self.update(Message::MoveSession { id, session, slot: index - 1 });
                    }
                    ui::SessionCommand::MoveDown => {
                        return self.update(Message::MoveSession { id, session, slot: index + 2 });
                    }
                    ui::SessionCommand::MoveUp => {}
                    ui::SessionCommand::Rename => {
                        self.dialog = Some(Dialog::RenameSession { config: config.clone(), session, label });
                    }
                    ui::SessionCommand::Close => {
                        // Only the session on screen has its tabs known here.
                        let tabs = (now.session.as_ref() == Some(&session)).then_some(now.tabs.len());
                        let config = config.clone();
                        self.sheets += 1;
                        let number = self.sheets;
                        self.dialog = Some(Dialog::CloseSession { config, session, label, tabs, number, armed: false });
                        return Task::perform(rex_backend::pause(Duration::from_millis(300)), move |()| {
                            cosmic::Action::App(Message::DialogArmed(number))
                        });
                    }
                }
            }
            Message::MoveSession { id, session, slot } => {
                if let Some(host) = self.boxes.host(&id) {
                    host.move_session(&session, slot);
                }
            }
            Message::CloseSession { id, session } => {
                self.dialog = None;
                if let Some(host) = self.boxes.host(&id) {
                    host.close_session(&session);
                }
            }
            Message::RenameSession => {
                if let Some(Dialog::RenameSession { config, session, label }) = &self.dialog
                    && !label.trim().is_empty()
                {
                    if let Some(host) = self.boxes.host(&config.id) {
                        host.rename_session(session, label.trim());
                    }
                    self.dialog = None;
                }
            }
            Message::CloseTab { id, session, window } => {
                self.dialog = None;
                if let Some(host) = self.boxes.host(&id) {
                    host.close_tab(&session, &window);
                    self.closing.push((id, window, Instant::now()));
                }
            }
            Message::ClosePane { id, session, block } => {
                self.dialog = None;
                if let Some(host) = self.boxes.host(&id) {
                    host.close_pane(&session, &block);
                    self.closing_panes.push((id, block, Instant::now()));
                }
            }
            Message::RenameDraft(text) => {
                if let Some(Dialog::Rename { label, .. } | Dialog::RenameSession { label, .. }) = &mut self.dialog {
                    *label = text;
                }
            }
            Message::RenameTab => {
                if let Some(Dialog::Rename { config, session, window, label }) = self.dialog.take()
                    && let Some(host) = self.boxes.host(&config.id)
                {
                    host.rename_tab(&session, &window, label.trim());
                }
            }
            Message::Surface(action) => return cosmic::task::message(cosmic::Action::Surface(action)),
            Message::OpenAgent(id, session, window) => {
                if let Some(host) = self.boxes.host(&id) {
                    host.open(session, Some(window));
                }
                return self.show_box(&id);
            }
            Message::ToggleInbox => {
                self.core.window.show_context = !self.core.window.show_context;
            }
            Message::AddBox => {
                self.dialog = Some(Dialog::Add {
                    label: String::new(),
                    address: String::new(),
                    error: None,
                    editing: None,
                });
            }
            Message::EditBox(id) => {
                self.dialog = self.boxes.config(&id).map(|c| Dialog::Add {
                    label: c.label.clone(),
                    address: c.endpoint.clone(),
                    error: None,
                    editing: Some(c.id.clone()),
                });
            }
            Message::Draft(field, value) => {
                if let Some(Dialog::Add { label, address, error, .. }) = &mut self.dialog {
                    *error = None;
                    *(match field {
                        Field::Label => label,
                        Field::Address => address,
                    }) = value;
                }
            }
            Message::SaveBox => {
                let Some(Dialog::Add { label, address, error, editing }) = &mut self.dialog else {
                    return Task::none();
                };
                let all = self.boxes.configs();
                let saved = match editing.as_deref().and_then(|id| all.iter().find(|c| c.id == id)) {
                    Some(old) => old.edited(label, address, &all).and_then(|new| {
                        let id = new.id.clone();
                        self.boxes.replace(new).map(|()| id)
                    }),
                    None => BoxConfig::new(label, address, &all).and_then(|new| {
                        let id = new.id.clone();
                        self.boxes.add(new).map(|()| id)
                    }),
                };
                match saved {
                    Ok(id) => {
                        self.dialog = None;
                        self.boxes.connect(&id, None);
                        let shown = self.show_box(&id);
                        return Task::batch([shown, self.refresh()]);
                    }
                    Err(e) => *error = Some(e),
                }
            }
            Message::CancelDialog => self.dialog = None,
            Message::AskRemove(id) => {
                self.dialog = self.boxes.config(&id).cloned().map(Dialog::Remove);
            }
            Message::Remove(id) => {
                self.dialog = None;
                match self.boxes.remove(&id) {
                    Ok(()) => {
                        self.problem = None;
                        if self.current.as_ref() == Some(&id) {
                            self.current = None;
                            if let Some(next) = self.boxes.configs().first().map(|c| c.id.clone()) {
                                let shown = self.show_box(&next);
                                return Task::batch([shown, self.refresh()]);
                            }
                        }
                    }
                    Err(error) => self.problem = Some(error),
                }
                return self.refresh();
            }
            Message::Fit(id, session, block) => {
                self.dialog = None;
                if let Some(host) = self.boxes.host(&id) {
                    host.fit_here(&session, &block);
                }
            }
            Message::Ask(id, agent) => {
                let Some(config) = self.boxes.config(&id).cloned() else { return Task::none() };
                self.sheets += 1;
                let number = self.sheets;
                let target = (agent.session.clone(), agent.block.clone());
                let demo = self.demo.as_ref().map(|_| rex_backend::demo::question(&agent));
                self.dialog = Some(Dialog::Answer(Box::new(Answer {
                    number,
                    config,
                    agent: *agent,
                    question: None,
                    armed: false,
                    reply: String::new(),
                    sending: false,
                    outcome: None,
                })));
                if let Some(question) = demo {
                    return self.update(Message::Asked(number, Ok(question)));
                }
                let Some(asker) = self.boxes.host(&id).map(Host::asker) else {
                    return self.update(Message::Asked(number, Err("The box is disconnected".into())));
                };
                return Task::perform(asker.question(target.0, target.1), move |q| {
                    cosmic::Action::App(Message::Asked(number, q))
                });
            }
            Message::Asked(number, question) => {
                if let Some(a) = self.sheet().filter(|a| a.number == number) {
                    a.question = Some(question);
                    // Long enough that a key or click meant for something else can't land here.
                    return Task::perform(rex_backend::pause(Duration::from_millis(300)), move |()| {
                        cosmic::Action::App(Message::AnswerArmed(number))
                    });
                }
            }
            Message::AnswerArmed(number) => {
                if let Some(a) = self.sheet().filter(|a| a.number == number) {
                    a.armed = true;
                }
            }
            Message::AnswerDraft(text) => {
                if let Some(a) = self.sheet() {
                    a.reply = text;
                }
            }
            Message::AnswerKey(key) => return self.send_answer(Reply::Key(key)),
            Message::AnswerLine => {
                if let Some(line) = self.sheet().map(|a| a.reply.clone()).filter(|r| !r.trim().is_empty()) {
                    return self.send_answer(Reply::Line(line));
                }
            }
            Message::Answered(number, outcome) => {
                if let Some(a) = self.sheet().filter(|a| a.number == number) {
                    a.sending = false;
                    a.outcome = Some(match outcome {
                        Outcome::Reacted => "Sent, and the program moved on.".into(),
                        Outcome::Unconfirmed => {
                            "Sent, but the program has not reacted. Open the terminal to check.".into()
                        }
                        Outcome::Changed => "The question changed. Nothing was sent.".into(),
                        Outcome::NotSent(why) => format!("Nothing was sent: {why}"),
                        Outcome::Unknown => {
                            "The connection failed while sending. Open the terminal to check; nothing is sent again.".into()
                        }
                    });
                }
            }
            Message::OpenAsked => {
                if let Some(Dialog::Answer(a)) = self.dialog.take() {
                    return self.update(Message::OpenAgent(a.config.id, a.agent.session, a.agent.window));
                }
            }
            Message::Connect(id) => {
                match self.boxes.host(&id) {
                    Some(host) => host.reconnect(),
                    None => self.boxes.connect(&id, None),
                }
                return self.refresh();
            }
            Message::Disconnect(id) => {
                self.boxes.disconnect(&id);
                return self.refresh();
            }
            Message::Minute => {}
            Message::Scale(f) => {
                // COSMIC's own interface scale multiplies the compositor's.
                let scale = f * self.core.scale_factor();
                if (scale - self.metrics.scale).abs() > 1e-3 {
                    self.metrics = Metrics::measure(self.font_size, scale);
                }
            }
            Message::DemoTick => {
                if let Some(demo) = &mut self.demo {
                    demo.tick();
                }
            }
            Message::BenchDone => {
                if let Some(demo) = self.demo.as_ref().filter(|_| self.args.stress) {
                    demo.report(self.started.elapsed(), self.cache.shaped.get());
                }
                match (self.args.screenshot.is_some(), self.core.main_window_id()) {
                    (true, Some(id)) => {
                        return cosmic::iced::window::screenshot(id)
                            .map(|s| cosmic::Action::App(Message::Captured(s)));
                    }
                    _ => std::process::exit(0),
                }
            }
            Message::Captured(shot) => {
                if let Some(path) = &self.args.screenshot {
                    let mut ppm = format!("P6\n{} {}\n255\n", shot.size.width, shot.size.height)
                        .into_bytes();
                    ppm.extend(shot.rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]));
                    if let Err(error) = std::fs::write(path, ppm) {
                        eprintln!("could not save {}: {error}", path.display());
                        std::process::exit(1);
                    }
                }
                std::process::exit(0);
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let mut page = widget::column::with_capacity(4).spacing(8);
        if let Some(problem) = &self.problem {
            page = page.push(widget::text::body(problem.as_str()).width(Length::Fill));
        }
        let Some((config, now)) = self.here() else {
            let (title, body, button) = match self.current.as_deref().and_then(|id| self.boxes.config(id)) {
                Some(c) => (
                    format!("{} is disconnected", c.label),
                    c.endpoint.clone(),
                    widget::button::suggested("Connect").on_press(Message::Connect(c.id.clone())),
                ),
                None => (
                    "Add a box to begin".to_owned(),
                    "A box is a machine running a Rex server.".to_owned(),
                    widget::button::suggested("Add box").on_press(Message::AddBox),
                ),
            };
            return page.push(notice(title, body, vec![button.into()])).into();
        };
        if let Some(n) = now.notice.as_ref().or(self.copy_problem.as_ref()) {
            page = page.push(widget::text::caption(n.as_str()).width(Length::Fill));
        }
        let panes = self.panes();
        if panes.is_empty() {
            let (title, buttons): (String, Vec<Element<'_, Message>>) = if now.connected {
                (format!("No session open on {}", config.label), Vec::new())
            } else {
                (
                    format!("Not connected to {}", config.label),
                    vec![
                        widget::button::suggested("Try again")
                            .on_press(Message::Connect(config.id.clone()))
                            .into(),
                        widget::button::standard("Disconnect")
                            .on_press(Message::Disconnect(config.id.clone()))
                            .into(),
                        widget::button::standard("Edit…")
                            .on_press(Message::EditBox(config.id.clone()))
                            .into(),
                        widget::button::text("Remove…")
                            .on_press(Message::AskRemove(config.id.clone()))
                            .into(),
                    ],
                )
            };
            let body = if now.connected { "Choose one in the sidebar." } else { now.status.as_str() };
            return page.push(notice(title, body.to_owned(), buttons)).into();
        }
        let identity = Identity { monogram: config.monogram.clone(), tint: ui::tint(config) };
        let closing = now.tab.as_deref().is_some_and(|t| self.closing(t));
        // The menu is for the pane the right button went down on: that press comes first.
        let target = (self.menu_pane.as_deref())
            .and_then(|b| panes.iter().find(|p| p.block == b))
            .or_else(|| panes.iter().find(|p| p.focused));
        let closing_panes: Vec<String> = (self.closing_panes.iter())
            .filter(|(b, _, sent)| self.current.as_ref() == Some(b) && sent.elapsed() < CLOSE_WAIT)
            .map(|(_, block, _)| block.clone())
            .collect();
        let tab = Tab {
            keyboard: self.dialog.is_none() && !closing,
            closing,
            closing_panes,
            panes: now.tab_panes,
            last: now.tab_panes <= 1 && now.tabs.len() <= 1,
        };
        let menu = ui::pane_menu(ui::PaneMenu {
            selected: target.and_then(|p| p.frame.as_ref()).is_some_and(|f| f.selected_text().is_some()),
            live: target.is_some_and(|p| p.live),
            zoomed: target.is_some_and(|p| p.zoomed),
            can_zoom: tab.panes > 1 || target.is_some_and(|p| p.zoomed),
            can_close: !tab.last,
        });
        let grid = Grid::new(
            panes,
            identity,
            tab,
            &self.cache,
            self.metrics,
            Message::Grid,
        );
        // A margin all round, as in Rex, so no card runs into the window's edge.
        page.push(
            widget::container(widget::context_menu(grid, Some(menu)).on_surface_action(Message::Surface))
                .padding([0, 12, 12, 0]),
        )
        .into()
    }
}

impl App {
    /// A new tab opens beside the pane with the keyboard, as Rex's + does, so it needs one.
    fn can_open_tab(&self) -> bool {
        self.panes().iter().any(|p| p.focused)
    }
    fn configs(&self) -> impl Iterator<Item = &BoxConfig> {
        self.boxes.iter()
    }
}

fn notice<'a>(
    title: String,
    body: String,
    buttons: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    widget::container(
        widget::column::with_capacity(3)
            .spacing(12)
            .align_x(Alignment::Center)
            .push(widget::text::title3(title))
            .push(widget::text::body(body))
            .push(widget::row::with_children(buttons).spacing(8)),
    )
    .center(Length::Fill)
    .into()
}
