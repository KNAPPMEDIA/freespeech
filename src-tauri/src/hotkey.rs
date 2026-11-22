//! Globales Tastenkürzel über einen Low-Level-Tastatur-Hook.
//! Nötig, weil Kürzel nur aus Zusatztasten (z. B. Strg links + Win links) und
//! "halten zum Sprechen" mit RegisterHotKey nicht gehen.

use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
};
use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN, WM_TIMER,
};

/// Kennzeichnet Tastendrücke, die FreeSpeech selbst erzeugt (Einfügen, Maskentaste).
pub const MAGIC: usize = 0x4653_5045;

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
    binding: Vec<Vec<u32>>,
    cancel: Vec<Vec<u32>>,
    pressed: HashSet<u32>,
    swallowed: HashSet<u32>,
    active: bool,
    recording: bool,
    capture: Option<Vec<u32>>,
    tx: Option<Sender<Event>>,
}

/// Mit FREESPEECH_DEBUG=1 schreibt der Hook jede Taste ins Log.
fn debug() -> bool {
    static D: OnceLock<bool> = OnceLock::new();
    *D.get_or_init(|| std::env::var("FREESPEECH_DEBUG").is_ok())
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

const KEYS: &[(&str, u32)] = &[
    ("ctrl_left", 0xA2), ("ctrl_right", 0xA3), ("shift_left", 0xA0), ("shift_right", 0xA1),
    ("alt_left", 0xA4), ("alt_right", 0xA5), ("super_left", 0x5B), ("super_right", 0x5C),
    ("space", 0x20), ("escape", 0x1B), ("tab", 0x09), ("enter", 0x0D), ("backspace", 0x08),
    ("capslock", 0x14), ("insert", 0x2D), ("delete", 0x2E), ("home", 0x24), ("end", 0x23),
    ("pageup", 0x21), ("pagedown", 0x22), ("left", 0x25), ("up", 0x26), ("right", 0x27), ("down", 0x28),
    ("pause", 0x13), ("scrolllock", 0x91), ("printscreen", 0x2C), ("menu", 0x5D),
    ("comma", 0xBC), ("period", 0xBE), ("minus", 0xBD), ("plus", 0xBB), ("hash", 0xBF), ("caret", 0xDC),
    ("less", 0xE2),
];

fn token_to_group(tok: &str) -> Option<Vec<u32>> {
    let t = tok.trim().to_lowercase();
    match t.as_str() {
        "ctrl" | "control" | "strg" => return Some(vec![0xA2, 0xA3]),
        "shift" | "umschalt" => return Some(vec![0xA0, 0xA1]),
        "alt" => return Some(vec![0xA4, 0xA5]),
        "super" | "win" | "meta" | "cmd" => return Some(vec![0x5B, 0x5C]),
        "esc" => return Some(vec![0x1B]),
        "return" => return Some(vec![0x0D]),
        _ => {}
    }
    if let Some((_, vk)) = KEYS.iter().find(|(n, _)| *n == t) {
        return Some(vec![*vk]);
    }
    if t.len() == 1 {
        let c = t.chars().next().unwrap().to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            return Some(vec![c as u32]);
        }
    }
    if let Some(n) = t.strip_prefix("numpad").and_then(|n| n.parse::<u32>().ok()) {
        if n <= 9 {
            return Some(vec![0x60 + n]);
        }
    }
    if let Some(n) = t.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(vec![0x70 + n - 1]);
        }
    }
    if let Some(n) = t.strip_prefix("vk_").and_then(|n| u32::from_str_radix(n, 16).ok()) {
        return Some(vec![n]);
    }
    None
}

/// "ctrl_left+super_left" → Gruppen von Tasten, von denen je eine gedrückt sein muss.
pub fn parse(combo: &str) -> Option<Vec<Vec<u32>>> {
    let groups: Option<Vec<Vec<u32>>> = combo.split('+').map(token_to_group).collect();
    groups.filter(|g| !g.is_empty())
}

fn vk_to_token(vk: u32) -> String {
    if let Some((n, _)) = KEYS.iter().find(|(_, v)| *v == vk) {
        return n.to_string();
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_ascii_lowercase().to_string(),
        0x60..=0x69 => format!("numpad{}", vk - 0x60),
        0x70..=0x87 => format!("f{}", vk - 0x70 + 1),
        _ => format!("vk_{vk:02x}"),
    }
}

