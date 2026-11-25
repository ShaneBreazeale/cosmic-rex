//! Rex for COSMIC: a libcosmic frontend over `rex-backend`.
mod grid;
mod keys;
mod ui;

use cosmic::{
    Application, ApplicationExt as _, Core, Element,
    app::{Settings, Task, context_drawer},
    iced::{Alignment, Length, Size, Subscription, clipboard},
    widget::{self, nav_bar},
};
use futures_util::StreamExt as _;
use grid::{Action, Cache, Grid, Identity, Metrics, Shortcut};
use rex_backend::{
    Agent, AgentState, AnswerKey, BoxConfig, Boxes, Host, Outcome, PaneView, Question, Reply, Snapshot, Split, Wake,
    demo::Demo,
};
use std::{
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};

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
    /// Start with the sidebar and inbox closed
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
    if !args.font_size.is_finite() || !(6.0..=72.0).contains(&args.font_size) {
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
    OpenAgent(String, String, String),
    Pasted(String, String, Option<String>),
    ToggleInbox,
    AddBox,
    Draft(Field, String),
    SaveBox,
    CancelDialog,
    AskRemove(String),
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
    Add { label: String, address: String, error: Option<String> },
    Remove(BoxConfig),
    Fit { config: BoxConfig, session: String, block: String },
    Answer(Box<Answer>),
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
    title: String,
    /// Answer sheets opened so far.
    sheets: u64,
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
    fn host(&self) -> Option<&Host> {
        self.boxes.host(self.current.as_deref()?)
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

    fn inbox(&self) -> Vec<(&BoxConfig, &Snapshot, &Agent)> {
        self.now
            .iter()
            .filter_map(|(id, s)| Some((self.boxes.config(id)?, s)))
            .flat_map(|(c, s)| s.agents.iter().map(move |a| (c, s, a)))
            .collect()
    }

    fn ask_fit(&mut self, block: String) {
        if let Some((config, now)) = self.here()
            && let Some(session) = now.session.clone()
        {
            self.dialog = Some(Dialog::Fit { config: config.clone(), session, block });
        }
    }

    fn shortcut(&mut self, shortcut: Shortcut) -> Task<Message> {
        if shortcut == Shortcut::Agents {
            return self.update(Message::ToggleInbox);
        }
        let Some((config, now)) = self.here() else { return Task::none() };
        let (Some(host), Some(session), Some(block)) =
            (self.host(), now.session.as_deref(), now.focused())
        else {
            return Task::none();
        };
        match shortcut {
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
            Shortcut::NewTab => host.new_tab(session, block),
            Shortcut::SplitRight => host.split(session, block, Split::Right),
            Shortcut::SplitDown => host.split(session, block, Split::Down),
            Shortcut::Agents => {}
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
            metrics: Metrics::measure(args.font_size),
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
            started: Instant::now(),
            core,
            args,
        };
        app.core.window.context_is_overlay = false;
        app.core.window.show_context = app.args.demo && !app.args.quiet;
        if app.args.quiet && !app.args.demo {
            app.core.nav_bar_set_toggled(false);
        }
        let mut task = app.refresh();
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
        let Some((config, now)) = self.here() else { return Vec::new() };
        let mut path = config.label.clone();
        if let Some(s) = now.sessions.iter().find(|s| Some(&s.id) == now.session.as_ref()) {
            path = format!("{path} › {}", s.label);
        }
        if let Some(t) = now.tabs.iter().find(|t| Some(&t.id) == now.tab.as_ref()) {
            path = format!("{path} › {}", t.label);
        }
        use cosmic::iced::advanced::text::{Ellipsize, EllipsizeHeightLimit, Wrapping};
        vec![
            widget::space::horizontal().width(8).into(),
            ui::monogram(config),
            widget::space::horizontal().width(4).into(),
            widget::text::heading(path)
                .wrapping(Wrapping::None)
                .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
                .into(),
        ]
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
            Dialog::Add { label, address, error } => {
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
                widget::dialog()
                    .title("Add a box")
                    .body("Connecting gives this app everything your account can do on that machine. It starts, stops and resizes nothing by itself.")
                    .control(form)
                    .primary_action(widget::button::suggested("Add box").on_press(Message::SaveBox))
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
                    return self.retitle();
                }
            }
            Message::Grid(Action::Shortcut(s)) => return self.shortcut(s),
            Message::Grid(Action::Fit(block)) => self.ask_fit(block),
            Message::Grid(Action::Resize(block, cols, rows)) => {
                let cell = self.metrics.cell;
                if let Some(demo) = &mut self.demo {
                    demo.resize(&block, cols, rows);
                }
                if let Some(host) = self.host() {
                    host.resize(&block, cols, rows, (cell.width as u16, cell.height as u16));
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
                    Action::Resize(..) | Action::Shortcut(_) | Action::Fit(_) => {}
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
                });
            }
            Message::Draft(field, value) => {
                if let Some(Dialog::Add { label, address, error }) = &mut self.dialog {
                    *error = None;
                    *(match field {
                        Field::Label => label,
                        Field::Address => address,
                    }) = value;
                }
            }
            Message::SaveBox => {
                let Some(Dialog::Add { label, address, error }) = &mut self.dialog else {
                    return Task::none();
                };
                let saved = BoxConfig::new(label, address, &self.boxes.configs()).and_then(|new| {
                    let id = new.id.clone();
                    self.boxes.add(new).map(|()| id)
                });
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
        if now.tabs.len() > 1 {
            let mut tabs = widget::row::with_capacity(now.tabs.len()).spacing(4);
            for t in &now.tabs {
                let active = now.tab.as_ref() == Some(&t.id);
                tabs = tabs.push(ui::tab(&t.label, active, Message::Tab(t.id.clone())));
            }
            page = page.push(tabs);
        }
        if let Some(n) = &now.notice {
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
        page.push(Grid::new(
            panes,
            identity,
            self.dialog.is_none(),
            &self.cache,
            self.metrics,
            Message::Grid,
        ))
        .into()
    }
}

impl App {
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
