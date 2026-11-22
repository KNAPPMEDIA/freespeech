//! FreeSpeech: Tastenkürzel halten, sprechen, loslassen, Text steht im aktiven Fenster.
//! Alles läuft lokal über transcribe.cpp.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Mac: die Engine kommt aus dem Paketverzeichnis statt aus ../vendor (siehe Cargo.toml).
#[cfg(target_os = "macos")]
extern crate transcribe_cpp_up as transcribe_cpp;

mod audio;
mod download;
mod engine;
#[cfg(windows)]
mod hotkey;
/// Auf dem Mac kommt das Tastenkürzel aus dem Paket `fs-mac`, mit denselben Namen.
#[cfg(target_os = "macos")]
mod hotkey {
    pub use fs_mac::hotkey::{
        begin_capture, end_capture, labels, parse, permission_missing, set_bindings, set_recording, Event,
    };

    pub fn start(tx: std::sync::mpsc::Sender<Event>) {
        fs_mac::hotkey::start(tx, crate::settings::log)
    }
}
mod paste;
mod settings;

use audio::Audio;
use engine::{DeviceInfo, Engine, EngineInfo};
use serde::Serialize;
use settings::{HistoryEntry, ModelFile, Settings};
use std::sync::atomic::{AtomicIsize, AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

const IDLE: u8 = 0;
const RECORDING: u8 = 1;
const TRANSCRIBING: u8 = 2;

#[derive(Debug, Clone, Serialize)]
struct Status {
    state: String,
    message: String,
}

enum Ctl {
    Hotkey(hotkey::Event),
    UiToggle,
}

struct App {
    settings: Mutex<Settings>,
    history: Mutex<Vec<HistoryEntry>>,
    status: Mutex<Status>,
    engine_info: Mutex<Option<EngineInfo>>,
    engine_error: Mutex<Option<String>>,
    devices: Mutex<Vec<DeviceInfo>>,
    capture_target: Mutex<String>,
    engine: Engine,
    audio: Audio,
    ctl: Mutex<Sender<Ctl>>,
    phase: AtomicU8,
    /// Zählt Aufnahmen, damit eine alte Vorschau-Schleife nicht in die nächste hineinredet.
    rec_id: AtomicU32,
    loading: AtomicU8,
    overlay_hwnd: AtomicIsize,
}

type Shared = Arc<App>;

/// Die Oberfläche (Fenster, Infobereich-Symbol) entsteht erst nach dem Start;
/// ihr Aufbau kann unter Last viele Sekunden dauern. Kürzel, Aufnahme und
/// Modell laufen deshalb ohne sie los und sprechen sie nur an, wenn sie da ist.
static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

#[derive(Clone, Copy)]
struct Ui;

impl Ui {
    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<(), ()> {
        if let Some(app) = APP.get() {
            let _ = app.emit(event, payload);
        }
        Ok(())
    }

    fn tray_by_id(&self, id: &str) -> Option<tauri::tray::TrayIcon> {
        APP.get().and_then(|app| app.tray_by_id(id))
    }
}

#[derive(Serialize)]
struct Snapshot {
    version: String,
    /// "windows" oder "macos", für Texte wie "Strg+V" / "Command+V".
    platform: &'static str,
    settings: Settings,
    status: Status,
    models: Vec<ModelFile>,
    devices: Vec<DeviceInfo>,
    microphones: Vec<String>,
    engine: Option<EngineInfo>,
    engine_error: Option<String>,
    history: Vec<HistoryEntry>,
    hotkey_labels: Vec<String>,
    cancel_labels: Vec<String>,
    models_dir: String,
    catalog: Vec<download::CatalogEntry>,
    download: Option<download::Progress>,
}

/// Auf dem Mac: Hinweis, solange die Berechtigung "Bedienungshilfen" fehlt.
fn permission_notice() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        if hotkey::permission_missing() {
            return Some(
                "FreeSpeech braucht die Berechtigung „Bedienungshilfen“, um das Tastenkürzel zu erkennen und Text einzufügen. Öffne Systemeinstellungen → Datenschutz & Sicherheit → Bedienungshilfen und schalte FreeSpeech ein."
                    .to_string(),
            );
        }
    }
    None
}

