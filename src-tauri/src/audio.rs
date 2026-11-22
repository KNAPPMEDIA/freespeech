//! Mikrofon-Aufnahme über cpal (WASAPI). Der Stream lebt in einem eigenen Thread,
//! weil er nicht zwischen Threads wandern darf. Ergebnis ist immer 16 kHz mono.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

enum Cmd {
    Start { mic: String, reply: Sender<Result<String, String>> },
    Stop { reply: Sender<Vec<f32>> },
    Snapshot { max_secs: u32, reply: Sender<(Vec<f32>, bool)> },
    Cancel,
}

#[derive(Clone)]
pub struct Audio {
    tx: Sender<Cmd>,
    level: Arc<AtomicU32>,
}

impl Audio {
    pub fn start_thread() -> Audio {
        let (tx, rx) = channel::<Cmd>();
        let level = Arc::new(AtomicU32::new(0));
        let level_t = level.clone();
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                let mut active: Option<(cpal::Stream, Arc<Mutex<Vec<f32>>>, u32)> = None;
                for cmd in rx {
                    crate::settings::stage(2, 1);
                    match cmd {
                        Cmd::Start { mic, reply } => {
                            crate::settings::stage(2, 2);
                            active = None;
                            level_t.store(0, Ordering::Relaxed);
                            match open(&mic, level_t.clone()) {
                                Ok((stream, buf, rate, name)) => {
                                    active = Some((stream, buf, rate));
                                    let _ = reply.send(Ok(name));
                                }
                                Err(e) => {
                                    let _ = reply.send(Err(e));
                                }
                            }
                        }
                        Cmd::Stop { reply } => {
                            crate::settings::stage(2, 7);
                            let pcm = match active.take() {
                                Some((stream, buf, rate)) => {
                                    drop(stream);
                                    crate::settings::stage(2, 8);
                                    let raw = std::mem::take(&mut *buf.lock().unwrap());
                                    resample_to_16k(&raw, rate)
                                }
                                None => Vec::new(),
                            };
                            level_t.store(0, Ordering::Relaxed);
                            let _ = reply.send(pcm);
                        }
                        Cmd::Snapshot { max_secs, reply } => {
                            crate::settings::stage(2, 3);
                            let out = match &active {
                                Some((_, buf, rate)) => {
                                    let (raw, cut) = {
                                        let b = buf.lock().unwrap();
                                        crate::settings::stage(2, 4);
                                        let start = b.len().saturating_sub(*rate as usize * max_secs as usize);
                                        (b[start..].to_vec(), start > 0)
                                    };
                                    crate::settings::stage(2, 5);
                                    (resample_to_16k(&raw, *rate), cut)
                                }
                                None => (Vec::new(), false),
                            };
                            crate::settings::stage(2, 6);
                            let _ = reply.send(out);
                            crate::settings::stage(2, 0);
                        }
                        Cmd::Cancel => {
                            active = None;
                            level_t.store(0, Ordering::Relaxed);
                        }
                    }
                }
            })
            .expect("audio thread");
        Audio { tx, level }
    }

    /// Startet die Aufnahme. Gibt den Namen des benutzten Mikrofons zurück.
    pub fn start(&self, mic: &str) -> Result<String, String> {
        let (reply, rx) = channel();
        self.tx.send(Cmd::Start { mic: mic.into(), reply }).map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }

    pub fn stop(&self) -> Vec<f32> {
        let (reply, rx) = channel();
        if self.tx.send(Cmd::Stop { reply }).is_err() {
            return Vec::new();
        }
        rx.recv().unwrap_or_default()
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(Cmd::Cancel);
    }

    /// Bisher aufgenommener Ton (höchstens die letzten `max_secs` Sekunden) als
    /// 16 kHz mono, ohne die Aufnahme zu unterbrechen. `true` heißt: vorne gekürzt.
    pub fn snapshot(&self, max_secs: u32) -> (Vec<f32>, bool) {
        let (reply, rx) = channel();
        if self.tx.send(Cmd::Snapshot { max_secs, reply }).is_err() {
            return (Vec::new(), false);
        }
        rx.recv().unwrap_or_default()
    }

    /// Aktueller Spitzenpegel 0..1 (fällt beim Lesen ab, damit die Anzeige lebt).
    pub fn level(&self) -> f32 {
        let v = f32::from_bits(self.level.load(Ordering::Relaxed));
        self.level.store((v * 0.6).to_bits(), Ordering::Relaxed);
        v
    }
}

pub fn list_microphones() -> Vec<String> {
    let host = cpal::default_host();
    let mut names: Vec<String> = host
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    names.sort();
    names.dedup();
    names
}

type Opened = (cpal::Stream, Arc<Mutex<Vec<f32>>>, u32, String);

