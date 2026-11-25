//! Local echo: show typed characters before the server does.
//!
//! What was typed and not yet echoed is kept as plain text with no positions; it is drawn from
//! the server's cursor on. A guess is right when the cursor moves over exactly those characters.
//!
//! Guesses are hidden until one has come true since the last thing that could change who is
//! reading: Enter, any control or navigation key, or a cursor move typing doesn't explain. A
//! password prompt never echoes, so nothing typed at one is ever drawn.
use crate::Frame;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

static ENABLED: AtomicBool = AtomicBool::new(true);

/// On unless turned off.
pub fn set_local_echo(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Below this the real echo is as good as immediate.
const WORTH_IT: Duration = Duration::from_millis(25);
const MOST: usize = 64;

#[derive(Default)]
pub(crate) struct Predictor {
    pending: VecDeque<(char, Instant)>,
    /// A guess has come true since the last reset.
    sure: bool,
    cursor: Option<(u16, u16)>,
}

impl Predictor {
    pub fn typed(&mut self, c: char, now: Instant) {
        if self.pending.len() == MOST {
            self.reset();
        }
        self.pending.push_back((c, now));
    }

    /// Something other than plain typing was sent.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.sure = false;
    }

    /// Compare a new screen with what was expected.
    pub fn observe(&mut self, frame: &Frame, now: Instant, round_trip: Duration) {
        let late = (round_trip * 4).max(Duration::from_millis(800));
        // Only what is overdue: the rest is still on its way and will be echoed in order.
        while self.pending.front().is_some_and(|(_, at)| now.duration_since(*at) > late) {
            self.pending.pop_front();
            self.sure = false;
        }
        let old = std::mem::replace(&mut self.cursor, frame.cursor());
        let (Some(old), Some(new)) = (old, self.cursor) else {
            self.sure = false;
            return;
        };
        if old == new {
            return;
        }
        let advance = usize::from(new.0.saturating_sub(old.0));
        if old.1 == new.1 && advance > 0 && advance <= self.pending.len() {
            let mut row = frame.row(usize::from(new.1)).skip(usize::from(old.0));
            let echoed = self
                .pending
                .iter()
                .take(advance)
                .all(|(c, _)| row.next().is_some_and(|cell| is(cell.text(), *c)));
            if echoed {
                self.pending.drain(..advance);
                self.sure = true;
            } else {
                // Not what was next: the echo of something sent before a reset, or a program
                // that shows something else. What is pending stays, unseen, and must prove
                // itself; dropping it would put every later guess one character out of step.
                self.sure = false;
            }
        } else {
            // A prompt, another program, a redrawn or wrapped line.
            self.sure = false;
            self.resync(frame, new);
        }
    }

    /// The server can redraw a line and echo typing in one step. Forget pending characters that
    /// already stand just left of the cursor. This proves nothing, so shows nothing.
    fn resync(&mut self, frame: &Frame, (x, y): (u16, u16)) {
        let x = usize::from(x);
        let row: Vec<_> = frame.row(usize::from(y)).take(x).collect();
        let here = (1..=self.pending.len().min(row.len())).rev().find(|n| {
            row[row.len() - n..]
                .iter()
                .zip(&self.pending)
                .all(|(cell, (c, _))| is(cell.text(), *c))
        });
        if let Some(n) = here {
            self.pending.drain(..n);
        }
    }

    /// Characters to draw from the cursor on.
    pub fn show(&self, frame: &Frame, round_trip: Duration) -> String {
        let Some((x, y)) = frame.cursor() else { return String::new() };
        let room = usize::from(frame.cols().saturating_sub(x + 1));
        if !ENABLED.load(Ordering::Relaxed)
            || !self.sure
            || round_trip < WORTH_IT
            || self.pending.len() > room
        {
            return String::new();
        }
        // Over other text, only where it already says the same, as an autosuggestion does:
        // typing inside a line inserts, and guessing at that would draw nonsense.
        let mut row = frame.row(usize::from(y)).skip(usize::from(x));
        let clear = self.pending.iter().all(|(c, _)| {
            row.next().is_some_and(|cell| matches!(cell.text(), "" | " ") || is(cell.text(), *c))
        });
        if clear { self.pending.iter().map(|(c, _)| c).collect() } else { String::new() }
    }
}

