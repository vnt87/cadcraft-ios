//! Hardware-keyboard translation for the iPad: USB HID usage codes (what GameController reports as
//! `GCKeyCode`) to egui key events.
//!
//! winit's UIKit view only receives text (`insertText:`) and Backspace from a keyboard, so Enter,
//! arrows, Escape, Tab, function keys and Cmd/Ctrl shortcuts never reach egui. This module turns the
//! raw key state into `egui::Event::Key` for exactly those keys, leaving printable characters and
//! plain Backspace to winit so nothing is typed twice. It is free of UIKit types so it is tested on
//! the host.
use eframe::egui::{Event, Key, Modifiers};

/// Seconds a key is held before it starts repeating, and the gap between repeats.
const REPEAT_DELAY: f64 = 0.4;
const REPEAT_INTERVAL: f64 = 1.0 / 30.0;

const LEFT_CTRL: u32 = 0xE0;
const LEFT_SHIFT: u32 = 0xE1;
const LEFT_ALT: u32 = 0xE2;
const LEFT_GUI: u32 = 0xE3;
const RIGHT_CTRL: u32 = 0xE4;
const RIGHT_SHIFT: u32 = 0xE5;
const RIGHT_ALT: u32 = 0xE6;
const RIGHT_GUI: u32 = 0xE7;

/// The egui key for a HID usage code, if CADCraft has one.
pub fn key_for(code: u32) -> Option<Key> {
    use Key::*;
    const LETTERS: [Key; 26] = [A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z];
    const DIGITS: [Key; 10] = [Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9, Num0];
    const FUNCTION: [Key; 12] = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    Some(match code {
        0x04..=0x1D => *LETTERS.get((code - 0x04) as usize)?,
        0x1E..=0x27 => *DIGITS.get((code - 0x1E) as usize)?,
        0x28 | 0x58 => Enter,
        0x29 => Escape,
        0x2A => Backspace,
        0x2B => Tab,
        0x2C => Space,
        0x2D => Minus,
        0x2E => Equals,
        0x2F => OpenBracket,
        0x30 => CloseBracket,
        0x31 => Backslash,
        0x33 => Semicolon,
        0x34 => Quote,
        0x35 => Backtick,
        0x36 => Comma,
        0x37 => Period,
        0x38 => Slash,
        0x3A..=0x45 => *FUNCTION.get((code - 0x3A) as usize)?,
        0x49 => Insert,
        0x4A => Home,
        0x4B => PageUp,
        0x4C => Delete,
        0x4D => End,
        0x4E => PageDown,
        0x4F => ArrowRight,
        0x50 => ArrowLeft,
        0x51 => ArrowDown,
        0x52 => ArrowUp,
        _ => return None,
    })
}

/// The US-layout character a key types, if it types one.
pub fn text_for(code: u32, shift: bool) -> Option<char> {
    let (plain, shifted) = match code {
        0x04..=0x1D => {
            let letter = char::from(b'a'.checked_add(u8::try_from(code - 0x04).ok()?)?);
            (letter, letter.to_ascii_uppercase())
        }
        0x1E..=0x26 => {
            let digit = char::from(b'1'.checked_add(u8::try_from(code - 0x1E).ok()?)?);
            (digit, *['!', '@', '#', '$', '%', '^', '&', '*', '('].get((code - 0x1E) as usize)?)
        }
        0x27 => ('0', ')'),
        0x2C => (' ', ' '),
        0x2D => ('-', '_'),
        0x2E => ('=', '+'),
        0x2F => ('[', '{'),
        0x30 => (']', '}'),
        0x31 => ('\\', '|'),
        0x33 => (';', ':'),
        0x34 => ('\'', '"'),
        0x35 => ('`', '~'),
        0x36 => (',', '<'),
        0x37 => ('.', '>'),
        0x38 => ('/', '?'),
        _ => return None,
    };
    Some(if shift { shifted } else { plain })
}

/// Keys winit never delivers from a hardware keyboard: always forwarded.
fn is_navigation(key: Key) -> bool {
    use Key::*;
    matches!(
        key,
        Enter
            | Escape
            | Tab
            | Insert
            | Delete
            | Home
            | End
            | PageUp
            | PageDown
            | ArrowUp
            | ArrowDown
            | ArrowLeft
            | ArrowRight
            | F1
            | F2
            | F3
            | F4
            | F5
            | F6
            | F7
            | F8
            | F9
            | F10
            | F11
            | F12
    )
}

