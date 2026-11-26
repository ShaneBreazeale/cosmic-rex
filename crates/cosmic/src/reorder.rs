//! A column whose rows can be dragged into a new order.
//!
//! The rows stay ordinary buttons: a press that never travels is their click. Once it travels, it
//! is a drag: a bar shows where the row would land, and letting go there reports the move, never
//! the click. Escape, or letting go outside the column, puts nothing anywhere.
use cosmic::{
    Element, Renderer, Theme,
    iced::{
        Background, Border, Event, Length, Point, Rectangle, Size, Vector,
        advanced::{
            Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer,
            widget::{Operation, Tree, tree},
        },
        keyboard,
    },
    widget,
};

/// How far a press travels before it is a drag rather than a click.
const THRESHOLD: f32 = 6.0;
const SPACING: u16 = 2;

pub struct Reorder<'a, M> {
    content: Element<'a, M>,
    on_move: Option<Box<dyn Fn(usize, usize) -> M + 'a>>,
}

/// `rows` in a column. A row dropped at a slot sends `on_move(row, slot)`: slot 0 is the top,
/// `rows.len()` the end. Without `on_move` nothing drags.
pub fn reorder<'a, M: 'a>(
    rows: Vec<Element<'a, M>>,
    on_move: Option<impl Fn(usize, usize) -> M + 'a>,
) -> Reorder<'a, M> {
    Reorder {
        content: widget::column::with_children(rows).spacing(SPACING).into(),
        on_move: on_move.map(|f| Box::new(f) as Box<dyn Fn(usize, usize) -> M + 'a>),
    }
}

#[derive(Default)]
struct State {
    /// The row a left press landed on, and where.
    press: Option<(usize, Point)>,
    dragging: bool,
}

fn rows<'b>(layout: Layout<'b>) -> impl Iterator<Item = Rectangle> + 'b {
    layout.children().next().into_iter().flat_map(|c| c.children()).map(|r| r.bounds())
}

/// The slot under `y`: the number of rows whose middle is above it.
fn slot(layout: Layout<'_>, y: f32) -> usize {
    rows(layout).filter(|r| r.center_y() < y).count()
}

/// Letting go here moves the row: over the column, or just above or below it.
fn droppable(layout: Layout<'_>, cursor: mouse::Cursor) -> Option<Point> {
    let b = layout.bounds();
    let reach = Rectangle { y: b.y - 12.0, height: b.height + 24.0, ..b };
    cursor.position().filter(|p| reach.contains(*p))
}

impl<M> Widget<M, Theme, Renderer> for Reorder<'_, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let child = self.content.as_widget_mut().layout(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(child.size(), vec![child])
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let child = layout.children().next().expect("column");
        self.content.as_widget_mut().operate(&mut tree.children[0], child, renderer, operation);
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let mut cursor = cursor;
        if let Some(on_move) = &self.on_move {
            let state = tree.state.downcast_mut::<State>();
            match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    state.dragging = false;
                    state.press = cursor
                        .position()
                        .and_then(|p| rows(layout).position(|r| r.contains(p)).map(|i| (i, p)));
                }
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    if let Some((_, start)) = state.press
                        && !state.dragging
                        && start.distance(*position) > THRESHOLD
                    {
                        state.dragging = true;
                    }
                    if state.dragging {
                        shell.request_redraw();
                    }
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    let press = state.press.take();
                    if std::mem::take(&mut state.dragging) {
                        if let Some((row, _)) = press
                            && let Some(p) = droppable(layout, cursor)
                        {
                            shell.publish(on_move(row, slot(layout, p.y)));
                        }
                        // The row sees its press end away from it, so it does not click.
                        cursor = mouse::Cursor::Unavailable;
                        shell.request_redraw();
                    }
                }
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) if state.dragging => {
                    state.press = None;
                    state.dragging = false;
                    shell.request_redraw();
                    shell.capture_event();
                    return;
                }
                _ => {}
            }
            if state.dragging && !matches!(event, Event::Mouse(mouse::Event::ButtonReleased(_))) {
                // Rows under a drag don't light up as if pointed at.
                cursor = mouse::Cursor::Unavailable;
            }
        }
        let child = layout.children().next().expect("column");
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        if tree.state.downcast_ref::<State>().dragging {
            return mouse::Interaction::Grabbing;
        }
        let child = layout.children().next().expect("column");
        self.content.as_widget().mouse_interaction(&tree.children[0], child, cursor, viewport, renderer)
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let child = layout.children().next().expect("column");
        self.content.as_widget().draw(&tree.children[0], renderer, theme, style, child, cursor, viewport);
        let state = tree.state.downcast_ref::<State>();
        let (true, Some(p)) = (state.dragging, droppable(layout, cursor)) else { return };
        let bounds: Vec<Rectangle> = rows(layout).collect();
        let slot = slot(layout, p.y);
        let gap = f32::from(SPACING) / 2.0;
        let y = match bounds.get(slot) {
            Some(r) => r.y - gap,
            None => bounds.last().map_or(layout.bounds().y, |r| r.y + r.height + gap),
        };
        let b = layout.bounds();
        let c = theme.cosmic();
        <Renderer as renderer::Renderer>::fill_quad(
            renderer,
            renderer::Quad {
                bounds: Rectangle { x: b.x + 4.0, y: y - 1.0, width: b.width - 8.0, height: 2.0 },
                border: Border { radius: 1.0.into(), ..Default::default() },
                ..Default::default()
            },
            Background::Color(c.accent_color().into()),
        );
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        let child = layout.children().next().expect("column");
        self.content.as_widget_mut().overlay(&mut tree.children[0], child, renderer, viewport, translation)
    }
}

impl<'a, M: 'a> From<Reorder<'a, M>> for Element<'a, M> {
    fn from(r: Reorder<'a, M>) -> Self {
        Element::new(r)
    }
}
