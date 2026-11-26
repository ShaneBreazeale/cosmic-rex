//! Minimum contrast, as Ghostty's `minimum-contrast` setting: text whose colour is too close to
//! its background is moved toward black or white until it reaches the ratio. It moves only as
//! far as it must, so it keeps its hue: grey stays grey, red stays red.
//!
//! Ratios are WCAG's, from 1 (the same colour) to 21 (black on white). 4.5 is the usual
//! minimum for body text, 7 the stricter one.

/// The View menu's choices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Contrast {
    /// Colours as the program sent them.
    #[default]
    Off,
    /// 3:1, the minimum for large text.
    Low,
    /// 4.5:1, the usual minimum for body text.
    Medium,
    /// 7:1, the stricter minimum.
    High,
}

impl Contrast {
    pub const ALL: [Contrast; 4] = [Contrast::Off, Contrast::Low, Contrast::Medium, Contrast::High];

    pub fn ratio(self) -> f32 {
        match self {
            Contrast::Off => 1.0,
            Contrast::Low => 3.0,
            Contrast::Medium => 4.5,
            Contrast::High => 7.0,
        }
    }

    /// The choice saved as `ratio`; anything else is Off.
    pub fn from_ratio(ratio: f32) -> Self {
        Self::ALL.into_iter().find(|c| c.ratio() == ratio).unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            Contrast::Off => "Off",
            Contrast::Low => "Low (3:1)",
            Contrast::Medium => "Medium (4.5:1)",
            Contrast::High => "High (7:1)",
        }
    }
}

const BLACK: u32 = 0x000000;
const WHITE: u32 = 0xffffff;

/// Relative luminance of 0xRRGGBB, from 0 (black) to 1 (white).
fn luminance(rgb: u32) -> f32 {
    let channel = |shift: u32| {
        let c = ((rgb >> shift) & 0xff) as f32 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

pub fn ratio(a: u32, b: u32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `a` moved a fraction `t` of the way to `b`.
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let channel = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xff) as f32, ((b >> shift) & 0xff) as f32);
        ((x + (y - x) * t).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// `fg`, or the smallest move of it toward black or white that gives at least `min` against
/// `bg`. Text darker than its background gets darker, lighter text lighter. Where that side
/// can't reach `min`, the text takes whichever of black and white stands out more.
pub fn adjust(fg: u32, bg: u32, min: f32) -> u32 {
    if min <= 1.0 || ratio(fg, bg) >= min {
        return fg;
    }
    let lighter = luminance(fg) >= luminance(bg);
    let (toward, other) = if lighter { (WHITE, BLACK) } else { (BLACK, WHITE) };
    if ratio(toward, bg) < min {
        return if ratio(other, bg) > ratio(toward, bg) { other } else { toward };
    }
    // On this side the ratio only grows on the way to `toward`, so halving finds the least move.
    let (mut short, mut enough) = (0.0f32, 1.0f32);
    for _ in 0..12 {
        let t = (short + enough) / 2.0;
        if ratio(mix(fg, toward, t), bg) >= min {
            enough = t;
        } else {
            short = t;
        }
    }
    mix(fg, toward, enough)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_match_wcag() {
        assert!((ratio(BLACK, WHITE) - 21.0).abs() < 0.01);
        assert!((ratio(0x666666, WHITE) - 5.74).abs() < 0.01);
        assert!((ratio(0x777777, 0x777777) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn off_or_enough_leaves_text_alone() {
        assert_eq!(adjust(0x666666, WHITE, Contrast::Off.ratio()), 0x666666);
        assert_eq!(adjust(0x666666, WHITE, 4.5), 0x666666, "5.7:1 already passes 4.5");
        assert_eq!(adjust(0x1f2328, WHITE, 7.0), 0x1f2328);
    }

    #[test]
    fn grey_on_white_darkens_just_enough_and_stays_grey() {
        let out = adjust(0x666666, WHITE, 7.0);
        let (r, g, b) = (out >> 16, (out >> 8) & 0xff, out & 0xff);
        assert!(r == g && g == b, "still grey: {out:06x}");
        assert!(ratio(out, WHITE) >= 7.0, "{out:06x}");
        assert!(ratio(out, WHITE) < 7.2, "no darker than it must be: {out:06x}");
    }

    #[test]
    fn light_text_on_dark_lightens_and_keeps_its_hue() {
        let (fg, bg) = (0x803030, 0x202020);
        let out = adjust(fg, bg, 4.5);
        assert!(ratio(out, bg) >= 4.5, "{out:06x}");
        let (r, g, b) = (out >> 16, (out >> 8) & 0xff, out & 0xff);
        assert!(r > g && g == b, "still red: {out:06x}");
    }

    #[test]
    fn every_choice_is_reached_on_any_ground() {
        for bg in [WHITE, BLACK, 0x777777, 0x001528, 0xfdf6e3, 0x1d2a2b] {
            for fg in [WHITE, BLACK, 0x777777, 0x7f7f7f, 0x5ec4b6, 0xff0000, 0x0000ff] {
                for c in Contrast::ALL {
                    let out = adjust(fg, bg, c.ratio());
                    let best = ratio(BLACK, bg).max(ratio(WHITE, bg));
                    assert!(
                        ratio(out, bg) >= c.ratio().min(best) - 1e-3,
                        "{fg:06x} on {bg:06x} at {c:?}: {out:06x}"
                    );
                }
            }
        }
    }
}
