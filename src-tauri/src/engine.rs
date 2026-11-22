//! Die Sprach-Engine (transcribe.cpp) läuft in einem eigenen Thread.
//! Laden und Transkribieren kommen über eine Warteschlange, so bleibt die
//! Reihenfolge erhalten und nichts blockiert Oberfläche oder Tastatur-Hook.

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::Instant;
use transcribe_cpp::{
    devices, init_backends_default, CancelToken, Model, ModelOptions, RunOptions, Session, SessionOptions,
};

#[derive(Debug, Clone, Serialize)]
pub struct EngineInfo {
    pub model_path: String,
    pub arch: String,
    pub variant: String,
    pub backend: String,
    pub device: String,
    pub languages: Vec<String>,
    pub language_detect: bool,
    pub load_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    pub name: String,
    pub kind: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub text: String,
    pub took_ms: u64,
}

enum Cmd {
    Load {
        path: String,
        device: String,
        threads: i32,
        reply: Sender<Result<EngineInfo, String>>,
    },
    Run {
        pcm: Vec<f32>,
        language: String,
        vocabulary: Vec<String>,
        reply: Sender<Result<RunResult, String>>,
    },
    /// Zwischenstand während der Aufnahme. Wird übersprungen oder abgebrochen,
    /// sobald die Aufnahme endet, damit der endgültige Lauf nicht warten muss.
    Preview {
        pcm: Vec<f32>,
        language: String,
        vocabulary: Vec<String>,
        reply: Sender<Result<String, String>>,
    },
    Devices {
        reply: Sender<Vec<DeviceInfo>>,
    },
}

#[derive(Clone)]
pub struct Engine {
    tx: Sender<Cmd>,
    cancel: CancelToken,
    preview_on: Arc<AtomicBool>,
}

struct Loaded {
    _model: Model,
    session: Session,
    info: EngineInfo,
}

impl Engine {
    pub fn start() -> Engine {
        let (tx, rx) = channel::<Cmd>();
        let cancel = CancelToken::new();
        let preview_on = Arc::new(AtomicBool::new(false));
        let (cancel_t, preview_t) = (cancel.clone(), preview_on.clone());
        std::thread::Builder::new()
            .name("engine".into())
            .spawn(move || {
                let (cancel, preview_on) = (cancel_t, preview_t);
                // Rechenmodule (ggml-cpu-*.dll, ggml-vulkan.dll) liegen neben transcribe.dll.
                let init = init_backends_default().map_err(|e| e.to_string());
                let mut loaded: Option<Loaded> = None;
                for cmd in rx {
                    match cmd {
                        Cmd::Devices { reply } => {
                            let list = devices()
                                .into_iter()
                                .map(|d| DeviceInfo { name: d.name, kind: d.kind, description: d.description.trim().to_string() })
                                .collect();
                            let _ = reply.send(list);
                        }
                        Cmd::Load { path, device, threads, reply } => {
                            loaded = None; // altes Modell zuerst freigeben
                            let res = init.clone().and_then(|_| load(&path, &device, threads, &cancel));
                            let _ = reply.send(match res {
                                Ok(l) => {
                                    let info = l.info.clone();
                                    loaded = Some(l);
                                    Ok(info)
                                }
                                Err(e) => Err(e),
                            });
                        }
                        Cmd::Run { pcm, language, vocabulary, reply } => {
                            cancel.reset();
                            let res = match loaded.as_mut() {
                                None => Err("Kein Modell geladen. Wähle unter Modelle eines aus.".to_string()),
                                Some(l) => run(l, &pcm, &language, &vocabulary),
                            };
                            let _ = reply.send(res);
                        }
                        Cmd::Preview { pcm, language, vocabulary, reply } => {
                            crate::settings::stage(3, 1);
                            let res = match loaded.as_mut() {
                                Some(l) if preview_on.load(Ordering::SeqCst) => {
                                    let r = run(l, &pcm, &language, &vocabulary).map(|r| r.text);
                                    // Ein abgebrochener Lauf liefert nur einen Teil: verwerfen.
                                    if cancel.is_cancelled() || !preview_on.load(Ordering::SeqCst) {
                                        Err("abgebrochen".to_string())
                                    } else {
                                        r
                                    }
                                }
                                _ => Err("keine Vorschau".to_string()),
                            };
                            crate::settings::stage(3, 0);
                            let _ = reply.send(res);
                        }
                    }
                }
            })
            .expect("engine thread");
        Engine { tx, cancel, preview_on }
    }

    /// Aufnahme beginnt: Zwischenstände sind ab jetzt erlaubt.
    pub fn begin_preview(&self) {
        self.cancel.reset();
        self.preview_on.store(true, Ordering::SeqCst);
    }

