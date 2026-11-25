//! iced key events -> backend keys. The server encodes; we only describe the key.
use cosmic::iced::keyboard::{
    Key, Modifiers,
    key::{Code, Physical},
};
use rex_backend::KeyAction as Action;

/// Ghostty's name for a W3C code: `KeyA` -> `key_a`, `Digit1` -> `digit_1`, `F12` -> `f12`.
pub fn code_name(code: &Code) -> String {
    let debug = format!("{code:?}");
    if let Some(n) = debug.strip_prefix('F')
        && !n.is_empty()
        && n.bytes().all(|b| b.is_ascii_digit())
    {
        return format!("f{n}");
    }
    let mut out = String::with_capacity(debug.len() + 4);
    let mut previous: Option<char> = None;
    for ch in debug.chars() {
        if previous.is_some_and(|p| {
            (ch.is_ascii_uppercase() && (p.is_ascii_lowercase() || p.is_ascii_digit()))
                || (ch.is_ascii_digit() && p.is_ascii_alphabetic())
        }) {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
        previous = Some(ch);
    }
    out
}

/// `shift+ctrl+alt+super`, in the order the server documents.
pub fn mods(m: Modifiers) -> String {
    [
        (m.shift(), "shift"),
        (m.control(), "ctrl"),
        (m.alt(), "alt"),
        (m.logo(), "super"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect::<Vec<_>>()
    .join("+")
}

/// `None` for keys Rex has no name for.
pub fn describe(
    key: &Key,
    physical: &Physical,
    m: Modifiers,
    text: Option<&str>,
    action: Action,
) -> Option<rex_backend::Key> {
    let Physical::Code(code) = physical else {
        return None;
    };
    let base = match key {
        Key::Character(c) => c.chars().next().filter(|_| c.chars().count() == 1),
        _ => None,
    };
    // No text for releases or command chords; control characters are the server's job, except
    // Enter and Tab, which it expects as \n and \t.
    let text = match (action, code) {
        (Action::Release, _) => "",
        _ if m.control() || m.logo() => "",
        (_, Code::Enter | Code::NumpadEnter) if !m.alt() => "\n",
        (_, Code::Tab) if !m.alt() => "\t",
        _ => text
            .filter(|t| !t.chars().any(char::is_control))
            .unwrap_or(""),
    };
    let consumed = m.shift()
        && !text.is_empty()
        && base.is_some_and(|b| {
            b.is_lowercase() || b.is_uppercase() || text.chars().ne(std::iter::once(b))
        });
    Some(rex_backend::Key {
        name: code_name(code),
        mods: mods(m),
        consumed_mods: if consumed { "shift" } else { "" },
        text: text.to_owned(),
        unshifted: base.map_or(0, |c| c.to_lowercase().next().unwrap_or(c) as u32),
        action,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::iced::keyboard::key::Named;

    fn press(key: Key, code: Code, m: Modifiers, text: Option<&str>) -> rex_backend::Key {
        describe(&key, &Physical::Code(code), m, text, Action::Press).unwrap()
    }

    #[test]
    fn names_follow_ghostty() {
        for (code, name) in [
            (Code::KeyA, "key_a"),
            (Code::Digit1, "digit_1"),
            (Code::ArrowUp, "arrow_up"),
            (Code::Numpad0, "numpad_0"),
            (Code::F12, "f12"),
            (Code::BracketLeft, "bracket_left"),
            (Code::PageUp, "page_up"),
        ] {
            assert_eq!(code_name(&code), name);
        }
    }

    #[test]
    fn shifted_letter_consumes_shift() {
        let v = press(Key::Character("a".into()), Code::KeyA, Modifiers::SHIFT, Some("A"));
        assert_eq!(v.text, "A");
        assert_eq!(v.consumed_mods, "shift");
        assert_eq!(v.unshifted, 97);
    }

    #[test]
    fn control_chords_and_releases_carry_no_text() {
        let v = press(Key::Character("c".into()), Code::KeyC, Modifiers::CTRL, Some("\u{3}"));
        assert_eq!((v.text.as_str(), v.mods.as_str()), ("", "ctrl"));
        let r = describe(
            &Key::Character("a".into()),
            &Physical::Code(Code::KeyA),
            Modifiers::empty(),
            Some("a"),
            Action::Release,
        )
        .unwrap();
        assert_eq!(r.text, "");
    }

    #[test]
    fn enter_and_escape() {
        let v = press(Key::Named(Named::Enter), Code::Enter, Modifiers::empty(), Some("\r"));
        assert_eq!(v.text, "\n");
        let v = press(Key::Named(Named::Escape), Code::Escape, Modifiers::empty(), Some("\u{1b}"));
        assert_eq!((v.name.as_str(), v.text.as_str()), ("escape", ""));
    }
}
