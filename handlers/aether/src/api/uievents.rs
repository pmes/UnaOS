//! UI Events (W3C UI Events, KeyboardEvent key/code values): the shell's key strings mapped to the
//! members of a trusted `KeyboardEvent` (AETHEREVENTS, SR80).
//!
//! CHARTER: Aether owns the browser (docs/CODEX.md §2); this is inside the live handler. The JS-side
//! construction is `crate::js::events::user_key`.
//!
//! The shell sends GDK keysym names (`Left`, `BackSpace`, `Page_Down`, `space`, `Shift_L`) or DOM key
//! names (`ArrowLeft`, `Backspace`, `PageDown`, `" "`), prefixed by `Ctrl+`, `Alt+`, `Shift+`, `Meta+`
//! in any order — the grammar of `forms::edit::parse_key`. A printable key is its character.

/// The members of one trusted KeyboardEvent.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyInit {
    /// `key`: the key value (UI Events KeyboardEvent key Values).
    pub key: String,
    /// `code`: the physical key (UI Events KeyboardEvent code Values, US layout).
    pub code: String,
    /// `location`: 0 standard, 1 left, 2 right, 3 numpad.
    pub location: u32,
    /// The legacy `keyCode` (and `which`).
    pub key_code: u32,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

/// The `getModifierState` key names and the EventModifierInit member each reads.
pub const MODIFIERS: &[(&str, &str)] = &[
    ("Alt", "altKey"),
    ("AltGraph", "modifierAltGraph"),
    ("CapsLock", "modifierCapsLock"),
    ("Control", "ctrlKey"),
    ("Fn", "modifierFn"),
    ("FnLock", "modifierFnLock"),
    ("Hyper", "modifierHyper"),
    ("Meta", "metaKey"),
    ("NumLock", "modifierNumLock"),
    ("ScrollLock", "modifierScrollLock"),
    ("Shift", "shiftKey"),
    ("Super", "modifierSuper"),
    ("Symbol", "modifierSymbol"),
    ("SymbolLock", "modifierSymbolLock"),
];

/// The dictionary-only EventModifierInit members (no IDL attribute; `getModifierState` reads them).
pub const MODIFIER_MEMBERS: &[&str] = &[
    "modifierAltGraph",
    "modifierCapsLock",
    "modifierFn",
    "modifierFnLock",
    "modifierHyper",
    "modifierNumLock",
    "modifierScrollLock",
    "modifierSuper",
    "modifierSymbol",
    "modifierSymbolLock",
];

