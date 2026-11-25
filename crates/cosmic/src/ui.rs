//! Shared visual language: box identity, status, cards. See docs/design/ux-spec.md.
use crate::Message;
use cosmic::{
    Element, Theme,
    iced::{Alignment, Background, Border, Color, Length},
    theme, widget,
};
use rex_backend::{Agent, AgentState, BoxConfig, Snapshot, WaitingFor};

/// Box identity only, never status. Dark enough for white text, far apart in hue and lightness.
const TINTS: [Color; rex_backend::TINTS as usize] = [
    Color::from_rgb8(0x4f, 0x5b, 0xd5),
    Color::from_rgb8(0xb8, 0x3a, 0x8e),
    Color::from_rgb8(0x9a, 0x56, 0x2b),
    Color::from_rgb8(0x1f, 0x7a, 0x6d),
    Color::from_rgb8(0x5d, 0x6b, 0x78),
    Color::from_rgb8(0x7a, 0x4f, 0xb8),
];

pub fn tint(config: &BoxConfig) -> Color {
    TINTS[usize::from(config.tint) % TINTS.len()]
}

/// The two-letter chip: which machine this is.
pub fn monogram<'a>(config: &BoxConfig) -> Element<'a, Message> {
    let color = tint(config);
    widget::container(widget::text::caption(config.monogram.clone()).size(11))
        .center_x(Length::Fixed(24.0))
        .center_y(Length::Fixed(20.0))
        .class(theme::Container::custom(move |t: &Theme| widget::container::Style {
            text_color: Some(Color::WHITE),
            background: Some(Background::Color(color)),
            border: Border { radius: t.cosmic().radius_xs().into(), ..Default::default() },
            ..Default::default()
        }))
        .into()
}

/// Shapes differ in outline, so status survives greyscale and colour blindness.
pub fn shape(state: AgentState) -> &'static str {
    match state {
        AgentState::Blocked => "◆",
        AgentState::Error => "✕",
        AgentState::Done => "✓",
        AgentState::Working => "◔",
        AgentState::Idle => "○",
    }
}

pub fn color(state: AgentState, t: &Theme) -> Color {
    let c = t.cosmic();
    let muted = |a| Color { a, ..Color::from(c.on_bg_color()) };
    match state {
        AgentState::Blocked => c.warning_color().into(),
        AgentState::Error => c.destructive_color().into(),
        AgentState::Done => c.success_color().into(),
        AgentState::Working => muted(0.75),
        AgentState::Idle => muted(0.45),
    }
}

pub fn status<'a>(state: AgentState) -> Element<'a, Message> {
    widget::container(
        widget::text(shape(state))
            .size(12)
            .class(theme::Text::Color(color(state, &theme::active()))),
    )
    .center_x(Length::Fixed(16.0))
    .into()
}

pub fn word(a: &Agent) -> &'static str {
    match (a.state, a.waiting_for) {
        (AgentState::Blocked, Some(WaitingFor::Permission)) => "Approval",
        (AgentState::Blocked, Some(WaitingFor::Question)) => "Question",
        (AgentState::Blocked, Some(WaitingFor::Auth)) => "Sign-in",
        (AgentState::Blocked, None) => "Needs input",
        (AgentState::Error, _) => "Error",
        (AgentState::Done, _) => "Done",
        (AgentState::Working, _) => "Working",
        (AgentState::Idle, _) => "Idle",
    }
}

fn muted<'a>(text: impl Into<std::borrow::Cow<'a, str>> + 'a) -> Element<'a, Message> {
    let c = Color { a: 0.7, ..Color::from(theme::active().cosmic().on_bg_color()) };
    widget::text::caption(text).class(theme::Text::Color(c)).into()
}