/// Keys that repeat while held (text keys too, when typed here rather than through UIKit's `insertText:`).
fn repeats(key: Key) -> bool {
    use Key::*;
    matches!(key, Enter | Tab | Backspace | Delete | Home | End | PageUp | PageDown | ArrowUp | ArrowDown | ArrowLeft | ArrowRight)
}

#[derive(Clone, Copy)]
struct Held {
    key: Key,
    /// Set for a key that types a character: its repeats are text, not key events.
    text: Option<char>,
    next_repeat: f64,
}

/// Held modifiers and the key being repeated.
#[derive(Default)]
pub struct HwKeys {
    ctrl: [bool; 2],
    shift: [bool; 2],
    alt: [bool; 2],
    gui: [bool; 2],
    held: Option<Held>,
}

impl HwKeys {
    pub fn modifiers(&self) -> Modifiers {
        let command = self.gui.iter().any(|d| *d);
        Modifiers {
            alt: self.alt.iter().any(|d| *d),
            ctrl: self.ctrl.iter().any(|d| *d),
            shift: self.shift.iter().any(|d| *d),
            mac_cmd: command,
            command,
        }
    }

    /// Forget every held key and modifier (the app lost focus, so key-ups may never arrive).
    pub fn release_all(&mut self) -> Vec<Event> {
        let had_modifiers = self.modifiers() != Modifiers::default();
        *self = Self::default();
        if had_modifiers { vec![Event::ModifiersChanged(Modifiers::default())] } else { Vec::new() }
    }

    /// Whether a repeating key is held.
    #[cfg(test)]
    pub fn is_repeating(&self) -> bool {
        self.held.is_some()
    }

    /// Seconds until the next repeat is due, if a key is held.
    pub fn next_wake(&self, now: f64) -> Option<f64> {
        self.held.map(|h| (h.next_repeat - now).max(0.0))
    }

