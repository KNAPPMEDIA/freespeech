//! Sprachmodelle von HuggingFace laden. Auf einem frischen Rechner gibt es keins,
//! und mit ausliefern müssten wir 700 MB. Geladen wird mit dem curl, das Windows
//! mitbringt: das kann abgebrochene Downloads fortsetzen und braucht keine
//! zusätzliche Bibliothek.

use serde::Serialize;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct CatalogItem {
    pub id: &'static str,
    pub name: &'static str,
    pub note: &'static str,
    pub repo: &'static str,
    pub file: &'static str,
    pub bytes: u64,
    pub license: &'static str,
}

/// Nur Modelle, deren Lizenz die Weitergabe erlaubt (CC BY 4.0).
pub const CATALOG: &[CatalogItem] = &[
    CatalogItem {
        id: "primeline",
        name: "Parakeet Primeline",
        note: "Deutsch. Beste Wahl für deutsche Diktate.",
        repo: "handy-computer/parakeet-primeline-gguf",
        file: "parakeet-primeline-Q8_0.gguf",
        bytes: 739_508_640,
        license: "CC BY 4.0",
    },
    CatalogItem {
        id: "canary",
        name: "Canary 180M Flash",
        note: "Deutsch, Englisch, Französisch, Spanisch. Klein und schnell.",
        repo: "handy-computer/canary-180m-flash-gguf",
        file: "canary-180m-flash-Q8_0.gguf",
        bytes: 218_447_648,
        license: "CC BY 4.0",
    },
];

#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub note: String,
    pub size_mb: u64,
    pub license: String,
    pub source: String,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub id: String,
    pub done_mb: u64,
    pub total_mb: u64,
    /// "running", "done", "error" oder "cancelled"
    pub state: String,
    pub message: String,
}

struct Job {
    cancel: Arc<AtomicBool>,
    progress: Progress,
}

static JOB: Mutex<Option<Job>> = Mutex::new(None);

const MB: u64 = 1024 * 1024;

pub fn catalog() -> Vec<CatalogEntry> {
    let have = crate::settings::scan_models();
    CATALOG
        .iter()
        .map(|c| CatalogEntry {
            id: c.id.into(),
            name: c.name.into(),
            note: c.note.into(),
            size_mb: c.bytes / MB,
            license: c.license.into(),
            source: c.repo.into(),
            installed: have.iter().any(|m| m.file.eq_ignore_ascii_case(c.file)),
        })
        .collect()
}

/// Laufender Download oder der letzte Fehler, für das Fenster beim Öffnen.
pub fn current() -> Option<Progress> {
    JOB.lock().unwrap().as_ref().map(|j| j.progress.clone())
}

pub fn cancel() {
    let mut job = JOB.lock().unwrap();
    match job.as_ref() {
        Some(j) if j.progress.state == "running" => j.cancel.store(true, Ordering::SeqCst),
        // Ein stehengebliebener Fehler wird durch Abbrechen einfach weggeräumt.
        Some(_) => *job = None,
        None => {}
    }
}

#[cfg(not(windows))]
fn curl_exe() -> PathBuf {
    PathBuf::from("/usr/bin/curl")
}

#[cfg(windows)]
fn curl_exe() -> PathBuf {
    let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let p = sys.join("System32").join("curl.exe");
    if p.is_file() {
        p
    } else {
        PathBuf::from("curl.exe")
    }
}

