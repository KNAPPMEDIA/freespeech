//! Misst Ladezeit und Tempo je Modell und Rechengeraet, im Stapel und im Streaming.
//! Aufruf: fs-bench <wav 16 kHz mono> <modell.gguf>... [--dev=cpu|vulkan] [--threads=N]
//! Die Engine-DLLs muessen neben der EXE oder im PATH liegen.
use std::time::Instant;
use transcribe_cpp::{
    devices, init_backends_default, Model, ModelOptions, ParakeetBufferedStreamOptions, RunOptions, SessionOptions,
    StreamExtension, StreamOptions,
};

fn load_wav(path: &str) -> Vec<f32> {
    let mut r = hound::WavReader::open(path).expect("wav");
    let s = r.spec();
    assert_eq!(s.sample_rate, 16000, "bench erwartet 16 kHz");
    let max = (1i64 << (s.bits_per_sample - 1)) as f32;
    let raw: Vec<f32> = r.samples::<i32>().map(|x| x.unwrap() as f32 / max).collect();
    let ch = s.channels as usize;
    raw.chunks(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect()
}

fn short(s: &str) -> String {
    s.trim().chars().take(100).collect()
}

fn main() {
    let all: Vec<String> = std::env::args().skip(1).collect();
    let only = all.iter().find_map(|a| a.strip_prefix("--dev=").map(str::to_string)).unwrap_or("cpu".into());
    let threads: i32 = all.iter().find_map(|a| a.strip_prefix("--threads=").and_then(|x| x.parse().ok())).unwrap_or(0);
    let args: Vec<&String> = all.iter().filter(|a| !a.starts_with("--")).collect();
    let pcm = load_wav(args[0]);
    let secs = pcm.len() as f32 / 16000.0;
    init_backends_default().expect("init_backends_default");
    for m in &args[1..] {
        let name = std::path::Path::new(m.as_str()).file_name().unwrap().to_string_lossy().to_string();
        let Some(dev) = devices().into_iter().find(|d| d.kind == only) else { continue };
        let t = Instant::now();
        let model = match Model::load_with(m, &ModelOptions { device: Some(dev), ..Default::default() }) {
            Ok(x) => x,
            Err(e) => { println!("{name}: LADEFEHLER {e}"); continue; }
        };
        let load = t.elapsed().as_secs_f32();
        let caps = model.capabilities().unwrap();
        let lang = caps.languages.iter().find(|l| l.as_str() == "de" || l.starts_with("de-")).cloned();
        let opts = RunOptions { language: lang.clone(), ..Default::default() };
        let mut sess = model.session_with(&SessionOptions { n_threads: threads, ..Default::default() }).unwrap();
        println!("== {name}: geladen in {load:.2}s, arch={}, streaming={}, sprache={:?}", model.arch(), caps.supports_streaming, lang);
        for pass in 0..2 {
            let t = Instant::now();
            match sess.run(&pcm, &opts) {
                Ok(tr) => { let el = t.elapsed().as_secs_f32(); println!("   stapel {pass}: {el:.2}s = {:.1}x | {}", secs / el, short(&tr.text)); }
                Err(e) => println!("   stapel {pass}: FEHLER {e}"),
            }
        }
        let variants: Vec<(&str, StreamOptions)> = vec![
            ("stream standard", StreamOptions::default()),
            ("stream gepuffert", StreamOptions { family: Some(StreamExtension::ParakeetBuffered(ParakeetBufferedStreamOptions::default())), ..Default::default() }),
        ];
        for (label, so) in variants {
            let t = Instant::now();
            let mut st = match sess.stream(&opts, &so) {
                Ok(s) => s,
                Err(e) => { println!("   {label}: nicht moeglich ({e})"); continue; }
            };
            let (mut updates, mut first, mut worst) = (0, None, 0.0f32);
            let mut failed = None;
            for (i, frame) in pcm.chunks(1600).enumerate() {
                let tf = Instant::now();
                match st.feed(frame) {
                    Ok(u) => {
                        worst = worst.max(tf.elapsed().as_secs_f32());
                        if u.committed_changed || u.tentative_changed {
                            updates += 1;
                            if first.is_none() { first = Some((i + 1) as f32 * 0.1); }
                        }
                    }
                    Err(e) => { failed = Some(e.to_string()); break; }
                }
            }
            if let Some(e) = failed { println!("   {label}: FEHLER beim Fuettern ({e})"); continue; }
            let tfin = Instant::now();
            let fin = st.finalize();
            let el = t.elapsed().as_secs_f32();
            println!("   {label}: {el:.2}s = {:.1}x, {updates} Aktualisierungen, erste nach {:?}s Ton, laengster Schritt {:.0} ms, Abschluss {:.0} ms, ok={} | {}",
                secs / el, first, worst * 1000.0, tfin.elapsed().as_secs_f32() * 1000.0, fin.is_ok(), short(&st.text().full));
        }
    }
}
