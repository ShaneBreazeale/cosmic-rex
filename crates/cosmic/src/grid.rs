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
            layout, renderer, svg,
            renderer::Renderer as _,
            widget::{Tree, tree},
        },
        keyboard, mouse,
    },
};
use rex_backend::{
    Button, CellRef, CursorShape, Frame, Key, KeyAction, Mouse, PaneView, Pointer, Select, Size as Owner, Underline,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};

use crate::keys;

pub const PAD: f32 = 10.0;
const HEADER: f32 = 34.0;
const GAP: f32 = 12.0;
/// One header button, and the order they sit in.
const BUTTON: f32 = 26.0;
const BUTTONS: [Shortcut; 4] = [Shortcut::SplitRight, Shortcut::SplitDown, Shortcut::Zoom, Shortcut::ClosePane];
/// How long a pane may be down before it is drawn as paused. Panes coming into view, as when a
/// tab closes and the next one shows, connect within this, and so never flash as paused.
const PAUSE_GRACE: Duration = Duration::from_millis(1200);
/// Presses closer together than this, on the same cell, count as a double or triple click.
const MULTI_CLICK: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub size: f32,
    pub cell: Size,
    /// Screen pixels per layout pixel: 1.5 on a 1440p 24-inch screen.
    pub scale: f32,
}

