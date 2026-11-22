//! Globales Tastenkürzel auf dem Mac über einen Event-Tap (Quartz).
//! Entspricht dem Windows-Hook: auch Kürzel nur aus Zusatztasten und "halten zum
//! Sprechen". Braucht die Berechtigung "Bedienungshilfen".

use crate::{key_is_down, MAGIC};
use core_foundation::runloop::{kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoop};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    EventField,
};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum Event {
    /// Kürzel vollständig gedrückt.
    Down,
    /// Kürzel losgelassen.
    Up,
    /// Abbruchtaste während der Aufnahme.
    Cancel,
    /// Im Aufnahmemodus der Einstellungen: dieses Kürzel wurde gedrückt.
    Captured(String),
}

struct State {
    binding: Vec<Vec<u16>>,
    cancel: Vec<Vec<u16>>,
    pressed: HashSet<u16>,
    swallowed: HashSet<u16>,
    active: bool,
    recording: bool,
    capture: Option<Vec<u16>>,
    tx: Option<Sender<Event>>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(State {
            binding: Vec::new(),
            cancel: Vec::new(),
            pressed: HashSet::new(),
            swallowed: HashSet::new(),
            active: false,
            recording: false,
            capture: None,
            tx: None,
        })
    })
}

/// Fehlt die Berechtigung "Bedienungshilfen", steht das hier, bis sie erteilt ist.
static BLOCKED: AtomicBool = AtomicBool::new(false);

pub fn permission_missing() -> bool {
    BLOCKED.load(Ordering::Relaxed)
}

/// Tastennamen wie unter Windows, Werte sind die Mac-Tastencodes (nach Position
/// auf der Tastatur, unabhängig von der Belegung).
const KEYS: &[(&str, u16)] = &[
    ("ctrl_left", 59), ("ctrl_right", 62), ("shift_left", 56), ("shift_right", 60),
    ("alt_left", 58), ("alt_right", 61), ("super_left", 55), ("super_right", 54), ("fn", 63),
    ("space", 49), ("escape", 53), ("tab", 48), ("enter", 36), ("backspace", 51),
    ("delete", 117), ("home", 115), ("end", 119), ("pageup", 116), ("pagedown", 121),
    ("left", 123), ("right", 124), ("down", 125), ("up", 126),
    ("a", 0), ("s", 1), ("d", 2), ("f", 3), ("h", 4), ("g", 5), ("z", 6), ("x", 7), ("c", 8), ("v", 9),
    ("b", 11), ("q", 12), ("w", 13), ("e", 14), ("r", 15), ("y", 16), ("t", 17),
    ("1", 18), ("2", 19), ("3", 20), ("4", 21), ("6", 22), ("5", 23), ("9", 25), ("7", 26), ("8", 28), ("0", 29),
    ("o", 31), ("u", 32), ("i", 34), ("p", 35), ("l", 37), ("j", 38), ("k", 40), ("n", 45), ("m", 46),
    ("comma", 43), ("period", 47), ("minus", 27), ("plus", 24),
    ("f1", 122), ("f2", 120), ("f3", 99), ("f4", 118), ("f5", 96), ("f6", 97), ("f7", 98), ("f8", 100),
    ("f9", 101), ("f10", 109), ("f11", 103), ("f12", 111), ("f13", 105), ("f14", 107), ("f15", 113),
    ("f16", 106), ("f17", 64), ("f18", 79), ("f19", 80), ("f20", 90),
];

fn token_to_group(tok: &str) -> Option<Vec<u16>> {
    let t = tok.trim().to_lowercase();
    match t.as_str() {
        "ctrl" | "control" | "strg" => return Some(vec![59, 62]),
        "shift" | "umschalt" => return Some(vec![56, 60]),
        "alt" | "option" => return Some(vec![58, 61]),
        "super" | "cmd" | "command" | "meta" | "win" => return Some(vec![55, 54]),
        "esc" => return Some(vec![53]),
        "return" => return Some(vec![36]),
        _ => {}
    }
    if let Some((_, k)) = KEYS.iter().find(|(n, _)| *n == t) {
        return Some(vec![*k]);
    }
    if let Some(n) = t.strip_prefix("key_").and_then(|n| n.parse::<u16>().ok()) {
        return Some(vec![n]);
    }
    None
}

/// "ctrl_left+alt_left" → Gruppen von Tasten, von denen je eine gedrückt sein muss.
pub fn parse(combo: &str) -> Option<Vec<Vec<u16>>> {
    let groups: Option<Vec<Vec<u16>>> = combo.split('+').map(token_to_group).collect();
    groups.filter(|g| !g.is_empty())
}