fn is(text: &str, c: char) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first == c && chars.next().is_none(),
        None => c == ' ',
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghostty_vt::{Terminal, terminal::Options};
    use std::sync::Arc;

    const RTT: Duration = Duration::from_millis(110);

    struct Rig {
        terminal: Terminal,
        renderer: rex_rune::terminal::Renderer,
        p: Predictor,
        now: Instant,
    }
    impl Rig {
        fn new() -> Self {
            let mut rig = Self {
                terminal: Terminal::new(Options { cols: 40, rows: 5, max_scrollback: 0 }).unwrap(),
                renderer: rex_rune::terminal::Renderer::new().unwrap(),
                p: Predictor::default(),
                now: Instant::now(),
            };
            rig.server("$ ");
            rig
        }
        fn frame(&mut self) -> Frame {
            Frame(Arc::new(self.renderer.frame(&self.terminal).unwrap()))
        }
        fn server(&mut self, bytes: &str) {
            self.terminal.vt_write(bytes.as_bytes());
            let frame = self.frame();
            self.p.observe(&frame, self.now, RTT);
        }
        fn typed(&mut self, text: &str) {
            text.chars().for_each(|c| self.p.typed(c, self.now));
        }
        fn shown(&mut self) -> String {
            let frame = self.frame();
            self.p.show(&frame, RTT)
        }
    }

    #[test]
    fn shows_only_after_an_echo_and_shrinks_as_echoes_arrive() {
        let mut r = Rig::new();
        r.typed("l");
        assert_eq!(r.shown(), "", "unproven");
        r.server("l");
        r.typed("s -");
        assert_eq!(r.shown(), "s -");
        r.server("s");
        assert_eq!(r.shown(), " -");
        r.server(" -");
        assert_eq!(r.shown(), "");
    }

    #[test]
    fn a_password_is_never_shown() {
        let mut r = Rig::new();
        r.typed("sudo x");
        r.server("sudo x");
        r.p.reset(); // Enter
        r.typed("hun"); // typed ahead of the prompt
        assert_eq!(r.shown(), "");
        r.server("\r\nPassword: ");
        assert_eq!(r.shown(), "");
        r.typed("ter2");
        for _ in 0..3 {
            r.server("");
            assert_eq!(r.shown(), "");
        }
        // A prompt that appears on the same line, without Enter.
        let mut r = Rig::new();
        r.typed("a");
        r.server("a");
        r.server(" Password: ");
        r.typed("hunter2");
        assert_eq!(r.shown(), "");
        // Stars instead of letters.
        r.server("*");
        r.typed("x");
        assert_eq!(r.shown(), "");
    }

    #[test]
    fn typing_through_enter_recovers_without_a_pause() {
        let mut r = Rig::new();
        r.typed("ls");
        r.server("ls");
        r.p.reset(); // Enter
        r.typed("pwd"); // before the new prompt is here
        r.server("\r\nfile\r\n$ ");
        assert_eq!(r.shown(), "");
        r.server("p");
        assert_eq!(r.shown(), "wd");
    }

    #[test]
    fn a_redraw_and_an_echo_in_one_step_do_not_put_it_out_of_step() {
        // Seen live: Ctrl+U and the next letter's echo arrive as one screen update.
        let mut r = Rig::new();
        r.typed("abc");
        r.server("abc");
        r.p.reset(); // Ctrl+U
        r.typed("ec");
        r.server("\r\x1b[K$ e");
        assert_eq!(r.shown(), "", "not proven yet");
        r.typed("h");
        r.server("c");
        assert_eq!(r.shown(), "h");

        // The echo of something typed before a reset is not the echo of what came after.
        let mut r = Rig::new();
        r.p.reset();
        r.typed("o");
        r.server("h");
        assert_eq!(r.shown(), "");
        r.typed("k");
        r.server("o");
        assert_eq!(r.shown(), "k");
    }

    #[test]
    fn wrong_guesses_late_echoes_and_hidden_cursors_show_nothing() {
        let mut r = Rig::new();
        r.typed("a");
        r.server("a");
        r.typed("bc");
        r.server("X");
        assert_eq!(r.shown(), "", "a program that shows something else");

        let mut r = Rig::new();
        r.typed("a");
        r.server("a");
        r.typed("b");
        assert_eq!(r.shown(), "b");
        r.now += Duration::from_secs(2);
        r.server("");
        assert_eq!(r.shown(), "", "never echoed");

        let mut r = Rig::new();
        r.typed("a");
        r.server("a");
        r.typed("b");
        r.server("\x1b[?25l");
        assert_eq!(r.shown(), "", "full-screen programs that draw their own cursor");
    }

    #[test]
    fn inside_a_line_it_stays_out_of_the_way_but_follows_a_suggestion() {
        let mut r = Rig::new();
        r.typed("l");
        r.server("l\x1b[2ms -la\x1b[0m\x1b[5D");
        r.typed("s");
        assert_eq!(r.shown(), "s", "what an autosuggestion already says");
        let mut r = Rig::new();
        r.typed("a");
        r.server("a");
        r.server("\x1b[sbc\x1b[u"); // text to the right, cursor back
        r.typed("b");
        assert_eq!(r.shown(), "b");
        r.typed("z");
        assert_eq!(r.shown(), "", "would overwrite other text");
    }

    #[test]
    fn nothing_on_a_fast_link_or_past_the_edge() {
        let mut r = Rig::new();
        r.typed("a");
        r.server("a");
        r.typed("b");
        let frame = r.frame();
        assert_eq!(r.p.show(&frame, Duration::from_millis(3)), "");
        r.typed(&"x".repeat(40));
        assert_eq!(r.shown(), "");
    }
}
