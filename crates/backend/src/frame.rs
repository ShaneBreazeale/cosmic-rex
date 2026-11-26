//! A terminal's screen, read in place: no cell is copied on the way to the renderer.
use ghostty_vt::style::Underline as CoreUnderline;
use rex_rune::terminal as core;
use std::sync::Arc;

#[derive(Clone)]
pub struct Frame(pub(crate) Arc<core::Frame>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
    Hollow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Underline {
    None,
    Single,
    Double,
    /// Curly, dotted or dashed: drawn as single until the renderer has patterns.
    Other,
}

/// One cell. Colours are 0xRRGGBB, with inverse already applied and invisible text removed.
#[derive(Clone, Copy)]
pub struct CellRef<'a>(&'a core::Cell);

impl<'a> CellRef<'a> {
    pub fn text(self) -> &'a str {
        &self.0.text
    }
    /// The second half of a wide character.
    pub fn is_spacer(self) -> bool {
        self.0.spacer_tail
    }
    pub fn fg(self) -> u32 {
        self.0.fg
    }
    pub fn bg(self) -> u32 {
        self.0.bg
    }
    pub fn bold(self) -> bool {
        self.0.style.bold
    }
    pub fn italic(self) -> bool {
        self.0.style.italic
    }
    pub fn faint(self) -> bool {
        self.0.style.faint
    }
    pub fn strikethrough(self) -> bool {
        self.0.style.strikethrough
    }
    pub fn underline(self) -> Underline {
        match self.0.style.underline {
            CoreUnderline::None => Underline::None,
            CoreUnderline::Single => Underline::Single,
            CoreUnderline::Double => Underline::Double,
            _ => Underline::Other,
        }
    }
    pub fn underline_color(self) -> u32 {
        self.0.underline_color.unwrap_or(self.0.fg)
    }
}

impl Frame {
    pub fn cols(&self) -> u16 {
        self.0.cols
    }
    pub fn rows(&self) -> u16 {
        self.0.rows
    }
    pub fn fg(&self) -> u32 {
        self.0.fg
    }
    pub fn bg(&self) -> u32 {
        self.0.bg
    }
    pub fn row_count(&self) -> usize {
        self.0.cells.len()
    }
    pub fn row(&self, y: usize) -> impl ExactSizeIterator<Item = CellRef<'_>> + DoubleEndedIterator {
        self.0.cells[y].iter().map(CellRef)
    }
    /// Selected columns of a row, inclusive.
    pub fn selection(&self, y: usize) -> Option<(usize, usize)> {
        self.0.selection_rows.get(y).copied().flatten()
    }
    pub fn cursor(&self) -> Option<(u16, u16)> {
        self.0.cursor
    }
    pub fn cursor_color(&self) -> u32 {
        self.0.cursor_color
    }
    pub fn cursor_shape(&self) -> CursorShape {
        match self.0.cursor_shape {
            core::CursorShape::Block => CursorShape::Block,
            core::CursorShape::Bar => CursorShape::Bar,
            core::CursorShape::Underline => CursorShape::Underline,
            core::CursorShape::Hollow => CursorShape::Hollow,
        }
    }
    /// The selection's text, unwrapped and trimmed, if anything is selected. Terminal text:
    /// for the clipboard only, never shown as a label.
    pub fn selected_text(&self) -> Option<&str> {
        self.0.selection_text.as_deref().filter(|t| !t.is_empty())
    }
    /// Why the selection could not be copied (it is over the copy limit), if it could not.
    pub fn selection_error(&self) -> Option<&str> {
        self.0.selection_error.as_deref()
    }
    /// The last selection gesture this frame has applied; see [`crate::Host::selection`].
    pub fn selection_revision(&self) -> u64 {
        self.0.selection_revision
    }
    pub fn mouse_tracking(&self) -> bool {
        self.0.mouse_tracking
    }
    /// The first row to draw when fewer than all rows fit.
    pub fn first_row(&self, visible: usize) -> usize {
        self.0.display_row_start(visible)
    }
}