/// A quiet row: neutral text, a soft fill on hover, a stronger one when it is open. The accent is
/// left for the one thing that has the keyboard.
fn row_class(selected: bool) -> theme::Button {
    let style = move |t: &Theme, fill: f32, text: f32| {
        let c = t.cosmic();
        let on: Color = c.on_primary_container_color().into();
        let mut s = widget::button::Style::new();
        s.background = Some(Background::Color(Color { a: fill, ..on }));
        s.border_radius = c.radius_s().into();
        s.text_color = Some(Color { a: text, ..on });
        s.icon_color = s.text_color;
        s
    };
    let rest = if selected { 0.14 } else { 0.0 };
    theme::Button::Custom {
        active: Box::new(move |focused, t| {
            let mut s = style(t, rest, 1.0);
            if focused {
                s.outline_width = 2.0;
                s.outline_color = t.cosmic().accent_color().into();
            }
            s
        }),
        disabled: Box::new(move |t| style(t, 0.0, 0.5)),
        hovered: Box::new(move |_, t| style(t, rest + 0.06, 1.0)),
        pressed: Box::new(move |_, t| style(t, rest + 0.12, 1.0)),
    }
}

/// An inbox card: the same quiet row with a resting fill.
fn card_class() -> theme::Button {
    let style = |t: &Theme, lift: f32, text: f32| {
        let c = t.cosmic();
        let on: Color = c.on_primary_container_color().into();
        let mut s = widget::button::Style::new();
        s.background = Some(Background::Color(Color { a: lift, ..on }));
        s.border_radius = c.radius_s().into();
        s.text_color = Some(Color { a: text, ..on });
        s
    };
    theme::Button::Custom {
        active: Box::new(move |focused, t| {
            let mut s = style(t, 0.06, 1.0);
            if focused {
                s.outline_width = 2.0;
                s.outline_color = t.cosmic().accent_color().into();
            }
            s
        }),
        disabled: Box::new(move |t| style(t, 0.03, 0.6)),
        hovered: Box::new(move |_, t| style(t, 0.1, 1.0)),
        pressed: Box::new(move |_, t| style(t, 0.14, 1.0)),
    }
}

/// A tab pill: neutral, filled when active.
pub fn tab<'a>(label: &'a str, active: bool, on_press: Message) -> Element<'a, Message> {
    widget::button::custom(widget::text::body(label))
        .padding([6, 14])
        .class(tab_class(active))
        .on_press(on_press)
        .into()
}

fn tab_class(active: bool) -> theme::Button {
    let style = |t: &Theme, fill: f32, text: f32| {
        let c = t.cosmic();
        let on: Color = c.on_bg_color().into();
        let mut s = widget::button::Style::new();
        s.background = Some(Background::Color(Color { a: fill, ..on }));
        s.border_radius = c.radius_xl().into();
        s.text_color = Some(Color { a: text, ..on });
        s
    };
    let (rest, text) = if active { (0.14, 1.0) } else { (0.0, 0.75) };
    theme::Button::Custom {
        active: Box::new(move |_, t| style(t, rest, text)),
        disabled: Box::new(move |t| style(t, rest, 0.5)),
        hovered: Box::new(move |_, t| style(t, rest + 0.06, 1.0)),
        pressed: Box::new(move |_, t| style(t, rest + 0.1, 1.0)),
    }
}

/// A tab under its session: text only, brighter when it is the one on screen.
fn tab_row_class(active: bool) -> theme::Button {
    let style = |t: &Theme, fill: f32, text: f32| {
        let c = t.cosmic();
        let on: Color = c.on_primary_container_color().into();
        let mut s = widget::button::Style::new();
        s.background = Some(Background::Color(Color { a: fill, ..on }));
        s.border_radius = c.radius_s().into();
        s.text_color = Some(Color { a: text, ..on });
        s
    };
    let text = if active { 1.0 } else { 0.6 };
    theme::Button::Custom {
        active: Box::new(move |_, t| style(t, 0.0, text)),
        disabled: Box::new(move |t| style(t, 0.0, 0.5)),
        hovered: Box::new(move |_, t| style(t, 0.06, 1.0)),
        pressed: Box::new(move |_, t| style(t, 0.12, 1.0)),
    }
}

type Text<'a> = widget::Text<'a, Theme>;

