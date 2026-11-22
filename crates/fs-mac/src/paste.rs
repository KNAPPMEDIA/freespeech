//! Text ins aktive Fenster bringen: Command+V senden oder direkt tippen.
//! Braucht wie das Tastenkürzel die Berechtigung "Bedienungshilfen".

use crate::{key_is_down, MAGIC};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, EventField};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::time::{Duration, Instant};

fn source() -> Option<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()
}

fn post(event: CGEvent) {
    event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, MAGIC);
    event.post(CGEventTapLocation::HID);
}

/// Wartet, bis Command/Option/Control/Umschalt losgelassen sind. Sonst würde aus
/// dem Einfügen ein anderes Tastenkürzel.
pub fn wait_modifiers_released() {
    let start = Instant::now();
    let mods = [54u16, 55, 56, 58, 59, 60, 61, 62];
    while start.elapsed() < Duration::from_millis(1500) {
        if !mods.iter().any(|k| key_is_down(*k)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// Command+V.
pub fn press_paste() {
    const KEY_V: u16 = 9;
    for down in [true, false] {
        let Some(src) = source() else { return };
        if let Ok(e) = CGEvent::new_keyboard_event(src, KEY_V, down) {
            e.set_flags(CGEventFlags::CGEventFlagCommand);
            post(e);
        }
        std::thread::sleep(Duration::from_millis(12));
    }
}

/// Tippt den Text als Unicode-Zeichenfolge, in kleinen Stücken.
pub fn type_text(text: &str) {
    const KEY_RETURN: u16 = 36;
    let mut piece = String::new();
    let flush = |piece: &mut String| {
        if piece.is_empty() {
            return;
        }
        for down in [true, false] {
            let Some(src) = source() else { return };
            if let Ok(e) = CGEvent::new_keyboard_event(src, 0, down) {
                e.set_string(piece);
                post(e);
            }
        }
        piece.clear();
        std::thread::sleep(Duration::from_millis(4));
    };
    for ch in text.chars() {
        if ch == '\r' {
            continue;
        }
        if ch == '\n' {
            flush(&mut piece);
            for down in [true, false] {
                let Some(src) = source() else { return };
                if let Ok(e) = CGEvent::new_keyboard_event(src, KEY_RETURN, down) {
                    post(e);
                }
            }
            continue;
        }
        piece.push(ch);
        // Das System nimmt je Ereignis nur wenige Zeichen an.
        if piece.encode_utf16().count() >= 16 {
            flush(&mut piece);
        }
    }
    flush(&mut piece);
}
