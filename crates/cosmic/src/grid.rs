//! One widget that draws every visible pane of a Rex window.
//!
//! Text goes through iced's glyph atlas as one shaped `cosmic_text::Buffer` per row, cached by a
//! hash of the row's content: an idle pane shapes nothing, and typing reshapes one row.
use cosmic::{
    Renderer, Theme,
    iced::{
        Background, Border, Color, Event, Length, Point, Rectangle, Size,
        advanced::{
            Clipboard, Layout, Shell, Widget,
            graphics::text::{Raw, Renderer as _, cosmic_text as ct, font_system},
            layout, renderer,
            renderer::Renderer as _,
            widget::{Tree, tree},
        },
        keyboard, mouse,
    },
};
use rex_backend::{CellRef, CursorShape, Frame, Key, KeyAction, PaneView, Size as Owner, Underline};
use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};

use crate::keys;

pub const PAD: f32 = 10.0;
const HEADER: f32 = 28.0;
const GAP: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub size: f32,
    pub cell: Size,
}

impl Metrics {
    /// The nearest size to `wanted` whose advance is a whole number of pixels, so glyphs,
    /// backgrounds and underlines stay on one grid however long the row is.
    pub fn measure(wanted: f32) -> Self {
        let mut fonts = font_system().write().expect("font system");
        let advance = advance(fonts.raw(), wanted);
        let width = advance.round().max(1.0);
        Self {
            size: wanted * width / advance,
            cell: Size::new(width, (wanted * 1.3).round()),
        }
    }

    pub fn grid(&self, pane: Size) -> (u16, u16) {
        let n = |space: f32, cell: f32| ((space - 2.0 * PAD) / cell).floor().clamp(1.0, 2000.0) as u16;
        (n(pane.width, self.cell.width), n(pane.height - HEADER, self.cell.height))
    }
}

fn advance(fonts: &mut ct::FontSystem, size: f32) -> f32 {
    let mut buffer = ct::Buffer::new(fonts, ct::Metrics::new(size, size * 1.3));
    buffer.set_text("M", &attrs(), ct::Shaping::Basic, None);
    buffer.shape_until_scroll(fonts, false);
    buffer
        .layout_runs()
        .next()
        .and_then(|run| run.glyphs.first().map(|g| g.w))
        .filter(|w| *w > 0.0)
        .unwrap_or(size * 0.6)
}

fn attrs() -> ct::Attrs<'static> {
    ct::Attrs::new().family(ct::Family::Monospace)
}

struct Row {
    hash: u64,
    segments: Segments,
}

/// Shaped text and the column each piece starts at.
type Segments = Arc<[(usize, Arc<ct::Buffer>)]>;

/// Shaped rows per pane, owned by the app so they outlive a view.
#[derive(Default)]
pub struct Cache {
    panes: RefCell<HashMap<String, Vec<Row>>>,
    /// Guessed text being drawn; the renderer only keeps weak references.
    guesses: RefCell<HashMap<String, (String, Arc<ct::Buffer>)>>,
    pub shaped: std::cell::Cell<u64>,
}

impl Cache {
    pub fn retain(&self, keep: impl Fn(&str) -> bool) {
        self.panes.borrow_mut().retain(|block, _| keep(block));
        self.guesses.borrow_mut().retain(|block, _| keep(block));
    }

    fn guess(&self, block: &str, text: &str, m: Metrics) -> Arc<ct::Buffer> {
        let mut guesses = self.guesses.borrow_mut();
        if let Some((old, buffer)) = guesses.get(block)
            && old == text
        {
            return buffer.clone();
        }
        let mut fonts = font_system().write().expect("font system");
        let mut buffer = ct::Buffer::new(fonts.raw(), ct::Metrics::new(m.size, m.cell.height));
        buffer.set_wrap(ct::Wrap::None);
        buffer.set_text(text, &attrs(), ct::Shaping::Advanced, None);
        buffer.shape_until_scroll(fonts.raw(), false);
        let buffer = Arc::new(buffer);
        guesses.insert(block.to_owned(), (text.to_owned(), buffer.clone()));
        buffer
    }