fn is_modifier(vk: u32) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0xA0..=0xA5 | 0x5B | 0x5C)
}

fn satisfied(groups: &[Vec<u32>], pressed: &HashSet<u32>) -> bool {
    !groups.is_empty() && groups.iter().all(|g| g.iter().any(|k| pressed.contains(k)))
}

/// Genau das Kürzel, keine zusätzlichen Tasten (sonst löst z. B. Strg+Win+Pfeil aus).
fn exact(groups: &[Vec<u32>], pressed: &HashSet<u32>) -> bool {
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

/// Nächstes gedrücktes Kürzel abfangen und als Event::Captured melden.
pub fn begin_capture() {
    let mut s = state().lock().unwrap();
    s.capture = Some(Vec::new());
    s.pressed.clear();
}

pub fn end_capture() {
    state().lock().unwrap().capture = None;
}

fn send_key(vk: u16, up: bool) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: MAGIC,
            },
        },
    };
    unsafe {
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

unsafe extern "system" fn hook(code: i32, wparam: usize, lparam: isize) -> isize {
    if code < 0 {
        return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
    }
    let k = &*(lparam as *const KBDLLHOOKSTRUCT);
    if k.dwExtraInfo == MAGIC {
        return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
    }
    let vk = k.vkCode;
    let down = wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN;
    crate::settings::stage(4, 1);
    // Nur Zusatztasten und die Testtaste, nie Buchstaben oder Ziffern: im Log soll
    // nicht nachlesbar sein, was getippt wurde.
    if debug() && (is_modifier(vk) || vk == 0xE8) {
        crate::settings::log(&format!("Taste {vk:#04x} {}", if down { "runter" } else { "hoch" }));
    }
    let mut swallow = false;
    let mut mask_win = false;
    let mut up_note: Option<String> = None;
    {
        let Ok(mut s) = state().lock() else {
            return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
        };

        if s.capture.is_some() {
            if down {
                s.pressed.insert(vk);
                let cap = s.capture.as_mut().unwrap();
                if !cap.contains(&vk) {
                    cap.push(vk);
                }
            } else {
                s.pressed.remove(&vk);
                if s.pressed.is_empty() {
                    let mut keys = s.capture.take().unwrap_or_default();
                    // Zusatztasten zuerst, damit das Kürzel lesbar bleibt.
                    keys.sort_by_key(|k| !is_modifier(*k));
                    let combo = keys.iter().map(|k| vk_to_token(*k)).collect::<Vec<_>>().join("+");
                    if !combo.is_empty() {
                        if let Some(tx) = &s.tx {
                            let _ = tx.send(Event::Captured(combo));
                        }
                    }
                }
            }
            return 1;
        }

        if down {
            let repeat = s.pressed.contains(&vk);
            if !repeat {
                // Verpasste Loslass-Ereignisse (Sperrbildschirm, UAC) ausbügeln.
                s.pressed.retain(|k| (GetAsyncKeyState(*k as i32) as u16 & 0x8000) != 0);
                s.pressed.insert(vk);
            }
            if s.swallowed.contains(&vk) {
                swallow = true;
            } else if !repeat {
                if s.recording && !s.active && satisfied(&s.cancel, &s.pressed) && s.cancel.iter().any(|g| g.contains(&vk)) {
                    if let Some(tx) = &s.tx {
                        let _ = tx.send(Event::Cancel);
                    }
                    s.swallowed.insert(vk);
                    swallow = true;
                } else if !s.active && !exact(&s.binding, &s.pressed) && satisfied(&s.binding, &s.pressed) {
                    if debug() {
                        let extra = s.pressed.iter().filter(|k| !s.binding.iter().any(|g| g.contains(k))).count();
                        crate::settings::log(&format!("Kürzel gedrückt, aber {extra} weitere Taste(n) gehalten"));
                    }
                } else if !s.active && exact(&s.binding, &s.pressed) {
                    s.active = true;
                    if let Some(tx) = &s.tx {
                        let _ = tx.send(Event::Down);
                    }
                    if !is_modifier(vk) {
                        s.swallowed.insert(vk);
                        swallow = true;
                    }
                    mask_win = s.binding.iter().any(|g| g.contains(&0x5B) || g.contains(&0x5C));
                }
            }
        } else {
            let held_before = s.pressed.remove(&vk);
            if s.active && !satisfied(&s.binding, &s.pressed) {
                s.active = false;
                if let Some(tx) = &s.tx {
                    let _ = tx.send(Event::Up);
                }
                // Festhalten, welche Taste die Aufnahme beendet hat. Gehört sie nicht
                // zum Kürzel, wurde eine Kürzel-Taste vorher fälschlich als losgelassen
                // gewertet.
                let in_binding = s.binding.iter().any(|g| g.contains(&vk));
                up_note = Some(format!(
                    "Kürzel losgelassen ({}{})",
                    if in_binding { vk_to_token(vk) } else { "fremde Taste".to_string() },
                    if held_before { "" } else { ", war nicht als gedrückt vermerkt" }
                ));
            } else if !s.active && exact(&s.binding, &s.pressed) {
                // Das Kürzel wurde gedrückt, während noch eine andere Taste unten war
                // (schnelles Tippen). Sobald die losgelassen ist, gilt es.
                s.active = true;
                if let Some(tx) = &s.tx {
                    let _ = tx.send(Event::Down);
                }
                mask_win = s.binding.iter().any(|g| g.contains(&0x5B) || g.contains(&0x5C));
            }
            if s.swallowed.remove(&vk) {
                swallow = true;
            }
        }
    }
    if let Some(note) = up_note {
        crate::settings::log(&note);
    }
    if mask_win {
        // Unbelegte Taste senden, solange Win gehalten ist: so öffnet das Loslassen
        // der Win-Taste nicht das Startmenü.
        send_key(0xE8, false);
        send_key(0xE8, true);
    }
    crate::settings::stage(4, 0);
    if swallow {
        return 1;
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

/// Installiert den Hook in einem eigenen Thread mit Nachrichtenschleife.
///
/// Windows entfernt einen Low-Level-Hook stillschweigend, sobald er einmal zu
/// langsam antwortet (z. B. wenn ein Build alle Kerne belegt). Darum läuft der
/// Thread mit höchster Priorität, und der Hook wird alle paar Sekunden neu
/// gesetzt, solange gerade keine Taste gehalten wird.
pub fn start(tx: Sender<Event>) {
    state().lock().unwrap().tx = Some(tx);
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(|| unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
            let hmod = GetModuleHandleW(std::ptr::null());
            let mut h = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), hmod, 0);
            if h.is_null() {
                crate::settings::log("Tastatur-Hook konnte nicht installiert werden");
                return;
            }
            crate::settings::log("Tastatur-Hook aktiv");
            SetTimer(std::ptr::null_mut(), 0, 5000, None);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                if msg.message != WM_TIMER {
                    continue;
                }
                crate::settings::stage(4, 2);
                let idle = {
                    let mut s = state().lock().unwrap();
                    s.pressed.retain(|k| (GetAsyncKeyState(*k as i32) as u16 & 0x8000) != 0);
                    !s.active && s.capture.is_none() && s.pressed.is_empty() && s.swallowed.is_empty()
                };
                if idle {
                    let fresh = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), hmod, 0);
                    if !fresh.is_null() {
                        let old_alive = UnhookWindowsHookEx(h) != 0;
                        h = fresh;
                        if debug() || !old_alive {
                            crate::settings::log(if old_alive {
                                "Hook neu gesetzt"
                            } else {
                                "Hook war von Windows entfernt worden, neu gesetzt"
                            });
                        }
                    }
                } else if debug() {
                    crate::settings::log("Hook-Takt: Taste gehalten, nicht neu gesetzt");
                }
            }
        })
        .expect("hotkey thread");
}

/// Lesbare Fassung für die Oberfläche: "ctrl_left+super_left" → ["Strg links", "Win links"].
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
                "ctrl" | "control" | "strg" => "Strg".to_string(),
                "shift" => "Umschalt".to_string(),
                "alt" => "Alt".to_string(),
                "super" | "win" | "meta" => "Win".to_string(),
                "space" => "Leertaste".to_string(),
                "escape" | "esc" => "Esc".to_string(),
                "enter" => "Enter".to_string(),
                "backspace" => "Rücktaste".to_string(),
                "capslock" => "Feststell".to_string(),
                "delete" => "Entf".to_string(),
                "insert" => "Einfg".to_string(),
                "pageup" => "Bild auf".to_string(),
                "pagedown" => "Bild ab".to_string(),
                other => other.to_uppercase(),
            };
            format!("{name}{side}")
        })
        .collect()
}