fn snapshot(st: &App) -> Snapshot {
    let settings = st.settings.lock().unwrap().clone();
    Snapshot {
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: if cfg!(target_os = "macos") { "macos" } else { "windows" },
        hotkey_labels: hotkey::labels(&settings.hotkey),
        cancel_labels: hotkey::labels(&settings.cancel_key),
        status: st.status.lock().unwrap().clone(),
        models: settings::scan_models(),
        devices: st.devices.lock().unwrap().clone(),
        microphones: audio::list_microphones(),
        engine: st.engine_info.lock().unwrap().clone(),
        engine_error: permission_notice().or_else(|| st.engine_error.lock().unwrap().clone()),
        history: st.history.lock().unwrap().clone(),
        models_dir: settings::models_dir().to_string_lossy().to_string(),
        catalog: download::catalog(),
        download: download::current(),
        settings,
    }
}

// ---------- Anzeige: Status, Infobereich, Overlay ----------

fn tray_image(kind: u8) -> tauri::image::Image<'static> {
    // Fünf Pegelbalken. Ruhe = hell, Aufnahme = rot, Schreiben = Bernstein.
    const N: usize = 32;
    let color: [u8; 3] = match kind {
        RECORDING => [255, 91, 79],
        TRANSCRIBING => [242, 178, 76],
        _ => [232, 230, 225],
    };
    let heights = [10usize, 18, 28, 18, 10];
    let mut px = vec![0u8; N * N * 4];
    for (i, h) in heights.iter().enumerate() {
        let x0 = 2 + i * 6;
        let y0 = (N - h) / 2;
        for y in y0..y0 + h {
            for x in x0..x0 + 4 {
                let o = (y * N + x) * 4;
                px[o..o + 3].copy_from_slice(&color);
                px[o + 3] = 255;
            }
        }
    }
    tauri::image::Image::new_owned(px, N as u32, N as u32)
}

/// Anzeige ein- oder ausblenden, nur mit Tauri-Mitteln (Mac).
#[cfg_attr(windows, allow(dead_code))]
fn show_overlay_plain(show: bool) {
    let Some(ov) = APP.get().and_then(|app| app.get_webview_window("overlay")) else { return };
    if show {
        let _ = ov.set_always_on_top(true);
        let _ = ov.show();
    } else {
        let _ = ov.hide();
    }
}

#[cfg(not(windows))]
fn show_overlay(_st: &App, show: bool) {
    show_overlay_plain(show)
}

#[cfg(windows)]
fn show_overlay(st: &App, show: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE,
    };
    let hwnd = st.overlay_hwnd.load(Ordering::Relaxed);
    if hwnd != 0 {
        unsafe {
            if show {
                // Nicht nur einblenden, sondern jedes Mal ausdrücklich ganz nach oben
                // holen. Ein bloßes Einblenden lässt das Fenster an seinem Platz im
                // Stapel, und der lag in der Praxis hinter den Programmfenstern: die
                // Aufnahme lief, aber man sah nichts.
                SetWindowPos(
                    hwnd as _,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            } else {
                ShowWindow(hwnd as _, SW_HIDE);
            }
        }
    }
}

fn set_status(app: &Ui, st: &App, state: &str, message: &str) {
    let status = Status { state: state.to_string(), message: message.to_string() };
    *st.status.lock().unwrap() = status.clone();
    let _ = app.emit("status", status);

    let kind = match state {
        "recording" => RECORDING,
        "transcribing" | "loading" => TRANSCRIBING,
        _ => IDLE,
    };
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_icon(Some(tray_image(kind)));
        let tip = match state {
            "recording" => "FreeSpeech: Aufnahme läuft",
            "transcribing" => "FreeSpeech: schreibt",
            "loading" => "FreeSpeech: lädt Modell",
            "error" => "FreeSpeech: Fehler",
            _ => "FreeSpeech: bereit",
        };
        let _ = tray.set_tooltip(Some(tip));
    }

    let overlay_on = st.settings.lock().unwrap().overlay;
    let visible = matches!(state, "recording" | "transcribing" | "error") && overlay_on;
    show_overlay(st, visible);
}

/// Fehler zeigen und nach kurzer Zeit zurück auf "bereit".
fn fail(app: &Ui, st: &Shared, message: &str) {
    settings::log(&format!("Fehler: {message}"));
    set_status(app, st, "error", message);
    let (app, st, msg) = (app.clone(), st.clone(), message.to_string());
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(3500));
        let still = {
            let s = st.status.lock().unwrap();
            s.state == "error" && s.message == msg
        };
        if still {
            set_status(&app, &st, "idle", "");
        }
    });
}

// ---------- Modell laden ----------

fn resolve_model(st: &App) -> Option<String> {
    let wanted = st.settings.lock().unwrap().model.clone();
    if !wanted.is_empty() && std::path::Path::new(&wanted).is_file() {
        return Some(wanted);
    }
    let models = settings::scan_models();
    // Für Deutsch zuerst das deutsch trainierte Parakeet, sonst das erste gefundene.
    let pick = models
        .iter()
        .find(|m| m.file.to_lowercase().contains("primeline"))
        .or_else(|| models.first())
        .map(|m| m.path.clone())?;
    let mut s = st.settings.lock().unwrap();
    s.model = pick.clone();
    settings::save_settings(&s);
    Some(pick)
}