fn one_line(text: Text<'_>) -> Text<'_> {
    use cosmic::iced::advanced::text::{Ellipsize, EllipsizeHeightLimit, Wrapping};
    text.wrapping(Wrapping::None).ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
}

fn faint<'a>(text: impl Into<std::borrow::Cow<'a, str>> + 'a) -> Text<'a> {
    let c = Color { a: 0.7, ..Color::from(theme::active().cosmic().on_bg_color()) };
    widget::text::caption(text).class(theme::Text::Color(c))
}

/// The short fact at a row's edge: progress while working, how long otherwise.
fn edge(a: &Agent) -> String {
    match (a.state, a.progress, a.since) {
        (AgentState::Idle, ..) => String::new(),
        (AgentState::Working, Some(p), _) => format!("{p}%"),
        (_, _, Some(since)) => elapsed(since.elapsed()),
        _ => String::new(),
    }
}

pub fn elapsed(d: std::time::Duration) -> String {
    match d.as_secs() {
        0..60 => "now".into(),
        s @ 60..3600 => format!("{} min", s / 60),
        s @ 3600..86400 => format!("{} h", s / 3600),
        s => format!("{} d", s / 86400),
    }
}

pub struct BoxRow<'a> {
    pub config: &'a BoxConfig,
    /// None while this box is disconnected by choice.
    pub now: Option<&'a Snapshot>,
}

pub fn sidebar<'a>(rows: Vec<BoxRow<'a>>, current: Option<&str>) -> Element<'a, Message> {
    let mut list = widget::column::with_capacity(rows.len() * 4).spacing(2);
    for row in rows {
        let id = &row.config.id;
        let online = row.now.is_some_and(|s| s.connected);
        let mut head = widget::row::with_capacity(4)
            .spacing(8)
            .align_y(Alignment::Center)
            .push(monogram(row.config))
            .push(widget::text::heading(row.config.label.as_str()).width(Length::Fill));
        // Quiet when all is well; words when not.
        let link = match row.now {
            None => Some("⦸ Disconnected"),
            Some(s) if !s.connected => Some("◌ Not connected"),
            Some(_) => None,
        };
        if let Some(link) = link {
            head = head.push(muted(link));
        }
        list = list.push(
            widget::button::custom(head.padding([4, 4]))
                .width(Length::Fill)
                .class(row_class(false))
                .on_press(Message::SelectBox(id.clone())),
        );
        let Some(now) = row.now else { continue };
        if !online && !now.status.is_empty() {
            list = list.push(widget::container(muted(now.status.as_str())).padding([0, 8, 4, 40]));
        }
        for s in &now.sessions {
            let selected = online
                && current == Some(id.as_str())
                && now.session.as_deref() == Some(s.id.as_str());
            // The agent that most needs a look speaks for its session.
            let top = now.agents.iter().find(|a| a.session == s.id);
            let news = top.map_or_else(String::new, edge);
            let name = widget::row::with_capacity(2)
                .spacing(8)
                .push(one_line(widget::text::body(s.label.as_str())).width(Length::Fill))
                .push(muted(news));
            let says = match top {
                Some(a) => match &a.message {
                    Some(m) if a.state != AgentState::Idle => format!("{}: {m}", word(a)),
                    _ => word(a).to_owned(),
                },
                None => s.about.clone(),
            };
            let lines = widget::column::with_capacity(2)
                .push(name)
                .push(one_line(faint(says)).width(Length::Fill));
            let line = widget::row::with_capacity(2)
                .spacing(8)
                .align_y(Alignment::Center)
                .push(status(s.state))
                .push(lines.width(Length::Fill));
            // Sessions of an unreachable box are what was last seen, and can't be opened.
            list = list.push(
                widget::button::custom(line.padding([2, 2, 2, 6]))
                    .width(Length::Fill)
                    .class(row_class(selected))
                    .on_press_maybe(online.then(|| Message::OpenSession(id.clone(), s.id.clone()))),
            );
            if selected && now.tabs.len() > 1 {
                for t in &now.tabs {
                    let active = now.tab.as_ref() == Some(&t.id);
                    list = list.push(
                        widget::button::custom(
                            one_line(widget::text::caption(t.label.as_str()))
                                .width(Length::Fill),
                        )
                        .padding([4, 8, 4, 44])
                        .width(Length::Fill)
                        .class(tab_row_class(active))
                        .on_press(Message::Tab(t.id.clone())),
                    );
                }
            }
        }
        if online && now.sessions.is_empty() {
            list = list.push(widget::container(muted("No sessions")).padding([2, 8, 6, 40]));
        }
        list = list.push(widget::space::vertical().height(8));
    }
    let panel = widget::column::with_capacity(2)
        .push(widget::scrollable(list).height(Length::Fill))
        .push(
            widget::button::custom(
                widget::row::with_capacity(2)
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(widget::icon::from_name("list-add-symbolic").size(16))
                    .push(widget::text::body("Add box"))
                    .padding([4, 4]),
            )
            .width(Length::Fill)
            .class(row_class(false))
            .on_press(Message::AddBox),
        );
    widget::container(panel)
        .padding(12)
        .width(Length::Fixed(280.0))
        .height(Length::Fill)
        .class(theme::Container::custom(|t: &Theme| {
            let c = t.cosmic();
            widget::container::Style {
                text_color: Some(c.on_primary_container_color().into()),
                background: Some(Background::Color(c.primary_container_color().into())),
                border: Border { radius: c.radius_s().into(), ..Default::default() },
                ..Default::default()
            }
        }))
        .into()
}