/// Named (non-printable) keys: shell names → (key, code, location, keyCode).
fn named(name: &str) -> Option<(&'static str, &'static str, u32, u32)> {
    Some(match name {
        "Left" | "ArrowLeft" => ("ArrowLeft", "ArrowLeft", 0, 37),
        "Right" | "ArrowRight" => ("ArrowRight", "ArrowRight", 0, 39),
        "Up" | "ArrowUp" => ("ArrowUp", "ArrowUp", 0, 38),
        "Down" | "ArrowDown" => ("ArrowDown", "ArrowDown", 0, 40),
        "KP_Left" => ("ArrowLeft", "Numpad4", 3, 37),
        "KP_Right" => ("ArrowRight", "Numpad6", 3, 39),
        "KP_Up" => ("ArrowUp", "Numpad8", 3, 38),
        "KP_Down" => ("ArrowDown", "Numpad2", 3, 40),
        "Home" => ("Home", "Home", 0, 36),
        "End" => ("End", "End", 0, 35),
        "KP_Home" => ("Home", "Numpad7", 3, 36),
        "KP_End" => ("End", "Numpad1", 3, 35),
        "Page_Up" | "PageUp" | "Prior" => ("PageUp", "PageUp", 0, 33),
        "Page_Down" | "PageDown" | "Next" => ("PageDown", "PageDown", 0, 34),
        "KP_Page_Up" | "KP_Prior" => ("PageUp", "Numpad9", 3, 33),
        "KP_Page_Down" | "KP_Next" => ("PageDown", "Numpad3", 3, 34),
        "BackSpace" | "Backspace" => ("Backspace", "Backspace", 0, 8),
        "Delete" | "Del" => ("Delete", "Delete", 0, 46),
        "KP_Delete" => ("Delete", "NumpadDecimal", 3, 46),
        "Insert" => ("Insert", "Insert", 0, 45),
        "KP_Insert" => ("Insert", "Numpad0", 3, 45),
        "Return" | "Enter" | "ISO_Enter" => ("Enter", "Enter", 0, 13),
        "KP_Enter" => ("Enter", "NumpadEnter", 3, 13),
        "Tab" | "ISO_Left_Tab" => ("Tab", "Tab", 0, 9),
        "KP_Tab" => ("Tab", "Tab", 3, 9),
        "Escape" | "Esc" => ("Escape", "Escape", 0, 27),
        "space" | "Space" | " " => (" ", "Space", 0, 32),
        "KP_Space" => (" ", "Space", 3, 32),
        "Shift_L" => ("Shift", "ShiftLeft", 1, 16),
        "Shift_R" => ("Shift", "ShiftRight", 2, 16),
        "Shift" => ("Shift", "ShiftLeft", 1, 16),
        "Control_L" => ("Control", "ControlLeft", 1, 17),
        "Control_R" => ("Control", "ControlRight", 2, 17),
        "Control" => ("Control", "ControlLeft", 1, 17),
        "Alt_L" => ("Alt", "AltLeft", 1, 18),
        "Alt_R" => ("Alt", "AltRight", 2, 18),
        "Alt" => ("Alt", "AltLeft", 1, 18),
        "ISO_Level3_Shift" | "AltGraph" => ("AltGraph", "AltRight", 2, 225),
        "Meta_L" | "Super_L" => ("Meta", "MetaLeft", 1, 91),
        "Meta_R" | "Super_R" => ("Meta", "MetaRight", 2, 92),
        "Meta" => ("Meta", "MetaLeft", 1, 91),
        "Caps_Lock" | "CapsLock" => ("CapsLock", "CapsLock", 0, 20),
        "Num_Lock" | "NumLock" => ("NumLock", "NumLock", 0, 144),
        "Scroll_Lock" | "ScrollLock" => ("ScrollLock", "ScrollLock", 0, 145),
        "Pause" => ("Pause", "Pause", 0, 19),
        "Print" | "PrintScreen" => ("PrintScreen", "PrintScreen", 0, 44),
        "Menu" | "ContextMenu" => ("ContextMenu", "ContextMenu", 0, 93),
        "F1" => ("F1", "F1", 0, 112),
        "F2" => ("F2", "F2", 0, 113),
        "F3" => ("F3", "F3", 0, 114),
        "F4" => ("F4", "F4", 0, 115),
        "F5" => ("F5", "F5", 0, 116),
        "F6" => ("F6", "F6", 0, 117),
        "F7" => ("F7", "F7", 0, 118),
        "F8" => ("F8", "F8", 0, 119),
        "F9" => ("F9", "F9", 0, 120),
        "F10" => ("F10", "F10", 0, 121),
        "F11" => ("F11", "F11", 0, 122),
        "F12" => ("F12", "F12", 0, 123),
        _ => return None,
    })
}

/// A printable character's physical key on a US layout: (code, keyCode).
fn printable(c: char) -> (String, u32) {
    if c.is_ascii_alphabetic() {
        let u = c.to_ascii_uppercase();
        return (format!("Key{u}"), u as u32);
    }
    if c.is_ascii_digit() {
        return (format!("Digit{c}"), c as u32);
    }
    let (code, kc) = match c {
        '!' => ("Digit1", 49),
        '@' => ("Digit2", 50),
        '#' => ("Digit3", 51),
        '$' => ("Digit4", 52),
        '%' => ("Digit5", 53),
        '^' => ("Digit6", 54),
        '&' => ("Digit7", 55),
        '*' => ("Digit8", 56),
        '(' => ("Digit9", 57),
        ')' => ("Digit0", 48),
        ';' | ':' => ("Semicolon", 186),
        '=' | '+' => ("Equal", 187),
        ',' | '<' => ("Comma", 188),
        '-' | '_' => ("Minus", 189),
        '.' | '>' => ("Period", 190),
        '/' | '?' => ("Slash", 191),
        '`' | '~' => ("Backquote", 192),
        '[' | '{' => ("BracketLeft", 219),
        '\\' | '|' => ("Backslash", 220),
        ']' | '}' => ("BracketRight", 221),
        '\'' | '"' => ("Quote", 222),
        ' ' => ("Space", 32),
        _ => ("", 0),
    };
    (code.to_string(), kc)
}