    /// One key went down or up. Returns the egui events to inject.
    ///
    /// `wants_text` is true while an egui text field has focus: winit then delivers the text and
    /// Backspace itself. Otherwise (the command line, the canvas) no UIKit text input is active, so
    /// printable keys and Backspace are produced here.
    pub fn handle(&mut self, code: u32, pressed: bool, now: f64, wants_text: bool) -> Vec<Event> {
        let before = self.modifiers();
        let slot = |flags: &mut [bool; 2], index: usize| {
            if let Some(flag) = flags.get_mut(index) {
                *flag = pressed;
            }
        };
        match code {
            LEFT_CTRL => slot(&mut self.ctrl, 0),
            RIGHT_CTRL => slot(&mut self.ctrl, 1),
            LEFT_SHIFT => slot(&mut self.shift, 0),
            RIGHT_SHIFT => slot(&mut self.shift, 1),
            LEFT_ALT => slot(&mut self.alt, 0),
            RIGHT_ALT => slot(&mut self.alt, 1),
            LEFT_GUI => slot(&mut self.gui, 0),
            RIGHT_GUI => slot(&mut self.gui, 1),
            _ => {}
        }
        let modifiers = self.modifiers();
        // egui learns the modifiers from this event (RawInput has no field for them).
        let mut events = if modifiers != before { vec![Event::ModifiersChanged(modifiers)] } else { Vec::new() };
        let Some(key) = key_for(code) else { return events };
        let shortcut = modifiers.ctrl || modifiers.command;
        if !wants_text
            && !shortcut
            && !modifiers.alt
            && let Some(ch) = text_for(code, modifiers.shift)
        {
            if pressed {
                events.push(Event::Text(ch.to_string()));
                self.held = Some(Held { key, text: Some(ch), next_repeat: now + REPEAT_DELAY });
            } else if self.held.is_some_and(|h| h.key == key) {
                self.held = None;
            }
            return events;
        }
        let own_backspace = !wants_text && key == Key::Backspace;
        // Plain text keys and Backspace arrive through winit; everything else is ours.
        if !(is_navigation(key) || own_backspace || (shortcut && key != Key::Backspace)) {
            return events;
        }
        if pressed {
            if repeats(key) {
                self.held = Some(Held { key, text: None, next_repeat: now + REPEAT_DELAY });
            }
        } else if self.held.is_some_and(|h| h.key == key) {
            self.held = None;
        }
        events.push(Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers });
        events
    }

    /// Repeats due at `now` for a key that is still held.
    pub fn tick(&mut self, now: f64) -> Vec<Event> {
        let modifiers = self.modifiers();
        let Some(held) = self.held.as_mut() else { return Vec::new() };
        let mut events = Vec::new();
        // Bounded: a long stall yields a few repeats, not hundreds.
        while now >= held.next_repeat && events.len() < 4 {
            events.push(match held.text {
                Some(ch) => Event::Text(ch.to_string()),
                None => Event::Key { key: held.key, physical_key: Some(held.key), pressed: true, repeat: true, modifiers },
            });
            held.next_repeat += REPEAT_INTERVAL;
        }
        if now >= held.next_repeat {
            held.next_repeat = now + REPEAT_INTERVAL;
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RETURN: u32 = 0x28;
    const A: u32 = 0x04;
    const BACKSPACE: u32 = 0x2A;
    const LEFT: u32 = 0x50;

    fn key_events(events: &[Event]) -> Vec<(Key, bool, bool)> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Key { key, pressed, repeat, .. } => Some((*key, *pressed, *repeat)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn maps_hid_usages() {
        assert_eq!(key_for(A), Some(Key::A));
        assert_eq!(key_for(0x1D), Some(Key::Z));
        assert_eq!(key_for(0x1E), Some(Key::Num1));
        assert_eq!(key_for(0x27), Some(Key::Num0));
        assert_eq!(key_for(RETURN), Some(Key::Enter));
        assert_eq!(key_for(0x58), Some(Key::Enter));
        assert_eq!(key_for(0x3A), Some(Key::F1));
        assert_eq!(key_for(0x45), Some(Key::F12));
        assert_eq!(key_for(LEFT), Some(Key::ArrowLeft));
        assert_eq!(key_for(0x52), Some(Key::ArrowUp));
        assert_eq!(key_for(0), None);
        assert_eq!(key_for(u32::MAX), None);
    }

    #[test]
    fn enter_and_arrows_are_forwarded() {
        let mut keys = HwKeys::default();
        assert_eq!(key_events(&keys.handle(RETURN, true, 0.0, true)), vec![(Key::Enter, true, false)]);
        assert_eq!(key_events(&keys.handle(RETURN, false, 0.1, true)), vec![(Key::Enter, false, false)]);
        assert_eq!(key_events(&keys.handle(LEFT, true, 0.2, true)), vec![(Key::ArrowLeft, true, false)]);
    }

    #[test]
    fn plain_text_keys_and_backspace_are_left_to_winit() {
        let mut keys = HwKeys::default();
        assert!(keys.handle(A, true, 0.0, true).is_empty());
        assert!(keys.handle(A, false, 0.1, true).is_empty());
        assert!(keys.handle(BACKSPACE, true, 0.2, true).is_empty());
        // Shift alone is still text.
        keys.handle(LEFT_SHIFT, true, 0.3, true);
        assert!(keys.handle(A, true, 0.3, true).is_empty());
    }

    #[test]
    fn cmd_a_is_a_command_shortcut() {
        let mut keys = HwKeys::default();
        assert_eq!(keys.handle(LEFT_GUI, true, 0.0, true).len(), 1);
        let events = keys.handle(A, true, 0.1, true);
        assert_eq!(key_events(&events), vec![(Key::A, true, false)]);
        assert!(matches!(events.first(), Some(Event::Key { modifiers, .. }) if modifiers.command && modifiers.mac_cmd && !modifiers.ctrl));
        // Cmd+Backspace is deleteBackward's to handle, not ours.
        assert!(keys.handle(BACKSPACE, true, 0.2, true).is_empty());
        keys.handle(LEFT_GUI, false, 0.3, true);
        assert!(keys.handle(A, true, 0.4, true).is_empty());
        assert!(!keys.modifiers().command);
    }

    #[test]
    fn shift_arrow_carries_shift() {
        let mut keys = HwKeys::default();
        keys.handle(RIGHT_SHIFT, true, 0.0, true);
        let events = keys.handle(LEFT, true, 0.1, true);
        assert!(matches!(events.first(), Some(Event::Key { modifiers, .. }) if modifiers.shift && !modifiers.command));
        assert!(matches!(keys.handle(RIGHT_SHIFT, false, 0.2, true).first(), Some(Event::ModifiersChanged(m)) if !m.shift));
    }

    #[test]
    fn both_sides_of_a_modifier_must_be_released() {
        let mut keys = HwKeys::default();
        keys.handle(LEFT_GUI, true, 0.0, true);
        keys.handle(RIGHT_GUI, true, 0.0, true);
        keys.handle(LEFT_GUI, false, 0.1, true);
        assert!(keys.modifiers().command);
        keys.handle(RIGHT_GUI, false, 0.2, true);
        assert!(!keys.modifiers().command);
    }

    #[test]
    fn held_arrow_repeats_after_the_delay_until_released() {
        let mut keys = HwKeys::default();
        keys.handle(LEFT, true, 1.0, true);
        assert!(keys.tick(1.2).is_empty());
        assert!(keys.is_repeating());
        let repeats = key_events(&keys.tick(1.4));
        assert_eq!(repeats, vec![(Key::ArrowLeft, true, true)]);
        assert!(keys.next_wake(1.4).is_some_and(|s| s > 0.0 && s < REPEAT_INTERVAL + 1e-6));
        keys.handle(LEFT, false, 1.5, true);
        assert!(!keys.is_repeating());
        assert!(keys.tick(5.0).is_empty());
    }

    #[test]
    fn a_long_stall_does_not_flood_repeats() {
        let mut keys = HwKeys::default();
        keys.handle(LEFT, true, 0.0, true);
        assert!(keys.tick(1000.0).len() <= 4);
    }

    #[test]
    fn text_shortcuts_do_not_repeat() {
        let mut keys = HwKeys::default();
        keys.handle(LEFT_GUI, true, 0.0, true);
        keys.handle(A, true, 0.0, true);
        assert!(!keys.is_repeating());
    }

    #[test]
    fn release_all_clears_state() {
        let mut keys = HwKeys::default();
        keys.handle(LEFT_GUI, true, 0.0, true);
        keys.handle(LEFT, true, 0.0, true);
        assert!(matches!(keys.release_all().first(), Some(Event::ModifiersChanged(m)) if !m.command));
        assert!(!keys.modifiers().command);
        assert!(!keys.is_repeating());
    }

    #[test]
    fn typing_without_a_text_field_produces_text_and_backspace() {
        let mut keys = HwKeys::default();
        assert!(matches!(keys.handle(A, true, 0.0, false).as_slice(), [Event::Text(t)] if t == "a"));
        assert!(keys.handle(A, false, 0.1, false).is_empty());
        keys.handle(LEFT_SHIFT, true, 0.2, false);
        assert!(matches!(keys.handle(A, true, 0.3, false).as_slice(), [Event::Text(t)] if t == "A"));
        assert_eq!(key_events(&keys.handle(BACKSPACE, true, 0.4, false)), vec![(Key::Backspace, true, false)]);
        assert!(keys.is_repeating());
    }

    #[test]
    fn a_focused_text_field_keeps_text_with_winit() {
        let mut keys = HwKeys::default();
        assert!(keys.handle(A, true, 0.0, true).is_empty());
        assert!(keys.handle(BACKSPACE, true, 0.1, true).is_empty());
    }

    #[test]
    fn shortcuts_never_type() {
        let mut keys = HwKeys::default();
        keys.handle(LEFT_GUI, true, 0.0, false);
        let events = keys.handle(A, true, 0.1, false);
        assert!(events.iter().all(|e| !matches!(e, Event::Text(_))));
    }

    #[test]
    fn us_layout_characters() {
        assert_eq!(text_for(0x1E, false), Some('1'));
        assert_eq!(text_for(0x1E, true), Some('!'));
        assert_eq!(text_for(0x27, true), Some(')'));
        assert_eq!(text_for(0x2C, false), Some(' '));
        assert_eq!(text_for(0x36, true), Some('<'));
        assert_eq!(text_for(0x29, false), None);
    }

    #[test]
    fn a_held_letter_repeats_as_text_and_stops_on_release() {
        let mut keys = HwKeys::default();
        keys.handle(A, true, 0.0, false);
        assert!(keys.tick(0.2).is_empty());
        assert!(matches!(keys.tick(0.41).as_slice(), [Event::Text(t)] if t == "a"));
        keys.handle(A, false, 0.5, false);
        assert!(keys.tick(1.0).is_empty());
    }
}
