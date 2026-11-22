use std::{env, fs, path::PathBuf};

fn main() {
    // Engine-DLLs neben die EXE legen: transcribe.dll wird beim Start aufgeloest,
    // die ggml-Module laedt die Engine aus demselben Ordner nach.
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_dir = out.ancestors().nth(3).unwrap().to_path_buf();
    let engine = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../engine");
    println!("cargo:rerun-if-changed={}", engine.display());
    let windows = env::var("CARGO_CFG_TARGET_OS").map(|t| t == "windows").unwrap_or(false);
    if !windows {
        // Auf dem Mac wird die Engine aus dem Quelltext gebaut und fest eingebunden.
    } else if let Ok(entries) = fs::read_dir(&engine) {
        for e in entries.flatten() {
            let dst = target_dir.join(e.file_name());
            let newer = match (e.metadata(), fs::metadata(&dst)) {
                (Ok(a), Ok(b)) => a.len() != b.len(),
                _ => true,
            };
            if newer {
                let _ = fs::copy(e.path(), &dst);
            }
        }
    }
    tauri_build::build();
}