    fn rows(&self, block: &str, frame: &Frame, m: Metrics) -> Vec<Segments> {
        let mut panes = self.panes.borrow_mut();
        let rows = panes.entry(block.to_owned()).or_default();
        rows.truncate(frame.row_count());
        let mut fonts = None;
        for i in 0..frame.row_count() {
            let hash = row_hash(frame, i, m);
            if rows.get(i).is_some_and(|row| row.hash == hash) {
                continue;
            }
            let fonts = fonts.get_or_insert_with(|| font_system().write().expect("font system"));
            let row = Row {
                hash,
                segments: shape(fonts.raw(), frame, i, m),
            };
            self.shaped.set(self.shaped.get() + 1);
            match rows.get_mut(i) {
                Some(old) => *old = row,
                None => rows.push(row),
            }
        }
        rows.iter().map(|row| row.segments.clone()).collect()
    }
}

fn row_hash(frame: &Frame, y: usize, m: Metrics) -> u64 {
    let mut h = DefaultHasher::new();
    m.size.to_bits().hash(&mut h);
    for c in frame.row(y) {
        c.text().hash(&mut h);
        (c.is_spacer(), c.fg(), c.bold(), c.italic(), c.faint()).hash(&mut h);
    }
    h.finish()
}

/// Characters a monospace font is sure to draw one cell wide. Anything else may come from a
/// fallback font with another advance, so it gets its own segment at its own column.
fn on_grid(text: &str) -> bool {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (None, _) => true,
        (Some(c), None) => matches!(c, ' '..='\u{24f}' | '\u{2500}'..='\u{259f}'),
        _ => false,
    }
}

fn shape(fonts: &mut ct::FontSystem, frame: &Frame, y: usize, m: Metrics) -> Segments {
    type Style = (u32, bool, bool, bool);
    let end = frame
        .row(y)
        .rposition(|c| !c.text().is_empty() && c.text() != " ")
        .map_or(0, |i| i + 1);
    let mut pieces: Vec<(usize, Vec<(Style, String)>)> = Vec::new();
    let mut open = false;
    for (x, c) in frame.row(y).take(end).enumerate().filter(|(_, c)| !c.is_spacer()) {
        let style = (c.fg(), c.bold(), c.italic(), c.faint());
        let text = if c.text().is_empty() { " " } else { c.text() };
        let joins = on_grid(text);
        if !(open && joins) {
            pieces.push((x, Vec::new()));
        }
        open = joins;
        let spans = &mut pieces.last_mut().expect("piece").1;
        match spans.last_mut() {
            Some((last, run)) if *last == style => run.push_str(text),
            _ => spans.push((style, text.to_owned())),
        }
    }
    pieces
        .into_iter()
        .map(|(x, spans)| {
            let mut buffer = ct::Buffer::new(fonts, ct::Metrics::new(m.size, m.cell.height));
            buffer.set_wrap(ct::Wrap::None);
            let styled = spans.iter().map(|((fg, bold, italic, faint), text)| {
                let (r, g, b) = bytes(*fg);
                let mut a = attrs().color(ct::Color::rgba(r, g, b, if *faint { 150 } else { 255 }));
                if *bold {
                    a = a.weight(ct::Weight::BOLD);
                }
                if *italic {
                    a = a.style(ct::Style::Italic);
                }
                (text.as_str(), a)
            });
            buffer.set_rich_text(styled, &attrs(), ct::Shaping::Advanced, None);
            buffer.shape_until_scroll(fonts, false);
            (x, Arc::new(buffer))
        })
        .collect()
}

fn bytes(rgb: u32) -> (u8, u8, u8) {
    ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}
fn color(rgb: u32) -> Color {
    let (r, g, b) = bytes(rgb);
    Color::from_rgb8(r, g, b)
}

#[derive(Clone, Debug)]
pub enum Action {
    Focus(String),
    Key(Key),
    Shortcut(Shortcut),
    /// Viewport lines, negative is up.
    Scroll(String, isize),
    Wheel(String, u16, u16, i32),
    Resize(String, u16, u16),
    /// Take this pane's size, from a click on its size chip.
    Fit(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shortcut {
    Paste,
    NewTab,
    SplitRight,
    SplitDown,
    Zoom,
    Agents,
    Fit,
}

/// Which machine the panes are on, drawn where typing happens.
#[derive(Clone)]
pub struct Identity {
    pub monogram: String,
    pub tint: Color,
}

pub struct Grid<'a, M> {
    panes: Vec<PaneView>,
    identity: Identity,
    /// False while a dialog has the keyboard: no key may reach a terminal then.
    keyboard: bool,
    cache: &'a Cache,
    metrics: Metrics,
    on: Box<dyn Fn(Action) -> M + 'a>,
}

impl<'a, M> Grid<'a, M> {
    pub fn new(
        panes: Vec<PaneView>,
        identity: Identity,
        keyboard: bool,
        cache: &'a Cache,
        metrics: Metrics,
        on: impl Fn(Action) -> M + 'a,
    ) -> Self {
        Self { panes, identity, keyboard, cache, metrics, on: Box::new(on) }
    }