fn key_to_token(k: u16) -> String {
    match KEYS.iter().find(|(_, v)| *v == k) {
        Some((n, _)) => n.to_string(),
        None => format!("key_{k}"),
    }
}

fn is_modifier(k: u16) -> bool {
    matches!(k, 54..=63)
}

/// Zu welcher Zusatztasten-Familie gehört der Code? (für "runter oder hoch?")
fn family_flag(k: u16) -> Option<CGEventFlags> {
    Some(match k {
        59 | 62 => CGEventFlags::CGEventFlagControl,
        56 | 60 => CGEventFlags::CGEventFlagShift,
        58 | 61 => CGEventFlags::CGEventFlagAlternate,
        55 | 54 => CGEventFlags::CGEventFlagCommand,
        63 => CGEventFlags::CGEventFlagSecondaryFn,
        _ => return None,
    })
}

fn satisfied(groups: &[Vec<u16>], pressed: &HashSet<u16>) -> bool {
    !groups.is_empty() && groups.iter().all(|g| g.iter().any(|k| pressed.contains(k)))
}

/// Genau das Kürzel, keine zusätzlichen Tasten.
fn exact(groups: &[Vec<u16>], pressed: &HashSet<u16>) -> bool {
    satisfied(groups, pressed) && pressed.iter().all(|k| groups.iter().any(|g| g.contains(k)))
}

pub fn set_bindings(hotkey: &str, cancel: &str) {
    let mut s = state().lock().unwrap();
    s.binding = parse(hotkey).unwrap_or_default();
    s.cancel = parse(cancel).unwrap_or_default();
    s.active = false;
}

pub fn set_recording(on: bool) {
    state().lock().unwrap().recording = on;
}

pub fn begin_capture() {
    let mut s = state().lock().unwrap();
    s.capture = Some(Vec::new());
    s.pressed.clear();
}

pub fn end_capture() {
    state().lock().unwrap().capture = None;
}

/// Verarbeitet einen Tastendruck. Rückgabe `true` heißt: nicht an andere
/// Programme weiterreichen.
fn handle(k: u16, down: bool, repeat: bool, log: fn(&str)) -> bool {
    let mut swallow = false;
    let mut note: Option<String> = None;
    {
        let Ok(mut s) = state().lock() else { return false };

        if s.capture.is_some() {
            if down {
                s.pressed.insert(k);
                let cap = s.capture.as_mut().unwrap();
                if !cap.contains(&k) {
                    cap.push(k);
                }
            } else {
                s.pressed.remove(&k);
                if s.pressed.is_empty() {
                    let mut keys = s.capture.take().unwrap_or_default();
                    keys.sort_by_key(|k| !is_modifier(*k));
                    let combo = keys.iter().map(|k| key_to_token(*k)).collect::<Vec<_>>().join("+");
                    if !combo.is_empty() {
                        if let Some(tx) = &s.tx {
                            let _ = tx.send(Event::Captured(combo));
                        }
                    }
                }
            }
            // Zusatztasten lassen sich auf dem Mac nicht verschlucken, alles andere schon.
            return !is_modifier(k);
        }

        if down {
            let repeat = repeat || s.pressed.contains(&k);
            if !repeat {
                // Verpasste Loslass-Ereignisse ausbügeln.
                s.pressed.retain(|p| key_is_down(*p));
                s.pressed.insert(k);
            }
            if s.swallowed.contains(&k) {
                swallow = true;
            } else if !repeat {
                if s.recording && !s.active && satisfied(&s.cancel, &s.pressed) && s.cancel.iter().any(|g| g.contains(&k)) {
                    if let Some(tx) = &s.tx {
                        let _ = tx.send(Event::Cancel);
                    }
                    s.swallowed.insert(k);
                    swallow = true;
                } else if !s.active && exact(&s.binding, &s.pressed) {
                    s.active = true;
                    if let Some(tx) = &s.tx {
                        let _ = tx.send(Event::Down);
                    }
                    if !is_modifier(k) {
                        s.swallowed.insert(k);
                        swallow = true;
                    }
                }
            }
        } else {
            let held_before = s.pressed.remove(&k);
            if s.active && !satisfied(&s.binding, &s.pressed) {
                s.active = false;
                if let Some(tx) = &s.tx {
                    let _ = tx.send(Event::Up);
                }
                let in_binding = s.binding.iter().any(|g| g.contains(&k));
                note = Some(format!(
                    "Kürzel losgelassen ({}{})",
                    if in_binding { key_to_token(k) } else { "fremde Taste".to_string() },
                    if held_before { "" } else { ", war nicht als gedrückt vermerkt" }
                ));
            } else if !s.active && exact(&s.binding, &s.pressed) {
                s.active = true;
                if let Some(tx) = &s.tx {
                    let _ = tx.send(Event::Down);
                }
            }
            if s.swallowed.remove(&k) {
                swallow = true;
            }
        }
    }
    if let Some(n) = note {
        log(&n);
    }
    swallow && !is_modifier(k)
}