fn reload_model(app: &Ui, st: &Shared) {
    let (app, st) = (app.clone(), st.clone());
    std::thread::spawn(move || {
        let Some(path) = resolve_model(&st) else {
            *st.engine_info.lock().unwrap() = None;
            *st.engine_error.lock().unwrap() =
                Some("Kein Modell gefunden. Lade eines oder lege eine .gguf-Datei in den Modellordner.".into());
            // Der Startzustand ist "lädt": ohne Modell gibt es nichts zu laden.
            if st.loading.load(Ordering::SeqCst) == 0 && st.status.lock().unwrap().state == "loading" {
                set_status(&app, &st, "idle", "");
            }
            let _ = app.emit("refresh", ());
            return;
        };
        let (device, threads) = {
            let s = st.settings.lock().unwrap();
            (s.device.clone(), s.threads)
        };
        st.loading.fetch_add(1, Ordering::SeqCst);
        if st.phase.load(Ordering::SeqCst) == IDLE {
            set_status(&app, &st, "loading", "");
        }
        let res = st.engine.load(&path, &device, threads);
        match res {
            Ok(info) => {
                settings::log(&format!("Modell geladen: {} auf {} ({}) in {} ms", info.variant, info.backend, info.device, info.load_ms));
                *st.engine_info.lock().unwrap() = Some(info);
                *st.engine_error.lock().unwrap() = None;
            }
            Err(e) => {
                settings::log(&format!("Modell-Fehler: {e}"));
                *st.engine_info.lock().unwrap() = None;
                *st.engine_error.lock().unwrap() = Some(e);
            }
        }
        let left = st.loading.fetch_sub(1, Ordering::SeqCst) - 1;
        if left == 0 && st.status.lock().unwrap().state == "loading" {
            set_status(&app, &st, "idle", "");
        }
        let _ = app.emit("refresh", ());
    });
}

// ---------- Aufnahme und Transkription ----------

fn begin(app: &Ui, st: &Shared) {
    let (mic, sounds) = {
        let s = st.settings.lock().unwrap();
        (s.microphone.clone(), s.sounds)
    };
    settings::log(&format!("Kürzel gedrückt, öffne Mikrofon „{mic}“"));
    match st.audio.start(&mic) {
        Ok(name) => {
            settings::log(&format!("Aufnahme gestartet: {name}"));
            st.phase.store(RECORDING, Ordering::SeqCst);
            *TEST_BEGUN.lock().unwrap() = Some(Instant::now());
            hotkey::set_recording(true);
            if sounds {
                paste::play(true);
            }
            set_status(app, st, "recording", "");
            let id = st.rec_id.fetch_add(1, Ordering::SeqCst) + 1;
            if st.settings.lock().unwrap().live_preview {
                st.engine.begin_preview();
                let (app, st) = (app.clone(), st.clone());
                std::thread::spawn(move || preview_loop(app, st, id));
            }
        }
        Err(e) => fail(app, st, &e),
    }
}

#[derive(Clone, Serialize)]
struct Partial {
    text: String,
    /// Vorne gekürzt, weil nur die letzten 30 Sekunden neu gerechnet werden.
    cut: bool,
}