    fn rect(&self, pane: &PaneView, b: Rectangle) -> Rectangle {
        let [x, y, w, h] = pane.rect.map(|v| v as f32);
        // Half the gap on inner edges only, so outer edges line up with the rest of the window.
        let (l, t) = (if x > 0.001 { GAP / 2.0 } else { 0.0 }, if y > 0.001 { GAP / 2.0 } else { 0.0 });
        let r = if x + w < 0.999 { GAP / 2.0 } else { 0.0 };
        let bottom = if y + h < 0.999 { GAP / 2.0 } else { 0.0 };
        Rectangle {
            x: (b.x + x * b.width + l).round(),
            y: (b.y + y * b.height + t).round(),
            width: (w * b.width - l - r).round().max(1.0),
            height: (h * b.height - t - bottom).round().max(1.0),
        }
    }

    fn chip(&self, pane: &PaneView, rect: Rectangle) -> Option<(Rectangle, String)> {
        let grid = pane.frame.as_ref().map(|f| format!(" · {}×{}", f.cols(), f.rows())).unwrap_or_default();
        let text = match pane.size {
            Owner::Here => return None,
            Owner::Elsewhere => format!("Sized elsewhere{grid} · Fit here"),
            Owner::Unclaimed => format!("Unclaimed{grid} · Fit here"),
        };
        // Shortened rather than dropped: whose size this is must never go unsaid.
        [text, "⊡ Fit here".to_owned(), "⊡".to_owned()].into_iter().find_map(|text| {
            let width = text.chars().count() as f32 * 5.9 + 18.0;
            (width + 130.0 < rect.width || text == "⊡").then(|| {
                let x = rect.x + rect.width - width - 28.0;
                (Rectangle { x, y: rect.y + 4.0, width, height: HEADER - 8.0 }, text)
            })
        })
    }