    /// Aufnahme endet: laufenden Zwischenstand abbrechen, wartende überspringen.
    pub fn end_preview(&self) {
        self.preview_on.store(false, Ordering::SeqCst);
        self.cancel.cancel();
    }

    pub fn preview(&self, pcm: Vec<f32>, language: &str, vocabulary: Vec<String>) -> Result<String, String> {
        let (reply, rx) = channel();
        self.tx
            .send(Cmd::Preview { pcm, language: language.into(), vocabulary, reply })
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }

    pub fn load(&self, path: &str, device: &str, threads: i32) -> Result<EngineInfo, String> {
        let (reply, rx) = channel();
        self.tx
            .send(Cmd::Load { path: path.into(), device: device.into(), threads, reply })
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }

    pub fn run(&self, pcm: Vec<f32>, language: &str, vocabulary: Vec<String>) -> Result<RunResult, String> {
        let (reply, rx) = channel();
        self.tx
            .send(Cmd::Run { pcm, language: language.into(), vocabulary, reply })
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }

    pub fn devices(&self) -> Vec<DeviceInfo> {
        let (reply, rx) = channel();
        if self.tx.send(Cmd::Devices { reply }).is_err() {
            return Vec::new();
        }
        rx.recv().unwrap_or_default()
    }
}

fn load(path: &str, device: &str, threads: i32, cancel: &CancelToken) -> Result<Loaded, String> {
    let t = Instant::now();
    let devs = devices();
    let want_gpu = device == "gpu";
    let dev = devs
        .iter()
        .find(|d| if want_gpu { d.kind != "cpu" } else { d.kind == "cpu" })
        .or_else(|| devs.iter().find(|d| d.kind == "cpu"))
        .cloned()
        .ok_or_else(|| "Kein Rechengerät gefunden (Engine-DLLs fehlen neben der EXE?).".to_string())?;

    let model = Model::load_with(path, &ModelOptions { device: Some(dev), ..Default::default() })
        .map_err(|e| format!("Modell lässt sich nicht laden: {e}"))?;
    let caps = model.capabilities().map_err(|e| e.to_string())?;
    let mut session = model
        .session_with(&SessionOptions { n_threads: threads, ..Default::default() })
        .map_err(|e| format!("Sitzung lässt sich nicht öffnen: {e}"))?;

    cancel.reset();
    session.set_cancel_token(cancel);

    // Aufwärmen: der erste Lauf nach dem Laden ist deutlich langsamer als alle
    // weiteren. Eine Sekunde Stille nimmt das vorweg, damit das erste Diktat zählt.
    let silence = vec![0.0f32; 16000];
    let _ = session.run(&silence, &RunOptions { language: pick_language(&caps.languages, "de"), ..Default::default() });

    let info = EngineInfo {
        model_path: path.to_string(),
        arch: model.arch(),
        variant: model.variant(),
        backend: model.backend(),
        device: model.device().map(|d| d.description.trim().to_string()).unwrap_or_default(),
        languages: caps.languages.clone(),
        language_detect: caps.supports_language_detect,
        load_ms: t.elapsed().as_millis() as u64,
    };
    Ok(Loaded { _model: model, session, info })
}

/// Modelle erwarten teils "de", teils "de-DE". Nimmt den passenden Eintrag aus der
/// Liste des Modells; `None` heißt automatisch erkennen.
fn pick_language(supported: &[String], wanted: &str) -> Option<String> {
    if wanted.is_empty() || wanted == "auto" {
        return None;
    }
    let w = wanted.to_lowercase();
    supported
        .iter()
        .find(|l| l.to_lowercase() == w)
        .or_else(|| supported.iter().find(|l| l.to_lowercase().starts_with(&format!("{w}-"))))
        .cloned()
}

fn run(l: &mut Loaded, pcm: &[f32], language: &str, vocabulary: &[String]) -> Result<RunResult, String> {
    let t = Instant::now();
    let mut opts = RunOptions {
        language: pick_language(&l.info.languages, language),
        vocabulary: vocabulary.to_vec(),
        ..Default::default()
    };
    let res = match l.session.run(pcm, &opts) {
        Ok(r) => Ok(r),
        // Manche Modelle lehnen Sprachvorgabe oder Wortliste ab: dann ohne beides.
        Err(_) if opts.language.is_some() || !opts.vocabulary.is_empty() => {
            opts.language = None;
            opts.vocabulary.clear();
            l.session.run(pcm, &opts)
        }
        Err(e) => Err(e),
    };
    match res {
        Ok(tr) => Ok(RunResult { text: tr.text.trim().to_string(), took_ms: t.elapsed().as_millis() as u64 }),
        Err(e) => Err(format!("Transkription fehlgeschlagen: {e}")),
    }
}