/// Startet den Abgriff in einem eigenen Thread. Solange die Berechtigung
/// "Bedienungshilfen" fehlt, wird alle paar Sekunden erneut versucht.
pub fn start(tx: Sender<Event>, log: fn(&str)) {
    state().lock().unwrap().tx = Some(tx);
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || {
            let mut asked = false;
            loop {
                if !crate::system::accessibility_trusted(!asked) {
                    if !asked {
                        log("Berechtigung „Bedienungshilfen“ fehlt: Tastenkürzel und Einfügen gehen erst danach");
                    }
                    asked = true;
                    BLOCKED.store(true, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                }

                let tap = CGEventTap::new(
                    CGEventTapLocation::HID,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::Default,
                    vec![CGEventType::KeyDown, CGEventType::KeyUp, CGEventType::FlagsChanged],
                    move |_proxy, kind, event: &CGEvent| -> Option<CGEvent> {
                        if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == MAGIC {
                            return Some(event.clone());
                        }
                        let k = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                        let swallow = match kind {
                            CGEventType::KeyDown => {
                                let repeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
                                handle(k, true, repeat, log)
                            }
                            CGEventType::KeyUp => handle(k, false, false, log),
                            CGEventType::FlagsChanged => {
                                if let Some(flag) = family_flag(k) {
                                    let known = state().lock().map(|s| s.pressed.contains(&k)).unwrap_or(false);
                                    let down = event.get_flags().contains(flag) && !known;
                                    handle(k, down, false, log);
                                }
                                false
                            }
                            _ => false,
                        };
                        if swallow {
                            None
                        } else {
                            Some(event.clone())
                        }
                    },
                );
                let Ok(tap) = tap else {
                    log("Tastatur-Abgriff ließ sich nicht einrichten, neuer Versuch in 3 s");
                    BLOCKED.store(true, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                };
                let Ok(source) = tap.mach_port.create_runloop_source(0) else {
                    log("Tastatur-Abgriff: keine Schleifenquelle, neuer Versuch in 3 s");
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                };
                unsafe {
                    CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes);
                }
                tap.enable();
                BLOCKED.store(false, Ordering::Relaxed);
                log("Tastatur-Abgriff aktiv");
                // Das System schaltet einen Abgriff ab, wenn er zu langsam antwortet.
                // Darum alle zwei Sekunden wieder einschalten (schadet nicht, wenn er läuft).
                loop {
                    unsafe {
                        CFRunLoop::run_in_mode(kCFRunLoopDefaultMode, Duration::from_secs(2), false);
                    }
                    tap.enable();
                    if !crate::system::accessibility_trusted(false) {
                        log("Berechtigung „Bedienungshilfen“ wurde entzogen");
                        break;
                    }
                }
            }
        })
        .expect("hotkey thread");
}

/// Lesbare Fassung für die Oberfläche: "ctrl_left+alt_left" → ["Control links", "Option links"].
pub fn labels(combo: &str) -> Vec<String> {
    combo
        .split('+')
        .map(|t| {
            let t = t.trim().to_lowercase();
            let (base, side) = match t.rsplit_once('_') {
                Some((b, "left")) => (b.to_string(), " links"),
                Some((b, "right")) => (b.to_string(), " rechts"),
                _ => (t.clone(), ""),
            };
            let name = match base.as_str() {
                "ctrl" | "control" | "strg" => "Control".to_string(),
                "shift" => "Umschalt".to_string(),
                "alt" | "option" => "Option".to_string(),
                "super" | "cmd" | "command" | "meta" | "win" => "Command".to_string(),
                "fn" => "fn".to_string(),
                "space" => "Leertaste".to_string(),
                "escape" | "esc" => "esc".to_string(),
                "enter" => "Return".to_string(),
                "backspace" => "Rücktaste".to_string(),
                "delete" => "Entf".to_string(),
                "pageup" => "Bild auf".to_string(),
                "pagedown" => "Bild ab".to_string(),
                other => other.to_uppercase(),
            };
            format!("{name}{side}")
        })
        .collect()
}