/// The keypad's printable keys (`KP_0`…`KP_9`, `KP_Add`, …): (key, code, keyCode).
fn keypad(name: &str) -> Option<(String, &'static str, u32)> {
    let d = name.strip_prefix("KP_")?;
    if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() {
        let n = (d.as_bytes()[0] - b'0') as u32;
        let code = ["Numpad0", "Numpad1", "Numpad2", "Numpad3", "Numpad4", "Numpad5", "Numpad6", "Numpad7", "Numpad8", "Numpad9"][n as usize];
        return Some((d.to_string(), code, 96 + n));
    }
    Some(match d {
        "Add" => ("+".into(), "NumpadAdd", 107),
        "Subtract" => ("-".into(), "NumpadSubtract", 109),
        "Multiply" => ("*".into(), "NumpadMultiply", 106),
        "Divide" => ("/".into(), "NumpadDivide", 111),
        "Decimal" => (".".into(), "NumpadDecimal", 110),
        "Equal" => ("=".into(), "NumpadEqual", 187),
        _ => return None,
    })
}

/// Maps one shell key string to KeyboardEvent members; `None` for a string that names no key.
pub fn key_init(shell: &str) -> Option<KeyInit> {
    let (mut ctrl, mut alt, mut shift, mut meta) = (false, false, false, false);
    let mut rest = shell;
    loop {
        let Some(i) = rest.find('+') else { break };
        if i == 0 || i + 1 >= rest.len() {
            break; // "+" itself, or a trailing '+': the key is the plus sign
        }
        match rest[..i].to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "primary" => ctrl = true,
            "alt" | "option" | "mod1" => alt = true,
            "shift" => shift = true,
            "meta" | "super" | "cmd" | "command" => meta = true,
            _ => break,
        }
        rest = &rest[i + 1..];
    }
    let (key, code, location, key_code) = if let Some((k, c, l, kc)) = named(rest) {
        // A modifier key reports its own modifier as held (UI Events §5.6.4: the keydown of Shift
        // has shiftKey true).
        match k {
            "Shift" => shift = true,
            "Control" => ctrl = true,
            "Alt" => alt = true,
            "Meta" => meta = true,
            _ => {}
        }
        (k.to_string(), c.to_string(), l, kc)
    } else if let Some((k, c, kc)) = keypad(rest) {
        (k, c.to_string(), 3, kc)
    } else {
        let mut it = rest.chars();
        let (Some(ch), None) = (it.next(), it.next()) else { return None };
        if ch.is_control() {
            return None;
        }
        let (code, kc) = printable(ch);
        (ch.to_string(), code, 0, kc)
    };
    Some(KeyInit { key, code, location, key_code, ctrl, shift, alt, meta })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        let k = key_init("a").unwrap();
        assert_eq!((k.key.as_str(), k.code.as_str(), k.key_code, k.location), ("a", "KeyA", 65, 0));
        let k = key_init("Ctrl+Shift+Left").unwrap();
        assert_eq!((k.key.as_str(), k.code.as_str(), k.key_code, k.ctrl, k.shift, k.alt), ("ArrowLeft", "ArrowLeft", 37, true, true, false));
        let k = key_init("BackSpace").unwrap();
        assert_eq!((k.key.as_str(), k.key_code), ("Backspace", 8));
        let k = key_init("space").unwrap();
        assert_eq!((k.key.as_str(), k.code.as_str()), (" ", "Space"));
        let k = key_init("Shift_R").unwrap();
        assert_eq!((k.key.as_str(), k.code.as_str(), k.location, k.shift), ("Shift", "ShiftRight", 2, true));
        let k = key_init("KP_5").unwrap();
        assert_eq!((k.key.as_str(), k.code.as_str(), k.location, k.key_code), ("5", "Numpad5", 3, 101));
        let k = key_init("+").unwrap();
        assert_eq!((k.key.as_str(), k.code.as_str()), ("+", "Equal"));
        assert!(key_init("NoSuchKey").is_none());
    }
}
