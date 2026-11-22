//! Einstellungen, Verlauf und Modell-Suche. Alles liegt als JSON in %APPDATA%\FreeSpeech.

use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Replacement {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Voller Pfad zur .gguf-Datei. Leer = erstes gefundenes Modell.
    pub model: String,
    /// "cpu" oder "gpu".
    pub device: String,
    /// 0 = Engine entscheidet.
    pub threads: i32,
    /// "auto" oder Sprachkürzel wie "de".
    pub language: String,
    pub hotkey: String,
    pub cancel_key: String,
    /// "ptt" (halten) oder "toggle" (einmal drücken, nochmal drücken).
    pub mode: String,
    /// Leer = Standard-Mikrofon von Windows.
    pub microphone: String,
    /// "ctrl_v", "type" oder "none" (nur Verlauf).
    pub paste_method: String,
    pub restore_clipboard: bool,
    pub trailing_space: bool,
    pub sounds: bool,
    pub overlay: bool,
    /// Text schon während des Sprechens zeigen.
    pub live_preview: bool,
    pub history_limit: usize,
    pub autostart: bool,
    pub start_hidden: bool,
    pub replacements: Vec<Replacement>,
    pub vocabulary: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            model: String::new(),
            device: "cpu".into(),
            threads: 0,
            language: "de".into(),
            // Mac: Control + Option, weil Control + Command dort Systemkürzel auslöst.
            hotkey: if cfg!(target_os = "macos") { "ctrl_left+alt_left" } else { "ctrl_left+super_left" }.into(),
            cancel_key: "escape".into(),
            mode: "ptt".into(),
            microphone: String::new(),
            paste_method: "ctrl_v".into(),
            restore_clipboard: true,
            trailing_space: true,
            sounds: false,
            overlay: true,
            live_preview: true,
            history_limit: 50,
            autostart: false,
            start_hidden: false,
            replacements: Vec::new(),
            vocabulary: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: u64,
    pub text: String,
    pub audio_ms: u64,
    pub took_ms: u64,
    pub model: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelFile {
    pub path: String,
    pub name: String,
    pub file: String,
    pub size_mb: u64,
    pub source: String,
}

/// Probelauf in einem eigenen Datenordner (Umgebungsvariable FREESPEECH_DATA):
/// eigene Einstellungen, keine Modelle aus dem HuggingFace-Cache, kein Tastenkürzel,
/// und die normal laufende App bleibt unberührt. So lässt sich der erste Start auf
/// einem "frischen Rechner" prüfen.
pub fn sandbox() -> Option<PathBuf> {
    std::env::var_os("FREESPEECH_DATA").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

pub fn data_dir() -> PathBuf {
    let dir = sandbox().unwrap_or_else(|| {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("FreeSpeech")
    });
    let _ = fs::create_dir_all(dir.join("models"));
    dir
}

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

pub fn load_settings() -> Settings {
    let path = data_dir().join("settings.json");
    match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) if sandbox().is_some() => Settings::default(),
        Err(_) => first_run_settings(),
    }
}

pub fn save_settings(s: &Settings) {
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let _ = fs::write(data_dir().join("settings.json"), json);
    }
}

/// Beim ersten Start: Kürzel, Mikrofon und Sprache aus Handy übernehmen, falls vorhanden.
fn first_run_settings() -> Settings {
    let mut s = Settings::default();
    let handy = dirs::config_dir().map(|d| d.join("com.pais.handy").join("settings_store.json"));
    if let Some(Ok(raw)) = handy.map(fs::read_to_string) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            let h = &v["settings"];
            if let Some(b) = h["bindings"]["transcribe"]["current_binding"].as_str() {
                if crate::hotkey::parse(b).is_some() {
                    s.hotkey = b.to_string();
                }
            }
            if let Some(m) = h["selected_microphone"].as_str() {
                s.microphone = m.to_string();
            }
            if let Some(l) = h["selected_language"].as_str() {
                s.language = l.to_string();
            }
            if let Some(p) = h["push_to_talk"].as_bool() {
                s.mode = if p { "ptt".into() } else { "toggle".into() };
            }
        }
    }
    save_settings(&s);
    s
}