/// One queue across every box. A waiting row opens its answer sheet; others go to the pane.
pub fn inbox<'a>(items: Vec<(&'a BoxConfig, &'a Snapshot, &'a Agent)>) -> Element<'a, Message> {
    let mut list = widget::column::with_capacity(items.len() + 4).spacing(8);
    let mut shown = false;
    for (title, state) in [
        ("Needs you", AgentState::Blocked),
        ("Failed", AgentState::Error),
        ("Finished", AgentState::Done),
        ("Working", AgentState::Working),
    ] {
        let mut group = items.iter().filter(|(_, _, a)| a.state == state).peekable();
        if group.peek().is_none() {
            continue;
        }
        shown = true;
        list = list.push(widget::text::caption_heading(title));
        for (config, now, a) in group {
            let head = widget::row::with_capacity(3)
                .spacing(8)
                .align_y(Alignment::Center)
                .push(status(a.state))
                .push(widget::text::heading(word(a)).width(Length::Fill))
                .push(muted(if now.connected { edge(a) } else { "Last seen".to_owned() }));
            let place = widget::row::with_capacity(2)
                .spacing(8)
                .align_y(Alignment::Center)
                .push(monogram(config))
                .push(widget::text::caption(a.place.as_str()));
            let mut card = widget::column::with_capacity(4).spacing(6).push(head).push(place);
            if let Some(message) = &a.message {
                // The program's words, never on a line with ours.
                card = card.push(
                    widget::container(widget::text::monotext(message.as_str()).size(13))
                        .padding([4, 8])
                        .width(Length::Fill)
                        .class(theme::Container::custom(|t: &Theme| {
                            let c = t.cosmic();
                            widget::container::Style {
                                background: Some(Background::Color(c.bg_color().into())),
                                border: Border { radius: c.radius_xs().into(), ..Default::default() },
                                ..Default::default()
                            }
                        })),
                );
            }
            card = card.push(muted(format!("reports itself as {}", a.app)));
            list = list.push(
                widget::button::custom(card.padding(12).width(Length::Fill))
                    .width(Length::Fill)
                    .class(card_class())
                    .on_press_maybe(now.connected.then(|| {
                        if a.state == AgentState::Blocked {
                            Message::Ask(config.id.clone(), Box::new((*a).clone()))
                        } else {
                            Message::OpenAgent(config.id.clone(), a.session.clone(), a.window.clone())
                        }
                    })),
            );
        }
    }
    if shown { list.into() } else { widget::text::body("Nothing needs you.").into() }
}

