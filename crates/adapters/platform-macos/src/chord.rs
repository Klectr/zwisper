//! Shortcut-chord parsing. Pure logic, compiled and tested on every platform.
//!
//! Accepts the user-facing forms stored in settings ("Alt+Space",
//! "Option+Shift+M", "⌘⇧K", "cmd + f5") and normalises them to the token
//! syntax `global_hotkey::hotkey::HotKey::from_str` understands
//! ("alt+shift+KeyM").

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// ⌘ on macOS.
    pub meta: bool,
    /// Canonical key token, e.g. "Space", "KeyM", "Digit1", "F5".
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChordError {
    Empty,
    NoKey(String),
    TwoKeys(String),
    UnknownKey(String),
}

impl fmt::Display for ChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChordError::Empty => write!(f, "empty shortcut"),
            ChordError::NoKey(s) => write!(f, "shortcut '{s}' has modifiers but no key"),
            ChordError::TwoKeys(s) => write!(f, "shortcut '{s}' has more than one non-modifier key"),
            ChordError::UnknownKey(k) => write!(f, "unknown key '{k}'"),
        }
    }
}

impl std::error::Error for ChordError {}

enum Token {
    Ctrl,
    Alt,
    Shift,
    Meta,
    Key(String),
}

fn modifier(tok: &str) -> Option<Token> {
    Some(match tok.to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "⌃" | "^" => Token::Ctrl,
        "alt" | "option" | "opt" | "⌥" => Token::Alt,
        "shift" | "⇧" => Token::Shift,
        "cmd" | "command" | "super" | "meta" | "win" | "⌘" | "cmdorctrl" | "commandorcontrol" => Token::Meta,
        _ => return None,
    })
}

/// Canonical key token for global-hotkey, or `None` if unknown.
fn key_token(raw: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let mut chars = raw.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphabetic() {
            return Some(format!("Key{}", c.to_ascii_uppercase()));
        }
        if c.is_ascii_digit() {
            return Some(format!("Digit{c}"));
        }
        let named = match c {
            '`' => "Backquote",
            '\\' => "Backslash",
            '[' => "BracketLeft",
            ']' => "BracketRight",
            ',' => "Comma",
            '=' => "Equal",
            '-' => "Minus",
            '.' => "Period",
            '\'' => "Quote",
            ';' => "Semicolon",
            '/' => "Slash",
            '␣' => "Space",
            '⎋' => "Escape",
            '↩' | '⏎' => "Enter",
            '⇥' => "Tab",
            '⌫' => "Backspace",
            '⌦' => "Delete",
            '↑' => "ArrowUp",
            '↓' => "ArrowDown",
            '←' => "ArrowLeft",
            '→' => "ArrowRight",
            _ => return None,
        };
        return Some(named.to_string());
    }
    if let Some(rest) = lower.strip_prefix("key") {
        if rest.len() == 1 && rest.as_bytes()[0].is_ascii_alphabetic() {
            return Some(format!("Key{}", rest.to_ascii_uppercase()));
        }
    }
    if let Some(rest) = lower.strip_prefix("digit") {
        if rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit() {
            return Some(format!("Digit{rest}"));
        }
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        if (1..=24).contains(&n) {
            return Some(format!("F{n}"));
        }
    }
    let named = match lower.as_str() {
        "space" | "spacebar" => "Space",
        "tab" => "Tab",
        "enter" | "return" => "Enter",
        "esc" | "escape" => "Escape",
        "backspace" => "Backspace",
        "delete" | "del" | "forwarddelete" => "Delete",
        "up" | "arrowup" => "ArrowUp",
        "down" | "arrowdown" => "ArrowDown",
        "left" | "arrowleft" => "ArrowLeft",
        "right" | "arrowright" => "ArrowRight",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "backquote" | "grave" => "Backquote",
        "backslash" => "Backslash",
        "bracketleft" => "BracketLeft",
        "bracketright" => "BracketRight",
        "comma" => "Comma",
        "equal" => "Equal",
        "minus" => "Minus",
        "period" => "Period",
        "quote" => "Quote",
        "semicolon" => "Semicolon",
        "slash" => "Slash",
        _ => return None,
    };
    Some(named.to_string())
}