pub fn load_history() -> Vec<HistoryEntry> {
    fs::read_to_string(data_dir().join("history.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_history(h: &[HistoryEntry]) {
    if let Ok(json) = serde_json::to_string(h) {
        let _ = fs::write(data_dir().join("history.json"), json);
    }
}

/// Sucht .gguf-Sprachmodelle im eigenen Modellordner und im HuggingFace-Cache
/// (dort legt Handy seine Modelle ab).
pub fn scan_models() -> Vec<ModelFile> {
    let mut out: Vec<ModelFile> = Vec::new();
    let mut push = |path: PathBuf, source: &str| {
        let file = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let lower = file.to_lowercase();
        if !lower.ends_with(".gguf") || lower.contains("vad") {
            return;
        }
        // fs::metadata folgt Symlinks: im HF-Cache sind die Snapshots Links auf blobs.
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size < 20 * 1024 * 1024 {
            return;
        }
        let p = path.to_string_lossy().to_string();
        if out.iter().any(|m| m.file == file) {
            return;
        }
        out.push(ModelFile {
            name: pretty_name(&file),
            file,
            path: p,
            size_mb: size / (1024 * 1024),
            source: source.to_string(),
        });
    };

    if let Ok(rd) = fs::read_dir(models_dir()) {
        for e in rd.flatten() {
            push(e.path(), "FreeSpeech-Ordner");
        }
    }
    if let Some(home) = dirs::home_dir().filter(|_| sandbox().is_none()) {
        let hub = home.join(".cache").join("huggingface").join("hub");
        if let Ok(repos) = fs::read_dir(&hub) {
            for repo in repos.flatten() {
                let snaps = repo.path().join("snapshots");
                let Ok(snaps) = fs::read_dir(snaps) else { continue };
                for snap in snaps.flatten() {
                    let Ok(files) = fs::read_dir(snap.path()) else { continue };
                    for f in files.flatten() {
                        push(f.path(), "HuggingFace-Cache");
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn pretty_name(file: &str) -> String {
    let stem = file.trim_end_matches(".gguf");
    // Quantisierungs-Suffix wie -Q8_0 abtrennen
    let stem = match stem.rfind("-Q") {
        Some(i) if stem[i + 2..].chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) => &stem[..i],
        _ => stem,
    };
    stem.split(|c| c == '-' || c == '_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) if p.chars().all(|x| x.is_ascii_alphabetic()) => {
                    f.to_uppercase().collect::<String>() + c.as_str()
                }
                _ => p.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(target_os = "macos")]
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    fs_mac::system::set_autostart(enabled, &exe)
}

#[cfg(windows)]
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = hkcu
        .create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
        .map_err(|e| e.to_string())?;
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        run.set_value("FreeSpeech", &format!("\"{}\" --hidden", exe.display()))
            .map_err(|e| e.to_string())
    } else {
        let _ = run.delete_value("FreeSpeech");
        Ok(())
    }
}

/// Diagnose: wo steht welcher Thread gerade? 0 Vorschau, 1 Steuerung, 2 Ton, 3 Engine, 4 Hook.
pub static STAGE: [std::sync::atomic::AtomicU32; 5] = [
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
];

pub fn stage(who: usize, at: u32) {
    STAGE[who].store(at, std::sync::atomic::Ordering::Relaxed);
}

/// Schreibt eine Zeile nach %APPDATA%\FreeSpeech\freespeech.log (für die Fehlersuche).
pub fn log(msg: &str) {
    use std::io::Write;
    let path = data_dir().join("freespeech.log");
    // Bei über 1 MB neu anfangen, damit die Datei nicht endlos wächst.
    if fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        let _ = fs::remove_file(&path);
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{secs}] {msg}");
    }
}