impl Metrics {
    /// The nearest size to `wanted` whose advance is a whole number of screen pixels at
    /// `scale`, so glyphs, backgrounds and underlines stay on the screen's own pixel grid
    /// however long the row is. A half-pixel advance would blur every other glyph.
    pub fn measure(wanted: f32, scale: f32) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let mut fonts = font_system().write().expect("font system");
        let advance = advance(fonts.raw(), wanted);
        let width = (advance * scale).round().max(1.0) / scale;
        Self {
            size: wanted * width / advance,
            cell: Size::new(width, (wanted * 1.45 * scale).round().max(1.0) / scale),
            scale,
        }
    }

    /// These metrics, or smaller ones that show all of a `cols` × `rows` grid in a pane of
    /// `space`: a pane another device sized is drawn whole rather than cut off, and nothing is
    /// resized. Never below 70% of the usual size, which stays readable; past that the grid is
    /// cut off as before, keeping the rows around the cursor.
    pub fn fit(&self, cols: u16, rows: u16, space: Size) -> Self {
        let (w, h) = (space.width - 2.0 * PAD, space.height - HEADER - 2.0 * PAD);
        let fits = |m: &Self| f32::from(cols) * m.cell.width <= w && f32::from(rows) * m.cell.height <= h;
        if fits(self) || cols == 0 || rows == 0 {
            return *self;
        }
        let ratio = (w / (f32::from(cols) * self.cell.width)).min(h / (f32::from(rows) * self.cell.height));
        let floor = (self.size * 0.7).max(6.0);
        let mut wanted = (self.size * ratio).max(floor);
        loop {
            let m = Self::measure(wanted, self.scale);
            if fits(&m) || wanted <= floor {
                return m;
            }
            wanted = (wanted - 0.25).max(floor);
        }
    }

    /// The nearest edge between screen pixels.
    pub fn snap(&self, v: f32) -> f32 {
        (v * self.scale).round() / self.scale
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

/// Base size and scale, grid columns and rows, pane width and height.
type FitKey = (u32, u16, u16, u32, u32);

/// Shaped rows per pane, owned by the app so they outlive a view.
#[derive(Default)]
pub struct Cache {
    panes: RefCell<HashMap<String, Vec<Row>>>,
    /// Guessed text being drawn; the renderer only keeps weak references.
    guesses: RefCell<HashMap<String, (String, Arc<ct::Buffer>)>>,
    pub shaped: std::cell::Cell<u64>,
    /// Fitted metrics by base size, grid and pane size: measuring shapes text.
    fits: RefCell<HashMap<FitKey, Metrics>>,
}

impl Cache {
    pub fn retain(&self, keep: impl Fn(&str) -> bool) {
        self.panes.borrow_mut().retain(|block, _| keep(block));
        self.guesses.borrow_mut().retain(|block, _| keep(block));
    }

    fn fit(&self, m: Metrics, cols: u16, rows: u16, space: Size) -> Metrics {
        let key = (m.size.to_bits() ^ m.scale.to_bits().rotate_left(16), cols, rows, space.width as u32, space.height as u32);
        let mut fits = self.fits.borrow_mut();
        if fits.len() > 256 {
            fits.clear();
        }
        *fits.entry(key).or_insert_with(|| m.fit(cols, rows, space))
    }

    fn guess(&self, block: &str, text: &str, m: Metrics) -> Arc<ct::Buffer> {
        let mut guesses = self.guesses.borrow_mut();
        if let Some((old, buffer)) = guesses.get(block)
            && old == text
            && buffer.metrics().font_size == m.size
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

/// The header buttons' icons, as vectors: drawn at the display's own scale, and the same on
/// every machine, which theme icons are not.
fn icon(s: Shortcut, zoomed: bool) -> &'static svg::Handle {
    use std::sync::LazyLock;
    fn make(body: &str) -> svg::Handle {
        svg::Handle::from_memory(format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="black" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
        ).into_bytes())
    }
    static RIGHT: LazyLock<svg::Handle> =
        LazyLock::new(|| make(r#"<rect x="1.7" y="2.7" width="12.6" height="10.6" rx="2"/><path d="M8 2.7v10.6"/>"#));
    static DOWN: LazyLock<svg::Handle> =
        LazyLock::new(|| make(r#"<rect x="1.7" y="2.7" width="12.6" height="10.6" rx="2"/><path d="M1.7 8h12.6"/>"#));
    static ZOOM: LazyLock<svg::Handle> =
        LazyLock::new(|| make(r#"<path d="M3 3l10 10M3 7.5V3h4.5M13 8.5V13H8.5"/>"#));
    static RESTORE: LazyLock<svg::Handle> =
        LazyLock::new(|| make(r#"<path d="M2.5 2.5l4.5 4.5M7 3.2V7H3.2M13.5 13.5L9 9M9 12.8V9h3.8"/>"#));
    static CLOSE: LazyLock<svg::Handle> = LazyLock::new(|| make(r#"<path d="M4 4l8 8M12 4l-8 8"/>"#));
    match s {
        Shortcut::SplitRight => &RIGHT,
        Shortcut::SplitDown => &DOWN,
        Shortcut::Zoom if zoomed => &RESTORE,
        Shortcut::Zoom => &ZOOM,
        _ => &CLOSE,
    }
}

fn glyph(r: &mut Renderer, b: Rectangle, s: Shortcut, zoomed: bool, ink: Color, clip: Rectangle) {
    use cosmic::iced::advanced::svg::Renderer as _;
    let at = Rectangle { x: b.center_x() - 8.0, y: b.center_y() - 8.0, width: 16.0, height: 16.0 };
    r.draw_svg(svg::Svg::new(icon(s, zoomed).clone()).color(ink), at, clip);
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
    /// A header button: this pane, not whichever has the keyboard.
    Pane(String, Shortcut),
    /// A step of selecting with the pointer in this pane.
    Select(String, Select, Pointer),
    /// A mouse report for the program in this pane, which asked for them: the cell and the
    /// modifiers held, never Shift.
    Mouse(String, Mouse, (u16, u16), String),
    /// The right button went down here: the pane menu about to open acts on this pane.
    Menu(String),
    /// A middle click: paste the primary selection here.
    PastePrimary(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shortcut {
    Copy,
    Paste,
    NewTab,
    SplitRight,
    SplitDown,
    Zoom,
    /// Asks first: it ends the pane's program.
    ClosePane,
    Agents,
    Fit,
    FontBigger,
    FontSmaller,
    FontReset,
}

/// What holds for the tab on screen as a whole.
#[derive(Clone, Debug)]
pub struct Tab {
    /// False while a dialog has the keyboard: no key may reach a terminal then.
    pub keyboard: bool,
    /// Its programs have ended, so it is not "paused".
    pub closing: bool,
    /// Panes whose close was confirmed: their program has ended, so they are not "paused".
    pub closing_panes: Vec<String>,
    /// Panes in it, shown or behind a zoomed one: zooming needs two.
    pub panes: usize,
    /// It holds the session's only pane, which its header does not offer to close.
    pub last: bool,
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
    tab: Tab,
    cache: &'a Cache,
    metrics: Metrics,
    on: Box<dyn Fn(Action) -> M + 'a>,
}

impl<'a, M> Grid<'a, M> {
    pub fn new(
        panes: Vec<PaneView>,
        identity: Identity,
        tab: Tab,
        cache: &'a Cache,
        metrics: Metrics,
        on: impl Fn(Action) -> M + 'a,
    ) -> Self {
        Self { panes, identity, tab, cache, metrics, on: Box::new(on) }
    }

    fn rect(&self, pane: &PaneView, b: Rectangle) -> Rectangle {
        let [x, y, w, h] = pane.rect.map(|v| v as f32);
        // Half the gap on inner edges only, so outer edges line up with the rest of the window.
        let (l, t) = (if x > 0.001 { GAP / 2.0 } else { 0.0 }, if y > 0.001 { GAP / 2.0 } else { 0.0 });
        let r = if x + w < 0.999 { GAP / 2.0 } else { 0.0 };
        let bottom = if y + h < 0.999 { GAP / 2.0 } else { 0.0 };
        // Edges on screen pixels, so hairlines and text are sharp at any scale.
        let snap = |v: f32| self.metrics.snap(v);
        let (left, top) = (snap(b.x + x * b.width + l), snap(b.y + y * b.height + t));
        Rectangle {
            x: left,
            y: top,
            width: (snap(b.x + (x + w) * b.width - r) - left).max(1.0),
            height: (snap(b.y + (y + h) * b.height - bottom) - top).max(1.0),
        }
    }

    /// The cell size this pane is drawn at: ours always fits, as we size it; another device's
    /// grid is drawn smaller if that is what it takes to show all of it.
    fn pane_metrics(&self, pane: &PaneView, rect: Rectangle) -> Metrics {
        match (&pane.frame, pane.size) {
            (Some(f), Owner::Elsewhere | Owner::Unclaimed) => self.cache.fit(self.metrics, f.cols(), f.rows(), rect.size()),
            _ => self.metrics,
        }
    }

    /// The top-left corner of a pane's character grid, on a screen pixel.
    fn origin(&self, rect: Rectangle) -> Point {
        Point::new(self.metrics.snap(rect.x + PAD), self.metrics.snap(rect.y + HEADER + PAD))
    }

    /// Split right, split down, zoom and close at the header's end, as in Rex. A narrow pane
    /// keeps zoom and close; one narrower still has none.
    fn buttons(&self, rect: Rectangle) -> Vec<(Rectangle, Shortcut)> {
        let shown = match rect.width {
            w if w >= 300.0 => &BUTTONS[..],
            w if w >= 180.0 => &BUTTONS[2..],
            _ => return Vec::new(),
        };
        let end = rect.x + rect.width - 8.0;
        let y = rect.y + (HEADER - BUTTON) / 2.0;
        (shown.iter().enumerate())
            .map(|(i, s)| {
                let x = end - (shown.len() - i) as f32 * BUTTON;
                (Rectangle { x, y, width: BUTTON, height: BUTTON }, *s)
            })
            .collect()
    }

    /// Whether a header button can act on this pane now.
    fn can(&self, pane: &PaneView, s: Shortcut) -> bool {
        match s {
            Shortcut::Zoom => pane.zoomed || self.tab.panes > 1,
            Shortcut::ClosePane => !self.tab.last,
            _ => true,
        }
    }

    /// Where the header's own text and chips must end.
    fn header_end(&self, rect: Rectangle) -> f32 {
        self.buttons(rect).first().map_or(rect.x + rect.width - 8.0, |(b, _)| b.x - 4.0)
    }

    fn chip(&self, pane: &PaneView, rect: Rectangle) -> Option<(Rectangle, String)> {
        let grid = pane.frame.as_ref().map(|f| format!(" · {}×{}", f.cols(), f.rows())).unwrap_or_default();
        let (long, short) = match pane.size {
            Owner::Here => return None,
            Owner::Elsewhere => ("Sized elsewhere", "Elsewhere"),
            Owner::Unclaimed => ("Unclaimed", "Unclaimed"),
        };
        let end = self.header_end(rect) - 22.0;
        // Shortened rather than dropped: whose size this is must never go unsaid.
        [
            format!("{long}{grid} · Fit here"),
            format!("{long} · Fit here"),
            format!("{short} · Fit here"),
            format!("⊡ {short}"),
            "⊡".to_owned(),
        ]
        .into_iter()
        .find_map(|text| {
            let width = text.chars().count() as f32 * 5.9 + 18.0;
            let fits = width + 100.0 < end - rect.x || text == "⊡";
            fits.then_some((Rectangle { x: end - width, y: rect.y + 7.0, width, height: HEADER - 14.0 }, text))
        })
    }

    /// Topmost pane first: floating layers come after the tiled one.
    fn at(&self, p: Point, b: Rectangle) -> Option<&PaneView> {
        self.panes.iter().rev().find(|pane| self.rect(pane, b).contains(p))
    }

    /// The terminal under a point, which may be outside the pane while a drag goes past its
    /// edge: the cell is clamped to the rows on screen, the position is not.
    fn pointer(&self, pane: &PaneView, rect: Rectangle, p: Point) -> Option<Pointer> {
        let frame = pane.frame.as_ref()?;
        let m = self.pane_metrics(pane, rect);
        let origin = self.origin(rect);
        let (x, y) = (p.x - origin.x, p.y - origin.y);
        let visible = ((rect.height - HEADER - 2.0 * PAD) / m.cell.height).floor().max(1.0) as usize;
        let first = frame.first_row(visible);
        let col = ((x / m.cell.width).floor().max(0.0) as usize).min(usize::from(frame.cols().max(1)) - 1);
        let row = ((y / m.cell.height).floor().max(0.0) as usize).min(visible - 1) + first;
        let row = row.min(usize::from(frame.rows().max(1)) - 1);
        Some(Pointer {
            cell: (col as u16, row as u16),
            position: (x, y),
            cell_width: m.cell.width,
            height: visible as f32 * m.cell.height,
        })
    }

    /// The program asked for the mouse, and Shift is not held to select instead.
    fn reports(pane: &PaneView, m: keyboard::Modifiers) -> bool {
        pane.frame.as_ref().is_some_and(Frame::mouse_tracking) && !m.shift()
    }
}

/// Modifiers as a mouse report carries them. Shift never goes along: it is the selection override.
fn mouse_mods(m: keyboard::Modifiers) -> String {
    [(m.control(), "ctrl"), (m.alt(), "alt"), (m.logo(), "super")]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect::<Vec<_>>()
        .join("+")
}

/// A left-button gesture under way, in the pane it began in.
struct Press {
    block: String,
    /// Reported to the program rather than selecting.
    reported: bool,
    cell: (u16, u16),
}

#[derive(Default)]
struct State {
    sizes: HashMap<String, (u16, u16)>,
    modifiers: keyboard::Modifiers,
    scroll: f32,
    /// Presses sent to the program, so a release is sent only for those.
    down: std::collections::HashSet<keyboard::key::Physical>,
    press: Option<Press>,
    /// A middle press reported to the program, so its release is too.
    middle: Option<(String, (u16, u16))>,
    /// The last left press, for counting double and triple clicks.
    click: Option<(Instant, String, (u16, u16), u32)>,
    /// Since when each pane on screen has not been live.
    down_since: HashMap<String, Instant>,
    /// The pane under the pointer and the header button, if any, as last drawn.
    hover: Option<(String, Option<Shortcut>)>,
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
        let now = Instant::now();
        state.down_since.retain(|b, _| self.panes.iter().any(|p| &p.block == b && !p.live));
        for pane in self.panes.iter().filter(|p| !p.live) {
            let since = *state.down_since.entry(pane.block.clone()).or_insert(now);
            if now < since + PAUSE_GRACE {
                shell.request_redraw_at(since + PAUSE_GRACE);
            }
        }

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => state.modifiers = *m,
            Event::Keyboard(keyboard::Event::KeyPressed {
                key, physical_key, modifiers, text, repeat, ..
            }) if self.tab.keyboard && !shell.is_event_captured() => {
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
                let rect = self.rect(pane, bounds);
                if let Some((_, s)) = self.buttons(rect).into_iter().find(|(b, _)| b.contains(p)) {
                    if self.can(pane, s) {
                        shell.publish((self.on)(Action::Pane(pane.block.clone(), s)));
                    }
                    shell.capture_event();
                    return;
                } else if self.chip(pane, rect).is_some_and(|(c, _)| c.contains(p)) {
                    shell.publish((self.on)(Action::Fit(pane.block.clone())));
                    shell.capture_event();
                    return;
                }
                if !pane.focused {
                    shell.publish((self.on)(Action::Focus(pane.block.clone())));
                }
                shell.capture_event();
                let Some(at) = self.pointer(pane, rect, p).filter(|_| p.y >= rect.y + HEADER) else { return };
                let block = pane.block.clone();
                if Self::reports(pane, state.modifiers) {
                    if self.tab.keyboard {
                        let report = Mouse::Press(Button::Left);
                        shell.publish((self.on)(Action::Mouse(block.clone(), report, at.cell, mouse_mods(state.modifiers))));
                        state.press = Some(Press { block, reported: true, cell: at.cell });
                    }
                    return;
                }
                let clicks = match &state.click {
                    Some((when, b, cell, n)) if when.elapsed() < MULTI_CLICK && *b == block && *cell == at.cell => n % 3 + 1,
                    _ => 1,
                };
                state.click = Some((Instant::now(), block.clone(), at.cell, clicks));
                shell.publish((self.on)(Action::Select(block.clone(), Select::Press(clicks), at)));
                state.press = Some(Press { block, reported: false, cell: at.cell });
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                // Nothing else redraws this widget as the pointer goes from button to button.
                let hover = cursor.position_over(bounds).and_then(|p| {
                    let pane = self.at(p, bounds)?;
                    let button = self.buttons(self.rect(pane, bounds)).into_iter().find(|(b, _)| b.contains(p));
                    Some((pane.block.clone(), button.map(|(_, s)| s)))
                });
                if hover != state.hover {
                    state.hover = hover;
                    shell.request_redraw();
                }
                let Some(press) = &mut state.press else { return };
                let Some(pane) = self.panes.iter().find(|p| p.block == press.block) else { return };
                let Some(at) = self.pointer(pane, self.rect(pane, bounds), *position) else { return };
                // A report per cell crossed, and a selection step per cell too: the core
                // places a selection by cell and half cell, from the position.
                if press.reported {
                    if at.cell != press.cell {
                        let report = Mouse::Drag(Button::Left);
                        shell.publish((self.on)(Action::Mouse(press.block.clone(), report, at.cell, mouse_mods(state.modifiers))));
                    }
                } else {
                    shell.publish((self.on)(Action::Select(press.block.clone(), Select::Drag, at)));
                }
                press.cell = at.cell;
            }
            Event::Mouse(mouse::Event::CursorLeft) => {
                if state.hover.take().is_some() {
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let Some(press) = state.press.take() else { return };
                let Some(pane) = self.panes.iter().find(|p| p.block == press.block) else { return };
                let at = cursor
                    .position()
                    .and_then(|p| self.pointer(pane, self.rect(pane, bounds), p))
                    .unwrap_or(Pointer { cell: press.cell, position: (0.0, 0.0), cell_width: 1.0, height: 1.0 });
                shell.publish((self.on)(if press.reported {
                    Action::Mouse(press.block, Mouse::Release(Button::Left), at.cell, mouse_mods(state.modifiers))
                } else {
                    Action::Select(press.block, Select::Release, at)
                }));
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                // Not captured: the pane menu around this widget opens on the release.
                let Some(p) = cursor.position_over(bounds) else { return };
                if let Some(pane) = self.at(p, bounds) {
                    shell.publish((self.on)(Action::Menu(pane.block.clone())));
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)) => {
                let Some(p) = cursor.position_over(bounds) else { return };
                let Some(pane) = self.at(p, bounds) else { return };
                let rect = self.rect(pane, bounds);
                let Some(at) = self.pointer(pane, rect, p).filter(|_| p.y >= rect.y + HEADER) else { return };
                if !self.tab.keyboard {
                    return;
                }
                shell.capture_event();
                let block = pane.block.clone();
                if Self::reports(pane, state.modifiers) {
                    let report = Mouse::Press(Button::Middle);
                    shell.publish((self.on)(Action::Mouse(block.clone(), report, at.cell, mouse_mods(state.modifiers))));
                    state.middle = Some((block, at.cell));
                } else {
                    shell.publish((self.on)(Action::PastePrimary(block)));
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Middle)) => {
                if let Some((block, cell)) = state.middle.take() {
                    let report = Mouse::Release(Button::Middle);
                    shell.publish((self.on)(Action::Mouse(block, report, cell, mouse_mods(state.modifiers))));
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
                let at = self.pointer(pane, self.rect(pane, bounds), p);
                shell.publish((self.on)(if let Some(at) = at.filter(|_| Self::reports(pane, state.modifiers)) {
                    Action::Wheel(pane.block.clone(), at.cell.0, at.cell.1, lines as i32)
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
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _: &Rectangle,
        _: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        if let Some(p) = cursor.position_over(bounds) {
            let Some(pane) = self.at(p, bounds) else { return mouse::Interaction::None };
            let rect = self.rect(pane, bounds);
            if self.chip(pane, rect).is_some_and(|(c, _)| c.contains(p))
                || self.buttons(rect).iter().any(|(b, _)| b.contains(p))
            {
                mouse::Interaction::Pointer
            } else if p.y < rect.y + HEADER || Self::reports(pane, tree.state.downcast_ref::<State>().modifiers) {
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
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _: &renderer::Style,
        layout: Layout<'_>,
        _: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        // What `update` last found under the pointer, the same state that asked for this frame.
        let state = tree.state.downcast_ref::<State>();
        let over = &state.hover;
        for pane in &self.panes {
            let rect = self.rect(pane, bounds);
            let Some(clip) = rect.intersection(viewport) else { continue };
            let hover = over.as_ref().filter(|(b, _)| *b == pane.block).map(|(_, s)| *s);
            // A layer per pane keeps a wider server-sized grid from spilling into neighbours.
            let paused = !pane.live && state.down_since.get(&pane.block).is_some_and(|t| t.elapsed() >= PAUSE_GRACE);
            renderer.with_layer(clip, |r| self.draw_pane(r, pane, rect, clip, theme, hover, paused));
        }
    }
}

impl<M> Grid<'_, M> {
    #[allow(clippy::too_many_arguments)]
    fn draw_pane(
        &self,
        r: &mut Renderer,
        pane: &PaneView,
        rect: Rectangle,
        clip: Rectangle,
        theme: &Theme,
        // Over this pane, and over which of its buttons.
        hover: Option<Option<Shortcut>>,
        // Down for longer than a reconnect takes.
        paused: bool,
    ) {
        let m = self.pane_metrics(pane, rect);
        let c = theme.cosmic();
        let accent: Color = c.accent_color().into();
        let radius = c.radius_m();
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
            || (Color::from(c.primary_container_color()), Color::from(c.on_bg_color())),
            |f| (color(f.bg()), color(f.fg())),
        );
        rounded(r, rect, ground, radius);
        let on = Color { a: if pane.focused { 1.0 } else { 0.62 }, ..ink };
        let chip = Rectangle { x: rect.x + 10.0, y: rect.y + 8.0, width: 22.0, height: 18.0 };
        rounded(r, chip, self.identity.tint, c.radius_xs());
        label(r, &self.identity.monogram, chip.x + 3.0, 20.0, 11.0, Color::WHITE);
        let size = self.chip(pane, rect);
        let busy = pane.state != rex_backend::AgentState::Idle;
        let last = self.header_end(rect);
        let end = size.as_ref().map_or(last - if busy { 22.0 } else { 0.0 }, |(c, _)| c.x);
        label(r, &pane.title, rect.x + 42.0, (end - rect.x - 50.0).max(0.0), 14.0, on);
        if let Some((bounds, text)) = &size {
            rounded(r, *bounds, Color { a: 0.1, ..ink }, c.radius_xl());
            label(r, text, bounds.x + 8.0, bounds.width - 8.0, 11.0, on);
        }
        if busy {
            let color = crate::ui::color(pane.state, theme);
            label(r, crate::ui::shape(pane.state), last - 18.0, 18.0, 12.0, color);
        }
        // Quiet until wanted: the buttons show on the pane with the keyboard or under the pointer.
        if hover.is_some() || pane.focused {
            for (b, s) in self.buttons(rect) {
                let can = self.can(pane, s);
                if can && hover == Some(Some(s)) {
                    rounded(r, b, Color { a: 0.1, ..ink }, c.radius_s());
                }
                glyph(r, b, s, pane.zoomed, Color { a: if can { 0.8 } else { 0.3 }, ..ink }, clip);
            }
        }
        let body = Rectangle { y: rect.y + HEADER, height: (rect.height - HEADER).max(0.0), ..rect };
        let outline = |r: &mut Renderer| {
            // A hairline on every card; a soft accent one only where the keyboard is.
            let (color, width) =
                if pane.focused { (Color { a: 0.5, ..accent }, 1.0) } else { (Color { a: 0.08, ..ink }, 1.0) };
            {
                r.fill_quad(
                    renderer::Quad {
                        bounds: rect,
                        border: Border { color, width, radius: radius.into() },
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
        let origin = self.origin(Rectangle { y: body.y - HEADER, ..body });
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
                fill(r, span, ground);
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
        // A pane being closed only dims: its program has ended, and words would flash.
        let closing = self.tab.closing || self.tab.closing_panes.contains(&pane.block);
        if closing {
            rounded(r, body, Color { a: 0.45, ..Color::from(c.bg_color()) }, lower);
        } else if paused {
            rounded(r, body, Color { a: 0.45, ..Color::from(c.bg_color()) }, lower);
            // Pattern and words, not just colour: this screen is only the last one seen.
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
    let Physical::Code(code) = key else { return None };
    if m.alt() || m.logo() || !m.control() {
        return None;
    }
    // Ctrl with or without Shift, as in other terminals: Ctrl+= and Ctrl++ are one key.
    let font = match code {
        Code::Equal | Code::NumpadAdd => Some(Shortcut::FontBigger),
        Code::Minus | Code::NumpadSubtract => Some(Shortcut::FontSmaller),
        Code::Digit0 | Code::Numpad0 => Some(Shortcut::FontReset),
        _ => None,
    };
    if font.is_some() {
        return font;
    }
    if !m.shift() {
        return None;
    }
    Some(match code {
        Code::KeyC => Shortcut::Copy,
        Code::KeyV => Shortcut::Paste,
        Code::KeyT => Shortcut::NewTab,
        Code::KeyD => Shortcut::SplitRight,
        Code::KeyE => Shortcut::SplitDown,
        Code::KeyW => Shortcut::ClosePane,
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
    fn font_keys_take_ctrl_with_or_without_shift() {
        use keyboard::key::{Code, Physical};
        let (ctrl, shift) = (keyboard::Modifiers::CTRL, keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT);
        let at = |c, m| shortcut(&Physical::Code(c), m);
        assert_eq!(at(Code::Equal, ctrl), Some(Shortcut::FontBigger));
        assert_eq!(at(Code::Equal, shift), Some(Shortcut::FontBigger));
        assert_eq!(at(Code::Minus, ctrl), Some(Shortcut::FontSmaller));
        assert_eq!(at(Code::Digit0, ctrl), Some(Shortcut::FontReset));
        // The rest stay Ctrl+Shift, so Ctrl+D and friends still reach the program.
        assert_eq!(at(Code::KeyD, ctrl), None);
        assert_eq!(at(Code::Minus, keyboard::Modifiers::empty()), None);
    }

    #[test]
    fn a_long_run_stays_on_the_cell_grid() {
        for (wanted, scale) in [(10.0, 1.0), (12.0, 1.0), (14.0, 1.5), (15.5, 1.25), (20.0, 2.0), (13.0, 1.5)] {
            let m = Metrics::measure(wanted, scale);
            for v in [m.cell.width, m.cell.height] {
                assert!(((v * scale) - (v * scale).round()).abs() < 1e-3, "{wanted}@{scale}: {m:?}");
            }
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
    fn the_pointer_finds_its_cell_and_a_drag_past_the_edge_stays_on_the_grid() {
        let panes = rex_backend::demo::Demo::new().panes();
        let pane = panes[0].clone();
        let frame = pane.frame.clone().unwrap();
        let cache = Cache::default();
        let m = Metrics { size: 14.0, cell: Size::new(8.0, 18.0), scale: 1.0 };
        let grid = Grid::new(panes, Identity { monogram: "AB".into(), tint: Color::BLACK }, Tab { keyboard: true, closing: false, closing_panes: Vec::new(), panes: 2, last: false }, &cache, m, |a| a);
        let rect = Rectangle { x: 100.0, y: 50.0, width: 800.0, height: 600.0 };
        let origin = Point::new(rect.x + PAD, rect.y + HEADER + PAD);
        let at = |dx: f32, dy: f32| grid.pointer(&pane, rect, Point::new(origin.x + dx, origin.y + dy)).unwrap();
        let visible = ((rect.height - HEADER - 2.0 * PAD) / 18.0).floor() as usize;
        let first = frame.first_row(visible) as u16;
        assert_eq!(at(0.0, 0.0).cell, (0, first));
        assert_eq!(at(8.0 * 3.0 + 7.9, 18.0 * 2.0 + 1.0).cell, (3, first + 2));
        assert_eq!(at(28.5, 0.0).position, (28.5, 0.0));
        // Past the top-left and the bottom-right, the cell stops at the grid's edge.
        let above = at(-50.0, -40.0);
        assert_eq!((above.cell, above.position), ((0, first), (-50.0, -40.0)));
        let below = at(1e4, 1e4).cell;
        assert_eq!(below.0, frame.cols() - 1);
        assert!(below.1 < frame.rows() && usize::from(below.1 - first) < visible);
    }

    #[test]
    fn a_grid_sized_elsewhere_is_drawn_whole_and_never_bigger() {
        let m = Metrics::measure(14.0, 1.5);
        let room = |m: &Metrics, cols: u16, rows: u16| {
            (f32::from(cols) * m.cell.width + 2.0 * PAD, f32::from(rows) * m.cell.height + HEADER + 2.0 * PAD)
        };
        let space = Size::new(700.0, 500.0);
        assert_eq!(m.fit(40, 10, space), m, "a grid that fits keeps the usual size");
        for (cols, rows) in [(100, 20), (70, 28), (110, 30)] {
            let f = m.fit(cols, rows, space);
            let (w, h) = room(&f, cols, rows);
            assert!(w <= space.width && h <= space.height && f.size < m.size, "{cols}x{rows}: {f:?}");
            assert!(((f.cell.width * 1.5) - (f.cell.width * 1.5).round()).abs() < 1e-3);
        }
        let floor = m.fit(2000, 900, space);
        assert!(floor.size >= m.size * 0.69, "never unreadably small: {floor:?}");
    }

    #[test]
    fn mouse_reports_never_carry_shift() {
        use keyboard::Modifiers as M;
        assert_eq!(mouse_mods(M::SHIFT | M::CTRL | M::ALT), "ctrl+alt");
        assert_eq!(mouse_mods(M::SHIFT), "");
    }

    #[test]
    fn only_sure_single_cells_join_a_run() {
        assert!(on_grid("a") && on_grid("é") && on_grid("─") && on_grid(""));
        assert!(!on_grid("日") && !on_grid("🙂") && !on_grid("e\u{301}") && !on_grid("\u{e0b0}"));
    }
}