/// Startet den Download. `on_progress` wird etwa zweimal pro Sekunde gerufen und
/// am Ende einmal mit "done", "error" oder "cancelled". Bei Erfolg liefert
/// `on_done` den Pfad der fertigen Datei.
pub fn start(
    id: &str,
    on_progress: impl Fn(Progress) + Send + 'static,
    on_done: impl FnOnce(PathBuf) + Send + 'static,
) -> Result<(), String> {
    let item = CATALOG.iter().find(|c| c.id == id).ok_or_else(|| "Dieses Modell kennt FreeSpeech nicht.".to_string())?;
    let cancel = Arc::new(AtomicBool::new(false));
    let total_mb = item.bytes / MB;
    {
        let mut job = JOB.lock().unwrap();
        if job.as_ref().map(|j| j.progress.state == "running").unwrap_or(false) {
            return Err("Es lädt schon ein Modell. Warte, bis es fertig ist, oder brich es ab.".into());
        }
        *job = Some(Job {
            cancel: cancel.clone(),
            progress: Progress { id: id.into(), done_mb: 0, total_mb, state: "running".into(), message: String::new() },
        });
    }

    let target = crate::settings::models_dir().join(item.file);
    let part = crate::settings::models_dir().join(format!("{}.part", item.file));
    let url = format!("https://huggingface.co/{}/resolve/main/{}", item.repo, item.file);
    let (id, bytes) = (id.to_string(), item.bytes);

    std::thread::Builder::new()
        .name("download".into())
        .spawn(move || {
            let report = |state: &str, done: u64, message: String| {
                let p = Progress { id: id.clone(), done_mb: done / MB, total_mb, state: state.into(), message };
                let mut job = JOB.lock().unwrap();
                match state {
                    // Fehler bleiben stehen, damit das Fenster sie auch später noch zeigt.
                    "running" | "error" => {
                        if let Some(j) = job.as_mut() {
                            j.progress = p.clone();
                        }
                    }
                    _ => *job = None,
                }
                drop(job);
                on_progress(p);
            };

            crate::settings::log(&format!("Download gestartet: {url}"));
            let mut cmd = Command::new(curl_exe());
            cmd.args(["--location", "--fail", "--silent", "--show-error", "--continue-at", "-"])
                .args(["--retry", "3", "--connect-timeout", "20", "--output"])
                .arg(&part)
                .arg(&url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000); // kein Konsolenfenster
            let child = cmd.spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => {
                    crate::settings::log(&format!("Download: curl nicht startbar: {e}"));
                    report("error", 0, "Der Download lässt sich nicht starten (curl.exe fehlt).".into());
                    return;
                }
            };

            let size = || std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            let status = loop {
                if cancel.load(Ordering::SeqCst) {
                    let _ = child.kill();
                    let _ = child.wait();
                    crate::settings::log("Download abgebrochen");
                    // Die Teildatei bleibt liegen: beim nächsten Versuch geht es dort weiter.
                    report("cancelled", size(), String::new());
                    return;
                }
                match child.try_wait() {
                    Ok(Some(s)) => break s,
                    Ok(None) => {}
                    Err(e) => {
                        report("error", size(), format!("Download abgebrochen: {e}"));
                        return;
                    }
                }
                report("running", size(), String::new());
                std::thread::sleep(Duration::from_millis(450));
            };

            let got = size();
            if status.success() && got == bytes {
                let _ = std::fs::remove_file(&target);
                match std::fs::rename(&part, &target) {
                    Ok(()) => {
                        crate::settings::log(&format!("Download fertig: {} ({} MB)", target.display(), got / MB));
                        report("done", got, String::new());
                        on_done(target);
                    }
                    Err(e) => report("error", got, format!("Die geladene Datei lässt sich nicht ablegen: {e}")),
                }
                return;
            }

            let mut detail = String::new();
            if let Some(mut err) = child.stderr.take() {
                use std::io::Read;
                let _ = err.read_to_string(&mut detail);
            }
            let detail = detail.trim().lines().last().unwrap_or("").to_string();
            crate::settings::log(&format!("Download fehlgeschlagen: Status {status}, {got} von {bytes} Bytes, {detail}"));
            let message = if status.success() {
                // Größe passt nicht: unbrauchbare Datei nicht als Modell anbieten.
                let _ = std::fs::remove_file(&part);
                "Die geladene Datei ist unvollständig. Versuch es nochmal.".to_string()
            } else {
                "Der Download ist fehlgeschlagen. Prüfe die Internetverbindung und versuch es nochmal.".to_string()
            };
            report("error", got, message);
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}