/// Live-Vorschau: solange aufgenommen wird, den bisherigen Ton immer wieder
/// transkribieren und an Anzeige und Hauptfenster schicken. Das geht mit jedem
/// Modell, auch ohne eingebautes Streaming.
fn preview_loop(app: Ui, st: Shared, id: u32) {
    let alive = |st: &Shared| st.rec_id.load(Ordering::SeqCst) == id && st.phase.load(Ordering::SeqCst) == RECORDING;
    let begun = Instant::now();
    // Nur zum Prüfen: die Test-WAV so weit "abspielen", wie die Aufnahme schon läuft.
    let test = std::env::var("FREESPEECH_TEST_WAV").ok().and_then(|p| read_test_wav(&p));
    std::thread::sleep(Duration::from_millis(500));
    let mut rounds = 0u32;
    while alive(&st) {
        let started = Instant::now();
        settings::stage(0, 1);
        let (mut pcm, cut) = st.audio.snapshot(30);
        settings::stage(0, 2);
        if let Some(t) = &test {
            let n = (begun.elapsed().as_millis() as usize * 16).min(t.len());
            pcm = t[..n].to_vec();
        }
        if pcm.len() >= 8000 {
            let audio_ms = pcm.len() / 16;
            let s = st.settings.lock().unwrap().clone();
            settings::stage(0, 3);
            let res = st.engine.preview(pcm, &s.language, s.vocabulary.clone());
            settings::stage(0, 4);
            if let Ok(text) = res {
                let mut text = clean_text(&text, &s);
                text.truncate(text.trim_end().len());
                if alive(&st) && !text.is_empty() {
                    rounds += 1;
                    if test.is_some() {
                        settings::log(&format!(
                            "Test: Vorschau {rounds} nach {} ms, {} ms Ton in {} ms, {} Zeichen | {text}",
                            begun.elapsed().as_millis(),
                            audio_ms,
                            started.elapsed().as_millis(),
                            text.chars().count()
                        ));
                    }
                    settings::stage(0, 5);
                    let _ = app.emit("partial", Partial { text, cut });
                    settings::stage(0, 6);
                }
            }
        }
        settings::stage(0, 7);
        // Mindestens 350 ms Abstand, damit kurze Aufnahmen nicht pausenlos rechnen.
        let rest = Duration::from_millis(350).saturating_sub(started.elapsed());
        std::thread::sleep(rest.max(Duration::from_millis(40)));
    }
    settings::log(&format!("Vorschau: {rounds} Zwischenstände gezeigt"));
}

fn clean_text(text: &str, s: &Settings) -> String {
    let mut out = text.trim().to_string();
    // Manche Modelle (Canary) setzen ein Leerzeichen vor Satzzeichen: "Start ." → "Start."
    // Nur wenn danach ein Leerzeichen oder das Ende kommt, damit ".NET" o. ä. heil bleibt.
    let chars: Vec<char> = out.chars().collect();
    let mut fixed = String::with_capacity(out.len());
    for (i, &c) in chars.iter().enumerate() {
        let before_mark = c == ' '
            && chars.get(i + 1).map(|n| ".,?!:;".contains(*n)).unwrap_or(false)
            && chars.get(i + 2).map(|n| n.is_whitespace()).unwrap_or(true);
        if !before_mark {
            fixed.push(c);
        }
    }
    out = fixed;
    for r in &s.replacements {
        if r.from.trim().is_empty() {
            continue;
        }
        out = replace_ci(&out, r.from.trim(), &r.to);
    }
    if s.trailing_space && !out.is_empty() {
        out.push(' ');
    }
    out
}

/// Ersetzt ohne Rücksicht auf Groß-/Kleinschreibung.
fn replace_ci(hay: &str, needle: &str, with: &str) -> String {
    let lower_hay = hay.to_lowercase();
    let lower_needle = needle.to_lowercase();
    if lower_needle.is_empty() || lower_hay.len() != hay.len() {
        return hay.replace(needle, with);
    }
    let mut out = String::with_capacity(hay.len());
    let mut pos = 0;
    while let Some(i) = lower_hay[pos..].find(&lower_needle) {
        let at = pos + i;
        out.push_str(&hay[pos..at]);
        out.push_str(with);
        pos = at + lower_needle.len();
    }
    out.push_str(&hay[pos..]);
    out
}

/// Nur zum Prüfen: Beginn der laufenden Aufnahme, damit die Pegelanzeige der
/// Test-WAV folgen kann statt dem echten Mikrofon.
static TEST_BEGUN: Mutex<Option<Instant>> = Mutex::new(None);

/// Nur zum Prüfen: Millisekunden seit Beginn der laufenden Aufnahme, wenn eine Test-WAV läuft.
fn test_ms() -> Option<u128> {
    std::env::var_os("FREESPEECH_TEST_WAV")?;
    TEST_BEGUN.lock().unwrap().map(|b| b.elapsed().as_millis())
}

/// Nur zum Prüfen: liest eine 16-Bit-WAV-Datei statt des Mikrofons
/// (Umgebungsvariable FREESPEECH_TEST_WAV).
fn read_test_wav(path: &str) -> Option<Vec<f32>> {
    let b = std::fs::read(path).ok()?;
    if b.len() < 44 || &b[0..4] != b"RIFF" {
        return None;
    }
    let (mut ch, mut rate, mut bits) = (1usize, 16000u32, 16u16);
    let mut pos = 12;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= b.len() {
            ch = u16::from_le_bytes([b[body + 2], b[body + 3]]).max(1) as usize;
            rate = u32::from_le_bytes(b[body + 4..body + 8].try_into().ok()?);
            bits = u16::from_le_bytes([b[body + 14], b[body + 15]]);
        } else if id == b"data" {
            if bits != 16 {
                return None;
            }
            let end = (body + len).min(b.len());
            let mono: Vec<f32> = b[body..end]
                .chunks_exact(2 * ch)
                .map(|f| f.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0).sum::<f32>() / ch as f32)
                .collect();
            return Some(audio::resample_to_16k(&mono, rate));
        }
        pos = body + len + (len & 1);
    }
    None
}