/// A program's words, framed, never on a line with ours.
fn quoted<'a>(lines: impl Iterator<Item = &'a str>, size: f32) -> Element<'a, Message> {
    let column = lines.fold(widget::column::with_capacity(4), |c, line| {
        // An empty line still takes its height.
        let line = if line.is_empty() { " " } else { line };
        c.push(
            widget::text::monotext(line)
                .size(size)
                .wrapping(cosmic::iced::advanced::text::Wrapping::None),
        )
    });
    widget::container(column)
        .padding([8, 12])
        .width(Length::Fill)
        .clip(true)
        .class(theme::Container::custom(|t: &Theme| {
            let c = t.cosmic();
            widget::container::Style {
                background: Some(Background::Color(c.bg_color().into())),
                text_color: Some(c.on_bg_color().into()),
                border: Border { radius: c.radius_s().into(), ..Default::default() },
                ..Default::default()
            }
        }))
        .into()
}

/// What an answer sheet shows about the question, and what can still be done with it.
pub struct Sheet<'a> {
    pub config: &'a BoxConfig,
    pub agent: &'a Agent,
    pub question: Option<Result<&'a rex_backend::Question, &'a str>>,
    pub reply: &'a str,
    pub outcome: Option<&'a str>,
    /// Controls are on: the question is loaded, nothing is in flight, and it has been readable
    /// for long enough that a key meant for something else can't land here.
    pub armed: bool,
}

pub fn answer<'a>(s: Sheet<'a>) -> Element<'a, Message> {
    let title = match s.agent.waiting_for {
        Some(WaitingFor::Permission) => "Approval request",
        Some(WaitingFor::Question) => "Question",
        Some(WaitingFor::Auth) => "Sign-in request",
        None => "Waiting for input",
    };
    let mut body = widget::column::with_capacity(8).spacing(12).push(
        widget::row::with_capacity(2)
            .spacing(8)
            .align_y(Alignment::Center)
            .push(monogram(s.config))
            .push(widget::text::body(format!("{} › {}", s.config.label, s.agent.place))),
    );
    let sign_in = s.agent.waiting_for == Some(WaitingFor::Auth);
    match s.question {
        None => body = body.push(muted("Reading the question…")),
        Some(Err(e)) => body = body.push(widget::text::body(e)),
        Some(Ok(q)) => {
            body = body.push(muted(format!("The program reports itself as {}", q.app)));
            if let Some(m) = &q.message {
                body = body
                    .push(widget::text::caption_heading("The program says"))
                    .push(quoted(std::iter::once(m.as_str()), 13.0));
            }
            body = body
                .push(widget::text::caption_heading("End of its screen"))
                .push(quoted(q.screen.iter().map(String::as_str), 12.0));
            if sign_in {
                body = body.push(widget::text::body("Sign-in is answered in the terminal."));
            } else if s.outcome.is_none() {
                let mut keys = widget::row::with_capacity(8)
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(widget::text::caption("Send key"));
                for k in rex_backend::AnswerKey::ALL {
                    keys = keys.push(
                        widget::button::standard(k.label())
                            .on_press_maybe(s.armed.then_some(Message::AnswerKey(k))),
                    );
                }
                let mut line = widget::text_input("Type a reply, then Enter", s.reply);
                if s.armed {
                    line = line.on_input(Message::AnswerDraft).on_submit(|_| Message::AnswerLine);
                }
                body = body.push(keys).push(line).push(muted(format!(
                    "Goes to {}. Sent once, and only if the question is still this one.",
                    s.config.label
                )));
            }
        }
    }
    if let Some(outcome) = s.outcome {
        body = body.push(widget::text::heading(outcome));
    }
    widget::dialog()
        .title(title)
        .control(body)
        .primary_action(widget::button::suggested("Open terminal").on_press(Message::OpenAsked))
        .secondary_action(
            widget::button::standard(if s.outcome.is_some() { "Close" } else { "Cancel" })
                .on_press(Message::CancelDialog),
        )
        .width(620)
        .into()
}