/// Split "⌘⇧K" style strings (no separators) into tokens.
fn split_tokens(s: &str) -> Vec<String> {
    if s.contains('+') && s.trim() != "+" {
        return s.split('+').map(|t| t.trim().to_string()).collect();
    }
    // Symbol form: peel leading modifier glyphs, rest is the key.
    let mut out = Vec::new();
    let mut rest = s.trim();
    while let Some(c) = rest.chars().next() {
        if matches!(c, '⌃' | '⌥' | '⇧' | '⌘') {
            out.push(c.to_string());
            rest = rest[c.len_utf8()..].trim_start();
        } else {
            break;
        }
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

pub fn parse_chord(s: &str) -> Result<Chord, ChordError> {
    if s.trim().is_empty() {
        return Err(ChordError::Empty);
    }
    let mut c = Chord { ctrl: false, alt: false, shift: false, meta: false, key: String::new() };
    let mut key: Option<String> = None;
    for tok in split_tokens(s) {
        if tok.is_empty() {
            return Err(ChordError::NoKey(s.to_string()));
        }
        match modifier(&tok).unwrap_or(Token::Key(tok.clone())) {
            Token::Ctrl => c.ctrl = true,
            Token::Alt => c.alt = true,
            Token::Shift => c.shift = true,
            Token::Meta => c.meta = true,
            Token::Key(k) => {
                if key.is_some() {
                    return Err(ChordError::TwoKeys(s.to_string()));
                }
                key = Some(key_token(&k).ok_or(ChordError::UnknownKey(k))?);
            }
        }
    }
    c.key = key.ok_or_else(|| ChordError::NoKey(s.to_string()))?;
    Ok(c)
}

impl Chord {
    /// String accepted by `global_hotkey::hotkey::HotKey::from_str`.
    pub fn to_hotkey_string(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.ctrl {
            parts.push("ctrl");
        }
        if self.alt {
            parts.push("alt");
        }
        if self.shift {
            parts.push("shift");
        }
        if self.meta {
            parts.push("super");
        }
        parts.push(&self.key);
        parts.join("+")
    }
}

/// Suppresses key auto-repeat: only the first Pressed after a Released passes.
#[derive(Debug, Default)]
pub struct RepeatFilter {
    down: std::collections::HashSet<u32>,
}

impl RepeatFilter {
    /// Returns true if the event should be delivered.
    pub fn accept(&mut self, id: u32, pressed: bool) -> bool {
        if pressed {
            self.down.insert(id)
        } else {
            self.down.remove(&id)
        }
    }

    pub fn clear(&mut self) {
        self.down.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> String {
        parse_chord(x).unwrap().to_hotkey_string()
    }

    #[test]
    fn common_chords() {
        assert_eq!(s("Alt+Space"), "alt+Space");
        assert_eq!(s("Alt+Shift+M"), "alt+shift+KeyM");
        assert_eq!(s("option + shift + m"), "alt+shift+KeyM");
        assert_eq!(s("Shift+Alt+M"), "alt+shift+KeyM"); // order-insensitive
        assert_eq!(s("Cmd+Ctrl+1"), "ctrl+super+Digit1");
        assert_eq!(s("F13"), "F13");
        assert_eq!(s("ctrl+f5"), "ctrl+F5");
        assert_eq!(s("Ctrl+KeyD"), "ctrl+KeyD");
        assert_eq!(s("Ctrl+Escape"), "ctrl+Escape");
        assert_eq!(s("Alt+/"), "alt+Slash");
    }

    #[test]
    fn symbol_chords() {
        assert_eq!(s("⌥Space"), "alt+Space");
        assert_eq!(s("⌘⇧K"), "shift+super+KeyK");
        assert_eq!(s("⌥⇧M"), "alt+shift+KeyM");
    }

    #[test]
    fn errors() {
        assert_eq!(parse_chord(""), Err(ChordError::Empty));
        assert!(matches!(parse_chord("Alt+Shift"), Err(ChordError::NoKey(_))));
        assert!(matches!(parse_chord("Alt+"), Err(ChordError::NoKey(_))));
        assert!(matches!(parse_chord("Alt+A+B"), Err(ChordError::TwoKeys(_))));
        assert!(matches!(parse_chord("Alt+Banana"), Err(ChordError::UnknownKey(_))));
        assert!(matches!(parse_chord("F25"), Err(ChordError::UnknownKey(_))));
    }

    #[test]
    fn repeat_filter() {
        let mut f = RepeatFilter::default();
        assert!(f.accept(1, true));
        assert!(!f.accept(1, true)); // auto-repeat
        assert!(f.accept(2, true));
        assert!(f.accept(1, false));
        assert!(!f.accept(1, false)); // stray release
        assert!(f.accept(1, true));
    }
}