fn finish(app: &Ui, st: &Shared) {
    settings::stage(1, 1);
    st.engine.end_preview();
    let mut pcm = st.audio.stop();
    settings::stage(1, 2);
    if let Ok(path) = std::env::var("FREESPEECH_TEST_WAV") {
        let mic_peak = pcm.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        settings::log(&format!("Test: Mikrofon lieferte {} ms, Spitze {:.4}", pcm.len() / 16, mic_peak));
        if let Some(test) = read_test_wav(&path) {
            pcm = test;
        }
    }
    hotkey::set_recording(false);
    let s = st.settings.lock().unwrap().clone();
    if s.sounds {
        paste::play(false);
    }
    let audio_ms = (pcm.len() as u64 * 1000) / 16000;
    let peak = pcm.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    settings::log(&format!("Aufnahme beendet: {audio_ms} ms Ton, Spitze {peak:.4}"));
    if audio_ms < 300 {
        // Nur angetippt: nicht stumm verwerfen, sondern sagen, was fehlt.
        st.phase.store(IDLE, Ordering::SeqCst);
        let hint = if s.mode == "toggle" {
            "Zu kurz. Kürzel drücken, sprechen, nochmal drücken."
        } else {
            "Zu kurz. Kürzel gedrückt halten, solange du sprichst."
        };
        fail(app, st, hint);
        return;
    }
    if peak < 0.002 {
        st.phase.store(IDLE, Ordering::SeqCst);
        fail(app, st, "Kein Ton vom Mikrofon. Prüfe Mikrofon und Pegel in den Einstellungen.");
        return;
    }

    st.phase.store(TRANSCRIBING, Ordering::SeqCst);
    set_status(app, st, "transcribing", "");
    if let Some(ms) = test_ms() {
        settings::log(&format!("Test: Schreibt ab {ms} ms"));
    }
    let (app, st) = (app.clone(), st.clone());
    std::thread::spawn(move || {
        let result = st.engine.run(pcm, &s.language, s.vocabulary.clone());
        st.phase.store(IDLE, Ordering::SeqCst);
        match result {
            Ok(r) => {
                settings::log(&format!(
                    "Diktat: {} ms Ton in {} ms ({:.1}x Echtzeit), {} Zeichen",
                    audio_ms, r.took_ms, audio_ms as f32 / r.took_ms.max(1) as f32, r.text.chars().count()
                ));
                let text = clean_text(&r.text, &s);
                if text.trim().is_empty() {
                    set_status(&app, &st, "idle", "");
                    return;
                }
                let delivered = paste::deliver(&text, &s.paste_method, s.restore_clipboard);
                if let Some(ms) = test_ms() {
                    settings::log(&format!("Test: Eingefügt nach {ms} ms | {text}"));
                }
                let model = settings::scan_models()
                    .into_iter()
                    .find(|m| m.path == s.model)
                    .map(|m| m.name)
                    .unwrap_or_default();
                {
                    let mut h = st.history.lock().unwrap();
                    let id = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
                    h.insert(0, HistoryEntry { id, text: text.trim().to_string(), audio_ms, took_ms: r.took_ms, model });
                    h.truncate(s.history_limit);
                    settings::save_history(&h);
                }
                match delivered {
                    Ok(()) => set_status(&app, &st, "idle", ""),
                    Err(e) => fail(&app, &st, &e),
                }
                let _ = app.emit("refresh", ());
            }
            Err(e) => fail(&app, &st, &e),
        }
    });
}