    /// Topmost pane first: floating layers come after the tiled one.
    fn at(&self, p: Point, b: Rectangle) -> Option<&PaneView> {
        self.panes.iter().rev().find(|pane| self.rect(pane, b).contains(p))
    }
}

#[derive(Default)]
struct State {
    sizes: HashMap<String, (u16, u16)>,
    modifiers: keyboard::Modifiers,
    scroll: f32,
    /// Presses sent to the program, so a release is sent only for those.
    down: std::collections::HashSet<keyboard::key::Physical>,
}

impl<M> Widget<M, Theme, Renderer> for Grid<'_, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }
    fn layout(&mut self, _: &mut Tree, _: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _: &Renderer,
        _: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        // Only for panes this client owns. Forgetting the rest means a pane is sized as soon as
        // it becomes ours.
        let mut sizes = HashMap::new();
        for pane in self.panes.iter().filter(|p| p.live && p.size == Owner::Here) {
            let grid = self.metrics.grid(self.rect(pane, bounds).size());
            if state.sizes.get(&pane.block) != Some(&grid) {
                shell.publish((self.on)(Action::Resize(pane.block.clone(), grid.0, grid.1)));
            }
            sizes.insert(pane.block.clone(), grid);
        }
        state.sizes = sizes;

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => state.modifiers = *m,
            Event::Keyboard(keyboard::Event::KeyPressed {
                key, physical_key, modifiers, text, repeat, ..
            }) if self.keyboard && !shell.is_event_captured() => {
                if let Some(shortcut) = shortcut(physical_key, *modifiers) {
                    if !*repeat {
                        shell.publish((self.on)(Action::Shortcut(shortcut)));
                    }
                    shell.capture_event();
                    return;
                }
                let action = if *repeat { KeyAction::Repeat } else { KeyAction::Press };
                if let Some(v) =
                    keys::describe(key, physical_key, *modifiers, text.as_deref(), action)
                {
                    state.down.insert(*physical_key);
                    shell.publish((self.on)(Action::Key(v)));
                    shell.capture_event();
                }
            }
            Event::Keyboard(keyboard::Event::KeyReleased { key, physical_key, modifiers, .. }) => {
                if state.down.remove(physical_key)
                    && let Some(v) =
                        keys::describe(key, physical_key, *modifiers, None, KeyAction::Release)
                {
                    shell.publish((self.on)(Action::Key(v)));
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(p) = cursor.position_over(bounds) else { return };
                let Some(pane) = self.at(p, bounds) else { return };
                if self.chip(pane, self.rect(pane, bounds)).is_some_and(|(c, _)| c.contains(p)) {
                    shell.publish((self.on)(Action::Fit(pane.block.clone())));
                    shell.capture_event();
                } else if !pane.focused {
                    shell.publish((self.on)(Action::Focus(pane.block.clone())));
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let Some(p) = cursor.position_over(bounds) else { return };
                let Some(pane) = self.at(p, bounds) else { return };
                state.scroll += match delta {
                    mouse::ScrollDelta::Lines { y, .. } => -y * 3.0,
                    mouse::ScrollDelta::Pixels { y, .. } => -y / self.metrics.cell.height,
                };
                let lines = state.scroll.trunc();
                if lines == 0.0 {
                    return;
                }
                state.scroll -= lines;
                let r = self.rect(pane, bounds);
                let tracking = pane.frame.as_ref().is_some_and(|f| f.mouse_tracking());
                shell.publish((self.on)(if tracking && !state.modifiers.shift() {
                    let cell = |v: f32, size: f32| ((v - PAD) / size).max(0.0) as u16;
                    Action::Wheel(
                        pane.block.clone(),
                        cell(p.x - r.x, self.metrics.cell.width),
                        cell(p.y - r.y - HEADER, self.metrics.cell.height),
                        lines as i32,
                    )
                } else {
                    Action::Scroll(pane.block.clone(), lines as isize)
                }));
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _: &Rectangle,
        _: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        if let Some(p) = cursor.position_over(bounds) {
            let Some(pane) = self.at(p, bounds) else { return mouse::Interaction::None };
            let rect = self.rect(pane, bounds);
            if self.chip(pane, rect).is_some_and(|(c, _)| c.contains(p)) {
                mouse::Interaction::Pointer
            } else if p.y < rect.y + HEADER {
                mouse::Interaction::None
            } else {
                mouse::Interaction::Text
            }
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        _: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _: &renderer::Style,
        layout: Layout<'_>,
        _: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for pane in &self.panes {
            let rect = self.rect(pane, layout.bounds());
            let Some(clip) = rect.intersection(viewport) else { continue };
            // A layer per pane keeps a wider server-sized grid from spilling into neighbours.
            renderer.with_layer(clip, |r| self.draw_pane(r, pane, rect, clip, theme));
        }
    }
}

impl<M> Grid<'_, M> {
    fn draw_pane(&self, r: &mut Renderer, pane: &PaneView, rect: Rectangle, clip: Rectangle, theme: &Theme) {
        let m = self.metrics;
        let c = theme.cosmic();
        let accent: Color = c.accent_color().into();
        let radius = c.radius_s();
        let fill = |r: &mut Renderer, bounds: Rectangle, c: Color| {
            r.fill_quad(renderer::Quad { bounds, ..Default::default() }, Background::Color(c));
        };
        let rounded = |r: &mut Renderer, bounds: Rectangle, c: Color, radius: [f32; 4]| {
            let border = Border { radius: radius.into(), ..Default::default() };
            r.fill_quad(renderer::Quad { bounds, border, ..Default::default() }, Background::Color(c));
        };
        let label = |r: &mut Renderer, text: &str, x: f32, width: f32, size: f32, color: Color| {
            use cosmic::iced::advanced::text::{self, Renderer as _};
            // Cut here: the renderer wraps rather than ellipsizes, whatever it is asked.
            let fit = (width / (size * 0.56)).max(0.0) as usize;
            r.fill_text(
                text::Text {
                    content: rex_backend::plain(text, fit.max(1)),
                    bounds: Size::new(width + size * 2.0, HEADER),
                    size: size.into(),
                    line_height: text::LineHeight::default(),
                    font: cosmic::font::default(),
                    align_x: text::Alignment::Left,
                    align_y: cosmic::iced::alignment::Vertical::Center,
                    shaping: text::Shaping::Advanced,
                    wrapping: text::Wrapping::None,
                    ellipsize: text::Ellipsize::End(text::EllipsizeHeightLimit::Lines(1)),
                },
                Point::new(x, rect.y + HEADER / 2.0),
                color,
                clip,
            );
        };
        // One surface: the header sits on the terminal's own background.
        let (ground, ink) = pane.frame.as_ref().map_or_else(
            || (Color::from(c.bg_color()), Color::from(c.on_bg_color())),
            |f| (color(f.bg()), color(f.fg())),
        );
        rounded(r, rect, ground, radius);
        let on = Color { a: if pane.focused { 1.0 } else { 0.7 }, ..ink };
        let chip = Rectangle { x: rect.x + 8.0, y: rect.y + 5.0, width: 22.0, height: 18.0 };
        rounded(r, chip, self.identity.tint, c.radius_xs());
        label(r, &self.identity.monogram, chip.x + 3.0, 20.0, 11.0, Color::WHITE);
        let size = self.chip(pane, rect);
        let end = size.as_ref().map_or(rect.x + rect.width - 28.0, |(c, _)| c.x);
        label(r, &pane.title, rect.x + 38.0, (end - rect.x - 46.0).max(0.0), 13.0, on);
        if let Some((bounds, text)) = &size {
            rounded(r, *bounds, Color { a: 0.1, ..ink }, c.radius_xl());
            label(r, text, bounds.x + 8.0, bounds.width - 8.0, 11.0, on);
        }
        if pane.state != rex_backend::AgentState::Idle {
            let color = crate::ui::color(pane.state, theme);
            label(r, crate::ui::shape(pane.state), rect.x + rect.width - 22.0, 18.0, 12.0, color);
        }
        let body = Rectangle { y: rect.y + HEADER, height: (rect.height - HEADER).max(0.0), ..rect };
        let outline = |r: &mut Renderer| {
            if pane.focused {
                r.fill_quad(
                    renderer::Quad {
                        bounds: rect,
                        border: Border { color: accent, width: 1.5, radius: radius.into() },
                        ..Default::default()
                    },
                    Background::Color(Color::TRANSPARENT),
                );
            }
        };
        let lower = [0.0, 0.0, radius[2], radius[3]];
        let Some(frame) = &pane.frame else {
            outline(r);
            return;
        };
        let origin = Point::new(body.x + PAD, body.y + PAD);
        let rect = body;
        let visible = (((rect.height - 2.0 * PAD) / m.cell.height).floor().max(1.0) as usize).max(1);
        // A pane someone else sized may be taller than our view: the core says which rows to show.
        let first = frame.first_row(visible);
        let cell = |col: usize, row: usize, cols: usize| Rectangle {
            x: origin.x + col as f32 * m.cell.width,
            y: origin.y + (row - first) as f32 * m.cell.height,
            width: cols as f32 * m.cell.width,
            height: m.cell.height,
        };
        let rows = self.cache.rows(&pane.block, frame, m);
        for y in (first..frame.row_count()).take(visible) {
            let row: Vec<CellRef> = frame.row(y).collect();
            let mut x = 0;
            while x < row.len() {
                let bg = row[x].bg();
                let start = x;
                while x < row.len() && row[x].bg() == bg {
                    x += 1;
                }
                if bg != frame.bg() {
                    fill(r, cell(start, y, x - start), color(bg));
                }
            }
            if let Some((a, b)) = frame.selection(y) {
                fill(r, cell(a, y, b.saturating_sub(a) + 1), Color { a: 0.35, ..accent });
            }
            for (x, buffer) in rows[y].iter() {
                r.fill_raw(Raw {
                    buffer: Arc::downgrade(buffer),
                    position: cell(*x, y, 1).position(),
                    color: color(frame.fg()),
                    clip_bounds: clip,
                });
            }
            for (x, c) in row.iter().enumerate() {
                let b = cell(x, y, 1);
                let line = |dy: f32| Rectangle { y: b.y + dy, height: 1.0, ..b };
                let c_line = color(c.underline_color());
                if c.underline() != Underline::None {
                    fill(r, line(m.cell.height - 2.0), c_line);
                    if c.underline() == Underline::Double {
                        fill(r, line(m.cell.height - 4.0), c_line);
                    }
                }
                if c.strikethrough() {
                    fill(r, line((m.cell.height / 2.0).round()), color(c.fg()));
                }
            }
        }
        if pane.live
            && let Some((x, y)) = frame.cursor()
            && (first..first + visible).contains(&(y as usize))
        {
            let guessed = pane.guess.chars().count();
            if guessed > 0 {
                // Underlined until the server confirms it.
                let span = cell(x as usize, y as usize, guessed);
                fill(r, span, color(frame.bg()));
                r.fill_raw(Raw {
                    buffer: Arc::downgrade(&self.cache.guess(&pane.block, &pane.guess, m)),
                    position: span.position(),
                    color: color(frame.fg()),
                    clip_bounds: clip,
                });
                let line = Rectangle { y: span.y + span.height - 2.0, height: 1.0, ..span };
                fill(r, line, Color { a: 0.5, ..color(frame.fg()) });
            }
            let b = cell(x as usize + guessed, y as usize, 1);
            let c = color(frame.cursor_color());
            if !pane.focused {
                r.fill_quad(
                    renderer::Quad {
                        bounds: b,
                        border: Border { color: c, width: 1.0, ..Default::default() },
                        ..Default::default()
                    },
                    Background::Color(Color::TRANSPARENT),
                );
            } else {
                match frame.cursor_shape() {
                    CursorShape::Bar => fill(r, Rectangle { width: 2.0, ..b }, c),
                    CursorShape::Underline => {
                        fill(r, Rectangle { y: b.y + b.height - 2.0, height: 2.0, ..b }, c)
                    }
                    // Translucent, so the glyph below stays readable without a second text pass.
                    _ => fill(r, b, Color { a: 0.6, ..c }),
                }
            }
        }
        if !pane.live {
            // Pattern and words, not just colour: this screen is only the last one seen.
            rounded(r, body, Color { a: 0.45, ..Color::from(c.bg_color()) }, lower);
            let mut y = body.y + 12.0;
            while y < body.y + body.height {
                fill(r, Rectangle { y, height: 1.0, ..body }, Color { a: 0.25, ..on });
                y += 12.0;
            }
            let plate = Rectangle {
                x: body.x + 12.0,
                y: body.y + 12.0,
                width: (body.width - 24.0).clamp(0.0, 300.0),
                height: HEADER,
            };
            rounded(r, plate, c.primary_component_color().into(), c.radius_s());
            use cosmic::iced::advanced::text::{self, Renderer as _};
            r.fill_text(
                text::Text {
                    content: "Paused · last screen seen · keys are not sent".to_owned(),
                    bounds: plate.size(),
                    size: 12.0.into(),
                    line_height: text::LineHeight::default(),
                    font: cosmic::font::default(),
                    align_x: text::Alignment::Left,
                    align_y: cosmic::iced::alignment::Vertical::Center,
                    shaping: text::Shaping::Advanced,
                    wrapping: text::Wrapping::None,
                    ellipsize: text::Ellipsize::End(text::EllipsizeHeightLimit::Lines(1)),
                },
                Point::new(plate.x + 10.0, plate.center_y()),
                on,
                clip,
            );
        }
        outline(r);
    }
}

impl<'a, M: 'a> From<Grid<'a, M>> for cosmic::Element<'a, M> {
    fn from(grid: Grid<'a, M>) -> Self {
        Self::new(grid)
    }
}

fn shortcut(key: &keyboard::key::Physical, m: keyboard::Modifiers) -> Option<Shortcut> {
    use keyboard::key::{Code, Physical};
    if !(m.control() && m.shift()) || m.alt() || m.logo() {
        return None;
    }
    let Physical::Code(code) = key else { return None };
    Some(match code {
        Code::KeyV => Shortcut::Paste,
        Code::KeyT => Shortcut::NewTab,
        Code::KeyD => Shortcut::SplitRight,
        Code::KeyE => Shortcut::SplitDown,
        Code::Enter => Shortcut::Zoom,
        Code::KeyA => Shortcut::Agents,
        Code::KeyS => Shortcut::Fit,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_run_stays_on_the_cell_grid() {
        for wanted in [10.0, 12.0, 14.0, 15.5, 20.0] {
            let m = Metrics::measure(wanted);
            let mut fonts = font_system().write().unwrap();
            let fonts = fonts.raw();
            let mut buffer = ct::Buffer::new(fonts, ct::Metrics::new(m.size, m.cell.height));
            buffer.set_wrap(ct::Wrap::None);
            buffer.set_text(&"M".repeat(200), &attrs(), ct::Shaping::Advanced, None);
            buffer.shape_until_scroll(fonts, false);
            let width = buffer.layout_runs().next().unwrap().line_w;
            assert!((width - 200.0 * m.cell.width).abs() < 1.0, "{wanted}: {width} vs {m:?}");
        }
    }

    #[test]
    fn only_sure_single_cells_join_a_run() {
        assert!(on_grid("a") && on_grid("é") && on_grid("─") && on_grid(""));
        assert!(!on_grid("日") && !on_grid("🙂") && !on_grid("e\u{301}") && !on_grid("\u{e0b0}"));
    }
}