fn open(mic: &str, level: Arc<AtomicU32>) -> Result<Opened, String> {
    let host = cpal::default_host();
    let wanted = if mic.is_empty() {
        None
    } else {
        host.input_devices()
            .ok()
            .and_then(|mut it| it.find(|d| d.name().map(|n| n == mic).unwrap_or(false)))
    };
    let device = wanted
        .or_else(|| host.default_input_device())
        .ok_or_else(|| "Kein Mikrofon gefunden. Prüfe die Windows-Toneinstellungen.".to_string())?;
    let name = device.name().unwrap_or_else(|_| "Mikrofon".into());
    let cfg = device
        .default_input_config()
        .map_err(|e| format!("Mikrofon „{name}“ lässt sich nicht öffnen: {e}"))?;
    let rate = cfg.sample_rate().0;
    let ch = cfg.channels().max(1) as usize;
    let fmt = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let buf = Arc::new(Mutex::new(Vec::<f32>::with_capacity(rate as usize * 30)));

    // Kanäle mitteln, Pegel mitschreiben.
    fn sink(buf: &Mutex<Vec<f32>>, level: &AtomicU32, ch: usize, it: impl Iterator<Item = f32>) {
        let mut b = buf.lock().unwrap();
        let mut peak = f32::from_bits(level.load(Ordering::Relaxed));
        let mut acc = 0.0f32;
        let mut n = 0usize;
        for s in it {
            acc += s;
            n += 1;
            if n == ch {
                let m = acc / ch as f32;
                peak = peak.max(m.abs());
                b.push(m);
                acc = 0.0;
                n = 0;
            }
        }
        level.store(peak.min(1.0).to_bits(), Ordering::Relaxed);
    }

    let err_fn = |e| eprintln!("audio stream error: {e}");
    let (b, l) = (buf.clone(), level);
    let stream = match fmt {
        SampleFormat::F32 => device.build_input_stream(
            &config,
            move |d: &[f32], _: &_| sink(&b, &l, ch, d.iter().copied()),
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            &config,
            move |d: &[i16], _: &_| sink(&b, &l, ch, d.iter().map(|&s| s as f32 / 32768.0)),
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            &config,
            move |d: &[u16], _: &_| sink(&b, &l, ch, d.iter().map(|&s| (s as f32 - 32768.0) / 32768.0)),
            err_fn,
            None,
        ),
        SampleFormat::I32 => device.build_input_stream(
            &config,
            move |d: &[i32], _: &_| sink(&b, &l, ch, d.iter().map(|&s| s as f32 / 2147483648.0)),
            err_fn,
            None,
        ),
        other => return Err(format!("Mikrofon „{name}“ liefert ein Format, das FreeSpeech nicht kennt ({other:?}).")),
    }
    .map_err(|e| format!("Mikrofon „{name}“ lässt sich nicht starten: {e}"))?;
    stream.play().map_err(|e| format!("Mikrofon „{name}“ lässt sich nicht starten: {e}"))?;
    Ok((stream, buf, rate, name))
}

/// Auf 16 kHz bringen: erst Tiefpass (gegen Aliasing), dann linear abtasten.
pub fn resample_to_16k(input: &[f32], rate: u32) -> Vec<f32> {
    if rate == 16000 || input.is_empty() {
        return input.to_vec();
    }
    let ratio = rate as f64 / 16000.0;
    let filtered;
    let src: &[f32] = if ratio > 1.0 {
        filtered = lowpass(input, 0.5 / ratio * 0.92);
        &filtered
    } else {
        input
    };
    let n = (src.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let p = i as f64 * ratio;
        let i0 = p.floor() as usize;
        let f = (p - i0 as f64) as f32;
        let a = src[i0.min(src.len() - 1)];
        let b = src[(i0 + 1).min(src.len() - 1)];
        out.push(a + (b - a) * f);
    }
    out
}

/// Fenster-Sinc-Tiefpass (Hamming, 63 Koeffizienten). `fc` in Zyklen pro Abtastwert.
fn lowpass(x: &[f32], fc: f64) -> Vec<f32> {
    const TAPS: usize = 63;
    let mid = (TAPS / 2) as isize;
    let mut h = [0.0f32; TAPS];
    let mut sum = 0.0f64;
    for (i, hv) in h.iter_mut().enumerate() {
        let n = i as isize - mid;
        let sinc = if n == 0 {
            2.0 * fc
        } else {
            (2.0 * std::f64::consts::PI * fc * n as f64).sin() / (std::f64::consts::PI * n as f64)
        };
        let w = 0.54 - 0.46 * (2.0 * std::f64::consts::PI * i as f64 / (TAPS - 1) as f64).cos();
        let v = sinc * w;
        sum += v;
        *hv = v as f32;
    }
    for hv in h.iter_mut() {
        *hv = (*hv as f64 / sum) as f32;
    }
    let len = x.len() as isize;
    let mut out = vec![0.0f32; x.len()];
    for (i, o) in out.iter_mut().enumerate() {
        let mut acc = 0.0f32;
        for (k, hv) in h.iter().enumerate() {
            let j = i as isize + k as isize - mid;
            if j >= 0 && j < len {
                acc += x[j as usize] * hv;
            }
        }
        *o = acc;
    }
    out
}