fn controller(app: Ui, st: Shared, rx: Receiver<Ctl>) {
    for msg in rx {
        settings::stage(1, 10);
        let phase = st.phase.load(Ordering::SeqCst);
        let toggle = st.settings.lock().unwrap().mode == "toggle";
        match msg {
            Ctl::Hotkey(hotkey::Event::Down) => match (phase, toggle) {
                (IDLE, _) => begin(&app, &st),
                (RECORDING, true) => finish(&app, &st),
                _ => {}
            },
            Ctl::Hotkey(hotkey::Event::Up) => {
                if phase == RECORDING && !toggle {
                    finish(&app, &st);
                }
            }
            Ctl::Hotkey(hotkey::Event::Cancel) => {
                if phase == RECORDING {
                    settings::log("Aufnahme abgebrochen (Abbruchtaste)");
                    st.engine.end_preview();
                    st.audio.cancel();
                    hotkey::set_recording(false);
                    st.phase.store(IDLE, Ordering::SeqCst);
                    set_status(&app, &st, "idle", "");
                }
            }
            Ctl::Hotkey(hotkey::Event::Captured(combo)) => {
                let target = st.capture_target.lock().unwrap().clone();
                if hotkey::parse(&combo).is_some() {
                    let mut s = st.settings.lock().unwrap();
                    if target == "cancel_key" {
                        s.cancel_key = combo;
                    } else {
                        s.hotkey = combo;
                    }
                    settings::save_settings(&s);
                    hotkey::set_bindings(&s.hotkey, &s.cancel_key);
                }
                let _ = app.emit("refresh", ());
            }
            Ctl::UiToggle => match phase {
                IDLE => begin(&app, &st),
                RECORDING => finish(&app, &st),
                _ => {}
            },
        }
    }
}

// ---------- Befehle für die Oberfläche ----------

#[tauri::command(async)]
fn get_state(st: State<'_, Shared>) -> Snapshot {
    snapshot(&st)
}

#[tauri::command(async)]
fn save_settings(st: State<'_, Shared>, settings: Settings) -> Result<Snapshot, String> {
    if hotkey::parse(&settings.hotkey).is_none() {
        return Err("Dieses Tastenkürzel kennt FreeSpeech nicht.".into());
    }
    if hotkey::parse(&settings.cancel_key).is_none() {
        return Err("Diese Abbruchtaste kennt FreeSpeech nicht.".into());
    }
    let mut new = settings;
    new.history_limit = new.history_limit.min(500);
    new.threads = new.threads.clamp(0, 64);
    let old = std::mem::replace(&mut *st.settings.lock().unwrap(), new.clone());
    settings::save_settings(&new);
    hotkey::set_bindings(&new.hotkey, &new.cancel_key);
    if old.autostart != new.autostart {
        settings::set_autostart(new.autostart)?;
    }
    if old.history_limit != new.history_limit {
        let mut h = st.history.lock().unwrap();
        h.truncate(new.history_limit);
        settings::save_history(&h);
    }
    if old.model != new.model || old.device != new.device || old.threads != new.threads {
        reload_model(&Ui, &st);
    }
    Ok(snapshot(&st))
}

#[tauri::command]
fn capture_hotkey(st: State<'_, Shared>, target: String) {
    *st.capture_target.lock().unwrap() = target;
    hotkey::begin_capture();
}

#[tauri::command]
fn cancel_capture() {
    hotkey::end_capture();
}

#[tauri::command]
fn toggle_record(st: State<'_, Shared>) {
    let _ = st.ctl.lock().unwrap().send(Ctl::UiToggle);
}

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    paste::copy(&text)
}

#[tauri::command(async)]
fn delete_history(st: State<'_, Shared>, id: u64) -> Snapshot {
    {
        let mut h = st.history.lock().unwrap();
        h.retain(|e| e.id != id);
        settings::save_history(&h);
    }
    snapshot(&st)
}

#[tauri::command(async)]
fn clear_history(st: State<'_, Shared>) -> Snapshot {
    {
        let mut h = st.history.lock().unwrap();
        h.clear();
        settings::save_history(&h);
    }
    snapshot(&st)
}

/// Lädt ein Modell aus dem Katalog. Ist danach noch keines aktiv, wird es gleich
/// ausgewählt und geladen: auf einem frischen Rechner reicht ein Klick.
fn begin_download(st: &Shared, id: &str) -> Result<(), String> {
    let st2 = st.clone();
    download::start(
        id,
        |p| {
            let _ = Ui.emit("download", p);
        },
        move |path| {
            let nothing_loaded = st2.engine_info.lock().unwrap().is_none();
            if nothing_loaded {
                {
                    let mut s = st2.settings.lock().unwrap();
                    s.model = path.to_string_lossy().to_string();
                    settings::save_settings(&s);
                }
                reload_model(&Ui, &st2);
            }
            let _ = Ui.emit("refresh", ());
        },
    )
}

#[tauri::command(async)]
fn download_model(st: State<'_, Shared>, id: String) -> Result<Snapshot, String> {
    begin_download(&st, &id)?;
    Ok(snapshot(&st))
}

#[tauri::command(async)]
fn cancel_download(st: State<'_, Shared>) -> Snapshot {
    download::cancel();
    // Kurz warten, bis der Download-Thread den Abbruch verbucht hat.
    std::thread::sleep(Duration::from_millis(600));
    snapshot(&st)
}

#[tauri::command]
fn open_models_dir() {
    let opener = if cfg!(windows) { "explorer" } else { "open" };
    let _ = std::process::Command::new(opener).arg(settings::models_dir()).spawn();
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

#[cfg_attr(not(windows), allow(unused_variables))]
fn setup_overlay(app: &AppHandle, st: &App) {
    let Some(ov) = app.get_webview_window("overlay") else { return };
    if let Ok(Some(mon)) = ov.primary_monitor() {
        let scale = mon.scale_factor();
        let (w, h) = ((560.0 * scale) as i32, (168.0 * scale) as i32);
        let x = mon.position().x + (mon.size().width as i32 - w) / 2;
        let y = mon.position().y + mon.size().height as i32 - h - (72.0 * scale) as i32;
        let _ = ov.set_position(tauri::PhysicalPosition::new(x, y));
    }
    let _ = ov.set_ignore_cursor_events(true);
    #[cfg(windows)]
    if let Ok(hwnd) = ov.hwnd() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        };
        let raw = hwnd.0 as isize;
        unsafe {
            // Nie den Fokus nehmen: der Text soll ja ins gerade aktive Fenster.
            let ex = GetWindowLongPtrW(raw as _, GWL_EXSTYLE);
            SetWindowLongPtrW(raw as _, GWL_EXSTYLE, ex | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize);
        }
        st.overlay_hwnd.store(raw, Ordering::Relaxed);
    }
}

fn main() {
    // Diktieren soll auch flüssig bleiben, wenn nebenher ein Build alle Kerne belegt.
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, ABOVE_NORMAL_PRIORITY_CLASS};
        SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS);
    }

    // Jede Panik landet im Log, egal in welchem Thread. Ohne das stirbt ein
    // Hintergrund-Thread lautlos und die App wirkt nur "tot".
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        settings::log(&format!("PANIK im Thread „{}“: {info}", thread.name().unwrap_or("?")));
    }));
    settings::log(&format!("Start, Version {}", env!("CARGO_PKG_VERSION")));

    let (ctl_tx, ctl_rx) = channel::<Ctl>();
    let state: Shared = Arc::new(App {
        settings: Mutex::new(settings::load_settings()),
        history: Mutex::new(settings::load_history()),
        status: Mutex::new(Status { state: "loading".into(), message: String::new() }),
        engine_info: Mutex::new(None),
        engine_error: Mutex::new(None),
        devices: Mutex::new(Vec::new()),
        capture_target: Mutex::new(String::new()),
        engine: Engine::start(),
        audio: Audio::start_thread(),
        ctl: Mutex::new(ctl_tx.clone()),
        phase: AtomicU8::new(IDLE),
        rec_id: AtomicU32::new(0),
        loading: AtomicU8::new(0),
        overlay_hwnd: AtomicIsize::new(0),
    });
    let hidden_arg = std::env::args().any(|a| a == "--hidden");

    // Alles, was zum Diktieren nötig ist, startet sofort: Kürzel, Steuerung, Modell.
    {
        let s = state.settings.lock().unwrap();
        hotkey::set_bindings(&s.hotkey, &s.cancel_key);
    }
    let sandbox = settings::sandbox().is_some();
    let (hk_tx, hk_rx) = channel::<hotkey::Event>();
    if sandbox {
        settings::log("Probelauf mit eigenem Datenordner: kein Tastenkürzel, normale App bleibt unberührt");
        // Der Sender muss leben bleiben, sonst endet die Weiterleitung sofort.
        std::mem::forget(hk_tx);
    } else {
        hotkey::start(hk_tx);
    }
    let forward = ctl_tx.clone();
    std::thread::spawn(move || {
        for e in hk_rx {
            let _ = forward.send(Ctl::Hotkey(e));
        }
    });

    let s2 = state.clone();
    std::thread::Builder::new()
        .name("controller".into())
        .spawn(move || controller(Ui, s2, ctl_rx))
        .expect("controller thread");

    // Pegel an die Oberfläche
    let s3 = state.clone();
    std::thread::spawn(move || {
        let mut tick = 0u32;
        let test_wav = std::env::var("FREESPEECH_TEST_WAV").ok().and_then(|p| read_test_wav(&p));
        loop {
            std::thread::sleep(Duration::from_millis(33));
            let phase = s3.phase.load(Ordering::Relaxed);
            if phase == RECORDING {
                let mut level = s3.audio.level();
                // Nur zum Prüfen: Pegel aus der Test-WAV an der Stelle, die gerade "läuft".
                if let (Some(t), Some(b)) = (&test_wav, *TEST_BEGUN.lock().unwrap()) {
                    let at = (b.elapsed().as_millis() as usize * 16).min(t.len());
                    level = t[at.saturating_sub(530)..at].iter().fold(0.0f32, |m, v| m.max(v.abs()));
                }
                let _ = Ui.emit("level", level);
            }
            // Während Aufnahme und Schreiben die Anzeige etwa jede Sekunde wieder nach
            // oben holen, falls sich ein anderes Fenster davorgeschoben hat.
            tick = tick.wrapping_add(1);
            if phase != IDLE && tick % 30 == 0 && s3.settings.lock().unwrap().overlay {
                show_overlay(&s3, true);
                // Ist die Aufnahme genau in diesem Moment zu Ende gegangen, nicht
                // versehentlich ein leeres Fenster stehen lassen.
                if s3.phase.load(Ordering::Relaxed) == IDLE && s3.status.lock().unwrap().state == "idle" {
                    show_overlay(&s3, false);
                }
            }
        }
    });

    // Nur zum Prüfen: FREESPEECH_TEST_AUTORUN=<Sekunden> nimmt nach dem Start
    // einmal so lange auf, ganz ohne Tastatur.
    if let Some(secs) = std::env::var("FREESPEECH_TEST_AUTORUN").ok().and_then(|v| v.parse::<f32>().ok()) {
        let auto = ctl_tx.clone();
        // FREESPEECH_TEST_DELAY=<Millisekunden> verschiebt den Beginn (Standard 5 s).
        let delay = std::env::var("FREESPEECH_TEST_DELAY").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(5000);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(delay));
            let _ = auto.send(Ctl::UiToggle);
            std::thread::sleep(Duration::from_secs_f32(secs));
            let _ = auto.send(Ctl::UiToggle);
        });
    }
    if std::env::var("FREESPEECH_TEST_WAV").is_ok() {
        let s5 = state.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(1000));
            if s5.phase.load(Ordering::Relaxed) != IDLE {
                let v: Vec<u32> = settings::STAGE.iter().map(|a| a.load(Ordering::Relaxed)).collect();
                settings::log(&format!(
                    "Wächter: Phase {}, Vorschau {}, Steuerung {}, Ton {}, Engine {}, Hook {}",
                    s5.phase.load(Ordering::Relaxed), v[0], v[1], v[2], v[3], v[4]
                ));
            }
        });
    }

    // Rechengeräte einmal lesen, dann das Modell laden
    let s4 = state.clone();
    std::thread::spawn(move || {
        *s4.devices.lock().unwrap() = s4.engine.devices();
        reload_model(&Ui, &s4);
    });

    // Nur zum Prüfen: FREESPEECH_TEST_DOWNLOAD=<Katalog-Id> lädt gleich nach dem Start.
    if let Ok(id) = std::env::var("FREESPEECH_TEST_DOWNLOAD") {
        let s6 = state.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            if let Err(e) = begin_download(&s6, &id) {
                settings::log(&format!("Test-Download nicht gestartet: {e}"));
            }
        });
    }

    let mut builder = tauri::Builder::default();
    if !sandbox {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)));
    }
    builder
        .manage(state.clone())
        .invoke_handler(tauri::generate_handler![
            get_state,
            save_settings,
            capture_hotkey,
            cancel_capture,
            toggle_record,
            copy_text,
            delete_history,
            clear_history,
            open_models_dir,
            download_model,
            cancel_download,
            quit_app
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            let st = state.clone();

            // Infobereich
            let open = MenuItem::with_id(app, "open", "FreeSpeech öffnen", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Beenden", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(tray_image(IDLE))
                .tooltip("FreeSpeech")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            setup_overlay(&handle, &st);

            // Ab jetzt gibt es eine Oberfläche: den Zustand nachreichen, der schon
            // vorher entstanden ist (z. B. eine laufende Aufnahme).
            let _ = APP.set(handle.clone());
            let now = st.status.lock().unwrap().clone();
            set_status(&Ui, &st, &now.state, &now.message);
            let _ = Ui.emit("refresh", ());
            settings::log("Oberfläche bereit");

            let start_hidden = hidden_arg || st.settings.lock().unwrap().start_hidden;
            if !start_hidden {
                show_main(&handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("FreeSpeech konnte nicht starten")
        .run(|_app, event| match event {
            tauri::RunEvent::ExitRequested { code, .. } => {
                settings::log(&format!("Beenden angefordert (Code {code:?})"));
            }
            tauri::RunEvent::Exit => settings::log("Beendet"),
            _ => {}
        });
}
